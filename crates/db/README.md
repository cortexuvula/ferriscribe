# medical-db

SQLite database layer for the FerriScribe medical transcription desktop app.

## Purpose

`medical-db` owns all persistent state: consultation recordings, application
settings, vocabulary rules, vector embeddings for RAG, a CozoDB-backed medical
knowledge graph, processing queues, generation history, letter audiences, user
dictionary, and an append-only audit log. It exposes typed repository structs
that accept a `&rusqlite::Connection`, keeping the SQL surface area isolated
from the rest of the application.

## How It Fits

```
medical-core  (types: Recording, AppConfig, VocabularyEntry, ...)
      ^
      |
  medical-db  <-- this crate
    ^   ^   ^
    |   |   |
   rag  processing  src-tauri
```

- **Depends on:** `medical-core` (shared domain types).
- **Used by:**
  - `rag` -- vector store (`VectorsRepo`) for document chunks and embeddings.
  - `processing` -- pipeline state (`ProcessingQueueRepo`) for task scheduling.
  - `src-tauri` -- every Tauri command that reads or writes persistent state.

## Key Types

| Type | Module | Role |
|------|--------|------|
| `Database` | `lib.rs` | Top-level facade; owns the `r2d2` connection pool and runs migrations on open. |
| `DbPool` / `PooledConnection` | `pool.rs` | Type aliases for `r2d2::Pool<SqliteConnectionManager>` and its checked-out connection. |
| `MigrationEngine` | `migrations/mod.rs` | Applies pending migrations in order; idempotent via the `schema_version` table. |
| `RecordingsRepo` | `recordings.rs` | CRUD for the `recordings` table -- the central entity of the app. |
| `VectorsRepo` | `vectors.rs` | RAG vector store -- insert/retrieve document chunks with `f32` embeddings serialized via `bytemuck`. |
| `GraphRepo` | `graph.rs` *(feature-gated)* | CozoDB-backed knowledge graph for medical entities and relations. |
| `SettingsRepo` | `settings.rs` | Key-value settings store plus `AppConfig` load/save. |
| `DbError` / `DbResult<T>` | `lib.rs` | Unified error enum and result alias used across all repos. |

## How It Works

### Connection Pool Lifecycle

1. `Database::open(path, key)` calls `pool::create_pool`, which builds an
   `r2d2::Pool` (max 8 connections) backed by `SqliteConnectionManager::file`.
2. Every fresh connection runs `apply_init`:
   - If an encryption key is provided, `PRAGMA key` is applied **first**
     (SQLCipher requires this before any other statement).
   - Standard pragmas: `journal_mode=WAL`, `synchronous=NORMAL`,
     `foreign_keys=ON`, `busy_timeout=5000`.
3. Migrations run once against the first checked-out connection.
4. Callers borrow connections via `db.conn()` which returns a
   `PooledConnection` that auto-returns to the pool on drop.

In-memory pools (`Database::open_in_memory`) use `max_size=1` since SQLite
in-memory databases are per-connection.

### Migration Strategy

Migrations live in `src/migrations/m001_*.rs` through `m006_*.rs`. Each
exposes a single `pub fn up(conn: &Connection) -> DbResult<()>`.

The `MigrationEngine::migrate` function:
1. Creates the `schema_version` table if absent.
2. Reads the current max version (0 if empty).
3. Iterates `all_migrations()` and applies any whose version exceeds the
   current max, recording each in `schema_version`.

Re-running is safe: already-applied migrations are skipped.

**Current migrations:**

| # | Name | Tables |
|---|------|--------|
| 1 | `initial_schema` | `recordings`, `recordings_fts`, `settings`, `audit_log`, `saved_recipients`, `processing_queue`, `batch_processing` |
| 2 | `rag_tables` | `document_chunks`, `chunks_fts` |
| 3 | `vocabulary` | `vocabulary_entries` |
| 4 | `generations` | `generations` |
| 5 | `user_dictionary` | `user_dictionary` |
| 6 | `letter_audiences` | `letter_audiences` (seeds 6 built-in rows) |

