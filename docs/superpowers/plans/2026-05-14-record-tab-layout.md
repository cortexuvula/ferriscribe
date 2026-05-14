# Record Tab Layout Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Patient Context out of a top-stacked accordion and into a permanent right-side sidebar that is collapsible (toggle button), user-resizable (drag handle), and persists those preferences across sessions — so the recording timer, waveform, and pipeline status stay center-stage and stable in size regardless of context content.

**Architecture:** Two new tiny TS modules (`recordSidebar` localStorage-backed Svelte writable store; pure `clampSidebarWidth` resize helper) consumed by four new Svelte components (`ResizeHandle`, `PatientContextStructuredTab`, `PatientContextNotesTab`, `PatientContextSidebar`). `RecordTab.svelte` rewires its body to a horizontal `.record-body` flex with the new sidebar on the right; the existing `PatientContextPanel.svelte` is deleted.

**Tech Stack:** Svelte 5 runes (`$state`, `$derived`, `$bindable`, `$props`, `$effect`), TypeScript, `svelte/store` `writable` (matching the existing `theme.ts` pattern), Vitest. No new npm dependencies. No backend changes.

**Spec:** [`docs/superpowers/specs/2026-05-14-record-tab-layout-design.md`](../specs/2026-05-14-record-tab-layout-design.md)

---

## File Structure

**New files:**
- `src/lib/stores/recordSidebar.ts` — localStorage-backed writable store for sidebar open/width
- `src/lib/stores/recordSidebar.test.ts` — Vitest unit tests
- `src/lib/utils/resize.ts` — pure `clampSidebarWidth` helper
- `src/lib/utils/resize.test.ts` — Vitest unit tests
- `src/lib/pages/record/ResizeHandle.svelte` — 6px drag bar with pointer capture
- `src/lib/pages/record/PatientContextStructuredTab.svelte` — Medications / Allergies / Conditions textareas
- `src/lib/pages/record/PatientContextNotesTab.svelte` — Notes textarea + template picker + Save modal
- `src/lib/pages/record/PatientContextSidebar.svelte` — container: header, tabs row, mounts active tab

**Modified files:**
- `src/lib/pages/RecordTab.svelte` — replace `.record-tab` body with new `.record-body` horizontal flex; wire sidebar state from `recordSidebar` store; drop the local `contextCollapsed` state and `<PatientContextPanel>` mount

**Deleted files:**
- `src/lib/pages/record/PatientContextPanel.svelte` — replaced by Sidebar + two tab components

**Worktree:** Use `superpowers:using-git-worktrees` to create `.worktrees/record-tab-layout` from `master` at the spec commit (`e5a17c5`) before Task 1. The project requires worktree isolation per `CLAUDE.md`.

---

## Task 1: `recordSidebar.ts` store (TDD)

**Files:**
- Create: `src/lib/stores/recordSidebar.ts`
- Create: `src/lib/stores/recordSidebar.test.ts`

**Why:** The persistence layer is the smallest unit with non-trivial logic (defaults, clamping on read, swallowing storage errors). Building it first means later components can subscribe to it with no shimming.

- [ ] **Step 1: Write the failing tests**

Create `src/lib/stores/recordSidebar.test.ts`:

```ts
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

const OPEN_KEY = 'record.sidebar.open';
const WIDTH_KEY = 'record.sidebar.width';

// Re-import the module under test FRESHLY for each test so module-level
// initialization (which reads localStorage) reflects the current mock state.
async function freshStore() {
  vi.resetModules();
  return await import('./recordSidebar');
}

describe('recordSidebar store', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });

  it('defaults to open=true and width=360 when localStorage is empty', async () => {
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.open)).toBe(true);
    expect(get(recordSidebar.width)).toBe(360);
  });

  it('reads persisted open=false', async () => {
    localStorage.setItem(OPEN_KEY, 'false');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.open)).toBe(false);
  });

  it('reads any non-"false" value as open (open-on-doubt)', async () => {
    localStorage.setItem(OPEN_KEY, 'malformed');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.open)).toBe(true);
  });

  it('persists open via setOpen', async () => {
    const { recordSidebar } = await freshStore();
    recordSidebar.setOpen(false);
    expect(get(recordSidebar.open)).toBe(false);
    expect(localStorage.getItem(OPEN_KEY)).toBe('false');
    recordSidebar.setOpen(true);
    expect(localStorage.getItem(OPEN_KEY)).toBe('true');
  });

  it('reads persisted width within [280, 600] verbatim', async () => {
    localStorage.setItem(WIDTH_KEY, '420');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.width)).toBe(420);
  });

  it('clamps too-small persisted width up to 280', async () => {
    localStorage.setItem(WIDTH_KEY, '100');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.width)).toBe(280);
  });

  it('clamps too-large persisted width down to 600', async () => {
    localStorage.setItem(WIDTH_KEY, '9999');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.width)).toBe(600);
  });

  it('falls back to 360 when persisted width is non-numeric', async () => {
    localStorage.setItem(WIDTH_KEY, 'abc');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.width)).toBe(360);
  });

  it('falls back to 360 when persisted width is zero or negative', async () => {
    localStorage.setItem(WIDTH_KEY, '0');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.width)).toBe(360);
  });

  it('persists width via setWidth, clamping the value', async () => {
    const { recordSidebar } = await freshStore();
    recordSidebar.setWidth(500);
    expect(get(recordSidebar.width)).toBe(500);
    expect(localStorage.getItem(WIDTH_KEY)).toBe('500');
    recordSidebar.setWidth(50);
    expect(get(recordSidebar.width)).toBe(280);
    expect(localStorage.getItem(WIDTH_KEY)).toBe('280');
    recordSidebar.setWidth(99999);
    expect(get(recordSidebar.width)).toBe(600);
  });

  it('survives localStorage.getItem throwing on init', async () => {
    const spy = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.open)).toBe(true);
    expect(get(recordSidebar.width)).toBe(360);
    spy.mockRestore();
  });

  it('survives localStorage.setItem throwing on update', async () => {
    const { recordSidebar } = await freshStore();
    const spy = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota');
    });
    expect(() => recordSidebar.setOpen(false)).not.toThrow();
    expect(get(recordSidebar.open)).toBe(false); // in-memory value still updates
    expect(() => recordSidebar.setWidth(420)).not.toThrow();
    expect(get(recordSidebar.width)).toBe(420);
    spy.mockRestore();
  });
});
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run:

```bash
npx vitest run src/lib/stores/recordSidebar.test.ts
```

Expected: all 12 tests fail (module does not exist yet).

- [ ] **Step 3: Implement the store**

Create `src/lib/stores/recordSidebar.ts`:

```ts
import { writable, type Readable } from 'svelte/store';

