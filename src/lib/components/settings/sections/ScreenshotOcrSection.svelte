<script lang="ts">
  /**
   * Screenshot-region OCR settings: global hotkey toggle + custom binding +
   * a "capture now" action. Region capture → local vision model OCR → the
   * extracted TEXT lands on the clipboard (pixels never touch the clipboard
   * — macOS/Windows sync clipboard history to the cloud).
   */
  import { settings } from '../../../stores/settings.svelte';
  import { toasts } from '../../../stores/toasts.svelte';
  import { captureRegionOcr, captureOutcomeMessage } from '../../../api/screenshotOcr';

  const DEFAULT_HOTKEY = 'CmdOrCtrl+Alt+O';

  const isLinux =
    typeof navigator !== 'undefined' && /linux/i.test(navigator.userAgent);

  let shortcutError = $state('');
  let capturing = $state(false);

  async function handleEnabledChange(e: Event) {
    const checked = (e.target as HTMLInputElement).checked;
    try {
      await settings.updateField('screenshot_ocr_hotkey_enabled', checked);
    } catch (err) {
      toasts.error(`Failed to save setting: ${err}`);
    }
  }

  /** Save on change; empty input = the default binding. The backend parses
   *  and rejects invalid accelerators at save time — surface that inline. */
  async function handleShortcutChange(e: Event) {
    const input = e.target as HTMLInputElement;
    const value = input.value.trim();
    try {
      shortcutError = '';
      await settings.updateField('screenshot_ocr_hotkey', value === '' ? null : value);
    } catch (err) {
      shortcutError = String(err);
      // Revert the field to the persisted value.
      input.value = settings.state.screenshot_ocr_hotkey ?? '';
    }
  }

  async function handleCaptureNow() {
    if (capturing) return;
    capturing = true;
    try {
      const outcome = await captureRegionOcr();
      const message = captureOutcomeMessage(outcome);
      if (outcome.status === 'empty' || outcome.status === 'cancelled') {
        toasts.add({ message, type: 'success', autoDismiss: true });
      } else {
        toasts.success(message);
      }
    } catch (err) {
      // A capture started elsewhere (hotkey) holding the in-flight guard is
      // a quiet no-op here, not an error.
      if (!String(err).includes('already in progress')) {
        toasts.error(`Screenshot OCR failed: ${err}`);
      }
    } finally {
      capturing = false;
    }
  }

  function copyCompositorBinding() {
    navigator.clipboard?.writeText(
      'o.bind("CTRL + ALT + O", "FerriScribe OCR capture", "rust-medical-assistant --capture-ocr")'
    );
  }
</script>

<h3 class="section-title">Screenshot Region OCR</h3>

<div class="form-group">
  <label class="form-label checkbox-label">
    <input
      type="checkbox"
      checked={settings.state.screenshot_ocr_hotkey_enabled}
      onchange={handleEnabledChange}
    />
    <span>Global hotkey ({settings.state.screenshot_ocr_hotkey || DEFAULT_HOTKEY})</span>
  </label>
  <span class="form-hint">
    Press the hotkey anywhere, drag-select a screen region, and its text is OCR'd by your
    local vision model and copied to the clipboard. Everything stays on this machine.
  </span>
</div>

<div class="form-group">
  <label for="ocr-hotkey" class="form-label">Hotkey binding</label>
  <input
    id="ocr-hotkey"
    type="text"
    placeholder={DEFAULT_HOTKEY}
    value={settings.state.screenshot_ocr_hotkey ?? ''}
    onchange={handleShortcutChange}
    disabled={!settings.state.screenshot_ocr_hotkey_enabled}
  />
  {#if shortcutError}
    <span class="field-error" role="alert">{shortcutError}</span>
  {:else}
    <span class="form-hint">
      Tauri accelerator syntax, e.g. <code>CmdOrCtrl+Alt+O</code>, <code>Ctrl+Shift+F9</code>.
      Leave empty for the default. Changes apply immediately.
    </span>
  {/if}
</div>

<div class="form-group">
  <span class="form-label">Capture now</span>
  <button class="btn-capture" onclick={handleCaptureNow} disabled={capturing}>
    {capturing ? 'Waiting for selection…' : 'OCR a screen region'}
  </button>
  <span class="form-hint">
    Same flow as the hotkey, triggered in-app — works on every platform, including
    Wayland sessions where apps can't register global shortcuts themselves.
  </span>
</div>

{#if isLinux}
  <div class="form-group">
    <span class="form-label">Wayland / Hyprland binding</span>
    <span class="form-hint">
      Under Wayland, compositors own global shortcuts — the app can't register one itself.
      Add the binding to your Hyprland config instead. For
      <strong>Omarchy</strong> (check which override file exists on your machine:
      <code>~/.config/hypr/bindings.lua</code> on current Omarchy, or
      <code>~/.config/hypr/bindings.conf</code> on pre-Quattro Omarchy — the upgrade
      silently stopped loading the old file):
    </span>
    <pre><code>o.bind("CTRL + ALT + O", "FerriScribe OCR capture", "rust-medical-assistant --capture-ocr")</code></pre>
    <span class="form-hint">For vanilla (non-Omarchy) Hyprland, use a .conf line instead:</span>
    <pre><code>bind = CTRL ALT, O, exec, rust-medical-assistant --capture-ocr</code></pre>
    <button class="btn-copy" onclick={copyCompositorBinding}>Copy Omarchy line</button>
  </div>
{/if}

<style>
  .btn-capture {
    padding: 6px 12px;
    font-size: 12px;
    font-weight: 500;
    border-radius: var(--radius-sm);
    cursor: pointer;
    background-color: var(--accent);
    color: var(--text-inverse);
  }

  .btn-capture:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .btn-capture:not(:disabled):hover {
    background-color: var(--accent-hover);
  }

  .btn-copy {
    margin-top: 6px;
    padding: 4px 10px;
    font-size: 11px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
  }

  pre {
    margin: 6px 0;
    padding: 8px 10px;
    background-color: var(--bg-code, #1e1e1e);
    color: var(--text-primary, #e5e5e5);
    border-radius: var(--radius-sm);
    font-size: 11px;
    overflow-x: auto;
  }
</style>
