# OCR Document Support in the Record Tab

**Date:** 2026-07-23
**Status:** Draft
**Branch:** `feature/ocr-record-tab`

## Problem

The OCR document drop zone currently exists only in the Generate tab's Context Panel. Users who want to include lab results, referral letters, or other clinical documents during the recording workflow (where SOAP is auto-generated) must wait until the recording is done, then navigate to the Generate tab to drop documents. This breaks the clinical workflow — the user wants to drop documents while the recording session is active, before SOAP generation runs.

## Solution

Extract the OCR drop zone into a shared `OcrDropZone.svelte` component, add it to the Record tab's `PatientContextSidebar`, and thread the extracted text into the pipeline's `context` parameter alongside the notes field. The existing OCR backend (`ocr_documents` command, `extract_text` pipeline) is fully reused — no backend changes needed.

## Key Decisions

1. **Shared component** — Extract `OcrDropZone.svelte` from `ContextPanel.svelte` so both the Record and Generate tabs use it. Avoids duplicating ~100 lines of Svelte and the Tauri drag-drop listener.
2. **Transient OCR** — OCR text is session-scoped (cleared on new recording / tab switch). The text gets baked into `metadata.context` after SOAP generation, so it persists naturally as part of the recording's context without a separate metadata key.
3. **No backend changes** — The pipeline's `context: Option<String>` parameter is the injection point. OCR text is combined with notes before calling `pipeline.launch()`.
4. **Combined context** — OCR text is concatenated with the notes field (same pattern as GenerateTab), not passed as a separate channel.

## Architecture

### Component extraction: `OcrDropZone.svelte`

New component at `src/lib/components/OcrDropZone.svelte` containing:
- The drop zone div (with Tauri native `onDragDropEvent` listener)
- File status chips (loading/done/error with remove button)
- Loading indicator
- Collapsible editable preview textarea
- All CSS styles currently inline in ContextPanel

**Props interface:**
```typescript
interface OcrFileStatus {
  id: string;
  filename: string;
  status: 'done' | 'loading' | 'error';
  pageCount: number;
  text?: string;
  path?: string;
}

interface Props {
  ocrFiles: OcrFileStatus[];
  ocrText: string;
  ocrLoading: boolean;
  onOcrFilesSelected: (paths: string[]) => void;
  onOcrTextChange: (text: string) => void;
  onRemoveOcrFile: (id: string) => void;
}
```

The component owns the `isDragging` visual state and the `$effect` that registers the Tauri `onDragDropEvent` listener. The listener cleans up on destroy (unmount).

### Refactor: `ContextPanel.svelte`

Replace the inline OCR section (drop zone + chips + preview + CSS) with:
```svelte
<OcrDropZone
  {ocrFiles}
  {ocrText}
  {ocrLoading}
  {onOcrFilesSelected}
  {onOcrTextChange}
  {onRemoveOcrFile}
/>
```

The props and their defaults stay on ContextPanel — it just delegates rendering.

### Wire into `PatientContextSidebar.svelte`

Add the same OCR props to `PatientContextSidebar`'s interface, and render `<OcrDropZone>` at the bottom of the sidebar (after the Notes tab content). The props are passed through from `RecordTab.svelte`.

### State in `RecordTab.svelte`

Mirror the GenerateTab pattern:
```typescript
let ocrFiles = $state<OcrFileStatus[]>([]);
let ocrLoading = $state(false);
let ocrTextOverride = $state<string | null>(null);
// Derived from done-file texts
let ocrText = $derived(/* join done files */);
let ocrTextDisplay = $derived(ocrTextOverride ?? ocrText);
```

Handlers: `handleOcrFilesSelected`, `handleOcrTextChange`, `handleRemoveOcrFile` — identical logic to GenerateTab's implementations.

### Threading into the pipeline

In every call site where `pipeline.launch()` is invoked in RecordTab, combine OCR text with notes:
```typescript
const combinedContext = [contextText.trim(), ocrTextDisplay.trim()]
  .filter(Boolean).join('\n\n') || undefined;
pipeline.launch(recordingId, combinedContext, undefined, buildPatientContext(...));
```

Call sites to update:
1. `maybeLaunchPipeline(recordingId)` — line ~177
2. `confirmSilentProcess()` — line ~199
3. `handleRetry(recordingId)` — line ~232
4. `handleRegenerateSoap()` — line ~247

### Clear OCR state

In `clearAllContextFields()` (called on new recording / start recording), add:
```typescript
ocrFiles = [];
ocrTextOverride = null;
```

### Tauri drag-drop listener management

The `$effect` in `OcrDropZone` registers a window-level `onDragDropEvent` listener. Since only one tab is visible at a time and tab components unmount on switch (verified by the existing clear-on-switch behavior), the listener lifecycle is safe. If tab switching does not unmount the inactive tab, the drop callback will fire on both instances — but since both check `onOcrFilesSelected`, only the visible one's handler will be active (the inactive one's state is irrelevant).

## Privacy & Security

- All OCR runs locally through Ollama/LM Studio — no data leaves the machine
- OCR'd text is PHI — treated identically to the notes field (flows through `context` param, never logged)
- File paths are transient — only extracted text is held in memory

## Files Changed

| File | Change |
|------|--------|
| `src/lib/components/OcrDropZone.svelte` | **New** — extracted shared component |
| `src/lib/components/ContextPanel.svelte` | Replace inline OCR UI with `<OcrDropZone>` |
| `src/lib/pages/record/PatientContextSidebar.svelte` | Add OCR props + render `<OcrDropZone>` |
| `src/lib/pages/RecordTab.svelte` | OCR state + handlers + context threading + clear |

No backend changes. No new dependencies.

## Testing

- **Frontend tests**: Verify OcrDropZone renders, file chips show status, remove button works, preview is editable
- **Existing tests**: ContextPanel tests still pass (props unchanged)
- **Manual**: Drop a document in the Record tab sidebar, verify text appears in preview, start recording, verify SOAP generation includes the OCR text
