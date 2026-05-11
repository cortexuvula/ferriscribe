# Training Corpus — Phase 2: Curate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the manual review surface where the clinician promotes auto-captured generations into their training corpus (or rejects them). Builds on Phase 1's `generations` table + `GenerationsRepo`. No export yet — that's Phase 3.

**Architecture:** Two layers: (1) backend pagination + mutation methods on `GenerationsRepo`, wrapped in Tauri commands; (2) a new "Training Corpus" tab in the existing Settings dialog with three sub-views (Candidates, Promoted, Rejected), keyboard shortcuts, and a summary header.

**Tech Stack:** Rust workspace (extend existing repo + commands), Svelte 5 (new component). No new crates.

**Spec reference:** `docs/superpowers/specs/2026-05-11-training-corpus-design.md` — Phase 2 (Curate).

**Depends on:** Phase 1 (Capture) — the `generations` table and `GenerationsRepo` must exist before this plan can run.

---

## File Structure

**Created:**
- `src-tauri/src/commands/training_corpus.rs` — Tauri command module
- `src/lib/components/settings/TrainingCorpus.svelte` — new settings tab component
- `src/lib/components/settings/training_corpus/CandidatesList.svelte` — sub-view
- `src/lib/components/settings/training_corpus/PromotedList.svelte` — sub-view
- `src/lib/components/settings/training_corpus/RejectedList.svelte` — sub-view
- `src/lib/components/settings/training_corpus/GenerationCard.svelte` — per-row card used by all three lists

**Modified:**
- `crates/db/src/generations.rs` — add `list_by_status`, `set_corpus_status`, `count_by_status`
- `src-tauri/src/commands/mod.rs` — register the new command module
- `src-tauri/src/lib.rs` — register the new commands in the `tauri::Builder::invoke_handler`
- `src/lib/components/settings/SettingsDialog.svelte` (or wherever tabs are wired — locate via grep) — add the new tab

**No other files touched.**

---

## Task 1: Backend list + mutation methods

**Files:**
- Modify: `crates/db/src/generations.rs`

### Steps

- [ ] **Step 1: Add the list method with pagination**

  Append to `impl GenerationsRepo` in `crates/db/src/generations.rs`:

  ```rust
      /// List generations matching the given corpus_status, paginated
      /// by created_at DESC. Returns `(rows, total_count)` so the UI
      /// can show "N candidates" + "page X of Y" in one call.
      ///
      /// `limit` is capped to 200 to avoid loading absurd batches.
      pub fn list_by_status(
          conn: &Connection,
          status: &str,
          limit: u32,
          offset: u32,
      ) -> DbResult<(Vec<Generation>, u32)> {
          let limit = limit.min(200);

          let total: u32 = conn.query_row(
              "SELECT count(*) FROM generations WHERE corpus_status = ?",
              params![status],
              |r| r.get(0),
          )?;

          let mut stmt = conn.prepare(
              "SELECT id, recording_id, output_type, created_at, finalized_at,
                      ai_provider, ai_model, prompt_template_name,
                      input_transcript, input_context_json,
                      draft_text, final_text,
                      corpus_status, corpus_curated_at,
                      edit_distance, edit_ratio, regeneration_seq
               FROM generations
               WHERE corpus_status = ?
               ORDER BY created_at DESC
               LIMIT ? OFFSET ?",
          )?;
          let rows = stmt
              .query_map(params![status, limit, offset], Self::row_to_generation)?
              .filter_map(|r| r.ok())
              .collect();
          Ok((rows, total))
      }

      /// Counts per status, for the summary header. Returns
      /// `(candidates, promoted, rejected, excluded)`. Single query.
      pub fn count_by_status(conn: &Connection) -> DbResult<(u32, u32, u32, u32)> {
          let mut stmt = conn.prepare(
              "SELECT
                  SUM(CASE WHEN corpus_status='candidate' THEN 1 ELSE 0 END) AS c,
                  SUM(CASE WHEN corpus_status='promoted'  THEN 1 ELSE 0 END) AS p,
                  SUM(CASE WHEN corpus_status='rejected'  THEN 1 ELSE 0 END) AS r,
                  SUM(CASE WHEN corpus_status='excluded'  THEN 1 ELSE 0 END) AS e
               FROM generations",
          )?;
          let (c, p, r, e) = stmt.query_row([], |row| {
              Ok((
                  row.get::<_, Option<u32>>(0)?.unwrap_or(0),
                  row.get::<_, Option<u32>>(1)?.unwrap_or(0),
                  row.get::<_, Option<u32>>(2)?.unwrap_or(0),
                  row.get::<_, Option<u32>>(3)?.unwrap_or(0),
              ))
          })?;
          Ok((c, p, r, e))
      }

      /// Change a single row's corpus_status. Updates corpus_curated_at
      /// to now on every call (so unpromote → promote shows a fresh
      /// curation time, intentionally).
      ///
      /// Validates the input status; returns DbError on invalid value.
      pub fn set_corpus_status(
          conn: &Connection,
          id: Uuid,
          new_status: &str,
      ) -> DbResult<()> {
          if !matches!(new_status, "candidate" | "promoted" | "rejected" | "excluded") {
              return Err(DbError::Other(format!("invalid corpus_status: {new_status}")));
          }
          let affected = conn.execute(
              "UPDATE generations
                  SET corpus_status = ?,
                      corpus_curated_at = datetime('now')
                WHERE id = ?",
              params![new_status, id.to_string()],
          )?;
          if affected == 0 {
              return Err(DbError::Other(format!("generation {id} not found")));
          }
          Ok(())
      }
  ```

  Adjust `DbError::Other` to whichever variant exists in the crate (verify with `grep -n "enum DbError" crates/db/src/`). If only `Sqlite` exists, use `DbError::Sqlite(rusqlite::Error::QueryReturnedNoRows)` for the not-found case (less ideal but unblocking).