const OPEN_KEY = 'record.sidebar.open';
const WIDTH_KEY = 'record.sidebar.width';
const DEFAULT_WIDTH = 360;
const MIN_WIDTH = 280;
const MAX_WIDTH = 600;

function clamp(n: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, n));
}

function readOpen(): boolean {
  try {
    const v = localStorage.getItem(OPEN_KEY);
    // Open-on-doubt: only the exact string "false" disables.
    return v !== 'false';
  } catch {
    return true;
  }
}

function readWidth(): number {
  try {
    const raw = localStorage.getItem(WIDTH_KEY);
    if (raw === null) return DEFAULT_WIDTH;
    const n = Number(raw);
    if (!Number.isFinite(n) || n <= 0) return DEFAULT_WIDTH;
    return clamp(n, MIN_WIDTH, MAX_WIDTH);
  } catch {
    return DEFAULT_WIDTH;
  }
}

const _open = writable<boolean>(readOpen());
const _width = writable<number>(readWidth());

export const recordSidebar = {
  open: { subscribe: _open.subscribe } as Readable<boolean>,
  width: { subscribe: _width.subscribe } as Readable<number>,

  setOpen(v: boolean) {
    _open.set(v);
    try {
      localStorage.setItem(OPEN_KEY, v ? 'true' : 'false');
    } catch {
      // Persistence best-effort; in-memory value is authoritative.
    }
  },

  setWidth(v: number) {
    const clamped = clamp(Math.round(v), MIN_WIDTH, MAX_WIDTH);
    _width.set(clamped);
    try {
      localStorage.setItem(WIDTH_KEY, String(clamped));
    } catch {
      // Persistence best-effort; in-memory value is authoritative.
    }
  },

  // Exposed for tests and for the resize-helper consumer.
  MIN_WIDTH,
  MAX_WIDTH,
  DEFAULT_WIDTH,
};
```

- [ ] **Step 4: Run the tests and confirm they pass**

Run:

```bash
npx vitest run src/lib/stores/recordSidebar.test.ts
```

Expected: 12/12 pass.

- [ ] **Step 5: Verify type-check is clean**

Run:

```bash
npm run check
```

Expected: 0 errors. (The pre-existing `ExportDialog.svelte` `state_referenced_locally` warning is unchanged.)

- [ ] **Step 6: Commit**

```bash
git add src/lib/stores/recordSidebar.ts src/lib/stores/recordSidebar.test.ts
git commit -m "feat(record): add recordSidebar localStorage-backed store"
```

---

## Task 2: `resize.ts` clamping helper (TDD)

**Files:**
- Create: `src/lib/utils/resize.ts`
- Create: `src/lib/utils/resize.test.ts`

**Why:** Pure helper used by both the drag handler and the viewport-resize `$effect`. Trivial to TDD; isolated from DOM so the unit tests cover all four clamp directions.

- [ ] **Step 1: Write the failing tests**

Create `src/lib/utils/resize.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { clampSidebarWidth } from './resize';

describe('clampSidebarWidth', () => {
  // Standard params: min=280, max=600, mainMin=320.
  const args = (requested: number, viewport: number) =>
    [requested, viewport, 280, 600, 320] as const;

  it('returns requested value when within all bounds', () => {
    expect(clampSidebarWidth(...args(400, 2000))).toBe(400);
  });

  it('clamps to max=600 when requested above max', () => {
    expect(clampSidebarWidth(...args(700, 2000))).toBe(600);
  });

  it('clamps to min=280 when requested below min', () => {
    expect(clampSidebarWidth(...args(200, 2000))).toBe(280);
  });

  it('clamps to viewport-mainMin when that is the tightest', () => {
    // viewport=700 leaves 380 for the sidebar after reserving mainMin=320.
    expect(clampSidebarWidth(...args(500, 700))).toBe(380);
  });

  it('falls back to min when viewport is too narrow for both', () => {
    // viewport=500 leaves only 180 — but min wins.
    expect(clampSidebarWidth(...args(500, 500))).toBe(280);
  });

  it('returns exactly min on a viewport that exactly fits min + mainMin', () => {
    // viewport=600 leaves 280 — sidebar fits exactly at min.
    expect(clampSidebarWidth(...args(280, 600))).toBe(280);
  });

  it('returns exactly max on a viewport that has plenty of room', () => {
    expect(clampSidebarWidth(...args(600, 2000))).toBe(600);
  });

  it('rounds the requested value to an integer-friendly number', () => {
    // Helper does not need to round itself — but it should accept floats
    // and return the same float when within bounds (rounding is caller's job).
    expect(clampSidebarWidth(...args(400.5, 2000))).toBe(400.5);
  });
});
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run:

```bash
npx vitest run src/lib/utils/resize.test.ts
```

Expected: 8 tests fail (module does not exist).

- [ ] **Step 3: Implement the helper**

Create `src/lib/utils/resize.ts`:

```ts
/**
 * Clamp a requested sidebar width against three constraints:
 *   - [min, max] absolute bounds
 *   - viewport must allow at least `mainMin` px for the non-sidebar area
 *
 * If even `min` doesn't fit alongside `mainMin`, returns `min` and lets the
 * caller's layout handle the overflow (typically a horizontal scroll).
 */
export function clampSidebarWidth(
  requested: number,
  viewportWidth: number,
  min: number,
  max: number,
  mainMin: number,
): number {
  // First clamp to absolute [min, max].
  let w = Math.max(min, Math.min(max, requested));
  // Then enforce viewport: main area gets at least mainMin px.
  const viewportAllows = viewportWidth - mainMin;
  if (viewportAllows < w) {
    w = Math.max(min, viewportAllows);
  }
  return w;
}
```

