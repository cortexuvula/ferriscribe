# Training Corpus — Phase 1: Capture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lay the data foundation for the training-corpus feature. Every successful SOAP generation gets captured into a new `generations` table; every save updates the matching row's `final_text`. Adds a single user-facing toggle. No curate or export UI in this plan — those are Phase 2 and Phase 3.

**Architecture:** New `m004_generations.rs` migration adds the table per the spec. A new `GenerationsRepo` in `crates/db` provides CRUD. Two integration points: the SOAP generation Tauri command inserts a row after a successful AI completion; the SOAP save flow updates the matching row's `final_text`. A background `spawn_blocking` task computes `edit_distance` and `edit_ratio` after each save. A new boolean field `capture_for_training` on `AppConfig` gates the whole thing — default `false`.

**Tech Stack:** Rust workspace, existing SQLCipher + rusqlite + tokio infrastructure. Word-level Levenshtein implemented inline (≤30 LOC, no new dep). Svelte settings tab. No new crates.

**Spec reference:** `docs/superpowers/specs/2026-05-11-training-corpus-design.md` — Phase 1 (Capture).

---

## File Structure

**Created:**
- `crates/db/src/migrations/m004_generations.rs` — schema migration
- `crates/db/src/generations.rs` — `GenerationsRepo` with insert/update/finalize/fetch
- `crates/processing/src/edit_distance.rs` — word-level Levenshtein helper (~30 LOC + tests)

**Modified:**
- `crates/db/src/migrations/mod.rs` — register the new migration
- `crates/db/src/lib.rs` — expose `GenerationsRepo`
- `crates/db/src/settings.rs` — add `capture_for_training: bool` field to `AppConfig` (default false), migrate older config rows
- `crates/core/src/types/settings.rs` — same field on the mirrored shared type if it lives here (verify and apply)
- `src-tauri/src/commands/generation/<soap-command-file>.rs` — call `GenerationsRepo::record_generation` after successful AI completion (gated on `capture_for_training`)
- `src-tauri/src/commands/recordings.rs` (or wherever SOAP save lives) — call `GenerationsRepo::update_final_text` in the same transaction as the existing `recordings.soap_note` update
- `src/lib/components/settings/Audio.svelte` (or a new TrainingCorpus sub-component if cleaner) — add the toggle row

**No other files touched.**

---

## Task 1: Add `m004_generations` migration

**Files:**
- Create: `crates/db/src/migrations/m004_generations.rs`
- Modify: `crates/db/src/migrations/mod.rs` (register the migration)

### Steps

- [ ] **Step 1: Read existing migration pattern**

  Read `crates/db/src/migrations/m003_vocabulary.rs` end-to-end. The pattern is: a `pub fn migrate(conn: &Connection) -> rusqlite::Result<()>` that runs `conn.execute_batch(SQL)`. Match this exact shape.

- [ ] **Step 2: Write the migration file**

  Create `crates/db/src/migrations/m004_generations.rs` with:

  ```rust
  //! Migration 004: `generations` table for the training-corpus feature.
  //!
  //! Captures (transcript, AI draft, clinician final) triples. See
  //! docs/superpowers/specs/2026-05-11-training-corpus-design.md for
  //! the data model rationale. Personal-use-only; no PHI ever leaves
  //! this device.

  use rusqlite::Connection;

  pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
      conn.execute_batch(
          r#"
          CREATE TABLE IF NOT EXISTS generations (
              id                    TEXT PRIMARY KEY NOT NULL,
              recording_id          TEXT NOT NULL,
              output_type           TEXT NOT NULL,

              created_at            TEXT NOT NULL DEFAULT (datetime('now')),
              finalized_at          TEXT,

              ai_provider           TEXT NOT NULL,
              ai_model              TEXT NOT NULL,
              prompt_template_name  TEXT,

              input_transcript      TEXT NOT NULL,
              input_context_json    TEXT,

              draft_text            TEXT NOT NULL,
              final_text            TEXT,

              corpus_status         TEXT NOT NULL DEFAULT 'candidate'
                  CHECK (corpus_status IN ('candidate','promoted','rejected','excluded')),
              corpus_curated_at     TEXT,

              edit_distance         INTEGER,
              edit_ratio            REAL,

              regeneration_seq      INTEGER NOT NULL DEFAULT 1,

              FOREIGN KEY (recording_id) REFERENCES recordings(id) ON DELETE CASCADE
          );

          CREATE INDEX IF NOT EXISTS idx_generations_recording
              ON generations (recording_id);
          CREATE INDEX IF NOT EXISTS idx_generations_corpus_status
              ON generations (corpus_status, created_at DESC);
          CREATE INDEX IF NOT EXISTS idx_generations_created
              ON generations (created_at DESC);
          "#,
      )?;
      Ok(())
  }
  ```

- [ ] **Step 3: Register the migration**

  Read `crates/db/src/migrations/mod.rs`. Find the list of registered migrations (likely a `match` on schema_version that calls `mNNN::migrate`). Add the m004 entry following the existing pattern:

  ```rust
  // (after the m003 entry)
  pub mod m004_generations;

  // inside the MigrationEngine::migrate function or wherever the version
  // dispatch happens, add the next match arm:
  4 => m004_generations::migrate(conn)?,
  ```

  Confirm by reading the file what the current latest version is and what numbering convention applies. Match it exactly.

