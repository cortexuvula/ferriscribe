<script lang="ts">
  import GenerateItem from './GenerateItem.svelte';
  import type { LetterAudience } from '../types/letterAudience';
  import type { Recording } from '../types';
  import type { GeneratingType } from '../stores/generation.svelte';
  import { extractIcdCodesValidated } from '../icd';
  import { icd9 as icd9Store } from '../stores/icd9.svelte';
  import { settings } from '../stores/settings.svelte';

  interface Props {
    recording: Recording | null;
    generationState: {
      generating: GeneratingType;
      progressStatus: string | null;
      error: string | null;
    };
    copyStatus: Record<string, 'idle' | 'copying' | 'copied'>;
    selectedAudienceId: string | null;
    letterType: string;
    audiences: LetterAudience[];
    physicianName: string;
    specialty: string;
    discussionReason: string;
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
    selectedAudienceId,
    letterType,
    audiences,
    physicianName,
    specialty,
    discussionReason,
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
</script>

{#if generationState.error}
  <div class="error-banner">
    <span>{generationState.error}</span>
    <button class="error-dismiss" onclick={onClearError}>Dismiss</button>
  </div>
{/if}

{#if generationState.progressStatus}
  <div class="progress-banner">{generationState.progressStatus}</div>
{/if}

<div class="generate-sections">
  <div class="generate-group">
    <h3 class="group-heading">Clinical Notes</h3>
    <div class="generate-buttons">
      <GenerateItem
        title="SOAP Note"
        icon="🏥"
        useWhen="you need the standard visit note"
        description="Structured clinical note (Subjective, Objective, Assessment, Plan)"
        generating={generationState.generating === 'soap'}
        anyGenerating={generationState.generating !== null}
        done={!!recording?.soap_note}
        copyStatus={copyStatus['soap']}
        icdCodes={recording?.soap_note ? extractIcdCodesValidated(recording.soap_note, icd9Store.codeSet, settings.state.icd_version) : undefined}
        onGenerate={() => onGenerate('soap')}
        onCopy={() => onCopy('soap')}
        onSpeedRead={() => onSpeedRead('soap')}
      />
      <div class="letter-card">
        <div class="letter-card-header">
          <div class="letter-card-fields">
            <div class="letter-field">
              <label class="field-label" for="pd-physician">Physician Name</label>
              <input
                id="pd-physician"
                type="text"
                class="letter-input"
                placeholder="e.g. Dr. Jane Smith"
                value={physicianName}
                oninput={(e) => onPhysicianNameChange(e.currentTarget.value)}
              />
            </div>
            <div class="letter-field">
              <label class="field-label" for="pd-specialty">Specialty</label>
              <input
                id="pd-specialty"
                type="text"
                class="letter-input"
                placeholder="e.g. Cardiology"
                value={specialty}
                oninput={(e) => onSpecialtyChange(e.currentTarget.value)}
              />
            </div>
            <div class="letter-field">
              <label class="field-label" for="pd-reason">Reason for Discussion</label>
              <input
                id="pd-reason"
                type="text"
                class="letter-input"
                placeholder="e.g. Review of abnormal ECG findings"
                value={discussionReason}
                oninput={(e) => onDiscussionReasonChange(e.currentTarget.value)}
              />
            </div>
          </div>
        </div>
        <GenerateItem
          title="Peer Discussion"
          icon="👥"
          useWhen="documenting a curbside consult with another physician"
          description="Physician-to-physician discussion note"
          generating={generationState.generating === 'peer_discussion'}
          anyGenerating={generationState.generating !== null}
          done={!!recording?.peer_discussion}
          copyStatus={copyStatus['peer_discussion']}
          onGenerate={() => onGenerate('peer_discussion')}
          onCopy={() => onCopy('peer_discussion')}
          onSpeedRead={() => onSpeedRead('peer_discussion')}
        />
      </div>
    </div>
  </div>

  <div class="generate-group">
    <h3 class="group-heading">Outgoing Letters</h3>
    <div class="generate-buttons">
      <GenerateItem
        title="Referral Letter"
        icon="📨"
        useWhen="sending the patient to another clinician"
        description="Specialist referral letter based on the consultation"
        generating={generationState.generating === 'referral'}
        anyGenerating={generationState.generating !== null}
        done={!!recording?.referral}
        copyStatus={copyStatus['referral']}
        onGenerate={() => onGenerate('referral')}
        onCopy={() => onCopy('referral')}
        onSpeedRead={() => onSpeedRead('referral')}
      />
      <div class="letter-card">
        <div class="letter-card-header">
          <div class="letter-card-fields">
            <div class="letter-field">
              <label class="field-label" for="letter-audience">Audience</label>
              <select
                id="letter-audience"
                class="letter-select"
                value={selectedAudienceId ?? ''}
                onchange={(e) => onAudienceChange(e.currentTarget.value || null)}
              >
                {#each audiences as audience}
                  <option value={audience.id}>{audience.name}</option>
                {/each}
              </select>
            </div>
            <div class="letter-field">
              <label class="field-label" for="letter-type">Purpose</label>
              <input
                id="letter-type"
                type="text"
                class="letter-input"
                placeholder="e.g. follow-up, pre-authorization"
                value={letterType}
                oninput={(e) => onLetterTypeChange(e.currentTarget.value)}
              />
            </div>
          </div>
        </div>
        <GenerateItem
          title="Letter"
          icon="✉️"
          useWhen="writing to a patient, insurer, employer, or court"
          description={selectedAudienceId
            ? (() => {
                const a = audiences.find((x) => x.id === selectedAudienceId);
                return a ? `Letter for ${a.name}` : 'Letter';
              })()
            : 'Letter'}
          generating={generationState.generating === 'letter'}
          anyGenerating={generationState.generating !== null}
          done={!!recording?.letter}
          copyStatus={copyStatus['letter']}
          onGenerate={() => onGenerate('letter')}
          onCopy={() => onCopy('letter')}
          onSpeedRead={() => onSpeedRead('letter')}
        />
      </div>
    </div>
  </div>
</div>

<style>
  .error-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    margin-bottom: 16px;
    background-color: rgba(239, 68, 68, 0.1);
    border: 1px solid var(--danger, #ef4444);
    border-radius: var(--radius-md);
    font-size: 13px;
    color: var(--danger, #ef4444);
  }

  .error-dismiss {
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    font-size: 12px;
    color: var(--danger, #ef4444);
    border: 1px solid var(--danger, #ef4444);
    background: transparent;
    cursor: pointer;
  }

  .error-dismiss:hover {
    background-color: var(--danger, #ef4444);
    color: white;
  }

  .progress-banner {
    padding: 8px 12px;
    margin-bottom: 16px;
    background-color: rgba(59, 130, 246, 0.1);
    border: 1px solid var(--accent, #3b82f6);
    border-radius: var(--radius-md);
    font-size: 13px;
    color: var(--accent, #3b82f6);
  }

  .generate-sections {
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .generate-group {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .group-heading {
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-secondary);
    margin: 0 0 2px 0;
    padding-bottom: 4px;
    border-bottom: 1px solid var(--border-light);
  }

  .generate-buttons {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .letter-card {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background-color: var(--bg-card);
    overflow: hidden;
  }

  .letter-card-header {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--border-light);
  }

  .letter-card-fields {
    display: flex;
    gap: 12px;
    flex: 1;
    min-width: 0;
  }

  .letter-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
    min-width: 0;
  }

  .field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .letter-select,
  .letter-input {
    width: 100%;
    height: 32px;
    padding: 0 10px;
    font-size: 13px;
    font-family: inherit;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    transition: border-color 0.15s ease;
    box-sizing: border-box;
  }

  .letter-select {
    cursor: pointer;
  }

  .letter-input::placeholder {
    color: var(--text-muted);
  }

  .letter-select:focus,
  .letter-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .letter-card > :global(.generate-item) {
    border: none;
    border-radius: 0;
  }
</style>
