<script lang="ts">
  import { onDestroy } from 'svelte';
  import { settings } from '../../stores/settings.svelte';
  import { getDefaultPrompt, type DocType } from '../../api/prompts';

  type PromptInfo = {
    key: DocType;
    label: string;
    configField: 'custom_soap_prompt' | 'custom_referral_prompt' | 'custom_letter_prompt' | 'custom_synopsis_prompt' | 'custom_peer_discussion_prompt';
    placeholders: { token: string; description: string }[];
  };

  const PROMPT_TYPES: PromptInfo[] = [
    {
      key: 'soap',
      label: 'SOAP Note',
      configField: 'custom_soap_prompt',
      placeholders: [
        { token: '{icd_label}', description: 'ICD code header line (from ICD version setting)' },
        { token: '{icd_instruction}', description: 'Inline ICD reference phrase' },
        { token: '{template_guidance}', description: 'SOAP template hint (FollowUp, NewPatient, etc.)' },
      ],
    },
    {
      key: 'referral',
      label: 'Referral Letter',
      configField: 'custom_referral_prompt',
      placeholders: [
        { token: '{recipient_type}', description: 'e.g. Cardiologist, Orthopaedics' },
        { token: '{urgency}', description: 'routine, urgent, emergency' },
      ],
    },
    {
      key: 'letter',
      label: 'Patient Letter',
      configField: 'custom_letter_prompt',
      placeholders: [
        { token: '{letter_type}', description: 'e.g. results, instructions, follow-up' },
      ],
    },
    {
      key: 'synopsis',
      label: 'Clinical Synopsis',
      configField: 'custom_synopsis_prompt',
      placeholders: [],
    },
    {
      key: 'peer_discussion',
      label: 'Peer Discussion',
      configField: 'custom_peer_discussion_prompt',
      placeholders: [
        { token: '{physician_name}', description: 'Name of the physician being discussed with' },
        { token: '{specialty}', description: 'Physician specialty' },
        { token: '{reason}', description: 'Reason for the discussion' },
      ],
    },
  ];

  let activePromptKey = $state<DocType>('soap');
  let promptEditorText = $state<string>('');
  let promptIsCustom = $state<boolean>(false);
  let promptDirty = $state<boolean>(false);
  let promptLoading = $state<boolean>(false);
  let promptSaveStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');
  // Handle for the "saved → idle" status timer; tracked so it can be cleared
  // on unmount and on rapid re-saves instead of firing on a stale closure.
  let promptStatusTimer: ReturnType<typeof setTimeout> | null = null;

  onDestroy(() => {
    if (promptStatusTimer) clearTimeout(promptStatusTimer);
  });

  async function loadPromptEditor(docType: DocType) {
    promptLoading = true;
    promptDirty = false;
    promptSaveStatus = 'idle';
    try {
      const info = PROMPT_TYPES.find((p) => p.key === docType)!;
      const customValue = settings.state?.[info.configField] as string | null | undefined;
      if (customValue && customValue.length > 0) {
        promptEditorText = customValue;
        promptIsCustom = true;
      } else {
        promptEditorText = await getDefaultPrompt(docType);
        promptIsCustom = false;
      }
    } catch (e) {
      console.error('Failed to load prompt editor:', e);
      promptEditorText = '';
      promptIsCustom = false;
    } finally {
      promptLoading = false;
    }
  }

  async function handlePromptSelect(docType: DocType) {
    // Suppress the discard dialog while a save is in flight — the dirty flag
    // will be cleared once the save resolves, so prompting mid-save is spurious.
    if (promptDirty && promptSaveStatus !== 'saving') {
      const confirmed = confirm('You have unsaved changes. Discard them?');
      if (!confirmed) return;
    }
    activePromptKey = docType;
  }

  async function handlePromptSave() {
    const info = PROMPT_TYPES.find((p) => p.key === activePromptKey)!;
    promptSaveStatus = 'saving';
    try {
      await settings.updateField(info.configField, promptEditorText);
      promptIsCustom = true;
      promptDirty = false;
      promptSaveStatus = 'saved';
      // Clear any prior pending timer so rapid saves don't stack callbacks.
      if (promptStatusTimer) clearTimeout(promptStatusTimer);
      promptStatusTimer = setTimeout(() => {
        promptSaveStatus = 'idle';
        promptStatusTimer = null;
      }, 1500);
    } catch (e) {
      console.error('Failed to save custom prompt:', e);
      promptSaveStatus = 'error';
    }
  }

  async function handlePromptReset() {
    const info = PROMPT_TYPES.find((p) => p.key === activePromptKey)!;
    if (promptIsCustom && !confirm('Clear the custom prompt and restore the default?')) return;
    try {
      await settings.updateField(info.configField, null);
      promptEditorText = await getDefaultPrompt(activePromptKey);
      promptIsCustom = false;
      promptDirty = false;
      promptSaveStatus = 'idle';
    } catch (e) {
      console.error('Failed to reset prompt:', e);
      promptSaveStatus = 'error';
    }
  }

  $effect(() => {
    loadPromptEditor(activePromptKey);
  });
