# OCR in Record Tab — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a document OCR drop zone to the Record tab's patient context sidebar so users can drop documents during a recording session and have the extracted text included in the auto-generated SOAP note.

**Architecture:** Extract the existing OCR UI (drop zone, file chips, preview) from `ContextPanel.svelte` into a shared `OcrDropZone.svelte` component. Add it to the Record tab's `PatientContextSidebar`. Thread OCR text into the pipeline's `context` parameter alongside the notes field. No backend changes.

**Tech Stack:** Svelte 5 (runes mode) / TypeScript / Tauri 2.0

---

## File Map

### Create
- `src/lib/components/OcrDropZone.svelte` — shared OCR drop zone component

### Modify
- `src/lib/components/ContextPanel.svelte` — replace inline OCR UI with `<OcrDropZone>`
- `src/lib/pages/record/PatientContextSidebar.svelte` — add OCR props + render `<OcrDropZone>`
- `src/lib/pages/RecordTab.svelte` — OCR state + handlers + context threading + clear

---

## Task 1: Create OcrDropZone Component

Extract the OCR drop zone, file chips, preview textarea, handlers, and CSS from `ContextPanel.svelte` into a new self-contained component.

**Files:**
- Create: `src/lib/components/OcrDropZone.svelte`

- [ ] **Step 1: Create the component**

Create `src/lib/components/OcrDropZone.svelte` with all the OCR-related code extracted from ContextPanel. This includes the `OcrFileStatus` type, the props interface, the `isDragging` state, `handleBrowse`, the Tauri `onDragDropEvent` `$effect`, the full OCR markup, and all OCR-related CSS.

