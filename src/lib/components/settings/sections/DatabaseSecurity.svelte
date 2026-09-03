<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';

  let encryptionState = $state<'no-database' | 'plaintext' | 'encrypted' | 'unknown'>('unknown');
  let keyPresent = $state<boolean | null>(null);

  async function loadEncryptionStatus() {
    try {
      const result = await invoke<{ state: string; key_present?: boolean }>('database_encryption_status');
      encryptionState = (result.state as 'no-database' | 'plaintext' | 'encrypted') || 'unknown';
      keyPresent = typeof result.key_present === 'boolean' ? result.key_present : null;
    } catch (e) {
      console.error('Failed to query database encryption status:', e);
      encryptionState = 'unknown';
      keyPresent = null;
    }
  }

  onMount(loadEncryptionStatus);
</script>

<h3 class="section-title" style="margin-top: 24px">Database Security</h3>
<p class="section-desc">
  Your medical records are stored in a SQLite database. The encryption
  key is stored in your operating system's keychain. Back up your
  database regularly — if the keychain entry is lost, the data cannot
  be recovered.
</p>
<div class="form-group">
  <span class="form-label">Encryption status</span>
  {#if encryptionState === 'encrypted' && keyPresent === false}
    <span class="status-pill plaintext">
      ⚠ Encrypted — key NOT found in OS keychain (data may be unrecoverable)
    </span>
  {:else if encryptionState === 'encrypted'}
    <span class="status-pill encrypted">✓ Encrypted (key in OS keychain)</span>
  {:else if encryptionState === 'plaintext'}
    <span class="status-pill plaintext">⚠ Plaintext (encryption disabled)</span>
  {:else if encryptionState === 'no-database'}
    <span class="status-pill">No database yet</span>
  {:else}
    <span class="status-pill">Checking…</span>
  {/if}
</div>

<style>
  .section-desc {
    font-size: 12px;
    color: var(--text-muted);
    margin-top: -8px;
  }

  .status-pill {
    display: inline-block;
    padding: 4px 10px;
    border-radius: 4px;
    font-size: 13px;
    background: var(--bg-tertiary, #f1f3f5);
    color: var(--text-secondary, #495057);
    border: 1px solid var(--border, #dee2e6);
  }

  .status-pill.encrypted {
    background: color-mix(in srgb, var(--success) 12%, transparent);
    color: var(--success);
    border-color: color-mix(in srgb, var(--success) 35%, transparent);
  }

  .status-pill.plaintext {
    background: color-mix(in srgb, var(--warning) 12%, transparent);
    color: var(--warning);
    border-color: color-mix(in srgb, var(--warning) 35%, transparent);
  }
</style>
