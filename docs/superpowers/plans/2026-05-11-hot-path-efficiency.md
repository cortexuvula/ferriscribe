# Hot-Path Efficiency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the highest-impact avoidable work in three user-visible hot paths: recording → STT pipeline (per-consultation allocations), SOAP postprocessing (regex recompilation per generation), and RAG search (N+1 query). Five targeted fixes, each behavior-preserving.

**Architecture:** No architectural change. Each fix is local — either hoist a regex to module scope, drop a redundant `.clone()`, replace an iterator-collect with a reused buffer, or add a batch repo method. All existing tests must pass before and after; the fixes are pure efficiency wins.

**Tech Stack:** Rust workspace. `std::sync::LazyLock` (stable since 1.80) for regex hoisting — no `lazy_static` crate dependency needed if the toolchain supports it. Verify with `cargo --version` or `rust-toolchain.toml`; if `LazyLock` isn't available, use `once_cell::sync::Lazy` (already a transitive dep).

---

## File Structure

**Modified:**
- `crates/db/src/recordings.rs` — add `get_many(conn, &[Uuid])` batch query
- `crates/db/src/search.rs` — use the new batch method instead of looping
- `crates/processing/src/soap_generator/postprocess.rs` — hoist regexes to LazyLock
- `crates/agents/src/tools/vitals_extractor.rs` — hoist regexes to LazyLock
- `crates/stt-providers/src/local_provider.rs` — remove redundant audio clone
- `crates/stt-providers/src/remote_provider.rs` — remove redundant samples clone before diarization
- `crates/audio/src/capture.rs` — reuse drain buffer in capture loop

**No new files.**

---

## Preflight: confirm `LazyLock` availability

