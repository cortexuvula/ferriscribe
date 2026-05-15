# Component Splits — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Behavior-preserving extraction of 7 new Svelte components from `VocabularyDialog.svelte` (728 lines) and `settings/Audio.svelte` (692 lines), shrinking each parent shell to a coordinator role.

**Architecture:** Svelte 5 runes throughout. Parents own cross-cutting state and orchestration. Children own state local to their concern. Pass data via props, mutations via callback props.

**Spec:** `docs/superpowers/specs/2026-05-15-component-splits-design.md`

**Worktree:** `.worktrees/component-splits` on branch `component-splits`

**Baseline:** `npx vitest run` → 233 passed. `npm run check` → clean.

**Per-task verification recipe:**
1. After every file create/edit: `npx vitest run` → expect 233 passed
2. After every task: `npm run check` → no new errors
3. Git diff sanity: only the files named in the task changed
4. Commit with the message shown in the task

---

## Task 1: Extract VocabularyForm

**Files:**
- Create: `src/lib/components/VocabularyForm.svelte`
- Modify: `src/lib/components/VocabularyDialog.svelte`

**What moves into VocabularyForm.svelte:**
- All `form*` state vars (`formFind`, `formReplace`, `formCategory`, `formCaseSensitive`, `formPriority`, `formEnabled`, `formError`) — declared inside child as `$state(...)`
- The `CATEGORIES` constant + `categoryLabel` helper IS NOT moved (still needed in shell for the table category column). Pass `CATEGORIES` as a prop.
- The form template block (current lines 254–298 of VocabularyDialog.svelte)
- CSS rules: `.vocab-form`, `.form-header`, `.btn-close-form`, `.form-error`, `.form-grid`, `.field`, `.form-toggles`, `.vocab-toggle`, `.toggle-text`, `.form-actions`, `.btn-save`, `.btn-cancel`

**Child Props:**
```ts
interface Props {
  editing: VocabularyEntry | null;
  categories: { value: string; label: string }[];
  onSave: (values: {
    findText: string;
    replacement: string;
    category: string;
    caseSensitive: boolean;
    priority: number;
    enabled: boolean;
  }) => Promise<void> | void;
  onCancel: () => void;
}
```

**Child internal logic:**
- On mount / when `editing` changes, initialize form fields. Use a `$effect`:
  ```ts
  $effect(() => {
    if (editing) {
      formFind = editing.find_text;
      formReplace = editing.replacement;
      formCategory = editing.category;
      formCaseSensitive = editing.case_sensitive;
      formPriority = editing.priority;
      formEnabled = editing.enabled;
    } else {
      formFind = '';
      formReplace = '';
      formCategory = 'general';
      formCaseSensitive = false;
      formPriority = 0;
      formEnabled = true;
    }
    formError = '';
  });
  ```
- `handleSave()` validates (`!formFind.trim() || !formReplace.trim() → formError = "Find and replacement text are required."; return`) then calls `onSave({ findText: formFind.trim(), replacement: formReplace.trim(), category: formCategory, caseSensitive: formCaseSensitive, priority: formPriority, enabled: formEnabled })`. The caller will close the form via the existing `showForm` toggle in the shell.

**Parent (VocabularyDialog) changes:**
- Remove `form*` state vars (lines 43-52 of current file)
- Replace the inline form template (lines 254-298) with:
  ```svelte
  {#if showForm}
    <VocabularyForm
      {editing}
      categories={CATEGORIES}
      onSave={async (values) => {
        if (editing) {
          await updateVocabularyEntry(
            editing.id, values.findText, values.replacement,
            values.category, values.caseSensitive, values.priority, values.enabled,
          );
        } else {
          await addVocabularyEntry(
            values.findText, values.replacement,
            values.category, values.caseSensitive, values.priority, values.enabled,
          );
        }
        showForm = false;
        editing = null;
        await loadEntries();
      }}
      onCancel={() => { showForm = false; editing = null; }}
    />
  {/if}
  ```
