<script lang="ts">
  import { pipeline, type PipelineStage } from '../../stores/pipeline.svelte';

  type CopyStatus = 'idle' | 'copying' | 'copied';

  interface Props {
    copyStatus?: CopyStatus;
    onCancel: () => void;
    onRetry: () => void;
    onCopySoap: () => Promise<void> | void;
    onSpeedRead: () => void;
  }
  let {
    copyStatus = $bindable<CopyStatus>('idle'),
    onCancel,
    onRetry,
    onCopySoap,
    onSpeedRead,
  }: Props = $props();

  function stageLabel(stage: PipelineStage): string {
    switch (stage) {
      case 'transcribing': return 'Transcribing audio...';
      case 'generating_soap': return 'Generating SOAP note...';
      case 'completed': return 'SOAP note ready';
      case 'failed': return 'Pipeline failed';
      default: return '';
    }
  }

  // Live clock for the pipeline-elapsed counter. Ticks once per second while
  // a pipeline is in flight, then stops so we don't burn a timer forever.
  let nowMs = $state(Date.now());
  $effect(() => {
    const cur = pipeline.state.current;
    if (!cur || cur.finishedAt !== null) return;
    nowMs = Date.now();
    const id = setInterval(() => { nowMs = Date.now(); }, 1000);
    return () => clearInterval(id);
  });

  function formatPipelineElapsed(ms: number): string {
    const secs = Math.max(0, Math.floor(ms / 1000));
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return m > 0 ? `${m}m ${s}s` : `${s}s`;
  }
</script>

{#if pipeline.state.current}
  <div class="pipeline-status">
    <div class="pipeline-stages">
      <div class="stage" class:active={pipeline.state.current.stage === 'transcribing'} class:done={['generating_soap', 'completed'].includes(pipeline.state.current.stage)}>
        {#if pipeline.state.current.stage === 'transcribing'}
          <span class="spinner"></span>
        {:else if ['generating_soap', 'completed'].includes(pipeline.state.current.stage)}
          <span class="stage-check">✓</span>
        {:else}
          <span class="stage-dot">○</span>
        {/if}
        Transcribe
      </div>
      <span class="stage-arrow">→</span>
      <div class="stage" class:active={pipeline.state.current.stage === 'generating_soap'} class:done={pipeline.state.current.stage === 'completed'}>
        {#if pipeline.state.current.stage === 'generating_soap'}
          <span class="spinner"></span>
        {:else if pipeline.state.current.stage === 'completed'}
          <span class="stage-check">✓</span>
        {:else}
          <span class="stage-dot">○</span>
        {/if}
        SOAP Note
      </div>
      <span class="stage-arrow">→</span>
      <div class="stage" class:done={pipeline.state.current.stage === 'completed'}>
        {#if pipeline.state.current.stage === 'completed'}
          <span class="stage-check">✓</span>
        {:else}
          <span class="stage-dot">○</span>
        {/if}
        Done
      </div>
    </div>

    <p class="pipeline-label">{stageLabel(pipeline.state.current.stage)}</p>

    <p class="pipeline-elapsed">
      {#if pipeline.state.current.finishedAt !== null}
        {#if pipeline.state.current.stage === 'completed'}
          Processing took {formatPipelineElapsed(pipeline.state.current.finishedAt - pipeline.state.current.startedAt)}
        {:else}
          Stopped after {formatPipelineElapsed(pipeline.state.current.finishedAt - pipeline.state.current.startedAt)}
        {/if}
      {:else}
        Elapsed {formatPipelineElapsed(nowMs - pipeline.state.current.startedAt)}
      {/if}
    </p>

    {#if ['transcribing', 'generating_soap'].includes(pipeline.state.current.stage)}
      <div class="post-actions">
        <button class="btn-secondary" onclick={onCancel}>Cancel</button>
      </div>
    {/if}

    {#if pipeline.state.current.stage === 'completed'}
      <div class="post-actions">
        <button class="btn-secondary" onclick={onSpeedRead}>Speed Read</button>
        <button
          class="btn-primary"
          onclick={onCopySoap}
          disabled={copyStatus !== 'idle'}
        >
          {copyStatus === 'copying' ? 'Copying…' : copyStatus === 'copied' ? 'Copied!' : 'Copy SOAP Note'}
        </button>
      </div>
    {/if}

    {#if pipeline.state.current.stage === 'failed'}
      <div class="error-text">{pipeline.state.current.error}</div>
      <div class="post-actions">
        <button class="btn-primary" onclick={onRetry}>Retry</button>
      </div>
    {/if}
  </div>
{/if}

<style>
  .pipeline-status {
    text-align: center;
    max-width: 500px;
  }

  .pipeline-stages {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    margin-bottom: 16px;
  }

  .stage {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-muted);
    transition: color 0.2s ease;
  }

  .stage.active {
    color: var(--accent);
  }

  .stage.done {
    color: var(--success);
  }

  .stage-arrow {
    color: var(--text-muted);
    font-size: 14px;
  }

  .stage-check {
    color: var(--success);
    font-size: 14px;
  }

  .stage-dot {
    color: var(--text-muted);
    font-size: 12px;
  }

  .pipeline-label {
    font-size: 15px;
    font-weight: 500;
    color: var(--text-primary);
    margin-bottom: 4px;
  }

  .pipeline-elapsed {
    font-size: 13px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    margin-bottom: 8px;
  }

  .post-actions {
    margin-top: 16px;
    margin-bottom: 8px;
    display: flex;
    gap: 10px;
    justify-content: center;
    flex-wrap: wrap;
  }

  .btn-primary {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 10px 24px;
    background-color: var(--accent);
    color: white;
    border-radius: var(--radius-md);
    font-size: 14px;
    font-weight: 500;
    transition: background-color 0.15s ease;
  }

  .btn-primary:hover:not(:disabled) {
    background-color: var(--accent-hover);
  }

  .btn-primary:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-secondary {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 10px 24px;
    background-color: transparent;
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    font-size: 14px;
    font-weight: 500;
    transition: background-color 0.15s ease, border-color 0.15s ease;
    cursor: pointer;
  }

  .btn-secondary:hover:not(:disabled) {
    background-color: var(--bg-hover);
    border-color: var(--accent);
  }

  .spinner {
    display: inline-block;
    width: 14px;
    height: 14px;
    border: 2px solid rgba(255, 255, 255, 0.3);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .error-text {
    margin-top: 8px;
    padding: 8px 12px;
    border-radius: var(--radius-sm);
    background-color: rgba(239, 68, 68, 0.1);
    color: var(--danger, #ef4444);
    font-size: 13px;
  }
</style>
