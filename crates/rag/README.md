# medical-rag

Retrieval-augmented generation (RAG) for FerriScribe's clinical AI chat.

This crate implements the full RAG pipeline: ingesting documents into a
searchable index, expanding and encoding user queries, retrieving candidate
passages through three complementary strategies (vector similarity, BM25
keyword search, and knowledge-graph traversal), fusing the result sets into a
single ranking, and re-ranking for diversity with Maximal Marginal Relevance.

> **Audience:** future-you returning to this crate after months away.

---

## How It Fits in the Workspace

```
medical-core  ──(types)──▶  medical-rag  ◀──(storage)──  medical-db
                                │   │
                    ┌───────────┘   └────────────┐
                    ▼                             ▼
              medical-agents                 src-tauri
          (RAG tool for AI chat)    (ingestion commands, direct queries)
```

| Relationship | Crate | What it provides / consumes |
|---|---|---|
| **Depends on** | `medical-core` | Shared types: `DocumentChunk`, `RagResult`, `ExpandedQuery`, `GraphEntity`, `GraphRelation`, `SearchSource`, error types |
| **Depends on** | `medical-db` | `Database` handle, `VectorsRepo` (SQLite FTS5 + embedding storage) |
| **Used by** | `medical-agents` | Calls the RAG pipeline as a tool during AI chat to ground responses in uploaded clinical documents |
| **Used by** | `src-tauri` | Tauri commands that trigger document ingestion and run ad-hoc RAG queries from the frontend |

---

## Module Map

| Module | Purpose |
|---|---|
| `embeddings` | `EmbeddingGenerator` — HTTP client that talks to a local Ollama `/api/embeddings` endpoint. Produces 768-dim vectors (default: `nomic-embed-text`). |
| `vector_store` | `VectorStore` — persists `DocumentChunk`s with their embeddings into SQLite; brute-force cosine similarity search. |
| `bm25` | `Bm25Search` — full-text keyword search via SQLite FTS5. Normalizes FTS5's negative rank to a positive `(0, 1]` score. |
| `graph_search` | `GraphSearch` — SQLite-backed knowledge graph (entities + relations). Retrieves matching entities and their neighbours. |
| `fusion` | `reciprocal_rank_fusion` and `weighted_fusion` — combine multiple ranked result sets into one. |
| `mmr` | `mmr_rerank` — Maximal Marginal Relevance re-ranking for diversity. Also houses `cosine_similarity` and `jaccard_similarity`. |
| `query_expander` | `QueryExpander` — expands medical abbreviations (HTN → hypertension) and adds synonyms (heart attack → myocardial infarction) to improve recall. |
| `ingestion` | `IngestionPipeline` — orchestrates chunking, embedding, vector-store persistence, and medical entity extraction in one call. Also exposes the standalone `chunk_text` helper. |

---

## Key Types

### `RagError` / `RagResult<T>`

The crate-level error enum covering search, embedding, ingestion, and database
failures. Every fallible function in this crate returns `Result<_, RagError>`
(or `AppResult` from `medical-core` when the error needs to cross crate
boundaries).

### `EmbeddingGenerator`

An HTTP-backed embedding client. Talks exclusively to **local Ollama**
(no hosted APIs — PHI constraint). Defaults:

- Host: `http://localhost:11434`
- Model: `nomic-embed-text`
- Dimensions: 768

Provides `embed(text)` for single texts and `embed_batch(texts)` with bounded
concurrency (8 parallel requests).

### `VectorStore`

Wraps `medical_db::vectors::VectorsRepo` for chunk storage and retrieval.
Search loads all embeddings from SQLite, computes cosine similarity against
each, filters by threshold, and returns the top-k.

### `Bm25Search`

Delegates to SQLite FTS5 via `VectorsRepo::search_fts`. FTS5 rank values are
negative (more negative = better match); the normalizer converts them to
positive scores: `score = -rank / (1 + -rank)`.

### `GraphSearch`

Stores medical entities (drugs, conditions) and typed relations in two SQLite
tables (`graph_entities`, `graph_relations`). Tables are created lazily on
first use via `OnceLock`. Search finds entities by case-insensitive substring
match on `name`, then follows outgoing and incoming relations to include
contextual neighbours.

### `IngestionPipeline`

Composes `EmbeddingGenerator`, `VectorStore`, and `GraphSearch` into a single
`ingest_text(doc_id, title, text)` call that:

1. Splits the document into overlapping word-level chunks (200 words, 50-word
   overlap by default).
2. Generates embeddings for all chunks in one batch.
3. Persists each chunk + embedding to the vector store.
4. Extracts medical entities (drug names, conditions) via keyword matching and
   stores them in the knowledge graph.

### `QueryExpander`

A pure-Rust dictionary expander with ~40 medical abbreviations and ~25 synonym
phrases. `expand(query)` returns an `ExpandedQuery` containing the original
text, a deduplicated list of expansion terms, and a `full_query` combining
both.

---

## How It Works

### Ingestion Flow

```
Document text
    │
    ▼
chunk_text(200 words, 50-word overlap)
    │
    ▼
EmbeddingGenerator::embed_batch(chunks)
    │
    ▼
VectorStore::store_chunk(chunk + embedding)  ←  SQLite
    │
    ▼
extract_medical_entities(text)
    │
    ▼
GraphSearch::store_entity(entity)  ←  SQLite graph_entities table
```