- [ ] **Step 4: Write the migration test**

  Add to `crates/db/src/migrations/m004_generations.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::migrations::MigrationEngine;

      fn migrated() -> Connection {
          let conn = Connection::open_in_memory().unwrap();
          MigrationEngine::migrate(&conn).unwrap();
          conn
      }

      #[test]
      fn generations_table_exists_after_migration() {
          let conn = migrated();
          let exists: bool = conn
              .query_row(
                  "SELECT 1 FROM sqlite_master WHERE type='table' AND name='generations'",
                  [],
                  |r| r.get(0),
              )
              .unwrap_or(false);
          assert!(exists, "generations table should exist after migration");
      }

      #[test]
      fn generations_table_has_required_columns() {
          let conn = migrated();
          let columns: Vec<String> = {
              let mut stmt = conn.prepare("PRAGMA table_info(generations)").unwrap();
              let rows = stmt
                  .query_map([], |row| row.get::<_, String>(1))
                  .unwrap();
              rows.filter_map(|r| r.ok()).collect()
          };
          for required in &[
              "id",
              "recording_id",
              "output_type",
              "created_at",
              "finalized_at",
              "ai_provider",
              "ai_model",
              "prompt_template_name",
              "input_transcript",
              "input_context_json",
              "draft_text",
              "final_text",
              "corpus_status",
              "corpus_curated_at",
              "edit_distance",
              "edit_ratio",
              "regeneration_seq",
          ] {
              assert!(
                  columns.iter().any(|c| c == required),
                  "missing column: {required}; have: {columns:?}"
              );
          }
      }

      #[test]
      fn cascade_delete_removes_generations() {
          let conn = migrated();
          // Insert a parent recording first (FK requires it). Use minimal
          // columns since the schema allows null on most.
          conn.execute(
              "INSERT INTO recordings (id, filename, processing_status, created_at) \
               VALUES ('rec1','file.wav','done',datetime('now'))",
              [],
          )
          .unwrap();
          conn.execute(
              "INSERT INTO generations (id, recording_id, output_type, ai_provider, \
                  ai_model, input_transcript, draft_text) \
               VALUES ('gen1','rec1','soap','ollama','llama3','t','d')",
              [],
          )
          .unwrap();

          conn.execute("DELETE FROM recordings WHERE id='rec1'", []).unwrap();

          let remaining: i64 = conn
              .query_row("SELECT count(*) FROM generations WHERE id='gen1'", [], |r| r.get(0))
              .unwrap();
          assert_eq!(remaining, 0, "generation should cascade-delete with its parent recording");
      }
  }
  ```

