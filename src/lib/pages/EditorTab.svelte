<script lang="ts">
  import { onDestroy } from 'svelte';
  import type { Recording } from '../types';
  import { recordings } from '../stores/recordings.svelte';
  import { copyToClipboard } from '../utils/clipboard';
  import RichEditor from '../components/RichEditor.svelte';
  import TranscriptView from '../components/TranscriptView.svelte';
  import { rsvp } from '../stores/rsvp.svelte';
  import type { DocKind } from '../stores/rsvp.svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { formatError } from '../types/errors';
  import { extractIcdCodes } from '../rsvp/engine';

  let { tabId }: { tabId: 'transcript' | 'soap' | 'referral' | 'letter' | 'peer_discussion' } = $props();

  type TabConfig = { field: keyof Recording; label: string };

  const tabConfigs: Record<string, TabConfig> = {
    transcript:       { field: 'transcript',       label: 'Transcript' },
    soap:             { field: 'soap_note',         label: 'SOAP Note' },
    referral:         { field: 'referral',          label: 'Referral Letter' },
    letter:           { field: 'letter',            label: 'Patient Letter' },
    peer_discussion:  { field: 'peer_discussion',   label: 'Peer Discussion' },
  };

  const config = $derived(tabConfigs[tabId]);
  const content = $derived(
    recordings.selectedRecording
      ? (recordings.selectedRecording[config.field] as string | null) ?? ''
      : null
  );

  // Extract ICD codes from SOAP note content (only relevant for soap tab)
  const icdCodes = $derived(
    tabId === 'soap' && content && typeof content === 'string'
      ? extractIcdCodes(content)
      : []
  );

  // Structured transcript segments from recording metadata (stored by backend
  // during transcription). Used by TranscriptView for rich speaker display.
  // Validated with a type guard so a malformed payload renders as empty
  // rather than crashing TranscriptView on an unexpected shape.
  function isTranscriptSegments(
    v: unknown,
  ): v is Array<{ speaker: string | null; text: string; start: number; end: number }> {
    return (
      Array.isArray(v) &&
      v.every(
        (seg) =>
          typeof seg === 'object' &&
          seg !== null &&
          typeof (seg as Record<string, unknown>).text === 'string' &&
          typeof (seg as Record<string, unknown>).start === 'number' &&
          typeof (seg as Record<string, unknown>).end === 'number' &&
          ((seg as Record<string, unknown>).speaker === null ||
            typeof (seg as Record<string, unknown>).speaker === 'string'),
      )
    );
  }
  const transcriptSegments = $derived.by(() => {
    const raw = recordings.selectedRecording?.metadata?.transcript_segments;
    return isTranscriptSegments(raw) ? raw : undefined;
  });

  let copyStatus = $state<'idle' | 'copying' | 'copied'>('idle');
  let saveStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');
  let saveError: string | null = $state(null);

  // Debounce timer
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let clearBadgeTimer: ReturnType<typeof setTimeout> | null = null;
  let copyBadgeTimer: ReturnType<typeof setTimeout> | null = null;
  let pendingValue: string | null = null;

  onDestroy(() => {
    if (saveTimer) clearTimeout(saveTimer);
    if (clearBadgeTimer) clearTimeout(clearBadgeTimer);
    if (copyBadgeTimer) clearTimeout(copyBadgeTimer);
  });

  // Track which (recordingId, field) the current content belongs to.
  // When the user switches recordings or tabs we MUST NOT save the
  // previous tab's content under the new tab's key.
  let lastSeenKey: string | null = null;
  const currentKey = $derived(
    recordings.selectedRecording ? `${recordings.selectedRecording.id}::${String(config.field)}` : null
  );

  $effect(() => {
    // Whenever the key changes (different recording or different tab),
    // reset debounce state to prevent cross-contamination.
    if (currentKey !== lastSeenKey) {
      if (saveTimer !== null) {
        clearTimeout(saveTimer);
        saveTimer = null;
      }
      pendingValue = null;
      lastSeenKey = currentKey;
      saveStatus = 'idle';
      saveError = null;
    }
  });

  function onEditorChange(newValue: string) {
    if (!recordings.selectedRecording) return;
    // Avoid triggering saves on programmatic value binding (no actual edit).
    if (newValue === content) return;

    pendingValue = newValue;

    // Optimistic local update so the UI doesn't flicker.
    recordings.selectedRecording = {
      ...recordings.selectedRecording,
      [config.field]: newValue,
    };

    if (saveTimer !== null) clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      saveTimer = null;
      const value = pendingValue;
      pendingValue = null;
      if (value === null || !recordings.selectedRecording) return;
      saveStatus = 'saving';
      saveError = null;
      try {
        await invoke('save_recording_field', {
          recordingId: recordings.selectedRecording.id,
          field: String(config.field),
          value,
        });
        saveStatus = 'saved';
        // Clear the "Saved" badge after 1.5 s.
        clearBadgeTimer = setTimeout(() => {
          clearBadgeTimer = null;
          if (saveStatus === 'saved') saveStatus = 'idle';
        }, 1500);
      } catch (e) {
        saveStatus = 'error';
        saveError = formatError(e);
      }
    }, 1000); // 1 s debounce
  }

  async function handleCopy() {
    if (copyStatus !== 'idle') return;
    if (!content) return;
    copyStatus = 'copying';
    try {
      await copyToClipboard(content);
      copyStatus = 'copied';
      copyBadgeTimer = setTimeout(() => { copyBadgeTimer = null; copyStatus = 'idle'; }, 2000);
    } catch (e) {
      console.error('Failed to copy:', e);
      copyStatus = 'idle';
    }
  }

  function handleSpeedRead() {
    if (!content) return;
    const map: Record<string, DocKind> = {
      soap_note: 'soap',
      referral: 'referral',
      letter: 'letter',
      chat: 'letter', // chat/synopsis-like documents read generically
    };
    const kind: DocKind = map[config.field] ?? 'letter';
    if (kind === 'soap') {
      rsvp.openSoap(content);
    } else {
      rsvp.openGeneric(content, kind);
    }
  }
