<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { translation, decideLanguageChange } from '../stores/translation.svelte';
  import { settings } from '../stores/settings.svelte';
  import { audio } from '../stores/audio.svelte';
  import { toasts } from '../stores/toasts.svelte';
  import ConfirmDialog from '../components/ConfirmDialog.svelte';
  import { copyToClipboard } from '../utils/clipboard';
  import { formatMinsSecs, formatTimestamp } from '../utils/format';

  let messagesEl: HTMLDivElement | undefined = $state();
  let userNearBottom = $state(true);
  let typedInput = $state('');
  let typedSpeaker: 'provider' | 'patient' = $state('provider');
  let clearDialogOpen = $state(false);
  let langDialogOpen = $state(false);
  let pendingLangChange: {
    provider: string;
    patient: string;
    /** Store values before the change — restored when the user cancels the
     *  confirm dialog, so the selects snap back to the session's pair. */
    previous: { provider: string; patient: string };
  } | null = null;

  // Medical recording and translation capture share one microphone slot —
  // disable tap-to-talk while a recording runs elsewhere in the app.
  const medicalRecordingActive = $derived(
    audio.state.state === 'recording' || audio.state.state === 'paused'
  );

  onMount(() => {
    // Seed the language pair from persisted settings BEFORE rehydration —
    // without this, the physician <select> renders its first option while
    // the store value stays '' (looks selected, isn't), and picking just
    // the patient language early-returns on the empty provider value. A
    // live backend session (if any) then wins in rehydrate().
    if (!translation.providerLang) {
      translation.providerLang = settings.state.translation_provider_language || 'en';
    }
    if (!translation.patientLang) {
      translation.patientLang = settings.state.translation_patient_language || '';
    }
    void translation.init();
  });

  // The store is a singleton, so an in-flight utterance keeps completing
  // across tab switches — nothing to tear down on unmount.

  async function scrollToBottom() {
    await tick();
    if (messagesEl) {
      messagesEl.scrollTop = messagesEl.scrollHeight;
    }
  }

  function onScroll(e: Event) {
    const el = e.currentTarget as HTMLElement;
    userNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 100;
  }

  $effect(() => {
    translation.entries.length;
    translation.phase;
    if (userNearBottom) {
      void scrollToBottom();
    }
  });

  function speakerLabel(speaker: 'provider' | 'patient'): string {
    return speaker === 'provider' ? 'Physician' : 'Patient';
  }

  function languageName(code: string): string {
    const match = translation.languages.find((l) => l.code === code);
    return match ? match.name : code;
  }

  /** Handle a language-select change. The selects' visible choices are
   *  recorded in the store FIRST (the UI is the source of truth — the old
   *  silent-return guards left the store holding '' while the selects
   *  showed real languages, wedging the tab in "Pick both languages");
   *  then the session side-effect runs per decideLanguageChange. */
  function onLanguageChange(provider: string, patient: string) {
    const previous = {
      provider: translation.providerLang,
      patient: translation.patientLang,
    };
    translation.providerLang = provider;
    translation.patientLang = patient;

    const decision = decideLanguageChange(provider, patient, translation.entries.length);
    if (decision.action === 'none') return;
    if (decision.action === 'invalid') {
      translation.setNotice(decision.reason);
      return;
    }
    if (decision.action === 'confirm') {
      pendingLangChange = { provider, patient, previous };
      langDialogOpen = true;
      return;
    }
    void applyLanguageChange(provider, patient);
  }

  async function applyLanguageChange(provider: string, patient: string) {
    settings.updateField('translation_provider_language', provider);
    settings.updateField('translation_patient_language', patient);
    await translation.restartSession(provider, patient);
  }

  async function submitTyped() {
    const text = typedInput.trim();
    if (!text || translation.phase !== 'idle') return;
    typedInput = '';
    await translation.submitText(typedSpeaker, text);
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      void submitTyped();
    }
  }

  function requestClear() {
    if (translation.entries.length === 0 && translation.phase === 'idle') return;
    if (translation.entries.length > 0) {
      clearDialogOpen = true;
    } else {
      void translation.clear();
    }
  }

  async function copyTranscript() {
    const text = await translation.exportText();
    if (!text) return;
    try {
      await copyToClipboard(text);
      toasts.success('Transcript copied to clipboard');
    } catch {
      toasts.error('Could not copy the transcript');
    }
  }

  // Sampled level meter — the store keeps the last 256 waveform peaks;
  // 24 evenly-spaced bars are enough for a live feel.
  const meterBars = $derived(
    translation.waveform.length > 0
      ? Array.from({ length: 24 }, (_, i) => {
          const idx = Math.floor((i / 24) * translation.waveform.length);
          return Math.min(1, Math.abs(translation.waveform[idx] ?? 0) * 4);
        })
      : Array.from({ length: 24 }, () => 0)
  );

  const phaseLabel = $derived(
    translation.phase === 'recording'
      ? `Listening — ${speakerLabel(translation.activeSpeaker ?? 'provider')} · tap again to stop`
      : translation.phase === 'transcribing'
        ? 'Transcribing…'
        : 'Translating…'
  );
