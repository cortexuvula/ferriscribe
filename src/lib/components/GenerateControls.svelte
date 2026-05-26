<script lang="ts">
  import GenerateItem from './GenerateItem.svelte';
  import type { LetterAudience } from '../types/letterAudience';
  import type { Recording } from '../types';
  import type { GeneratingType } from '../stores/generation.svelte';

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
    onGenerate: (type: 'soap' | 'referral' | 'letter') => void;
    onCopy: (type: string) => void;
    onSpeedRead: (type: string) => void;
    onClearError: () => void;
    onAudienceChange: (id: string | null) => void;
    onLetterTypeChange: (type: string) => void;
  }

  let {
    recording,
    generationState,
    copyStatus,
    selectedAudienceId,
    letterType,
    audiences,
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

<div class="generate-buttons">
  <GenerateItem
    title="SOAP Note"
    description="Structured clinical note (Subjective, Objective, Assessment, Plan)"
    generating={generationState.generating === 'soap'}
    anyGenerating={generationState.generating !== null}
    done={!!recording?.soap_note}
    copyStatus={copyStatus['soap']}
    onGenerate={() => onGenerate('soap')}
    onCopy={() => onCopy('soap')}
    onSpeedRead={() => onSpeedRead('soap')}
  />
  <GenerateItem
    title="Referral Letter"
    description="Specialist referral letter based on the consultation"
    generating={generationState.generating === 'referral'}
    anyGenerating={generationState.generating !== null}
    done={!!recording?.referral}
    copyStatus={copyStatus['referral']}
    onGenerate={() => onGenerate('referral')}
    onCopy={() => onCopy('referral')}
    onSpeedRead={() => onSpeedRead('referral')}
  />
  <div class="letter-controls">
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

    <label class="field-label" for="letter-type">Letter purpose</label>
    <input
      id="letter-type"
      type="text"
      class="letter-input"
      placeholder="e.g. follow-up, pre-authorization"
      value={letterType}
      oninput={(e) => onLetterTypeChange(e.currentTarget.value)}
    />
  </div>
  <GenerateItem
    title="Letter"
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

  .generate-buttons {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .letter-controls {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background-color: var(--bg-card);
  }

  .field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-top: 4px;
    margin-bottom: -4px;
  }

  .letter-select,
  .letter-input {
    width: 100%;
    height: 34px;
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
    appearance: auto;
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
</style>
