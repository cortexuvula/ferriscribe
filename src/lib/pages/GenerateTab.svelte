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

  // OCR state: chips for each dropped document, the extracted text per file,
  // and a loading flag. Owned here (not in a store) because the text is
  // transient per-generation context, not persisted recording metadata.
  // Each chip carries its own text block so removal cleans up properly.
  let ocrFiles = $state<Array<{ id: string; filename: string; status: 'done' | 'loading' | 'error'; pageCount: number; text: string }>>([]);
  let ocrLoading = $state(false);

  /// Derived: concatenation of all done-file text blocks. The user edits this
  /// in the preview textarea, but we rebuild from chips on removal. Using a
  /// derived value ensures consistency.
  let ocrText = $derived(
    ocrFiles
      .filter((f) => f.status === 'done' && f.text)
      .map((f) => `--- ${f.filename} ---\n${f.text}`)
      .join('\n\n'),
  );

  /// Mutable copy of the derived text — the user can edit the preview, which
  /// overrides the derived value until a file is added/removed.
  let ocrTextOverride = $state<string | null>(null);
  let ocrTextDisplay = $derived(ocrTextOverride ?? ocrText);

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
    // Clear OCR state on recording switch — OCR text from a previous patient
    // must never leak into the next patient's generation context.
    ocrFiles = [];
    ocrTextOverride = null;
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
    ocrTextOverride = null; // reset manual edits when new files arrive
    // Add loading chips immediately so the user sees feedback. Track the chip
    // IDs created by THIS invocation so concurrent drops don't interfere.
    const chipIds: string[] = [];
    const pendingChips = paths.map((p) => {
      const id = crypto.randomUUID();
      chipIds.push(id);
      const filename = p.split(/[/\\]/).pop() || p;
      return {
        id,
        filename,
        path: p,
        status: 'loading' as const,
        pageCount: 0,
        text: '',
      };
    });
    ocrFiles = [...ocrFiles, ...pendingChips];
    const idSet = new Set(chipIds);

    try {
      const results = await ocrDocuments(paths);
      // Match results to THIS invocation's chips by filename within our batch.
      ocrFiles = ocrFiles.map((f) => {
        if (!idSet.has(f.id)) return f; // not our chip
        const result = results.find((r) => r.filename === f.filename);
        if (result) {
          return {
            ...f,
            status: 'done' as const,
            pageCount: result.page_count,
            text: result.text,
          };
        }
        return { ...f, status: 'error' as const };
      });
    } catch (e) {
      if (e instanceof OfflineCancelled) {
        // Dialog already informed the user; mark chips as error.
        ocrFiles = ocrFiles.map((f) =>
          idSet.has(f.id) ? { ...f, status: 'error' as const } : f,
        );
      } else {
        ocrFiles = ocrFiles.map((f) =>
          idSet.has(f.id) ? { ...f, status: 'error' as const } : f,
        );
        console.error('OCR failed:', e);
      }
    } finally {
      // Only clear the loading flag if no other batch is in flight.
      ocrLoading = ocrFiles.some((f) => f.status === 'loading');
    }
  }

  function handleOcrTextChange(text: string) {
    ocrTextOverride = text;
  }

  function handleRemoveOcrFile(id: string) {
    ocrFiles = ocrFiles.filter((f) => f.id !== id);
    ocrTextOverride = null; // rebuild from remaining chips
  }

  /// Maximum context string length. Mirrors the backend MAX_CONTEXT_CHARS.
  /// If OCR text + notes exceed this, the user must trim the preview.
  const MAX_CONTEXT_CHARS = 50_000;

  /** Format medications/allergies/conditions as context text for non-SOAP docs. */
  function formatStructuredContext(): string {
    const parts: string[] = [];
    if (medicationsText.trim()) {
      parts.push(`Medications:\n${medicationsText.trim()}`);
    }
    if (allergiesText.trim()) {
      parts.push(`Allergies:\n${allergiesText.trim()}`);
    }
    if (conditionsText.trim()) {
      parts.push(`Known conditions:\n${conditionsText.trim()}`);
    }
    return parts.join('\n\n');
  }

  async function handleGenerate(type: 'soap' | 'referral' | 'letter' | 'peer_discussion') {
    if (!recordings.selectedRecording) return;
    const recordingId = recordings.selectedRecording.id;
    generation.startGenerating(type);
    // Combine structured patient context + notes context + OCR text into a
    // single context string threaded to every generation type. SOAP already
    // passes the structured fields via buildPatientContext, but including them
    // here too is harmless redundancy. Empty/whitespace-only input yields
    // undefined so the backend treats context as absent.
    const combinedContext = [formatStructuredContext(), contextText.trim(), ocrTextDisplay.trim()]
      .filter(Boolean)
      .join('\n\n') || undefined;

    // Guard against oversized context — the backend enforces this for SOAP,
    // but letter/referral/peer-discussion don't have the check yet.
    if (combinedContext && combinedContext.length > MAX_CONTEXT_CHARS) {
      generation.setError(
        `Supporting context is ${combinedContext.length.toLocaleString()} characters (max ${MAX_CONTEXT_CHARS.toLocaleString()}). Please trim the OCR preview or notes.`,
      );
      return;
    }
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
        ocrText={ocrTextDisplay}
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
