<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import type { Recording } from '../types';
  import { recordings, selectRecording } from '../stores/recordings.svelte';
  import { copyToClipboard } from '../utils/clipboard';
  import RichEditor from '../components/RichEditor.svelte';
  import TranscriptView from '../components/TranscriptView.svelte';
  import { rsvp } from '../stores/rsvp.svelte';
  import type { DocKind } from '../stores/rsvp.svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { formatError } from '../types/errors';
  import { resolveIcdCodes, billingCodesLabel } from '../icd';
  import { icd9 as icd9Store } from '../stores/icd9.svelte';
  import { settings } from '../stores/settings.svelte';
  import IcdCodeList from '../components/IcdCodeList.svelte';
  import { save as saveDialog } from '@tauri-apps/plugin-dialog';
  import { exportAudio } from '../api/export';
  import { fetchAudioFromServer } from '../api/contentSync';
  import { toasts } from '../stores/toasts.svelte';

  const { tabId }: { tabId: 'transcript' | 'soap' | 'referral' | 'letter' | 'peer_discussion' } = $props();

  type TabConfig = { field: keyof Recording; label: string };

  const tabConfigs: Record<string, TabConfig> = {
    transcript:       { field: 'transcript',       label: 'Transcript' },
    soap:             { field: 'soap_note',         label: 'SOAP Note' },
    referral:         { field: 'referral',          label: 'Referral Letter' },
    letter:           { field: 'letter',            label: 'Patient Letter' },
    peer_discussion:  { field: 'peer_discussion',   label: 'Peer Discussion' },
  };

  const config = $derived(tabConfigs[tabId]);
  const content = $derived(
    recordings.selectedRecording
      ? (recordings.selectedRecording[config.field] as string | null) ?? ''
      : null
  );

  // Billing codes for the soap tab. New-format recordings carry them in
  // `metadata.icd_codes` (the note body is code-free); legacy recordings
  // fall back to mining the note text. Validation is against the BC MSP
  // ICD-9 list; codes not on the list render as amber. Each row also
  // carries its explaining title (the model-written description, with the
  // official MSP description as fallback).
  const icdCodes = $derived(
    tabId === 'soap' && content
      ? resolveIcdCodes(
          recordings.selectedRecording?.metadata ?? null,
          content,
          icd9Store.codeSet,
          settings.state.icd_version,
          icd9Store.descriptions,
        )
      : []
  );

  // Structured transcript segments from recording metadata (stored by backend
  // during transcription). Used by TranscriptView for rich speaker display.
  // Validated with a type guard so a malformed payload renders as empty
  // rather than crashing TranscriptView on an unexpected shape.
  function isTranscriptSegments(
    v: unknown,
  ): v is Array<{ speaker: string | null; text: string; start: number; end: number }> {
    return (
      Array.isArray(v) &&
      v.every(
        (seg) =>
          typeof seg === 'object' &&
          seg !== null &&
          typeof (seg as Record<string, unknown>).text === 'string' &&
          typeof (seg as Record<string, unknown>).start === 'number' &&
          typeof (seg as Record<string, unknown>).end === 'number' &&
          ((seg as Record<string, unknown>).speaker === null ||
            typeof (seg as Record<string, unknown>).speaker === 'string'),
      )
    );
  }
  const transcriptSegments = $derived.by(() => {
    const raw = recordings.selectedRecording?.metadata?.transcript_segments;
    return isTranscriptSegments(raw) ? raw : undefined;
  });

  let copyStatus = $state<'idle' | 'copying' | 'copied'>('idle');
  let saveStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');
  let saveError: string | null = $state(null);

  // Debounce timer
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let clearBadgeTimer: ReturnType<typeof setTimeout> | null = null;
  let copyBadgeTimer: ReturnType<typeof setTimeout> | null = null;
  let pendingValue: string | null = null;

  onDestroy(() => {
    // Destroy fires whenever the user switches tabs (App renders exactly one
    // EditorTab at a time) — flush the debounced edit so it is not silently
    // dropped. The optimistic store update already made it look saved, so
    // skipping this is invisible data loss.
    flushPendingEdit();
    disposed = true;
    if (saveTimer) clearTimeout(saveTimer);
    if (clearBadgeTimer) clearTimeout(clearBadgeTimer);
    if (copyBadgeTimer) clearTimeout(copyBadgeTimer);
    if (unlistenUpdate) unlistenUpdate();
  });

  // Listener for remote `recording-updated` events (content sync merge). When
  // the currently-open recording is updated on the server and the user is NOT
  // actively editing it (no pending debounced save / save in flight / failed
  // save still showing), reload it so the editor reflects the merged content
  // and surface a subtle toast. Actively-edited recordings are left alone to
  // avoid clobbering the user's in-progress edits — the next manual save
  // round-trips their version. A FAILED save also counts as dirty: the store
  // still holds the optimistic value, so a remote reload would silently
  // discard an edit that never persisted.
  let unlistenUpdate: (() => void) | null = null;

  // Race guard: tab switches unmount this component constantly; a listen()
  // resolving after unmount must unregister itself instead of leaking.
  let disposed = false;

  onMount(async () => {
    try {
      const un = await listen('recording-updated', (e) => {
        const payload = e.payload as { id: string };
        // Read the underlying $state fields live inside the callback rather
        // than capturing the $derived values (isDirty / currentRecordingId),
        // which are evaluated once at mount time and never update. Reading
        // the $state fields here gives the current values each invocation
        // (Bug C5).
        const recId = recordings.selectedRecording?.id ?? null;
        const dirty =
          pendingValue !== null || saveStatus === 'saving' || saveStatus === 'error';
        if (payload.id === recId && !dirty) {
          // Reload the recording to show the updated content. Voided — we
          // don't await here to avoid blocking the event loop.
          void selectRecording(payload.id);
          toasts.add({
            message: 'Recording updated from another machine',
            type: 'success',
            autoDismiss: true,
          });
        }
      });
      if (disposed) un();
      else unlistenUpdate = un;
    } catch (err) {
      console.error('Failed to listen for recording-updated events:', err);
    }
  });

  // Track which (recordingId, field) the current content belongs to.
  // When the user switches recordings or tabs we MUST NOT save the
  // previous tab's content under the new tab's key.
  let lastSeenKey: string | null = null;
  const currentKey = $derived(
    recordings.selectedRecording ? `${recordings.selectedRecording.id}::${String(config.field)}` : null
  );

  // Flush a pending (debounced) edit as a fire-and-forget save, keyed to the
  // (recording, field) it belongs to. Called when the key changes and from
  // onDestroy — a failure is surfaced as a toast because the optimistic local
  // update makes the edit look saved, so a silently dropped flush would be
  // invisible data loss.
  function flushPendingEdit() {
    if (pendingValue === null || saveTimer === null) return;
    clearTimeout(saveTimer);
    saveTimer = null;
    const value = pendingValue;
    const prevKey = lastSeenKey;
    pendingValue = null;
    if (!prevKey) return;
    const sep = prevKey.indexOf('::');
    if (sep <= 0) return;
    const recordingId = prevKey.slice(0, sep);
    const field = prevKey.slice(sep + 2);
    invoke('save_recording_field', { recordingId, field, value }).catch((e) => {
      console.error('Failed to flush pending edit:', e);
      toasts.error(
        `Save failed — pending ${config.label} edit may be lost (${formatError(e)})`,
      );
    });
  }

  $effect(() => {
    // Whenever the key changes (different recording or different tab),
    // reset debounce state to prevent cross-contamination.
    if (currentKey !== lastSeenKey) {
      // Flush any pending edit before switching so it isn't lost.
      flushPendingEdit();
      if (saveTimer !== null) {
        clearTimeout(saveTimer);
        saveTimer = null;
      }
      pendingValue = null;
      lastSeenKey = currentKey;
      saveStatus = 'idle';
      saveError = null;
    }
  });

  function onEditorChange(newValue: string) {
    if (!recordings.selectedRecording) return;
    // Avoid triggering saves on programmatic value binding (no actual edit).
    if (newValue === content) return;

    pendingValue = newValue;

    // Optimistic local update so the UI doesn't flicker.
    recordings.selectedRecording = {
      ...recordings.selectedRecording,
      [config.field]: newValue,
    };

    if (saveTimer !== null) clearTimeout(saveTimer);
    // Capture the identity of the edit at schedule time. Resolving
    // `recordings.selectedRecording`/`config.field` at fire time instead
    // could (if the timer ever beats the key-change $effect's flush) save
    // recording A's pending edit under recording B's id — cross-patient
    // contamination.
    const editRecordingId = recordings.selectedRecording.id;
    const editField = String(config.field);
    const editKey = currentKey;
    saveTimer = setTimeout(async () => {
      saveTimer = null;
      const value = pendingValue;
      pendingValue = null;
      if (value === null) return;
      saveStatus = 'saving';
      saveError = null;
      try {
        await invoke('save_recording_field', {
          recordingId: editRecordingId,
          field: editField,
          value,
        });
        // Scope the completion writes to the edit's context: if the user
        // switched recording/tab mid-save, the new context already reset
        // these and a stale "Saved"/error badge would mislead.
        if (editKey === currentKey) {
          saveStatus = 'saved';
          // Clear the "Saved" badge after 1.5 s.
          clearBadgeTimer = setTimeout(() => {
            clearBadgeTimer = null;
            if (saveStatus === 'saved') saveStatus = 'idle';
          }, 1500);
        }
      } catch (e) {
        if (editKey === currentKey) {
          saveStatus = 'error';
          saveError = formatError(e);
        } else {
          // The failed edit's context is gone; surface it as a toast so
          // the (silently optimistic) edit isn't invisible data loss.
          toasts.error(`Save failed — ${config.label} edit may be lost (${formatError(e)})`);
        }
      }
    }, 1000); // 1 s debounce
  }

  // Retry a failed save. The optimistic store still holds the value (the
  // backend never received it), so re-send the current editor content —
  // without this there was no path back from saveStatus === 'error' short
  // of editing again.
  async function retrySave() {
    if (!recordings.selectedRecording) return;
    const value = content;
    if (value === null) return;
    saveStatus = 'saving';
    saveError = null;
    try {
      await invoke('save_recording_field', {
        recordingId: recordings.selectedRecording.id,
        field: String(config.field),
        value,
      });
      saveStatus = 'saved';
      clearBadgeTimer = setTimeout(() => {
        clearBadgeTimer = null;
        if (saveStatus === 'saved') saveStatus = 'idle';
      }, 1500);
    } catch (e) {
      saveStatus = 'error';
      saveError = formatError(e);
    }
  }

  async function handleCopy() {
    if (copyStatus !== 'idle') return;
    if (!content) return;
    copyStatus = 'copying';
    try {
      await copyToClipboard(content);
      copyStatus = 'copied';
      copyBadgeTimer = setTimeout(() => { copyBadgeTimer = null; copyStatus = 'idle'; }, 2000);
    } catch (e) {
      console.error('Failed to copy:', e);
      copyStatus = 'idle';
    }
  }

  let exportingAudio = $state(false);

  async function handleExportAudio() {
    const rec = recordings.selectedRecording;
    if (!rec || exportingAudio) return;
    exportingAudio = true;
    try {
      const selected = await saveDialog({
        title: 'Export recording audio',
        defaultPath: `${rec.patient_name ?? rec.filename}.wav`,
        filters: [{ name: 'WAV', extensions: ['wav'] }],
      });
      if (!selected) return;
      await exportAudio(rec.id, selected);
      toasts.success('Audio exported as WAV');
    } catch (e) {
      toasts.error(formatError(e));
    } finally {
      exportingAudio = false;
    }
  }

  // On-demand audio fetch from the server (content sync archives audio on the
  // server and pulls it back only when the user wants to play/export it).
  let fetchingAudio = $state(false);

  async function handleFetchAudio() {
    const rec = recordings.selectedRecording;
    if (!rec || fetchingAudio) return;
    fetchingAudio = true;
    try {
      await fetchAudioFromServer(rec.id);
      // Reload the recording so the UI picks up the newly available audio.
      await selectRecording(rec.id);
      toasts.success('Audio fetched from server');
    } catch (e) {
      toasts.error(formatError(e));
    } finally {
      fetchingAudio = false;
    }
  }

  function handleSpeedRead() {
    if (!content) return;
    const map: Record<string, DocKind> = {
      soap_note: 'soap',
      referral: 'referral',
      letter: 'letter',
      chat: 'letter', // chat/synopsis-like documents read generically
    };
    const kind: DocKind = map[config.field] ?? 'letter';
    if (kind === 'soap') {
      rsvp.openSoap(content);
    } else {
      rsvp.openGeneric(content, kind);
    }
  }
