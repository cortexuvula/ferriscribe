<script lang="ts">
  import type { RecordingSummary } from '../types';
  import { formatDate, formatDuration } from '../utils/format';

  interface Props {
    recording: RecordingSummary;
    selected?: boolean;
    onClick?: () => void;
    onDelete?: (() => void) | null;
    onRetry?: (() => void) | null;
  }

  let { recording, selected = false, onClick = () => {}, onDelete = null, onRetry = null }: Props = $props();

  function statusIcon(status: RecordingSummary['status']): string {
    switch (status.status) {
      case 'completed':   return '✓';
      case 'processing':  return '⟳';
      case 'failed':      return '✗';
      default:            return '—';
    }
  }

  function statusColor(status: RecordingSummary['status']): string {
    switch (status.status) {
      case 'completed':  return 'var(--success)';
      case 'processing': return 'var(--warning)';
      case 'failed':     return 'var(--danger)';
      default:           return 'var(--text-muted)';
    }
  }
</script>

<div
  class="recording-card"
  class:selected
  onclick={onClick}
  onkeydown={(e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onClick();
    }
  }}
  role="button"
  tabindex="0"
>
  <div class="card-status" style="color: {statusColor(recording.status)}">
    {statusIcon(recording.status)}
  </div>

  <div class="card-main">
    <div class="card-name truncate">
      {recording.patient_name ?? recording.filename}
    </div>
    <div class="card-meta">
      <span>{formatDate(recording.created_at)}</span>
      <span class="sep">·</span>
      <span>{formatDuration(recording.duration_seconds)}</span>
    </div>
  </div>

  <div class="card-badges">
    {#if recording.has_transcript}
      <span class="badge" title="Transcript">T</span>
    {/if}
    {#if recording.has_soap_note}
      <span class="badge" title="SOAP Note">S</span>
    {/if}
    {#if recording.has_referral}
      <span class="badge" title="Referral">R</span>
    {/if}
    {#if recording.has_letter}
      <span class="badge" title="Letter">L</span>
    {/if}
    {#if recording.has_peer_discussion}
      <span class="badge" title="Peer Discussion">PD</span>
    {/if}
  </div>

  {#if onRetry}
    <button
      class="btn-retry"
      title="Re-transcribe"
      onclick={(e: MouseEvent) => { e.stopPropagation(); onRetry(); }}
    >
      ↻
    </button>
  {/if}

  {#if onDelete}
    <button
      class="btn-delete"
      title="Delete recording"
      onclick={(e: MouseEvent) => { e.stopPropagation(); onDelete!(); }}
    >
      ×
    </button>
  {/if}
</div>

<style>
  .recording-card {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border-light);
    text-align: left;
    transition: background-color 0.1s ease;
    background-color: transparent;
    cursor: pointer;
  }

  .recording-card:hover {
    background-color: var(--bg-hover);
  }

  .recording-card.selected {
    background-color: var(--accent-light);
  }

  .recording-card:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  .card-status {
    font-size: 14px;
    width: 18px;
    flex-shrink: 0;
    text-align: center;
  }

  .card-main {
    flex: 1;
    min-width: 0;
  }

  .card-name {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary);
    margin-bottom: 2px;
  }

  .card-meta {
    font-size: 11px;
    color: var(--text-muted);
    display: flex;
    gap: 4px;
  }

  .sep {
    color: var(--border);
  }

  .card-badges {
    display: flex;
    gap: 3px;
    flex-shrink: 0;
  }

  .badge {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 18px;
    height: 18px;
    padding: 0 3px;
    border-radius: var(--radius-sm);
    font-size: 10px;
    font-weight: 700;
    background-color: var(--accent-light);
    color: var(--accent);
    border: 1px solid var(--accent);
  }

  .btn-delete {
    display: none;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: var(--radius-sm);
    font-size: 16px;
    font-weight: 600;
    color: var(--text-muted);
    background: transparent;
    flex-shrink: 0;
    transition: color 0.15s ease, background-color 0.15s ease;
  }

  .recording-card:hover .btn-delete,
  .recording-card:focus-within .btn-delete {
    display: inline-flex;
  }

  .btn-delete:hover {
    color: var(--danger, #ef4444);
    background-color: rgba(239, 68, 68, 0.1);
  }

  .btn-retry {
    display: none;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: var(--radius-sm);
    font-size: 14px;
    font-weight: 600;
    color: var(--text-muted);
    background: transparent;
    flex-shrink: 0;
    transition: color 0.15s ease, background-color 0.15s ease;
  }

  .recording-card:hover .btn-retry,
  .recording-card:focus-within .btn-retry {
    display: inline-flex;
  }

  .btn-retry:hover {
    color: var(--accent);
    background-color: color-mix(in srgb, var(--accent) 10%, transparent);
  }
</style>
