<script lang="ts">
  import { onMount } from 'svelte';
  import { recordings, selectRecording } from '../stores/recordings.svelte';
  import { generateSoap, generateReferral, generateLetter, generatePeerDiscussion } from '../api/generation';
  import { ocrDocuments } from '../api/ocr';
  import { generation } from '../stores/generation.svelte';
  import { copyWithStatus } from '../utils/clipboard';
  import { buildPatientContext } from '../utils/patient_context';
  import GenerateControls from '../components/GenerateControls.svelte';
  import ContextPanel from '../components/ContextPanel.svelte';
  import { rsvp } from '../stores/rsvp.svelte';
  import type { DocKind } from '../stores/rsvp.svelte';
  import type { PatientContext } from '../types';
  import { formatError } from '../types/errors';
  import { OfflineCancelled } from '../api/invokeWithOfflineHandling';
  import { letterAudiences } from '../stores/letterAudiences.svelte';

  let selectedAudienceId = $state<string | null>(null);
  let letterType = $state('');
  let physicianName = $state('');
  let specialty = $state('');
  let discussionReason = $state('');

  // Load letter audiences once on mount. Previously in a $effect, which is a
  // Svelte 5 footgun: it ran once by luck (no reactive read before the async
  // call), but any future reactive read added above it would turn it into an
  // infinite fetch loop.
  onMount(() => {
    letterAudiences.list();
  });

  $effect(() => {
    if (!selectedAudienceId && letterAudiences.audiences.length > 0) {
      const patient = letterAudiences.audiences.find((a) => a.id === 'builtin-patient');
      if (patient) selectedAudienceId = patient.id;
    }
  });

  let copyStatus = $state<Record<string, 'idle' | 'copying' | 'copied'>>({});
  let contextText = $state('');
  let medicationsText = $state('');
  let allergiesText = $state('');
  let conditionsText = $state('');
  let contextExpanded = $state(false);
  let lastContextRecordingId = $state<string | null>(null);

  // OCR state: chips for each dropped document, the concatenated extracted
  // text, and a loading flag. Owned here (not in a store) because the text is
  // transient per-generation context, not persisted recording metadata.
  let ocrFiles = $state<Array<{ id: string; filename: string; status: 'done' | 'loading' | 'error'; pageCount: number }>>([]);
  let ocrText = $state('');
  let ocrLoading = $state(false);

  // Load saved context + structured fields from recording metadata only when
  // the recording ID changes. Prevents overwriting user-typed values on the
  // store-refresh that follows generation.
  $effect(() => {
    const rec = recordings.selectedRecording;
    const currentId = rec?.id ?? null;
    if (currentId === lastContextRecordingId) return;
    lastContextRecordingId = currentId;
    const meta = rec?.metadata;
    if (meta && typeof meta === 'object' && !Array.isArray(meta)) {
      contextText = typeof meta.context === 'string' ? meta.context : '';
      const pc = meta.patient_context as PatientContext | undefined;
      medicationsText = pc?.medications?.join('\n') ?? '';
      allergiesText = pc?.allergies?.join('\n') ?? '';
      conditionsText = pc?.conditions?.join('\n') ?? '';
    } else {
      contextText = '';
      medicationsText = '';
      allergiesText = '';
      conditionsText = '';
    }
  });

  // The Active badge lights up if ANY field has user input — derived state.
  const hasActiveContext = $derived(
    contextText.trim().length > 0 ||
      medicationsText.trim().length > 0 ||
      allergiesText.trim().length > 0 ||
      conditionsText.trim().length > 0,
  );

  function insertTemplate(text: string) {
    contextText = contextText ? contextText + '\n' + text : text;
    contextExpanded = true;
  }

  async function handleCopy(type: string) {
    if (copyStatus[type] && copyStatus[type] !== 'idle') return;
    if (!recordings.selectedRecording) return;
    const text = type === 'soap' ? recordings.selectedRecording.soap_note
      : type === 'referral' ? recordings.selectedRecording.referral
      : recordings.selectedRecording.letter;
    if (!text) return;
    await copyWithStatus({
      setStatus: (s) => (copyStatus = { ...copyStatus, [type]: s }),
      getText: () => text,
    });
  }

  function handleSpeedRead(type: string) {
    if (!recordings.selectedRecording) return;
    const text = type === 'soap' ? recordings.selectedRecording.soap_note
      : type === 'referral' ? recordings.selectedRecording.referral
      : recordings.selectedRecording.letter;
    if (!text) return;
    if (type === 'soap') {
      rsvp.openSoap(text);
    } else {
      rsvp.openGeneric(text, type as DocKind);
    }
  }

  async function handleOcrFilesSelected(paths: string[]) {
    if (paths.length === 0) return;
    ocrLoading = true;
    // Add loading chips immediately so the user sees feedback.
    const pendingChips = paths.map((p) => {
      const filename = p.split(/[/\\]/).pop() || p;
      return {
        id: crypto.randomUUID(),
        filename,
        status: 'loading' as const,
        pageCount: 0,
      };
    });
    ocrFiles = [...ocrFiles, ...pendingChips];

    try {
      const results = await ocrDocuments(paths);
      // Replace loading chips with done chips, matching by filename.
      ocrFiles = ocrFiles.map((f) => {
        if (f.status === 'loading') {
          const result = results.find((r) => r.filename === f.filename);
          if (result) {
            return {
              ...f,
              status: 'done' as const,
              pageCount: result.page_count,
            };
          }
          return { ...f, status: 'error' as const };
        }
        return f;
      });
      // Append extracted text, one block per file.
      const newText = results
        .map((r) => `--- ${r.filename} ---\n${r.text}`)
        .join('\n\n');
      ocrText = ocrText ? `${ocrText}\n\n${newText}` : newText;
    } catch (err) {
      ocrFiles = ocrFiles.map((f) =>
        f.status === 'loading' ? { ...f, status: 'error' as const } : f,
      );
      console.error('OCR failed:', err);
    } finally {
      ocrLoading = false;
    }
  }

  function handleOcrTextChange(text: string) {
    ocrText = text;
  }

  function handleRemoveOcrFile(id: string) {
    ocrFiles = ocrFiles.filter((f) => f.id !== id);
  }

  async function handleGenerate(type: 'soap' | 'referral' | 'letter' | 'peer_discussion') {
    if (!recordings.selectedRecording) return;
    const recordingId = recordings.selectedRecording.id;
    generation.startGenerating(type);
    // Combine notes context + OCR text into a single context string threaded
    // to every generation type. Empty/whitespace-only input yields undefined
    // so the backend treats context as absent.
    const combinedContext = [contextText.trim(), ocrText.trim()]
      .filter(Boolean)
      .join('\n\n') || undefined;
    try {
      if (type === 'soap') {
        const pc = buildPatientContext(medicationsText, allergiesText, conditionsText);
        await generateSoap(recordingId, undefined, combinedContext, pc);
      } else if (type === 'referral') {
        await generateReferral(recordingId, undefined, undefined, combinedContext);
      } else if (type === 'letter') {
        await generateLetter(recordingId, letterType || undefined, selectedAudienceId ?? undefined, combinedContext);
      } else if (type === 'peer_discussion') {
        await generatePeerDiscussion(recordingId, physicianName, specialty, discussionReason, combinedContext);
      }
      await Promise.all([
        selectRecording(recordingId),
        recordings.load(),
      ]);
      generation.finish();
    } catch (e) {
      if (e instanceof OfflineCancelled) {
        // Dialog already informed the user; restore idle state without an error banner.
        generation.finish();
        return;
      }
      generation.setError(formatError(e) || `Failed to generate ${type}`);
    }
  }