</script>

<div class="translate-tab">
  <div class="translate-header">
    <div class="lang-pair">
      <label class="lang-select">
        <span class="lang-label">Physician</span>
        <select
          value={translation.providerLang}
          onchange={(e) =>
            onLanguageChange(e.currentTarget.value, translation.patientLang)}
          disabled={translation.phase !== 'idle'}
        >
          {#if !translation.providerLang}
            <option value="" disabled hidden>Pick a language…</option>
          {/if}
          {#each translation.languages as lang (lang.code)}
            <option value={lang.code}>{lang.name}</option>
          {/each}
        </select>
      </label>
      <span class="lang-arrow" aria-hidden="true">⇄</span>
      <label class="lang-select">
        <span class="lang-label">Patient</span>
        <select
          value={translation.patientLang}
          onchange={(e) =>
            onLanguageChange(translation.providerLang, e.currentTarget.value)}
          disabled={translation.phase !== 'idle'}
        >
          {#if !translation.patientLang}
            <option value="" disabled hidden>Pick a language…</option>
          {/if}
          {#each translation.languages as lang (lang.code)}
            <option value={lang.code}>{lang.name}</option>
          {/each}
        </select>
      </label>
    </div>
    <div class="header-actions">
      <button
        class="header-btn"
        onclick={copyTranscript}
        disabled={translation.entries.length === 0}
        title="Copy the conversation transcript"
      >
        Copy transcript
      </button>
      <button
        class="header-btn"
        onclick={requestClear}
        disabled={translation.entries.length === 0 && translation.phase === 'idle'}
        title="Clear the conversation"
      >
        Clear
      </button>
    </div>
  </div>

  <div class="messages-area" bind:this={messagesEl} onscroll={onScroll}>
    {#if translation.entries.length === 0 && translation.phase === 'idle'}
      <div class="welcome">
        <div class="welcome-icon">🌐</div>
        <h2>Conversation Translation</h2>
        <p>
          Talk with a patient who speaks another language: tap who is
          speaking, and their words are transcribed and translated for the
          other person to read (or hear, with 🔊). Nothing is saved — the
          conversation lives only in this session.
        </p>
        {#if !translation.patientLang}
          <p class="welcome-hint">Pick the patient's language above to begin.</p>
        {/if}
      </div>
    {:else}
      {#each translation.entries as entry, i (i)}
        <div
          class="entry"
          class:provider={entry.speaker === 'provider'}
          class:patient={entry.speaker === 'patient'}
        >
          <div class="bubble">
            <div class="meta">
              <span class="role">{speakerLabel(entry.speaker)}</span>
              <span class="time">{formatTimestamp(entry.timestamp)}</span>
              <button
                class="meta-btn"
                onclick={() => translation.speak(entry)}
                title="Read the translation aloud ({languageName(entry.target_lang)})"
                aria-label="Read translation aloud"
              >
                🔊
              </button>
            </div>
            <div class="original">{entry.original}</div>
            <div class="translated">{entry.translated}</div>
          </div>
        </div>
      {/each}

      {#if translation.phase !== 'idle'}
        <div
          class="entry pending"
          class:provider={translation.activeSpeaker === 'provider'}
          class:patient={translation.activeSpeaker === 'patient'}
        >
          <div class="bubble">
            <div class="meta">
              <span class="role">
                {translation.activeSpeaker
                  ? speakerLabel(translation.activeSpeaker)
                  : 'Processing'}
              </span>
              {#if translation.phase === 'recording'}
                <span class="time">{formatMinsSecs(translation.elapsed)}</span>
              {/if}
            </div>
            {#if translation.phase === 'recording'}
              <div class="meter" aria-hidden="true">
                {#each meterBars as bar, i (i)}
                  <span class="bar" style="height:{Math.max(8, bar * 100)}%"></span>
                {/each}
              </div>
            {:else}
              <div class="phase-label">{phaseLabel}</div>
              <div class="dots" aria-hidden="true">
                <span class="dot"></span>
                <span class="dot"></span>
                <span class="dot"></span>
              </div>
            {/if}
          </div>
        </div>
      {/if}
    {/if}
  </div>

  {#if translation.notice}
    <div class="notice-banner" role="status">
      {translation.notice}
      <button
        class="notice-dismiss"
        onclick={() => translation.dismissNotice()}
        aria-label="Dismiss notice"
      >
        ✕
      </button>
    </div>
  {:else if translation.error}
    <div class="error-banner" role="alert">
      {translation.error}
      <button class="error-dismiss" onclick={() => (translation.error = null)}>✕</button>
    </div>
  {/if}

  <div class="input-area">
    <div class="talk-row">
      <button
        class="talk-btn patient"
        class:active-capture={translation.phase === 'recording' && translation.activeSpeaker === 'patient'}
        onclick={() => translation.capture('patient')}
        disabled={(translation.phase !== 'idle' && translation.activeSpeaker !== 'patient') || medicalRecordingActive}
        title={medicalRecordingActive ? 'A medical recording is in progress' : undefined}
      >
        {#if translation.phase === 'recording' && translation.activeSpeaker === 'patient'}
          ⏹ Stop
        {:else}
          🎤 Patient
        {/if}
      </button>
      <button
        class="talk-btn provider"
        class:active-capture={translation.phase === 'recording' && translation.activeSpeaker === 'provider'}
        onclick={() => translation.capture('provider')}
        disabled={(translation.phase !== 'idle' && translation.activeSpeaker !== 'provider') || medicalRecordingActive}
        title={medicalRecordingActive ? 'A medical recording is in progress' : undefined}
      >
        {#if translation.phase === 'recording' && translation.activeSpeaker === 'provider'}
          ⏹ Stop
        {:else}
          🎤 Physician
        {/if}
      </button>
    </div>
    <div class="typed-row">
      <div class="speaker-toggle" role="group" aria-label="Typed text speaker">
        <button
          class:sel={typedSpeaker === 'provider'}
          onclick={() => (typedSpeaker = 'provider')}
          type="button"
        >
          {languageName(translation.providerLang || 'en')}
        </button>
        <button
          class:sel={typedSpeaker === 'patient'}
          onclick={() => (typedSpeaker = 'patient')}
          type="button"
        >
          {languageName(translation.patientLang || '…')}
        </button>
      </div>
      <input
        class="typed-input"
        placeholder="…or type what was said"
        bind:value={typedInput}
        onkeydown={handleKeyDown}
        disabled={translation.phase !== 'idle'}
      />
      <button
        class="send-btn"
        onclick={submitTyped}
        disabled={!typedInput.trim() || translation.phase !== 'idle'}
      >
        Translate
      </button>
    </div>
  </div>
</div>

<ConfirmDialog
  open={clearDialogOpen}
  title="Clear this conversation?"
  message="The translated conversation is only kept for this session — clearing it cannot be undone."
  confirmLabel="Clear"
  cancelLabel="Keep"
  danger
  onConfirm={() => {
    clearDialogOpen = false;
    void translation.clear();
  }}
  onCancel={() => (clearDialogOpen = false)}
/>

<ConfirmDialog
  open={langDialogOpen}
  title="Change languages?"
  message="Changing the language pair starts a new conversation — the current translated history will be cleared."
  confirmLabel="Change languages"
  cancelLabel="Keep current"
  danger
  onConfirm={() => {
    langDialogOpen = false;
    if (pendingLangChange) {
      void applyLanguageChange(pendingLangChange.provider, pendingLangChange.patient);
      pendingLangChange = null;
    }
  }}
  onCancel={() => {
    langDialogOpen = false;
    if (pendingLangChange) {
      // Snap the selects back to the session's language pair.
      translation.providerLang = pendingLangChange.previous.provider;
      translation.patientLang = pendingLangChange.previous.patient;
      pendingLangChange = null;
    }
  }}
/>

<style>
  .translate-tab {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .translate-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }

  .lang-pair {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .lang-select {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .lang-label {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-muted);
  }

  .lang-select select {
    font-size: 13px;
    padding: 4px 8px;
    border-radius: var(--radius-sm);
    background-color: var(--bg-card);
    color: var(--text-primary);
    border: 1px solid var(--border);
    min-width: 140px;
  }

  .lang-arrow {
    font-size: 16px;
    color: var(--text-muted);
    margin-top: 12px;
  }

  .header-actions {
    display: flex;
    gap: 8px;
  }

  .header-btn {
    font-size: 12px;
    padding: 4px 12px;
    color: var(--text-secondary);
    background-color: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .header-btn:hover:not(:disabled) {
    background-color: var(--bg-primary);
  }

  .header-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .messages-area {
    flex: 1;
    overflow-y: auto;
    padding: 12px 0;
  }

  .welcome {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    text-align: center;
    padding: 40px;
    gap: 10px;
    color: var(--text-muted);
  }

  .welcome-icon {
    font-size: 48px;
    margin-bottom: 8px;
  }

  .welcome h2 {
    font-size: 20px;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .welcome p {
    font-size: 13px;
    max-width: 400px;
    line-height: 1.6;
  }

  .welcome-hint {
    font-style: italic;
  }

  .entry {
    display: flex;
    margin: 6px 12px;
  }

  .entry.provider {
    justify-content: flex-start;
  }

  .entry.patient {
    justify-content: flex-end;
  }

  .bubble {
    max-width: 75%;
    border-radius: var(--radius-md);
    padding: 10px 12px;
    font-size: 13px;
    line-height: 1.6;
  }

  .provider .bubble {
    background-color: var(--bg-card);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-bottom-left-radius: 2px;
  }

  .patient .bubble {
    background-color: var(--accent);
    color: white;
    border-bottom-right-radius: 2px;
  }

  .meta {
    display: flex;
    gap: 8px;
    align-items: baseline;
    margin-bottom: 4px;
  }

  .role {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    opacity: 0.7;
  }

  .time {
    font-size: 10px;
    opacity: 0.5;
  }

  .meta-btn {
    margin-left: auto;
    padding: 0 4px;
    font-size: 12px;
    background: transparent;
    border: none;
    cursor: pointer;
    opacity: 0.7;
  }

  .meta-btn:hover {
    opacity: 1;
  }

  .original {
    white-space: pre-wrap;
    word-break: break-word;
    font-size: 12px;
    opacity: 0.75;
    margin-bottom: 2px;
  }

  .translated {
    white-space: pre-wrap;
    word-break: break-word;
  }

  .meter {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 32px;
  }

  .bar {
    width: 4px;
    border-radius: 2px;
    background-color: currentColor;
    opacity: 0.7;
    transition: height 80ms linear;
  }

  .pending .phase-label {
    font-size: 12px;
    color: var(--text-muted);
  }

  /* Patient-side pending bubble sits on the accent background, where the
   * muted grey would be unreadable — use white at reduced opacity. */
  .entry.pending.patient .phase-label {
    color: white;
    opacity: 0.85;
  }

  .dots {
    display: flex;
    gap: 4px;
    padding: 6px 0 2px;
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background-color: currentColor;
    opacity: 0.6;
    animation: pulse 1.2s ease-in-out infinite;
  }

  .dot:nth-child(2) { animation-delay: 0.2s; }
  .dot:nth-child(3) { animation-delay: 0.4s; }

  @keyframes pulse {
    0%, 80%, 100% { transform: scale(0.7); opacity: 0.4; }
    40% { transform: scale(1); opacity: 0.9; }
  }

  .notice-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 12px 4px;
    padding: 8px 12px;
    font-size: 12px;
    color: var(--text-secondary);
    background-color: var(--bg-card);
    border: 1px solid var(--border);
    border-left: 3px solid var(--accent);
    border-radius: var(--radius-sm);
  }

  .notice-dismiss {
    margin-left: auto;
    background: transparent;
    border: none;
    color: inherit;
    cursor: pointer;
    font-size: 12px;
    padding: 0 4px;
  }

  .error-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 12px 4px;
    padding: 8px 12px;
    font-size: 12px;
    color: var(--danger);
    background-color: var(--bg-card);
    border: 1px solid var(--danger);
    border-radius: var(--radius-sm);
  }

  .error-dismiss {
    margin-left: auto;
    background: transparent;
    border: none;
    color: inherit;
    cursor: pointer;
    font-size: 12px;
    padding: 0 4px;
  }

  .input-area {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border-top: 1px solid var(--border);
    background-color: var(--bg-secondary);
    flex-shrink: 0;
  }

  .talk-row {
    display: flex;
    gap: 8px;
  }

  .talk-btn {
    flex: 1;
    padding: 12px 16px;
    font-size: 14px;
    font-weight: 500;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background-color: var(--bg-card);
    color: var(--text-primary);
    cursor: pointer;
    transition: background-color 0.15s ease, border-color 0.15s ease;
  }

  .talk-btn:hover:not(:disabled) {
    background-color: var(--bg-hover);
    border-color: var(--accent);
  }

  .talk-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .talk-btn.active-capture {
    background-color: var(--danger);
    border-color: var(--danger);
    color: white;
  }

  .typed-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .speaker-toggle {
    display: flex;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    overflow: hidden;
    flex-shrink: 0;
  }

  .speaker-toggle button {
    padding: 6px 10px;
    font-size: 12px;
    background-color: var(--bg-card);
    color: var(--text-muted);
    border: none;
    cursor: pointer;
    white-space: nowrap;
  }

  .speaker-toggle button.sel {
    background-color: var(--accent);
    color: white;
  }

  .typed-input {
    flex: 1;
    font-size: 13px;
    border-radius: var(--radius-md);
  }

  .send-btn {
    padding: 6px 14px;
    background-color: var(--accent);
    color: white;
    border: none;
    border-radius: var(--radius-md);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
  }

  .send-btn:hover:not(:disabled) {
    background-color: var(--accent-hover);
  }

  .send-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
