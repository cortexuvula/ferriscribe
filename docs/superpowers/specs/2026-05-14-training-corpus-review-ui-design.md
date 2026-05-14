# Training Corpus Review UI — Redesign

**Status:** Draft — pending user review
**Date:** 2026-05-14
**Author:** Brainstorming session with Claude Code
**Related:** [2026-05-11-training-corpus-design.md](./2026-05-11-training-corpus-design.md)

## Goal

Make it materially easier for a clinician to triage Training Corpus candidates by replacing the current side-by-side preview cards with a master+detail layout that surfaces a real diff between the AI draft and the clinician's saved final version.

## Problem

The current Candidates tab (Settings → Training Corpus) renders each generation as a single row with two 150-character preview boxes labelled "DRAFT" and "FINAL", followed by Promote / Reject buttons. For "light edit" cases — where the clinician changed a word or two — both preview boxes show essentially the same text, and the reviewer cannot see what changed without expanding the rows manually. Decisions become guesses.

Concretely: in the screenshot that motivated this work, the only visible difference between draft and final is the chip "light edit" — the two text panels are visually identical for the first 150 characters of a Subjective. Promote/Reject without seeing the actual edits is not a meaningful decision.

## Non-goals

- **Server-side or backend rework.** The DB schema and the `training_corpus_list` / `training_corpus_set_status` contracts stay essentially the same; one filter change (see Data flow).
- **Word-level diff rendering.** Considered and rejected during brainstorming — line-level unified diff is the chosen default. A word-level or split-view toggle is future work.
- **Inline editing of the final version.** Reviewer is auditing past edits, not making new ones.
- **Bulk operations.** One candidate at a time. Future work could add multi-select.
- **Diff rendering for the Generation/Export flow.** Out of scope — this is purely the curation UI.

## Hard constraints honored

- **Local-only.** Diff is computed in the renderer; no remote call.
- **No PHI in logs.** No `tracing` or `console.log` of draft/final text, snippets, or diff hunks introduced.
- **No new runtime dependencies on hosted services.** One new npm dep (`diff`, jsdiff) for line-level diff computation.

## Decisions captured from brainstorming

| Question | Choice |
|---|---|
| Where does review happen? | **Master + detail split** (list on left, full diff on right) |
| How is the diff rendered? | **Line-level unified diff** (`+`/`−` lines, like git) |
| What does each master row show? | **Metadata + first changed line pair** (date, model, edit chip, one `−`/`+` snippet) |
| Apply to all tabs? | **Yes — Candidates, Promoted, Rejected all use the same layout.** Promoted/Rejected detail panes show Unpromote / Restore actions instead of Promote / Reject. |
| Null `final_text` candidates? | **Excluded from Candidates queue server-side.** Still visible in Promoted/Rejected for audit. |

## Architecture overview

```
src/lib/components/settings/training_corpus/
├── ReviewLayout.svelte        NEW — owns master+detail split, selection, keyboard nav
├── MasterRow.svelte           NEW — compact row (metadata + first-change snippet)
├── DetailPane.svelte          NEW — sticky header + scrollable diff + action footer
├── diff.ts                    NEW — pure helpers around jsdiff
│
├── CandidatesList.svelte      MODIFIED — becomes a thin wrapper over ReviewLayout
├── PromotedList.svelte        MODIFIED — becomes a thin wrapper over ReviewLayout
├── RejectedList.svelte        MODIFIED — becomes a thin wrapper over ReviewLayout
├── GenerationCard.svelte      DELETED — no longer used
└── (other files unchanged)
```

`ReviewLayout` is parameterized by `mode: 'candidate' | 'promoted' | 'rejected'`. The mode determines:
- Which status string is passed to `training_corpus_list`
- Which actions render in the detail-pane header (Promote/Reject vs Unpromote vs Restore)
- The keyboard shortcuts on offer:
  - **Candidates:** J/K navigate, P promote, R reject, S skip (unchanged from today)
  - **Promoted:** J/K navigate, U unpromote
  - **Rejected:** J/K navigate, R restore (R is unambiguous here because Promote/Reject are not available in this mode)

## Component contracts

### `diff.ts`

```ts
export type DiffLine =
  | { kind: 'context'; text: string }
  | { kind: 'add'; text: string }
  | { kind: 'remove'; text: string };

// Memoized at the module level by (draft, final) reference equality
export function diffLines(draft: string, final: string): DiffLine[];

// Returns null if there is no change, or if final is null/empty
export function firstChangeSnippet(
  draft: string,
  final: string | null,
): { removed: string | null; added: string | null } | null;
```