```svelte
<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';

  /** Status of a single OCR-processed document chip. */
  interface OcrFileStatus {
    id: string;
    filename: string;
    status: 'done' | 'loading' | 'error';
    pageCount: number;
    text?: string;
    path?: string;
  }

  type Props = {
    ocrFiles: OcrFileStatus[];
    ocrText: string;
    ocrLoading: boolean;
    onOcrFilesSelected: (paths: string[]) => void;
    onOcrTextChange: (text: string) => void;
    onRemoveOcrFile: (id: string) => void;
  };

  let {
    ocrFiles = [],
    ocrText = '',
    ocrLoading = false,
    onOcrFilesSelected = () => {},
    onOcrTextChange = () => {},
    onRemoveOcrFile = () => {},
  }: Props = $props();

  let isDragging = $state(false);

  async function handleBrowse() {
    const selected = await open({
      multiple: true,
      filters: [
        {
          name: 'Documents',
          extensions: ['pdf', 'png', 'jpg', 'jpeg', 'bmp', 'webp', 'txt', 'md', 'csv'],
        },
      ],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    onOcrFilesSelected(paths);
  }

  // Tauri v2 intercepts OS file drops at the window layer — HTML5 dragover/drop
  // events never receive real file paths. We use Tauri's native onDragDropEvent
  // instead, which delivers { paths } payloads directly.
  $effect(() => {
    let unlisten: (() => void) | undefined;
    let cleanup = false;

    (async () => {
      const { getCurrentWebview } = await import('@tauri-apps/api/webview');
      unlisten = await getCurrentWebview().onDragDropEvent((event) => {
        if (event.payload.type === 'over' || event.payload.type === 'enter') {
          isDragging = true;
        } else if (event.payload.type === 'leave') {
          isDragging = false;
        } else if (event.payload.type === 'drop') {
          isDragging = false;
          const paths = event.payload.paths;
          if (paths && paths.length > 0) {
            onOcrFilesSelected(paths);
          }
        }
      });
      if (cleanup) unlisten?.();
    })();

    return () => {
      cleanup = true;
      unlisten?.();
    };
  });
</script>

<div class="ocr-section">
  <div
    class="dropzone"
    class:dragging={isDragging}
    onclick={handleBrowse}
    role="button"
    tabindex="0"
    onkeydown={(e) => { if (e.key === 'Enter') handleBrowse(); }}
  >
    <span class="dropzone-icon">📎</span>
    <span class="dropzone-text">Drop documents here</span>
    <span class="dropzone-hint">or click to browse — PDF, PNG, JPG, TXT — max 100 MB per file</span>
  </div>

  {#if ocrFiles.length > 0}
    <div class="ocr-files">
      {#each ocrFiles as file (file.id)}
        <span class="ocr-file-chip" class:chip-error={file.status === 'error'}>
          <span class="chip-name">{file.filename}</span>
          {#if file.status === 'done'}
            <span class="chip-status">✓ {file.pageCount}p</span>
          {:else if file.status === 'loading'}
            <span class="chip-status">⏳</span>
          {:else}
            <span class="chip-status">⚠</span>
          {/if}
          <button
            class="chip-remove"
            onclick={(e) => { e.stopPropagation(); onRemoveOcrFile(file.id); }}
            aria-label="Remove file"
          >×</button>
        </span>
      {/each}
    </div>
  {/if}

  {#if ocrLoading}
    <div class="ocr-status">Extracting text…</div>
  {/if}

  {#if ocrText || ocrLoading}
    <details class="ocr-preview-details">
      <summary>Preview extracted text (editable)</summary>
      <textarea
        class="ocr-preview"
        placeholder="Extracted text will appear here…"
        value={ocrText}
        oninput={(e) => onOcrTextChange((e.currentTarget as HTMLTextAreaElement).value)}
        rows="6"
      ></textarea>
    </details>
  {/if}
</div>

<style>
  .ocr-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 4px;
    border-top: 1px solid var(--border);
    margin-top: 4px;
  }

  .dropzone {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
    padding: 20px;
    border: 2px dashed var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
    transition: border-color 0.15s ease, background-color 0.15s ease;
    text-align: center;
  }

  .dropzone:hover,
  .dropzone.dragging {
    border-color: var(--accent);
    background-color: var(--bg-hover);
  }

  .dropzone.dragging {
    border-style: solid;
  }

  .dropzone-icon {
    font-size: 24px;
  }

  .dropzone-text {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
  }

  .dropzone-hint {
    font-size: 11px;
    color: var(--text-muted);
  }

  .ocr-files {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .ocr-file-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 8px;
    border-radius: var(--radius-sm);
    background-color: var(--bg-hover);
    font-size: 12px;
    color: var(--text-secondary);
  }

  .ocr-file-chip.chip-error {
    background-color: rgba(239, 68, 68, 0.1);
    color: var(--danger, #ef4444);
  }

  .chip-remove {
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 0 2px;
    opacity: 0.6;
  }

  .chip-remove:hover {
    opacity: 1;
  }

  .ocr-status {
    font-size: 12px;
    color: var(--text-muted);
    font-style: italic;
  }

  .ocr-preview-details summary {
    cursor: pointer;
    font-size: 12px;
    color: var(--text-muted);
    margin-bottom: 6px;
  }

  .ocr-preview {
    width: 100%;
    font-size: 13px;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background-color: var(--bg-primary);
    color: var(--text-primary);
    resize: vertical;
    font-family: inherit;
  }
</style>
```

- [ ] **Step 2: Verify type-check**

Run: `npm run check 2>&1 | tail -5`
Expected: PASS (the component is self-contained, not yet referenced anywhere)

- [ ] **Step 3: Commit**

```bash
git add src/lib/components/OcrDropZone.svelte
git commit -m "feat: extract OcrDropZone shared component from ContextPanel"
```

---

## Task 2: Refactor ContextPanel to Use OcrDropZone

Replace the inline OCR UI in `ContextPanel.svelte` with the new `<OcrDropZone>` component. Remove the now-duplicated handlers, CSS, and `OcrFileStatus` type from ContextPanel.

**Files:**
- Modify: `src/lib/components/ContextPanel.svelte`

- [ ] **Step 1: Add the import**

At the top of the `<script>` block in `src/lib/components/ContextPanel.svelte`, add the import after the existing imports (after line 4, the `ConditionChips` import):

```typescript
  import OcrDropZone from './OcrDropZone.svelte';
```

- [ ] **Step 2: Remove inline OCR code from the script**

Remove these from the `<script>` block in `ContextPanel.svelte`:
1. The `OcrFileStatus` interface (lines 6-14) — it now lives in `OcrDropZone.svelte`
2. The `isDragging` state (line 60)
3. The `handleBrowse` function (lines 62-75)
4. The Tauri `onDragDropEvent` `$effect` (lines 77-106)

