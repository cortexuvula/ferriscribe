<script lang="ts">
  import { updater } from '../stores/updater.svelte';
</script>

{#if updater.state === 'available' || updater.state === 'downloading' || updater.state === 'installed' || updater.state === 'error'}
  <div class="update-banner" role="alert">
    {#if updater.state === 'available'}
      <span class="banner-text">
        🟢 FerriScribe {updater.availableVersion} is available
      </span>
      <div class="banner-actions">
        <button class="btn-install" onclick={() => updater.downloadAndInstall()}>
          Download &amp; Install
        </button>
        <button class="btn-later" onclick={() => updater.dismiss()}>Later</button>
      </div>
    {:else if updater.state === 'downloading'}
      <span class="banner-text">Downloading… {updater.downloadProgress}%</span>
      <div class="progress-bar">
        <div class="progress-fill" style="width: {updater.downloadProgress}%"></div>
      </div>
    {:else if updater.state === 'installed'}
      <span class="banner-text">✓ Update installed</span>
      <div class="banner-actions">
        <button class="btn-install" onclick={() => updater.relaunch()}>Restart now</button>
        <button class="btn-later" onclick={() => updater.dismiss()}>Later</button>
      </div>
    {:else if updater.state === 'error'}
      <span class="banner-text banner-error">⚠ Update failed: {updater.errorMessage}</span>
      <div class="banner-actions">
        <button class="btn-install" onclick={() => updater.downloadAndInstall()}>Retry</button>
        <button class="btn-later" onclick={() => updater.dismiss()}>Dismiss</button>
      </div>
    {/if}
  </div>
{/if}

<style>
  .update-banner {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 14px;
    padding: 8px 16px;
    background-color: var(--bg-hover, #2a2a2a);
    border-bottom: 1px solid var(--border, #333);
    font-size: 13px;
    color: var(--text-primary);
    flex-shrink: 0;
  }
  .banner-text { white-space: nowrap; }
  .banner-error { color: var(--danger, #ef4444); }
  .banner-actions { display: flex; gap: 8px; }
  .btn-install {
    padding: 4px 14px; font-size: 12px; font-weight: 600; color: white;
    background-color: var(--accent, #3b82f6); border: none;
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-install:hover { background-color: var(--accent-hover, #2563eb); }
  .btn-later {
    padding: 4px 10px; font-size: 12px; color: var(--text-muted);
    background: none; border: none; cursor: pointer; text-decoration: underline;
  }
  .btn-later:hover { color: var(--text-secondary); }
  .progress-bar {
    flex: 1; max-width: 200px; height: 6px; background-color: var(--border, #333);
    border-radius: 3px; overflow: hidden;
  }
  .progress-fill {
    height: 100%; background-color: var(--accent, #3b82f6);
    transition: width 0.3s ease;
  }
</style>
