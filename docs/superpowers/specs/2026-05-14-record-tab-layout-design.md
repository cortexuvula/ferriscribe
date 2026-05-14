# Record Tab Layout Redesign

**Status:** Draft — pending user review
**Date:** 2026-05-14
**Author:** Brainstorming session with Claude Code

## Goal

Stop the Record tab's Patient Context block from squashing the recording controls when it expands. Move Patient Context into a permanent (toggleable) right sidebar so the timer, waveform, and pipeline status are always visible and stable in size, no matter how much content the clinician has entered.

## Problem

The current `RecordTab.svelte` lays out three children in a single flex column:

1. `PatientContextPanel` (collapsible accordion, top)
2. `RecordingHeader` (file name + audio level indicators)
3. `record-content` (flex:1, centered — shows `PipelineStatus` or `RecordingStateCards`)

When Patient Context is expanded — and especially when the Notes textarea has long pasted content (e.g., an Ocean intake questionnaire) — the panel can consume 60-70% of the vertical viewport. The recording timer, waveform, and pipeline status get pushed below the fold. On 13-inch laptops the user has to scroll just to see whether transcription is in progress.

The motivating screenshot (2026-05-14) shows a 21:28 timer and pipeline status pushed almost entirely off-screen with a fully expanded Patient Context block above.

## Non-goals

- **Mobile / narrow viewport responsiveness.** The app is desktop-only. The redesign degrades gracefully on narrow windows (see Width clamping) but does not add a phone layout.
- **Reworking the recording controls themselves.** Timer, waveform, and pipeline status all keep their current implementations and behavior.
- **Reworking the pipeline-launch contract.** `pipeline.launch(...)` still receives `contextText` and the structured `PatientContext` exactly as today — no new backend, no new commands, no new schema.
- **Changes to other tabs** (Generate, Chat, Recordings, etc.). This is Record-only.
- **Reworking template management.** Save-as-template, Apply-template, and the `contextTemplates` store all keep their current behavior; the modal and picker just move into the sidebar.

## Hard constraints honored

- **Local-only AI providers.** Layout-only change — no AI/network surface touched.
- **No PHI in logs.** New components don't log textarea contents, just structural events (open/close, resize delta).
- **Frontend stack unchanged.** Svelte 5 runes, no new libraries.

## Decisions captured from brainstorming

| Question | Choice |
|---|---|
| Where does Patient Context live? | **Right sidebar (always-visible, toggleable).** |
| Where does the Notes field go? | **Inside the sidebar, in tabs alongside Structured fields.** |
| Sidebar visibility behavior | **Collapsible with a toggle button.** |
| Sidebar width | **User-resizable via drag handle, persisted to localStorage.** |
| Active-state indicator | **Per-tab dot — one for Structured (any of meds/allergies/conditions filled), one for Notes.** |

## Architecture overview

```
src/lib/pages/RecordTab.svelte                          MODIFIED (replace .record-tab body)
src/lib/pages/record/
├── PatientContextPanel.svelte                          DELETED
├── PatientContextSidebar.svelte                        NEW (replaces above)
├── PatientContextStructuredTab.svelte                  NEW (3 textareas + labels)
├── PatientContextNotesTab.svelte                       NEW (template picker + textarea + Save modal)
├── ResizeHandle.svelte                                 NEW (6px drag bar)
├── PipelineStatus.svelte                               UNCHANGED
└── RecordingStateCards.svelte                          UNCHANGED

src/lib/stores/recordSidebar.ts                         NEW (rune-based persistence)
src/lib/stores/recordSidebar.test.ts                    NEW (unit tests)
src/lib/utils/resize.ts                                 NEW (clamping helper)
src/lib/utils/resize.test.ts                            NEW (unit tests)
```

The two tabs (`PatientContextStructuredTab` and `PatientContextNotesTab`) are split out of the original `PatientContextPanel` so each file has one clear responsibility. The original panel mixed three textareas with a template picker, a Save-as-template modal, and the badge-active logic — splitting along the tab boundary makes each piece small and individually understandable.

