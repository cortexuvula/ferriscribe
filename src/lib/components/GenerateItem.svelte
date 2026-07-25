<script lang="ts">
  import type { ValidatedIcdCode } from '../icd';
  import IcdChip from './IcdChip.svelte';

  interface Props {
    title: string;
    description: string;
    icon?: string;
    useWhen?: string;
    generating: boolean;
    anyGenerating: boolean;
    done: boolean;
    copyStatus: 'idle' | 'copying' | 'copied' | undefined;
    icdCodes?: ValidatedIcdCode[];
    generatedText?: string | null;
    progressText?: string | null;
    failed?: boolean;
    onGenerate: () => void;
    onCopy: () => void;
    onSpeedRead?: () => void;
  }

  const {
    title,
    description,
    icon,
    useWhen,
    generating,
    anyGenerating,
    done,
    copyStatus,
    icdCodes,
    generatedText = null,
    progressText = null,
    failed = false,
    onGenerate,
    onCopy,
    onSpeedRead,
  }: Props = $props();
</script>

<div class="generate-item" class:failed>
  {#if icon}<span class="item-icon" aria-hidden="true">{icon}</span>{/if}
  <div class="item-info">
    <div class="item-title">{title}</div>
    <div class="item-desc">{description}</div>
    {#if useWhen}<div class="item-use-when">Use when: {useWhen}</div>{/if}
  </div>
  <div class="item-action">
    {#if generating}
      <button class="btn-generate" disabled>
        <span class="spinner"></span> Generating...
      </button>
      {#if progressText}
        <span class="progress-phase" role="status" aria-live="polite">{progressText}</span>
      {/if}
    {:else if done}
      <div class="done-group">
        <span class="done-badge">Done</span>
        <button
          class="btn-copy"
          class:copied={copyStatus === 'copied'}
          onclick={onCopy}
          disabled={copyStatus === 'copying' || copyStatus === 'copied'}
        >
          {copyStatus === 'copying' ? 'Copying…' : copyStatus === 'copied' ? 'Copied!' : 'Copy'}
        </button>
        {#if onSpeedRead}
          <button
            class="btn-copy"
            onclick={onSpeedRead}
            title="Speed Read (Cmd/Ctrl+Shift+R)"
          >
            Speed Read
          </button>
        {/if}
        <button
          class="btn-regenerate"
          onclick={onGenerate}
          disabled={anyGenerating}
        >
          Regenerate
        </button>
      </div>
      {#if icdCodes && icdCodes.length > 0}
        <div class="icd-codes">
          <span class="icd-label">ICD Codes:</span>
          {#each icdCodes as code (code.raw)}
            <IcdChip code={code.raw} valid={code.valid} />
          {/each}
        </div>
      {/if}
      {#if generatedText}
        <details class="generated-preview">
          <summary>Preview</summary>
          <pre class="preview-text">{generatedText}</pre>
        </details>
      {/if}
    {:else}
      <button
        class="btn-generate"
        onclick={onGenerate}
        disabled={anyGenerating}
      >
        Generate
      </button>
    {/if}
  </div>
</div>

<style>
  .generate-item {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 16px;
    background-color: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
  }

  .item-icon {
    font-size: 22px;
    line-height: 1;
    flex-shrink: 0;
    width: 32px;
    text-align: center;
  }

  .item-info {
    flex: 1;
    min-width: 0;
  }

  .item-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: 2px;
  }

  .item-desc {
    font-size: 12px;
    color: var(--text-muted);
  }

  .item-use-when {
    font-size: 11px;
    color: var(--accent);
    margin-top: 3px;
    opacity: 0.85;
  }

  .item-action {
    flex-shrink: 1;
    min-width: 0;
  }

  .btn-generate {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    background-color: var(--accent);
    color: white;
    border-radius: var(--radius-sm);
    font-size: 13px;
    font-weight: 500;
    transition: background-color 0.15s ease;
  }

  .btn-generate:hover:not(:disabled) {
    background-color: var(--accent-hover);
  }

  .btn-generate:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .spinner {
    display: inline-block;
    width: 12px;
    height: 12px;
    border: 2px solid rgba(255, 255, 255, 0.3);
    border-top-color: white;
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .done-group {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .done-badge {
    display: inline-flex;
    align-items: center;
    padding: 6px 12px;
    border-radius: var(--radius-sm);
    font-size: 12px;
    font-weight: 500;
    background-color: var(--accent-light);
    color: var(--success);
    border: 1px solid var(--success);
  }

  .btn-regenerate {
    padding: 6px 12px;
    font-size: 12px;
    font-weight: 500;
    color: var(--accent);
    background-color: color-mix(in srgb, var(--accent) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease, border-color 0.15s ease;
  }

  .btn-regenerate:hover:not(:disabled) {
    background-color: color-mix(in srgb, var(--accent) 20%, transparent);
    border-color: var(--accent);
  }

  .btn-regenerate:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-copy {
    padding: 6px 12px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease, border-color 0.15s ease;
  }

  .btn-copy:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-copy.copied {
    color: var(--success, #22c55e);
    border-color: var(--success, #22c55e);
    background-color: color-mix(in srgb, var(--success, #22c55e) 10%, transparent);
  }

  .icd-codes {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
    flex-wrap: wrap;
  }

  .icd-label {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .generated-preview {
    margin-top: 8px;
  }

  .generated-preview summary {
    cursor: pointer;
    font-size: 12px;
    color: var(--text-muted);
    margin-bottom: 6px;
  }

  .preview-text {
    max-height: 300px;
    overflow-y: auto;
    white-space: pre-wrap;
    word-break: break-word;
    font-size: 13px;
    line-height: 1.5;
    padding: 12px;
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-family: inherit;
    margin: 0;
  }

  .progress-phase {
    font-size: 11px;
    color: var(--text-muted);
    font-style: italic;
    margin-top: 4px;
  }

  .generate-item.failed {
    border-left: 3px solid var(--danger, #ef4444);
    padding-left: 8px;
  }
</style>
