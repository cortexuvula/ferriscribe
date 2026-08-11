<script lang="ts">
  import OcrDropZone from '../components/OcrDropZone.svelte';
  import { letterWriter } from '../stores/letterWriter.svelte';

  // View-layer option lists (constants, not state).
  const letterTypes = ['Referral', 'Cover', 'Thank-you', 'Response', 'Summary', 'Other'];
  const tones = ['Formal', 'Warm', 'Clinical', 'Concise'];
</script>

<div class="letter-writer-tab">
  <div class="lw-content">
    <div class="lw-header">
      <div>
        <h2>Letter Writer</h2>
        <p class="hint">OCR a document, then draft a letter from it.</p>
      </div>
      <button
        class="btn-secondary"
        onclick={() => letterWriter.handleClearAll()}
        disabled={letterWriter.generating}
      >
        Clear
      </button>
    </div>

    <!-- Step 1: source document -->
    <section class="lw-section">
      <h3>1. Source document</h3>
      <OcrDropZone
        ocrFiles={letterWriter.ocr.ocrFiles}
        ocrText={letterWriter.ocr.ocrTextDisplay}
        ocrLoading={letterWriter.ocr.ocrLoading}
        onOcrFilesSelected={letterWriter.ocr.handleOcrFilesSelected}
        onOcrTextChange={letterWriter.ocr.handleOcrTextChange}
        onRemoveOcrFile={letterWriter.ocr.handleRemoveOcrFile}
      />
    </section>

    <!-- Step 2: letter details + instructions -->
    <section class="lw-section">
      <h3>2. Letter details</h3>
      <div class="lw-fields">
        <label class="field">
          <span class="field-label">To</span>
          <input
            type="text"
            placeholder="e.g. Dr. Smith, Cardiology"
            bind:value={letterWriter.recipient}
            disabled={letterWriter.generating}
          />
        </label>

        <label class="field">
          <span class="field-label">Type</span>
          <select bind:value={letterWriter.letterType} disabled={letterWriter.generating}>
            <option value="">(auto)</option>
            {#each letterTypes as t (t)}<option value={t}>{t}</option>{/each}
          </select>
        </label>

        <label class="field">
          <span class="field-label">Tone</span>
          <select bind:value={letterWriter.tone} disabled={letterWriter.generating}>
            {#each tones as tn (tn)}<option value={tn}>{tn}</option>{/each}
          </select>
        </label>

        <label class="field">
          <span class="field-label">RE</span>
          <input
            type="text"
            placeholder="subject line (optional)"
            bind:value={letterWriter.reLine}
            disabled={letterWriter.generating}
          />
        </label>
      </div>

      <label class="field field-full">
        <span class="field-label">Additional instructions</span>
        <textarea
          class="instructions"
          placeholder="e.g. Keep it brief; mention the abnormal ECG; request an urgent appointment."
          rows="3"
          bind:value={letterWriter.userInstructions}
          disabled={letterWriter.generating}
        ></textarea>
      </label>
    </section>

    <!-- Generate -->
    <div class="lw-actions">
      <button
        class="btn-primary"
        onclick={() => letterWriter.handleGenerate()}
        disabled={!letterWriter.canGenerate}
      >
        {letterWriter.generating ? 'Generating…' : '✉ Generate letter'}
      </button>
    </div>

    {#if letterWriter.error}
      <div class="lw-error" role="alert">
        <span>{letterWriter.error}</span>
        <button class="error-dismiss" onclick={() => (letterWriter.error = null)} aria-label="Dismiss"
          >×</button
        >
      </div>
    {/if}

    {#if letterWriter.output}
      <section class="lw-section">
        <div class="output-header">
          <h3>3. Generated letter</h3>
          <button
            class="btn-secondary btn-copy"
            onclick={() => letterWriter.handleCopy()}
            disabled={letterWriter.copyStatus !== 'idle'}
          >
            {letterWriter.copyStatus === 'copying'
              ? 'Copying…'
              : letterWriter.copyStatus === 'copied'
                ? 'Copied ✓'
                : 'Copy'}
          </button>
        </div>
        <textarea class="output" bind:value={letterWriter.output} rows="16"></textarea>
      </section>
    {/if}
  </div>
</div>

<style>
  .letter-writer-tab {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .lw-content {
    flex: 1;
    overflow-y: auto;
    padding: 24px;
    max-width: 880px;
    width: 100%;
    margin: 0 auto;
  }

  .lw-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    margin-bottom: 24px;
  }

  .lw-header h2 {
    font-size: 18px;
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: 4px;
  }

  .hint {
    font-size: 13px;
    color: var(--text-muted);
  }

  .lw-section {
    margin-bottom: 24px;
  }

  .lw-section h3 {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-secondary);
    margin-bottom: 10px;
  }

  .lw-fields {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    margin-bottom: 12px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .field-full {
    width: 100%;
  }

  .field-label {
    font-size: 12px;
    color: var(--text-muted);
  }

  input,
  select,
  textarea {
    width: 100%;
    font-size: 13px;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background-color: var(--bg-primary);
    color: var(--text-primary);
    font-family: inherit;
  }

  textarea {
    resize: vertical;
  }

  .instructions {
    font-size: 13px;
  }

  input:focus,
  select:focus,
  textarea:focus {
    outline: none;
    border-color: var(--accent);
  }

  input:disabled,
  select:disabled,
  textarea:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .lw-actions {
    display: flex;
    justify-content: center;
    margin: 8px 0 20px;
  }

  .btn-primary {
    padding: 10px 24px;
    font-size: 14px;
    font-weight: 500;
    color: white;
    background-color: var(--accent);
    border: none;
    border-radius: var(--radius-md);
    cursor: pointer;
    transition: opacity 0.15s ease;
  }

  .btn-primary:hover:not(:disabled) {
    opacity: 0.9;
  }

  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-secondary {
    padding: 6px 14px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .btn-secondary:hover:not(:disabled) {
    background-color: var(--bg-primary);
  }

  .btn-secondary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .output-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 10px;
  }

  .output {
    width: 100%;
    font-size: 13px;
    line-height: 1.5;
  }

  .lw-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 14px;
    margin-bottom: 20px;
    background-color: rgba(239, 68, 68, 0.1);
    border: 1px solid var(--danger, #ef4444);
    border-radius: var(--radius-sm);
    color: var(--danger, #ef4444);
    font-size: 13px;
  }

  .error-dismiss {
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
    font-size: 18px;
    line-height: 1;
    padding: 0;
    opacity: 0.7;
  }

  .error-dismiss:hover {
    opacity: 1;
  }

  @media (max-width: 640px) {
    .lw-fields {
      grid-template-columns: 1fr;
    }
  }
</style>
