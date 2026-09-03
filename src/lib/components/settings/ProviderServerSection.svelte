<script lang="ts">
  /**
   * Shared "provider server" subsection of Settings → Models: host input
   * with the public-endpoint PHI warning, port input with 1–65535
   * validation, an optional API key (persisted to the keychain slot),
   * a Test Connection button (useTestConnection state machine), and the
   * disable-thinking toggle. One component instead of three
   * hand-maintained copies (LM Studio / Ollama / oMLX).
   *
   * All edits persist via `settings.updateField` and re-init providers so
   * the new endpoint takes effect without an app restart. Section copy is
   * passed as snippets (`hint`, `thinkingHint`) so callers keep rich markup
   * without an {@html} XSS surface.
   */
  import type { Snippet } from 'svelte';
  import { onMount } from 'svelte';
  import { getApiKey, setApiKey } from '../../api/settings';
  import { reinitProviders } from '../../api/chat';
  import { settings } from '../../stores/settings.svelte';
  import type { AppConfig } from '../../types';
  import { classifyEndpoint, isLocalOrAllowed } from '../../utils/endpointPolicy';
  import { useTestConnection } from '../../composables/useTestConnection.svelte';
  import Callout from './Callout.svelte';

  type HostField = Extract<keyof AppConfig, `${'lmstudio' | 'ollama' | 'omlx'}_host`>;
  type PortField = Extract<keyof AppConfig, `${'lmstudio' | 'ollama' | 'omlx'}_port`>;
  type ThinkingField = Extract<
    keyof AppConfig,
    `${'lmstudio' | 'ollama' | 'omlx'}_disable_thinking`
  >;

  interface Props {
    /** Prefix for element ids (e.g. "omlx" → id="omlx-host"). */
    idPrefix: string;
    /** Section heading, e.g. "oMLX Server". */
    title: string;
    /** Hide the built-in heading (callers that embed this section under
     *  their own header, e.g. the collapsible provider <details> in
     *  Models.svelte, own the title markup). */
    hideTitle?: boolean;
    /** Intro hint rendered under the heading (optional). */
    hint?: Snippet;
    /** AppConfig field names this section edits. */
    hostField: HostField;
    portField: PortField;
    /** Default/fallback port (placeholder + `?? fallback` reads). */
    defaultPort: number;
    /** Keychain slot holding the optional bearer key. */
    apiKeySlot: string;
    /** `(host, port, apiKey) => success message` — the connection test. */
    testConnection: (host: string, port: number, apiKey: string | null) => Promise<string>;
    /** AppConfig field for the thinking toggle; omit to hide the toggle. */
    thinkingField?: ThinkingField;
    /** Hint rendered under the thinking toggle. */
    thinkingHint?: Snippet;
  }

  let {
    idPrefix,
    title,
    hideTitle = false,
    hint,
    hostField,
    portField,
    defaultPort,
    apiKeySlot,
    testConnection,
    thinkingField,
    thinkingHint,
  }: Props = $props();

  const test = useTestConnection();

  /** Draft key for the optional bearer-token field. Pre-filled from the
   *  keychain on mount; persisted only when "Save key" is clicked (mirrors
   *  SttRemoteSection's flow — the keychain write must never be implicit). */
  let apiKey = $state('');

  onMount(() => {
    getApiKey(apiKeySlot)
      .then((key) => {
        if (key) apiKey = key;
      })
      .catch(() => {
        // Keychain unavailable/empty — leave the field blank.
      });
  });

  const host = $derived(settings.state[hostField]);
  const port = $derived(settings.state[portField] ?? defaultPort);
  const thinkingDisabled = $derived(
    thinkingField ? (settings.state[thinkingField] ?? false) : false,
  );

  const endpointOk = $derived(isLocalOrAllowed(host ?? '', settings.state.allow_public_endpoint));
  const endpointKind = $derived(classifyEndpoint(host ?? ''));

  /** Inline port-validation message — an invalid port is never persisted,
   *  so the field is reverted and told why instead of silently lying. */
  let portError = $state('');

  async function onHostChange(e: Event) {
    await settings.updateField(hostField, (e.target as HTMLInputElement).value);
    test.reset();
    await reinitProviders();
  }

  async function onPortChange(e: Event) {
    const value = parseInt((e.target as HTMLInputElement).value, 10);
    if (value >= 1 && value <= 65535) {
      portError = '';
      await settings.updateField(portField, value);
      test.reset();
      await reinitProviders();
    } else {
      portError = 'Port must be between 1 and 65535.';
      (e.target as HTMLInputElement).value = String(port);
    }
  }

  async function onTestConnection() {
    await test.run(async () => {
      // Prefer the key currently in the field: a just-typed (but unsaved)
      // key should be what gets verified. Fall back to the stored one.
      let key: string | null = apiKey || null;
      if (!key) {
        try {
          key = await getApiKey(apiKeySlot);
        } catch {
          // Keychain unavailable — try without auth.
        }
      }
      return testConnection(host || 'localhost', port, key);
    });
  }

  async function onSaveKey() {
    await test.run(async () => {
      await setApiKey(apiKeySlot, apiKey);
      await reinitProviders();
      return 'Key saved.';
    });
  }

  async function onThinkingToggle(e: Event) {
    if (!thinkingField) return;
    await settings.updateField(thinkingField, (e.target as HTMLInputElement).checked);
    try {
      await reinitProviders();
    } catch (err) {
      console.error('Failed to reinit providers after thinking toggle:', err);
    }
  }