</script>

<section class="settings-section prompts-section">
  <h2>Prompts</h2>
  <p class="section-description">
    View and customize the system prompts sent to the AI for each document type.
    Placeholder tokens are substituted at generation time.
  </p>

  <div class="prompts-layout">
    <aside class="prompts-sidebar">
      {#each PROMPT_TYPES as pt (pt.key)}
        <button
          class="prompts-nav-item"
          class:active={activePromptKey === pt.key}
          onclick={() => handlePromptSelect(pt.key)}
        >
          {pt.label}
        </button>
      {/each}
    </aside>

    <div class="prompts-editor">
      {#if promptLoading}
        <div class="prompts-loading">Loading…</div>
      {:else}
        {@const info = PROMPT_TYPES.find((p) => p.key === activePromptKey)}
        <h3>{info?.label}</h3>

        <textarea
          class="prompt-textarea"
          bind:value={promptEditorText}
          oninput={() => (promptDirty = true)}
          rows="20"
          spellcheck="false"
        ></textarea>

        {#if info && info.placeholders.length > 0}
          <details class="prompts-placeholders">
            <summary>Available placeholders</summary>
            <ul>
              {#each info.placeholders as ph (ph.token)}
                <li>
                  <code>{ph.token}</code> — {ph.description}
                </li>
              {/each}
            </ul>
          </details>
        {/if}

        <div class="prompts-status">
          Using: <strong>{promptIsCustom ? 'custom' : 'default'}</strong>
          {#if promptDirty}<span class="dirty-indicator"> (unsaved changes)</span>{/if}
        </div>

        <div class="prompts-actions">
          <button
            class="btn btn-primary"
            onclick={handlePromptSave}
            disabled={!promptDirty || promptSaveStatus === 'saving'}
          >
            {promptSaveStatus === 'saving' ? 'Saving…' : promptSaveStatus === 'saved' ? 'Saved' : 'Save as custom'}
          </button>
          <button
            class="btn"
            onclick={handlePromptReset}
            disabled={!promptIsCustom && !promptDirty}
          >
            Reset to default
          </button>
        </div>
        {#if promptSaveStatus === 'error'}
          <p class="error-message">Failed to save. See console for details.</p>
        {/if}
      {/if}
    </div>
  </div>
</section>

<style>
  .prompts-layout {
    display: grid;
    grid-template-columns: 160px 1fr;
    gap: 1.25rem;
    align-items: start;
    margin-top: 1rem;
  }

  .prompts-sidebar {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    border-right: 1px solid var(--border);
    padding-right: 0.75rem;
  }

  .prompts-nav-item {
    text-align: left;
    padding: 0.5rem 0.75rem;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    color: var(--text-primary);
    cursor: pointer;
    font-size: 0.9rem;
  }

  .prompts-nav-item:hover {
    background: var(--bg-hover);
  }

  .prompts-nav-item.active {
    background: var(--accent-light);
    border-color: var(--accent);
  }

  .prompts-editor {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .prompt-textarea {
    width: 100%;
    font-family: var(--font-mono, monospace);
    font-size: 0.85rem;
    line-height: 1.4;
    padding: 0.75rem;
    background: var(--bg-input);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: 6px;
    resize: vertical;
    min-height: 400px;
  }

  .prompts-placeholders {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.5rem 0.75rem;
  }

  .prompts-placeholders summary {
    cursor: pointer;
    font-weight: 500;
  }

  .prompts-placeholders ul {
    margin: 0.5rem 0 0;
    padding-left: 1.25rem;
  }

  .prompts-placeholders code {
    background: var(--bg-code);
    padding: 0.1rem 0.3rem;
    border-radius: 3px;
    font-size: 0.85rem;
  }

  .prompts-status {
    font-size: 0.9rem;
    color: var(--text-secondary);
  }

  .prompts-status .dirty-indicator {
    color: var(--warning);
  }

  .prompts-actions {
    display: flex;
    gap: 0.5rem;
  }

  .prompts-loading {
    padding: 2rem;
    text-align: center;
    color: var(--text-secondary);
  }
</style>