`RecordTab.svelte` keeps ownership of the four text values (`contextText`, `medicationsText`, `allergiesText`, `conditionsText`) because the pipeline launch flow already reads them at action time. The new sidebar only re-routes UI plumbing.

## Component contracts

### `recordSidebar.ts`

Lightweight Svelte 5 rune-based store with localStorage persistence:

```ts
const SIDEBAR_OPEN_KEY = 'record.sidebar.open';
const SIDEBAR_WIDTH_KEY = 'record.sidebar.width';
const DEFAULT_WIDTH = 360;
const MIN_WIDTH = 280;
const MAX_WIDTH = 600;

let _open = $state<boolean>(readOpen());
let _width = $state<number>(readWidth());

export const recordSidebar = {
  get open() { return _open; },
  set open(v: boolean) {
    _open = v;
    try { localStorage.setItem(SIDEBAR_OPEN_KEY, v ? 'true' : 'false'); } catch {}
  },
  get width() { return _width; },
  set width(v: number) {
    _width = clamp(v, MIN_WIDTH, MAX_WIDTH);
    try { localStorage.setItem(SIDEBAR_WIDTH_KEY, String(_width)); } catch {}
  },
};

function readOpen(): boolean {
  try {
    const v = localStorage.getItem(SIDEBAR_OPEN_KEY);
    return v !== 'false'; // open-on-doubt: any non-"false" → true
  } catch { return true; }
}

function readWidth(): number {
  try {
    const v = Number(localStorage.getItem(SIDEBAR_WIDTH_KEY));
    if (!Number.isFinite(v) || v <= 0) return DEFAULT_WIDTH;
    return clamp(v, MIN_WIDTH, MAX_WIDTH);
  } catch { return DEFAULT_WIDTH; }
}

function clamp(n: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, n));
}
```

To avoid 60-writes-per-second to localStorage during a drag, callers update the in-memory width every frame for smooth UI feedback and call the setter (which persists) only on `pointerup`. The store doesn't enforce this — the caller (`RecordTab.svelte`) controls it.

### `resize.ts`

Pure helper used by the resize-handle drag handler and by viewport-shrink handling:

```ts
export function clampSidebarWidth(
  requested: number,
  viewportWidth: number,
  min: number,         // 280
  max: number,         // 600
  mainMin: number,     // 320
): number {
  // First clamp to [min, max].
  let w = Math.max(min, Math.min(max, requested));
  // Then ensure the main area gets at least mainMin px.
  const viewportAllows = viewportWidth - mainMin;
  if (viewportAllows < w) w = Math.max(min, viewportAllows);
  return w;
}
```

Pure function; deterministic; trivially unit-testable. No DOM access.

### `ResizeHandle.svelte`

```ts
{
  onResize: (deltaPx: number) => void,   // fired during drag (negative = sidebar wider)
  onResizeEnd: () => void,                // fired on pointerup (caller persists)
}
```

Implementation outline:

```svelte
<script lang="ts">
  let { onResize, onResizeEnd }: Props = $props();
  let startX = 0;
  let dragging = false;

  function pointerdown(ev: PointerEvent) {
    dragging = true;
    startX = ev.clientX;
    (ev.target as HTMLElement).setPointerCapture(ev.pointerId);
    ev.preventDefault();
  }
  function pointermove(ev: PointerEvent) {
    if (!dragging) return;
    onResize(ev.clientX - startX);
    startX = ev.clientX;
  }
  function pointerup(ev: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    (ev.target as HTMLElement).releasePointerCapture(ev.pointerId);
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
></div>

<style>
  .resize-handle {
    width: 6px;
    cursor: col-resize;
    background: var(--border);
    flex: 0 0 6px;
    user-select: none;
  }
  .resize-handle:hover {
    background: var(--accent);
  }
</style>
```

ARIA: `role="separator"` with `aria-orientation="vertical"` is the correct semantic for a resize divider; the `aria-label` is read by screen readers when the handle has focus (though keyboard-driven resize is out of scope — drag is mouse/touch only).

### `PatientContextSidebar.svelte`

