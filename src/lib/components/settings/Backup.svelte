<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getBackupStatus,
    escrowInit,
    escrowVerify,
    installBackupSchedule,
    uninstallBackupSchedule,
    runBackupNow,
    onBackupJobEvent,
    type BackupStatus,
    type BackupJobEvent,
  } from '../../api/backup';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { settings } from '../../stores/settings.svelte';
  import { formatError } from '../../types/errors';

  let status = $state<BackupStatus | null>(null);
  let statusError = $state<string | null>(null);
  let busy = $state<string | null>(null); // which action is running
  let message = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  // Escrow flow
  let escrowDir = $state('');
  let escrowResult = $state<{ sheetPath: string; usbPath: string } | null>(null);

  // Schedule form
  let hour = $state(3);
  let minute = $state(30);
  let targetUrl = $state('');
  let appendToken = $state('');

  // Drill log lines
  let jobLines = $state<BackupJobEvent[]>([]);
  let jobRunning = $state(false);

  async function refresh(prefillUrl = false) {
    statusError = null;
    try {
      status = await getBackupStatus();
      // Prefill only when explicitly requested (initial load / after
      // install) — a plain refresh must never clobber user edits.
      if (prefillUrl) {
        targetUrl = settings.state.backup_target_url ?? '';
      }
      // Never echo the stored token back into the field.
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
      await refresh(true);
    })();
    return () => {
      unlisten?.();
    };
  });

  async function handleEscrowInit() {
    message = null;
    if (!escrowDir.trim()) {
      message = { kind: 'error', text: 'Choose a folder for the escrow files first (e.g. your Desktop).' };
      return;
    }
    busy = 'escrow';
    try {
      escrowResult = await escrowInit(escrowDir.trim());
      message = {
        kind: 'ok',
        text: 'Escrow files written. PRINT the recovery sheet now and store it in a safe, off-machine place; copy the USB file to an offline stick.',
      };
      await refresh();
    } catch (e) {
      message = { kind: 'error', text: formatError(e) || 'escrow setup failed' };
    } finally {
      busy = null;
    }
  }

  async function handleVerify(path: string) {
    message = null;
    busy = 'verify';
    try {
      message = { kind: 'ok', text: await escrowVerify(path) };
    } catch (e) {
      message = { kind: 'error', text: formatError(e) || 'verification failed' };
    } finally {
      busy = null;
    }
  }

  async function handleInstallSchedule() {
    message = null;
    // #27: clamp/validate before invoke — a cleared number input binds
    // NaN, which serializes to null and confuses serde downstream.
    const h = Math.trunc(Number(hour));
    const m = Math.trunc(Number(minute));
    if (!Number.isInteger(h) || h < 0 || h > 23 || !Number.isInteger(m) || m < 0 || m > 59) {
      message = { kind: 'error', text: 'Time must be a valid 24h hour (0-23) and minute (0-59).' };
      return;
    }
    // #25: a URL without a token silently degrades run-now to local-only
    // and the scheduled job to failing pushes — say it now, not at 3am.
    if (targetUrl.trim() && !appendToken.trim()) {
      message = {
        kind: 'error',
        text: 'A target URL needs the append token — paste the target\'s FERRISCRIBE_BACKUP_APPEND_TOKEN.',
      };
      return;
    }
    busy = 'schedule';
    try {
      message = { kind: 'ok', text: await installBackupSchedule(h, m, targetUrl.trim() || null, appendToken.trim() || null) };
      // #25: wipe the plaintext from the field/state immediately, and
      // #24: reload the store so the pane reflects what was persisted.
      appendToken = '';
      await settings.load();
      await refresh(true);
    } catch (e) {
      message = { kind: 'error', text: formatError(e) || 'schedule install failed' };
    } finally {
      busy = null;
    }
  }

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
    if (status.stale) return { label: `Stale (ran ${fmtWhen(status.lastRunAt)})`, cls: 'warn' };
    return { label: `Healthy · ran ${fmtWhen(status.lastRunAt)}`, cls: 'good' };
  });

  const scheduleSupported = $derived(status?.scheduleSupported ?? false);
</script>

