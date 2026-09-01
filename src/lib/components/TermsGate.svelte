<script lang="ts">
  /**
   * One-time Terms of Service acceptance gate.
   *
   * Rendered by App.svelte in place of the app shell whenever
   * `settings.state.tos_accepted_at` is null (new users see it before
   * onboarding; existing users see it exactly once after the update that
   * introduces the field). Deliberately NOT closable — the Terms state
   * "if you do not accept, do not use the software", so the only exits are
   * Accept or quitting the application.
   */
  import { settings } from '../stores/settings.svelte';
  import { TERMS_OF_SERVICE_TEXT } from '../terms';

  let accepting = $state(false);
  let error = $state<string | null>(null);

  async function accept() {
    if (accepting) return;
    accepting = true;
    error = null;
    try {
      await settings.updateField('tos_accepted_at', new Date().toISOString());
      // No navigation needed: the store update flips the App.svelte gate
      // reactively and this component unmounts.
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      accepting = false;
    }
  }
</script>

<div class="tos-gate" role="dialog" aria-modal="true" aria-label="Terms of Service">
  <div class="tos-panel">
    <h1>FerriScribe — Terms of Service</h1>
    <p class="tos-intro">
      Your use of FerriScribe means you accept these Terms. Please read them
      before continuing — they cover your obligations as the treating
      physician, patient consent, privacy, and the limits of the project's
      liability.
    </p>
    <div class="tos-scroll">
      <pre>{TERMS_OF_SERVICE_TEXT}</pre>
    </div>
    {#if error}
      <p class="tos-error" role="alert">Could not save your acceptance: {error} — try again.</p>
    {/if}
    <div class="tos-actions">
      <button class="accept" onclick={accept} disabled={accepting}>
        {accepting ? 'Saving…' : 'I have read and accept the Terms of Service'}
      </button>
      <span class="decline-hint">
        If you do not accept, close FerriScribe (⌘Q) — the software cannot be
        used without acceptance.
      </span>
    </div>
  </div>
</div>

<style>
  .tos-gate {
    position: fixed;
    inset: 0;
    background-color: var(--bg-primary, #111);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    z-index: 1000;
  }
  .tos-panel {
    display: flex;
    flex-direction: column;
    max-width: 860px;
    width: 100%;
    height: 100%;
    max-height: 92vh;
    background-color: var(--bg-secondary, #1c1c1e);
    border: 1px solid var(--border, #333);
    border-radius: var(--radius-md, 8px);
    padding: 24px 28px;
    gap: 14px;
  }
  .tos-panel h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 700;
  }
  .tos-intro {
    margin: 0;
    font-size: 13px;
    color: var(--text-secondary);
    line-height: 1.5;
  }
  .tos-scroll {
    flex: 1;
    overflow-y: auto;
    border: 1px solid var(--border, #333);
    border-radius: var(--radius-sm, 4px);
    background-color: var(--bg-primary, #111);
    padding: 16px;
  }
  .tos-scroll pre {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
    line-height: 1.55;
    color: var(--text-primary);
  }
  .tos-error {
    margin: 0;
    font-size: 12px;
    color: var(--danger, #ef4444);
  }
  .tos-actions {
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-start;
  }
  .accept {
    padding: 10px 18px;
    font-size: 13px;
    font-weight: 600;
    color: white;
    background-color: var(--accent, #3b82f6);
    border: none;
    border-radius: var(--radius-sm, 4px);
    cursor: pointer;
  }
  .accept:hover:not(:disabled) { background-color: var(--accent-hover, #2563eb); }
  .accept:disabled { opacity: 0.6; cursor: not-allowed; }
  .decline-hint {
    font-size: 11px;
    color: var(--text-muted);
  }
</style>
