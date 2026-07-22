# Document OCR for Generation Context

**Date:** 2026-07-22
**Status:** Draft
**Branch:** `feature/document-ocr`

## Problem

Users often have lab results, referral letters, or other clinical documents as PDFs or images that contain information relevant to the letter/referral/SOAP they're generating. Currently there's no way to incorporate external documents into the generation flow — the user must manually retype relevant content into the Notes field.

## Solution

Add a document drop zone in the Context Panel that OCR's dropped files (PDFs, images, text files) using a locally-hosted vision model (e.g. `glm-ocr` via Ollama/LM Studio). The extracted text is shown in an editable preview and prepended as "Supporting Documents" context to every generation type (SOAP, letter, referral, peer discussion).

## Key Decisions

1. **Extract text first, then generate** — OCR runs the vision model to extract text, then the existing text model writes the letter. Keeps generation quality high.
2. **Drop zone in Context Panel** — sits under the Notes textarea so it benefits all generation types, not just one.
3. **Separate OCR model setting** — `ocr_model` in Settings, independent from `ai_model`. The OCR model must be vision-capable.
4. **Text-based PDFs + images for v1** — `pdf-extract` crate for embedded text (pure Rust, no native deps). Image OCR via vision model. Scanned-PDF rasterization deferred to v2 (would require `pdfium-render` + bundled binary).
5. **Drag-and-drop + browse button** — implement DnD with a button fallback, since Tauri's webview DnD can be unreliable (noted in ConditionChips.svelte).

## Architecture

### Layer A: Vision message support

**Files:**
- `crates/core/src/types/ai.rs` — extend `MessageContent`
- `crates/ai-providers/src/openai_compat/client.rs` — serialize multipart content
- `crates/ai-providers/src/openai_compat/wire.rs` — wire types for multipart

`MessageContent` gains a `Parts` variant for the OpenAI vision format:

```rust
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    ToolResult { tool_call_id: String, content: String },
    /// Multipart content for vision models (OpenAI format).
    Parts(Vec<ContentPart>),
}

#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlData },
}

#[derive(Serialize, Deserialize)]
pub struct ImageUrlData {
    pub url: String, // data:image/png;base64,...
}
```

`convert_message` in `client.rs` serializes `Parts` as the array content:
```json
{
  "role": "user",
  "content": [
    {"type": "text", "text": "Extract all text..."},
    {"type": "image_url", "image_url": {"url": "data:image/png;base64,..."}}
  ]
}
```

Both Ollama and LM Studio accept this format when a vision model is loaded.

### Layer B: OCR pipeline

**Files:**
- `crates/processing/src/ocr/mod.rs` — new module
- `crates/processing/src/lib.rs` — declare `pub mod ocr;`
- `crates/processing/Cargo.toml` — add `pdf-extract`, `base64`
- `src-tauri/src/commands/ocr.rs` — new Tauri command module
- `src-tauri/src/lib.rs` — register the command

**OCR module** (`crates/processing/src/ocr/mod.rs`):

```rust
pub struct OcrPageResult {
    pub filename: String,
    pub text: String,
    pub page_count: usize,
}

pub struct OcrRequest {
    pub file_paths: Vec<String>,
    pub ocr_model: String,
    pub provider: Arc<dyn AiProvider>,
}

/// Extract text from documents. Strategy per format:
/// - .txt/.md/.csv: read directly (no model call)
/// - .png/.jpg/.jpeg/.bmp/.webp: send to vision model
/// - .pdf: extract embedded text via pdf-extract; if empty, mark as "scanned"
///         (v2 will rasterize and OCR)
pub async fn extract_text(req: OcrRequest) -> Result<Vec<OcrPageResult>, OcrError>;
```

**Tauri command** (`src-tauri/src/commands/ocr.rs`):