- [ ] **Step 4: Run the tests and confirm they pass**

Run:

```bash
npx vitest run src/lib/utils/resize.test.ts
```

Expected: 8/8 pass.

- [ ] **Step 5: Verify type-check is clean**

Run:

```bash
npm run check
```

Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/utils/resize.ts src/lib/utils/resize.test.ts
git commit -m "feat(record): add clampSidebarWidth helper"
```

---

## Task 3: `ResizeHandle.svelte`

**Files:**
- Create: `src/lib/pages/record/ResizeHandle.svelte`

**Why:** Tiny presentational + pointer-capture component. No state of its own; emits `onResize` and `onResizeEnd` callbacks the parent uses to update the sidebar width. No unit test (no logic-bearing TS to test; correctness verified by manual smoke).

- [ ] **Step 1: Create the file**

Create `src/lib/pages/record/ResizeHandle.svelte`:

```svelte
<script lang="ts">
  type Props = {
    onResize: (deltaPx: number) => void;
    onResizeEnd: () => void;
  };
  let { onResize, onResizeEnd }: Props = $props();

  let startX = 0;
  let dragging = false;

  function pointerdown(ev: PointerEvent) {
    dragging = true;
    startX = ev.clientX;
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
    ev.preventDefault();
  }

  function pointermove(ev: PointerEvent) {
    if (!dragging) return;
    const delta = ev.clientX - startX;
    if (delta !== 0) {
      onResize(delta);
      startX = ev.clientX;
    }
  }

  function pointerup(ev: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    try {
      (ev.currentTarget as HTMLElement).releasePointerCapture(ev.pointerId);
    } catch {
      // Capture may already have been released; ignore.
    }
    onResizeEnd();
  }
</script>

<div
  class="resize-handle"
  role="separator"
  aria-orientation="vertical"
  aria-label="Resize patient context sidebar"
  onpointerdown={pointerdown}
  onpointermove={pointermove}
  onpointerup={pointerup}
  onpointercancel={pointerup}
></div>

<style>
  .resize-handle {
    width: 6px;
    cursor: col-resize;
    background: var(--border);
    flex: 0 0 6px;
    user-select: none;
    touch-action: none;
  }
  .resize-handle:hover {
    background: var(--accent);
  }
</style>
```

- [ ] **Step 2: Verify type-check is clean**

Run:

```bash
npm run check
```

Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
git add src/lib/pages/record/ResizeHandle.svelte
git commit -m "feat(record): ResizeHandle component"
```

---

## Task 4: `PatientContextStructuredTab.svelte`

**Files:**
- Create: `src/lib/pages/record/PatientContextStructuredTab.svelte`

**Why:** Pure extraction of the three structured textareas from the existing `PatientContextPanel.svelte`. Keeps Task 7 (the Sidebar container) small. No new behavior — just relocation and a thin component contract.

- [ ] **Step 1: Look up the current structured textarea markup**

Read `src/lib/pages/record/PatientContextPanel.svelte` lines 100–125 for the exact field labels, placeholders, row counts, and CSS class names. The styles for `.field-label` and `.context-textarea.structured` come from the same file's `<style>` block — they will move to the new tab component.

- [ ] **Step 2: Create the component**

Create `src/lib/pages/record/PatientContextStructuredTab.svelte`:

```svelte
<script lang="ts">
  type Props = {
    medicationsText: string;
    allergiesText: string;
    conditionsText: string;
  };
  let {
    medicationsText = $bindable(''),
    allergiesText = $bindable(''),
    conditionsText = $bindable(''),
  }: Props = $props();
</script>

<div class="structured-fields">
  <label class="field-label" for="rt-medications">Medications (one per line)</label>
  <textarea
    id="rt-medications"
    class="context-textarea structured"
    placeholder="Lisinopril 10mg PO daily"
    bind:value={medicationsText}
    rows="3"
  ></textarea>

  <label class="field-label" for="rt-allergies">Allergies (one per line)</label>
  <textarea
    id="rt-allergies"
    class="context-textarea structured"
    placeholder="Penicillin (rash)"
    bind:value={allergiesText}
    rows="2"
  ></textarea>

  <label class="field-label" for="rt-conditions">Known conditions (one per line)</label>
  <textarea
    id="rt-conditions"
    class="context-textarea structured"
    placeholder="Type 2 diabetes"
    bind:value={conditionsText}
    rows="3"
  ></textarea>
</div>

<style>
  .structured-fields {
    display: flex;
    flex-direction: column;
    padding: 8px 12px 12px;
  }

  .field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-top: 8px;
    margin-bottom: 4px;
    display: block;
  }

  .context-textarea {
    display: block;
    width: 100%;
    padding: 8px 10px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: 4px;
    resize: vertical;
    min-height: 80px;
    max-height: 200px;
  }

  .context-textarea.structured {
    min-height: 56px;
  }

  .context-textarea:focus {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .context-textarea::placeholder {
    color: var(--text-muted);
  }
</style>
```

- [ ] **Step 3: Verify type-check is clean**

Run:

```bash
npm run check
```

Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git add src/lib/pages/record/PatientContextStructuredTab.svelte
git commit -m "feat(record): PatientContextStructuredTab component"
```

---

## Task 5: `PatientContextNotesTab.svelte`

**Files:**
- Create: `src/lib/pages/record/PatientContextNotesTab.svelte`

**Why:** Extracts the Notes textarea, template picker, and Save-as-template modal from the existing `PatientContextPanel.svelte`. All behavior preserved — only the call site moves.

- [ ] **Step 1: Look up the current Notes / template markup**

Read `src/lib/pages/record/PatientContextPanel.svelte` lines 127–190 for:
- `applyTemplate` / `openSaveModal` / `closeSaveModal` / `confirmSaveTemplate` functions
- Template picker `<select>` + Save button `<div class="template-toolbar">`
- Notes `<textarea id="rt-notes">`
- Save modal markup (`saveModalOpen`, `saveModalName`, `saveModalError`, `saveModalOverwriteConfirm`)
- The two imports: `upsertContextTemplate` from `../../api/contextTemplates` and `contextTemplates` from `../../stores/contextTemplates`, `formatError` from `../../types/errors`

- [ ] **Step 2: Create the component**

Create `src/lib/pages/record/PatientContextNotesTab.svelte`:

```svelte
<script lang="ts">
  import { upsertContextTemplate } from '../../api/contextTemplates';
  import { contextTemplates } from '../../stores/contextTemplates';
  import { formatError } from '../../types/errors';

  type Props = {
    contextText: string;
  };
  let { contextText = $bindable('') }: Props = $props();

  let selectedTemplate = $state('');
  let saveModalOpen = $state(false);
  let saveModalName = $state('');
  let saveModalError = $state('');
  let saveModalOverwriteConfirm = $state(false);

  function applyTemplate(name: string) {
    if (!name) return;
    const t = $contextTemplates.find((x) => x.name === name);
    if (!t) return;
    if (contextText.trim() === '') {
      contextText = t.body;
    } else {
      contextText = contextText.replace(/\s+$/, '') + '\n\n' + t.body;
    }
    selectedTemplate = '';
  }

  function openSaveModal() {
    if (contextText.trim() === '') return;
    saveModalName = '';
    saveModalError = '';
    saveModalOverwriteConfirm = false;
    saveModalOpen = true;
  }

  function closeSaveModal() {
    saveModalOpen = false;
    saveModalError = '';
    saveModalOverwriteConfirm = false;
  }

  async function confirmSaveTemplate() {
    const name = saveModalName.trim();
    if (!name) {
      saveModalError = 'Name is required.';
      return;
    }
    const exists = $contextTemplates.some((t) => t.name === name);
    if (exists && !saveModalOverwriteConfirm) {
      saveModalOverwriteConfirm = true;
      saveModalError = `A template named "${name}" exists. Click Save again to overwrite.`;
      return;
    }
    try {
      await upsertContextTemplate(name, contextText);
      await contextTemplates.load();
      closeSaveModal();
    } catch (err: any) {
      saveModalError = formatError(err) || 'Failed to save template.';
    }
  }
</script>

