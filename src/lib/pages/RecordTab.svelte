<script lang="ts">
  import { audio } from '../stores/audio.svelte';
  import { settings } from '../stores/settings.svelte';
  import { pipeline } from '../stores/pipeline.svelte';
  import { recordings } from '../stores/recordings.svelte';
  import { importAudioFile, getRecording } from '../api/recordings';
  import { checkRecordingAudioLevels } from '../api/audio';
  import { copyWithStatus } from '../utils/clipboard';
  import { clampSidebarWidth } from '../utils/resize';
  import { recordSidebar } from '../stores/recordSidebar.svelte';
  import RecordingHeader from '../components/RecordingHeader.svelte';
  import ConfirmDialog from '../components/ConfirmDialog.svelte';
  import RecordingStateCards from './record/RecordingStateCards.svelte';
  import PipelineStatus from './record/PipelineStatus.svelte';
  import PatientContextSidebar from './record/PatientContextSidebar.svelte';
  import ResizeHandle from './record/ResizeHandle.svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { onMount, onDestroy } from 'svelte';
  import { contextTemplates } from '../stores/contextTemplates.svelte';
  import { toasts } from '../stores/toasts.svelte';
  import { playSoapCompleteChime } from '../utils/notificationSound';
  import { rsvp } from '../stores/rsvp.svelte';
  import { formatError } from '../types/errors';
  import { buildPatientContext } from '../utils/patient_context';
  import { contextFromMetadata } from '../utils/recordingContext';
  import { generateSoap } from '../api/generation';
  import { generation } from '../stores/generation.svelte';
  import { OfflineCancelled } from '../api/invokeWithOfflineHandling';
  import { useOcr } from '../composables/useOcr.svelte';

  type Props = {
    onopenSettings?: (target: 'models' | 'audio') => void;
  };
  const { onopenSettings = () => {} }: Props = $props();

  // Patient-context text state — owned here because buildPatientContext(...) needs them at pipeline-launch time.
  let contextText = $state('');
  let medicationsText = $state('');
  let allergiesText = $state('');
  let conditionsText = $state('');

  // OCR state: shared composable (same logic as GenerateTab). Transient —
  // cleared on new recording via ocr.clearOcr().
  const ocr = useOcr();

  /// Maximum context string length. Mirrors the backend MAX_CONTEXT_CHARS.
  const MAX_CONTEXT_CHARS = 50_000;

  // Clear OCR state when the active recording changes to prevent
  // cross-patient PHI leakage (same guard as GenerateTab).
  let lastOcrRecordingId: string | null = null;
  $effect(() => {
    const id = pipelineRecordingId;
    if (id !== lastOcrRecordingId && lastOcrRecordingId !== null) {
      ocr.clearOcr();
    }
    lastOcrRecordingId = id;
  });

  // Repopulate context fields from recording history when the user selects a
  // different recording in the list — parity with GenerateTab. Without this,
  // the Record tab's context fields were ephemeral (never loaded from saved
  // metadata), so switching patients left the previous patient's meds visible
  // and the upload-wipe bug had no self-healing path. The id-guard prevents
  // the store refresh that follows generation from clobbering user edits.
  let lastContextRecordingId: string | null = null;
  $effect(() => {
    const rec = recordings.selectedRecording;
    const currentId = rec?.id ?? null;
    if (currentId === lastContextRecordingId) return;
    lastContextRecordingId = currentId;
    const fields = contextFromMetadata(rec?.metadata);
    contextText = fields.contextText;
    medicationsText = fields.medicationsText;
    allergiesText = fields.allergiesText;
    conditionsText = fields.conditionsText;
  });

  // Sidebar UI state — synced with the persisted recordSidebar store.
  let sidebarOpen = $state(true);
  let sidebarWidth = $state(360);

  // Sync local state from the persisted recordSidebar rune store.
  $effect(() => {
    sidebarOpen = recordSidebar.open;
    sidebarWidth = recordSidebar.width;
  });

  function toggleSidebar() {
    recordSidebar.setOpen(!sidebarOpen);
  }

  function onSidebarResize(delta: number) {
    // Negative delta (drag handle left) = sidebar widens. The handle sits
    // to the LEFT of the sidebar, so dragging right narrows it.
    const next = clampSidebarWidth(
      sidebarWidth - delta,
      window.innerWidth,
      recordSidebar.MIN_WIDTH,
      recordSidebar.MAX_WIDTH,
      320,
    );
    sidebarWidth = next;
  }

  function onSidebarResizeEnd() {
    recordSidebar.setWidth(sidebarWidth);
  }

  // Re-clamp the sidebar width when the window resizes so the main area
  // always retains at least 320px. Persisted width stays untouched.
  $effect(() => {
    function handler() {
      const next = clampSidebarWidth(
        sidebarWidth,
        window.innerWidth,
        recordSidebar.MIN_WIDTH,
        recordSidebar.MAX_WIDTH,
        320,
      );
      if (next !== sidebarWidth) {
        sidebarWidth = next;
      }
    }
    window.addEventListener('resize', handler);
    return () => window.removeEventListener('resize', handler);
  });

  onMount(() => {
    contextTemplates.load();
    window.addEventListener('keydown', handleGlobalKeydown);
  });

  onDestroy(() => {
    window.removeEventListener('keydown', handleGlobalKeydown);
  });

  // Import flow state
  let importedRecordingId = $state<string | null>(null);
  let importedFilename = $state<string | null>(null);
  let importing = $state(false);
  let importError = $state<string | null>(null);

  // Track the recording ID the current pipeline status refers to
  let pipelineRecordingId = $state<string | null>(null);

  // SOAP note text for ICD code extraction — fetched when pipeline completes
  let soapNoteText = $state<string | null>(null);

  // Fetch SOAP note when pipeline completes
  $effect(() => {
    const current = pipeline.state.current;
    if (current?.stage === 'completed' && pipelineRecordingId) {
      getRecording(pipelineRecordingId).then((rec) => {
        soapNoteText = rec?.soap_note ?? null;
      }).catch(() => {
        soapNoteText = null;
      });
    } else {
      soapNoteText = null;
    }
  });

  // Silent-recording warning dialog state
  let silenceDialogOpen = $state(false);
  let silenceDialogRecordingId = $state<string | null>(null);
  let silenceDialogMessage = $state('');

  function clearAllContextFields() {
    // Both the freeform "Notes" box and the structured Patient Context
    // fields (medications / allergies / conditions) are tied to the
    // current encounter — fresh encounter, fresh form.
    contextText = '';
    medicationsText = '';
    allergiesText = '';
    conditionsText = '';
    ocr.clearOcr();
  }

  /** Combine notes + OCR text into the pipeline context string. */
  function buildPipelineContext(): string | undefined {
    const combined = [contextText.trim(), ocr.ocrTextDisplay.trim()]
      .filter(Boolean)
      .join('\n\n') || undefined;
    if (combined && combined.length > MAX_CONTEXT_CHARS) {
      toasts.error(
        `Supporting context is ${combined.length.toLocaleString()} characters (max ${MAX_CONTEXT_CHARS.toLocaleString()}). Please trim the OCR preview or notes.`,
      );
      return undefined;
    }
    return combined;
  }

  /** Wait for in-flight OCR to finish (up to 60s) so its text is included
   *  in the pipeline context. Prevents the race where recording stops while
   *  OCR chips are still in 'loading' status. Shows a toast so the user
   *  knows why the pipeline hasn't started yet. */
  async function waitForOcrSettled(): Promise<void> {
    if (!ocr.ocrLoading) return;
    toasts.add({
      message: 'Waiting for document OCR to complete before processing…',
      type: 'success',
      autoDismiss: true,
    });
    const deadline = Date.now() + 60_000;
    while (ocr.ocrLoading && Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, 200));
    }
  }

  function handleStartRecording() {
    clearAllContextFields();
    importedRecordingId = null;
    importedFilename = null;
    importError = null;
    pipeline.clearCurrent();
    audio.startRecording();
  }

  function handleNewRecording() {
    clearAllContextFields();
    importedRecordingId = null;
    importedFilename = null;
    importError = null;
    pipeline.clearCurrent();
    audio.reset();
  }

  function describeSilence(rms: number): string {
    const rmsDb = rms > 0 ? 20 * Math.log10(rms) : -Infinity;
    const formatted = isFinite(rmsDb) ? `${rmsDb.toFixed(1)} dBFS` : 'digital silence';
    return (
      `The recording appears to contain no audio (${formatted}). ` +
      "Your microphone or audio routing likely isn't capturing sound — " +
      'processing this file will probably produce an unreliable transcript.'
    );
  }

  async function maybeLaunchPipeline(recordingId: string) {
    // Wait for any in-flight OCR to finish so its text is included.
    await waitForOcrSettled();
    try {
      const levels = await checkRecordingAudioLevels(recordingId);
      if (levels.is_silent) {
        silenceDialogRecordingId = recordingId;
        silenceDialogMessage = describeSilence(levels.rms);
        silenceDialogOpen = true;
        return;
      }
    } catch (_e) {
      // If the silence check itself fails, don't block the pipeline.
    }
    pipeline.launch(recordingId, buildPipelineContext(), undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
  }

  async function warnIfSilent(recordingId: string) {
    try {
      const levels = await checkRecordingAudioLevels(recordingId);
      if (levels.is_silent) {
        silenceDialogRecordingId = recordingId;
        silenceDialogMessage = describeSilence(levels.rms);
        silenceDialogOpen = true;
      }
    } catch (_e) {
      // Silent failure is fine — this is advisory only.
    }
  }

  function confirmSilentProcess() {
    const id = silenceDialogRecordingId;
    silenceDialogOpen = false;
    silenceDialogRecordingId = null;
    if (id) {
      pipelineRecordingId = id;
      pipeline.launch(id, buildPipelineContext(), undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
    }
  }

  function dismissSilenceDialog() {
    silenceDialogOpen = false;
    silenceDialogRecordingId = null;
  }

  function handleStopRecording() {
    audio.stop().then(() => {
      const recordingId = audio.state.lastRecordingId;
      if (!recordingId) return;

      pipelineRecordingId = recordingId;

      if (settings.state.auto_generate_soap) {
        maybeLaunchPipeline(recordingId);
      } else {
        warnIfSilent(recordingId);
      }
    });
  }

  function handleProcessRecording() {
    const recordingId = audio.state.lastRecordingId ?? importedRecordingId;
    if (!recordingId) return;
    pipelineRecordingId = recordingId;
    maybeLaunchPipeline(recordingId);
  }

  function handleRetry() {
    if (!pipelineRecordingId) return;
    pipeline.retry(pipelineRecordingId, buildPipelineContext(), undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
  }

  // Regenerate the SOAP note using the current Patient Context, without
  // re-running transcription. Reuses the stored transcript; overwrites the
  // existing SOAP note. Lets the clinician update patient context after the
  // initial pipeline run and fold it into a fresh note.
  let regenerating = $state(false);
  async function handleRegenerateSoap() {
    const rid = pipelineRecordingId;
    if (!rid || regenerating || generation.state.generating) return;
    regenerating = true;
    generation.startGenerating('soap');
    try {
      const ctx = buildPipelineContext();
      const pc = buildPatientContext(medicationsText, allergiesText, conditionsText);
      await generateSoap(rid, undefined, ctx, pc);
      // Re-fetch so soapNoteText (and the editor) reflect the new note.
      const rec = await getRecording(rid);
      soapNoteText = rec?.soap_note ?? null;
      await recordings.load();
      generation.finish();
      toasts.success('SOAP note generated');
      if (settings.state.soap_notification_sound) {
        playSoapCompleteChime();
      }
    } catch (e) {
      if (e instanceof OfflineCancelled) {
        generation.finish();
        return;
      }
      generation.setError(formatError(e) || 'Failed to regenerate SOAP note');
    } finally {
      regenerating = false;
    }
  }

  function handleCancelPipeline() {
    if (!pipelineRecordingId) return;
    pipeline.cancel(pipelineRecordingId);
  }

  async function handleUploadAudio() {
    importError = null;
    try {
      // Do NOT clear context here — the user typed these meds/conditions FOR
      // this uploaded encounter. Clearing was a bug (handleUploadAudio ran
      // before the file picker, wiping fields that maybeLaunchPipeline then
      // read as empty → SOAP got no patient_context). A fresh recording has
      // empty metadata, so the selectedRecording effect below leaves the
      // current fields in place for the current encounter.
      const selected = await open({
        multiple: false,
        filters: [
          { name: 'Audio Files', extensions: ['wav', 'mp3', 'ogg', 'flac', 'm4a', 'aac', 'wma', 'webm'] },
        ],
      });
      if (!selected) return;

      importing = true;
      const filePath = typeof selected === 'string' ? selected : selected;
      const recordingId = await importAudioFile(filePath);
      importedRecordingId = recordingId;
      importedFilename = filePath.split('/').pop()?.split('\\').pop() ?? 'audio file';
      await recordings.load();

      // Always launch — upload doesn't respect settings.state.auto_generate_soap (live recording still does).
      pipelineRecordingId = recordingId;
      maybeLaunchPipeline(recordingId);
    } catch (e) {
      importError = formatError(e) || 'Import failed';
    } finally {
      importing = false;
    }
  }

  let copyStatus = $state<'idle' | 'copying' | 'copied'>('idle');

  async function handleCopySoap() {
    if (copyStatus !== 'idle') return;
    const rid = pipelineRecordingId;
    if (!rid) return;
    await copyWithStatus({
      setStatus: (s) => (copyStatus = s),
      getText: async () => {
        const rec = await getRecording(rid);
        return rec?.soap_note ?? undefined;
      },
      onError: (e) => toasts.error(`Failed to copy SOAP note: ${e}`),
    });
  }

  async function handleSpeedRead() {
    const rid = pipelineRecordingId;
    if (!rid) return;
    try {
      const rec = await getRecording(rid);
      if (rec?.soap_note) {
        rsvp.openSoap(rec.soap_note);
      } else {
        toasts.error('No SOAP note to read yet.');
      }
    } catch (e) {
      console.error('Failed to open speed reader:', e);
      toasts.error(`Failed to open speed reader: ${e}`);
    }
  }

  // Global keyboard shortcuts for the Record tab.
  //   Space      — toggle record/stop
  //   Cmd+Enter  — generate (regenerate) SOAP for the selected recording
  // Both bail out when the user is typing in an input, textarea, or
  // contenteditable element so the shortcuts don't hijack normal text entry.
  function handleGlobalKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement | null;
    if (
      target &&
      (target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.isContentEditable)
    ) {
      return;
    }

    // Space — toggle record/stop.
    // recording -> stop; idle/stopped/paused -> start a fresh recording.
    if (e.code === 'Space') {
      e.preventDefault();
      if (audio.state.state === 'recording') {
        handleStopRecording();
      } else {
        handleStartRecording();
      }
      return;
    }

    // Cmd+Enter (Mac) or Ctrl+Enter — regenerate the SOAP note for the
    // selected recording. handleRegenerateSoap already no-ops when there's
    // no recording or a regeneration is in flight.
    if ((e.metaKey || e.ctrlKey) && e.code === 'Enter') {
      e.preventDefault();
      handleRegenerateSoap();
    }
  }
</script>

<div class="record-tab">
  <RecordingHeader
    {onopenSettings}
    onStart={handleStartRecording}
    onStop={handleStopRecording}
    onNewRecording={handleNewRecording}
  />

  <div class="record-body">
    <div class="record-main">
      {#if pipeline.state.current && pipelineRecordingId}
        <PipelineStatus
          bind:copyStatus
          soapNoteText={soapNoteText}
          {regenerating}
          onCancel={handleCancelPipeline}
          onRetry={handleRetry}
          onCopySoap={handleCopySoap}
          onSpeedRead={handleSpeedRead}
          onRegenerate={handleRegenerateSoap}
        />
      {:else}
        <RecordingStateCards
          {importedRecordingId}
          {importedFilename}
          {importing}
          {importError}
          onProcessRecording={handleProcessRecording}
          onUploadAudio={handleUploadAudio}
        />
      {/if}
    </div>

    {#if sidebarOpen}
      <ResizeHandle onResize={onSidebarResize} onResizeEnd={onSidebarResizeEnd} />
    {/if}

    <PatientContextSidebar
      bind:contextText
      bind:medicationsText
      bind:allergiesText
      bind:conditionsText
      open={sidebarOpen}
      width={sidebarWidth}
      onToggle={toggleSidebar}
      ocrFiles={ocr.ocrFiles}
      ocrText={ocr.ocrTextDisplay}
      ocrLoading={ocr.ocrLoading}
      onOcrFilesSelected={ocr.handleOcrFilesSelected}
      onOcrTextChange={ocr.handleOcrTextChange}
      onRemoveOcrFile={ocr.handleRemoveOcrFile}
    />
  </div>
</div>

<ConfirmDialog
  open={silenceDialogOpen}
  title="Silent recording detected"
  message={silenceDialogMessage}
  confirmLabel="Process anyway"
  cancelLabel="Cancel"
  danger
  onConfirm={confirmSilentProcess}
  onCancel={dismissSilenceDialog}
/>

<style>
  .record-tab {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .record-body {
    flex: 1;
    display: flex;
    min-height: 0;
    overflow: hidden;
  }

  .record-main {
    flex: 1;
    min-width: 320px;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 32px;
    overflow: auto;
  }
</style>