### Query Flow

```
User query
    │
    ├─▶ QueryExpander::expand(query) → full_query
    │
    ├─▶ EmbeddingGenerator::embed(full_query)
    │       │
    │       ▼
    │   VectorStore::search(embedding, top_k, threshold)
    │       → vector_results
    │
    ├─▶ Bm25Search::search(full_query, top_k)
    │       → bm25_results
    │
    ├─▶ GraphSearch::search(full_query, top_k)
    │       → graph_results
    │
    ▼
weighted_fusion(vector, bm25, graph, weights)
  OR reciprocal_rank_fusion(sets, k)
    │
    ▼
mmr_rerank(fused, lambda, top_k)
    │
    ▼
Final ranked results (source: SearchSource::Fused)
```

### Maximal Marginal Relevance (MMR)

After fusion, MMR iteratively selects results that balance relevance (original
score) against diversity (low Jaccard similarity to already-selected results).
The `lambda` parameter (0.0–1.0) controls the trade-off:

- `lambda = 1.0` → pure relevance ranking (no diversity penalty)
- `lambda = 0.5` → equal weight to relevance and diversity
- `lambda = 0.0` → pure diversity (ignores relevance)

The default in callers is typically `0.7`.

---

## Examples

### Ingesting a document

```rust
use std::sync::Arc;
use medical_rag::embeddings::EmbeddingGenerator;
use medical_rag::vector_store::VectorStore;
use medical_rag::graph_search::GraphSearch;
use medical_rag::ingestion::IngestionPipeline;
use uuid::Uuid;

let embeddings = Arc::new(EmbeddingGenerator::default());
let vector_store = Arc::new(VectorStore::new(db.clone()));
let graph_search = Arc::new(GraphSearch::new(db.clone()));

let pipeline = IngestionPipeline::new(embeddings, vector_store, graph_search);
let doc_id = Uuid::new_v4();
let chunk_count = pipeline.ingest_text(doc_id, "Patient Summary", text).await?;
```

### Running a hybrid search

```rust
use medical_rag::fusion::weighted_fusion;
use medical_rag::mmr::mmr_rerank;

let vector_results = vector_store.search(&query_embedding, 20, 0.3)?;
let bm25_results   = bm25_search.search(&expanded_query, 20)?;
let graph_results  = graph_search.search(&expanded_query, 10)?;

let fused = weighted_fusion(
    &vector_results, &bm25_results, &graph_results,
    0.5, 0.3, 0.2,
);
let final_results = mmr_rerank(&fused, 0.7, 10);
```

### Expanding a medical query

```rust
use medical_rag::query_expander::QueryExpander;

let expander = QueryExpander::new();
let expanded = expander.expand("patient with htn and sob");
// expanded.full_query == "patient with htn and sob hypertension shortness of breath dyspnea"
```

---

## Gotchas

1. **Embedding model must match.** The model used during ingestion must be the
   same model used at query time. Switching from `nomic-embed-text` to a
   different model invalidates all stored embeddings. Re-ingest documents
   after changing models.

2. **Vector search is brute-force.** `VectorStore::search` loads *all*
   embeddings from SQLite on every query. This is fine for a single-clinician
   desktop app with hundreds of documents but will not scale to millions of
   chunks. If the corpus grows substantially, consider an approximate
   nearest-neighbour index.

3. **BM25 score normalization.** FTS5 rank is negative; the normalization
   formula `-rank / (1 + -rank)` maps it to `(0, 1]`. If you change the FTS5
   tokenizer or ranking configuration, verify that the scores still land in
   the expected range.

4. **Graph tables are created lazily.** `GraphSearch` uses `OnceLock` to defer
   `CREATE TABLE IF NOT EXISTS` until the first operation. If the database is
   read-only at that point, every subsequent call will return the cached
   error.

5. **Entity extraction is keyword-based.** The `extract_medical_entities`
   function in `ingestion.rs` matches against a hard-coded list of ~15 drug
   names and ~14 conditions. It is not an NER model. Extend the lists as
   needed, but be aware that substring matching can produce false positives
   (e.g., "aspirin" in a drug-interaction warning is still captured).

6. **Deterministic entity IDs.** Entities use `Uuid::new_v5` with
   `NAMESPACE_OID` keyed on the lowercase term. The same term always produces
   the same UUID, which gives natural deduplication on upsert — but also
   means two different concepts that share a term string will collide.

7. **Chunk overlap semantics.** The `chunk_text` function operates on
   whitespace-delimited words, not tokens. A "200-word chunk with 50-word
   overlap" means the step size is 150 words. Very short documents (fewer
   words than `chunk_size`) produce exactly one chunk.

8. **Query expander is case-insensitive but not context-aware.** "mi" always
   expands to "myocardial infarction" even when the clinician means "miles"
   or "Michigan". This is acceptable for retrieval (the expanded terms are
   additive, not substitutive) but worth knowing when debugging surprising
   search results.

---

## Testing

```bash
cargo test -p medical-rag
```

All modules have unit tests using in-memory SQLite databases. No external
services (Ollama) are required for the test suite — embedding generation is
tested for configuration only, not HTTP round-trips.

---

## Line Count

~2,205 lines (including tests).