- [ ] **Step 2: Add tests**

  Append to the existing `mod tests` block in `generations.rs`. (Reuse the `migrated()` helper from Phase 1's tests; if it's not visible at the new test sites, copy it into a `fn` at the top of the test module.)

  ```rust
      #[test]
      fn list_by_status_returns_candidates_newest_first() {
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
              ai_model: "llama3",
              prompt_template_name: None,
              input_transcript: "t",
              input_context_json: None,
              draft_text: "d",
          };
          let _g1 = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
          // Force a different timestamp so ordering is deterministic.
          std::thread::sleep(std::time::Duration::from_millis(1100));
          let g2 = GenerationsRepo::record_generation(&conn, insert).unwrap();

          let (rows, total) =
              GenerationsRepo::list_by_status(&conn, "candidate", 10, 0).unwrap();
          assert_eq!(total, 2);
          assert_eq!(rows.len(), 2);
          assert_eq!(rows[0].id, g2.id, "newest first");
      }

      #[test]
      fn list_by_status_paginates() {
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
              ai_model: "llama3",
              prompt_template_name: None,
              input_transcript: "t",
              input_context_json: None,
              draft_text: "d",
          };
          for _ in 0..5 {
              GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
          }
          let (page1, total) = GenerationsRepo::list_by_status(&conn, "candidate", 2, 0).unwrap();
          assert_eq!(total, 5);
          assert_eq!(page1.len(), 2);
          let (page2, _) = GenerationsRepo::list_by_status(&conn, "candidate", 2, 2).unwrap();
          assert_eq!(page2.len(), 2);
          let (page3, _) = GenerationsRepo::list_by_status(&conn, "candidate", 2, 4).unwrap();
          assert_eq!(page3.len(), 1);
      }

      #[test]
      fn list_by_status_caps_limit_at_200() {
          let conn = migrated();
          let (rows, _) = GenerationsRepo::list_by_status(&conn, "candidate", 9999, 0).unwrap();
          assert_eq!(rows.len(), 0); // empty here, but limit-cap doesn't error
      }

      #[test]
      fn count_by_status_sums_all_buckets() {
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
              ai_model: "llama3",
              prompt_template_name: None,
              input_transcript: "t",
              input_context_json: None,
              draft_text: "d",
          };
          let g_cand = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
          let g_prom = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
          let g_rej = GenerationsRepo::record_generation(&conn, insert).unwrap();
          GenerationsRepo::set_corpus_status(&conn, g_prom.id, "promoted").unwrap();
          GenerationsRepo::set_corpus_status(&conn, g_rej.id, "rejected").unwrap();

          let (c, p, r, e) = GenerationsRepo::count_by_status(&conn).unwrap();
          assert_eq!(c, 1);
          assert_eq!(p, 1);
          assert_eq!(r, 1);
          assert_eq!(e, 0);

          // Sanity: original candidate row id is the un-promoted one.
          let _ = g_cand;
      }

      #[test]
      fn set_corpus_status_rejects_invalid_value() {
          let conn = migrated();
          let id = Uuid::new_v4();
          let err = GenerationsRepo::set_corpus_status(&conn, id, "favorited");
          assert!(err.is_err(), "should reject invalid status");
      }

      #[test]
      fn set_corpus_status_errors_when_id_missing() {
          let conn = migrated();
          let id = Uuid::new_v4();
          let err = GenerationsRepo::set_corpus_status(&conn, id, "promoted");
          assert!(err.is_err());
      }
  ```

