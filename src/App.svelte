<script lang="ts">
  import './app.css';
  import { onMount, onDestroy } from 'svelte';
  import { settings } from './lib/stores/settings';
  import { theme } from './lib/stores/theme';
  import { generation } from './lib/stores/generation';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { onOpenUrl } from '@tauri-apps/plugin-deep-link';

  import Sidebar from './lib/components/Sidebar.svelte';
  import StatusBar from './lib/components/StatusBar.svelte';
  import StatusBadge from './lib/components/StatusBadge.svelte';
  import SettingsDialog from './lib/dialogs/SettingsDialog.svelte';
  import DatabaseRecoveryDialog from './lib/dialogs/DatabaseRecoveryDialog.svelte';
  import EndpointOfflineDialog from './lib/components/EndpointOfflineDialog.svelte';
  import { settingsNav } from './lib/stores/settingsNav';
  import type { ServiceKind } from './lib/api/invokeWithOfflineHandling';
  import { selectedRecording, selectRecording } from './lib/stores/recordings';
  import { pipeline } from './lib/stores/pipeline';
  import { audio } from './lib/stores/audio';
  import { toasts } from './lib/stores/toasts';
  import ToastContainer from './lib/components/ToastContainer.svelte';
  import RsvpReader from './lib/components/RsvpReader.svelte';
  import RsvpSectionPicker from './lib/components/RsvpSectionPicker.svelte';
  import { rsvp } from './lib/stores/rsvp';
  import { get } from 'svelte/store';

  // Pages
  import RecordTab from './lib/pages/RecordTab.svelte';
  import RecordingsTab from './lib/pages/RecordingsTab.svelte';
  import GenerateTab from './lib/pages/GenerateTab.svelte';
  import ChatTab from './lib/pages/ChatTab.svelte';
  import EditorTab from './lib/pages/EditorTab.svelte';

  let activeTab = $state('record');
  let settingsOpen = $state(false);
  let previousTab = $state('record');

  /** Open Settings dialog and navigate to the pane relevant to the offline service. */
  function onEndpointOfflineOpenSettings(service: ServiceKind) {
    settingsOpen = true;
    settingsNav.navigateTo(service === 'AiProvider' ? 'models' : 'audio');
  }

  /** Open Settings dialog and navigate to the pane indicated by the health pill. */
  function onEndpointHealthOpenSettings(target: 'models' | 'audio') {
    settingsOpen = true;
    settingsNav.navigateTo(target);
  }

  // Database recovery dialog state. The backend always registers
  // `RecoveryState` (Some(reason) on recovery, None on normal boot), so we
  // query it on mount instead of subscribing to a timing-race event.
  let recoveryReason = $state<string | null>(null);

  // Intercept settings tab — open modal instead of navigating
  $effect(() => {
    if (activeTab === 'settings') {
      settingsOpen = true;
      activeTab = previousTab;
    } else {
      previousTab = activeTab;
    }
  });

  let progressUnlisten: UnlistenFn | null = null;
  let pipelineCompleteUnlisten: UnlistenFn | null = null;
  let pipelineFailedUnlisten: UnlistenFn | null = null;
  let settingsUnsubscribe: (() => void) | null = null;
  let onGlobalKeydown: ((e: KeyboardEvent) => void) | null = null;

  async function navigateToSoap(tab: string, recordingId: string) {
    await selectRecording(recordingId);
    activeTab = tab;
  }

  onMount(async () => {
    // Query recovery state first. If the backend signaled recovery is
    // needed, AppState was not registered, so further init calls would
    // fail. Render only the recovery dialog in that case.
    try {
      recoveryReason = await invoke<string | null>('get_database_recovery_state');
    } catch (e) {
      console.error('Failed to query recovery state:', e);
    }
    if (recoveryReason) {
      return;
    }

    // Tear down any prior listeners (Vite HMR re-runs onMount without onDestroy)
    progressUnlisten?.();
    pipelineCompleteUnlisten?.();
    pipelineFailedUnlisten?.();
    pipeline.destroy();

    // Listen for generation progress events globally so state persists across tab switches
    progressUnlisten = await listen<{ type: string; status: string }>(
      'generation-progress',
      (event) => {
        generation.setProgress(`${event.payload.type}: ${event.payload.status}`);
      }
    );

    await settings.load();
    // Set theme from loaded settings
    settingsUnsubscribe = settings.subscribe((cfg) => {
      theme.set(cfg.theme);
    });

    onGlobalKeydown = (e: KeyboardEvent) => {
      const cmdOrCtrl = e.metaKey || e.ctrlKey;
      if (!(cmdOrCtrl && e.shiftKey && (e.key === 'r' || e.key === 'R'))) return;
      e.preventDefault();
      // Already open — don't stack another reader/picker on top.
      const rsvpState = get(rsvp);
      if (rsvpState.reader.open || rsvpState.picker.open) return;
      const rec = $selectedRecording;
      if (!rec) return;
      // Respect the active tab so editor users speed-read the doc they see.
      if (activeTab === 'soap' && rec.soap_note) {
        rsvp.openSoap(rec.soap_note);
      } else if (activeTab === 'referral' && rec.referral) {
        rsvp.openGeneric(rec.referral, 'referral');
      } else if (activeTab === 'letter' && rec.letter) {
        rsvp.openGeneric(rec.letter, 'letter');
      } else if (activeTab === 'transcript') {
        // Spec: transcripts are excluded from RSVP.
      } else if (rec.soap_note) {
        // Fallback for 'record' / 'generate' / other: prefer SOAP.
        rsvp.openSoap(rec.soap_note);
      }
    };
    window.addEventListener('keydown', onGlobalKeydown);

    // Register deep-link handler for ferriscribe:// URLs.
    // Dispatches a custom event so the pairing screen (ClientPair.svelte)
    // can handle it without coupling directly to this root component.
    try {
      onOpenUrl((urls) => {
        const url = urls[0];
        if (url?.startsWith('ferriscribe://pair?')) {
          window.dispatchEvent(new CustomEvent('ferriscribe-pair-url', { detail: url }));
        }
      });
    } catch {
      // Plugin not available in dev/non-Tauri context; paste path still works.
    }

    await pipeline.init();

    // Recover orphan recording state (e.g. after a webview reload left the
    // backend capture running while the frontend thinks it's idle).
    await audio.rehydrate();

    pipelineCompleteUnlisten = await listen<{ recording_id: string; display_name: string }>(
      'pipeline-complete',
      (event) => {
        const { recording_id, display_name } = event.payload;
        toasts.add({
          message: `SOAP note ready for ${display_name}`,
          type: 'success',
          recordingId: recording_id,
          displayName: display_name,
          autoDismiss: true,
        });
      },
    );

    pipelineFailedUnlisten = await listen<{ recording_id: string; stage: string; error?: string }>(
      'pipeline-progress',
      (event) => {
        if (event.payload.stage === 'failed') {
          toasts.add({
            message: `Processing failed: ${event.payload.error ?? 'Unknown error'}`,
            type: 'error',
            recordingId: event.payload.recording_id,
            autoDismiss: false,
          });
        }
      },
    );
  });

  onDestroy(() => {
    if (onGlobalKeydown) window.removeEventListener('keydown', onGlobalKeydown);
    progressUnlisten?.();
    settingsUnsubscribe?.();
    pipeline.destroy();
    pipelineCompleteUnlisten?.();
    pipelineFailedUnlisten?.();
  });