- The `openAddForm` and `openEditForm` shell handlers no longer set form state, only `editing = entry | null` and `showForm = true`. Their bodies shrink dramatically.
- `closeForm` is now inlined (only the callback above uses it).
- The `handleSave` function in shell goes away.

**CSS to remove from VocabularyDialog.svelte:** `.vocab-form`, `.form-header` (and `.form-header h3`), `.btn-close-form` (+ hover), `.form-error`, `.form-grid`, `.field` (+ children), `.form-toggles`, `.vocab-toggle` (+ children), `.toggle-text`, `.form-actions`, `.btn-save` (+ hover), `.btn-cancel` (+ hover). Keep `.vocab-dialog input[type="checkbox"]` rule in the shell for now — table extraction (Task 2) will move it.

**Steps:**
- [ ] Read current VocabularyDialog.svelte and the spec section "VocabularyForm gets" CSS list.
- [ ] Create `VocabularyForm.svelte` with the moved markup + state + styles. Import `VocabularyEntry` from `../api/vocabulary`.
- [ ] Edit VocabularyDialog.svelte: import the child, replace the form block, remove form state + handlers + form CSS, update `openAddForm`/`openEditForm` to just set `editing` and `showForm`.
- [ ] Run `npx vitest run` → expect 233 passed.
- [ ] Run `npm run check` → expect no errors.
- [ ] Commit:
  ```
  refactor(vocab): extract VocabularyForm sub-component

  Form owns its own field state and validation. VocabularyDialog
  shrinks accordingly — openAdd/openEdit just toggle editing + showForm.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 2: Extract VocabularyTable

**Files:**
- Create: `src/lib/components/VocabularyTable.svelte`
- Modify: `src/lib/components/VocabularyDialog.svelte`

**What moves into VocabularyTable.svelte:**
- The table template block (current lines 301-339 of VocabularyDialog.svelte: `.vocab-table-wrap` div with loading/empty/table)
- The `filteredEntries()` function (currently in shell, only the table uses it). Becomes a `$derived` in the child:
  ```ts
  const filtered = $derived.by(() => {
    if (!searchText.trim()) return entries;
    const q = searchText.toLowerCase();
    return entries.filter(e =>
      e.find_text.toLowerCase().includes(q) || e.replacement.toLowerCase().includes(q),
    );
  });
  ```
- CSS rules: `.vocab-table-wrap`, `.loading-text`, `.empty-text`, `.vocab-table` (+ `th`, `td`, `tr.disabled`, `tr:hover`), `.mono`, `.truncate`, `.col-category`, `.col-enabled` (+ `input`), `.col-actions`, `.actions`, `.btn-edit` (+ hover), `.btn-delete` (+ hover)
- The `.vocab-dialog input[type="checkbox"]` global override (currently line 554) — the table is the remaining surface that needs it. Replace in child as plain `input[type="checkbox"] { ... }` since this scope is just the table.

**Child Props:**
```ts
interface Props {
  entries: VocabularyEntry[];
  loading: boolean;
  searchText: string;
  totalCount: number;  // for footer count display in parent — actually parent computes this itself; this prop NOT needed
  categoryLabel: (value: string) => string;
  onEdit: (entry: VocabularyEntry) => void;
  onDelete: (entry: VocabularyEntry) => void;
  onToggleEnabled: (entry: VocabularyEntry) => void;
}
```

Remove `totalCount` — not needed; parent computes its own count from full entries array.

**Child markup (template — copy verbatim from current VocabularyDialog.svelte lines 301-339, swap `filteredEntries()` → `filtered`, `loadEntries`/etc. callbacks → props):**

```svelte
<div class="vocab-table-wrap">
  {#if loading}
    <p class="loading-text">Loading...</p>
  {:else if filtered.length === 0}
    <p class="empty-text">No vocabulary entries found.</p>
  {:else}
    <table class="vocab-table">
      <thead>
        <tr>
          <th>Find</th>
          <th>Replace With</th>
          <th class="col-category">Category</th>
          <th class="col-enabled">Enabled</th>
          <th class="col-actions">Actions</th>
        </tr>
      </thead>
      <tbody>
        {#each filtered as entry (entry.id)}
          <tr class:disabled={!entry.enabled}>
            <td class="mono">{entry.find_text}</td>
            <td class="truncate">{entry.replacement}</td>
            <td class="col-category">{categoryLabel(entry.category)}</td>
            <td class="col-enabled">
              <input
                type="checkbox"
                checked={entry.enabled}
                onchange={() => onToggleEnabled(entry)}
              />
            </td>
            <td class="col-actions actions">
              <button class="btn-edit" onclick={() => onEdit(entry)}>Edit</button>
              <button class="btn-delete" onclick={() => onDelete(entry)}>Del</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>
```

**Parent (VocabularyDialog) changes:**
- Remove `filteredEntries()` function from shell (lines 85-93 of current file).
- Replace the table block (lines 301-339) with:
  ```svelte
  <VocabularyTable
    {entries}
    {loading}
    {searchText}
    {categoryLabel}
    onEdit={openEditForm}
    onDelete={handleDelete}
    onToggleEnabled={handleToggleEnabled}
  />
  ```
- Footer count must still work: currently `{filteredEntries().length} shown{...of ${entries.length}}`. After extraction, parent can recompute its own filter for the footer, or move the count display to be a child slot. Simplest: parent recomputes:
  ```svelte
  <span class="footer-count">
    {#if searchText || filterCategory !== 'all'}
      {filterCount} shown of {entries.length}
    {:else}
      {entries.length} shown
    {/if}
  </span>
  ```
  where `const filterCount = $derived.by(() => entries.filter(e => !searchText.trim() ? true : (e.find_text.toLowerCase().includes(searchText.toLowerCase()) || e.replacement.toLowerCase().includes(searchText.toLowerCase()))).length);`

  This means the filter logic lives in two places (shell + child). To keep it DRY, lift the filter to a shared util `filterVocabularyEntries(entries, searchText)` in `../api/vocabulary` or a new `../utils/vocabularyFilter.ts`. **Simplest:** put it in `src/lib/utils/vocabularyFilter.ts`:
  ```ts
  import type { VocabularyEntry } from '../api/vocabulary';
  export function filterVocabularyEntries(entries: VocabularyEntry[], searchText: string): VocabularyEntry[] {
    if (!searchText.trim()) return entries;
    const q = searchText.toLowerCase();
    return entries.filter(e =>
      e.find_text.toLowerCase().includes(q) || e.replacement.toLowerCase().includes(q),
    );
  }
  ```
  Both shell and child import + use this.

**CSS to remove from VocabularyDialog.svelte:** all the rules listed above.

**Steps:**
- [ ] Create `src/lib/utils/vocabularyFilter.ts` with the shared filter function.
- [ ] Create `VocabularyTable.svelte` (markup + props + scoped CSS).
- [ ] Edit VocabularyDialog.svelte: import child + filter util, remove `filteredEntries()`, remove table block + replace with `<VocabularyTable .../>`, update footer count to use shared filter, remove table CSS rules.
- [ ] Run `npx vitest run` → 233 passed.
- [ ] Run `npm run check` → no errors.
- [ ] Commit:
  ```
  refactor(vocab): extract VocabularyTable sub-component + shared filter

  Table is pure presentation. filteredEntries() lives in a tiny shared
  util so the shell's footer count and the table use the same logic.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 3: Extract VocabularyTestPanel

**Files:**
- Create: `src/lib/components/VocabularyTestPanel.svelte`
- Modify: `src/lib/components/VocabularyDialog.svelte`

**What moves into VocabularyTestPanel.svelte:**
- All `test*` state vars (`testInput`, `testResult`, `testError`, `testing`) — declared as `$state(...)` in child.
- `handleTest` function.
- Test panel template (lines 343-362 of current VocabularyDialog.svelte).
- CSS rules: `.vocab-test` (+ `h3`, `textarea`), `.btn-test` (+ disabled, hover), `.test-error`, `.test-result` (+ `strong`, `pre`).

**Child Props:**
```ts
interface Props {
  resetSignal: number;
}
```

Inside child, react to `resetSignal` changes by resetting `testResult` and `testError`:
```ts
$effect(() => {
  resetSignal; // explicit reactive read
  testResult = null;
  testError = null;
});
```

This effect runs once on mount and again each time `resetSignal` changes.

**Parent (VocabularyDialog) changes:**
- Remove `test*` state vars and `handleTest` function.
- Add `let resetSignal = $state(0);` to shell.
- The existing `$effect(() => { if (open) { loadEntries(); testResult = null; testError = null; } });` becomes `$effect(() => { if (open) { loadEntries(); resetSignal += 1; } });`
- Replace test panel template (lines 343-362) with `<VocabularyTestPanel {resetSignal} />`.

**CSS to remove from VocabularyDialog.svelte:** all `.vocab-test*`, `.btn-test*`, `.test-error`, `.test-result*` rules.

**Steps:**
- [ ] Create `VocabularyTestPanel.svelte` with state + template + scoped CSS. Import `testVocabularyCorrection`, `CorrectionResult` from `../api/vocabulary`; `formatError` from `../types/errors`.
- [ ] Edit VocabularyDialog.svelte: import child, add `resetSignal`, update the `if (open)` effect, replace template block, remove test state + handler + CSS.
- [ ] Run `npx vitest run` → 233 passed.
- [ ] Run `npm run check` → no errors.
- [ ] Commit:
  ```
  refactor(vocab): extract VocabularyTestPanel sub-component

  Test panel owns its own input/result state. Parent signals reset
  via resetSignal counter incremented when dialog opens.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 4: Finalize VocabularyDialog shell

**Files:**
- Modify: `src/lib/components/VocabularyDialog.svelte`

**What this task does:**
- Audit the shell for dead code remaining after Tasks 1-3.
- Confirm shell is ≤ 300 lines total.
- Confirm no unused imports, no unused state, no orphaned CSS rules, no `console.error` calls that should be `toasts.error` (or both, as in baseline).

**Steps:**
- [ ] Read VocabularyDialog.svelte.
- [ ] Remove any unused imports (likely: `onMount`, `onDestroy` are still needed; `formatError` might be removable if no longer used in shell — check).
- [ ] Verify shell imports are: `VocabularyForm`, `VocabularyTable`, `VocabularyTestPanel`, the vocabulary api functions still used (`addVocabularyEntry`, `updateVocabularyEntry`, `deleteVocabularyEntry`, `deleteAllVocabularyEntries`, `listVocabularyEntries`, types), `filterVocabularyEntries`, `toasts`.
- [ ] Confirm the shell template structure: overlay → dialog → header → toolbar → body (with `{#if showForm}<VocabularyForm.../>{/if}` and `<VocabularyTable .../>`) → `<VocabularyTestPanel .../>` → footer.
- [ ] If line count > 300 with all sections, audit CSS: remove any rule whose selector no longer matches anything in the remaining template.
- [ ] Run `npx vitest run` → 233 passed.
- [ ] Run `npm run check` → no errors.
- [ ] Verify VocabularyDialog.svelte line count: `wc -l src/lib/components/VocabularyDialog.svelte`. Should be ≤ 300.
- [ ] Commit (only if any changes were made):
  ```
  refactor(vocab): cleanup VocabularyDialog shell after extractions

  Remove unused imports, state, and CSS rules. Final shell holds only
  the dialog frame, toolbar, footer, and orchestration between the
  three extracted children.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```
  If no cleanup was needed (i.e., Tasks 1-3 left the shell perfectly clean), skip the commit and report DONE with a note.

---

## Task 5: Extract AudioInputSection

**Files:**
- Create: `src/lib/components/settings/AudioInputSection.svelte`
- Modify: `src/lib/components/settings/Audio.svelte`

**What moves:**
- The Input Device picker template (current lines 147-166 of `Audio.svelte`).
- The Sample Rate picker template (current lines 414-425).
- The `handleInputDeviceChange` and `handleSampleRateChange` handlers.

**Child Props:**
```ts
interface Props {
  audioDevices: AudioDevice[];
  devicesLoading: boolean;
}
```

**Child imports:**
- `settings` from `../../stores/settings`
- `AudioDevice` from `../../types`

**Child styles:** copy `.form-group`, `.form-label` rules from the parent (Audio.svelte). These are used by the shell + several other children; we're allowed to duplicate.

**Parent changes:**
- Remove `handleInputDeviceChange` and `handleSampleRateChange` functions.
- Replace the two `<div class="form-group">` blocks for Input Device (147-166) and Sample Rate (414-425) with a single `<AudioInputSection {audioDevices} {devicesLoading} />` line at the position where Input Device appears (top of the section). Note: this **reorders** Sample Rate — it currently sits BELOW the diarization model list, but logically it pairs with Input Device. Confirm before reordering — **DO NOT reorder; instead instantiate the child twice or keep Sample Rate in shell.** Simpler: keep Sample Rate in the shell for this batch (it's a single select, 12 lines). Update child scope: AudioInputSection contains ONLY the Input Device picker.

**Final AudioInputSection scope:** Input Device picker only (~30 lines). Sample Rate stays in shell.

**Steps:**
- [ ] Create `AudioInputSection.svelte` with the Input Device picker template, props for `audioDevices` and `devicesLoading`, and the input device change handler. Pull `settings` directly.
- [ ] Edit `Audio.svelte`: import child, remove `handleInputDeviceChange`, replace the Input Device `<div class="form-group">...</div>` block (~20 lines) with `<AudioInputSection {audioDevices} {devicesLoading} />`.
- [ ] Run `npx vitest run` → 233 passed.
- [ ] Run `npm run check` → no errors.
- [ ] Commit:
  ```
  refactor(audio): extract AudioInputSection sub-component

  Input Device picker becomes a self-contained child. Sample Rate
  stays in the shell to preserve current section ordering.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 6: Extract WhisperLocalSection

**Files:**
- Create: `src/lib/components/settings/WhisperLocalSection.svelte`
- Modify: `src/lib/components/settings/Audio.svelte`

**What moves:** the entire `{#if sttMode === 'local'}` block (current lines 196-256 of Audio.svelte): Whisper Model picker + Model Management list. (We're not moving the `{:else}` branch — that goes to SttRemoteSection in Task 7.)

**Child Props:**
```ts
interface Props {
  whisperModels: WhisperModelInfo[];
  modelsRefreshing: boolean;
  downloadingModel: string | null;
  downloadProgress: Record<string, { downloaded: number; total: number }>;
  onModelChange: (modelId: string) => Promise<void>;
  onDownload: (modelId: string) => Promise<void>;
  onDelete: (modelId: string) => Promise<void>;
  formatBytes: (bytes: number) => string;
}
```

Note: child pulls `$settings.whisper_model` directly via `import { settings } from '../../stores/settings';` to compare model.id to the active one.

**Child CSS:** copy `.form-group`, `.form-label`, `.form-hint`, `.model-list`, `.model-row`, `.model-info`, `.model-name`, `.model-desc`, `.model-size`, `.model-actions`, `.badge-downloaded`, `.download-progress`, `.btn-download-model` (+ hover, disabled), `.btn-delete-model` (+ hover, disabled).

**Parent changes:** replace lines 196-256 (the `{#if sttMode === 'local'}` block — minus the `{:else}` keyword that introduces remote) with:
```svelte
{#if sttMode === 'local'}
  <WhisperLocalSection
    {whisperModels}
    {modelsRefreshing}
    {downloadingModel}
    {downloadProgress}
    onModelChange={handleWhisperModelChange}
    onDownload={handleDownloadModel}
    onDelete={handleDeleteModel}
    {formatBytes}
  />
{:else}
  <!-- remote block stays for Task 7 -->
{/if}
```

The `handleWhisperModelChange` currently takes `(e: Event)`. The child's `onModelChange` takes `(modelId: string)`. Update the handler signature in Audio.svelte from `async function handleWhisperModelChange(e: Event) { const value = (e.target as HTMLSelectElement).value; await settings.updateField('whisper_model', value); }` to `async function handleWhisperModelChange(modelId: string) { await settings.updateField('whisper_model', modelId); }`. The child invokes `onModelChange(e.target.value)` from the select element.

**Steps:**
- [ ] Create `WhisperLocalSection.svelte` with template, props, styles.
- [ ] Edit `Audio.svelte`: import child, update `handleWhisperModelChange` signature, replace inline template.
- [ ] Run vitest + svelte-check.
- [ ] Commit:
  ```
  refactor(audio): extract WhisperLocalSection sub-component

  Whisper picker + model management list become a self-contained child
  rendered when sttMode === 'local'.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 7: Extract SttRemoteSection

**Files:**
- Create: `src/lib/components/settings/SttRemoteSection.svelte`
- Modify: `src/lib/components/settings/Audio.svelte`

**What moves:** the entire `{:else}` branch from current lines 258-366: host/port/model/API key inputs + Save key button + Test Connection button + result display. Plus the local form state `sttRemoteApiKey`, `sttRemoteTestStatus`, `sttRemoteTestMessage`. Plus the `sttOk` and `sttKind` `$derived` (currently lines 14-15 — both consumed only in the warning text inside the host input block).

**Child Props:** none. Child pulls `settings`, `setApiKey`, `getApiKey`, `testSttRemoteConnection`, `reinitProviders`, `classifyEndpoint`, `isLocalOrAllowed`, `formatError` directly.

**Child needs onMount to load API key:** the existing `getApiKey('stt_remote_api_key').then(...)` call in parent's `onMount` (lines 109-112) moves into child's onMount.

**Parent changes:**
- Remove `sttRemoteApiKey`, `sttRemoteTestStatus`, `sttRemoteTestMessage` state.
- Remove `sttOk`, `sttKind` derived values.
- Remove the api-key load from `onMount`.
- Remove the `{:else}` block content; replace with `<SttRemoteSection />`:
  ```svelte
  {:else}
    <SttRemoteSection />
  {/if}
  ```
- Remove imports that are no longer used in shell: `setApiKey`, `getApiKey`, `testSttRemoteConnection`, `classifyEndpoint`, `isLocalOrAllowed`. `reinitProviders` IS still used (for STT mode radio change). `formatError` no longer used in shell.

**Child CSS:** copy `.form-group`, `.form-label`, `.form-hint`, `.text-input` (if it exists — check; it's referenced but I don't see a rule for it in the styles; it inherits from the global stylesheet), `.port-input`, `.btn-test-connection` (+ hover, disabled), `.test-result`, `.test-success`, `.test-error`, `.endpoint-warning`.

**Steps:**
- [ ] Create `SttRemoteSection.svelte` with state, all the imports, template (verbatim from lines 258-366), and scoped CSS.
- [ ] Edit `Audio.svelte`: remove sttRemote state + derived + onMount call + imports + template body.
- [ ] Run vitest + svelte-check.
- [ ] Commit:
  ```
  refactor(audio): extract SttRemoteSection sub-component

  Remote STT config (host, port, model, API key, test connection)
  becomes a self-contained child rendered when sttMode === 'remote'.
  Owns its own form state and api-key load.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 8: Extract DiarizationModelsSection

**Files:**
- Create: `src/lib/components/settings/DiarizationModelsSection.svelte`
- Modify: `src/lib/components/settings/Audio.svelte`

**What moves:** the pyannote model list block (current lines 369-412 — the `<p class="form-hint">` intro line plus the `<div class="form-group">` with the model list).

**Child Props:**
```ts
interface Props {
  pyannoteModels: WhisperModelInfo[];
  downloadingModel: string | null;
  downloadProgress: Record<string, { downloaded: number; total: number }>;
  onDownload: (modelId: string) => Promise<void>;
  onDelete: (modelId: string) => Promise<void>;
  formatBytes: (bytes: number) => string;
}
```

**Child CSS:** copy `.form-group`, `.form-label`, `.form-hint`, `.model-list`, `.model-row`, etc. (same set as WhisperLocalSection — duplication is fine per spec).

**Parent changes:** replace lines 369-412 with `<DiarizationModelsSection {pyannoteModels} {downloadingModel} {downloadProgress} onDownload={handleDownloadModel} onDelete={handleDeleteModel} {formatBytes} />`.

**Steps:**
- [ ] Create `DiarizationModelsSection.svelte`.
- [ ] Edit `Audio.svelte`: import + replace block.
- [ ] Run vitest + svelte-check.
- [ ] Commit:
  ```
  refactor(audio): extract DiarizationModelsSection sub-component

  Pyannote model list becomes a self-contained child, always visible
  regardless of STT mode (diarization runs locally either way).

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 9: Finalize Audio.svelte shell

**Files:**
- Modify: `src/lib/components/settings/Audio.svelte`

**What this task does:**
- Audit shell for dead code after Tasks 5-8.
- Confirm shell is ≤ 280 lines.
- Confirm imports are minimal.

**Steps:**
- [ ] Read Audio.svelte.
- [ ] Remove unused imports (likely candidates: `listAudioDevices` still used; `listWhisperModels`, `listPyannoteModels`, `downloadModel`, `deleteModel`, `WhisperModelInfo` still used; `setApiKey`/`getApiKey`/`testSttRemoteConnection` no longer used in shell; `reinitProviders` still used for STT mode radio; `classifyEndpoint`/`isLocalOrAllowed` no longer used; `formatError` no longer used).
- [ ] Audit CSS rules — remove any that no longer match anything in the slimmed template.
- [ ] Confirm shell template structure: Input device child → STT mode radios → conditional whisper/remote child → diarization child → Sample Rate → auto-soap + capture toggles.
- [ ] Run `npx vitest run` → 233 passed.
- [ ] Run `npm run check` → no errors.
- [ ] `wc -l src/lib/components/settings/Audio.svelte` → ≤ 280.
- [ ] Commit (if changes made):
  ```
  refactor(audio): cleanup Audio.svelte shell after extractions

  Remove unused imports and CSS rules left over from the four sub-component
  extractions. Final shell coordinates state and renders the four children
  conditionally.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 10: Final verification

- [ ] `npx vitest run` → 233 passed (no new tests added — we're verifying the existing suite still passes).
- [ ] `npm run check` → svelte-check clean.
- [ ] `cargo test --workspace --lib` → all suites green (sanity check that frontend refactor didn't disturb backend).
- [ ] `git log --oneline master..HEAD` shows ~11 commits (spec + plan + 9 extraction commits + maybe shell finalize commits).
- [ ] `git status` clean.
- [ ] Spot-check: open `src/lib/components/VocabularyDialog.svelte` and `src/lib/components/settings/Audio.svelte` — each ≤ 300 / ≤ 280 lines.
- [ ] Dispatch final whole-branch code reviewer subagent.
- [ ] After review, present merge options menu (1/2/3/4).

After all tasks: Dispatch final code reviewer subagent for entire implementation. Then use superpowers:finishing-a-development-branch.