Implementation notes:
- Uses jsdiff's `diffLines` with `{ newlineIsToken: false }` and trims trailing whitespace per line before comparing (SOAP notes occasionally have inconsistent trailing newlines that would create spurious diffs).
- `firstChangeSnippet` walks the diff result and returns the first contiguous `remove`/`add` pair (or single side if only one exists), truncated to ~60 chars each.
- Pure functions; no Svelte runes; trivially unit-testable.

### `MasterRow.svelte`

Props:
```ts
{
  generation: Generation;
  selected: boolean;
  onclick: () => void;
}
```

Renders:
- Date (short format, e.g. "May 13, 10:57 PM")
- Edit chip (`light` / `moderate` / `heavy` / `no save` / `computing…` — same buckets as today's `editRatioChip`)
- Model name (monospace, smaller)
- Snippet box: one `−` line and one `+` line in monospace, color-coded, no background fill (saves visual weight)
- Selected state: 3px left border in accent color, slight background tint

Width ~240px. No buttons in the row (actions live in the detail pane).

### `DetailPane.svelte`

Props:
```ts
{
  generation: Generation | null;
  mode: 'candidate' | 'promoted' | 'rejected';
  loading: boolean;
  onAction: (id: string, action: 'promote' | 'reject' | 'unpromote' | 'restore') => void;
}
```

Layout:
- **Header (sticky top):** date + time, model, edit chip with % changed, action buttons aligned right.
- **Body (scrollable):** line-level diff. Context lines neutral; removed lines red background, added lines green background.
- **Footer:** position indicator ("Candidate N of M") and keyboard shortcut hints.

Empty state (when `generation === null`): centered message "Nothing to review."

Loading state (when `loading === true`): keep current content, dim the body. Avoids flicker during pagination.

### `ReviewLayout.svelte`

Props:
```ts
{
  mode: 'candidate' | 'promoted' | 'rejected';
  onchange?: () => void;  // bubble to parent so tab counts refresh
}
```

State:
- `items: Generation[]`, `total: number`, `offset: number`, `loading: boolean`, `error: string | null` (same as today's `CandidatesList`)
- `selectedId: string | null` — the id of the currently focused master row

Derived:
- `cursorIndex = items.findIndex(i => i.id === selectedId)` — single source of truth for keyboard nav
- `selected = items.find(i => i.id === selectedId) ?? null` — passed to `DetailPane`

Selection rules:
- On `load()` complete, if `selectedId` is no longer in `items`, snap to `items[0]?.id`.
- After a successful action, `load()` runs; `selectedId` snaps to the new item at the previous `cursorIndex` (or the previous one if we hit the end).

## Data flow

```
ReviewLayout.onMount
  → invoke('training_corpus_list', { status, limit, offset })
  → items = page.items, selectedId = items[0]?.id

User clicks MasterRow OR presses J/K
  → selectedId updates
  → DetailPane re-renders with new generation

DetailPane.actionClick (or P/R keypress)
  → invoke('training_corpus_set_status', { id, newStatus })
  → ReviewLayout.load() re-fires
  → onchange() bubbles up to parent (TrainingCorpus.svelte)
  → tab counts refresh

User presses J at last row of current page (and total > offset + items.length)
  → offset += PAGE_SIZE
  → load()
  → selectedId = items[0]?.id (top of new page)

User presses K at first row of non-first page
  → offset = max(0, offset - PAGE_SIZE)
  → load()
  → selectedId = items[items.length - 1]?.id (bottom of new page)
```

## Backend change

One small modification: in `src-tauri/src/training_corpus.rs` (or wherever `training_corpus_list` lives), the query for `status = 'candidate'` should add `AND final_text IS NOT NULL`. Promoted and Rejected queries are unchanged.

Migration: none. Existing null-final candidate rows simply stop appearing in the Candidates tab. If the user later promotes/rejects them via some other means (or they remain in limbo), they're still in the DB; they just don't clog the active review queue.

## Error handling

| Scenario | Behavior |
|---|---|
| `training_corpus_list` fails | Banner above the master+detail grid; `items` cleared; detail pane empty. (Same surface as today.) |
| `training_corpus_set_status` fails | Banner; selection preserved; action buttons re-enabled. |
| Diff computation throws (shouldn't happen with jsdiff but defensive) | Detail body shows "Could not render diff — Draft / Final shown below" and falls back to the existing two-column preview. |
| Empty queue | "Nothing to review. Generate a SOAP note with capture enabled." Both master and detail panes show variants of this. |
| Action in flight | Promote/Reject/Unpromote/Restore buttons disabled; `loading=true` gates keyboard shortcuts. (Same as today.) |
| Pagination edge | J at last item of last page is a no-op; K at first item of first page is a no-op. |

## Width & responsiveness

The Settings dialog is capped at 960px (`Modal.svelte`), with a 130px sidebar — so ~800px usable. Layout: 240px master, 1px divider, ~559px detail. Tight but workable for a line-level diff at 12px monospace (~70 char lines).

No responsive breakpoint needed for this MVP — the dialog itself doesn't scale down meaningfully. If future work changes the dialog width, the grid `240px 1fr` adapts naturally.

## Testing

### Frontend (Vitest)

- **`diff.ts` unit tests**
  - Identical inputs → empty diff array (only context lines).
  - Pure insertion: draft empty → all `add` lines.
  - Pure deletion: final empty → all `remove` lines.
  - Mixed edit: spot-check `+`/`−` counts.
  - Trailing whitespace normalization: `"foo\n"` vs `"foo"` → no diff.
  - `firstChangeSnippet`: identical inputs → `null`. Mixed → returns first `−`/`+` pair. Pure insertion → `{ removed: null, added: "..." }`. Null final → `null`.

- **`MasterRow.svelte`**
  - Renders date, model, chip class for each edit-ratio bucket.
  - Selected styling applies when `selected=true`.
  - Calls `onclick` on click.

- **`DetailPane.svelte`**
  - Renders sticky header + diff body when `generation` provided.
  - Renders empty state when `generation === null`.
  - Action buttons disabled when `loading=true`.
  - Mode `candidate` shows Promote + Reject; `promoted` shows Unpromote; `rejected` shows Restore.

- **`ReviewLayout.svelte`** (integration)
  - Mounting selects `items[0]`.
  - J/K within a page updates selection.
  - J at last row of multi-page result advances page and selects index 0.
  - K at first row of non-first page rewinds page and selects last index.
  - P/R triggers the action with the currently selected id (mode='candidate' only).
  - After a successful action, `load()` re-runs and selection moves forward.
  - Error during action surfaces banner, selection preserved.

### Backend (Rust)

- `training_corpus_list` with `status='candidate'` excludes rows where `final_text IS NULL`. Insert one null-final and one non-null candidate row, assert only the latter is returned and `total` reflects this.
- `training_corpus_list` with `status='promoted'` and `status='rejected'` still returns null-final rows.

### Manual smoke (per CLAUDE.md)

- Generate a SOAP note with capture enabled, edit it, save it.
- Open Settings → Training Corpus → Candidates.
- Verify the master row shows the metadata + first-change snippet.
- Click the row; verify diff renders with `+`/`−` highlighting.
- Promote with mouse, then with keyboard (P). Verify count updates and selection moves.
- Repeat for Reject (R).
- Switch to Promoted tab, verify same layout with Unpromote action.
- Switch to Rejected tab, verify Restore action.
- Generate 50+ candidates to test pagination; verify J at bottom of page 1 advances to page 2.

## Open questions

None blocking. Future iterations might add:
- Word-level inline diff toggle (B from the brainstorming — useful when only one word changed).
- Split side-by-side toggle (C — useful for restructured sections).
- "Diff stats" line in the header ("3 sections changed, +2 lines, −1 line").
- Section-aware diff that respects SOAP boundaries (Subjective / Objective / Assessment / Plan headers as collapse points).

## Implementation order

1. **`diff.ts`** + tests — pure, no UI dependency.
2. **Backend filter** + Rust test — small SQL change, independent.
3. **`MasterRow.svelte`** + tests — uses `diff.ts`.
4. **`DetailPane.svelte`** + tests — uses `diff.ts`.
5. **`ReviewLayout.svelte`** + integration tests — composes the above.
6. **Wire into `CandidatesList`, `PromotedList`, `RejectedList`** — thin wrappers; delete `GenerationCard.svelte`.
7. **Manual smoke + screenshot.**

Each step is independently testable and reviewable.
