<script lang="ts">
  import { theme } from '../stores/theme.svelte';
  import { settings } from '../stores/settings.svelte';

  interface Props {
    activeTab: string;
  }

  let { activeTab = $bindable('record') }: Props = $props();

  const workflowNav = [
    { id: 'record',        label: 'Record',        icon: '🎙' },
    { id: 'recordings',    label: 'Recordings',    icon: '📋' },
    { id: 'generate',      label: 'Generate',      icon: '⚡' },
    { id: 'letter_writer', label: 'Letter Writer', icon: '📝' },
    { id: 'chat',          label: 'Chat',          icon: '💬' },
    { id: 'translate',     label: 'Translate',     icon: '🌐' },
  ];

  const documentNav = [
    { id: 'transcript', label: 'Transcript', icon: '📄' },
    { id: 'soap',       label: 'SOAP Note',  icon: '🏥' },
    { id: 'referral',   label: 'Referral',   icon: '📨' },
    { id: 'letter',         label: 'Letter',         icon: '✉' },
    { id: 'peer_discussion', label: 'Peer Discussion', icon: '👥' },
  ];
</script>

<aside class="sidebar">
  <div class="sidebar-top">
    <div class="app-title">FerriScribe</div>

    <nav aria-label="Main navigation">
      <div class="nav-section-label">Workflow</div>
      {#each workflowNav as item (item.id)}
        <button
          class="nav-item"
          class:active={activeTab === item.id}
          aria-current={activeTab === item.id ? 'page' : undefined}
          onclick={() => (activeTab = item.id)}
        >
          <span class="nav-icon" aria-hidden="true">{item.icon}</span>
          <span class="nav-label">{item.label}</span>
        </button>
      {/each}

      <div class="nav-section-label" style="margin-top: 16px;">Documents</div>
      {#each documentNav as item (item.id)}
        <button
          class="nav-item"
          class:active={activeTab === item.id}
          aria-current={activeTab === item.id ? 'page' : undefined}
          onclick={() => (activeTab = item.id)}
        >
          <span class="nav-icon" aria-hidden="true">{item.icon}</span>
          <span class="nav-label">{item.label}</span>
        </button>
      {/each}
    </nav>
  </div>

  <div class="sidebar-footer">
    <button class="footer-btn" onclick={() => { theme.toggle(); settings.updateField('theme', theme.current); }} title="Toggle theme">
      <span>☀/🌙</span>
      <span>Theme</span>
    </button>
    <button class="footer-btn" onclick={() => (activeTab = 'settings')} title="Settings">
      <span>⚙</span>
      <span>Settings</span>
    </button>
  </div>
</aside>

<style>
  .sidebar {
    width: var(--sidebar-width);
    background-color: var(--bg-sidebar);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }

  .sidebar-top {
    flex: 1;
    overflow-y: auto;
    padding: 12px 8px;
  }

  .app-title {
    font-weight: 600;
    font-size: 13px;
    color: var(--text-secondary);
    padding: 4px 8px 12px 8px;
    letter-spacing: 0.02em;
  }

  .nav-section-label {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    padding: 4px 8px 6px 8px;
  }

  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 8px;
    border-radius: var(--radius-sm);
    font-size: 13px;
    color: var(--text-secondary);
    transition: background-color 0.12s ease, color 0.12s ease;
    text-align: left;
  }

  .nav-item:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .nav-item.active {
    background-color: var(--accent-light);
    color: var(--accent);
    font-weight: 500;
  }

  .nav-icon {
    font-size: 14px;
    width: 18px;
    text-align: center;
    flex-shrink: 0;
  }

  .nav-label {
    flex: 1;
  }

  .sidebar-footer {
    border-top: 1px solid var(--border);
    padding: 8px;
    display: flex;
    gap: 4px;
  }

  .footer-btn {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 6px 4px;
    border-radius: var(--radius-sm);
    font-size: 10px;
    color: var(--text-muted);
    transition: background-color 0.12s ease, color 0.12s ease;
  }

  .footer-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }
</style>
