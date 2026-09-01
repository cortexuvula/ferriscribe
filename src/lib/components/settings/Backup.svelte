<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getBackupStatus,
    isProtected,
    onBackupJobEvent,
    runBackupNow,
    uninstallBackupSchedule,
    type BackupStatus,
    type BackupJobEvent,
  } from '../../api/backup';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { formatError } from '../../types/errors';
  import BackupWizard from './BackupWizard.svelte';

  let status = $state<BackupStatus | null>(null);
  let statusError = $state<string | null>(null);
  let busy = $state<string | null>(null); // which action is running
  let message = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  // Drill log lines (Back up now)
  let jobLines = $state<BackupJobEvent[]>([]);
  let jobRunning = $state(false);

  async function refresh() {
    statusError = null;
    try {
      status = await getBackupStatus();
    } catch (e) {
      statusError = formatError(e) || 'failed to read backup status';
    }
  }

  onMount(() => {
    // Async setup + teardown-safe cleanup: Svelte's onMount only accepts
    // a cleanup return from its synchronous form.
    let unlisten: UnlistenFn | null = null;
    void (async () => {
      unlisten = await onBackupJobEvent((e) => jobLines.push(e));
      await refresh();
    })();
    return () => {
      unlisten?.();
    };
  });

  async function handleUninstallSchedule() {
    message = null;
    busy = 'schedule';
    try {
      message = { kind: 'ok', text: await uninstallBackupSchedule() };
      await refresh();
    } catch (e) {
      message = { kind: 'error', text: formatError(e) || 'schedule removal failed' };
    } finally {
      busy = null;
    }
  }

  async function handleRunNow() {
    message = null;
    jobLines = [];
    jobRunning = true;
    try {
      const ok = await runBackupNow();
      message = ok
        ? { kind: 'ok', text: 'Backup job passed.' }
        : { kind: 'error', text: 'Backup job FAILED — see the log below.' };
    } catch (e) {
      message = { kind: 'error', text: formatError(e) || 'backup job failed to start' };
    } finally {
      jobRunning = false;
      await refresh();
    }
  }

  function fmtWhen(iso: string | null): string {
    if (!iso) return 'never';
    const d = new Date(iso);
    const diffH = Math.round((Date.now() - d.getTime()) / 3_600_000);
    if (diffH < 1) return 'just now';
    if (diffH < 48) return `${diffH}h ago`;
    return d.toLocaleDateString();
  }

  const health = $derived.by(() => {
    if (!status) return { label: 'Unknown', cls: 'unknown' };
    if (!status.everRan) return { label: 'Never ran', cls: 'bad' };
    if (!status.drillPassed) return { label: 'Last drill FAILED', cls: 'bad' };
    // A drive that's simply unplugged is a WAITING state — the last good
    // snapshot is still on it and restorable. (The job preserves the last
    // good facts on these runs; see missing_destination_preserves… test.)
    if (status.destinationMissing) {
      return { label: `Waiting for backup drive · last good ${fmtWhen(status.lastRunAt)}`, cls: 'warn' };
    }
    if (status.stale) return { label: `Stale (ran ${fmtWhen(status.lastRunAt)})`, cls: 'warn' };
    return { label: `Healthy · ran ${fmtWhen(status.lastRunAt)}`, cls: 'good' };
  });

  const destinationLabel = $derived.by(() => {
    if (!status) return null;
    if (status.destinationKind === 'agent') return 'backup server';
    if (status.destinationKind === 'folder') {
      return status.destinationPresent ? 'folder · connected' : 'folder · NOT connected';
    }
    return 'this Mac only';
  });
</script>

