<script lang="ts">
  import PatientContextStructuredTab from './PatientContextStructuredTab.svelte';
  import PatientContextNotesTab from './PatientContextNotesTab.svelte';
  import OcrDropZone, { type OcrFileStatus } from '../../components/OcrDropZone.svelte';

  type Tab = 'structured' | 'notes';

  type Props = {
    contextText: string;
    medicationsText: string;
    allergiesText: string;
    conditionsText: string;
    open: boolean;
    width: number;
    onToggle: () => void;
    ocrFiles: OcrFileStatus[];
    ocrText: string;
    ocrLoading: boolean;
    onOcrFilesSelected: (paths: string[]) => void;
    onOcrTextChange: (text: string) => void;
    onRemoveOcrFile: (id: string) => void;
  };
  let {
    contextText = $bindable(''),
    medicationsText = $bindable(''),
    allergiesText = $bindable(''),
    conditionsText = $bindable(''),
    open,
    width,
    onToggle,
    ocrFiles = [],
    ocrText = '',
    ocrLoading = false,
    onOcrFilesSelected = () => {},
    onOcrTextChange = () => {},
    onRemoveOcrFile = () => {},
  }: Props = $props();

  let activeTab: Tab = $state('structured');

  const structuredHasContent = $derived(
    medicationsText.trim().length > 0 ||
      allergiesText.trim().length > 0 ||
      conditionsText.trim().length > 0,
  );
  const notesHasContent = $derived(contextText.trim().length > 0);
  const anyContent = $derived(structuredHasContent || notesHasContent);
</script>

{#if open}
  <aside
    class="sidebar"
    style="width: {width}px"
    aria-label="Patient context sidebar"
  >
    <header class="sidebar-header">
      <h2 class="sidebar-title">Patient Context</h2>
      <button
        class="toggle-btn"
        aria-label="Hide patient context sidebar"
        aria-expanded="true"
        onclick={onToggle}
        title="Hide patient context"
      >
        ▶
      </button>
    </header>

    <div class="tabs-row" role="tablist">
      <button
        role="tab"
        id="tab-structured"
        aria-selected={activeTab === 'structured'}
        aria-controls="panel-structured"
        class="tab-button"
        class:active={activeTab === 'structured'}
        onclick={() => (activeTab = 'structured')}
      >
        Structured
        {#if structuredHasContent}
          <span class="dot" aria-label="has content">●</span>
        {/if}
      </button>
      <button
        role="tab"
        id="tab-notes"
        aria-selected={activeTab === 'notes'}
        aria-controls="panel-notes"
        class="tab-button"
        class:active={activeTab === 'notes'}
        onclick={() => (activeTab = 'notes')}
      >
        Notes
        {#if notesHasContent}
          <span class="dot" aria-label="has content">●</span>
        {/if}
      </button>
    </div>

    <div class="tab-content">
      {#if activeTab === 'structured'}
        <div role="tabpanel" id="panel-structured" aria-labelledby="tab-structured" class="panel">
          <PatientContextStructuredTab
            bind:medicationsText
            bind:allergiesText
            bind:conditionsText
          />
        </div>
      {:else}
        <div role="tabpanel" id="panel-notes" aria-labelledby="tab-notes" class="panel">
          <PatientContextNotesTab bind:contextText />
        </div>
      {/if}
    </div>

    <div class="sidebar-ocr">
      <OcrDropZone
        {ocrFiles}
        {ocrText}
        {ocrLoading}
        {onOcrFilesSelected}
        {onOcrTextChange}
        {onRemoveOcrFile}
      />
    </div>
  </aside>
{:else}
  <button
    class="rail"
    aria-label="Show patient context sidebar"
    aria-expanded="false"
    onclick={onToggle}
    title="Show patient context"
  >
    <span class="rail-arrow">◀</span>
    <span class="rail-label">Patient Context</span>
    {#if anyContent}
      <span class="rail-dot" aria-label="has content">●</span>
    {/if}
  </button>
{/if}

<style>
  .sidebar {
    background: var(--bg-secondary);
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    flex: 0 0 auto;
    min-width: 0;
    overflow: hidden;
  }

  .sidebar-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
  }

  .sidebar-title {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .toggle-btn {
    background: transparent;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    padding: 4px 8px;
    border-radius: 3px;
    font-size: 11px;
  }

  .toggle-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .tabs-row {
    display: flex;
    border-bottom: 1px solid var(--border);
  }

  .tab-button {
    flex: 1;
    background: transparent;
    border: none;
    border-bottom: 2px solid transparent;
    padding: 8px 12px;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 12px;
    margin-bottom: -1px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }

  .tab-button:hover {
    color: var(--text-primary);
  }

  .tab-button.active {
    color: var(--text-primary);
    border-bottom-color: var(--accent);
  }

  .dot {
    font-size: 8px;
    color: #34d399;
    line-height: 1;
  }

  .tab-content {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }

  .sidebar-ocr {
    padding: 0 12px 12px;
    border-top: 1px solid var(--border);
    margin-top: 8px;
    overflow-y: auto;
    flex: 0 1 auto;
    max-height: 30%;
  }

  .panel {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .rail {
    flex: 0 0 28px;
    background: var(--bg-secondary);
    border: none;
    border-left: 1px solid var(--border);
    color: var(--text-secondary);
    cursor: pointer;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: flex-start;
    padding: 12px 0;
    gap: 10px;
    font-size: 11px;
  }

  .rail:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .rail-arrow {
    font-size: 12px;
  }

  .rail-label {
    writing-mode: vertical-rl;
    transform: rotate(180deg);
    letter-spacing: 0.5px;
    white-space: nowrap;
  }

  .rail-dot {
    font-size: 10px;
    color: #34d399;
    line-height: 1;
  }
</style>