<div class="notes-tab">
  <div class="template-toolbar">
    <select
      class="template-picker"
      bind:value={selectedTemplate}
      onchange={() => applyTemplate(selectedTemplate)}
      disabled={$contextTemplates.length === 0}
    >
      <option value="">
        {$contextTemplates.length === 0 ? 'No templates saved' : 'Apply template…'}
      </option>
      {#each $contextTemplates as t (t.name)}
        <option value={t.name}>{t.name}</option>
      {/each}
    </select>
    <button
      class="btn-save-template"
      onclick={openSaveModal}
      disabled={contextText.trim() === ''}
      title={contextText.trim() === '' ? 'Type something first' : 'Save current text as a new template'}
    >
      Save as template
    </button>
  </div>
  <textarea
    id="rt-notes"
    class="notes-textarea"
    placeholder="Paste chart notes, medications, history..."
    bind:value={contextText}
    rows="12"
  ></textarea>
</div>

{#if saveModalOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="save-modal-overlay" onclick={closeSaveModal}>
    <div class="save-modal" onclick={(e) => e.stopPropagation()}>
      <div class="save-modal-header">
        <h3>Save as Template</h3>
        <button class="btn-close" aria-label="Close" onclick={closeSaveModal}>&times;</button>
      </div>
      {#if saveModalError}
        <div class="save-modal-error">{saveModalError}</div>
      {/if}
      <label class="save-modal-field">
        <span>Name</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input type="text" bind:value={saveModalName} placeholder="e.g. Follow-up visit" autofocus />
      </label>
      <div class="save-modal-field">
        <span>Preview</span>
        <pre class="save-modal-preview">{contextText}</pre>
      </div>
      <div class="save-modal-actions">
        <button class="btn-save" onclick={confirmSaveTemplate}>
          {saveModalOverwriteConfirm ? 'Overwrite' : 'Save'}
        </button>
        <button class="btn-cancel" onclick={closeSaveModal}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .notes-tab {
    display: flex;
    flex-direction: column;
    padding: 8px 12px 12px;
    flex: 1;
    min-height: 0;
  }

  .template-toolbar {
    display: flex;
    gap: 8px;
    margin-bottom: 8px;
    align-items: center;
  }

  .template-picker {
    flex: 1 1 auto;
    min-width: 0;
    padding: 6px 10px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 0.88rem;
  }

  .template-picker:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-save-template {
    flex: 0 0 auto;
    padding: 6px 14px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
    font-size: 0.88rem;
    white-space: nowrap;
  }

  .btn-save-template:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .btn-save-template:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .notes-textarea {
    flex: 1;
    width: 100%;
    padding: 8px 10px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: 4px;
    resize: vertical;
    min-height: 200px;
  }

  .notes-textarea:focus {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .notes-textarea::placeholder {
    color: var(--text-muted);
  }

  .save-modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .save-modal {
    background: var(--bg-secondary);
    color: var(--text-primary);
    border-radius: 8px;
    width: 90vw;
    max-width: 520px;
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    padding: 20px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }

  .save-modal-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 12px;
  }

  .save-modal-header h3 {
    margin: 0;
    font-size: 1.05rem;
  }

  .save-modal .btn-close {
    background: none;
    border: none;
    color: var(--text-secondary);
    font-size: 1.4rem;
    line-height: 1;
    padding: 4px 8px;
    cursor: pointer;
    border-radius: 4px;
  }

  .save-modal .btn-close:hover {
    background: var(--bg-hover);
  }

  .save-modal-error {
    color: #ff6b6b;
    margin-bottom: 10px;
    font-size: 0.85rem;
    padding: 6px 10px;
    background: rgba(255, 107, 107, 0.1);
    border-radius: 4px;
  }

  .save-modal-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 0.85rem;
    color: var(--text-secondary);
    margin-bottom: 10px;
  }

  .save-modal-field span {
    font-weight: 500;
  }

  .save-modal-field input {
    padding: 7px 10px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 0.9rem;
  }

  .save-modal-preview {
    background: var(--bg-primary);
    padding: 10px;
    border-radius: 4px;
    border: 1px solid var(--border);
    max-height: 180px;
    overflow-y: auto;
    white-space: pre-wrap;
    font-size: 0.85rem;
    margin: 0;
    font-family: inherit;
  }

  .save-modal-actions {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }

  .save-modal .btn-save {
    padding: 7px 18px;
    border-radius: 4px;
    border: none;
    background: var(--accent);
    color: white;
    cursor: pointer;
    font-size: 0.9rem;
  }

  .save-modal .btn-save:hover {
    filter: brightness(1.1);
  }

  .save-modal .btn-cancel {
    padding: 7px 18px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
    font-size: 0.9rem;
  }
</style>
```

- [ ] **Step 3: Verify type-check is clean**

Run:

```bash
npm run check
```

Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git add src/lib/pages/record/PatientContextNotesTab.svelte
git commit -m "feat(record): PatientContextNotesTab component"
```

---

## Task 6: `PatientContextSidebar.svelte`

**Files:**
- Create: `src/lib/pages/record/PatientContextSidebar.svelte`

**Why:** The composing container. Owns tab state + the open/collapsed visual modes. Two tab components plug into the same active-tab switch.

- [ ] **Step 1: Create the component**

Create `src/lib/pages/record/PatientContextSidebar.svelte`:

```svelte
<script lang="ts">
  import PatientContextStructuredTab from './PatientContextStructuredTab.svelte';
  import PatientContextNotesTab from './PatientContextNotesTab.svelte';

  type Tab = 'structured' | 'notes';

  type Props = {
    contextText: string;
    medicationsText: string;
    allergiesText: string;
    conditionsText: string;
    open: boolean;
    width: number;
    onToggle: () => void;
  };
  let {
    contextText = $bindable(''),
    medicationsText = $bindable(''),
    allergiesText = $bindable(''),
    conditionsText = $bindable(''),
    open,
    width,
    onToggle,
  }: Props = $props();

  let activeTab: Tab = $state('structured');

  const structuredHasContent = $derived(
    medicationsText.trim().length > 0 ||
      allergiesText.trim().length > 0 ||
      conditionsText.trim().length > 0,
  );
  const notesHasContent = $derived(contextText.trim().length > 0);
  const anyContent = $derived(structuredHasContent || notesHasContent);
</script>

{#if open}
  <aside
    class="sidebar"
    style="width: {width}px"
    aria-label="Patient context sidebar"
  >
    <header class="sidebar-header">
      <h2 class="sidebar-title">Patient Context</h2>
      <button
        class="toggle-btn"
        aria-label="Hide patient context sidebar"
        aria-expanded="true"
        onclick={onToggle}
        title="Hide patient context"
      >
        ▶
      </button>
    </header>

    <div class="tabs-row" role="tablist">
      <button
        role="tab"
        id="tab-structured"
        aria-selected={activeTab === 'structured'}
        aria-controls="panel-structured"
        class="tab-button"
        class:active={activeTab === 'structured'}
        onclick={() => (activeTab = 'structured')}
      >
        Structured
        {#if structuredHasContent}
          <span class="dot" aria-label="has content">●</span>
        {/if}
      </button>
      <button
        role="tab"
        id="tab-notes"
        aria-selected={activeTab === 'notes'}
        aria-controls="panel-notes"
        class="tab-button"
        class:active={activeTab === 'notes'}
        onclick={() => (activeTab = 'notes')}
      >
        Notes
        {#if notesHasContent}
          <span class="dot" aria-label="has content">●</span>
        {/if}
      </button>
    </div>

    <div class="tab-content">
      {#if activeTab === 'structured'}
        <div role="tabpanel" id="panel-structured" aria-labelledby="tab-structured" class="panel">
          <PatientContextStructuredTab
            bind:medicationsText
            bind:allergiesText
            bind:conditionsText
          />
        </div>
      {:else}
        <div role="tabpanel" id="panel-notes" aria-labelledby="tab-notes" class="panel">
          <PatientContextNotesTab bind:contextText />
        </div>
      {/if}
    </div>
  </aside>
{:else}
  <button
    class="rail"
    aria-label="Show patient context sidebar"
    aria-expanded="false"
    onclick={onToggle}
    title="Show patient context"
  >
    <span class="rail-arrow">◀</span>
    <span class="rail-label">Patient Context</span>
    {#if anyContent}
      <span class="rail-dot" aria-label="has content">●</span>
    {/if}
  </button>
{/if}

<style>
  .sidebar {
    background: var(--bg-secondary);
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    flex: 0 0 auto;
    min-width: 0;
    overflow: hidden;
  }

  .sidebar-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
  }

  .sidebar-title {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .toggle-btn {
    background: transparent;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    padding: 4px 8px;
    border-radius: 3px;
    font-size: 11px;
  }

  .toggle-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .tabs-row {
    display: flex;
    border-bottom: 1px solid var(--border);
  }

  .tab-button {
    flex: 1;
    background: transparent;
    border: none;
    border-bottom: 2px solid transparent;
    padding: 8px 12px;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 12px;
    margin-bottom: -1px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }

  .tab-button:hover {
    color: var(--text-primary);
  }

  .tab-button.active {
    color: var(--text-primary);
    border-bottom-color: var(--accent);
  }

  .dot {
    font-size: 8px;
    color: #34d399;
    line-height: 1;
  }

  .tab-content {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }

  .panel {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .rail {
    flex: 0 0 28px;
    background: var(--bg-secondary);
    border: none;
    border-left: 1px solid var(--border);
    color: var(--text-secondary);
    cursor: pointer;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: flex-start;
    padding: 12px 0;
    gap: 10px;
    font-size: 11px;
  }

  .rail:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .rail-arrow {
    font-size: 12px;
  }

  .rail-label {
    writing-mode: vertical-rl;
    transform: rotate(180deg);
    letter-spacing: 0.5px;
    white-space: nowrap;
  }

  .rail-dot {
    font-size: 10px;
    color: #34d399;
    line-height: 1;
  }
</style>
```

- [ ] **Step 2: Verify type-check is clean**

Run:

```bash
npm run check
```

Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
git add src/lib/pages/record/PatientContextSidebar.svelte
git commit -m "feat(record): PatientContextSidebar component with Structured/Notes tabs"
```

---

## Task 7: Rewire `RecordTab.svelte` and delete `PatientContextPanel.svelte`

**Files:**
- Modify: `src/lib/pages/RecordTab.svelte`
- Delete: `src/lib/pages/record/PatientContextPanel.svelte`

**Why:** Integration step where the sidebar becomes live. `RecordTab.svelte` switches from a vertical-stack layout to a horizontal split, wires the new components, and uses the `recordSidebar` store. The old `PatientContextPanel.svelte` is now unused and gets removed.

- [ ] **Step 1: Replace `RecordTab.svelte`**

Overwrite `src/lib/pages/RecordTab.svelte` with:

```svelte
<script lang="ts">
  import { audio } from '../stores/audio';
  import { settings } from '../stores/settings';
  import { pipeline } from '../stores/pipeline';
  import { recordings } from '../stores/recordings';
  import { importAudioFile, getRecording } from '../api/recordings';
  import { checkRecordingAudioLevels } from '../api/audio';
  import { copyWithStatus } from '../utils/clipboard';
  import { clampSidebarWidth } from '../utils/resize';
  import { recordSidebar } from '../stores/recordSidebar';
  import RecordingHeader from '../components/RecordingHeader.svelte';
  import ConfirmDialog from '../components/ConfirmDialog.svelte';
  import RecordingStateCards from './record/RecordingStateCards.svelte';
  import PipelineStatus from './record/PipelineStatus.svelte';
  import PatientContextSidebar from './record/PatientContextSidebar.svelte';
  import ResizeHandle from './record/ResizeHandle.svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { onMount } from 'svelte';
  import { contextTemplates } from '../stores/contextTemplates';
  import { toasts } from '../stores/toasts';
  import { rsvp } from '../stores/rsvp';
  import { formatError } from '../types/errors';
  import { buildPatientContext } from '../utils/patient_context';

  type Props = {
    onopenSettings?: (target: 'models' | 'audio') => void;
  };
  let { onopenSettings = () => {} }: Props = $props();

  // Patient-context text state — owned here because buildPatientContext(...) needs them at pipeline-launch time.
  let contextText = $state('');
  let medicationsText = $state('');
  let allergiesText = $state('');
  let conditionsText = $state('');

  // Sidebar UI state — synced with the persisted recordSidebar store.
  let sidebarOpen = $state(true);
  let sidebarWidth = $state(360);

  // Snapshot initial store values once on mount, then write back on toggle/resize-end.
  // (We avoid two-way reactive subscription to keep the data flow simple.)
  $effect(() => {
    const unsubOpen = recordSidebar.open.subscribe((v) => {
      sidebarOpen = v;
    });
    const unsubWidth = recordSidebar.width.subscribe((v) => {
      sidebarWidth = v;
    });
    return () => {
      unsubOpen();
      unsubWidth();
    };
  });

  function toggleSidebar() {
    recordSidebar.setOpen(!sidebarOpen);
  }

  function onSidebarResize(delta: number) {
    // Negative delta (drag handle left) = sidebar widens. The handle sits
    // to the LEFT of the sidebar, so dragging right narrows it.
    const next = clampSidebarWidth(
      sidebarWidth - delta,
      window.innerWidth,
      recordSidebar.MIN_WIDTH,
      recordSidebar.MAX_WIDTH,
      320,
    );
    sidebarWidth = next;
  }

  function onSidebarResizeEnd() {
    recordSidebar.setWidth(sidebarWidth);
  }

  // Re-clamp the sidebar width when the window resizes so the main area
  // always retains at least 320px. Persisted width stays untouched.
  $effect(() => {
    function handler() {
      const next = clampSidebarWidth(
        sidebarWidth,
        window.innerWidth,
        recordSidebar.MIN_WIDTH,
        recordSidebar.MAX_WIDTH,
        320,
      );
      if (next !== sidebarWidth) {
        sidebarWidth = next;
      }
    }
    window.addEventListener('resize', handler);
    return () => window.removeEventListener('resize', handler);
  });

  onMount(() => {
    contextTemplates.load();
  });

  // Import flow state
  let importedRecordingId = $state<string | null>(null);
  let importedFilename = $state<string | null>(null);
  let importing = $state(false);
  let importError = $state<string | null>(null);

  // Track the recording ID the current pipeline status refers to
  let pipelineRecordingId = $state<string | null>(null);

  // Silent-recording warning dialog state
  let silenceDialogOpen = $state(false);
  let silenceDialogRecordingId = $state<string | null>(null);
  let silenceDialogMessage = $state('');

  function clearAllContextFields() {
    // Both the freeform "Notes" box and the structured Patient Context
    // fields (medications / allergies / conditions) are tied to the
    // current encounter — fresh encounter, fresh form.
    contextText = '';
    medicationsText = '';
    allergiesText = '';
    conditionsText = '';
  }

  function handleStartRecording() {
    clearAllContextFields();
    importedRecordingId = null;
    importedFilename = null;
    importError = null;
    pipeline.clearCurrent();
    audio.startRecording();
  }

  function handleNewRecording() {
    clearAllContextFields();
    importedRecordingId = null;
    importedFilename = null;
    importError = null;
    pipeline.clearCurrent();
    audio.reset();
  }

  function describeSilence(rms: number): string {
    const rmsDb = rms > 0 ? 20 * Math.log10(rms) : -Infinity;
    const formatted = isFinite(rmsDb) ? `${rmsDb.toFixed(1)} dBFS` : 'digital silence';
    return (
      `The recording appears to contain no audio (${formatted}). ` +
      'Your microphone or audio routing likely isn’t capturing sound — ' +
      'processing this file will probably produce an unreliable transcript.'
    );
  }

  async function maybeLaunchPipeline(recordingId: string) {
    try {
      const levels = await checkRecordingAudioLevels(recordingId);
      if (levels.is_silent) {
        silenceDialogRecordingId = recordingId;
        silenceDialogMessage = describeSilence(levels.rms);
        silenceDialogOpen = true;
        return;
      }
    } catch (_e) {
      // If the silence check itself fails, don't block the pipeline.
    }
    pipeline.launch(recordingId, contextText || undefined, undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
  }

  async function warnIfSilent(recordingId: string) {
    try {
      const levels = await checkRecordingAudioLevels(recordingId);
      if (levels.is_silent) {
        silenceDialogRecordingId = recordingId;
        silenceDialogMessage = describeSilence(levels.rms);
        silenceDialogOpen = true;
      }
    } catch (_e) {
      // Silent failure is fine — this is advisory only.
    }
  }

  function confirmSilentProcess() {
    const id = silenceDialogRecordingId;
    silenceDialogOpen = false;
    silenceDialogRecordingId = null;
    if (id) {
      pipelineRecordingId = id;
      pipeline.launch(id, contextText || undefined, undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
    }
  }

  function dismissSilenceDialog() {
    silenceDialogOpen = false;
    silenceDialogRecordingId = null;
  }

  function handleStopRecording() {
    audio.stop().then(() => {
      const recordingId = $audio.lastRecordingId;
      if (!recordingId) return;

      pipelineRecordingId = recordingId;

      if ($settings.auto_generate_soap) {
        maybeLaunchPipeline(recordingId);
      } else {
        warnIfSilent(recordingId);
      }
    });
  }

  function handleProcessRecording() {
    const recordingId = $audio.lastRecordingId ?? importedRecordingId;
    if (!recordingId) return;
    pipelineRecordingId = recordingId;
    maybeLaunchPipeline(recordingId);
  }

  function handleRetry() {
    if (!pipelineRecordingId) return;
    pipeline.retry(pipelineRecordingId, contextText || undefined, undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
  }

  function handleCancelPipeline() {
    if (!pipelineRecordingId) return;
    pipeline.cancel(pipelineRecordingId);
  }

  async function handleUploadAudio() {
    importError = null;
    try {
      const selected = await open({
        multiple: false,
        filters: [
          { name: 'Audio Files', extensions: ['wav', 'mp3', 'ogg', 'flac', 'm4a', 'aac', 'wma', 'webm'] },
        ],
      });
      if (!selected) return;

      importing = true;
      const filePath = typeof selected === 'string' ? selected : selected;
      const recordingId = await importAudioFile(filePath);
      importedRecordingId = recordingId;
      importedFilename = filePath.split('/').pop()?.split('\\').pop() ?? 'audio file';
      await recordings.load();

      // Always launch — upload doesn't respect $settings.auto_generate_soap (live recording still does).
      pipelineRecordingId = recordingId;
      maybeLaunchPipeline(recordingId);
    } catch (e: any) {
      importError = formatError(e) || 'Import failed';
    } finally {
      importing = false;
    }
  }

  let copyStatus = $state<'idle' | 'copying' | 'copied'>('idle');

  async function handleCopySoap() {
    if (copyStatus !== 'idle') return;
    const rid = pipelineRecordingId;
    if (!rid) return;
    await copyWithStatus({
      setStatus: (s) => (copyStatus = s),
      getText: async () => {
        const rec = await getRecording(rid);
        return rec?.soap_note ?? undefined;
      },
      onError: (e) => toasts.error(`Failed to copy SOAP note: ${e}`),
    });
  }

  async function handleSpeedRead() {
    const rid = pipelineRecordingId;
    if (!rid) return;
    try {
      const rec = await getRecording(rid);
      if (rec?.soap_note) {
        rsvp.openSoap(rec.soap_note);
      } else {
        toasts.error('No SOAP note to read yet.');
      }
    } catch (e) {
      console.error('Failed to open speed reader:', e);
      toasts.error(`Failed to open speed reader: ${e}`);
    }
  }
</script>

<div class="record-tab">
  <RecordingHeader
    {onopenSettings}
    onStart={handleStartRecording}
    onStop={handleStopRecording}
    onNewRecording={handleNewRecording}
  />

  <div class="record-body">
    <div class="record-main">
      {#if $pipeline.current && pipelineRecordingId}
        <PipelineStatus
          bind:copyStatus
          onCancel={handleCancelPipeline}
          onRetry={handleRetry}
          onCopySoap={handleCopySoap}
          onSpeedRead={handleSpeedRead}
        />
      {:else}
        <RecordingStateCards
          {importedRecordingId}
          {importedFilename}
          {importing}
          {importError}
          onProcessRecording={handleProcessRecording}
          onUploadAudio={handleUploadAudio}
        />
      {/if}
    </div>

    {#if sidebarOpen}
      <ResizeHandle onResize={onSidebarResize} onResizeEnd={onSidebarResizeEnd} />
    {/if}

    <PatientContextSidebar
      bind:contextText
      bind:medicationsText
      bind:allergiesText
      bind:conditionsText
      open={sidebarOpen}
      width={sidebarWidth}
      onToggle={toggleSidebar}
    />
  </div>
</div>

<ConfirmDialog
  open={silenceDialogOpen}
  title="Silent recording detected"
  message={silenceDialogMessage}
  confirmLabel="Process anyway"
  cancelLabel="Cancel"
  danger
  onConfirm={confirmSilentProcess}
  onCancel={dismissSilenceDialog}
/>

<style>
  .record-tab {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .record-body {
    flex: 1;
    display: flex;
    min-height: 0;
    overflow: hidden;
  }

  .record-main {
    flex: 1;
    min-width: 320px;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 32px;
    overflow: auto;
  }
</style>
```

- [ ] **Step 2: Delete `PatientContextPanel.svelte`**

Run:

```bash
git rm src/lib/pages/record/PatientContextPanel.svelte
```

- [ ] **Step 3: Verify nothing else imports `PatientContextPanel`**

Run:

```bash
grep -rn "PatientContextPanel" src/ 2>/dev/null
```

Expected: no matches.

- [ ] **Step 4: Verify type-check is clean and tests still pass**

Run:

```bash
npm run check
npx vitest run
```

Expected: 0 type errors (one pre-existing `ExportDialog.svelte` warning is OK). All vitest tests pass — should be 183 (baseline) + 12 (recordSidebar) + 8 (resize helper) = 203.

- [ ] **Step 5: Commit**

```bash
git add src/lib/pages/RecordTab.svelte
git commit -m "feat(record): switch Record tab to horizontal split with right sidebar"
```

(The `git rm` from Step 2 is committed together with the modify — `git add` of the parent dir would also include the deletion, but the rm is already staged.)

---

## Task 8: Manual smoke test

**Files:** none (verification only).

**Why:** Per `CLAUDE.md`: UI / frontend changes require running the dev server and walking the feature in a browser. The Svelte component framework is not present in this repo, so manual smoke is the verification path for visual + interaction correctness.

- [ ] **Step 1: Start the dev environment**

Run in one terminal from the worktree root:

```bash
npm run tauri dev
```

Wait for the Tauri window to open.

- [ ] **Step 2: Verify the initial layout**

- [ ] Record tab loads. Patient Context sidebar is visible on the right at ~360px.
- [ ] The recording controls (timer, "Start Recording" button, waveform area) occupy the main area with comfortable spacing.
- [ ] The "Structured" tab is active by default; the three labelled textareas (Medications / Allergies / Known conditions) are visible.

- [ ] **Step 3: Verify tab switching and active dots**

- [ ] Click into the Medications field and type something. A green `●` appears next to the "Structured" tab label.
- [ ] Click the "Notes" tab. The structured fields disappear; the template picker + Save-as-template button + Notes textarea appear.
- [ ] Type into Notes. A green `●` appears next to the "Notes" tab label as well.
- [ ] Clear the Medications field. The `●` next to "Structured" disappears.

- [ ] **Step 4: Verify collapse / expand**

- [ ] Click the "▶" toggle button in the sidebar header. Sidebar collapses to a 28px vertical rail with "◀ Patient Context" rotated label and (if Notes still has content) a green `●` dot.
- [ ] Recording area now fills the available width minus the 28px rail.
- [ ] Click the rail. Sidebar re-expands at its previous width.
- [ ] Click toggle again. Restart the app (`Cmd+R` or close + reopen). The sidebar comes back collapsed (state persisted). Expand again before the next step.

- [ ] **Step 5: Verify resize handle**

- [ ] Hover the 6px divider between main and sidebar — cursor becomes `col-resize`; divider tints to accent color.
- [ ] Drag left: sidebar widens smoothly. Drag right: it narrows.
- [ ] Drag far past 600px right: width clamps. Drag far past 280px left: width clamps.
- [ ] Release. Reload the app. The new width persists.

- [ ] **Step 6: Verify viewport shrink behavior**

- [ ] Resize the app window narrow enough that the sidebar + 320px main area cannot both fit. The sidebar shrinks proportionally; main area is never narrower than 320px.
- [ ] Widen the window back. The sidebar returns toward its persisted width up to the clamp limits.

- [ ] **Step 7: Verify pipeline integration**

- [ ] Fill both Structured (Medications) and Notes fields with distinctive text.
- [ ] Record a short audio clip (5–10 sec).
- [ ] Verify the pipeline launches and the Subjective / Plan sections of the generated SOAP reflect both the structured medications and the freeform notes content (regression: confirm both fields still flow through to the backend).

- [ ] **Step 8: Verify "New Recording" clears context but not sidebar UI state**

- [ ] After a recording, click "+ New Recording".
- [ ] All four text fields clear; both tab dots disappear.
- [ ] Sidebar open/collapsed state and width are unchanged.

- [ ] **Step 9: Verify Save-as-template still works**

- [ ] On the Notes tab, type something and click "Save as template". Modal appears above everything else.
- [ ] Enter a name and Save. Modal closes. Template appears in the dropdown.
- [ ] Apply the saved template. The text inserts into the Notes box and the "Notes" tab dot appears if it wasn't already.

- [ ] **Step 10: Verify no PHI leak in logs**

Watch the dev terminal during Steps 3–9. Per the project's hard constraint, none of the Notes / Medications / Allergies / Conditions content may appear in `console.log` or `tracing::*` output. The new code in this plan introduces no such logging — just confirm by visual inspection of the terminal.

- [ ] **Step 11: Final cleanup commit (if needed)**

If smoke surfaced anything that needs fixing, fix it now and commit:

```bash
git add <files>
git commit -m "fix(record): <what>"
```

If nothing needs fixing, proceed to finishing-a-development-branch.

---

## Self-review notes

- **Spec coverage:** every section of the spec maps to a task.
  - `recordSidebar.ts` → Task 1.
  - `clampSidebarWidth` → Task 2.
  - `ResizeHandle.svelte` → Task 3.
  - `PatientContextStructuredTab.svelte` → Task 4.
  - `PatientContextNotesTab.svelte` → Task 5.
  - `PatientContextSidebar.svelte` (with tabs + rail) → Task 6.
  - Updated `RecordTab.svelte` + deletion of `PatientContextPanel.svelte` → Task 7.
  - Manual smoke (replacing the spec's "Component tests" since `@testing-library/svelte` is absent) → Task 8.
- **Persisted vs in-memory width during drag.** Spec calls for in-memory updates during drag with persist on `pointerup`. Task 7's `onSidebarResize` updates `sidebarWidth` (in-memory) every move; `onSidebarResizeEnd` calls `recordSidebar.setWidth` (persists). Matches spec.
- **Store pattern.** Spec mentioned a rune-based store; the plan uses `svelte/store` `writable` instead, matching the project's existing `theme.ts` pattern. The contract is the same (`subscribe` for reactive read; `setOpen` / `setWidth` for write). Easier to unit-test under vitest because no Svelte runtime is required.
- **Active dot signal.** `structuredHasContent` and `notesHasContent` are `$derived` in the sidebar from bound text props; this matches the spec's per-tab indicator behavior and removes any need for the parent to compute the chip state.
- **No PHI leak.** Plan-introduced `console.log` / `tracing` calls: zero. Errors logged are stringified `e` from invokes (DB error strings, never generation content).
- **Type consistency.** `Tab = 'structured' | 'notes'`, `MIN_WIDTH=280`, `MAX_WIDTH=600`, `DEFAULT_WIDTH=360`, `mainMin=320` used identically across every task that references them.
- **No backend changes** — confirmed by inspecting the plan: zero edits under `src-tauri/` or `crates/`.
