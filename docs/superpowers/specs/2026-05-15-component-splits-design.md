# Component Splits — Design

**Date:** 2026-05-15
**Branch:** `component-splits`
**Predecessor:** Tier 3 sharing-crate test backfill (`e972654`)

## Goal

Behavior-preserving split of two oversized Svelte components into composed sub-components, so each piece is easier to hold in mind and independently editable. No new features, no logic changes, no behavioral changes — pure refactor verified by existing test suite + svelte-check.

## Scope

Two parent components, 7 new children, 2 shells slimmed:

### A. VocabularyDialog.svelte (728 → ~280 lines)

Splits into shell + 3 children:

1. **`VocabularyDialog.svelte`** (shell, ~280 lines) — keeps: dialog overlay, header, toolbar (filter+search+add — too small to extract), footer (count+deleteAll), escape handler, entries fetch, CRUD orchestration, conditional rendering of children.
2. **`VocabularyForm.svelte`** (NEW, ~140 lines) — owns its own form state (`formFind`, `formReplace`, `formCategory`, `formCaseSensitive`, `formPriority`, `formEnabled`, `formError`). Props: `editing: VocabularyEntry | null`, `categories`, callbacks `onSave(values)` and `onCancel`. Resets state when `editing` changes via `$effect`.
3. **`VocabularyTable.svelte`** (NEW, ~110 lines) — pure presentation. Props: `entries`, `loading`, `searchText`, `categoryLabel(value)`, callbacks `onEdit(entry)`, `onDelete(entry)`, `onToggleEnabled(entry)`. Filters by `searchText` locally.
4. **`VocabularyTestPanel.svelte`** (NEW, ~85 lines) — owns its own state (`testInput`, `testResult`, `testError`, `testing`). One prop `resetSignal: number` — parent increments to force reset on dialog open.

### B. settings/Audio.svelte (692 → ~260 lines)

Splits into shell + 4 children:

1. **`Audio.svelte`** (shell, ~260 lines) — keeps: top-level state (audioDevices, whisper/pyannote models, downloadingModel, downloadProgress, sttMode), onMount/onDestroy lifecycle, model fetch functions, download/delete callbacks, STT-mode radio toggle, capture toggles (auto-SOAP + capture-for-training — small enough to keep), conditional rendering.
2. **`AudioInputSection.svelte`** (NEW, ~60 lines) — Input Device picker + Sample Rate. Props: `audioDevices`, `devicesLoading`, plus `currentInputDevice`/`currentSampleRate` from settings store via direct import.
3. **`WhisperLocalSection.svelte`** (NEW, ~110 lines) — Whisper model picker + management list. Visible when `sttMode === 'local'`. Props: `whisperModels`, `currentModelId`, `modelsRefreshing`, `downloadingModel`, `downloadProgress`, callbacks `onModelChange(id)`, `onDownload(id)`, `onDelete(id)`, `formatBytes(bytes)`.
4. **`SttRemoteSection.svelte`** (NEW, ~150 lines) — host/port/model/API key/test buttons. Visible when `sttMode === 'remote'`. Owns its own form state (`sttRemoteApiKey`, `sttRemoteTestStatus`, `sttRemoteTestMessage`). Pulls settings via direct store import.
5. **`DiarizationModelsSection.svelte`** (NEW, ~70 lines) — pyannote model list (always visible regardless of STT mode). Props: `pyannoteModels`, `downloadingModel`, `downloadProgress`, callbacks `onDownload(id)`, `onDelete(id)`, `formatBytes(bytes)`.

## Architecture decisions

### Where state lives

- **Parent owns state that crosses children:** entries list, loading flag, model arrays, download progress, STT mode.
- **Children own state that's local to their concern:** form field values, test panel input, STT-remote api-key/test status.
- **Settings store accessed directly:** children that need `$settings.xxx` import the store at the top — no prop drilling for global app state.

### Why the "small parts stay" exceptions

- **VocabularyToolbar** (~40 lines: filter select, search input, add button) — only 2 pieces of state, 3 elements, no internal logic. Extracting creates more files than it saves complexity.
- **AudioCaptureToggles** (~50 lines: two checkboxes) — pure read-then-call-update on settings store, no shared logic. Same rationale.