- [ ] **Step 3: Run the tests**

  Run: `cargo test -p medical-db --lib generations`
  Expected: all (previous 5 + 6 new = 11) pass.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/db/src/generations.rs
  git commit -m "feat(db): GenerationsRepo list + count + set_corpus_status

  Adds the paginated list_by_status (capped at 200/page),
  count_by_status for the summary header, and set_corpus_status
  with input validation. Powers the Phase 2 curate UI."
  ```

---

## Task 2: Tauri command surface

**Files:**
- Create: `src-tauri/src/commands/training_corpus.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs` (register handlers)

### Steps

- [ ] **Step 1: Inspect existing command module patterns**

  Read one existing command module like `src-tauri/src/commands/recordings.rs` to understand:
  - The `#[tauri::command]` decoration
  - How `state: tauri::State<'_, AppState>` is wired
  - The DB-connection acquisition (`state.db.conn().map_err(...)?`)
  - The return-type convention (`AppResult<T>` or `Result<T, String>`)
  - How errors are converted

  Match the conventions exactly.

- [ ] **Step 2: Create the command module**

  Create `src-tauri/src/commands/training_corpus.rs`:

  ```rust
  //! Tauri commands for the training-corpus curate UI (Phase 2).
  //!
  //! Backend is GenerationsRepo (in crates/db). These commands wrap
  //! list/count/set_status with the AppState + error conversions.

  use medical_core::error::{AppError, AppResult};
  use medical_db::generations::{Generation, GenerationsRepo};
  use serde::Serialize;
  use uuid::Uuid;

  use crate::state::AppState;

  #[derive(Debug, Serialize)]
  pub struct CorpusCounts {
      pub candidates: u32,
      pub promoted: u32,
      pub rejected: u32,
      pub excluded: u32,
  }

  #[derive(Debug, Serialize)]
  pub struct GenerationPage {
      pub items: Vec<Generation>,
      pub total: u32,
  }

  #[tauri::command]
  pub async fn training_corpus_counts(
      state: tauri::State<'_, AppState>,
  ) -> AppResult<CorpusCounts> {
      let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
      let (c, p, r, e) = GenerationsRepo::count_by_status(&conn)
          .map_err(|e| AppError::Database(e.to_string()))?;
      Ok(CorpusCounts { candidates: c, promoted: p, rejected: r, excluded: e })
  }

  #[tauri::command]
  pub async fn training_corpus_list(
      state: tauri::State<'_, AppState>,
      status: String,
      limit: Option<u32>,
      offset: Option<u32>,
  ) -> AppResult<GenerationPage> {
      let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
      let (items, total) = GenerationsRepo::list_by_status(
          &conn,
          &status,
          limit.unwrap_or(50),
          offset.unwrap_or(0),
      )
      .map_err(|e| AppError::Database(e.to_string()))?;
      Ok(GenerationPage { items, total })
  }

  #[tauri::command]
  pub async fn training_corpus_set_status(
      state: tauri::State<'_, AppState>,
      id: String,
      new_status: String,
  ) -> AppResult<()> {
      let id = Uuid::parse_str(&id)
          .map_err(|e| AppError::Other(format!("invalid generation id: {e}")))?;
      let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
      GenerationsRepo::set_corpus_status(&conn, id, &new_status)
          .map_err(|e| AppError::Database(e.to_string()))?;
      Ok(())
  }
  ```

  Adjust error variants (`AppError::Database`, `AppError::Other`) to whichever exist in the project — check `crates/core/src/error.rs`.

