<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';

  let encryptionState = $state<'no-database' | 'plaintext' | 'encrypted' | 'unknown'>('unknown');

  async function loadEncryptionStatus() {
    try {
      const result = await invoke<{ state: string; key_present?: boolean }>('database_encryption_status');
      encryptionState = (result.state as 'no-database' | 'plaintext' | 'encrypted') || 'unknown';
    } catch (e) {
      console.error('Failed to query database encryption status:', e);
      encryptionState = 'unknown';
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
  {#if encryptionState === 'encrypted'}
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
    background: rgba(40, 167, 69, 0.1);
    color: #155724;
    border-color: rgba(40, 167, 69, 0.3);
  }

  .status-pill.plaintext {
    background: rgba(255, 193, 7, 0.1);
    color: #856404;
    border-color: rgba(255, 193, 7, 0.3);
  }
</style>