</script>

<div class="editor-tab">
  <div class="editor-header">
    <div class="editor-header-left">
      <h2 class="doc-type">{config.label}</h2>
      {#if recordings.selectedRecording?.patient_name}
        <span class="patient-name">— {recordings.selectedRecording.patient_name}</span>
      {/if}
    </div>
    <div class="editor-header-right">
      {#if saveStatus === 'saving'}
        <span class="save-status saving">Saving…</span>
      {:else if saveStatus === 'saved'}
        <span class="save-status saved">Saved</span>
      {:else if saveStatus === 'error'}
        <span class="save-status error" title={saveError ?? undefined}>Save failed</span>
      {/if}
      {#if content}
        <button class="btn-copy" onclick={handleSpeedRead} title="Speed Read (Cmd/Ctrl+Shift+R)">
          Speed Read
        </button>
        <button
          class="btn-copy"
          class:copied={copyStatus === 'copied'}
          onclick={handleCopy}
          disabled={copyStatus !== 'idle'}
        >
          {#if copyStatus === 'copying'}
            Copying…
          {:else if copyStatus === 'copied'}
            Copied!
          {:else}
            Copy
          {/if}
        </button>
        {#if icdCodes.length > 0}
          <div class="icd-codes">
            <span class="icd-label">ICD:</span>
            {#each icdCodes as code}
              <span class="icd-code">{code}</span>
            {/each}
          </div>
        {/if}
      {/if}
    </div>
  </div>

  {#if content === null}
    <div class="empty-state">
      <div class="empty-icon">📄</div>
      <h3>No recording selected</h3>
      <p>Select a recording from the <strong>Recordings</strong> tab to view its {config.label.toLowerCase()}.</p>
    </div>
  {:else if content === ''}
    <div class="empty-state">
      <div class="empty-icon">✏</div>
      <h3>No {config.label} yet</h3>
      <p>Go to the <strong>Generate</strong> tab to create this document.</p>
    </div>
  {:else}
    {#if tabId === 'transcript'}
      <TranscriptView value={content} segments={transcriptSegments} placeholder="No content…" onChange={onEditorChange} />
    {:else}
      <RichEditor value={content} placeholder="No content…" onChange={onEditorChange} />
    {/if}
  {/if}
</div>

<style>
  .editor-tab {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .editor-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    background-color: var(--bg-secondary);
    flex-shrink: 0;
  }

  .editor-header-left {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .editor-header-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .doc-type {
    font-size: 15px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .patient-name {
    font-size: 13px;
    color: var(--text-muted);
  }

  .save-status {
    font-size: 12px;
    font-weight: 500;
    padding: 3px 8px;
    border-radius: var(--radius-sm);
  }

  .save-status.saving {
    color: var(--text-muted, #888);
  }

  .save-status.saved {
    color: #059669;
    background-color: color-mix(in srgb, #059669 10%, transparent);
  }

  .save-status.error {
    color: #dc2626;
    cursor: help;
  }

  .btn-copy {
    padding: 5px 12px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease, border-color 0.15s ease;
  }

  .btn-copy:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-copy.copied {
    color: var(--success, #22c55e);
    border-color: var(--success, #22c55e);
    background-color: color-mix(in srgb, var(--success, #22c55e) 10%, transparent);
  }

  .empty-state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    padding: 40px;
    gap: 8px;
    color: var(--text-muted);
  }

  .empty-icon {
    font-size: 40px;
    margin-bottom: 8px;
  }

  h3 {
    font-size: 16px;
    font-weight: 500;
    color: var(--text-secondary);
  }

  p {
    font-size: 13px;
    line-height: 1.6;
  }

  strong {
    color: var(--text-secondary);
  }

  .icd-codes {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-left: 8px;
    padding: 4px 8px;
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 20%, transparent);
    border-radius: var(--radius-sm);
    flex-wrap: wrap;
  }

  .icd-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .icd-code {
    font-family: var(--font-mono);
    font-size: 12px;
    font-weight: 500;
    color: var(--accent);
    padding: 2px 6px;
    background: var(--bg-primary);
    border-radius: var(--radius-xs);
  }
</style>
