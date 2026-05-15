<script lang="ts">
  import { recordings, selectRecording } from '../stores/recordings.svelte';
  import { generateSoap, generateReferral, generateLetter } from '../api/generation';
  import { generation } from '../stores/generation.svelte';
  import { copyWithStatus } from '../utils/clipboard';
  import { buildPatientContext } from '../utils/patient_context';
  import GenerateItem from '../components/GenerateItem.svelte';
  import { rsvp } from '../stores/rsvp.svelte';
  import type { DocKind } from '../stores/rsvp.svelte';
  import type { PatientContext } from '../types';
  import { formatError } from '../types/errors';
  import { OfflineCancelled } from '../api/invokeWithOfflineHandling';

  let copyStatus = $state<Record<string, 'idle' | 'copying' | 'copied'>>({});
  let contextText = $state('');
  let medicationsText = $state('');
  let allergiesText = $state('');
  let conditionsText = $state('');
  let contextExpanded = $state(false);
  let lastContextRecordingId = $state<string | null>(null);

  const CONTEXT_TEMPLATES = [
    { label: 'Follow-up', text: 'Follow-up visit for ongoing condition. Previous visit findings:\n\n' },
    { label: 'New Patient', text: 'New patient consultation. No prior history available.\n\n' },
    { label: 'Lab Results', text: 'Recent lab results:\n- \n- \n- \n\n' },
    { label: 'Referral Info', text: 'Referred by: \nReason for referral: \nRelevant history: \n\n' },
  ];

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

  async function handleGenerate(type: 'soap' | 'referral' | 'letter') {
    if (!recordings.selectedRecording) return;
    const recordingId = recordings.selectedRecording.id;
    generation.startGenerating(type);
    try {
      if (type === 'soap') {
        const ctx = contextText.trim() || undefined;
        const pc = buildPatientContext(medicationsText, allergiesText, conditionsText);
        console.log(
          '[GenerateTab] SOAP generate —',
          'context:',
          ctx ? `${ctx.length} chars` : '(none)',
          'patient_context:',
          pc ? `meds=${pc.medications.length} allergies=${pc.allergies.length} conditions=${pc.conditions.length}` : '(none)',
        );
        await generateSoap(recordingId, undefined, ctx, pc);
      } else if (type === 'referral') {
        await generateReferral(recordingId);
      } else {
        await generateLetter(recordingId);
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
      <div class="context-panel" class:expanded={contextExpanded}>
        <button class="context-toggle" onclick={() => (contextExpanded = !contextExpanded)}>
          <span class="toggle-arrow">{contextExpanded ? '▾' : '▸'}</span>
          <span class="toggle-label">Additional Context</span>
          {#if hasActiveContext}
            <span class="context-badge">Active</span>
          {/if}
        </button>

        {#if contextExpanded}
          <div class="context-body">
            <p class="context-hint">
              Add medications, allergies, and known conditions as structured lists below. Use the Notes textarea for everything else (lab values, prior visit narrative, family/social history, etc.).
            </p>

            <label class="field-label" for="ctx-medications">Medications (one per line)</label>
            <textarea
              id="ctx-medications"
              class="context-textarea structured"
              placeholder="Lisinopril 10mg PO daily"
              bind:value={medicationsText}
              rows="3"
            ></textarea>

            <label class="field-label" for="ctx-allergies">Allergies (one per line)</label>
            <textarea
              id="ctx-allergies"
              class="context-textarea structured"
              placeholder="Penicillin (rash)"
              bind:value={allergiesText}
              rows="2"
            ></textarea>

            <label class="field-label" for="ctx-conditions">Known conditions (one per line)</label>
            <textarea
              id="ctx-conditions"
              class="context-textarea structured"
              placeholder="Type 2 diabetes"
              bind:value={conditionsText}
              rows="3"
            ></textarea>

            <label class="field-label" for="ctx-notes">Notes</label>
            <div class="context-templates">
              {#each CONTEXT_TEMPLATES as tmpl}
                <button class="template-chip" onclick={() => insertTemplate(tmpl.text)}>
                  {tmpl.label}
                </button>
              {/each}
            </div>
            <textarea
              id="ctx-notes"
              class="context-textarea"
              placeholder="Free-form notes (lab values, prior visit narrative, family/social history)..."
              bind:value={contextText}
              rows="6"
            ></textarea>
            {#if contextText.trim()}
              <button class="context-clear" onclick={() => (contextText = '')}>
                Clear notes
              </button>
            {/if}
          </div>
        {/if}
      </div>

      {#if generation.state.error}
        <div class="error-banner">
          <span>{generation.state.error}</span>
          <button class="error-dismiss" onclick={() => generation.clearError()}>Dismiss</button>
        </div>
      {/if}

      {#if generation.state.progressStatus}
        <div class="progress-banner">{generation.state.progressStatus}</div>
      {/if}

      <div class="generate-buttons">
        <GenerateItem
          title="SOAP Note"
          description="Structured clinical note (Subjective, Objective, Assessment, Plan)"
          generating={generation.state.generating === 'soap'}
          anyGenerating={generation.state.generating !== null}
          done={!!recordings.selectedRecording.soap_note}
          copyStatus={copyStatus['soap']}
          onGenerate={() => handleGenerate('soap')}
          onCopy={() => handleCopy('soap')}
          onSpeedRead={() => handleSpeedRead('soap')}
        />
        <GenerateItem
          title="Referral Letter"
          description="Specialist referral letter based on the consultation"
          generating={generation.state.generating === 'referral'}
          anyGenerating={generation.state.generating !== null}
          done={!!recordings.selectedRecording.referral}
          copyStatus={copyStatus['referral']}
          onGenerate={() => handleGenerate('referral')}
          onCopy={() => handleCopy('referral')}
          onSpeedRead={() => handleSpeedRead('referral')}
        />
        <GenerateItem
          title="Patient Letter"
          description="Patient-friendly summary of the consultation"
          generating={generation.state.generating === 'letter'}
          anyGenerating={generation.state.generating !== null}
          done={!!recordings.selectedRecording.letter}
          copyStatus={copyStatus['letter']}
          onGenerate={() => handleGenerate('letter')}
          onCopy={() => handleCopy('letter')}
          onSpeedRead={() => handleSpeedRead('letter')}
        />
      </div>
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

  /* Context Panel */
  .context-panel {
    margin-bottom: 16px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background-color: var(--bg-card);
    overflow: hidden;
  }

  .context-toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 14px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    transition: color 0.15s ease;
  }

  .context-toggle:hover {
    color: var(--text-primary);
  }

  .toggle-arrow {
    font-size: 11px;
    color: var(--text-muted);
  }

  .toggle-label {
    flex: 1;
  }

  .context-badge {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--accent);
    background-color: color-mix(in srgb, var(--accent) 15%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
    border-radius: var(--radius-sm);
    padding: 1px 6px;
  }

  .context-body {
    padding: 0 14px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .context-hint {
    font-size: 12px;
    color: var(--text-muted);
    line-height: 1.5;
    margin: 0;
  }

  .context-templates {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .template-chip {
    padding: 4px 10px;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: 12px;
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease, border-color 0.15s ease;
  }

  .template-chip:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
    border-color: var(--accent);
  }

  .context-textarea {
    width: 100%;
    resize: vertical;
    min-height: 80px;
    padding: 10px;
    font-size: 13px;
    font-family: inherit;
    line-height: 1.5;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    transition: border-color 0.15s ease;
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

  .context-textarea.structured {
    min-height: 56px;
  }

  .context-textarea::placeholder {
    color: var(--text-muted);
  }

  .context-textarea:focus {
    outline: none;
    border-color: var(--accent);
  }

  .context-clear {
    align-self: flex-end;
    padding: 4px 10px;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-muted);
    background: none;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: color 0.15s ease, border-color 0.15s ease;
  }

  .context-clear:hover {
    color: var(--danger, #ef4444);
    border-color: var(--danger, #ef4444);
  }

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
</style>
