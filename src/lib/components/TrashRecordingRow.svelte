<script lang="ts">
  import type { TrashedRecordingSummary } from '../types';
  import { formatDuration } from '../utils/format';
  import { formatTrashDateTime, trashRetentionLabel } from '../utils/trashRetention';

  /// One trashed recording. Deliberately NOT the RecordingCard pattern:
  /// no whole-row click (nothing to open — the row has no content fields),
  /// no transcript excerpts, no processing/generation controls. Restore is
  /// always visible (never hover-only): it is the row's only action.
  interface Props {
    item: TrashedRecordingSummary;
    onRestore: () => void;
    restoreDisabled?: boolean;
    /// Test seam: fixed "now" (epoch ms) for the days-remaining display.
    now?: number;
  }

  const { item, onRestore, restoreDisabled = false, now = Date.now() }: Props = $props();

  /// ONE label: patient name when present, else filename — never both.
  const displayName = $derived(item.patient_name ?? item.filename);
  const retention = $derived(trashRetentionLabel(item.deleted_at, now));
</script>

<div class="trash-row" class:near-expiry={retention.nearExpiry}>
  <div class="row-main">
    <div class="row-name truncate" title={displayName}>{displayName}</div>
    <div class="row-meta">
      <span>Recorded {formatTrashDateTime(item.created_at)} · {formatDuration(item.duration_seconds)}</span>
      <span>Moved to Trash {formatTrashDateTime(item.deleted_at)}</span>
    </div>
    <div class="row-retention" class:amber={retention.nearExpiry}>{retention.text}</div>
  </div>
  <button
    class="btn-restore"
    data-restore-btn={item.id}
    onclick={onRestore}
    disabled={restoreDisabled}
  >
    Restore
  </button>
</div>

<style>
  .trash-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
  }

  .row-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .row-name {
    font-size: 14px;
    font-weight: 500;
    color: var(--text-primary);
  }

  .row-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    font-size: 12px;
    color: var(--text-muted);
  }

  .row-retention {
    font-size: 12px;
    color: var(--text-muted);
  }

  /* Restrained amber near expiry: a tint, not a warning banner. */
  .row-retention.amber {
    color: var(--warning, #d97706);
  }

  .trash-row.near-expiry {
    background-color: color-mix(in srgb, var(--warning, #d97706) 4%, transparent);
  }

  .btn-restore {
    padding: 6px 14px;
    font-size: 12px;
    font-weight: 500;
    color: var(--accent);
    background-color: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    flex-shrink: 0;
    transition: background-color 0.15s ease, border-color 0.15s ease;
  }

  .btn-restore:hover:not(:disabled) {
    background-color: var(--bg-hover);
    border-color: var(--accent);
  }

  .btn-restore:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
