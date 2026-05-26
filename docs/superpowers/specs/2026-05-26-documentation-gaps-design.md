# FerriScribe Documentation Design

**Date:** 2026-05-26
**Status:** Draft
**Priority:** Low (Documentation)
**Estimated effort:** 6-8 hours

## Problem Statement

FerriScribe's 14 Rust crates plus the Tauri app shell have minimal documentation:

- **No crate READMEs** — each crate's purpose, key types, and internal flow are only discoverable by reading the code
- **Incomplete inline API docs** — public functions and types lack `///` doc comments, making cross-crate usage a discovery exercise
- **No architecture diagrams** — the workspace dependency graph and data flow through the pipeline exist only in the root README's text listing

This makes it slow to re-orient after time away from the codebase, and increases the cost of understanding cross-crate contracts.

## Goals

1. **Re-orientation** — future-you can open any crate and understand its purpose, boundaries, and key flows within minutes
2. **Big picture** — a visual map of how crates depend on each other and how data flows through the pipeline
3. **API surface clarity** — public items that other crates call have doc comments explaining parameters, errors, and non-obvious behavior

## Audience

Future contributors to this codebase (primarily future-you). Not exhaustive API references for external consumers — focused on boundaries, "why," and the things that aren't obvious from reading the code.

## Approach: Layer-First, Three Passes

Three documentation layers, executed in order:

### Layer 1: Architecture Diagrams

**Output:** `docs/architecture.md`

Four Mermaid diagrams with brief intros:

1. **Workspace Dependency Graph** — all 14 crates as nodes, arrows for `use` dependencies, grouped into four layers:
   - Foundation: `core`, `security`, `db`
   - Providers: `ai-providers`, `stt-providers`, `tts-providers`, `translation`
   - Features: `rag`, `agents`, `processing`, `export`, `sharing`, `audio`
   - Shell: `src-tauri`

2. **Transcription Pipeline** — data flow from recording capture through audio chunking, whisper STT, diarization, vocabulary correction, to persisted transcript. Shows which crates handle each stage (`audio` → `processing` → `stt-providers` → `db`).

3. **Generation & Export Flow** — data flow from transcript + context through AI generation (SOAP/referral/letter), RSVP review, to export (PDF/DOCX/FHIR). Shows `ai-providers`, `agents`, `rag`, `export` in action.

4. **LAN Sharing Architecture** — office server/laptop pairing: token auth, what crosses the wire (Whisper + Ollama calls), what stays local (diarization, db, audio capture).

Each diagram gets a 2-3 sentence intro explaining what it shows and why it matters. No prose essays — the diagram does the work.

### Layer 2: Crate READMEs

**Output:** `README.md` in each crate directory.

Template for each README:

```markdown
# crate-name

One-line purpose statement.

## How It Fits

Where this crate sits in the workspace — what depends on it, what it
depends on, and when you'd reach for it vs. another crate.

## Key Types

The 3-5 most important public types/traits with a sentence each. Not
exhaustive — just the ones you need to understand to read code that
uses this crate.

## How It Works

The main flow, algorithm, or lifecycle. For providers: how a request
flows. For stores: how data is persisted. For pipelines: the stage
sequence.

## Examples

How other crates call into this one — real patterns, not contrived
snippets. Show the typical 1-2 use cases.

## Edge Cases & Gotchas

Things that will bite you if you don't know about them. Concurrency
assumptions, ordering requirements, error modes, cross-crate contracts
that aren't enforced by the type system.
```

Execution order by layer:

1. **Foundation:** `core` (error types, traits), `security` (keystore), `db` (SQLite schema + queries)
2. **Providers:** `ai-providers` (Ollama/LM Studio), `stt-providers` (whisper + diarization), `tts-providers`, `translation`
3. **Features:** `rag` (embeddings + retrieval), `agents` (orchestrator), `processing` (pipeline), `export` (PDF/DOCX/FHIR), `sharing` (LAN), `audio` (capture)
4. **Shell:** `src-tauri` (commands, state, frontend bridge)

Each README targets 100-200 lines.

### Layer 3: Inline API Documentation

**Output:** `///` doc comments on public items forming each crate's API surface.

What gets documented:

- All public structs, enums, and traits (what they represent, when to use them)
- All public functions called from other crates (parameters, return values, error conditions, panic conditions)
- Async functions: cancellation behavior, lifetime expectations
- Error types: what each variant means and when it occurs
- Cross-crate contracts that can't be enforced by the type system (e.g., the `x-auth-reason` header protocol between `stt-providers` and `security`)

What doesn't get documented:

- Trivial getters/setters that are self-explanatory
- Private/internal functions (those get `//` comments if non-obvious, not `///`)
- Test helpers

Doc comment style:

```rust
/// Resolves the current base URL, using the cache if still valid.
///
/// Returns the cached URL when the cache entry exists and is within the
/// 30-second TTL. Otherwise probes the endpoint and updates the cache.
///
/// # Errors
///
/// Returns `AppError::Network` if the endpoint probe fails.
/// Returns `AppError::Config` if `endpoint` is `None` and no cache exists.
///
/// # Cancellation
///
/// This function is cancellation-safe. Dropping the future mid-await
/// will not leave the cache in an inconsistent state.
```

Execution order: Same layer-first order as READMEs. Inline docs are done alongside each crate's README — read the crate, write the README, then sweep through and add `///` comments to the public API.

## Acceptance Criteria

- [ ] `docs/architecture.md` exists with 4 Mermaid diagrams
- [ ] All 14 crates have a `README.md` in their directory
- [ ] All public API items called cross-crate have `///` doc comments
- [ ] `cargo doc --workspace` builds with no warnings
- [ ] No changes to runtime behavior

## Risks and Mitigations

### Risk 1: Documentation becomes stale
**Mitigation:** Keep READMEs focused on concepts that change slowly (purpose, key types, flow). Inline docs are tied to code — they change when the code changes. Diagrams use Mermaid text format, easy to update.

### Risk 2: Scope creep into exhaustive API reference
**Mitigation:** Crate READMEs target 100-200 lines. Inline docs only on cross-crate public API, not every pub fn. "Future-you" audience, not "new hire onboarding guide."

### Risk 3: Diagrams don't match reality
**Mitigation:** Derive dependency graph from actual `Cargo.toml` files. Data flow diagrams derived from reading the pipeline code. Verify against current codebase before committing.

## Success Metrics

1. Opening any crate directory, the README explains purpose and flow within 30 seconds
2. The architecture page gives a complete workspace overview in under 5 minutes of reading
3. `cargo doc --workspace` produces useful HTML docs from the inline `///` comments
4. No runtime code changes — documentation only

## References

- Current root README: `README.md` (192 lines, comprehensive)
- Existing docs: `docs/error-handling.md`, `docs/runbooks/`
- Crate locations: `crates/*/` (14 crates), `src-tauri/` (app shell)
- Workspace root: `Cargo.toml`