Container component. Props:

```ts
{
  // Bound text state — owned by RecordTab.svelte, identical to today's contract
  contextText: $bindable<string>,
  medicationsText: $bindable<string>,
  allergiesText: $bindable<string>,
  conditionsText: $bindable<string>,
  // UI state — bound to recordSidebar store via RecordTab
  open: $bindable<boolean>,
  width: $bindable<number>,
}
```

Internal state:
```ts
let activeTab: 'structured' | 'notes' = $state('structured');
```

Derived (passes down to tabs):
```ts
const structuredHasContent = $derived(
  medicationsText.trim().length > 0 ||
  allergiesText.trim().length > 0 ||
  conditionsText.trim().length > 0
);
const notesHasContent = $derived(contextText.trim().length > 0);
```

Renders three states:

**Open:** width set via `style="width: {width}px"`. Header with title and "▶ Hide context" toggle. Tabs row with the two tab buttons (each label gets a green `•` when its `*HasContent` is true). Below the tabs, mounts either `<PatientContextStructuredTab … />` or `<PatientContextNotesTab … />` based on `activeTab`.

**Collapsed:** 28px-wide vertical rail with `aria-label="Show patient context"`. Vertically-oriented "Patient Context" label. Green `•` dot when `structuredHasContent || notesHasContent`. Entire rail is clickable.

Width transitions between the two states are not animated in the MVP (instant toggle keeps reasoning simple).

### `PatientContextStructuredTab.svelte`

```ts
{
  medicationsText: $bindable<string>,
  allergiesText: $bindable<string>,
  conditionsText: $bindable<string>,
}
```

Renders the three labeled textareas (Medications / Allergies / Known conditions) exactly as today's `PatientContextPanel` — same labels, same placeholders, same row counts. No new behavior.

### `PatientContextNotesTab.svelte`

```ts
{
  contextText: $bindable<string>,
}
```

Owns:
- The template picker `<select>` + `applyTemplate` logic
- The "Save as template" button + Save modal
- The Notes `<textarea>`
- Internal state: `selectedTemplate`, `saveModalOpen`, `saveModalName`, `saveModalError`, `saveModalOverwriteConfirm`

All template-related code moves here from the original `PatientContextPanel.svelte`. The `contextTemplates` store and the `upsertContextTemplate` API are unchanged — only the call site moves.

The Save modal uses `position: fixed` (existing behavior), so it sits above the entire app, not just the sidebar.

### Updated `RecordTab.svelte`

The template body changes from:

```svelte
<div class="record-tab">
  <PatientContextPanel … />
  <RecordingHeader … />
  <div class="record-content">…</div>
</div>
```

To:

```svelte
<div class="record-tab">
  <RecordingHeader … />
  <div class="record-body">
    <div class="record-main">
      {#if $pipeline.current && pipelineRecordingId}
        <PipelineStatus … />
      {:else}
        <RecordingStateCards … />
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
      bind:open={sidebarOpen}
      bind:width={sidebarWidth}
    />
  </div>
</div>
```

with these additions to the script:

```ts
import { recordSidebar } from '../stores/recordSidebar';
import { clampSidebarWidth } from '../utils/resize';

let sidebarOpen = $state(recordSidebar.open);
let sidebarWidth = $state(recordSidebar.width);

// During drag: update in-memory only (smooth feedback).
let dragWidth = $state(sidebarWidth);
function onSidebarResize(delta: number) {
  // Negative delta moves the handle left = sidebar wider.
  const next = clampSidebarWidth(
    dragWidth - delta,
    window.innerWidth,
    280, 600, 320,
  );
  dragWidth = next;
  sidebarWidth = next;
}
function onSidebarResizeEnd() {
  recordSidebar.width = sidebarWidth;
  dragWidth = sidebarWidth;
}

// Persist open state when sidebar toggles.
$effect(() => { recordSidebar.open = sidebarOpen; });

// Re-clamp on viewport resize.
$effect(() => {
  const handler = () => {
    sidebarWidth = clampSidebarWidth(
      sidebarWidth, window.innerWidth, 280, 600, 320,
    );
  };
  window.addEventListener('resize', handler);
  return () => window.removeEventListener('resize', handler);
});
```