### CSS scoping strategy

Svelte's `<style>` is scoped per component, so styles travel with the markup they target. Mapping:

**VocabularyDialog shell keeps:**
- `.vocab-overlay`, `.vocab-dialog`, `.vocab-header`, `.btn-close`
- `.vocab-toolbar`, `.filter-select`, `.search-input`, `.btn-add`
- `.vocab-body` (single scroll container)
- `.vocab-footer`, `.footer-count`, `.btn-delete-all`

**VocabularyForm gets:**
- `.vocab-form`, `.form-header`, `.btn-close-form`, `.form-error`, `.form-grid`, `.field`, `.form-toggles`, `.vocab-toggle`, `.toggle-text`, `.form-actions`, `.btn-save`, `.btn-cancel`

**VocabularyTable gets:**
- `.vocab-table-wrap`, `.loading-text`, `.empty-text`, `.vocab-table`, `.mono`, `.truncate`, `.col-category`, `.col-enabled`, `.col-actions`, `.actions`, `.btn-edit`, `.btn-delete`
- The "global" `.vocab-dialog input[type="checkbox"]` override (line 554 of current file) — table is the remaining checkbox host (form's checkboxes use `.vocab-toggle` already). Restyled in the child as `input[type="checkbox"] { ... }`.

**VocabularyTestPanel gets:**
- `.vocab-test`, `.btn-test`, `.test-error`, `.test-result`

For Audio.svelte the model-row styling (`.model-list`, `.model-row`, `.model-info`, `.model-name`, etc.) is used by BOTH WhisperLocalSection AND DiarizationModelsSection. Two options:
- Duplicate the rules in both children (~40 lines of CSS duplication)
- Share via a global CSS module or `:global()` selectors in shell

**Decision:** duplicate. Svelte scoping means duplication is the cleanest path; it's a one-time cost. The shared shape isn't going to drift independently because they're visually identical.

### Type stability

`VocabularyEntry`, `CorrectionResult`, `AudioDevice`, `ModelInfo` types come from the `api/` modules — no new types created. Children import the same types as the parent currently does.

## Invariants — DO NOT change

- Keyboard escape closes the dialog (already done via `window.addEventListener` in shell).
- Form validation: `find_text` and `replacement` required (currently in `handleSave`).
- Toggle confirmations (delete confirm, delete-all confirm).
- Settings persistence via `settings.updateField(...)`.
- Model download progress event listener (`'model-download-progress'`) installed in shell `onMount`, removed in `onDestroy`.
- Endpoint policy warning when STT remote host is public.
- `reinitProviders()` called after any STT settings change.
- Auto-toggle when sample rate / input device changes — handlers still call `settings.updateField`.

## Acceptance criteria

- All existing tests pass: `npx vitest run` → 233 passed.
- `npm run check` (svelte-check) clean — no new errors.
- No PHI introduced (UI labels/placeholders contain none today; preserve).
- Each new component file < 200 lines (most well under).
- VocabularyDialog.svelte ≤ 300 lines after split.
- Audio.svelte ≤ 280 lines after split.
- 10–11 logical commits: one per extracted child + one for each parent finalization + one final verification.

## Risk register

- **Visual regression** is the main risk since no automated component tests cover these dialogs. Mitigation: preserve exact markup and class names byte-for-byte; preserve CSS rules verbatim.
- **CSS scope drift** between parent and child after extraction. Mitigation: per-task implementer checklist verifies all classes used in extracted markup exist in extracted styles.
- **Event handler binding loss** (e.g., `onclick={() => handleDelete(entry)}` becomes prop-driven). Mitigation: each task explicitly checks all click handlers fire via vitest's existence + by inspecting the call chain in the diff.

## Out of scope

- New component-level tests (e.g., `VocabularyForm.test.ts`) — covered by a future test-backfill plan if desired.
- Behavior changes to validation, error messages, or styling.
- Changes to API surface, type definitions, or store contracts.
- Browser-driven smoke testing (subagents can't drive a browser; user spot-check post-merge).
