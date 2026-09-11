<script lang="ts">
  import type { ValidatedIcdCode } from '../icd';
  import IcdCodeList from './IcdCodeList.svelte';

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
    /** Heading for the billing-code list (mode-aware, e.g. "Billing codes (ICD-9)"). */
    icdLabel?: string;
    generatedText?: string | null;
    progressText?: string | null;
    failed?: boolean;
    /** Only an authoritative backend verdict may mark retained output current. */
    freshness?: 'current' | 'stale' | 'unknown' | 'checking';
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
    icdLabel = 'Billing codes (ICD-9)',
    generatedText = null,
    progressText = null,
    failed = false,
    freshness = 'unknown',
    onGenerate,
    onCopy,
    onSpeedRead,
  }: Props = $props();
</script>

<article class="generate-item" class:failed aria-label={title}>
  {#if icon}<span class="item-icon" aria-hidden="true">{icon}</span>{/if}
  <div class="item-info">
    <h4 class="item-title">{title}</h4>
    <div class="item-desc">{description}</div>
    {#if useWhen}<div class="item-use-when">Use when: {useWhen}</div>{/if}
    {#if failed}<span class="failed-badge">Failed</span>{/if}
    {#if done}
      <span class="output-badge" class:stale={freshness === 'stale'} class:current={freshness === 'current'}>
        {freshness === 'stale' ? 'Stale' : freshness === 'current' ? 'Current' : freshness === 'checking' ? 'Checking freshness…' : 'Freshness unavailable'}
      </span>
      {#if freshness === 'stale'}
        <p class="stale-hint">Inputs changed. Regenerate before using this output.</p>
      {:else if freshness === 'unknown'}
        <p class="output-hint">Input history is unavailable. Review or regenerate before use.</p>
      {/if}
    {/if}
  </div>
  <div class="item-action">
    {#if generating}
      <button class="btn-generate" type="button" disabled><span class="spinner" aria-hidden="true"></span> Generating…</button>
      <span class="progress-phase" role="status" aria-live="polite">{progressText ?? 'Preparing generation…'}</span>
    {:else if !done}
      <button class="btn-generate" type="button" onclick={onGenerate} disabled={anyGenerating}>{failed ? 'Retry' : 'Generate'}</button>
    {/if}
    {#if done}
      <div class="done-group">
        <button class="btn-copy" type="button" class:copied={copyStatus === 'copied'} onclick={onCopy} disabled={copyStatus === 'copying' || copyStatus === 'copied'}>
          {copyStatus === 'copying' ? 'Copying…' : copyStatus === 'copied' ? 'Copied!' : 'Copy'}
        </button>
        {#if onSpeedRead}<button class="btn-copy" type="button" onclick={onSpeedRead} title="Speed Read (Cmd/Ctrl+Shift+R)">Speed Read</button>{/if}
        <button class="btn-regenerate" type="button" onclick={onGenerate} disabled={anyGenerating}>Regenerate</button>
      </div>
    {/if}
  </div>
  {#if done && icdCodes && icdCodes.length > 0}
    <div class="icd-list-row"><IcdCodeList codes={icdCodes} label={icdLabel} /></div>
  {/if}
  {#if done && generatedText}
    <details class="generated-preview"><summary>Preview</summary><pre class="preview-text">{generatedText}</pre></details>
  {/if}
</article>
<style>
  .generate-item {
    display: flex;
    flex-wrap: wrap;
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
    min-width: min(220px, 100%);
    overflow-wrap: anywhere;
  }

  .item-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0 0 2px;
  }

  .item-desc {
    font-size: 12px;
    color: var(--text-secondary);
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
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .output-badge, .failed-badge {
    display: inline-block; margin-top: 8px; padding: 4px 8px;
    font-size: 12px; font-weight: 600; border: 1px solid var(--border);
    border-radius: var(--radius-sm); color: var(--text-secondary); background: var(--bg-secondary);
  }
  .output-badge.stale {
    color: var(--text-primary);
    background: color-mix(in srgb, var(--warning) 18%, var(--bg-card));
    border-color: var(--warning);
  }
  .output-badge.current { border-color: var(--success); color: var(--text-primary); }
  .failed-badge { border-color: var(--danger); color: var(--danger); }
  .stale-hint, .output-hint { font-size: 12px; color: var(--text-secondary); margin: 6px 0 0; }

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

  .icd-list-row {
    width: 100%;
    margin-top: 8px;
  }

  .generated-preview {
    width: 100%;
    margin-top: 8px;
  }

  .generated-preview summary {
    cursor: pointer;
    font-size: 12px;
    color: var(--text-secondary);
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
    color: var(--text-secondary);
    font-style: italic;
    margin-top: 4px;
  }

  .generate-item.failed {
    border-left: 3px solid var(--danger, #ef4444);
    padding-left: 8px;
  }
  button, summary { min-height: 44px; box-sizing: border-box; }
  summary { display: flex; align-items: center; }
  summary::before { content: '▸'; margin-right: 8px; }
  details[open] summary::before { content: '▾'; }
  button:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
  .progress-phase { display: block; margin-bottom: 8px; }
  @media (prefers-reduced-motion: reduce) { .spinner { animation: none; } }
</style>