</script>

<div class="editor-tab">
  <div class="editor-header">
    <div class="editor-header-left">
      <h2 class="doc-type">{config.label}</h2>
      {#if recordings.selectedRecording?.patient_name}
        <span class="patient-name">— {recordings.selectedRecording.patient_name}</span>
      {/if}
    </div>
    <div class="editor-header-right">
      {#if saveStatus === 'saving'}
        <span class="save-status saving">Saving…</span>
      {:else if saveStatus === 'saved'}
        <span class="save-status saved">Saved</span>
      {:else if saveStatus === 'error'}
        <span class="save-status error" title={saveError ?? undefined}>Save failed</span>
        <button class="btn-copy" onclick={() => void retrySave()}>Retry save</button>
      {/if}
      {#if content}
        <button class="btn-copy" onclick={handleSpeedRead} title="Speed Read (Cmd/Ctrl+Shift+R)">
          Speed Read
        </button>
        <button
          class="btn-copy"
          class:copied={copyStatus === 'copied'}
          onclick={handleCopy}
          disabled={copyStatus !== 'idle'}
        >
          {#if copyStatus === 'copying'}
            Copying…
          {:else if copyStatus === 'copied'}
            Copied!
          {:else}
            Copy
          {/if}
        </button>
        {#if tabId === 'transcript' && recordings.selectedRecording}
          <button
            class="btn-copy"
            onclick={handleExportAudio}
            title="Export the recording audio as a standard WAV file (decrypted, 16-bit PCM)"
          >
            Export Audio
          </button>
          {#if settings.state.sync_content}
            <button
              class="btn-copy"
              onclick={handleFetchAudio}
              disabled={fetchingAudio}
              title="Fetch this recording's audio from the server over your Tailscale connection"
            >
              {#if fetchingAudio}Fetching…{:else}Fetch Audio from Server{/if}
            </button>
          {/if}
        {/if}
        {#if icd9Store.loadError && tabId === 'soap'}
          <button
            class="icd-validation-notice"
            title="The BC MSP ICD-9 code list could not be loaded. Codes are not validated against the billing list."
            onclick={() => icd9Store.retry()}
          >
            ICD-9 validation unavailable — click to retry
          </button>
        {/if}
      {/if}
    </div>
  </div>

  {#if icdCodes.length > 0}
    <div class="icd-strip">
      <IcdCodeList codes={icdCodes} label={billingCodesLabel(settings.state.icd_version)} />
    </div>
  {/if}

  {#if content === null}
    <div class="empty-state">
      <div class="empty-icon">📄</div>
      <h3>No recording selected</h3>
      <p>Select a recording from the <strong>Recordings</strong> tab to view its {config.label.toLowerCase()}.</p>
    </div>
  {:else if content === ''}
    <div class="empty-state">
      <div class="empty-icon">✏</div>
      <h3>No {config.label} yet</h3>
      <p>Go to the <strong>Generate</strong> tab to create this document.</p>
    </div>
  {:else}
    {#if tabId === 'transcript'}
      <TranscriptView value={content} segments={transcriptSegments} placeholder="No content…" onChange={onEditorChange} />
    {:else}
      <RichEditor value={content} placeholder="No content…" onChange={onEditorChange} />
    {/if}
  {/if}
</div>

<style>
  .editor-tab {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .editor-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    background-color: var(--bg-secondary);
    flex-shrink: 0;
  }

  .editor-header-left {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .editor-header-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .doc-type {
    font-size: 15px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .patient-name {
    font-size: 13px;
    color: var(--text-muted);
  }

  .save-status {
    font-size: 12px;
    font-weight: 500;
    padding: 3px 8px;
    border-radius: var(--radius-sm);
  }

  .save-status.saving {
    color: var(--text-muted, #888);
  }

  .save-status.saved {
    color: #059669;
    background-color: color-mix(in srgb, #059669 10%, transparent);
  }

  .save-status.error {
    color: #dc2626;
    cursor: help;
  }

  .btn-copy {
    padding: 5px 12px;
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

  .empty-state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    padding: 40px;
    gap: 8px;
    color: var(--text-muted);
  }

  .empty-icon {
    font-size: 40px;
    margin-bottom: 8px;
  }

  h3 {
    font-size: 16px;
    font-weight: 500;
    color: var(--text-secondary);
  }

  p {
    font-size: 13px;
    line-height: 1.6;
  }

  strong {
    color: var(--text-secondary);
  }

  .icd-strip {
    flex-shrink: 0;
    padding: 8px 16px;
    background-color: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
  }

  .icd-validation-notice {
    font-size: 11px;
    color: var(--warning);
    background: color-mix(in srgb, var(--warning) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--warning) 30%, transparent);
    border-radius: var(--radius-sm, 4px);
    padding: 2px 8px;
    cursor: pointer;
    margin-left: 4px;
  }
  .icd-validation-notice:hover {
    background: color-mix(in srgb, var(--warning) 20%, transparent);
  }
</style>