The `contextCollapsed` local state is removed (replaced by `sidebarOpen` from the store). Everything else in `RecordTab.svelte` is untouched.

CSS additions:

```css
.record-body {
  flex: 1;
  display: flex;
  min-height: 0;        /* allows children to scroll properly */
}
.record-main {
  flex: 1;
  min-width: 320px;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 32px;
}
```

The old `.record-content` rule moves into `.record-main` (same styles).

## Data flow

```
User types in a sidebar textarea
  → PatientContextStructuredTab (or NotesTab) updates its $bindable prop
  → propagates up to PatientContextSidebar
  → propagates up to RecordTab.svelte's $state
  → on next pipeline.launch(), reads the same $state values
  → unchanged backend flow

User clicks "▶ Hide context"
  → PatientContextSidebar sets open = false
  → propagates up to RecordTab's sidebarOpen
  → $effect fires: recordSidebar.open = false
  → store persists to localStorage
  → next mount restores from localStorage

User drags resize handle
  → ResizeHandle fires onResize(delta) on every pointermove
  → RecordTab.onSidebarResize clamps + sets sidebarWidth (in-memory)
  → PatientContextSidebar re-renders with new width
  → On pointerup: ResizeHandle fires onResizeEnd
  → RecordTab.onSidebarResizeEnd writes recordSidebar.width (persists)

User resizes the app window
  → window 'resize' listener fires
  → RecordTab clamps sidebarWidth (in-memory only — persisted width preserved)
```

## Error handling

