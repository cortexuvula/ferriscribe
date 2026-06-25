<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { open as openFileDialog } from '@tauri-apps/plugin-dialog';

  interface Props {
    reason: string;
  }
  const { reason }: Props = $props();

  let busy = $state(false);
  let errorMessage = $state<string | null>(null);
  let wipeConfirmText = $state('');
  let showWipeConfirm = $state(false);

  async function handleRestoreFromBackup() {
    if (busy) return;
    busy = true;
    errorMessage = null;
    try {
      const picked = await openFileDialog({
        multiple: false,
        filters: [{ name: 'SQLite database', extensions: ['db', 'sqlite', 'bak'] }],
      });
      if (!picked || Array.isArray(picked)) {
        busy = false;
        return;
      }
      await invoke('recover_database_from_path', { backupPath: picked });
      window.location.reload();
    } catch (e: unknown) {
      errorMessage = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function handleWipeAndReset() {
    if (busy) return;
    if (wipeConfirmText !== 'DELETE') return;
    busy = true;
    errorMessage = null;
    try {
      await invoke('recover_database_wipe');
      window.location.reload();
    } catch (e: unknown) {
      errorMessage = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  function handleQuit() {
    // plugin-process is not currently bundled; closing the window is the
    // simplest portable shutdown signal for the recovery overlay.
    window.close();
  }
</script>

<div class="recovery-overlay">
  <div class="recovery-dialog">
    <h2>Encrypted database, missing access key</h2>
    <p>
      The database is encrypted, but the access key stored in your system
      keychain is missing or inaccessible. The data cannot be decrypted
      without it.
    </p>
    <p class="reason-line">Detail: {reason}</p>
    <p>
      This usually means the keychain was reset, the app was reinstalled,
      or the data folder was copied from another machine.
    </p>

    {#if errorMessage}
      <div class="error">{errorMessage}</div>
    {/if}

    {#if showWipeConfirm}
      <div class="wipe-confirm">
        <p><strong>This will permanently delete your database.</strong></p>
        <p>Type <code>DELETE</code> to confirm:</p>
        <input bind:value={wipeConfirmText} disabled={busy} />
        <div class="actions">
          <button class="btn-cancel" onclick={() => (showWipeConfirm = false)} disabled={busy}>
            Cancel
          </button>
          <button class="btn-danger" onclick={handleWipeAndReset} disabled={busy || wipeConfirmText !== 'DELETE'}>
            Wipe and start fresh
          </button>
        </div>
      </div>
    {:else}
      <div class="actions">
        <button class="btn-primary" onclick={handleRestoreFromBackup} disabled={busy}>
          {busy ? 'Working…' : 'Restore from backup file'}
        </button>
        <button class="btn-danger" onclick={() => (showWipeConfirm = true)} disabled={busy}>
          Wipe and start fresh
        </button>
        <button class="btn-secondary" onclick={handleQuit} disabled={busy}>Quit</button>
      </div>
    {/if}
  </div>
</div>

<style>
  .recovery-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 9999;
  }
  .recovery-dialog {
    background: var(--bg-primary, #fff);
    color: var(--text-primary, #222);
    border-radius: 8px;
    padding: 24px;
    max-width: 540px;
    width: 90%;
    box-shadow: 0 20px 50px rgba(0, 0, 0, 0.3);
  }
  .reason-line {
    font-family: var(--font-mono, monospace);
    font-size: 0.85em;
    color: var(--text-secondary, #555);
  }
  .error {
    background: rgba(220, 53, 69, 0.1);
    color: #b00020;
    padding: 8px 12px;
    border-radius: 4px;
    margin-top: 12px;
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 20px;
  }
  .btn-primary, .btn-danger, .btn-secondary, .btn-cancel {
    padding: 8px 16px;
    border-radius: 4px;
    border: 1px solid var(--border, #ccc);
    cursor: pointer;
    font-size: 14px;
  }
  .btn-primary { background: var(--accent, #4c6ef5); color: white; border-color: var(--accent, #4c6ef5); }
  .btn-danger { background: #dc3545; color: white; border-color: #dc3545; }
  .btn-secondary { background: transparent; }
  .btn-cancel { background: transparent; }
  .wipe-confirm input {
    width: 100%;
    padding: 6px 10px;
    margin: 8px 0;
  }
</style>