- [ ] **Step 5: Run the tests**

  Run: `cargo test -p medical-db --lib migrations::m004_generations`
  Expected: 3/3 pass.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/db/src/migrations/m004_generations.rs crates/db/src/migrations/mod.rs
  git commit -m "feat(db): m004 generations table for training-corpus capture

  New table captures (recording, AI draft, clinician final) triples for
  the personal training-corpus feature. ON DELETE CASCADE on
  recording_id, three indexes (by recording, by corpus_status, by
  created). Detailed rationale in
  docs/superpowers/specs/2026-05-11-training-corpus-design.md."
  ```

---

## Task 2: `GenerationsRepo` with insert + finalize

**Files:**
- Create: `crates/db/src/generations.rs`
- Modify: `crates/db/src/lib.rs` (re-export)

### Steps

- [ ] **Step 1: Read existing repo patterns**

  Read `crates/db/src/recordings.rs` to understand:
  - The repo style (impl block on a unit struct, methods take `&Connection`)
  - Row-to-struct conversion patterns
  - Error type (`DbResult` / `DbError`)
  - UUID generation patterns

  Match these conventions exactly.

- [ ] **Step 2: Create the struct and insert method**

  Create `crates/db/src/generations.rs`:

  ```rust
  //! Repository for `generations` (training-corpus capture table).
  //!
  //! See docs/superpowers/specs/2026-05-11-training-corpus-design.md.
  //! Personal use only; data never leaves the device unless the
  //! clinician explicitly exports via the (Phase 3) pipeline.

  use crate::{DbError, DbResult};
  use rusqlite::{Connection, OptionalExtension, params};
  use serde::{Deserialize, Serialize};
  use uuid::Uuid;

  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
  pub struct Generation {
      pub id: Uuid,
      pub recording_id: Uuid,
      pub output_type: String,
      pub created_at: String,
      pub finalized_at: Option<String>,
      pub ai_provider: String,
      pub ai_model: String,
      pub prompt_template_name: Option<String>,
      pub input_transcript: String,
      pub input_context_json: Option<String>,
      pub draft_text: String,
      pub final_text: Option<String>,
      pub corpus_status: String,
      pub corpus_curated_at: Option<String>,
      pub edit_distance: Option<i64>,
      pub edit_ratio: Option<f64>,
      pub regeneration_seq: i64,
  }

  /// Inputs needed at row-insertion time. Some fields (final_text,
  /// edit_distance, etc.) are NULL at insert and get populated by
  /// later updates.
  #[derive(Debug, Clone)]
  pub struct GenerationInsert<'a> {
      pub recording_id: Uuid,
      pub output_type: &'a str,
      pub ai_provider: &'a str,
      pub ai_model: &'a str,
      pub prompt_template_name: Option<&'a str>,
      pub input_transcript: &'a str,
      pub input_context_json: Option<&'a str>,
      pub draft_text: &'a str,
  }

  pub struct GenerationsRepo;

  impl GenerationsRepo {
      /// Insert a new generation row. Computes `regeneration_seq` by
      /// finding the max for the same (recording_id, output_type) and
      /// adding 1; if none exists, starts at 1.
      pub fn record_generation(
          conn: &Connection,
          input: GenerationInsert<'_>,
      ) -> DbResult<Generation> {
          let id = Uuid::new_v4();
          let prev_max: Option<i64> = conn
              .query_row(
                  "SELECT MAX(regeneration_seq) FROM generations \
                   WHERE recording_id = ? AND output_type = ?",
                  params![input.recording_id.to_string(), input.output_type],
                  |r| r.get(0),
              )
              .optional()?;
          let seq = prev_max.unwrap_or(0) + 1;

          conn.execute(
              "INSERT INTO generations (
                  id, recording_id, output_type, ai_provider, ai_model,
                  prompt_template_name, input_transcript, input_context_json,
                  draft_text, regeneration_seq
              ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
              params![
                  id.to_string(),
                  input.recording_id.to_string(),
                  input.output_type,
                  input.ai_provider,
                  input.ai_model,
                  input.prompt_template_name,
                  input.input_transcript,
                  input.input_context_json,
                  input.draft_text,
                  seq,
              ],
          )?;
          Self::get_by_id(conn, id)
      }

      pub fn get_by_id(conn: &Connection, id: Uuid) -> DbResult<Generation> {
          conn.query_row(
              "SELECT id, recording_id, output_type, created_at, finalized_at,
                      ai_provider, ai_model, prompt_template_name,
                      input_transcript, input_context_json,
                      draft_text, final_text,
                      corpus_status, corpus_curated_at,
                      edit_distance, edit_ratio, regeneration_seq
               FROM generations WHERE id = ?",
              params![id.to_string()],
              Self::row_to_generation,
          )
          .map_err(DbError::from)
      }

      fn row_to_generation(row: &rusqlite::Row) -> rusqlite::Result<Generation> {
          Ok(Generation {
              id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_else(|_| Uuid::nil()),
              recording_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_else(|_| Uuid::nil()),
              output_type: row.get(2)?,
              created_at: row.get(3)?,
              finalized_at: row.get(4)?,
              ai_provider: row.get(5)?,
              ai_model: row.get(6)?,
              prompt_template_name: row.get(7)?,
              input_transcript: row.get(8)?,
              input_context_json: row.get(9)?,
              draft_text: row.get(10)?,
              final_text: row.get(11)?,
              corpus_status: row.get(12)?,
              corpus_curated_at: row.get(13)?,
              edit_distance: row.get(14)?,
              edit_ratio: row.get(15)?,
              regeneration_seq: row.get(16)?,
          })
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::migrations::MigrationEngine;

      fn migrated() -> Connection {
          let conn = Connection::open_in_memory().unwrap();
          MigrationEngine::migrate(&conn).unwrap();
          conn
      }

      fn insert_test_recording(conn: &Connection, id: &str) -> Uuid {
          conn.execute(
              "INSERT INTO recordings (id, filename, processing_status, created_at) \
               VALUES (?, 'test.wav', 'done', datetime('now'))",
              params![id],
          )
          .unwrap();
          Uuid::parse_str(id).unwrap_or_else(|_| {
              // If id is not a real UUID (e.g. 'rec1' fixture), generate one
              // and re-insert. Test simplification.
              Uuid::nil()
          })
      }

      #[test]
      fn record_generation_inserts_with_seq_1_on_first_call() {
          let conn = migrated();
          let rec_id = Uuid::new_v4();
          conn.execute(
              "INSERT INTO recordings (id, filename, processing_status, created_at) \
               VALUES (?, 'test.wav', 'done', datetime('now'))",
              params![rec_id.to_string()],
          )
          .unwrap();

          let gen = GenerationsRepo::record_generation(
              &conn,
              GenerationInsert {
                  recording_id: rec_id,
                  output_type: "soap",
                  ai_provider: "ollama",
                  ai_model: "llama3:70b",
                  prompt_template_name: Some("soap-default"),
                  input_transcript: "Patient reports cough.",
                  input_context_json: None,
                  draft_text: "S: cough. O: none. A: viral URI. P: rest.",
              },
          )
          .unwrap();

          assert_eq!(gen.regeneration_seq, 1);
          assert_eq!(gen.corpus_status, "candidate");
          assert_eq!(gen.draft_text, "S: cough. O: none. A: viral URI. P: rest.");
          assert!(gen.final_text.is_none());
          assert!(gen.finalized_at.is_none());
          assert!(gen.edit_distance.is_none());
      }

      #[test]
      fn record_generation_bumps_seq_on_regeneration() {
          let conn = migrated();
          let rec_id = Uuid::new_v4();
          conn.execute(
              "INSERT INTO recordings (id, filename, processing_status, created_at) \
               VALUES (?, 'test.wav', 'done', datetime('now'))",
              params![rec_id.to_string()],
          )
          .unwrap();

          let insert = GenerationInsert {
              recording_id: rec_id,
              output_type: "soap",
              ai_provider: "ollama",
              ai_model: "llama3:70b",
              prompt_template_name: None,
              input_transcript: "t",
              input_context_json: None,
              draft_text: "d1",
          };

          let g1 = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
          let g2 = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
          let g3 = GenerationsRepo::record_generation(&conn, insert).unwrap();

          assert_eq!(g1.regeneration_seq, 1);
          assert_eq!(g2.regeneration_seq, 2);
          assert_eq!(g3.regeneration_seq, 3);
      }
  }
  ```

- [ ] **Step 3: Re-export from `crates/db/src/lib.rs`**

  Read `crates/db/src/lib.rs`. Find where existing repos (e.g., `RecordingsRepo`) are re-exported. Add:

  ```rust
  pub mod generations;
  pub use generations::{Generation, GenerationInsert, GenerationsRepo};
  ```

- [ ] **Step 4: Run the tests**

  Run: `cargo test -p medical-db --lib generations`
  Expected: 2/2 pass.

- [ ] **Step 5: Commit**

  ```bash
  git add crates/db/src/generations.rs crates/db/src/lib.rs
  git commit -m "feat(db): GenerationsRepo with insert + auto-incrementing seq

  Foundational repo for the training-corpus capture flow.
  record_generation auto-computes regeneration_seq by querying the
  current max for the (recording_id, output_type) pair, so the
  caller doesn't have to track sequence externally."
  ```

---

## Task 3: `update_final_text` + `mark_edit_distance`

**Files:**
- Modify: `crates/db/src/generations.rs` — add two more methods

### Steps

- [ ] **Step 1: Add the methods**

  Append to `impl GenerationsRepo` in `crates/db/src/generations.rs`:

  ```rust
      /// Set `final_text` and `finalized_at` on the most recent
      /// generation row for the given (recording_id, output_type).
      /// Returns the updated row, or `Ok(None)` if no matching row
      /// exists (capture was off when the SOAP was generated).
      pub fn update_final_text(
          conn: &Connection,
          recording_id: Uuid,
          output_type: &str,
          final_text: &str,
      ) -> DbResult<Option<Generation>> {
          // Find the most recent row by regeneration_seq.
          let row_id: Option<String> = conn
              .query_row(
                  "SELECT id FROM generations
                   WHERE recording_id = ? AND output_type = ?
                   ORDER BY regeneration_seq DESC LIMIT 1",
                  params![recording_id.to_string(), output_type],
                  |r| r.get(0),
              )
              .optional()?;
          let row_id = match row_id {
              Some(s) => s,
              None => return Ok(None),
          };

          conn.execute(
              "UPDATE generations
                  SET final_text = ?, finalized_at = datetime('now')
                WHERE id = ?",
              params![final_text, row_id],
          )?;
          let id = Uuid::parse_str(&row_id).map_err(|e| DbError::Other(e.to_string()))?;
          Ok(Some(Self::get_by_id(conn, id)?))
      }

      /// Update the cached edit-distance signals. Called by the
      /// background task that computes word-level Levenshtein.
      /// Safe to call repeatedly (idempotent).
      pub fn set_edit_distance(
          conn: &Connection,
          id: Uuid,
          edit_distance: i64,
          edit_ratio: f64,
      ) -> DbResult<()> {
          conn.execute(
              "UPDATE generations
                  SET edit_distance = ?, edit_ratio = ?
                WHERE id = ?",
              params![edit_distance, edit_ratio, id.to_string()],
          )?;
          Ok(())
      }
  ```

  If `DbError::Other` doesn't exist, use whichever variant matches (check the enum — probably `DbError::Other(String)` or `DbError::Sqlite(rusqlite::Error)`). Adjust to fit.

- [ ] **Step 2: Add tests**

  Append to the existing `mod tests` block in `crates/db/src/generations.rs`:

  ```rust
      #[test]
      fn update_final_text_populates_the_most_recent_row() {
          let conn = migrated();
          let rec_id = Uuid::new_v4();
          conn.execute(
              "INSERT INTO recordings (id, filename, processing_status, created_at) \
               VALUES (?, 'test.wav', 'done', datetime('now'))",
              params![rec_id.to_string()],
          )
          .unwrap();

          let insert = GenerationInsert {
              recording_id: rec_id,
              output_type: "soap",
              ai_provider: "ollama",
              ai_model: "llama3:70b",
              prompt_template_name: None,
              input_transcript: "t",
              input_context_json: None,
              draft_text: "d",
          };
          let g1 = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
          let g2 = GenerationsRepo::record_generation(&conn, insert).unwrap();

          let updated = GenerationsRepo::update_final_text(&conn, rec_id, "soap", "final-v1")
              .unwrap()
              .expect("should have updated a row");

          // Only the most-recent (g2) should have final_text set.
          assert_eq!(updated.id, g2.id);
          assert_eq!(updated.final_text.as_deref(), Some("final-v1"));
          assert!(updated.finalized_at.is_some());

          // g1 should still have NULL final_text — that's the
          // "rejected draft" signal.
          let g1_refreshed = GenerationsRepo::get_by_id(&conn, g1.id).unwrap();
          assert!(g1_refreshed.final_text.is_none());
      }

      #[test]
      fn update_final_text_returns_none_when_no_matching_row() {
          let conn = migrated();
          let rec_id = Uuid::new_v4();
          conn.execute(
              "INSERT INTO recordings (id, filename, processing_status, created_at) \
               VALUES (?, 'test.wav', 'done', datetime('now'))",
              params![rec_id.to_string()],
          )
          .unwrap();

          let result = GenerationsRepo::update_final_text(&conn, rec_id, "soap", "x").unwrap();
          assert!(result.is_none(), "should be None when capture wasn't on");
      }

      #[test]
      fn set_edit_distance_writes_both_fields() {
          let conn = migrated();
          let rec_id = Uuid::new_v4();
          conn.execute(
              "INSERT INTO recordings (id, filename, processing_status, created_at) \
               VALUES (?, 'test.wav', 'done', datetime('now'))",
              params![rec_id.to_string()],
          )
          .unwrap();
          let gen = GenerationsRepo::record_generation(
              &conn,
              GenerationInsert {
                  recording_id: rec_id,
                  output_type: "soap",
                  ai_provider: "ollama",
                  ai_model: "llama3",
                  prompt_template_name: None,
                  input_transcript: "t",
                  input_context_json: None,
                  draft_text: "d",
              },
          )
          .unwrap();

          GenerationsRepo::set_edit_distance(&conn, gen.id, 12, 0.34).unwrap();

          let refreshed = GenerationsRepo::get_by_id(&conn, gen.id).unwrap();
          assert_eq!(refreshed.edit_distance, Some(12));
          assert_eq!(refreshed.edit_ratio, Some(0.34));
      }
  ```

- [ ] **Step 3: Run the tests**

  Run: `cargo test -p medical-db --lib generations`
  Expected: 5/5 pass.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/db/src/generations.rs
  git commit -m "feat(db): GenerationsRepo update_final_text + set_edit_distance

  update_final_text targets the most-recent (recording, output_type)
  row by regeneration_seq, leaving older rows' final_text as NULL —
  the 'rejected draft' signal documented in the spec.
  set_edit_distance is idempotent; called from the background
  Levenshtein task after a save."
  ```

---

## Task 4: Word-level Levenshtein helper

**Files:**
- Create: `crates/processing/src/edit_distance.rs`
- Modify: `crates/processing/src/lib.rs` (expose the module)

### Steps

- [ ] **Step 1: Write failing tests**

  Create `crates/processing/src/edit_distance.rs`:

  ```rust
  //! Word-level Levenshtein for the training-corpus edit-distance signal.
  //!
  //! Operates on whitespace-split tokens, not characters — the curate
  //! UI surfaces "you changed 20% of the words," which is more
  //! intuitive for clinicians than character delta. ~O(m·n) where m
  //! and n are token counts; for typical 200-800-word SOAPs that's a
  //! sub-millisecond computation.

  /// Word-level Levenshtein distance + ratio.
  ///
  /// Returns `(distance, ratio)` where `ratio = distance / max(a_words, b_words)`,
  /// clamped to `[0.0, 1.0]`. Empty inputs return `(0, 0.0)`.
  pub fn word_edit_distance(a: &str, b: &str) -> (usize, f64) {
      let a_words: Vec<&str> = a.split_whitespace().collect();
      let b_words: Vec<&str> = b.split_whitespace().collect();

      let m = a_words.len();
      let n = b_words.len();
      if m == 0 && n == 0 {
          return (0, 0.0);
      }

      // Two-row DP, O(min(m,n)) memory after the swap.
      let (short, long) = if m <= n { (&a_words, &b_words) } else { (&b_words, &a_words) };
      let s_len = short.len();
      let l_len = long.len();

      let mut prev: Vec<usize> = (0..=s_len).collect();
      let mut curr: Vec<usize> = vec![0; s_len + 1];

      for i in 1..=l_len {
          curr[0] = i;
          for j in 1..=s_len {
              let cost = if long[i - 1] == short[j - 1] { 0 } else { 1 };
              curr[j] = (curr[j - 1] + 1)         // insertion
                  .min(prev[j] + 1)               // deletion
                  .min(prev[j - 1] + cost);       // substitution
          }
          std::mem::swap(&mut prev, &mut curr);
      }

      let distance = prev[s_len];
      let denom = m.max(n) as f64;
      let ratio = (distance as f64 / denom).clamp(0.0, 1.0);
      (distance, ratio)
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn identical_strings_return_zero() {
          let (d, r) = word_edit_distance("hello world", "hello world");
          assert_eq!(d, 0);
          assert_eq!(r, 0.0);
      }

      #[test]
      fn empty_strings_return_zero() {
          let (d, r) = word_edit_distance("", "");
          assert_eq!(d, 0);
          assert_eq!(r, 0.0);
      }

      #[test]
      fn single_word_substitution() {
          let (d, r) = word_edit_distance("hello world", "hello there");
          assert_eq!(d, 1);
          assert!((r - 0.5).abs() < 1e-9, "expected ratio 0.5, got {r}");
      }

      #[test]
      fn complete_replacement_returns_max_ratio() {
          let (d, r) = word_edit_distance("a b c", "d e f");
          assert_eq!(d, 3);
          assert!((r - 1.0).abs() < 1e-9);
      }

      #[test]
      fn insertion_at_end() {
          let (d, _) = word_edit_distance("a b", "a b c d");
          assert_eq!(d, 2);
      }

      #[test]
      fn deletion_at_start() {
          let (d, _) = word_edit_distance("a b c d", "c d");
          assert_eq!(d, 2);
      }

      #[test]
      fn typical_soap_edit_is_moderate_ratio() {
          let draft = "S: Patient reports cough. O: temp 98.6. A: viral URI. P: rest.";
          let edited = "S: Patient reports productive cough. O: temp 99.1, mild rhonchi. A: viral URI. P: rest, fluids.";
          let (_d, r) = word_edit_distance(draft, edited);
          assert!(r > 0.1 && r < 0.6, "expected moderate edit ratio, got {r}");
      }
  }
  ```

- [ ] **Step 2: Run tests to verify they fail**

  Run: `cargo test -p medical-processing --lib edit_distance`
  Expected: tests fail to find module (until step 3 exports it).

- [ ] **Step 3: Expose the module**

  Read `crates/processing/src/lib.rs`. Add `pub mod edit_distance;` next to existing `pub mod` lines.

- [ ] **Step 4: Run tests to verify pass**

  Run: `cargo test -p medical-processing --lib edit_distance`
  Expected: 7/7 pass.

- [ ] **Step 5: Commit**

  ```bash
  git add crates/processing/src/edit_distance.rs crates/processing/src/lib.rs
  git commit -m "feat(processing): word-level Levenshtein for training-corpus signal

  Operates on whitespace-split tokens. ~O(m*n) with two-row DP,
  sub-millisecond on typical 200-800-word SOAP notes. Returns
  (distance, ratio) where ratio = distance / max(a_words, b_words),
  clamped to [0,1]. Empty input returns (0, 0.0)."
  ```

---

## Task 5: Add `capture_for_training` to `AppConfig`

**Files:**
- Modify: `crates/db/src/settings.rs` — add the field (may live here as part of `AppConfig`)
- Modify: `crates/core/src/types/settings.rs` (if the canonical `AppConfig` lives here — verify with grep)

### Steps

- [ ] **Step 1: Locate the `AppConfig` struct**

  Run: `grep -rn "struct AppConfig" crates/ src-tauri/src/ --include="*.rs"`

  Read the declaration. Note where it lives (core or db) and what fields exist. The struct likely already has dozens of fields like `ai_provider`, `stt_mode`, etc.

- [ ] **Step 2: Add the field**

  Add to the struct:

  ```rust
  /// When true, every successful SOAP generation is captured into the
  /// `generations` table for the training-corpus feature. Defaults to
  /// false — captures are opt-in. See
  /// docs/superpowers/specs/2026-05-11-training-corpus-design.md.
  #[serde(default)]
  pub capture_for_training: bool,
  ```

  The `#[serde(default)]` ensures older serialized AppConfig JSON parses cleanly (existing settings rows return `false` for this field).

- [ ] **Step 3: Add a migration test if there's a `migrate` method on AppConfig**

  If `AppConfig::migrate(&mut self)` is the canonical place to handle schema-version-style upgrades (check by reading), no extra test is needed — `#[serde(default)]` handles missing-field absorption. If there's an explicit migration function, ensure `capture_for_training` stays `false` for migrated old configs.

  If unsure, skip this step and verify in step 5.

- [ ] **Step 4: Run existing settings tests**

  Run: `cargo test -p medical-db --lib settings`
  Expected: all existing settings tests pass; new field doesn't break serialization.

- [ ] **Step 5: Add a round-trip test for the new field**

  Add to the existing `#[cfg(test)] mod tests` block in `settings.rs`:

  ```rust
  #[test]
  fn capture_for_training_defaults_to_false_in_older_configs() {
      // Simulate an older config JSON missing the new field.
      let old_json = r#"{"ai_provider":"ollama","stt_mode":"Local"}"#;
      let cfg: AppConfig = serde_json::from_str(old_json).expect("should parse with serde defaults");
      assert!(!cfg.capture_for_training, "default must be false");
  }

  #[test]
  fn capture_for_training_round_trips() {
      let mut cfg = AppConfig::default();
      cfg.capture_for_training = true;
      let json = serde_json::to_string(&cfg).unwrap();
      let back: AppConfig = serde_json::from_str(&json).unwrap();
      assert!(back.capture_for_training);
  }
  ```

  Adjust the "older config" JSON to use real field names from the existing AppConfig (don't invent — match what's actually there).

- [ ] **Step 6: Run the new tests**

  Run: `cargo test -p medical-db --lib settings`
  Expected: 2+ new tests pass.

- [ ] **Step 7: Commit**

  ```bash
  git add crates/db/src/settings.rs crates/core/src/types/settings.rs
  git commit -m "feat(settings): add capture_for_training toggle (default false)

  Gates the training-corpus capture flow. #[serde(default)] ensures
  existing AppConfig rows in production DBs parse cleanly with the
  field defaulting to false."
  ```

  (Drop `crates/core/src/types/settings.rs` from the add list if that file wasn't modified.)

---

## Task 6: Wire capture into the SOAP generation Tauri command

**Files:**
- Modify: the Tauri command that handles SOAP generation. Locate via:
  ```
  grep -rn "fn.*generate_soap\|fn.*process_soap\|soap_generator::generate\|generate_soap_note" src-tauri/src --include="*.rs"
  ```
  Read the matching function in full. The hook point is **after** the AI completion returns OK and **before** the response is returned to the frontend.

### Steps

- [ ] **Step 1: Locate the SOAP generation command**

  Use the grep above. Read the relevant function. Look for the pattern: a call into `crates/processing/soap_generator` or similar that takes a transcript + context and returns a string. Identify the variable holding the draft text returned by the AI.

  Also identify how to access:
  - The DB connection (via `state.db.conn()?` or similar)
  - The `AppConfig` (via `SettingsRepo::load_config(&conn)` or similar)
  - The recording_id, ai_provider, ai_model, prompt_template_name, input_transcript, input_context

  If any of these aren't already in scope at the hook point, identify how to propagate them.

  **If the structure is significantly different from what's described** — STOP and report. Don't guess.

- [ ] **Step 2: Add the capture call**

  Immediately after the successful AI completion, before returning the draft to the frontend, insert:

  ```rust
  // Capture the generation for the training-corpus feature, if the
  // user has enabled it. Failure to capture must NOT break the
  // user's workflow — log and continue.
  let config = medical_db::settings::SettingsRepo::load_config(&conn).unwrap_or_default();
  if config.capture_for_training {
      let insert = medical_db::generations::GenerationInsert {
          recording_id,                       // already in scope
          output_type: "soap",
          ai_provider: &ai_provider_id,       // identify from current scope
          ai_model: &ai_model_id,             // identify from current scope
          prompt_template_name: prompt_template_name.as_deref(),
          input_transcript: &transcript,      // the input used for generation
          input_context_json: input_context_json.as_deref(),
          draft_text: &draft,                 // the variable holding the AI's output
      };
      match medical_db::generations::GenerationsRepo::record_generation(&conn, insert) {
          Ok(g) => tracing::debug!(generation_id = %g.id, "captured generation for training"),
          Err(e) => tracing::warn!(error = %e, "training-corpus capture insert failed; continuing"),
      }
  }
  ```

  Adapt variable names to match what's actually in scope. If `input_context_json` isn't pre-computed, serialize the appropriate context fields:

  ```rust
  let input_context_json = serde_json::to_string(&context_value).ok();
  ```

- [ ] **Step 3: Locate the SOAP save flow**

  Run: `grep -rn "soap_note\s*=\|set_soap_note\|update.*soap_note" src-tauri/src --include="*.rs"` and `grep -rn "UPDATE recordings SET soap_note" crates/db --include="*.rs"`.

  The save flow is probably in `src-tauri/src/commands/recordings.rs` or similar — find where `recordings.soap_note` is updated. That's the hook for `update_final_text`.

- [ ] **Step 4: Add the finalize call**

  In the same transaction as the existing `recordings.soap_note` update, after that update succeeds, add:

  ```rust
  // Mirror the saved SOAP into the most-recent generations row for
  // this recording, populating final_text. Best-effort: if no
  // matching row exists (capture wasn't on at generation time),
  // update_final_text returns Ok(None) and we move on.
  let final_text_clone = soap_note_value.to_string();
  match medical_db::generations::GenerationsRepo::update_final_text(
      &conn,
      recording_id,
      "soap",
      &final_text_clone,
  ) {
      Ok(Some(g)) => {
          tracing::debug!(generation_id = %g.id, "updated final_text on generations row");
          // Spawn the edit-distance task (Task 7 wires this up).
          spawn_edit_distance_task(state.db.clone(), g.id, g.draft_text.clone(), final_text_clone);
      }
      Ok(None) => {}
      Err(e) => tracing::warn!(error = %e, "training-corpus finalize update failed; continuing"),
  }
  ```

  The `spawn_edit_distance_task` helper is implemented in Task 7. For now, stub it as:

  ```rust
  fn spawn_edit_distance_task(_db: std::sync::Arc<medical_db::Database>, _id: uuid::Uuid, _draft: String, _final_text: String) {
      // Implemented in Task 7
  }
  ```

  Inline this stub at the file scope (or a `mod helpers` block); Task 7 will replace it with the real implementation.

- [ ] **Step 5: Build and test**

  Run: `cargo build -p rust-medical-assistant`
  Expected: clean.

  Run: `cargo test -p rust-medical-assistant --lib`
  Expected: existing tests still pass. (No new test in this task — manual smoke-test happens in Task 8.)

- [ ] **Step 6: Commit**

  ```bash
  git add src-tauri/src/commands/generation/<file>.rs src-tauri/src/commands/recordings.rs
  git commit -m "feat(soap): capture generation + finalize into generations table

  Hook GenerationsRepo::record_generation into the SOAP generation
  command (gated on AppConfig.capture_for_training, default off) and
  GenerationsRepo::update_final_text into the save flow. Capture
  failures log at warn and never break the user's workflow.

  Spawn-edit-distance helper stubbed; populated in the next commit."
  ```

  (Replace `<file>` with the actual file you modified.)

---

## Task 7: Background edit-distance task

**Files:**
- Modify: wherever `spawn_edit_distance_task` was stubbed in Task 6
- May add: a small helper module if the file is getting long (`src-tauri/src/commands/generation/edit_distance_task.rs` or similar)

### Steps

- [ ] **Step 1: Replace the stub with a real implementation**

  Replace the Task 6 stub with:

  ```rust
  /// Spawn a background task that computes word-level Levenshtein on
  /// (draft, final) and writes the result back via
  /// GenerationsRepo::set_edit_distance. Best-effort; failures
  /// (lock contention, panic in the computation) are logged but
  /// don't propagate.
  fn spawn_edit_distance_task(
      db: std::sync::Arc<medical_db::Database>,
      generation_id: uuid::Uuid,
      draft: String,
      final_text: String,
  ) {
      tokio::task::spawn_blocking(move || {
          let (distance, ratio) = medical_processing::edit_distance::word_edit_distance(&draft, &final_text);
          match db.conn() {
              Ok(conn) => {
                  if let Err(e) = medical_db::generations::GenerationsRepo::set_edit_distance(
                      &conn,
                      generation_id,
                      distance as i64,
                      ratio,
                  ) {
                      tracing::warn!(error = %e, generation_id = %generation_id,
                          "set_edit_distance failed");
                  }
              }
              Err(e) => {
                  tracing::warn!(error = %e, generation_id = %generation_id,
                      "edit-distance task could not open DB connection");
              }
          }
      });
  }
  ```

  The `tokio::task::spawn_blocking` keeps the runtime free during the (cheap) Levenshtein computation; the DB write itself is sync.

- [ ] **Step 2: Build**

  Run: `cargo build -p rust-medical-assistant`
  Expected: clean.

- [ ] **Step 3: Integration test**

  Add an integration-style test in `src-tauri/src/commands/recordings.rs` or wherever the save flow lives:

  ```rust
  #[cfg(test)]
  mod tests {
      // ... existing tests ...

      #[tokio::test]
      async fn save_soap_with_capture_on_populates_final_and_edit_distance() {
          // Set up an in-memory DB with capture_for_training=true,
          // insert a recording + a generations row (simulating
          // capture having run), call the save flow, and assert
          // final_text + edit_distance are populated within a
          // reasonable time window (the background task is async).
          //
          // Skip if the existing tests don't already build an
          // AppState shape that supports this flow — note as a gap
          // and rely on manual smoke-test in Task 8.
      }
  }
  ```

  If the integration-test scaffolding is non-trivial (which it likely is for a Tauri command), defer to the manual smoke-test in Task 8 and skip this step. Document the deferral in the commit message.

- [ ] **Step 4: Commit**

  ```bash
  git add <files modified>
  git commit -m "feat(soap): background edit-distance task populates training-corpus signal

  spawn_blocking computes word-level Levenshtein on (draft, final)
  after each save, then writes distance + ratio back to the
  generations row. Best-effort; failures log at warn. Complements
  Task 6's capture + finalize wiring.

  Integration-level test deferred to manual smoke-test pending an
  AppState testing harness."
  ```

---

## Task 8: Settings toggle in the UI

**Files:**
- Modify: `src/lib/components/settings/Audio.svelte` (or create a new `TrainingCorpus.svelte` if cleaner)
- Modify: the parent settings dialog that registers tabs (locate via `grep -rn "Audio\|Sharing\|tab" src/lib/components/settings/ --include="*.svelte"`)
- Modify: the Tauri command that loads/saves AppConfig (may need to expose the new field through the existing path — usually no change required if the AppConfig flows through opaquely)

### Steps

- [ ] **Step 1: Locate the existing settings tab pattern**

  Read `src/lib/components/settings/Audio.svelte` to understand:
  - How config is loaded from the backend (probably `invoke('load_settings')` or similar)
  - How a single field's toggle is bound (probably `bind:checked` on a `<Switch>` or `<input type="checkbox">`)
  - How the change is persisted (probably an `invoke('save_settings', ...)`)

- [ ] **Step 2: Add the toggle row**

  In whichever settings tab feels most natural (Audio.svelte is fine for v1 since this relates to dictation flow; alternatively create a new `TrainingCorpus.svelte` tab):

  ```svelte
  <section class="settings-section">
    <h3>Training corpus capture</h3>
    <p class="settings-help">
      When enabled, the app records every SOAP generation along with your
      edited version into an encrypted on-device pool. Useful for later
      fine-tuning a model on your own dictation style. Data stays on this
      device; nothing is sent anywhere.
      <a href="#" on:click|preventDefault={openCorpusDocs}>Learn more</a>
    </p>
    <label class="settings-row">
      <input
        type="checkbox"
        bind:checked={config.capture_for_training}
        on:change={onSave}
      />
      <span>Capture generations for training corpus</span>
    </label>
  </section>
  ```

  Use whatever <Switch>/<Toggle> component the codebase already has. `openCorpusDocs` can be a stub for now (`alert('Documentation coming soon')`) or omit the "Learn more" link.

- [ ] **Step 3: Verify the field round-trips through the existing settings flow**

  The Tauri `save_settings`/`load_settings` (or equivalent) commands likely serialize the whole `AppConfig` opaquely, so the new field should round-trip without any backend changes. Verify by:

  - Reading whichever Tauri command saves settings (`grep -rn "save_settings\|update_settings\|fn.*save.*config" src-tauri/src/commands --include="*.rs"`).
  - Confirming it deserializes JSON into `AppConfig` and re-serializes on save (rather than passing only specific fields).

  If the command passes individual fields, add `capture_for_training` to its signature and ensure both directions handle it.

- [ ] **Step 4: Frontend smoke test**

  Run `npm run check` (svelte-check) to verify no type errors in the new component.

  Run: `npx vitest run` to confirm existing frontend tests still pass.

- [ ] **Step 5: Manual smoke test**

  Build and run the app:
  ```
  npm run tauri dev
  ```

  In the running app:
  1. Open Settings → Audio (or wherever the toggle is).
  2. Confirm "Capture generations for training corpus" is OFF.
  3. Record a short audio snippet and generate a SOAP.
  4. Inspect the DB: `sqlite3` against the encrypted DB isn't trivial (SQLCipher); easier path is to add a temporary debug log in the SOAP generation Tauri command that prints "captured: yes/no" and confirm "no" in this iteration.
  5. Enable the toggle. Record + generate again. Confirm "yes" appears in the log.
  6. Save the SOAP. Confirm a follow-up log line shows `update_final_text` succeeded.
  7. Remove the temporary debug logs before commit.

- [ ] **Step 6: Commit**

  ```bash
  git add src/lib/components/settings/Audio.svelte src/lib/components/settings/<other>
  git commit -m "feat(ui): add training-corpus capture toggle to settings

  Single boolean: 'Capture generations for training corpus'. Defaults
  to OFF; user must explicitly opt in. Field flows through the
  existing AppConfig save/load path."
  ```

---

## Final verification

- [ ] **Workspace test sweep**

  ```bash
  cargo test --workspace --lib
  cargo test -p medical-db
  cargo test -p medical-processing
  cargo test -p rust-medical-assistant
  ```

  Expected: all pass. Counts should be ≥43 baseline + new tests from Tasks 1, 2, 3, 4, 5.

- [ ] **Build verification**

  ```bash
  cargo build -p rust-medical-assistant --release
  ```

  Expected: clean. Release build catches issues that debug builds sometimes miss.

- [ ] **Schema sanity check**

  In a scratch script or via `sqlite3` against a fresh in-memory DB:

  ```bash
  cargo test -p medical-db --lib m004_generations
  ```

  Expected: pass — the table-existence and column tests from Task 1.

- [ ] **PHI policy check**

  ```
  git diff master..HEAD -- '*.rs' | grep -E "^\+.*tracing::(info|warn|error|debug)!"
  ```

  Expected: every new log line emits structural fields only (`generation_id`, `error`, `recording_id`) — no transcript content, no draft/final text. Read each carefully.

- [ ] **Note for the next plan**

  Phase 2 (Curate) builds on this. It needs:
  - List candidates / promoted / rejected (paginated)
  - Promote / reject / unpromote mutations
  - A new Svelte component with the three sub-views

  Phase 2 will add `list_*` and `set_corpus_status` methods to `GenerationsRepo`.

---

## Implementation handoff

After this plan is fully executed, the data layer is complete. The clinician can enable capture, generate SOAPs, save them, and the generations table fills with (draft, final, edit_distance) triples — but they have no way to view or curate them yet. That's Phase 2.

---

## Edit-save flow: manual smoke-test procedure

The edit-save flow (Tasks 8a–8d, added post-plan) wires `EditorTab` to call the
`save_recording_field` Tauri command on debounced edits. The round-trip
(recording update + generations `final_text` + `edit_distance`) cannot be fully
automated without a running Tauri runtime. Use the following steps to verify
the feature end-to-end:

1. **Enable capture** — Open Settings → Audio, turn on "Capture generations for
   training corpus".

2. **Generate a SOAP** — Create or select a recording, run transcription, then
   click Generate on the Generate tab. Confirm a SOAP appears.

3. **Open the editor** — Switch to the SOAP tab in the EditorTab. The text
   should now be editable (no `readonly` attribute).

4. **Edit a few words** — Change one or two phrases in the SOAP body.

5. **Watch the indicator** — After ~1 second of inactivity, "Saving…" should
   appear in the header, followed by "Saved" (green), which fades after ~1.5 s.

6. **Inspect the DB** — The encrypted SQLite DB is at
   `~/Library/Application Support/rust-medical-assistant/app.db` (macOS).
   Open with a SQLCipher-aware tool, or add a temporary `tracing::info!` line
   in `save_recording_field_inner` that emits `edit_distance` and
   `final_text.len()`.

   Expected after editing:
   - `recordings.soap_note` = the edited text
   - `generations.final_text` = the edited text (matching `recordings.soap_note`)
   - `generations.edit_distance` > 0 (non-zero, reflecting the number of changed
     words)
   - `generations.edit_ratio` between 0.0 and 1.0

7. **Verify cross-tab safety** — Edit the SOAP tab, immediately switch to the
   Transcript tab. The debounce timer should not fire a save on the Transcript
   field with the SOAP content.

8. **Verify capture-off path** — Disable the toggle, generate a fresh SOAP,
   edit it, wait for "Saved". Confirm no new row appears in `generations` for
   this recording (or the existing row's `final_text` remains NULL if it was
   captured during an earlier session).