<section class="settings-section">
  <h3 class="section-title">Backup</h3>
  <p class="subsection-hint">
    Encrypted, append-only off-machine backup. The daily schedule runs outside the app
    (launchd), so backups continue even if FerriScribe is closed. Recovery needs your
    printed recovery sheet (or the USB escrow file) plus the backup target.
  </p>

  {#if statusError}
    <div class="endpoint-warning" role="alert">⚠ {statusError}</div>
  {/if}

  {#if status}
    <div class="status-card" class:bad={health.cls === 'bad'} class:warn={health.cls === 'warn'} class:good={health.cls === 'good'}>
      <div class="status-line">
        <span class="dot"></span>
        <strong>{health.label}</strong>
      </div>
      <ul class="status-facts">
        {#if status.everRan}
          <li>last snapshot: <code>{status.snapshotId ?? '—'}</code></li>
          <li>pushed to: {status.pushedTo ?? 'local only (no target configured)'}</li>
          {#if status.failure}<li class="fail-line">failure: {status.failure}</li>{/if}
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
        {#each jobLines as line}
          <div class="job-line {line.kind}">{line.kind === 'ok' ? '✓' : line.kind === 'fail' ? '✗' : '·'} {line.line}</div>
        {/each}
      </div>
    {/if}
  {/if}

  {#if message}
    <div class="message {message.kind}" role="status">{message.text}</div>
  {/if}

  <!-- Escrow -->
  <div class="form-group-divider"></div>
  <h4 class="subsection-title">Recovery key escrow</h4>
  <p class="subsection-hint">
    Generates (once) the backup wrapping key and writes two independent recovery
    artifacts. Each alone can restore everything on a clean machine. Keep the sheet in a
    fire-safe and the USB copy offline — anyone holding one <em>and</em> access to the
    backup target can read your data.
  </p>
  <div class="form-group">
    <label for="escrow-dir" class="form-label">Output folder</label>
    <input
      id="escrow-dir"
      type="text"
      bind:value={escrowDir}
      placeholder="/Users/you/Desktop"
      class="text-input"
    />
    <button class="btn-secondary" onclick={handleEscrowInit} disabled={busy !== null}>
      {busy === 'escrow' ? 'Writing…' : 'Generate / re-emit escrow files'}
    </button>
  </div>
  {#if escrowResult}
    <ul class="artifact-list">
      <li>
        <code>{escrowResult.sheetPath}</code> — print this
        <button class="link-btn" onclick={() => handleVerify(escrowResult!.sheetPath)} disabled={busy !== null}>
          verify
        </button>
      </li>
      <li>
        <code>{escrowResult.usbPath}</code> — copy to an offline USB stick
        <button class="link-btn" onclick={() => handleVerify(escrowResult!.usbPath)} disabled={busy !== null}>
          verify
        </button>
      </li>
    </ul>
  {/if}

  <!-- Schedule -->
  <div class="form-group-divider"></div>
  <h4 class="subsection-title">Daily schedule</h4>
  {#if !scheduleSupported}
    <p class="subsection-hint">
      Scheduled backups are macOS-only in this build (launchd). On Linux, point a systemd
      timer at the bundled <code>ferriscribe-backup backup-and-push</code> command — see
      the README. "Back up now" works everywhere.
    </p>
  {/if}
  <div class="form-group schedule-row">
    <label class="form-label" for="bk-hour">Time</label>
    <input id="bk-hour" type="number" min="0" max="23" bind:value={hour} class="text-input tiny" />
    <span>:</span>
    <input id="bk-minute" type="number" min="0" max="59" bind:value={minute} class="text-input tiny" />
    <span class="hint-inline">24h, local time</span>
  </div>
  <div class="form-group">
    <label for="bk-url" class="form-label">Target URL (optional — leave empty for local-only snapshots)</label>
    <input id="bk-url" type="text" bind:value={targetUrl} placeholder="http://100.64.0.2:8741" class="text-input" />
  </div>
  <div class="form-group">
    <label for="bk-token" class="form-label">Append token (from the target's FERRISCRIBE_BACKUP_APPEND_TOKEN)</label>
    <input id="bk-token" type="password" bind:value={appendToken} placeholder="paste token" class="text-input" autocomplete="off" />
    <p class="form-hint">Stored encrypted in the app database; never shown again after saving.</p>
  </div>
  <div class="form-group">
    <button class="btn-secondary" onclick={handleInstallSchedule} disabled={busy !== null || !scheduleSupported}>
      {busy === 'schedule' ? 'Installing…' : 'Install daily schedule'}
    </button>
    {#if status?.scheduleInstalled}
      <button class="btn-secondary danger" onclick={handleUninstallSchedule} disabled={busy !== null || !scheduleSupported}>
        Remove schedule
      </button>
    {/if}
  </div>
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
  .subsection-title { font-size: 14px; font-weight: 600; margin: 0 0 4px; }
  .subsection-hint { font-size: 12px; color: var(--text-muted); margin: 0 0 12px; line-height: 1.5; }
  .form-group { margin-bottom: 12px; }
  .form-label { display: block; font-size: 12px; font-weight: 600; margin-bottom: 4px; }
  .text-input {
    width: 100%;
    padding: 8px 10px;
    background: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
  }
  .text-input.tiny { width: 70px; }
  .schedule-row { display: flex; align-items: center; gap: 8px; }
  .schedule-row .form-label { margin-bottom: 0; }
  .hint-inline { font-size: 11px; color: var(--text-muted); }
  .form-hint { font-size: 11px; color: var(--text-muted); margin: 4px 0 0; }
  .artifact-list { margin: 8px 0; padding-left: 18px; font-size: 12px; }
  .artifact-list code { font-size: 11px; }
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
  .link-btn {
    background: none; border: none; color: var(--accent);
    cursor: pointer; font-size: 11px; padding: 0 4px; text-decoration: underline;
  }
  .endpoint-warning { color: #b45309; background: #fef3c7; border: 1px solid #fbbf24; border-radius: 4px; padding: 6px 10px; margin-bottom: 10px; font-size: 0.85rem; }
</style>
