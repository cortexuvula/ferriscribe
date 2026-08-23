<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import {
    escrowInit,
    escrowVerify,
    installBackupSchedule,
    onBackupJobEvent,
    parseTimeOfDay,
    runBackupNow,
    testBackupAgent,
    testBackupDestination,
    type AgentProbe,
    type BackupJobEvent,
    type DestinationProbe,
  } from '../../api/backup';

  interface Props {
    /** Tighter chrome when rendered inside onboarding. */
    embedded?: boolean;
    /** Called when the user finishes the flow. */
    onDone?: () => void;
  }
  const { embedded = false, onDone }: Props = $props();

  type Step = 'destination' | 'escrow' | 'schedule' | 'firstrun';
  const STEPS: { id: Step; label: string }[] = [
    { id: 'destination', label: 'Where backups live' },
    { id: 'escrow', label: 'Recovery key' },
    { id: 'schedule', label: 'When' },
    { id: 'firstrun', label: 'First backup' },
  ];

  let step = $state<Step>('destination');
  let kind = $state<'folder' | 'agent' | null>(null);
  let folderPath = $state('');
  let probe = $state<DestinationProbe | null>(null);
  let probing = $state(false);
  let agentUrl = $state('');
  let agentToken = $state('');
  let agentTesting = $state(false);
  let agentProbe = $state<AgentProbe | null>(null);

  let escrowDir = $state('');
  let escrowBusy = $state(false);
  let escrowResult = $state<{ sheetPath: string; usbPath: string } | null>(null);
  let escrowVerified = $state(false);
  let escrowError = $state<string | null>(null);
  let sheetConfirmed = $state(false);

  let time = $state('03:30');
  let scheduleBusy = $state(false);
  let scheduleError = $state<string | null>(null);
  let scheduleInstalled = $state(false);

  let jobLines = $state<BackupJobEvent[]>([]);
  let jobRunning = $state(false);
  let jobPassed = $state<boolean | null>(null);

  const stepIndex = $derived(STEPS.findIndex((s) => s.id === step));

  async function pickFolder(defaultPath?: string): Promise<string | null> {
    const selected = await open({ directory: true, defaultPath });
    return typeof selected === 'string' ? selected : null;
  }

  async function chooseFolderDest() {
    probe = null;
    const p = await pickFolder();
    if (!p) return;
    folderPath = p;
    kind = 'folder';
    probing = true;
    try {
      probe = await testBackupDestination(folderPath);
    } catch (e) {
      probe = {
        writable: false,
        freeBytes: null,
        problem: e instanceof Error ? e.message : String(e),
      };
    } finally {
      probing = false;
    }
  }

  async function testAgent() {
    agentProbe = null;
    agentTesting = true;
    try {
      agentProbe = await testBackupAgent(agentUrl, agentToken);
    } catch (e) {
      agentProbe = { ok: false, problem: e instanceof Error ? e.message : String(e) };
    } finally {
      agentTesting = false;
    }
  }

  const destOk = $derived.by(() => {
    if (kind === 'folder') return probe?.writable === true;
    if (kind === 'agent') return agentUrl.trim().length > 0 && agentToken.trim().length > 0;
    return false;
  });

  async function chooseEscrowDir() {
    const p = await pickFolder();
    if (p) escrowDir = p;
  }

  async function runEscrow() {
    escrowError = null;
    escrowBusy = true;
    escrowVerified = false;
    try {
      escrowResult = await escrowInit(escrowDir.trim());
      // Verification of recovery material is not optional — run it for
      // both artifacts immediately (the old manual verify buttons are
      // gone).
      await escrowVerify(escrowResult.sheetPath);
      await escrowVerify(escrowResult.usbPath);
      escrowVerified = true;
    } catch (e) {
      escrowError = e instanceof Error ? e.message : String(e);
    } finally {
      escrowBusy = false;
    }
  }

  async function installSchedule() {
    scheduleError = null;
    const parsed = parseTimeOfDay(time);
    if (!parsed) {
      scheduleError = 'Pick a valid time (HH:MM).';
      return;
    }
    const [h, m] = parsed;
    scheduleBusy = true;
    try {
      await installBackupSchedule(
        h,
        m,
        kind === 'agent' ? agentUrl.trim() : null,
        kind === 'agent' ? agentToken.trim() : null,
        kind === 'folder' ? folderPath : null,
      );
      agentToken = '';
      scheduleInstalled = true;
    } catch (e) {
      scheduleError = e instanceof Error ? e.message : String(e);
    } finally {
      scheduleBusy = false;
    }
  }

  async function runFirstBackup() {
    jobLines = [];
    jobRunning = true;
    jobPassed = null;
    let unlisten: (() => void) | null = null;
    try {
      unlisten = await onBackupJobEvent((e) => jobLines.push(e));
      jobPassed = await runBackupNow();
    } finally {
      unlisten?.();
      jobRunning = false;
    }
  }

  function show(path: string) {
    void revealItemInDir(path);
  }

  function next() {
    if (step === 'destination' && destOk) step = 'escrow';
    else if (step === 'escrow' && escrowVerified && sheetConfirmed) step = 'schedule';
    else if (step === 'schedule' && scheduleInstalled) step = 'firstrun';
  }
  function back() {
    if (step === 'escrow') step = 'destination';
    else if (step === 'schedule') step = 'escrow';
    else if (step === 'firstrun') step = 'schedule';
  }
