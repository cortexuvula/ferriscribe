<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { settings } from '../../stores/settings.svelte';
  import { getDefaultPrompt, type DocType } from '../../api/prompts';
  import { listSpecialtyPacks, type SpecialtyPackInfo } from '../../api/specialty';
  import {
    activePack,
    activeSource,
    conflictingDocTypes,
    packProvides,
    sourceLabel,
  } from '../../utils/specialtyPacks';
  import { toasts } from '../../stores/toasts.svelte';
  import { confirmDialog } from '../../stores/confirm.svelte';
  import { formatError } from '../../types/errors';

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
        { token: '{icd_candidates}', description: 'BC MSP candidate-code list (ICD-9/both modes)' },
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

  // ---- Specialty packs ----
  let packs = $state<SpecialtyPackInfo[]>([]);
  let packsLoadError = $state<string | null>(null);

  async function loadPacks() {
    try {
      packs = await listSpecialtyPacks();
      packsLoadError = null;
    } catch (e) {
      console.error('Failed to list specialty packs:', e);
      packsLoadError = formatError(e);
    }
  }

  /** The pack serving the currently selected specialty (user packs already
   *  override bundled same-id packs on the backend). */
  const selectedPack = $derived(activePack(packs, settings.state?.specialty));

  /** True when the stored specialty id no longer resolves to a usable pack
   *  (uninstalled or broken since it was saved). */
  const specialtyMissing = $derived.by(() => {
    const id = settings.state?.specialty?.trim();
    if (!id) return false;
    return !packs.some((p) => p.id === id && !p.error);
  });

  /** Value bound to the specialty <select>. The bundled family-medicine
   *  pack is not listed separately — it IS the "Default" option — so an
   *  explicit family-medicine selection displays as Default (a user pack
   *  overriding that id still lists and selects normally). */
  const specialtySelectValue = $derived.by(() => {
    const id = settings.state?.specialty;
    if (!id) return '';
    if (
      id === 'family-medicine' &&
      !packs.some((p) => p.id === 'family-medicine' && p.source === 'user')
    ) {
      return '';
    }
    return id;
  });

  const specialtyConflicts = $derived(
    selectedPack
      ? conflictingDocTypes(
          {
            soap: settings.state?.custom_soap_prompt,
            referral: settings.state?.custom_referral_prompt,
            letter: settings.state?.custom_letter_prompt,
            synopsis: settings.state?.custom_synopsis_prompt,
            peer_discussion: settings.state?.custom_peer_discussion_prompt,
          },
          selectedPack,
        )
      : [],
  );

  async function handleSpecialtyChange(event: Event) {
    const value = (event.currentTarget as HTMLSelectElement).value;
    try {
      await settings.updateField('specialty', value === '' ? null : value);
    } catch (e) {
      console.error('Failed to save specialty:', e);
      toasts.error(`Could not save the specialty: ${formatError(e)}`);
    }
  }

  function packDocLabels(packInfo: SpecialtyPackInfo): string {
    const labels: Record<DocType, string> = {
      soap: 'SOAP',
      referral: 'Referral',
      letter: 'Letter',
      synopsis: 'Synopsis',
      peer_discussion: 'Peer Discussion',
    };
    return packInfo.provided_prompts.map((d) => labels[d]).join(', ');
  }

  onDestroy(() => {
    if (promptStatusTimer) clearTimeout(promptStatusTimer);
  });

  // Monotonic generation counter: increments on every prompt switch so a
  // stale async load (or save/reset completion) can detect it no longer
  // belongs to the active prompt and skip its state writes.
  let promptLoadGen = 0;

  async function loadPromptEditor(docType: DocType) {
    const gen = ++promptLoadGen;
    promptLoading = true;
    promptDirty = false;
    promptSaveStatus = 'idle';
    try {
      const info = PROMPT_TYPES.find((p) => p.key === docType)!;
      // untrack: the reload effect must depend on activePromptKey ONLY.
      // settings.updateField replaces settings.state wholesale, so a
      // tracked read here re-ran the effect on EVERY settings save —
      // discarding in-progress prompt edits from any other pane.
      const customValue = untrack(
        () => settings.state?.[info.configField],
      ) as string | null | undefined;
      if (customValue && customValue.length > 0) {
        if (gen !== promptLoadGen) return; // user switched while loading
        promptEditorText = customValue;
        promptIsCustom = true;
      } else {
        const defaultText = await getDefaultPrompt(docType);
        if (gen !== promptLoadGen) return; // stale load — user moved on
        promptEditorText = defaultText;
        promptIsCustom = false;
      }
    } catch (e) {
      if (gen !== promptLoadGen) return;
      console.error('Failed to load prompt editor:', e);
      promptEditorText = '';
      promptIsCustom = false;
    } finally {
      if (gen === promptLoadGen) promptLoading = false;
    }
  }

  /**
   * Guard for leaving the editor with unsaved changes — shared by the
   * prompt-type switcher here and by the settings-dialog close path
   * (SettingsContent.confirmDiscardEdits → SettingsDialog). Resolves true
   * when it is safe to leave (nothing dirty, a save in flight, or the user
   * confirmed the discard).
   */
  export async function confirmDiscard(): Promise<boolean> {
    if (promptDirty && promptSaveStatus !== 'saving') {
      return confirmDialog({
        title: 'Discard prompt changes?',
        message: 'You have unsaved changes to this prompt. Discard them?',
        confirmLabel: 'Discard',
        danger: true,
      });
    }
    return true;
  }

  async function handlePromptSelect(docType: DocType) {
    if (!(await confirmDiscard())) return;
    activePromptKey = docType;
  }

  async function handlePromptSave() {
    const info = PROMPT_TYPES.find((p) => p.key === activePromptKey)!;
    const gen = promptLoadGen;
    promptSaveStatus = 'saving';
    try {
      await settings.updateField(info.configField, promptEditorText);
      // The save itself landed for `info` regardless; only the shared
      // editor flags belong to the CURRENTLY selected prompt — skip them
      // if the user switched away mid-save.
      if (gen !== promptLoadGen) return;
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
      if (gen === promptLoadGen) promptSaveStatus = 'error';
      toasts.error(`Could not save the prompt: ${formatError(e)}`);
    }
  }

  async function handlePromptReset() {
    const info = PROMPT_TYPES.find((p) => p.key === activePromptKey)!;
    const gen = promptLoadGen;
    if (
      promptIsCustom &&
      !(await confirmDialog({
        title: 'Reset prompt?',
        message: 'Clear the custom prompt and restore the default?',
        confirmLabel: 'Reset',
        danger: true,
      }))
    ) {
      return;
    }
    try {
      await settings.updateField(info.configField, null);
      const defaultText = await getDefaultPrompt(info.key);
      if (gen !== promptLoadGen) return; // user switched mid-reset
      promptEditorText = defaultText;
      promptIsCustom = false;
      promptDirty = false;
      promptSaveStatus = 'idle';
    } catch (e) {
      console.error('Failed to reset prompt:', e);
      if (gen === promptLoadGen) promptSaveStatus = 'error';
      toasts.error(`Could not reset the prompt: ${formatError(e)}`);
    }
  }

  $effect(() => {
    loadPacks();
  });

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

  <div class="specialty-section">
    <h3>Specialty</h3>
    <p class="section-description">
      Select a specialty prompt pack to tailor every generated document. A
      locked anti-fabrication safety block is always appended to pack prompts
      and cannot be altered. Packs are read from the app's local
      <code>specialties</code> folder only — nothing is downloaded.
    </p>

    <select
      class="specialty-select"
      value={specialtySelectValue}
      onchange={handleSpecialtyChange}
    >
      <option value="">Default — Family Medicine (built-in)</option>
      {#each packs.filter((p) => !p.error && !(p.id === 'family-medicine' && p.source === 'bundled')) as packInfo (packInfo.id)}
        <option value={packInfo.id}>
          {packInfo.icon ? `${packInfo.icon} ` : ''}{packInfo.name} — v{packInfo.version} ({packInfo.source})
        </option>
      {/each}
      {#if settings.state?.specialty && specialtyMissing}
        <option value={settings.state.specialty}>
          {settings.state.specialty} (missing)
        </option>
      {/if}
    </select>

    {#if packsLoadError}
      <p class="specialty-warning">
        Could not load specialty packs: {packsLoadError}
      </p>
    {:else if specialtyMissing}
      <p class="specialty-warning">
        The selected specialty pack is missing or broken — the built-in
        prompts are used until it is restored or the selection is changed.
      </p>
    {:else if selectedPack}
      <div class="specialty-details">
        <p class="specialty-description">{selectedPack.description}</p>
        <p class="specialty-meta">
          Provides prompts for: {packDocLabels(selectedPack)}.
          Document types without a pack prompt use the built-in default.
        </p>
      </div>
      {#if specialtyConflicts.length > 0}
        <p class="specialty-warning">
          Custom prompts are set for
          {specialtyConflicts.map((doc) => PROMPT_TYPES.find((pt) => pt.key === doc)?.label).join(', ')}
          — they override the {selectedPack.name} pack for those documents.
          Reset them below to use the pack.
        </p>
      {/if}
    {/if}

    {#each packs.filter((pk) => pk.error) as broken (broken.name)}
      <p class="specialty-warning">
        Pack folder “{broken.name}” could not be loaded: {broken.error}
      </p>
    {/each}
  </div>

  <div class="prompts-layout">
    <aside class="prompts-sidebar">
      {#each PROMPT_TYPES as pt (pt.key)}
        <button
          class="prompts-nav-item"
          class:active={activePromptKey === pt.key}
          aria-current={activePromptKey === pt.key ? 'true' : undefined}
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

        {@const currentCustom = settings.state?.[info?.configField ?? 'custom_soap_prompt']}
        {@const currentSource = activeSource(activePromptKey, currentCustom, selectedPack)}
        <div class="prompts-status">
          Using: <strong>{sourceLabel(currentSource, selectedPack)}</strong>
          {#if promptDirty}<span class="dirty-indicator"> (unsaved changes)</span>{/if}
        </div>
        {#if currentSource === 'custom' && packProvides(selectedPack, activePromptKey)}
          <p class="specialty-warning">
            This custom prompt overrides the {selectedPack?.name} pack for this
            document type. Reset to the default to use the pack.
          </p>
        {:else if currentSource === 'pack'}
          <p class="specialty-note">
            The {selectedPack?.name} pack prompt is active for this document
            type — the text above is the built-in default, not the pack.
            Editing and saving it creates a custom prompt that overrides the
            pack.
          </p>
        {/if}

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
      {/if}
    </div>
  </div>
</section>

<style>
  .specialty-section {
    margin-top: 1rem;
    padding: 0.75rem 1rem;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: 8px;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .specialty-section h3 {
    margin: 0;
  }

  .specialty-select {
    max-width: 380px;
    padding: 0.4rem 0.5rem;
    background: var(--bg-input);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 0.9rem;
  }

  .specialty-details {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .specialty-description {
    margin: 0;
    color: var(--text-primary);
  }

  .specialty-meta {
    margin: 0;
    font-size: 0.85rem;
    color: var(--text-secondary);
  }

  .specialty-note {
    margin: 0;
    font-size: 0.85rem;
    color: var(--text-secondary);
  }

  .specialty-warning {
    margin: 0;
    font-size: 0.85rem;
    color: var(--warning);
  }

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