**IMPORTANT:** Keep the `OcrFileStatus` type import reference. Since `OcrDropZone` defines the type internally, ContextPanel needs a type-only reference. Add this import to keep prop types working:

```typescript
  import type { OcrFileStatus } from './OcrDropZone.svelte';
```

Wait — `OcrFileStatus` is an interface inside the component, not exported. The Props interface in ContextPanel references `OcrFileStatus[]`. Since the type is needed for the props, either:
- (a) Export it from OcrDropZone, or
- (b) Inline the type in ContextPanel's Props as a structural type.

The simplest approach: since `OcrDropZone` doesn't export the type, and ContextPanel just passes props through, change ContextPanel's Props to use the structural shape directly:

In ContextPanel's Props interface, change `ocrFiles: OcrFileStatus[]` to:
```typescript
  ocrFiles: Array<{ id: string; filename: string; status: 'done' | 'loading' | 'error'; pageCount: number; text?: string; path?: string }>;
```

Actually, the cleanest approach: export the type from OcrDropZone. Add this export before the interface in OcrDropZone.svelte:

```typescript
  /** Status of a single OCR-processed document chip. */
  export interface OcrFileStatus {
```

Then ContextPanel imports it:
```typescript
  import OcrDropZone, { type OcrFileStatus } from './OcrDropZone.svelte';
```

Do this — export the interface from OcrDropZone and import it in ContextPanel.

- [ ] **Step 3: Replace inline OCR markup with OcrDropZone**

In the template section of `ContextPanel.svelte`, find the OCR section (the `<!-- OCR Drop Zone -->` comment and everything in `<div class="ocr-section">` through its closing `</div>`). Replace the entire block with:

```svelte
      <!-- OCR Drop Zone — shared component -->
      <OcrDropZone
        {ocrFiles}
        {ocrText}
        {ocrLoading}
        {onOcrFilesSelected}
        {onOcrTextChange}
        {onRemoveOcrFile}
      />
```

- [ ] **Step 4: Remove OCR CSS from ContextPanel**

Remove all OCR-related CSS rules from ContextPanel's `<style>` block: `.ocr-section`, `.dropzone` and all its variants, `.ocr-files`, `.ocr-file-chip`, `.chip-error`, `.chip-name`, `.chip-status`, `.chip-remove`, `.ocr-status`, `.ocr-preview-details`, `.ocr-preview`. These now live in `OcrDropZone.svelte`.

- [ ] **Step 5: Verify type-check**

Run: `npm run check 2>&1 | tail -5`
Expected: PASS — ContextPanel delegates OCR rendering to OcrDropZone.

- [ ] **Step 6: Run frontend tests**

Run: `npx vitest run 2>&1 | tail -5`
Expected: All pass — ContextPanel's OCR props are unchanged (still passed through from GenerateTab).

- [ ] **Step 7: Commit**

```bash
git add src/lib/components/ContextPanel.svelte src/lib/components/OcrDropZone.svelte
git commit -m "refactor: ContextPanel delegates OCR rendering to OcrDropZone"
```

---

## Task 3: Add OCR Props to PatientContextSidebar

Add OCR props to the Record tab's sidebar and render `<OcrDropZone>` below the tab content.

**Files:**
- Modify: `src/lib/pages/record/PatientContextSidebar.svelte`

- [ ] **Step 1: Add the import**

At the top of the `<script>` block, after the existing imports:

```typescript
  import OcrDropZone, { type OcrFileStatus } from '../../components/OcrDropZone.svelte';
```

- [ ] **Step 2: Add OCR props to the interface**

In the `type Props` declaration (lines 7-15), add:

```typescript
    ocrFiles: OcrFileStatus[];
    ocrText: string;
    ocrLoading: boolean;
    onOcrFilesSelected: (paths: string[]) => void;
    onOcrTextChange: (text: string) => void;
    onRemoveOcrFile: (id: string) => void;
```

- [ ] **Step 3: Destructure the new props**

In the `$props()` destructure (lines 16-24), add:

```typescript
    ocrFiles = [],
    ocrText = '',
    ocrLoading = false,
    onOcrFilesSelected = () => {},
    onOcrTextChange = () => {},
    onRemoveOcrFile = () => {},
```

- [ ] **Step 4: Render OcrDropZone in the template**