</script>

<div class="wizard" class:embedded>
  <div class="progress" aria-hidden="true">
    {#each STEPS as s, i (s.id)}
      <span class="pip" class:done={i < stepIndex} class:current={i === stepIndex}></span>
    {/each}
    <span class="progress-label">
      Step {stepIndex + 1} of {STEPS.length} · {STEPS[stepIndex].label}
    </span>
  </div>

  {#if step === 'destination'}
    <h4>Where should your backups live?</h4>
    <p class="hint">
      Backups are encrypted — they contain no patient-identifying filenames — so an external
      drive or a cloud-synced folder is safe to carry around.
    </p>
    <div class="cards">
      <button class="card" class:selected={kind === 'folder'} onclick={chooseFolderDest}>
        <strong>USB or external drive</strong>
        <span>Plug in a drive and pick it. Unplug it between backups for the strongest protection.</span>
        {#if kind === 'folder' && folderPath}
          <code>{folderPath}</code>
        {/if}
      </button>
      <button class="card" class:selected={kind === 'folder'} onclick={chooseFolderDest}>
        <strong>Folder on this Mac or a network drive</strong>
        <span>Any folder — including one that syncs to iCloud Drive or Dropbox.</span>
        {#if kind === 'folder' && folderPath}
          <code>{folderPath}</code>
        {/if}
      </button>
      <button class="card" class:selected={kind === 'agent'} onclick={() => (kind = 'agent')}>
        <strong>Backup server (advanced)</strong>
        <span>An append-only server you run on another machine. Strongest ransomware protection.</span>
      </button>
    </div>
    {#if probing}<p class="hint">Checking the folder…</p>{/if}
    {#if probe && kind === 'folder'}
      {#if probe.writable}
        <p class="ok">
          ✓ FerriScribe can write here{probe.freeBytes != null
            ? ` · ${Math.round(probe.freeBytes / 1e9)} GB free`
            : ''}
        </p>
      {:else}
        <p class="err">✗ {probe.problem ?? 'Folder is not usable.'}</p>
      {/if}
    {/if}
    {#if kind === 'agent'}
      <div class="form-group">
        <label for="wiz-url" class="form-label">Target URL</label>
        <input
          id="wiz-url"
          class="text-input"
          type="text"
          bind:value={agentUrl}
          placeholder="http://100.64.0.2:8741"
        />
        <label for="wiz-token" class="form-label">Append token</label>
        <input
          id="wiz-token"
          class="text-input"
          type="password"
          bind:value={agentToken}
          placeholder="paste token"
          autocomplete="off"
        />
        <button class="btn-secondary" onclick={testAgent} disabled={agentTesting}>
          {agentTesting ? 'Testing…' : 'Test connection'}
        </button>
        {#if agentProbe}
          {#if agentProbe.ok}
            <p class="ok">✓ Backup server reachable.</p>
          {:else}
            <p class="err">✗ {agentProbe.problem}</p>
          {/if}
        {/if}
        <p class="hint">Stored encrypted; never shown again after saving.</p>
      </div>
    {/if}
  {:else if step === 'escrow'}
    <h4>Print your recovery key</h4>
    <p class="hint">
      This sheet is the only thing that can unlock your backups on a new machine. Print it and
      keep it somewhere safe away from this computer (a fire-safe is ideal).
    </p>
    {#if !escrowResult}
      <div class="form-group">
        <button class="btn-secondary" onclick={chooseEscrowDir}>
          {escrowDir ? 'Change folder…' : 'Choose where to save the recovery files…'}
        </button>
        {#if escrowDir}<code>{escrowDir}</code>{/if}
        <button
          class="btn-primary"
          onclick={runEscrow}
          disabled={!escrowDir.trim() || escrowBusy}
        >
          {escrowBusy ? 'Writing…' : 'Generate recovery files'}
        </button>
        {#if escrowError}<p class="err">{escrowError}</p>{/if}
      </div>
    {:else}
      {#if escrowVerified}
        <p class="ok">✓ Recovery files written and verified.</p>
      {/if}
      <ul class="artifact-list">
        <li>
          <code>{escrowResult.sheetPath}</code> — print this
          <button class="link-btn" onclick={() => show(escrowResult!.sheetPath)}>
            show in Finder
          </button>
        </li>
        <li>
          <code>{escrowResult.usbPath}</code> — copy to an offline USB stick (optional but
          recommended)
          <button class="link-btn" onclick={() => show(escrowResult!.usbPath)}>
            show in Finder
          </button>
        </li>
      </ul>
      <label class="confirm">
        <input type="checkbox" bind:checked={sheetConfirmed} />
        I've printed the sheet and put it somewhere safe away from this computer.
      </label>
    {/if}
  {:else if step === 'schedule'}
    <h4>When should backups run?</h4>
    <p class="hint">
      Daily, even when FerriScribe is closed. If the computer is asleep at that time, the backup
      runs when it wakes. {kind === 'folder'
        ? 'Connect your drive before then — or any time; plugging it in triggers a catch-up backup.'
        : ''}
    </p>
    <div class="form-group">
      <label for="wiz-time" class="form-label">Time</label>
      <input id="wiz-time" class="text-input time" type="time" bind:value={time} />
      <button class="btn-primary" onclick={installSchedule} disabled={scheduleBusy}>
        {scheduleBusy ? 'Scheduling…' : scheduleInstalled ? 'Re-schedule' : 'Turn on daily backups'}
      </button>
      {#if scheduleInstalled}<p class="ok">✓ Daily backups scheduled.</p>{/if}
      {#if scheduleError}<p class="err">{scheduleError}</p>{/if}
    </div>
  {:else}
    <h4>First backup + safety test</h4>
    <p class="hint">
      This runs a real backup now, then proves it can be restored — automatically. If both pass,
      your data is protected.
    </p>
    <button class="btn-primary" onclick={runFirstBackup} disabled={jobRunning}>
      {jobRunning ? 'Running…' : 'Back up now'}
    </button>
    {#if jobLines.length > 0}
      <div class="job-log" aria-live="polite">
        {#each jobLines as line, i (i)}
          <div class="job-line {line.kind}">
            {line.kind === 'ok' ? '✓' : line.kind === 'fail' ? '✗' : '·'} {line.line}
          </div>
        {/each}
      </div>
    {/if}
    {#if jobPassed === true}
      <p class="ok">✓ Your data is backed up and a test restore passed.</p>
    {:else if jobPassed === false}
      <p class="err">
        ✗ The backup or its test failed — see the log above. {kind === 'folder'
          ? 'If the drive was unplugged, connect it and try again.'
          : ''}
      </p>
    {/if}
  {/if}

  <div class="nav">
    {#if step !== 'destination'}
      <button class="btn-secondary" onclick={back}>Back</button>
    {/if}
    {#if step === 'firstrun' && jobPassed !== null}
      <button class="btn-primary" onclick={() => onDone?.()}>Done</button>
    {:else if step !== 'firstrun'}
      <button
        class="btn-primary"
        onclick={next}
        disabled={(step === 'destination' && !destOk) ||
          (step === 'escrow' && (!escrowVerified || !sheetConfirmed)) ||
          (step === 'schedule' && !scheduleInstalled)}
      >
        Continue
      </button>
    {/if}
  </div>
</div>

<style>
  .wizard {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .progress {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .pip {
    width: 22px;
    height: 4px;
    border-radius: 2px;
    background: var(--border);
  }
  .pip.done {
    background: var(--accent);
  }
  .pip.current {
    background: var(--accent);
    opacity: 0.55;
  }
  .progress-label {
    font-size: 11px;
    color: var(--text-muted);
    margin-left: 6px;
  }
  h4 {
    margin: 0;
    font-size: 15px;
  }
  .hint {
    font-size: 12px;
    color: var(--text-muted);
    line-height: 1.5;
    margin: 0;
  }
  .cards {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .card {
    text-align: left;
    padding: 10px 12px;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
    display: flex;
    flex-direction: column;
    gap: 3px;
    color: var(--text-primary);
  }
  .card.selected {
    border-color: var(--accent);
  }
  .card span {
    font-size: 12px;
    color: var(--text-secondary);
  }
  .card code {
    font-size: 11px;
  }
  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .form-label {
    font-size: 12px;
    font-weight: 600;
  }
  .text-input {
    padding: 8px 10px;
    background: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
  }
  .text-input.time {
    width: 120px;
  }
  .ok {
    color: var(--success, #22c55e);
    font-size: 13px;
    margin: 4px 0;
  }
  .err {
    color: var(--danger, #ef4444);
    font-size: 13px;
    margin: 4px 0;
  }
  .artifact-list {
    margin: 8px 0;
    padding-left: 18px;
    font-size: 12px;
  }
  .artifact-list code {
    font-size: 11px;
  }
  .confirm {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }
  .nav {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 6px;
  }
  .btn-secondary {
    padding: 8px 16px;
    background: transparent;
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .btn-primary {
    padding: 8px 16px;
    background: var(--accent);
    color: white;
    border: none;
    border-radius: var(--radius-md);
    cursor: pointer;
    font-weight: 500;
  }
  .btn-primary:disabled,
  .btn-secondary:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .link-btn {
    background: none;
    border: none;
    color: var(--accent);
    cursor: pointer;
    font-size: 11px;
    padding: 0 4px;
    text-decoration: underline;
  }
  .job-log {
    font-family: ui-monospace, monospace;
    font-size: 11px;
    background: var(--bg-tertiary, #1f2937);
    border-radius: var(--radius-sm);
    padding: 8px 10px;
    max-height: 180px;
    overflow-y: auto;
  }
  .job-line.ok {
    color: var(--success, #22c55e);
  }
  .job-line.fail {
    color: var(--danger, #ef4444);
  }
  .job-line.step {
    color: var(--text-muted);
  }
</style>
