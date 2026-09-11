<script lang="ts">
  import { onMount } from 'svelte';
  import { recordings, selectRecording } from '../stores/recordings.svelte';
  import {
    generateSoap,
    generateReferral,
    generateLetter,
    generatePeerDiscussion,
    getGenerationFreshness,
    type FreshnessReport,
  } from '../api/generation';
  import { generation } from '../stores/generation.svelte';
  import { copyWithStatus } from '../utils/clipboard';
  import { buildPatientContext } from '../utils/patient_context';
  import { contextFromMetadata } from '../utils/recordingContext';
  import GenerateControls from '../components/GenerateControls.svelte';
  import ContextPanel from '../components/ContextPanel.svelte';
  import { rsvp } from '../stores/rsvp.svelte';
  import type { DocKind } from '../stores/rsvp.svelte';
  import { formatError } from '../types/errors';
  import { OfflineCancelled } from '../api/invokeWithOfflineHandling';
  import { letterAudiences } from '../stores/letterAudiences.svelte';
  import { toasts } from '../stores/toasts.svelte';
  import { useOcr } from '../composables/useOcr.svelte';
  import { settings } from '../stores/settings.svelte';
  import { playSoapCompleteChime } from '../utils/notificationSound';

  interface Props {
    onNavigateRecordings?: () => void;
  }

  const { onNavigateRecordings = () => {} }: Props = $props();

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

  // OCR state: shared composable (same logic as RecordTab). Owned here (not
  // in a store) because the text is transient per-generation context, not
  // persisted recording metadata.
  const ocr = useOcr();

  // Load saved context + structured fields from recording metadata only when
  // the recording ID changes. Prevents overwriting user-typed values on the
  // store-refresh that follows generation. Shares the same metadata→fields
  // mapping as RecordTab via contextFromMetadata.
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
    contextExpanded = Object.values(fields).some((value) => value.trim().length > 0);
    // Document-specific encounter details must not follow a different recording.
    physicianName = '';
    specialty = '';
    discussionReason = '';
    letterType = '';
    copyStatus = {};
    // Clear OCR state on recording switch — OCR text from a previous patient
    // must never leak into the next patient's generation context.
    ocr.clearOcr();
  });

  // The Active badge lights up if ANY field has user input — derived state.
  const hasActiveContext = $derived(
    contextText.trim().length > 0 ||
      medicationsText.trim().length > 0 ||
      allergiesText.trim().length > 0 ||
      conditionsText.trim().length > 0 ||
      ocr.ocrTextDisplay.trim().length > 0,
  );

  const contextSummary = $derived([
    medicationsText.trim() && 'medications', allergiesText.trim() && 'allergies',
    conditionsText.trim() && 'conditions', contextText.trim() && 'notes',
    ocr.ocrTextDisplay.trim() && 'OCR',
  ].filter(Boolean).join(', '));

  const contextCharCount = $derived(
    contextText.length + ocr.ocrTextDisplay.length +
    medicationsText.length + allergiesText.length + conditionsText.length
  );

  function insertTemplate(text: string) {
    contextText = contextText ? contextText + '\n' + text : text;
    contextExpanded = true;
  }

  async function handleCopy(type: string) {
    if (copyStatus[type] && copyStatus[type] !== 'idle') return;
    if (!recordings.selectedRecording) return;
    const text = textForType(type);
    if (!text) return;
    await copyWithStatus({
      setStatus: (s) => (copyStatus = { ...copyStatus, [type]: s }),
      getText: () => text,
    });
  }

  function handleSpeedRead(type: string) {
    if (!recordings.selectedRecording) return;
    const text = textForType(type);
    if (!text) return;
    if (type === 'soap') {
      rsvp.openSoap(text);
    } else {
      rsvp.openGeneric(text, type as DocKind);
    }
  }

  /// Resolve the generated text for a doc type. Previously inline ternary
  /// chains in handleCopy/handleSpeedRead with no `peer_discussion` branch —
  /// the type fell through to `letter`, so copying a peer discussion with no
  /// letter generated silently did nothing (button never showed "Copied"),
  /// and with a letter present it copied/speed-read the LETTER instead.
  function textForType(type: string): string | null | undefined {
    const rec = recordings.selectedRecording;
    if (!rec) return null;
    return type === 'soap' ? rec.soap_note
      : type === 'referral' ? rec.referral
      : type === 'letter' ? rec.letter
      : type === 'peer_discussion' ? rec.peer_discussion
      : null;
  }

  /// Maximum context string length. Mirrors the backend MAX_CONTEXT_CHARS.
  /// If OCR text + notes exceed this, the user must trim the preview.
  const MAX_CONTEXT_CHARS = 50_000;

  /// Freeform supporting context: notes + OCR text only (F1 — structured
  /// fields NEVER ride the freeform context; SOAP receives them
  /// exclusively via patient_context, the derived types via the Rust-side
  /// fold of the same patient_context payload).
  function freeformContext(): string | undefined {
    return [contextText.trim(), ocr.ocrTextDisplay.trim()]
      .filter(Boolean)
      .join('\n\n') || undefined;
  }

  // ── Freshness (backend-authoritative tri-state) ──────────────────────
  // The Rust command rebuilds the effective-input digest from these live
  // values + current settings and compares against provenance. The
  // frontend only renders the verdict — it never guesses, and never
  // assumes a just-succeeded generation is current.
  type FreshnessView = 'current' | 'stale' | 'unknown' | 'checking';
  let freshness: Partial<Record<'soap' | 'referral' | 'letter' | 'peer_discussion', FreshnessView>> = $state({});
  // Monotonic request revision, NOT reactive state: bumped on every input
  // change (below) and on generation completion, so any still-pending
  // freshness response from a PREVIOUS input state fails the
  // `req !== freshnessRevision` guard and is dropped in both the success
  // and error paths. An in-flight comparison computed against the
  // pre-edit inputs must never paint its verdict after the user types.
  let freshnessRevision = 0;
  // Reactive trigger the effect also depends on: bumped ONLY when a
  // generation completes, so a response computed against the
  // pre-generation row is dropped as well.
  let freshnessReq = $state(0);

  $effect(() => {
    // Reactive inputs the digest depends on (mirrors handleGenerate's
    // payload): recording identity, freeform context sources, structured
    // lists, the per-type document fields — AND the settings the backend
    // folds into every effective-input digest (model, temperature,
    // specialty, ICD version, custom prompts). Settings changes must
    // re-run this effect: without these reads, switching the AI model
    // left a stale "Current" badge and issued no new comparison.
    const rec = recordings.selectedRecording;
    const rid = rec?.id;
    const ctx = freeformContext();
    const meds = medicationsText;
    const allergies = allergiesText;
    const conditions = conditionsText;
    const lt = letterType;
    const aud = selectedAudienceId;
    const phys = physicianName;
    const spec = specialty;
    const reason = discussionReason;
    // Digest-relevant settings (must mirror EffectiveSettings in
    // src-tauri/.../freshness.rs): reading each field registers it as an
    // effect dependency; void them so lint sees intentional reads.
    const s = settings.state;
    void s.ai_model;
    void s.temperature;
    void s.icd_version;
    void s.specialty;
    void s.custom_soap_prompt;
    void s.custom_referral_prompt;
    void s.custom_letter_prompt;
    void s.custom_peer_discussion_prompt;
    if (!rid) {
      freshness = {};
      return;
    }
    // Invalidate immediately on THIS run: any still-pending response from
    // a previous input state now fails the revision guard and is dropped
    // in both the success and error paths. The verdict also resets right
    // away (not after the debounce) so a stale "Current" never lingers
    // through the debounce window while the user types.
    freshnessRevision++;
    const req = freshnessRevision;
    // Dependency ONLY: bumping freshnessReq (generation completed) must
    // re-run this effect so a fresh comparison is issued against the new
    // row. The verdict guard uses freshnessRevision, not this value.
    const genRev = freshnessReq;
    void genRev;
    {
      // Clear the previous verdict synchronously: types with stored
      // outputs show 'checking' — the badge must never display the
      // pre-edit verdict once the inputs have changed.
      const hasOutputNow = {
        soap: !!rec?.soap_note,
        referral: !!rec?.referral,
        letter: !!rec?.letter,
        peer_discussion: !!rec?.peer_discussion,
      };
      freshness = Object.fromEntries(
        (Object.keys(hasOutputNow) as (keyof typeof hasOutputNow)[])
          .filter((k) => hasOutputNow[k])
          .map((k) => [k, 'checking' as FreshnessView]),
      );
    }
    // Debounce: keystroke-level refetches would stampede the command.
    const timer = setTimeout(async () => {
      // Types with a stored output start as 'checking' — the badge must
      // not show an older verdict while a newer one is in flight.
      const hasOutput = {
        soap: !!rec?.soap_note,
        referral: !!rec?.referral,
        letter: !!rec?.letter,
        peer_discussion: !!rec?.peer_discussion,
      };
      freshness = Object.fromEntries(
        (Object.keys(hasOutput) as (keyof typeof hasOutput)[])
          .filter((k) => hasOutput[k])
          .map((k) => [k, 'checking' as FreshnessView]),
      );
      try {
        const report: FreshnessReport = await getGenerationFreshness(rid, {
          context: ctx ?? null,
          patientContext: buildPatientContext(meds, allergies, conditions) ?? null,
          template: null,
          letterType: lt || null,
          audienceId: aud ?? null,
          recipientType: null,
          urgency: null,
          physicianName: phys,
          specialty: spec,
          reason,
        });
        // Request-scoped invalidation: apply only if this is still the
        // current recording AND no newer request/generation superseded
        // this one. Otherwise drop the response entirely — a late reply
        // must never paint another recording's (or an older input
        // state's) verdicts.
        if (req !== freshnessRevision || recordings.selectedRecording?.id !== rid) return;
        freshness = {
          soap: report.soap.status === 'fresh' ? 'current' : report.soap.status,
          referral: report.referral.status === 'fresh' ? 'current' : report.referral.status,
          letter: report.letter.status === 'fresh' ? 'current' : report.letter.status,
          peer_discussion: report.peer_discussion.status === 'fresh' ? 'current' : report.peer_discussion.status,
        };
      } catch {
        if (req !== freshnessRevision || recordings.selectedRecording?.id !== rid) return;
        // A failed read must never look like a verdict — unknown.
        freshness = Object.fromEntries(
          (Object.keys(hasOutput) as (keyof typeof hasOutput)[])
            .filter((k) => hasOutput[k])
            .map((k) => [k, 'unknown' as FreshnessView]),
        );
      }
    }, 300);
    return () => clearTimeout(timer);
  });

  async function handleGenerate(type: 'soap' | 'referral' | 'letter' | 'peer_discussion') {
    if (!recordings.selectedRecording) return;
    const recordingId = recordings.selectedRecording.id;
    generation.startGenerating(type, recordingId);
    // F1/F2: the freeform context carries ONLY notes + OCR. Structured
    // fields travel via patient_context on every type; Rust folds them
    // into the prompt for the derived types (and into the digest).
    const ctx = freeformContext();
    const pc = buildPatientContext(medicationsText, allergiesText, conditionsText);

    // Guard against oversized freeform context — the backend enforces the
    // cap too (on the folded string), but failing fast here names the
    // fields the user can actually trim.
    if (ctx && ctx.length > MAX_CONTEXT_CHARS) {
      generation.setError(
        `Supporting context is ${ctx.length.toLocaleString()} characters (max ${MAX_CONTEXT_CHARS.toLocaleString()}). Please trim the OCR preview or notes.`,
      );
      return;
    }
    try {
      if (type === 'soap') {
        await generateSoap(recordingId, undefined, ctx, pc);
      } else if (type === 'referral') {
        await generateReferral(recordingId, undefined, undefined, ctx, pc);
      } else if (type === 'letter') {
        await generateLetter(recordingId, letterType || undefined, selectedAudienceId ?? undefined, ctx, pc);
      } else if (type === 'peer_discussion') {
        await generatePeerDiscussion(recordingId, physicianName, specialty, discussionReason, ctx, pc);
      }
      // Only re-select when the user hasn't moved on mid-generation —
      // an unconditional selectRecording would hijack the view back to a
      // recording they deliberately switched away from (the pipeline store
      // guards the same way).
      const stillCurrent = recordings.selectedRecording?.id === recordingId;
      await Promise.all([
        ...(stillCurrent ? [selectRecording(recordingId)] : []),
        recordings.load(),
      ]);
      generation.finish();
      // Invalidate in-flight freshness responses: they were computed
      // against the pre-generation row and must not be applied.
      freshnessReq++;
      freshnessRevision++;
      const label = type === 'soap' ? 'SOAP note' : type === 'referral' ? 'Referral letter' : type === 'letter' ? 'Letter' : 'Peer discussion note';
      toasts.success(`${label} generated`);
      if (type === 'soap' && settings.state.soap_notification_sound) {
        playSoapCompleteChime();
      }
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
  {#if !recordings.selectedRecording && recordings.loading}
    <div class="empty-state" role="status">Loading recordings…</div>
  {:else if !recordings.selectedRecording}
    <div class="empty-state">
      <h2>Generate Documentation</h2>
      <p>Select a recording from the <strong>Recordings</strong> tab first.</p>
      <button class="btn-goto-recordings" onclick={() => onNavigateRecordings()}>
        Go to Recordings
      </button>
    </div>

  {:else}
    <div class="generate-content">
      <div class="generate-header">
        <h2>Generate Documentation</h2>
        {#if recordings.selectedRecording.patient_name}
          <p class="patient">for {recordings.selectedRecording.patient_name}</p>
        {/if}
      </div>

      <p class="workspace-step">Configure</p>
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
        {contextCharCount}
        ocrFiles={ocr.ocrFiles}
        ocrText={ocr.ocrTextDisplay}
        ocrLoading={ocr.ocrLoading}
        onOcrFilesSelected={ocr.handleOcrFilesSelected}
        onOcrTextChange={ocr.handleOcrTextChange}
        onRemoveOcrFile={ocr.handleRemoveOcrFile}
      />

      {#if hasActiveContext && !contextExpanded}
        <p class="context-summary">Included: {contextSummary}</p>
      {/if}

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
        generatedSoap={recordings.selectedRecording.soap_note}
        generatedReferral={recordings.selectedRecording.referral}
        generatedLetter={recordings.selectedRecording.letter}
        generatedPeerDiscussion={recordings.selectedRecording.peer_discussion}
        {freshness}
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
    width: 100%;
    max-width: 1080px;
    box-sizing: border-box;
    margin: 0 auto;
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

  .btn-goto-recordings {
    margin-top: 12px;
    padding: 8px 20px;
    font-size: 14px;
    font-weight: 500;
    color: white;
    background-color: var(--accent);
    border: none;
    border-radius: var(--radius-md);
    cursor: pointer;
    transition: opacity 0.15s ease;
  }

  .btn-goto-recordings:hover {
    opacity: 0.9;
  }
  .workspace-step { font-size: 12px; font-weight: 600; color: var(--text-secondary); margin-bottom: 12px; }
  .context-summary { color: var(--text-secondary); font-size: 12px; margin-top: -8px; }
  .btn-goto-recordings { min-height: 44px; }
  .btn-goto-recordings:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
  @media (max-width: 600px) { .generate-content { padding: 16px; } }
</style>