<section class="settings-section">
  <h3 class="section-title">Backup</h3>
  <p class="subsection-hint">
    Encrypted backups to a drive, folder, or server of your choice. The daily schedule runs
    outside the app (launchd), so backups continue even if FerriScribe is closed. Recovery
    needs your printed recovery sheet (or the USB escrow file).
  </p>

  {#if statusError}
    <div class="endpoint-warning" role="alert">⚠ {statusError}</div>
  {/if}

  {#if status && status.everRan && !isProtected(status)}
    <div class="endpoint-warning" role="alert">
      ⚠ Backups currently protect this Mac only. Connect a backup drive or server to be safe
      against disk failure.
    </div>
  {/if}

  {#if status}
    <div
      class="status-card"
      class:bad={health.cls === 'bad'}
      class:warn={health.cls === 'warn'}
      class:good={health.cls === 'good'}
    >
      <div class="status-line">
        <span class="dot"></span>
        <strong>{health.label}</strong>
      </div>
      <ul class="status-facts">
        {#if status.everRan}
          <li>last snapshot: <code>{status.snapshotId ?? '—'}</code></li>
          <li>pushed to: {status.pushedTo ?? 'local only (no destination configured)'}</li>
          {#if status.failure}<li class="fail-line">failure: {status.failure}</li>{/if}
        {/if}
        <li>destination: {destinationLabel}</li>
        {#if status.destinationKind === 'local-only'}
          <li class="fail-line">a disk failure still loses everything — pick a destination below</li>
        {/if}
        <li>escrow key: {status.wrappingKeyPresent ? 'initialized' : 'not set up'}</li>
        <li>schedule: {status.scheduleInstalled ? 'installed' : 'not installed'}</li>
        {#if !status.toolCopyOk && status.scheduleInstalled}
          <li class="fail-line">backup tool outdated — reinstall the schedule to update it</li>
        {/if}
      </ul>
      <button class="btn-secondary" onclick={handleRunNow} disabled={jobRunning || busy !== null}>
        {jobRunning ? 'Running…' : 'Back up now'}
      </button>
    </div>

    {#if jobLines.length > 0}
      <div class="job-log" aria-live="polite">
        {#each jobLines as line, i (i)}
          <div class="job-line {line.kind}">
            {line.kind === 'ok' ? '✓' : line.kind === 'fail' ? '✗' : '·'} {line.line}
          </div>
        {/each}
      </div>
    {/if}
  {/if}

  {#if message}
    <div class="message {message.kind}" role="status">{message.text}</div>
  {/if}

  <div class="form-group-divider"></div>
  <BackupWizard onDone={() => refresh()} />
  {#if status?.scheduleInstalled}
    <div class="form-group">
      <button
        class="btn-secondary danger"
        onclick={handleUninstallSchedule}
        disabled={busy !== null}
      >
        Remove schedule
      </button>
    </div>
  {/if}
</section>

<style>
  .status-card {
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 12px 14px;
    margin-bottom: 12px;
  }
  .status-card.bad { border-color: var(--danger, #ef4444); }
  .status-card.warn { border-color: var(--warning, #d97706); }
  .status-card.good { border-color: var(--success, #22c55e); }
  .status-line { display: flex; align-items: center; gap: 8px; margin-bottom: 6px; }
  .dot { width: 10px; height: 10px; border-radius: 50%; background: var(--text-muted); }
  .bad .dot { background: var(--danger, #ef4444); }
  .warn .dot { background: var(--warning, #d97706); }
  .good .dot { background: var(--success, #22c55e); }
  .status-facts { margin: 0 0 10px 0; padding-left: 18px; font-size: 12px; color: var(--text-secondary); }
  .status-facts code { font-size: 11px; }
  .fail-line { color: var(--danger, #ef4444); }
  .job-log {
    font-family: ui-monospace, monospace;
    font-size: 11px;
    background: var(--bg-tertiary, #1f2937);
    border-radius: var(--radius-sm);
    padding: 8px 10px;
    max-height: 180px;
    overflow-y: auto;
    margin-bottom: 12px;
  }
  .job-line.ok { color: var(--success, #22c55e); }
  .job-line.fail { color: var(--danger, #ef4444); }
  .job-line.step { color: var(--text-muted); }
  .message.ok { color: var(--success, #22c55e); font-size: 13px; margin: 8px 0; }
  .message.error { color: var(--danger, #ef4444); font-size: 13px; margin: 8px 0; }
  .form-group-divider { border-top: 1px solid var(--border); margin: 20px 0 16px; }
  .subsection-hint { font-size: 12px; color: var(--text-muted); margin: 0 0 12px; line-height: 1.5; }
  .form-group { margin: 12px 0; }
  .btn-secondary {
    padding: 8px 16px;
    background: transparent;
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .btn-secondary:disabled { opacity: 0.5; cursor: default; }
  .btn-secondary.danger { border-color: var(--danger, #ef4444); color: var(--danger, #ef4444); }
  .endpoint-warning { color: #b45309; background: #fef3c7; border: 1px solid #fbbf24; border-radius: 4px; padding: 6px 10px; margin-bottom: 10px; font-size: 0.85rem; }
</style>