### The `recordings.metadata` JSON Column

The `metadata` column on `recordings` stores a JSON blob with a dual-field
design:

- **`context`** (string) -- freeform clinician-provided context.
- **`patient_context`** (`PatientContext` shape) -- structured demographics
  and clinical background.

New metadata keys are **non-breaking**: the column accepts any valid JSON,
and the TypeScript index signature on the frontend accepts unknown keys.
Existing code that only reads known fields will simply ignore new additions.

### FTS5 Full-Text Search

Both `recordings` and `document_chunks` have companion FTS5 virtual tables
(`recordings_fts`, `chunks_fts`) kept in sync via SQLite triggers on
INSERT, UPDATE, and DELETE. The `SearchRepo` wraps FTS5 MATCH queries for
recordings; `VectorsRepo::search_fts` does the same for document chunks.

### Encryption

When a 32-byte key is supplied to `Database::open`, the database file is
encrypted with SQLCipher. The `encryption` module also provides
`migrate_plaintext_to_encrypted` for converting an existing plaintext SQLite
file to an encrypted one, with atomic backup/swap semantics.

## Examples

### Opening the database and querying recordings

```rust
use medical_db::Database;
use medical_db::recordings::RecordingsRepo;

let db = Database::open(Path::new("app.db"), None)?;
let conn = db.conn()?;

// Paginated list (newest first)
let page = RecordingsRepo::list_all(&conn, 20, 0)?;

// Single recording by UUID
let rec = RecordingsRepo::get_by_id(&conn, &some_uuid)?;
```

### Loading and saving settings

```rust
use medical_db::settings::SettingsRepo;

let conn = db.conn()?;

// Load full AppConfig (falls back to defaults)
let config = SettingsRepo::load_config(&conn)?;

// Save modified config
SettingsRepo::save_config(&conn, &config)?;

// Or use the key-value API directly
SettingsRepo::set(&conn, "theme", "dark")?;
let theme = SettingsRepo::get(&conn, "theme")?; // Some("dark")
```

### Using the vector store (from the `rag` crate)

```rust
use medical_db::vectors::VectorsRepo;

let conn = db.conn()?;
VectorsRepo::insert_chunk(
    &conn, "chunk-1", "doc-1", "Patient has hypertension",
    Some(&[0.1, 0.2, 0.3]), 0, "{}",
)?;

let all = VectorsRepo::get_all_embeddings(&conn)?;
let fts = VectorsRepo::search_fts(&conn, "hypertension", 10)?;
```

## Gotchas

- **`metadata` JSON column is non-breaking for new keys.** Adding new
  top-level fields to the JSON is safe; readers that don't know about them
  will ignore them. Never remove or rename existing keys without a migration
  plan for the frontend.
- **Migration ordering matters.** Migrations are numbered sequentially
  (`m001` through `m006`). New migrations must use the next available number
  and be registered in `all_migrations()`. Foreign keys reference tables from
  earlier migrations (e.g., `generations.recording_id` references
  `recordings` from `m001`).
- **Thread safety.** `DbPool` is `Send + Sync`. Each `PooledConnection` is
  bound to the thread that checked it out. SQLite's WAL mode allows
  concurrent readers but only one writer at a time; `busy_timeout=5000`
  mitigates transient write contention.
- **In-memory pools are single-connection.** `Database::open_in_memory()`
  creates a pool with `max_size=1` because each SQLite in-memory connection
  is a separate database. Don't try to share in-memory state across
  connections.
- **Audit log is append-only.** SQLite triggers reject UPDATE and DELETE on
  `audit_log`. This is enforced at the database level, not just in Rust.
- **FTS triggers are automatic.** You never need to manually sync the FTS5
  tables; the triggers on `recordings` and `document_chunks` handle it.
- **Graph module is feature-gated.** `GraphRepo` requires
  `--features graph` because CozoDB pulls in the Sled storage engine.
- **SQLCipher key must be first.** On encrypted connections, `PRAGMA key`
  must execute before any other statement. The pool's `with_init` callback
  handles this; don't bypass it.
