<script lang="ts">
  import { audio } from '../stores/audio.svelte';
  import { settings } from '../stores/settings.svelte';
  import { pipeline } from '../stores/pipeline.svelte';
  import { recordings } from '../stores/recordings.svelte';
  import { importAudioFile, getRecording } from '../api/recordings';
  import { checkRecordingAudioLevels } from '../api/audio';
  import { copyWithStatus } from '../utils/clipboard';
  import { clampSidebarWidth } from '../utils/resize';
  import { recordSidebar } from '../stores/recordSidebar.svelte.ts';
  import RecordingHeader from '../components/RecordingHeader.svelte';
  import ConfirmDialog from '../components/ConfirmDialog.svelte';
  import RecordingStateCards from './record/RecordingStateCards.svelte';
  import PipelineStatus from './record/PipelineStatus.svelte';
  import PatientContextSidebar from './record/PatientContextSidebar.svelte';
  import ResizeHandle from './record/ResizeHandle.svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { onMount } from 'svelte';
  import { contextTemplates } from '../stores/contextTemplates.svelte';
  import { toasts } from '../stores/toasts.svelte';
  import { rsvp } from '../stores/rsvp.svelte';
  import { formatError } from '../types/errors';
  import { buildPatientContext } from '../utils/patient_context';
  import { generateSoap } from '../api/generation';

  type Props = {
    onopenSettings?: (target: 'models' | 'audio') => void;
  };
  let { onopenSettings = () => {} }: Props = $props();

  // Patient-context text state — owned here because buildPatientContext(...) needs them at pipeline-launch time.
  let contextText = $state('');
  let medicationsText = $state('');
  let allergiesText = $state('');
  let conditionsText = $state('');

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
    pipeline.launch(recordingId, contextText || undefined, undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
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
      pipeline.launch(id, contextText || undefined, undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
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
    pipeline.retry(pipelineRecordingId, contextText || undefined, undefined, buildPatientContext(medicationsText, allergiesText, conditionsText));
  }

  // Regenerate the SOAP note using the current Patient Context, without
  // re-running transcription. Reuses the stored transcript; overwrites the
  // existing SOAP note. Lets the clinician update patient context after the
  // initial pipeline run and fold it into a fresh note.
  let regenerating = $state(false);
  async function handleRegenerateSoap() {
    const rid = pipelineRecordingId;
    if (!rid || regenerating) return;
    regenerating = true;
    try {
      const ctx = contextText.trim() || undefined;
      const pc = buildPatientContext(medicationsText, allergiesText, conditionsText);
      await generateSoap(rid, undefined, ctx, pc);
      // Re-fetch so soapNoteText (and the editor) reflect the new note.
      const rec = await getRecording(rid);
      soapNoteText = rec?.soap_note ?? null;
      await recordings.load();
    } catch (e) {
      toasts.error(`Failed to regenerate SOAP note: ${formatError(e)}`);
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
