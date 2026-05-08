<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount, createEventDispatcher, tick } from 'svelte';
  import PairingQr from './PairingQr.svelte';
  import { renameClient } from '../../../api/sharing';
  const dispatch = createEventDispatcher();

  type SharingStatus = {
    enabled: boolean;
    ollama_ok: boolean;
    whisper_ok: boolean;
    lmstudio_ok: boolean;
    mdns_ok: boolean;
    pairing_ok: boolean;
    paired_clients: number;
  };

  let qrPayload = '';
  let clients: { id: number; label: string }[] = [];
  let status: SharingStatus | null = null;
  let pollHandle: ReturnType<typeof setInterval>;

  let editingId: number | null = null;
  let draftLabel = '';
  let editError: string | null = null;
  let editInputEl: HTMLInputElement | null = null;

  async function refresh() {
    status = await invoke<SharingStatus>('sharing_status');
    // Don't clobber the input the user is typing in.
    if (editingId === null) {
      clients = await invoke('list_paired_clients');
    }
  }

  async function regenQr() {
    qrPayload = await invoke('pairing_qr');
  }

  async function revoke(id: number) {
    if (editingId === id) cancelEdit();
    await invoke('revoke_client', { id });
    await refresh();
  }

  async function startEdit(c: { id: number; label: string }) {
    editingId = c.id;
    draftLabel = c.label;
    editError = null;
    await tick();
    editInputEl?.focus();
    editInputEl?.select();
  }

  function cancelEdit() {
    editingId = null;
    draftLabel = '';
    editError = null;
  }

  async function commitEdit() {
    if (editingId === null) return;
    const trimmed = draftLabel.trim();
    if (!trimmed) {
      editError = 'Label cannot be empty.';
      return;
    }
    try {
      await renameClient(editingId, trimmed);
      editingId = null;
      draftLabel = '';
      editError = null;
      await refresh();
    } catch (e) {
      editError = String(e);
    }
  }

  function onEditKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      commitEdit();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancelEdit();
    }
  }

  async function stop() {
    await invoke('stop_sharing');
    dispatch('stopped');
  }

  onMount(() => {
    refresh().then(() => regenQr());
    pollHandle = setInterval(refresh, 5000);
    return () => clearInterval(pollHandle);
  });

  const checks: { key: keyof SharingStatus; label: string; offHint?: string }[] = [
    { key: 'ollama_ok', label: 'Ollama' },
    { key: 'whisper_ok', label: 'Whisper' },
    {
      key: 'lmstudio_ok',
      label: 'LM Studio',
      offHint: 'LM Studio not running on this machine. Start its local server, then Stop and Start sharing.',
    },
    { key: 'mdns_ok', label: 'mDNS' },
    { key: 'pairing_ok', label: 'Pairing' },
  ];
</script>

<section>
  <h3>This machine is the office server</h3>

  <div class="status-panel" aria-label="Sharing service health">
    {#each checks as c}
      {@const ok = !!status?.[c.key]}
      <div class="status-row" class:ok class:fail={!ok} title={!ok && c.offHint ? c.offHint : ''}>
        <span class="status-icon" aria-hidden="true">{ok ? '✓' : '✗'}</span>
        <span class="status-label">{c.label}</span>
        <span class="status-state">{ok ? 'OK' : 'Not running'}</span>
      </div>
    {/each}
  </div>

  <div class="grid">
    <div>
      <h4>Pairing</h4>
      <PairingQr payload={qrPayload} />
      <button class="btn" on:click={regenQr}>New code</button>
    </div>
    <div>
      <h4>Connected clients ({clients.length})</h4>
      {#if clients.length === 0}
        <p class="hint">No clinicians paired yet. Have them open
        Settings &rarr; Sharing &rarr; "This machine connects to an office server"
        and either pick this server from the list or scan the QR.</p>
      {:else}
        <ul class="clients">
          {#each clients as c (c.id)}
            <li>
              {#if editingId === c.id}
                <input
                  class="edit-input"
                  bind:this={editInputEl}
                  bind:value={draftLabel}
                  on:keydown={onEditKeydown}
                  aria-label="Rename client"
                />
                <button
                  class="btn btn-icon"
                  title="Save"
                  aria-label="Save"
                  on:click={commitEdit}>✓</button>
                <button
                  class="btn btn-icon"
                  title="Cancel"
                  aria-label="Cancel"
                  on:click={cancelEdit}>✕</button>
              {:else}
                <span class="client-label">{c.label}</span>
                <button
                  class="btn btn-icon"
                  title="Rename"
                  aria-label="Rename {c.label}"
                  on:click={() => startEdit(c)}>✎</button>
              {/if}
              <button class="btn btn-revoke" on:click={() => revoke(c.id)}>Revoke</button>
            </li>
            {#if editingId === c.id && editError}
              <li class="edit-error">{editError}</li>
            {/if}
          {/each}
        </ul>
      {/if}
    </div>
  </div>
  <button class="btn btn-danger" on:click={stop}>Stop sharing</button>
</section>

<style>
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 2rem; }
  .clients { list-style: none; padding: 0; }
  .clients li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.25rem 0;
  }
  .hint { color: var(--text-muted, #888); }

  .status-panel {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: 0.5rem;
    padding: 0.75rem;
    border: 1px solid var(--border, #d0d0d0);
    border-radius: 0.5rem;
    background: var(--surface-2, #f7f7f7);
  }
  .status-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.9rem;
  }
  .status-icon {
    width: 1.25rem;
    height: 1.25rem;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    font-weight: 700;
    color: white;
  }
  .status-row.ok .status-icon { background: #16a34a; }
  .status-row.fail .status-icon { background: #c0392b; }
  .status-label { font-weight: 600; }
  .status-state { color: var(--text-muted, #888); margin-left: auto; }
  .status-row.fail .status-state { color: #c0392b; }

  .btn {
    border: 1px solid var(--border, #c8c8c8);
    background: var(--surface-1, #fff);
    color: inherit;
    padding: 0.4rem 0.9rem;
    border-radius: 0.375rem;
    font-weight: 500;
    transition: background 0.12s ease, border-color 0.12s ease;
  }
  .btn:hover:not(:disabled) {
    background: var(--surface-2, #f0f0f0);
    border-color: var(--border-strong, #a0a0a0);
  }
  .btn-revoke { padding: 0.2rem 0.6rem; font-size: 0.85rem; }
  .btn-danger {
    margin-top: 1rem;
    border-color: #c0392b;
    color: #c0392b;
  }
  .btn-danger:hover:not(:disabled) {
    background: #c0392b;
    color: white;
  }
  .client-label { flex: 1; }
  .edit-input {
    flex: 1;
    padding: 0.2rem 0.45rem;
    border: 1px solid var(--border, #c8c8c8);
    border-radius: 0.3rem;
    background: var(--surface-1, transparent);
    color: inherit;
    font: inherit;
  }
  .btn-icon {
    padding: 0.15rem 0.45rem;
    font-size: 0.95rem;
    line-height: 1;
  }
  .edit-error {
    color: #c0392b;
    font-size: 0.85rem;
    padding-left: 0.25rem;
  }
</style>