</script>

<div class="form-group-divider"></div>
{#if !hideTitle}
  <h4 class="subsection-title">{title}</h4>
{/if}
{#if hint}
  <p class="subsection-hint">{@render hint()}</p>
{/if}

<div class="form-group">
  <label for="{idPrefix}-host" class="form-label">Host</label>
  <input
    id="{idPrefix}-host"
    type="text"
    value={host ?? ''}
    placeholder="localhost"
    onchange={onHostChange}
    class="text-input"
  />
  {#if !endpointOk}
    <Callout kind="warning">
      ⚠ This is a public-internet address ({endpointKind}). PHI may leave your device.
      Enable <em>Allow public endpoints</em> in Advanced settings to use this anyway.
    </Callout>
  {/if}
</div>

<div class="form-group">
  <label for="{idPrefix}-port" class="form-label">Port</label>
  <input
    id="{idPrefix}-port"
    type="number"
    value={port}
    placeholder={String(defaultPort)}
    min="1"
    max="65535"
    onchange={onPortChange}
    class="text-input port-input"
    aria-invalid={portError ? 'true' : undefined}
  />
  {#if portError}
    <span class="field-error" role="alert">{portError}</span>
  {/if}
</div>

<div class="form-group">
  <label for="{idPrefix}-api-key" class="form-label">API key (optional)</label>
  <input
    id="{idPrefix}-api-key"
    type="password"
    bind:value={apiKey}
    autocomplete="off"
    class="text-input"
  />
  <button class="btn-test-connection" type="button" onclick={onSaveKey}>
    Save key
  </button>
  <span class="form-hint">Sent as a Bearer token to the server. Leave blank and click Save key to clear.</span>
</div>

<div class="form-group">
  <button class="btn-test-connection" disabled={test.status === 'testing'} onclick={onTestConnection}>
    {#if test.status === 'testing'}
      Testing…
    {:else}
      Test Connection
    {/if}
  </button>
  {#if test.status === 'success'}
    <span class="test-result test-success">✓ {test.message}</span>
  {:else if test.status === 'error'}
    <span class="test-result test-error">✗ {test.message}</span>
  {/if}
</div>

{#if thinkingField}
  <div class="form-group">
    <label class="form-row">
      <input type="checkbox" checked={thinkingDisabled} onchange={onThinkingToggle} />
      <span>
        Disable thinking (reasoning models)
        {#if thinkingHint}
          <p class="form-hint">{@render thinkingHint()}</p>
        {/if}
      </span>
    </label>
  </div>
{/if}

<style>
  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-label {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .text-input {
    padding: 8px 10px;
    font-size: 13px;
    background-color: var(--bg-input);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }

  .port-input {
    max-width: 120px;
  }

  .field-error {
    font-size: 11px;
    color: var(--danger);
  }

  .form-group-divider {
    border-top: 1px solid var(--border);
    margin: 20px 0 16px;
  }

  .subsection-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0 0 4px;
  }

  .subsection-hint {
    font-size: 12px;
    color: var(--text-muted);
    margin: 0 0 12px;
    line-height: 1.5;
  }

  .subsection-hint :global(code) {
    font-size: 11px;
    background-color: var(--bg-tertiary, #374151);
    padding: 1px 5px;
    border-radius: 3px;
  }

  .btn-test-connection {
    align-self: flex-start;
    padding: 6px 14px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      color 0.15s ease,
      border-color 0.15s ease;
  }

  .btn-test-connection:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-primary);
    border-color: var(--accent);
  }

  .btn-test-connection:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .test-result {
    font-size: 13px;
    margin-left: 10px;
  }

  .test-result.test-success {
    color: var(--success);
  }

  .test-result.test-error {
    color: var(--danger, #ef4444);
  }

  .form-row {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    cursor: pointer;
  }

  .form-hint {
    font-size: 11px;
    color: var(--text-muted);
    margin: 4px 0 0;
    line-height: 1.5;
  }

  .form-hint :global(code) {
    font-size: 10px;
    background-color: var(--bg-tertiary, #374151);
    padding: 1px 4px;
    border-radius: 3px;
  }
</style>