```rust
#[tauri::command]
pub async fn ocr_documents(
    state: State<'_, AppState>,
    app: AppHandle,
    file_paths: Vec<String>,
) -> Result<Vec<OcrPageResult>, AppError> {
    // 1. Load config, get ocr_model (fallback to ai_model if unset)
    // 2. Resolve provider
    // 3. Call processing::ocr::extract_text
    // 4. Emit ocr-progress events per file
    // 5. Return results
}
```

**System prompt for OCR:** "Extract all text from this document image. Output only the extracted text, preserving the document's structure, headings, and table layout. Do not add commentary."

### Layer C: Context threading

**Files:**
- `crates/core/src/types/settings.rs` — add `ocr_model: Option<String>`
- `src/lib/api/generation.ts` — add `context` param to `generateReferral`, `generateLetter`, `generatePeerDiscussion`
- `src-tauri/src/commands/generation/{letter,referral,peer_discussion}.rs` — accept and forward `context`
- `crates/processing/src/document_generator.rs` — prepend context in prompt builders

**Prompt injection:** When context is non-empty, prompt builders prepend:

```
## Supporting Documents
{context}

---

## SOAP Note
{soap_note}
```

This applies to all four prompt builders (`build_soap_prompt`, `build_referral_prompt`, `build_letter_prompt`, `build_synopsis_prompt`).

### Layer D: UI

**Files:**
- `src/lib/components/ContextPanel.svelte` — add drop zone + preview
- `src/lib/pages/GenerateTab.svelte` — state + handlers
- `src/lib/components/settings/Models.svelte` — OCR model selector
- `src/lib/api/ocr.ts` — new API wrapper

**ContextPanel additions:**

New props on `ContextPanel`:
```typescript
ocrFiles: OcrFileStatus[];
ocrText: string;
ocrLoading: boolean;
onOcrFilesDropped: (paths: string[]) => void;
onOcrTextChange: (text: string) => void;
onRemoveOcrFile: (id: string) => void;
```

Drop zone markup (below Notes textarea, inside `.context-body`):

```svelte
<div class="ocr-section">
  <div
    class="dropzone"
    class:dragging={isDragging}
    ondragover={handleDragOver}
    ondragleave={handleDragLeave}
    ondrop={handleDrop}
    onclick={handleBrowse}
    role="button"
    tabindex="0"
  >
    <span class="dropzone-icon">📎</span>
    <span class="dropzone-text">Drop documents here</span>
    <span class="dropzone-hint">or click to browse — PDF, PNG, JPG, TXT</span>
  </div>

  {#if ocrFiles.length > 0}
    <div class="ocr-files">
      {#each ocrFiles as file (file.id)}
        <span class="ocr-file-chip" class:error={file.status === 'error'}>
          {file.filename}
          {#if file.status === 'done'}✓ {file.pageCount}p{/if}
          {#if file.status === 'loading'}⏳{/if}
          {#if file.status === 'error'}⚠{/if}
          <button onclick={() => onRemoveOcrFile(file.id)}>×</button>
        </span>
      {/each}
    </div>
  {/if}

  {#if ocrLoading}
    <div class="ocr-status">Extracting text…</div>
  {/if}

  {#if ocrText || ocrLoading}
    <details>
      <summary>Preview extracted text (editable)</summary>
      <textarea
        class="ocr-preview"
        value={ocrText}
        oninput={(e) => onOcrTextChange(e.currentTarget.value)}
      ></textarea>
    </details>
  {/if}
</div>
```

**Drag-and-drop in Tauri:** The Tauri webview intercepts OS file drops at the window level. We handle `ondrop` events on the drop zone element. The `DataTransfer` from a native file drop in Tauri provides file paths (not just names). If DnD doesn't fire on some platforms, the "Browse" button (`@tauri-apps/plugin-dialog` with file filters) is the fallback.

**State in GenerateTab.svelte:**