</script>

{#if recoveryReason}
  <DatabaseRecoveryDialog reason={recoveryReason} />
{:else}
<div class="app-shell">
  <aside class="app-sidebar">
    <Sidebar bind:activeTab />
  </aside>

  <main class="app-content">
    {#if $selectedRecording}
      <div class="selected-recording-banner">
        <span class="banner-icon">🎙</span>
        <span class="banner-name">{$selectedRecording.patient_name || $selectedRecording.filename}</span>
        <span class="banner-meta">{new Date($selectedRecording.created_at).toLocaleDateString()}</span>
      </div>
    {/if}
    {#if activeTab === 'record'}
      <RecordTab onopenSettings={onEndpointHealthOpenSettings} />
    {:else if activeTab === 'recordings'}
      <RecordingsTab />
    {:else if activeTab === 'generate'}
      <GenerateTab />
    {:else if activeTab === 'chat'}
      <ChatTab />
    {:else if activeTab === 'transcript'}
      <EditorTab tabId="transcript" />
    {:else if activeTab === 'soap'}
      <EditorTab tabId="soap" />
    {:else if activeTab === 'referral'}
      <EditorTab tabId="referral" />
    {:else if activeTab === 'letter'}
      <EditorTab tabId="letter" />
    {/if}
  </main>

  <footer class="app-statusbar">
    <StatusBar onopenSettings={onEndpointHealthOpenSettings} />
    <StatusBadge />
  </footer>

  <ToastContainer onNavigate={navigateToSoap} />
</div>

<SettingsDialog bind:open={settingsOpen} />

<RsvpSectionPicker />
<RsvpReader />

<EndpointOfflineDialog onopenSettings={onEndpointOfflineOpenSettings} />
{/if}

<style>
  .app-shell {
    display: grid;
    grid-template-columns: var(--sidebar-width) 1fr;
    grid-template-rows: 1fr var(--statusbar-height);
    height: 100vh;
    overflow: hidden;
  }

  .app-sidebar {
    grid-column: 1;
    grid-row: 1;
    overflow: hidden;
  }

  .app-content {
    grid-column: 2;
    grid-row: 1;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    background-color: var(--bg-primary);
  }

  .app-statusbar {
    grid-column: 1 / -1;
    grid-row: 2;
  }

  .selected-recording-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 16px;
    background-color: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
    font-size: 12px;
    flex-shrink: 0;
  }

  .banner-icon {
    font-size: 14px;
  }

  .banner-name {
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .banner-meta {
    color: var(--text-muted);
    margin-left: auto;
    flex-shrink: 0;
  }
</style>