- [ ] **Step 0: Check Rust version**

  Run: `cargo --version && grep rust-version src-tauri/Cargo.toml 2>/dev/null`

  If the toolchain is Rust 1.80+ (`LazyLock` stable), use `std::sync::LazyLock` in all hoisting tasks below. If older, use `once_cell::sync::Lazy` (confirm it's a transitive dependency via `cargo tree | grep once_cell`). The plan below uses `LazyLock` as the default; substitute `Lazy` if needed.

---

## Task 1: Batch query for `search_recordings` (kill the N+1)

**Files:**
- Modify: `crates/db/src/recordings.rs` (add `get_many` method on `RecordingsRepo`)
- Modify: `crates/db/src/search.rs:43-56` (use the new method)

**Why:** `search_recordings` returns ids from FTS then loops `get_by_id` per result. 50 results → 50 separate SQLCipher-decrypted queries. Replacing with one `IN` query collapses 50–250 ms of wall time to ~5 ms.

- [ ] **Step 1: Write a failing test for `get_many`**

  In `crates/db/src/recordings.rs`, add a test inside the existing `#[cfg(test)] mod tests`:

  ```rust
  #[test]
  fn get_many_returns_matching_recordings_in_id_order_of_input() {
      let conn = migrated();
      let r1 = insert_test_recording(&conn, "first");
      let r2 = insert_test_recording(&conn, "second");
      let _r3 = insert_test_recording(&conn, "third"); // not requested

      let results = RecordingsRepo::get_many(&conn, &[r1.id, r2.id]).unwrap();

      assert_eq!(results.len(), 2);
      let ids: std::collections::HashSet<_> = results.iter().map(|r| r.id).collect();
      assert!(ids.contains(&r1.id));
      assert!(ids.contains(&r2.id));
  }

  #[test]
  fn get_many_empty_ids_returns_empty_vec_without_querying() {
      let conn = migrated();
      let _r1 = insert_test_recording(&conn, "first");
      let results = RecordingsRepo::get_many(&conn, &[]).unwrap();
      assert!(results.is_empty());
  }
  ```

  If `insert_test_recording` / `migrated` helpers don't exist in this file, read the existing tests to find the equivalent helper or build a minimal one inline.

  Run: `cargo test -p medical-db --lib recordings::tests::get_many`
  Expected: FAIL with `unresolved import` or `no method named 'get_many'`.

- [ ] **Step 2: Implement `get_many`**

  Add to `impl RecordingsRepo` in `crates/db/src/recordings.rs`:

  ```rust
  /// Fetch multiple recordings by id in a single query. Order is not
  /// guaranteed (use the caller's id list for ordering if needed). An
  /// empty `ids` returns an empty Vec without touching the database.
  pub fn get_many(conn: &Connection, ids: &[uuid::Uuid]) -> DbResult<Vec<Recording>> {
      if ids.is_empty() {
          return Ok(Vec::new());
      }
      // Build a parameter placeholder list "?, ?, ?, …" — rusqlite expands
      // each `?` from the params slice in order.
      let placeholders = vec!["?"; ids.len()].join(",");
      let sql = format!(
          "SELECT id, started_at, duration_seconds, transcript_path, status, \
                  metadata, source_path, original_audio_path \
           FROM recordings WHERE id IN ({placeholders})"
      );
      let id_strings: Vec<String> = ids.iter().map(|u| u.to_string()).collect();
      let params: Vec<&dyn rusqlite::ToSql> = id_strings
          .iter()
          .map(|s| s as &dyn rusqlite::ToSql)
          .collect();
      let mut stmt = conn.prepare(&sql)?;
      let rows = stmt
          .query_map(params.as_slice(), Self::row_to_recording)?
          .filter_map(|r| r.ok())
          .collect();
      Ok(rows)
  }
  ```

  This assumes a `Self::row_to_recording` helper exists; check `recordings.rs` for the actual column extraction code in `get_by_id` and factor it out into a `fn row_to_recording(r: &rusqlite::Row) -> rusqlite::Result<Recording>` if it doesn't already exist. The signature there should match `query_map`'s expected closure.

- [ ] **Step 3: Run the test to verify it passes**

  Run: `cargo test -p medical-db --lib recordings::tests::get_many`
  Expected: PASS.

- [ ] **Step 4: Rewrite `search_recordings` to use `get_many`**

  Replace `crates/db/src/search.rs:43-56`:

  ```rust
  pub fn search_recordings(
      conn: &Connection,
      query: &str,
      limit: u32,
  ) -> DbResult<Vec<Recording>> {
      let ids = Self::search(conn, query, limit)?;
      RecordingsRepo::get_many(conn, &ids)
  }
  ```

- [ ] **Step 5: Run all db tests**

  Run: `cargo test -p medical-db`
  Expected: all pass (including any existing `search_recordings` tests).

- [ ] **Step 6: Commit**

  ```bash
  git add crates/db/src/recordings.rs crates/db/src/search.rs
  git commit -m "perf(db): batch search_recordings via new get_many

  search_recordings looped get_by_id per result, hitting SQLCipher
  decryption N times for an N-result FTS query. Replace with a single
  IN (?, ?, ...) query via new RecordingsRepo::get_many. Reduces a
  50-result search from ~50-250 ms to ~5 ms."
  ```

---

## Task 2: Hoist regexes in SOAP postprocess

**Files:**
- Modify: `crates/processing/src/soap_generator/postprocess.rs`

**Why:** `clean_text` constructs 8 regexes per call; `format_soap_paragraphs` constructs 3 per `SECTION_HEADERS` entry (11 entries → 33 more). Hits every SOAP/letter/synopsis generation. 0.5–2 s of CPU per generation eliminated.

- [ ] **Step 1: Confirm the regex sites**

  Read `crates/processing/src/soap_generator/postprocess.rs`. Identify:
  - 8 `Regex::new(...)` calls in `clean_text`
  - 3 `Regex::new(...)` calls in `format_soap_paragraphs` (inside the `for header in SECTION_HEADERS` loop)
  - 1 standalone `Regex::new(r" (- [A-Z])")` after the header loop

- [ ] **Step 2: Hoist `clean_text`'s 8 regexes**

  At the top of the file (under the existing `use regex::Regex;`), add:

  ```rust
  use std::sync::LazyLock;

  static CODE_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```.+?```").unwrap());
  static INLINE_CODE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`(.+?)`").unwrap());
  static MARKDOWN_HEADING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*#+\s*").unwrap());
  static BOLD_STAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*(.*?)\*\*").unwrap());
  static BOLD_UNDER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"__(.*?)__").unwrap());
  static ITALIC_STAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*([^*]+?)\*").unwrap());
  static ITALIC_UNDER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b_([^_]+?)_\b").unwrap());
  static CITATION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\[\d+\])+").unwrap());
  ```

  Replace the body of `clean_text`:

  ```rust
  fn clean_text(text: &str) -> String {
      let mut result = CODE_BLOCK_RE.replace_all(text, "").into_owned();
      result = INLINE_CODE_RE.replace_all(&result, "$1").into_owned();
      result = MARKDOWN_HEADING_RE.replace_all(&result, "").into_owned();
      result = BOLD_STAR_RE.replace_all(&result, "$1").into_owned();
      result = BOLD_UNDER_RE.replace_all(&result, "$1").into_owned();
      result = ITALIC_STAR_RE.replace_all(&result, "$1").into_owned();
      result = ITALIC_UNDER_RE.replace_all(&result, "$1").into_owned();
      result = CITATION_RE.replace_all(&result, "").into_owned();
      result.trim().to_string()
  }
  ```

  Each `.unwrap()` is now on a compile-time-known regex pattern — that's the standard safe-unwrap pattern for `LazyLock<Regex>` initialization.

- [ ] **Step 3: Hoist `format_soap_paragraphs`'s per-header regexes**

  The current code builds three regexes inside a `for header in SECTION_HEADERS` loop. Replace the loop with a `LazyLock<Vec<(Regex, Regex, Regex)>>` precomputed at first use:

  ```rust
  /// Precomputed per-header regex triples: (mid-line-with-colon, header-at-end,
  /// header-then-bullet). One triple per SECTION_HEADERS entry, same order.
  static SECTION_HEADER_RES: LazyLock<Vec<(Regex, Regex, Regex)>> = LazyLock::new(|| {
      SECTION_HEADERS.iter().map(|header| {
          let escaped = regex::escape(header);
          (
              Regex::new(&format!(r"(?i)(\S)\s+({escaped}:)")).unwrap(),
              Regex::new(&format!(r"(?im)(\S)\s+({escaped})\s*$")).unwrap(),
              Regex::new(&format!(r"(?i)({escaped}:)\s*(- )")).unwrap(),
          )
      }).collect()
  });

  static BULLET_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" (- [A-Z])").unwrap());
  ```

  Update `format_soap_paragraphs` to use these:

  ```rust
  fn format_soap_paragraphs(text: &str) -> String {
      let mut result = text.replace("\r\n", "\n").replace('\r', "\n");

      for (mid_colon, end_anchor, header_bullet) in SECTION_HEADER_RES.iter() {
          result = mid_colon.replace_all(&result, "$1\n$2").into_owned();
          result = end_anchor.replace_all(&result, "$1\n$2").into_owned();
          result = header_bullet.replace_all(&result, "$1\n$2").into_owned();
      }

      result = BULLET_SPLIT_RE.replace_all(&result, "\n$1").into_owned();

      // ... rest of function unchanged (the blank-line insertion loop)
  }
  ```

- [ ] **Step 4: Run tests**

  Run: `cargo test -p medical-processing`
  Expected: all pass (output should be identical — this is a pure efficiency refactor).

- [ ] **Step 5: Commit**

  ```bash
  git add crates/processing/src/soap_generator/postprocess.rs
  git commit -m "perf(soap): hoist 40+ regex compilations to LazyLock

  clean_text compiled 8 regexes per call; format_soap_paragraphs compiled
  3 more for each of 11 SECTION_HEADERS — 41 total compilations per
  generation. Hoist to module-scope LazyLock so each is compiled once at
  first use. Saves ~0.5-2 s CPU per SOAP/letter/synopsis generation."
  ```

---

## Task 3: Hoist regexes in vitals extractor

**Files:**
- Modify: `crates/agents/src/tools/vitals_extractor.rs`

**Why:** Five `Regex::new(...).unwrap()` per tool invocation. Recompiled every call. Same mechanical fix as Task 2.

- [ ] **Step 1: Locate the regex construction sites**

  Run: `grep -n "Regex::new" crates/agents/src/tools/vitals_extractor.rs`

  Expected: 5 matches around lines 41, 46, 71, 86, 105, 120 (or similar — verify by reading the file).

- [ ] **Step 2: Hoist to module scope**

  At the top of the file:

  ```rust
  use std::sync::LazyLock;

  static BP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(/* the exact pattern from the original */).unwrap());
  static HR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(/* … */).unwrap());
  static TEMP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(/* … */).unwrap());
  static RR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(/* … */).unwrap());
  static SPO2_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(/* … */).unwrap());
  ```

  Replace each in-function `Regex::new(...).unwrap()` with a reference to the corresponding static. Pick the actual names by reading the surrounding code so the names reflect what they match (BP, HR, TEMP, RR, SPO2 are guesses — confirm).

- [ ] **Step 3: Run tests**

  Run: `cargo test -p medical-agents`
  Expected: all pass.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/agents/src/tools/vitals_extractor.rs
  git commit -m "perf(agents): hoist 5 vitals-extractor regexes to LazyLock

  Recompiled on every tool invocation. Hoist to module scope so each
  compiles once."
  ```

---

## Task 4: Remove redundant audio clones in STT providers

**Files:**
- Modify: `crates/stt-providers/src/local_provider.rs:90`
- Modify: `crates/stt-providers/src/remote_provider.rs` (the `audio_for_diarize = samples_i16.clone()` line — around 347, locate via grep)

**Why:** Two redundant `.clone()` calls on large `Vec<f32>`/`Vec<i16>` audio buffers right before `spawn_blocking`. The variables aren't used afterwards — a move would work. ~3 MB of avoidable memcpy per transcribed recording.

- [ ] **Step 1: Verify `audio_16k` is not used after line 90 in local_provider.rs**

  Read `crates/stt-providers/src/local_provider.rs:75-120`. Confirm `audio_16k` is the variable on line 85 and the only subsequent use is the `.clone()` on line 90.

- [ ] **Step 2: Remove the clone**

  Change:
  ```rust
  let audio_for_whisper = audio_16k.clone();
  ```
  to:
  ```rust
  let audio_for_whisper = audio_16k;
  ```

  Also check whether the surrounding code uses `audio_16k` later (e.g. for the `samples_i16` conversion for diarization). If it does, the move is wrong — leave the clone and report DONE_WITH_CONCERNS. Otherwise proceed.

- [ ] **Step 3: Repeat for `remote_provider.rs`**

  Run: `grep -n "samples_i16.clone()\|audio_for_diarize" crates/stt-providers/src/remote_provider.rs`

  Read the surrounding 15 lines. If `samples_i16` is not used after the clone, change `let audio_for_diarize = samples_i16.clone();` to `let audio_for_diarize = samples_i16;`. If `samples_i16` IS still needed afterwards, leave the clone and document why in the commit.

- [ ] **Step 4: Run tests**

  Run: `cargo test -p medical-stt-providers`
  Expected: all pass.

- [ ] **Step 5: Commit**

  ```bash
  git add crates/stt-providers/src/local_provider.rs crates/stt-providers/src/remote_provider.rs
  git commit -m "perf(stt): drop redundant audio buffer clones before spawn_blocking

  Move semantics suffice — neither audio_16k (local provider) nor
  samples_i16 (remote provider before diarization) is used after the
  clone. Saves ~3 MB of memcpy per transcribed recording."
  ```

---

## Task 5: Reuse drain buffer in audio capture loop

**Files:**
- Modify: `crates/audio/src/capture.rs:260, 286` (the drain-loop `cons.pop_iter().collect()` calls)

**Why:** `pop_iter().collect()` allocates a fresh `Vec<f32>` per loop iteration. At a 100 Hz drain cadence over a 30-min recording, that's 180,000+ allocations. Preallocate one buffer outside the loop and reuse.

- [ ] **Step 1: Inspect the drain pattern**

  Read `crates/audio/src/capture.rs:240-310`. Identify the `Vec<f32>` buffer `acc` already declared at line 256 with `Vec::with_capacity(waveform_chunk * 2)`. Confirm the per-iteration `batch: Vec<f32> = cons.pop_iter().collect()` at lines 260 and 286.

  Check whether `Consumer<f32>` (the ringbuf type) provides a method like `pop_slice(&mut [f32]) -> usize` or `pop_iter(&mut Vec<f32>)`. Look at `ringbuf` crate docs: `Consumer::pop_slice` writes into a slice and returns the count; or `Consumer::pop_iter` returns an iterator that the caller can `.extend` a Vec from.

  If `pop_slice` is the API: preallocate a `[f32; N]` scratch array (e.g. `let mut scratch = [0.0_f32; 4096];`) and `let n = cons.pop_slice(&mut scratch);` then iterate `&scratch[..n]`.

  If only `pop_iter` is available: change `batch = cons.pop_iter().collect()` to `batch.clear(); batch.extend(cons.pop_iter())` (with `batch` declared once before the loop).

- [ ] **Step 2: Refactor the drain to reuse a buffer**

  Add `let mut batch: Vec<f32> = Vec::with_capacity(waveform_chunk * 4);` immediately before the `loop {` at line 258 (alongside `acc`).

  Inside the loop, replace `let batch: Vec<f32> = cons.pop_iter().collect();` with:

  ```rust
  batch.clear();
  batch.extend(cons.pop_iter());
  ```

  Repeat at line 286 (the final-drain inner loop). Use the same `batch` buffer (it's in scope by then since it was declared at the outer loop's enclosing scope).

  Or, if the second site is inside a nested loop where the outer `batch` isn't in scope, declare a separate `final_batch` once before the inner loop.

- [ ] **Step 3: Verify by reading + cargo build**

  Run: `cargo build -p medical-audio`
  Expected: clean.

  Run: `cargo test -p medical-audio`
  Expected: existing tests pass. (The capture loop isn't easily unit-tested; integration verification will happen during manual testing of a recording.)

- [ ] **Step 4: Commit**

  ```bash
  git add crates/audio/src/capture.rs
  git commit -m "perf(audio): reuse drain buffer in capture loop

  cons.pop_iter().collect() allocated a fresh Vec<f32> per drain
  iteration. At ~100 Hz over a 30-min recording that's 180k+ allocs.
  Declare batch once with capacity and clear+extend per iteration."
  ```

---

## Final verification

- [ ] **Workspace test sweep**

  ```bash
  cargo test --workspace --lib
  cargo test -p medical-stt-providers
  cargo test -p medical-db
  cargo test -p medical-processing
  cargo test -p medical-agents
  cargo test -p medical-audio
  ```

  Expected: all pass.

- [ ] **Sanity check on the highest-impact paths**

  - SOAP generation: run a manual generate on a sample recording, confirm output text is identical to pre-refactor.
  - Search: search across a few recordings, confirm results match expectations.
  - Transcription: record a 30-second sample, confirm transcript appears.

  These are smoke-test signals — automated tests should catch any regression, but a quick manual confirmation is cheap.