```typescript
let ocrFiles = $state<OcrFileStatus[]>([]);
let ocrText = $state('');
let ocrLoading = $state(false);

async function handleOcrFilesDropped(paths: string[]) {
  ocrLoading = true;
  try {
    const results = await ocrDocuments(paths);
    // Update file chips with status
    ocrFiles = results.map(r => ({
      id: crypto.randomUUID(),
      filename: r.filename,
      status: 'done',
      pageCount: r.page_count,
    }));
    // Append extracted text to preview
    const newText = results.map(r => `--- ${r.filename} ---\n${r.text}`).join('\n\n');
    ocrText = ocrText ? `${ocrText}\n\n${newText}` : newText;
  } catch (err) {
    toasts.error(`OCR failed: ${err}`);
  } finally {
    ocrLoading = false;
  }
}
```

**Generation dispatch** — `handleGenerate` passes `ocrText` as context to all types:

```typescript
const ctx = [contextText.trim(), ocrText.trim()].filter(Boolean).join('\n\n') || undefined;
// Pass ctx to ALL generation calls, not just SOAP
```

### Settings: OCR model selector

In `Models.svelte`, below the existing AI model dropdown, add:

```svelte
<div class="setting-row">
  <label for="ocr-model">OCR / Vision Model</label>
  <select id="ocr-model" bind:value={settings.ethicalReviewModel}>
    <option value="">(use generation model)</option>
    {#each models as m}<option value={m.id}>{m.name}</option>{/each}
  </select>
  <p class="hint">Used to extract text from dropped documents. Pick a vision-capable model (e.g. glm-ocr).</p>
</div>
```

`AppConfig` gains:
```rust
pub ocr_model: Option<String>, // None = fall back to ai_model
```

## File Format Support (v1)

| Format | Strategy | Dependency |
|--------|----------|------------|
| `.txt`, `.md`, `.csv` | Read directly | std::fs |
| `.png`, `.jpg`, `.jpeg`, `.bmp`, `.webp` | Base64-encode → vision model | `base64` crate |
| `.pdf` | Extract embedded text via `pdf-extract` | `pdf-extract` crate |
| `.pdf` (scanned) | Show "no text found, scanned PDFs need v2" | deferred |
| Other | Reject with toast | — |

## Error Handling

- **No OCR model configured**: Drop zone shows warning: "Set an OCR model in Settings → Models"
- **Model not loaded/unreachable**: File chip shows ⚠ with error message
- **Unsupported file type**: Toast: "Unsupported file type: .docx"
- **Empty extraction**: File chip shows "No text detected"
- **Large files**: Warn at > 20MB, block at > 100MB

## Privacy & Security

- All OCR runs locally through Ollama/LM Studio — no data leaves the machine
- OCR'd text is PHI — never logged (only filenames and page counts in tracing)
- File paths are transient (never persisted to DB); only extracted text held in memory
- Extracted text is session-scoped — cleared when the user switches recordings or closes the app

## Testing

**Rust unit tests** (`crates/processing/src/ocr/mod.rs`):
- Text file passthrough returns content unchanged
- Image base64 encoding produces valid data URL
- PDF text extraction returns embedded text
- Empty/scanned PDF returns appropriate error
- Unsupported extension returns error
- Multiple files concatenate results with filename headers

**Frontend tests** (`src/lib/components/ContextPanel.ocr.test.ts`):
- Drop zone renders when expanded
- Browse button calls dialog
- File chip shows status correctly
- Remove button clears file
- Preview textarea is editable
- OCR text flows to generation dispatch

**Integration**: OCR'd text appears in the generation prompt (prompt builder prepends "Supporting Documents" section)

## Dependencies Added

- `pdf-extract` (pure Rust PDF text extraction) — `crates/processing/Cargo.toml`
- `base64` (image encoding) — `crates/processing/Cargo.toml`

No native dependencies. No new Tauri plugins.

## Out of Scope (v2)

- Scanned PDF rasterization (requires `pdfium-render` + bundled binary)
- `.docx` / `.xlsx` support
- Persistent OCR document storage
- Multi-page image formats (TIFF)
- Batch OCR progress bar (v1 shows per-file status chips)