- [ ] **Step 3: Register the module**

  In `src-tauri/src/commands/mod.rs`, find the existing `pub mod` lines and add:

  ```rust
  pub mod training_corpus;
  ```

  In `src-tauri/src/lib.rs`, find the `tauri::Builder::default().invoke_handler(tauri::generate_handler![...])` block. Add the three new commands to the list (alphabetical or grouped to match existing style):

  ```rust
  commands::training_corpus::training_corpus_counts,
  commands::training_corpus::training_corpus_list,
  commands::training_corpus::training_corpus_set_status,
  ```

- [ ] **Step 4: Build and test**

  Run: `cargo build -p rust-medical-assistant`
  Expected: clean.

  Run: `cargo test -p rust-medical-assistant --lib`
  Expected: existing tests still pass. (No direct unit tests for the Tauri commands themselves — the underlying GenerationsRepo methods are unit-tested.)

- [ ] **Step 5: Commit**

  ```bash
  git add src-tauri/src/commands/training_corpus.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs
  git commit -m "feat(commands): Tauri command surface for training-corpus curate

  Three commands: training_corpus_counts (summary header data),
  training_corpus_list (paginated by status), and
  training_corpus_set_status (promote/reject/etc.). Thin wrappers
  over GenerationsRepo with error-type conversion."
  ```

---

## Task 3: Frontend — TrainingCorpus tab shell + sub-view routing

**Files:**
- Create: `src/lib/components/settings/TrainingCorpus.svelte`
- Modify: the settings tab registry (locate via grep — likely `src/lib/components/settings/SettingsDialog.svelte` or a similar parent)

### Steps

- [ ] **Step 1: Locate the settings tab registry**

  Run: `grep -rn "tab.*Audio\|registerTab\|tabs.*=" src/lib/components/settings --include="*.svelte" --include="*.ts"`

  Read the parent file to understand how tabs are declared. Likely a `tabs = [...]` array of `{label, component}` or similar. Match that pattern.

- [ ] **Step 2: Create the shell component**

  Create `src/lib/components/settings/TrainingCorpus.svelte`:

  ```svelte
  <script lang="ts">
    import { onMount } from 'svelte';
    import { invoke } from '@tauri-apps/api/core';
    import CandidatesList from './training_corpus/CandidatesList.svelte';
    import PromotedList from './training_corpus/PromotedList.svelte';
    import RejectedList from './training_corpus/RejectedList.svelte';

    type CorpusCounts = {
      candidates: number;
      promoted: number;
      rejected: number;
      excluded: number;
    };

    let activeView: 'candidates' | 'promoted' | 'rejected' = $state('candidates');
    let counts: CorpusCounts = $state({ candidates: 0, promoted: 0, rejected: 0, excluded: 0 });
    let loading = $state(false);
    let error: string | null = $state(null);

    async function refreshCounts() {
      try {
        counts = await invoke<CorpusCounts>('training_corpus_counts');
      } catch (e) {
        error = String(e);
      }
    }

    onMount(refreshCounts);
  </script>

  <section class="training-corpus">
    <header class="tc-header">
      <h2>Training corpus</h2>
      <p class="tc-summary">
        {counts.candidates} candidate{counts.candidates === 1 ? '' : 's'} ·
        <strong>{counts.promoted}</strong> promoted ·
        {counts.rejected} rejected
      </p>
    </header>

    <nav class="tc-tabs" role="tablist">
      <button
        role="tab"
        aria-selected={activeView === 'candidates'}
        class:active={activeView === 'candidates'}
        onclick={() => (activeView = 'candidates')}
      >
        Candidates ({counts.candidates})
      </button>
      <button
        role="tab"
        aria-selected={activeView === 'promoted'}
        class:active={activeView === 'promoted'}
        onclick={() => (activeView = 'promoted')}
      >
        Promoted ({counts.promoted})
      </button>
      <button
        role="tab"
        aria-selected={activeView === 'rejected'}
        class:active={activeView === 'rejected'}
        onclick={() => (activeView = 'rejected')}
      >
        Rejected ({counts.rejected})
      </button>
    </nav>

    {#if error}
      <div class="tc-error">{error}</div>
    {/if}

    <div class="tc-view">
      {#if activeView === 'candidates'}
        <CandidatesList onchange={refreshCounts} />
      {:else if activeView === 'promoted'}
        <PromotedList onchange={refreshCounts} />
      {:else if activeView === 'rejected'}
        <RejectedList onchange={refreshCounts} />
      {/if}
    </div>
  </section>

  <style>
    .training-corpus { display: flex; flex-direction: column; gap: 1rem; padding: 1rem; }
    .tc-header h2 { margin: 0 0 0.25rem 0; }
    .tc-summary { color: var(--muted-foreground, #888); margin: 0; }
    .tc-tabs { display: flex; gap: 0.5rem; border-bottom: 1px solid var(--border, #ddd); }
    .tc-tabs button {
      background: none;
      border: none;
      padding: 0.5rem 1rem;
      cursor: pointer;
      border-bottom: 2px solid transparent;
      color: var(--muted-foreground, #888);
    }
    .tc-tabs button.active { color: var(--foreground, #222); border-bottom-color: var(--accent, #0066cc); }
    .tc-error { background: #fee; border: 1px solid #fbb; padding: 0.5rem; border-radius: 4px; }
    .tc-view { flex: 1; overflow: auto; }
  </style>
  ```

  Match the existing project's Svelte 5 conventions: `$state`, `$props`, `$derived` runes, `onclick` (not `on:click`), etc. If the project uses Svelte 4 syntax (`export let prop`, `$:` reactivity), adjust accordingly — check existing components first.

