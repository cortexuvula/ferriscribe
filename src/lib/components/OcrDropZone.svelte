<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';

  /** Status of a single OCR-processed document chip. */
  export interface OcrFileStatus {
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
          extensions: ['pdf', 'png', 'jpg', 'jpeg', 'bmp', 'webp', 'tiff', 'tif', 'heic', 'heif', 'txt', 'md', 'csv', 'docx', 'xlsx'],
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
    <span class="dropzone-hint">or click to browse — PDF, PNG, JPG, HEIC, DOCX, XLSX, TIFF, TXT — max 100 MB per file</span>
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