| Scenario | Behavior |
|---|---|
| localStorage read throws (private mode, disabled) | Defaults: `open=true`, `width=360`. Try/catch in store. |
| localStorage value is NaN / negative / huge | Clamped to `[280, 600]` on read. |
| localStorage write fails (quota) | Swallowed silently. In-memory value still updates. |
| Persisted `width` is wider than viewport allows | `clampSidebarWidth` reduces it in-memory; persisted value preserved for when viewport widens. |
| User drags past min/max | Width clamps at `280` / `600`; cursor remains `col-resize` until release. |
| Viewport too narrow to fit sidebar + 320px main | Sidebar shrinks to `max(280, viewport - 320)`. If even 280 doesn't fit, sidebar is 280 and main may scroll. |
| User collapses sidebar with focused textarea | Textarea unmounts; bound text in parent is preserved. |
| Save-as-template modal open while sidebar collapses | Modal stays open (it's `position: fixed`); user closes via × or backdrop click as today. |
| `clearAllContextFields()` called (Start/New Recording) | All four text values cleared; sidebar open/width state untouched. |

## Width and viewport behavior

| Viewport content width | Sidebar behavior |
|---|---|
| ≥ 1100px and open | Sidebar at persisted width (up to 600px). |
| 900–1099px and open | Sidebar reduces to `viewport - 320` if needed; persisted width preserved. |
| < 900px and open | Sidebar at 280 (min); if even that overflows, main area scrolls horizontally. |
| Collapsed | 28px rail regardless of viewport. |

Manual collapse is always available regardless of width.

## Accessibility

- Sidebar header has `<h2 class="visually-hidden">Patient Context</h2>` for screen-reader navigation.
- Toggle buttons:
  - When open: `aria-label="Hide patient context sidebar" aria-expanded="true"`
  - When collapsed: `aria-label="Show patient context sidebar" aria-expanded="false"`
- Tabs row uses `role="tablist"`; each tab uses `role="tab"` with `aria-selected={activeTab === 'structured'}` and `aria-controls` linking to the corresponding panel id.
- The tab panel uses `role="tabpanel" aria-labelledby="…"`.
- The resize handle uses `role="separator" aria-orientation="vertical" aria-label="Resize patient context sidebar"`.
- The per-tab `•` indicator is visually decorative; screen readers get `aria-label="Structured tab — has content"` style label on the tab when the dot is present.

Keyboard navigation: Tab moves between toggle button → tab buttons → focused tabpanel content. Arrow keys are not implemented for tab switching in this MVP (keep scope tight; existing app doesn't use arrow-key tab nav elsewhere).

## Testing

### Vitest unit tests

**`src/lib/stores/recordSidebar.test.ts`:**

- Defaults when localStorage is empty (`open=true`, `width=360`).
- `open` round-trips through localStorage: set `false` → re-read → `false`.
- `open` reads `"true"` / `"false"` correctly; anything else (including missing) → `true`.
- `width` clamps to `[280, 600]` on read (`"100"` → `280`, `"9999"` → `600`).
- `width` clamps to `[280, 600]` on write.
- `width` non-numeric storage (`"abc"`, empty, missing) → `360`.
- `setItem` throwing (mocked) doesn't bubble; in-memory value still updates.

**`src/lib/utils/resize.test.ts`:**

- `clampSidebarWidth(400, 2000, 280, 600, 320)` → `400` (no clamp).
- `clampSidebarWidth(700, 2000, 280, 600, 320)` → `600` (max).
- `clampSidebarWidth(200, 2000, 280, 600, 320)` → `280` (min).
- `clampSidebarWidth(500, 700, 280, 600, 320)` → `380` (viewport allows only `700-320=380`).
- `clampSidebarWidth(500, 500, 280, 600, 320)` → `280` (viewport too narrow — min wins).
- `clampSidebarWidth(280, 600, 280, 600, 320)` → `280` (exactly min on a viewport that exactly fits).

No component tests — Svelte component framework not present in repo (verified earlier in Training Corpus work).

### Manual smoke (per `CLAUDE.md`)

Launch `npm run tauri dev`:

- Record tab loads with sidebar on the right at 360px, "Structured" tab active.
- Type into Medications field: `•` appears on Structured tab label.
- Switch to Notes tab: `•` still on Structured. Type in Notes: `•` appears on Notes tab.
- Click "▶ Hide context": sidebar collapses to 28px rail; both `•` indicators consolidate into one rail dot.
- Click rail: sidebar re-expands at previous width.
- Drag resize handle left: sidebar widens smoothly. Release. Reload app. Width persists.
- Drag resize handle to min (280) and max (600); confirm clamping.
- Resize app window narrow: sidebar shrinks to fit; main area keeps ≥ 320px.
- Restart the app: sidebar open state + width both persist.
- Apply a template from Notes tab: inserts text, switches to Notes tab if not active.
- Save current Notes as new template: modal opens above sidebar; saving works as before.
- Start a recording with content in both tabs. Confirm pipeline launches with both structured and free text reaching the backend (no regression in `pipeline.launch` payload).
- Click "+ New Recording": all four text fields clear; sidebar open/width unchanged.

### Verification

`npm run check` clean, `npx vitest run` green, `cargo test --workspace --lib` unaffected (no backend changes).

## Open questions

None blocking. Future iterations might:
- Animate the open/close transition (instant toggle in MVP keeps reasoning simple).
- Add a keyboard shortcut to toggle the sidebar (e.g., `[`).
- Move the sidebar to the left for left-handed users (config option).
- Sync the active tab to localStorage so it persists across reloads.

## Implementation order

1. **`recordSidebar.ts` + tests** — pure persistence layer, no UI dependency.
2. **`resize.ts` + tests** — pure clamping helper.
3. **`ResizeHandle.svelte`** — small standalone component.
4. **`PatientContextStructuredTab.svelte`** — extract from existing `PatientContextPanel`.
5. **`PatientContextNotesTab.svelte`** — extract template + modal logic.
6. **`PatientContextSidebar.svelte`** — compose the two tabs + header + tabs row.
7. **Update `RecordTab.svelte`** — new `.record-body` layout, wire the sidebar in, delete `contextCollapsed`.
8. **Delete `PatientContextPanel.svelte`.**
9. **Manual smoke.**

Each step is independently testable. Steps 1–2 are TDD-driven; steps 3–7 are presentational and verified at the end by manual smoke.