Inside the `<aside>` element, after the `<div class="tab-content">...</div>` closing tag (after line 101) and before the closing `</aside>` (line 102), add:

```svelte
    <div class="sidebar-ocr">
      <OcrDropZone
        {ocrFiles}
        {ocrText}
        {ocrLoading}
        {onOcrFilesSelected}
        {onOcrTextChange}
        {onRemoveOcrFile}
      />
    </div>
```

Add a minimal CSS rule for the container:

```css
  .sidebar-ocr {
    padding: 0 12px 12px;
    border-top: 1px solid var(--border);
    margin-top: 8px;
    overflow-y: auto;
    flex: 0 1 auto;
    max-height: 40%;
  }
```

Add this to the existing `<style>` block.

- [ ] **Step 5: Verify type-check**

Run: `npm run check 2>&1 | tail -5`
Expected: PASS (RecordTab hasn't been wired yet, but the sidebar accepts the props with defaults)

- [ ] **Step 6: Commit**

```bash
git add src/lib/pages/record/PatientContextSidebar.svelte
git commit -m "feat: add OcrDropZone to PatientContextSidebar"
```

---

## Task 4: Wire OCR State and Context Threading in RecordTab

Add OCR state to `RecordTab.svelte`, create the handlers, clear them on new recording, and thread combined context into every `pipeline.launch`/`pipeline.retry` call site.

**Files:**
- Modify: `src/lib/pages/RecordTab.svelte`

- [ ] **Step 1: Add import and state**

After line 24 (the `generateSoap` import), add:

```typescript
  import { ocrDocuments } from '../api/ocr';
  import { OfflineCancelled } from '../api/invokeWithOfflineHandling';
  import type { OcrFileStatus } from '../components/OcrDropZone.svelte';
```

After the existing context state (after line 35 `let conditionsText = $state('');`), add OCR state:

```typescript
  // OCR state: mirrors the GenerateTab pattern. Transient — cleared on new recording.
  let ocrFiles = $state<OcrFileStatus[]>([]);
  let ocrLoading = $state(false);
  let ocrTextOverride = $state<string | null>(null);
  let ocrText = $derived(
    ocrFiles
      .filter((f) => f.status === 'done' && f.text)
      .map((f) => `--- ${f.filename} ---\n${f.text}`)
      .join('\n\n'),
  );
  let ocrTextDisplay = $derived(ocrTextOverride ?? ocrText);
```

- [ ] **Step 2: Add OCR handlers**

Add these handler functions in the `<script>` block (after `clearAllContextFields`):

```typescript
  async function handleOcrFilesSelected(paths: string[]) {
    if (paths.length === 0) return;
    ocrLoading = true;
    ocrTextOverride = null;
    const chipIds: string[] = [];
    const pendingChips = paths.map((p) => {
      const id = crypto.randomUUID();
      chipIds.push(id);
      const filename = p.split(/[/\\]/).pop() || p;
      return { id, filename, path: p, status: 'loading' as const, pageCount: 0, text: '' };
    });
    ocrFiles = [...ocrFiles, ...pendingChips];
    const idSet = new Set(chipIds);

    try {
      const results = await ocrDocuments(paths);
      ocrFiles = ocrFiles.map((f) => {
        if (!idSet.has(f.id)) return f;
        const result = results.find((r) => r.filename === f.filename);
        if (result) {
          return { ...f, status: 'done' as const, pageCount: result.page_count, text: result.text };
        }
        return { ...f, status: 'error' as const };
      });
    } catch (e) {
      ocrFiles = ocrFiles.map((f) =>
        idSet.has(f.id) ? { ...f, status: 'error' as const } : f,
      );
      if (!(e instanceof OfflineCancelled)) {
        console.error('OCR failed:', e);
      }
    } finally {
      ocrLoading = ocrFiles.some((f) => f.status === 'loading');
    }
  }

  function handleOcrTextChange(text: string) {
    ocrTextOverride = text;
  }

  function handleRemoveOcrFile(id: string) {
    ocrFiles = ocrFiles.filter((f) => f.id !== id);
    ocrTextOverride = null;
  }
```

- [ ] **Step 3: Clear OCR state in clearAllContextFields**

In `clearAllContextFields()` (around line 127-135), add before the closing `}`:

```typescript
    ocrFiles = [];
    ocrTextOverride = null;
```

- [ ] **Step 4: Create a combined-context helper**

Add a helper function to avoid duplicating the combination logic across 3 call sites:

```typescript
  /** Combine notes + OCR text into the pipeline context string. */
  function buildPipelineContext(): string | undefined {
    return [contextText.trim(), ocrTextDisplay.trim()].filter(Boolean).join('\n\n') || undefined;
  }
```

- [ ] **Step 5: Replace contextText at all pipeline call sites**

Replace every `contextText || undefined` in pipeline calls with `buildPipelineContext()`:

**Call site 1 — `maybeLaunchPipeline` (~line 177):**
Change:
```typescript
    pipeline.launch(recordingId, contextText || undefined, undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
```
To:
```typescript
    pipeline.launch(recordingId, buildPipelineContext(), undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
```

**Call site 2 — `confirmSilentProcess` (~line 199):**
Change:
```typescript
      pipeline.launch(id, contextText || undefined, undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
```
To:
```typescript
      pipeline.launch(id, buildPipelineContext(), undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
```

**Call site 3 — `handleRetry` (~line 232):**
Change:
```typescript
    pipeline.retry(pipelineRecordingId, contextText || undefined, undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
```
To:
```typescript
    pipeline.retry(pipelineRecordingId, buildPipelineContext(), undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
```

- [ ] **Step 6: Pass OCR props to PatientContextSidebar**

In the markup, find the `<PatientContextSidebar>` usage (~lines 399-407). Add the OCR props:

```svelte
    <PatientContextSidebar
      bind:contextText
      bind:medicationsText
      bind:allergiesText
      bind:conditionsText
      open={sidebarOpen}
      width={sidebarWidth}
      onToggle={toggleSidebar}
      {ocrFiles}
      ocrText={ocrTextDisplay}
      {ocrLoading}
      onOcrFilesSelected={handleOcrFilesSelected}
      onOcrTextChange={handleOcrTextChange}
      onRemoveOcrFile={handleRemoveOcrFile}
    />
```

- [ ] **Step 7: Verify type-check**

Run: `npm run check 2>&1 | tail -5`
Expected: PASS

- [ ] **Step 8: Run all frontend tests**

Run: `npx vitest run 2>&1 | tail -5`
Expected: All pass.

- [ ] **Step 9: Commit**

```bash
git add src/lib/pages/RecordTab.svelte
git commit -m "feat: wire OCR state + context threading into Record tab pipeline"
```

---

## Task 5: Final Verification

- [ ] **Step 1: Type-check**

Run: `npm run check 2>&1 | tail -5`
Expected: 0 errors, 0 warnings.

- [ ] **Step 2: ESLint**

Run: `npm run lint 2>&1 | tail -5`
Expected: 0 errors.

- [ ] **Step 3: Frontend tests**

Run: `npx vitest run 2>&1 | tail -5`
Expected: All pass.

- [ ] **Step 4: Manual verification (if possible)**

Launch `npm run tauri dev`, go to the Record tab, expand the Patient Context sidebar, verify the drop zone is visible below the tab content. Drop a text file, verify it appears as a chip with extracted text in the preview. Start a recording, verify the pipeline context includes the OCR text.

- [ ] **Step 5: Commit any final fixes**

```bash
git add -A
git commit -m "feat: OCR document support in Record tab — complete"
```

---

## Self-Review Checklist

**Spec coverage:**
- ✅ Extract OcrDropZone shared component: Task 1
- ✅ Refactor ContextPanel to use it: Task 2
- ✅ Add OcrDropZone to PatientContextSidebar: Task 3
- ✅ Wire OCR state + context threading in RecordTab: Task 4
- ✅ Verification: Task 5

**Type consistency:**
- `OcrFileStatus` is exported from OcrDropZone and imported by both ContextPanel and RecordTab
- `buildPipelineContext()` returns `string | undefined` — matches `pipeline.launch(recordingId: string, context?: string, ...)`
- OCR handler signatures match between RecordTab and the OcrDropZone Props

**No backend changes:** The pipeline's `context` parameter already accepts any string and flows through `process_recording` → `generate_soap` → `build_user_prompt`. No Rust changes needed.
