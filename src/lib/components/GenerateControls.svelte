<script lang="ts">
  import GenerateItem from './GenerateItem.svelte';
  import type { LetterAudience } from '../types/letterAudience';
  import type { Recording, GenerationProgressStats } from '../types';
  import type { GeneratingType } from '../stores/generation.svelte';
  import { generationProgressText } from '../utils/generationStats';
  import { resolveIcdCodes, billingCodesLabel } from '../icd';
  import { icd9 as icd9Store } from '../stores/icd9.svelte';
  import { settings } from '../stores/settings.svelte';

  interface Props {
    recording: Recording | null;
    generationState: {
      generating: GeneratingType;
      progressStatus: string | null;
      progress: GenerationProgressStats | null;
      error: string | null;
      lastFailedType: GeneratingType;
    };
    /** Presentation-only backend verdicts. Missing evidence is never current.
     * GenerateTab does not populate this until the backend contract exists. */
    freshness?: Partial<Record<Exclude<GeneratingType, null>, 'current' | 'stale' | 'unknown' | 'checking'>>;
    copyStatus: Record<string, 'idle' | 'copying' | 'copied'>;
    selectedAudienceId: string | null;
    letterType: string;
    audiences: LetterAudience[];
    physicianName: string;
    specialty: string;
    discussionReason: string;
    generatedSoap?: string | null;
    generatedReferral?: string | null;
    generatedLetter?: string | null;
    generatedPeerDiscussion?: string | null;
    onPhysicianNameChange: (name: string) => void;
    onSpecialtyChange: (specialty: string) => void;
    onDiscussionReasonChange: (reason: string) => void;
    onGenerate: (type: 'soap' | 'referral' | 'letter' | 'peer_discussion') => void;
    onCopy: (type: string) => void;
    onSpeedRead: (type: string) => void;
    onClearError: () => void;
    onAudienceChange: (id: string | null) => void;
    onLetterTypeChange: (type: string) => void;
  }

  const {
    recording,
    generationState,
    copyStatus,
    freshness = {},
    selectedAudienceId,
    letterType,
    audiences,
    physicianName,
    specialty,
    discussionReason,
    generatedSoap = null,
    generatedReferral = null,
    generatedLetter = null,
    generatedPeerDiscussion = null,
    onPhysicianNameChange,
    onSpecialtyChange,
    onDiscussionReasonChange,
    onGenerate,
    onCopy,
    onSpeedRead,
    onClearError,
    onAudienceChange,
    onLetterTypeChange,
  }: Props = $props();

  // The selection configures a document; output lives in the shared list below.
  let selectedType = $state<Exclude<GeneratingType, null>>('soap');
  const documents = [
    { type: 'soap', title: 'SOAP note', description: 'Structured note for this consultation' },
    { type: 'referral', title: 'Referral letter', description: 'For another clinician, based on the SOAP note' },
    { type: 'letter', title: 'Letter', description: 'For a patient, insurer, employer or court' },
    { type: 'peer_discussion', title: 'Peer discussion', description: 'Document a discussion with another physician' },
  ] as const;
  const selectedDocument = $derived(documents.find((doc) => doc.type === selectedType)!);
  const outputs = $derived({
    soap: generatedSoap ?? recording?.soap_note,
    referral: generatedReferral ?? recording?.referral,
    letter: generatedLetter ?? recording?.letter,
    peer_discussion: generatedPeerDiscussion ?? recording?.peer_discussion,
  });
  const visibleDocuments = $derived(documents.filter((doc) =>
    outputs[doc.type] || generationState.generating === doc.type ||
    (generationState.error && generationState.lastFailedType === doc.type)));

  function liveProgressText(type: Exclude<GeneratingType, null>): string | null {
    if (generationState.generating !== type) return null;
    return generationState.progress
      ? generationProgressText(generationState.progress)
      : generationState.progressStatus;
  }

  $effect(() => {
    const id = recording?.id;
    void id;
    selectedType = 'soap';
  });

  function retry() {
    // Capture before clearError mutates the reactive store.
    const type = generationState.lastFailedType;
    if (!type) return;
    onClearError();
    onGenerate(type);
  }
</script>

