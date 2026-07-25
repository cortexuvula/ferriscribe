# Generate Tab UI Improvements

**Date:** 2026-07-25
**Status:** Draft

## Problem

The Generate tab has four UX gaps identified in a thorough review:

1. **No inline preview** — Users can generate documents but never see them in the Generate tab. They must navigate to the Editor tab or Copy-elsewhere to read what was produced.
2. **Missing context feedback** — The "Active" badge ignores Notes and OCR text (the largest context sources). No character counter despite a hard 50K limit that produces a confusing post-hoc error.
3. **Weak progress/error feedback** — Progress text is in a global banner far from the generating row. Failed rows get no visual marker. No success feedback.
4. **Cluttered layout** — Letter/peer-discussion fields stay expanded after generation. Empty state is passive.

## Solution

Four targeted improvements to the existing Generate tab components, no new components needed.

## Improvement 1: Inline Preview

**Files:** `GenerateItem.svelte`, `GenerateTab.svelte`

Add a `generatedText?: string | null` prop to `GenerateItem`. When the item is in the `done` state and `generatedText` is non-empty, render a collapsible preview:

```svelte
{#if done && generatedText}
  <details class="generated-preview">
    <summary>Preview</summary>
    <pre class="preview-text">{generatedText}</pre>
  </details>
{/if}
```

In `GenerateTab.svelte`, pass the appropriate field from the selected recording:
- SOAP → `recordings.selectedRecording.soap_note`
- Referral → `recordings.selectedRecording.referral`
- Letter → `recordings.selectedRecording.letter`
- Peer Discussion → `recordings.selectedRecording.peer_discussion`

CSS: `max-height: 300px; overflow-y: auto; white-space: pre-wrap; font-size: 13px`.

## Improvement 2: Context Feedback

**Files:** `GenerateTab.svelte`, `ContextPanel.svelte`

### Badge fix

Update `hasActiveContext` in `GenerateTab.svelte` to include notes and OCR text:

```typescript
const hasActiveContext = $derived(
  contextText.trim().length > 0 ||
    medicationsText.trim().length > 0 ||
    allergiesText.trim().length > 0 ||
    conditionsText.trim().length > 0 ||
    ocrTextDisplay.trim().length > 0,
);
```

### Character counter

Add a `contextCharCount` derived value and pass it to `ContextPanel` as a prop:

```typescript
const contextCharCount = $derived(
  contextText.length + ocrTextDisplay.length
);
```

In `ContextPanel.svelte`, add a small counter below the Notes textarea:

```svelte
<span class="char-counter" class:warning={contextCharCount > 40000}>
  {contextCharCount.toLocaleString()} / 50,000 chars
</span>
```

CSS: amber color when > 40,000, red when > 50,000.

## Improvement 3: Progress/Error Feedback

**Files:** `GenerateControls.svelte`, `GenerateItem.svelte`, `generation.svelte.ts`

### Inline progress

Pass `progressStatus` to the currently-generating `GenerateItem`. Show it below the spinner:

```svelte
{#if generating}
  <button disabled>
    <span class="spinner"></span> Generating...
  </button>
  {#if progressText}
    <span class="progress-phase">{progressText}</span>
  {/if}
{/if}
```

Add `role="status" aria-live="polite"` to the progress phase element.

### Per-row error indicator

Add a `failed?: boolean` prop to `GenerateItem`. When true, add a red left border + error icon:

```svelte
<div class="generate-item" class:failed>
```

CSS: `.failed { border-left: 3px solid var(--danger); padding-left: 8px; }`

In `GenerateControls.svelte`, compute per-item failed state from `generation.state.lastFailedType`:

```svelte
<GenerateItem failed={generation.state.lastFailedType === 'soap' && generation.state.error} />
```

### Success toast

In `GenerateTab.handleGenerate`, after `generation.finish()`:

```typescript
toasts.success(`${type.toUpperCase()} note generated`);
```

## Improvement 4: Layout De-clutter + Empty State CTA

**Files:** `GenerateControls.svelte`, `GenerateTab.svelte`

### Collapse letter fields after generation

In `GenerateControls.svelte`, add a `fieldsExpanded` state per card. When the document is `done` and fieldsExpanded is false, show a compact summary line:

```svelte
{#if done && !fieldsExpanded}
  <div class="compact-settings" onclick={() => fieldsExpanded = true}>
    📋 {audienceName} · {letterType || 'general'} · Edit
  </div>
{:else}
  <div class="letter-card-header">
    <!-- existing fields -->
  </div>
{/if}
```

Default `fieldsExpanded = true` (expanded before first generation, collapsed after).

### Empty state CTA

In `GenerateTab.svelte`, add a button to the empty state:

```svelte
<div class="empty-state">
  <div class="empty-icon">⚡</div>
  <h2>Generate Documentation</h2>
  <p>Select a recording from the <strong>Recordings</strong> tab first.</p>
  <button class="btn-primary" onclick={() => onNavigateRecordings?.()}>
    Go to Recordings
  </button>
</div>
```

Add an optional `onNavigateRecordings?: () => void` prop to `GenerateTab`.

## Files Changed

| File | Change |
|------|--------|
| `GenerateItem.svelte` | Inline preview, per-row error border, inline progress text |
| `GenerateControls.svelte` | Collapse fields after generation, pass progress/failed state |
| `GenerateTab.svelte` | Badge fix, char counter, success toast, empty state CTA, pass generatedText |
| `ContextPanel.svelte` | Char counter display |
| `generation.svelte.ts` | No changes (existing state is sufficient) |

## Testing

- Existing vitest tests should pass unchanged (no prop removals, only additions)
- Manual: generate a SOAP note, verify inline preview appears
- Manual: type 45K chars in Notes, verify counter turns amber
- Manual: trigger a model error, verify the specific row gets a red border
- Manual: generate a letter, verify fields collapse to compact summary