</script>

<div class="generate-tab">
  {#if !recordings.selectedRecording}
    <div class="empty-state">
      <div class="empty-icon">⚡</div>
      <h2>Generate Documentation</h2>
      <p>Select a recording from the <strong>Recordings</strong> tab first.</p>
    </div>

  {:else}
    <div class="generate-content">
      <div class="generate-header">
        <h2>Generate Documentation</h2>
        {#if recordings.selectedRecording.patient_name}
          <p class="patient">for {recordings.selectedRecording.patient_name}</p>
        {/if}
      </div>

      <!-- Context Panel -->
      <ContextPanel
        {medicationsText}
        {allergiesText}
        {conditionsText}
        {contextText}
        expanded={contextExpanded}
        {hasActiveContext}
        onToggle={() => (contextExpanded = !contextExpanded)}
        onInsertTemplate={insertTemplate}
        onClearContext={() => (contextText = '')}
        onMedicationsChange={(value) => (medicationsText = value)}
        onAllergiesChange={(value) => (allergiesText = value)}
        onConditionsChange={(value) => (conditionsText = value)}
        onContextChange={(value) => (contextText = value)}
        {ocrFiles}
        {ocrText}
        {ocrLoading}
        onOcrFilesSelected={handleOcrFilesSelected}
        onOcrTextChange={handleOcrTextChange}
        onRemoveOcrFile={handleRemoveOcrFile}
      />

      <GenerateControls
        recording={recordings.selectedRecording}
        generationState={generation.state}
        {copyStatus}
        {selectedAudienceId}
        {letterType}
        audiences={letterAudiences.audiences}
        {physicianName}
        {specialty}
        {discussionReason}
        onGenerate={handleGenerate}
        onCopy={handleCopy}
        onSpeedRead={handleSpeedRead}
        onClearError={() => generation.clearError()}
        onAudienceChange={(id) => (selectedAudienceId = id)}
        onLetterTypeChange={(type) => (letterType = type)}
        onPhysicianNameChange={(name) => (physicianName = name)}
        onSpecialtyChange={(s) => (specialty = s)}
        onDiscussionReasonChange={(reason) => (discussionReason = reason)}
      />
    </div>
  {/if}
</div>

<style>
  .generate-tab {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
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
    font-size: 48px;
    margin-bottom: 12px;
  }

  h2 {
    font-size: 20px;
    font-weight: 600;
    color: var(--text-primary);
  }

  p {
    font-size: 14px;
    color: var(--text-muted);
  }

  strong {
    color: var(--text-secondary);
  }

  .generate-content {
    flex: 1;
    overflow-y: auto;
    padding: 24px;
  }

  .generate-header {
    margin-bottom: 24px;
  }

  .generate-header h2 {
    font-size: 18px;
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: 4px;
  }

  .patient {
    font-size: 13px;
    color: var(--text-muted);
  }
</style>