<section class="create-section" aria-labelledby="create-heading">
  <div class="section-heading"><h3 id="create-heading">Create document</h3><span>Configure → Generate → Review</span></div>
  <div class="soap-primary">
    <div><h4>SOAP note</h4><p>Structured note for this consultation</p></div>
    <button class="primary" onclick={() => onGenerate('soap')} disabled={generationState.generating !== null}>
      {generationState.generating === 'soap' ? 'Generating SOAP note…' : 'Generate SOAP note'}
    </button>
  </div>
  <div class="document-choice" role="group" aria-label="Other document types">
    {#each documents.filter((doc) => doc.type !== 'soap') as doc (doc.type)}
      <button aria-pressed={selectedType === doc.type} onclick={() => selectedType = doc.type}>{doc.title}</button>
    {/each}
  </div>
  {#if selectedType !== 'soap'}
    <div class="document-configuration">
      <h4>{selectedDocument.title}</h4>
      <p>{selectedDocument.description}</p>
      {#if selectedType === 'letter'}
        <div class="document-fields">
          <label for="letter-audience">Audience
            <select id="letter-audience" value={selectedAudienceId ?? ''} onchange={(e) => onAudienceChange(e.currentTarget.value || null)}>
              {#each audiences as audience (audience.id)}<option value={audience.id}>{audience.name}</option>{/each}
            </select>
          </label>
          <label for="letter-type">Purpose
            <input id="letter-type" placeholder="e.g. follow-up, pre-authorization" value={letterType} oninput={(e) => onLetterTypeChange(e.currentTarget.value)} />
          </label>
        </div>
      {:else if selectedType === 'peer_discussion'}
        <div class="document-fields">
          <label for="pd-physician">Physician Name
            <input id="pd-physician" placeholder="e.g. Dr. Jane Smith" value={physicianName} oninput={(e) => onPhysicianNameChange(e.currentTarget.value)} />
          </label>
          <label for="pd-specialty">Specialty
            <input id="pd-specialty" placeholder="e.g. Cardiology" value={specialty} oninput={(e) => onSpecialtyChange(e.currentTarget.value)} />
          </label>
          <label for="pd-reason">Reason for Discussion
            <input id="pd-reason" placeholder="Reason for discussion" value={discussionReason} oninput={(e) => onDiscussionReasonChange(e.currentTarget.value)} />
          </label>
        </div>
      {/if}
      <button class="primary" onclick={() => onGenerate(selectedType)} disabled={generationState.generating !== null}>
        {generationState.generating === selectedType ? 'Generating…' : `Generate ${selectedDocument.title.toLowerCase()}`}
      </button>
    </div>
  {/if}
</section>

<section class="output-section" aria-labelledby="output-heading">
  <div class="section-heading"><h3 id="output-heading">Recent output</h3><span>Review before use</span></div>
  {#if generationState.error}
    <div class="error-banner" role="alert">
      <span>{generationState.error}</span>
      <div class="error-actions">
        {#if generationState.lastFailedType}<button onclick={retry} disabled={generationState.generating !== null}>Retry</button>{/if}
        <button onclick={onClearError}>Dismiss</button>
      </div>
    </div>
  {/if}
  {#if visibleDocuments.length === 0}
    <p class="empty-output">No documents generated yet.</p>
  {:else}
    <div class="output-list">
      {#each visibleDocuments as doc (doc.type)}
        <GenerateItem
          title={doc.title}
          description={doc.description}
          generating={generationState.generating === doc.type}
          anyGenerating={generationState.generating !== null}
          done={!!outputs[doc.type]}
          copyStatus={copyStatus[doc.type]}
          icdCodes={doc.type === 'soap' && recording ? resolveIcdCodes(recording.metadata ?? null, recording.soap_note ?? null, icd9Store.codeSet, settings.state.icd_version, icd9Store.descriptions) : undefined}
          icdLabel={billingCodesLabel(settings.state.icd_version)}
          generatedText={outputs[doc.type]}
          freshness={freshness[doc.type] ?? 'unknown'}
          progressText={liveProgressText(doc.type)}
          failed={!!generationState.error && generationState.lastFailedType === doc.type}
          onGenerate={() => onGenerate(doc.type)}
          onCopy={() => onCopy(doc.type)}
          onSpeedRead={() => onSpeedRead(doc.type)}
        />
      {/each}
    </div>
  {/if}
</section>

<style>
  section { margin-top: 28px; min-width: 0; }
  .section-heading { display: flex; flex-wrap: wrap; align-items: baseline; justify-content: space-between; gap: 8px; border-bottom: 1px solid var(--border); padding-bottom: 12px; margin-bottom: 16px; }
  h3, h4, p { margin: 0; }
  h3 { font-size: 15px; color: var(--text-primary); }
  h4 { font-size: 15px; color: var(--text-primary); margin-bottom: 4px; }
  p, .section-heading span { font-size: 13px; color: var(--text-secondary); }
  button { min-height: 44px; padding: 10px 14px; background: var(--bg-card); color: var(--text-primary); border: 1px solid var(--border); border-radius: var(--radius-sm); cursor: pointer; }
  button:hover:not(:disabled) { border-color: var(--accent); background: var(--bg-hover); }
  button:disabled { opacity: 0.6; cursor: not-allowed; }
  button:focus-visible, input:focus-visible, select:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
  .primary { color: white; background: var(--accent); border-color: var(--accent); font-weight: 600; }
  .primary:hover:not(:disabled) { background: var(--accent-hover); }
  .soap-primary { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 20px; background: var(--accent-light); border: 1px solid var(--accent); border-radius: var(--radius-md); }
  .document-choice { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 16px; }
  .document-choice button[aria-pressed='true'] { border-color: var(--accent); background: var(--accent-light); color: var(--accent); }
  .document-configuration { margin-top: 16px; padding: 16px; border-left: 2px solid var(--border); background: var(--bg-card); }
  .document-configuration > button { margin-top: 16px; }
  .document-fields { display: flex; flex-wrap: wrap; gap: 12px; margin-top: 16px; }
  label { flex: 1 1 180px; min-width: 0; display: flex; flex-direction: column; gap: 6px; font-size: 13px; color: var(--text-secondary); }
  input, select { box-sizing: border-box; width: 100%; min-width: 0; min-height: 44px; padding: 10px; background: var(--bg-input); color: var(--text-primary); border: 1px solid var(--border); border-radius: var(--radius-sm); }
  .output-list { display: flex; flex-direction: column; gap: 12px; }
  .empty-output { padding: 20px 0; }
  .error-banner { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: center; gap: 12px; padding: 12px; margin-bottom: 16px; border: 1px solid var(--danger); border-radius: var(--radius-md); color: var(--danger); overflow-wrap: anywhere; }
  .error-actions { display: flex; flex-wrap: wrap; gap: 8px; }
  @media (max-width: 600px) { .soap-primary { align-items: stretch; flex-direction: column; } }
</style>