- [ ] **Step 3: Register the tab**

  In whichever parent file declares the settings tabs, add the new entry next to Audio/Sharing/etc.:

  ```typescript
  { id: 'training-corpus', label: 'Training Corpus', component: TrainingCorpus },
  ```

  Import: `import TrainingCorpus from './TrainingCorpus.svelte';`

- [ ] **Step 4: Type-check**

  Run: `npm run check`
  Expected: 0 errors. (Warnings about missing CSS variables, etc., are OK if they're consistent with the rest of the project.)

- [ ] **Step 5: Commit**

  ```bash
  git add src/lib/components/settings/TrainingCorpus.svelte src/lib/components/settings/<tabs-registry-file>
  git commit -m "feat(ui): TrainingCorpus settings tab shell

  Three sub-views (Candidates / Promoted / Rejected) with a summary
  header showing counts from training_corpus_counts. Sub-view
  components stubbed in next commit."
  ```

---

## Task 4: `GenerationCard` and `CandidatesList`

**Files:**
- Create: `src/lib/components/settings/training_corpus/GenerationCard.svelte`
- Create: `src/lib/components/settings/training_corpus/CandidatesList.svelte`

### Steps

- [ ] **Step 1: Build the `GenerationCard` component**

  Create `src/lib/components/settings/training_corpus/GenerationCard.svelte`:

  ```svelte
  <script lang="ts">
    type Generation = {
      id: string;
      recording_id: string;
      created_at: string;
      draft_text: string;
      final_text: string | null;
      ai_model: string;
      edit_ratio: number | null;
      regeneration_seq: number;
    };

    type Props = {
      generation: Generation;
      onAction: (id: string, action: 'promote' | 'reject' | 'unpromote' | 'restore') => void;
      mode: 'candidate' | 'promoted' | 'rejected';
    };
    let { generation, onAction, mode }: Props = $props();

    function previewOf(text: string | null, max = 150): string {
      if (!text) return '(no saved version — rejected draft)';
      if (text.length <= max) return text;
      return text.slice(0, max).trimEnd() + '…';
    }

    function editRatioChip(): { label: string; cls: string } {
      if (generation.final_text === null) return { label: 'no save', cls: 'chip-red' };
      const r = generation.edit_ratio;
      if (r === null) return { label: 'computing…', cls: 'chip-gray' };
      if (r < 0.15) return { label: 'light edit', cls: 'chip-green' };
      if (r < 0.4) return { label: 'moderate edit', cls: 'chip-yellow' };
      return { label: 'heavy edit', cls: 'chip-orange' };
    }

    let chip = $derived(editRatioChip());
  </script>

  <article class="gen-card">
    <header class="gen-card-header">
      <span class="gen-date">{new Date(generation.created_at).toLocaleString()}</span>
      <span class="gen-model">{generation.ai_model}</span>
      {#if generation.regeneration_seq > 1}
        <span class="gen-regen">#{generation.regeneration_seq}</span>
      {/if}
      <span class="chip {chip.cls}">{chip.label}</span>
    </header>

    <div class="gen-bodies">
      <div class="gen-half">
        <div class="gen-label">Draft</div>
        <div class="gen-preview">{previewOf(generation.draft_text)}</div>
      </div>
      <div class="gen-half">
        <div class="gen-label">Final</div>
        <div class="gen-preview">{previewOf(generation.final_text)}</div>
      </div>
    </div>

    <footer class="gen-actions">
      {#if mode === 'candidate'}
        <button class="action promote" onclick={() => onAction(generation.id, 'promote')}>Promote</button>
        <button class="action reject" onclick={() => onAction(generation.id, 'reject')}>Reject</button>
      {:else if mode === 'promoted'}
        <button class="action unpromote" onclick={() => onAction(generation.id, 'unpromote')}>Unpromote</button>
      {:else if mode === 'rejected'}
        <button class="action restore" onclick={() => onAction(generation.id, 'restore')}>Restore</button>
      {/if}
    </footer>
  </article>

  <style>
    .gen-card { border: 1px solid var(--border, #ddd); border-radius: 6px; padding: 0.75rem; margin-bottom: 0.5rem; }
    .gen-card-header { display: flex; gap: 0.5rem; align-items: center; font-size: 0.85rem; margin-bottom: 0.5rem; }
    .gen-date { color: var(--muted-foreground, #888); }
    .gen-model { font-family: var(--font-mono, monospace); font-size: 0.8rem; opacity: 0.7; }
    .gen-regen { background: #fef3c7; padding: 0.1rem 0.4rem; border-radius: 3px; font-size: 0.75rem; }
    .chip { padding: 0.1rem 0.5rem; border-radius: 10px; font-size: 0.75rem; }
    .chip-green { background: #d1fae5; color: #065f46; }
    .chip-yellow { background: #fef3c7; color: #92400e; }
    .chip-orange { background: #fed7aa; color: #9a3412; }
    .chip-red { background: #fecaca; color: #991b1b; }
    .chip-gray { background: #e5e7eb; color: #4b5563; }
    .gen-bodies { display: grid; grid-template-columns: 1fr 1fr; gap: 0.75rem; margin-bottom: 0.5rem; }
    .gen-half { background: var(--muted, #f5f5f5); padding: 0.5rem; border-radius: 4px; }
    .gen-label { font-size: 0.7rem; text-transform: uppercase; opacity: 0.7; margin-bottom: 0.25rem; }
    .gen-preview { white-space: pre-wrap; font-size: 0.85rem; line-height: 1.4; }
    .gen-actions { display: flex; gap: 0.5rem; }
    .action { padding: 0.35rem 0.8rem; border-radius: 4px; border: 1px solid; cursor: pointer; font-size: 0.85rem; }
    .promote { background: #059669; color: white; border-color: #059669; }
    .reject { background: white; color: #dc2626; border-color: #dc2626; }
    .unpromote { background: white; color: #6b7280; border-color: #d1d5db; }
    .restore { background: white; color: #0066cc; border-color: #0066cc; }
  </style>
  ```

- [ ] **Step 2: Build `CandidatesList`**

  Create `src/lib/components/settings/training_corpus/CandidatesList.svelte`:

  ```svelte
  <script lang="ts">
    import { onMount } from 'svelte';
    import { invoke } from '@tauri-apps/api/core';
    import GenerationCard from './GenerationCard.svelte';

    type Generation = {
      id: string;
      recording_id: string;
      created_at: string;
      draft_text: string;
      final_text: string | null;
      ai_model: string;
      edit_ratio: number | null;
      regeneration_seq: number;
    };
    type Page = { items: Generation[]; total: number };

    let { onchange }: { onchange: () => void } = $props();

    let items: Generation[] = $state([]);
    let total = $state(0);
    let loading = $state(false);
    let error: string | null = $state(null);
    let cursorIndex = $state(0); // for keyboard navigation
    const PAGE_SIZE = 50;
    let offset = $state(0);

    async function load() {
      loading = true;
      error = null;
      try {
        const page = await invoke<Page>('training_corpus_list', {
          status: 'candidate',
          limit: PAGE_SIZE,
          offset,
        });
        items = page.items;
        total = page.total;
        cursorIndex = Math.min(cursorIndex, items.length - 1);
      } catch (e) {
        error = String(e);
      } finally {
        loading = false;
      }
    }

    async function act(id: string, action: 'promote' | 'reject' | 'unpromote' | 'restore') {
      const new_status =
        action === 'promote' ? 'promoted' :
        action === 'reject' ? 'rejected' :
        action === 'unpromote' || action === 'restore' ? 'candidate' :
        'candidate';
      try {
        await invoke('training_corpus_set_status', { id, newStatus: new_status });
        await load();
        onchange?.();
      } catch (e) {
        error = String(e);
      }
    }

    function onKey(ev: KeyboardEvent) {
      if (loading || items.length === 0) return;
      const key = ev.key.toLowerCase();
      if (key === 'j') { cursorIndex = Math.min(cursorIndex + 1, items.length - 1); ev.preventDefault(); }
      else if (key === 'k') { cursorIndex = Math.max(cursorIndex - 1, 0); ev.preventDefault(); }
      else if (key === 'p') { act(items[cursorIndex].id, 'promote'); ev.preventDefault(); }
      else if (key === 'r') { act(items[cursorIndex].id, 'reject'); ev.preventDefault(); }
      else if (key === 's') { cursorIndex = Math.min(cursorIndex + 1, items.length - 1); ev.preventDefault(); }
    }

    onMount(load);
  </script>

  <svelte:window onkeydown={onKey} />

  <div class="candidates-list">
    {#if loading}<div class="info">Loading…</div>{/if}
    {#if error}<div class="error">{error}</div>{/if}
    {#if !loading && items.length === 0}
      <div class="empty">No candidates. Generate a SOAP note with capture enabled to populate this list.</div>
    {/if}

    {#each items as g, i (g.id)}
      <div class:cursor-row={i === cursorIndex} class="row-wrap">
        <GenerationCard generation={g} mode="candidate" onAction={act} />
      </div>
    {/each}

    {#if total > PAGE_SIZE}
      <nav class="pagination">
        <button disabled={offset === 0} onclick={() => { offset = Math.max(0, offset - PAGE_SIZE); load(); }}>← Prev</button>
        <span>{offset + 1}–{Math.min(offset + PAGE_SIZE, total)} of {total}</span>
        <button disabled={offset + PAGE_SIZE >= total} onclick={() => { offset += PAGE_SIZE; load(); }}>Next →</button>
      </nav>
    {/if}

    <p class="kbd-hint">
      <kbd>J</kbd>/<kbd>K</kbd> navigate · <kbd>P</kbd> promote · <kbd>R</kbd> reject · <kbd>S</kbd> skip
    </p>
  </div>

  <style>
    .candidates-list { display: flex; flex-direction: column; gap: 0.25rem; }
    .info, .empty { padding: 1rem; color: var(--muted-foreground, #888); }
    .error { padding: 0.5rem; background: #fee; color: #991b1b; border-radius: 4px; }
    .row-wrap { padding: 0.15rem; border-radius: 6px; }
    .cursor-row { background: rgba(0,102,204,0.08); }
    .pagination { display: flex; gap: 1rem; align-items: center; padding: 0.5rem; }
    .pagination button { padding: 0.35rem 0.75rem; cursor: pointer; }
    .kbd-hint { font-size: 0.8rem; color: var(--muted-foreground, #888); }
    kbd { background: var(--muted, #f5f5f5); border: 1px solid var(--border, #ccc); border-bottom-width: 2px; padding: 0.1rem 0.35rem; border-radius: 3px; font-family: var(--font-mono, monospace); font-size: 0.8rem; }
  </style>
  ```

  Notes:
  - The `newStatus` (camelCase) is how Tauri exposes Rust's `new_status` parameter to the frontend. Verify this convention by checking how existing commands' snake_case parameters are invoked from the frontend.
  - The keyboard shortcuts assume the user is focused inside the settings dialog — the `svelte:window` binding fires globally. If this causes conflicts with other dialogs, scope to a div with `tabindex={0}` and `onkeydown` instead.

- [ ] **Step 3: Type-check**

  Run: `npm run check`
  Expected: 0 errors.

- [ ] **Step 4: Manual smoke test**

  Build + run:
  ```
  npm run tauri dev
  ```

  Pre-populate some data by enabling the capture toggle (from Phase 1), generating a SOAP, and saving. Then open Settings → Training Corpus → Candidates. Confirm:
  - The card renders with date, model, draft preview, final preview, and edit-ratio chip
  - Clicking Promote moves the card out (refetch shows 0 remaining)
  - Clicking Reject does the same for the Rejected view
  - Keyboard shortcuts (J/K/P/R) work

- [ ] **Step 5: Commit**

  ```bash
  git add src/lib/components/settings/training_corpus/
  git commit -m "feat(ui): CandidatesList with GenerationCard + keyboard shortcuts

  Per-card draft/final previews (~150 chars), edit-ratio chip
  (light/moderate/heavy/no-save), Promote and Reject actions.
  Keyboard nav: J/K navigate, P/R/S act. Pagination at 50 per page."
  ```

---

## Task 5: `PromotedList` and `RejectedList`

**Files:**
- Create: `src/lib/components/settings/training_corpus/PromotedList.svelte`
- Create: `src/lib/components/settings/training_corpus/RejectedList.svelte`

### Steps

- [ ] **Step 1: Build `PromotedList`**

  Create `src/lib/components/settings/training_corpus/PromotedList.svelte` — structurally identical to `CandidatesList` from Task 4, but:

  - Pass `mode="promoted"` to `GenerationCard`
  - Status to query: `'promoted'`
  - On action `'unpromote'`, transition status back to `'candidate'`
  - Keyboard shortcuts: J/K to navigate, U to unpromote (don't reuse P/R from candidates here)
  - Empty-state message: "No promoted candidates yet. Promote a candidate to add it to the training corpus."

  Reuse the CSS classes; the file structure is the same shape. Copy the file and adapt these four points.

- [ ] **Step 2: Build `RejectedList`**

  Similarly, create `RejectedList.svelte`:

  - `mode="rejected"`
  - Status: `'rejected'`
  - Action `'restore'` → status `'candidate'`
  - Shortcuts: J/K + R (restore)
  - Empty-state: "No rejected candidates."

- [ ] **Step 3: Type-check + smoke test**

  Run: `npm run check`

  Manual: pre-populate via the Candidates flow, then verify each tab shows the correct list and that Unpromote / Restore round-trip back to Candidates.

- [ ] **Step 4: Commit**

  ```bash
  git add src/lib/components/settings/training_corpus/PromotedList.svelte src/lib/components/settings/training_corpus/RejectedList.svelte
  git commit -m "feat(ui): PromotedList + RejectedList sub-views with un-promote / restore"
  ```

---

## Final verification

- [ ] **Workspace test sweep**

  ```bash
  cargo test --workspace --lib
  ```

  Expected: pass. New tests from Task 1 add to the count.

- [ ] **Frontend type-check + tests**

  ```bash
  npm run check
  npx vitest run
  ```

  Expected: 0 errors / 0 failures.

- [ ] **End-to-end manual flow**

  1. Build the app: `npm run tauri dev`
  2. Settings → Training Corpus (assuming this tab is wired). Should show all counts 0 if no data yet.
  3. Settings → Audio → enable capture. Record + generate + save a SOAP.
  4. Re-visit Training Corpus → Candidates. The new generation row should appear with date/model/preview/chip.
  5. Promote it. Counts update: candidates → 0, promoted → 1.
  6. Switch to Promoted tab. The same row is now there with an Unpromote button.
  7. Unpromote. It moves back to Candidates.
  8. Reject from Candidates. Counts: candidates 0, rejected 1.
  9. Restore from Rejected. Back to Candidates.

- [ ] **PHI policy spot-check**

  ```
  git diff master..HEAD -- '*.rs' '*.svelte' | grep -E "^\+.*tracing::|^\+.*console\."
  ```

  Expected: no new log lines that emit transcript or SOAP content. Frontend code may render draft/final previews into the DOM — that's user-facing UI, not logs, and is fine.

- [ ] **Note for the next plan**

  Phase 3 (Export) builds on Phase 2. It will:
  - Add a redaction layer extending `PhiRedactor` with patient-name + datetime patterns
  - Add a Tauri command that filters promoted rows, applies redaction, writes JSONL + manifest + README
  - Add an Export button + dialog into the PromotedList view

---

## Implementation handoff

After this plan completes, the clinician can capture, view, curate, promote, reject, and unpromote — but they can't yet export. That's Phase 3.
