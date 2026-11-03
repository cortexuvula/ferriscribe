<script lang="ts">
  import { onMount } from 'svelte';
  import { recordings } from '../stores/recordings.svelte';
  import { settings } from '../stores/settings.svelte';
  import { toasts } from '../stores/toasts.svelte';
  import SearchBar from './SearchBar.svelte';
  import TrashRecordingRow from './TrashRecordingRow.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import RestoreByDateDialog from './RestoreByDateDialog.svelte';
  import { sanitizedErr } from '../utils/sanitizedErr';
  import type { TrashPurgePreview } from '../api/recordings';

  /// Trash view panel. Search state is LOCAL and separate from the Active
  /// view's — entering Trash always starts with an empty query. The search
  /// filters the loaded rows client-side (trashed rows are de-indexed from
  /// FTS, so there is no backend search); "N matches · M in Trash" keeps
  /// the authoritative total visible while filtered, and bulk actions
  /// always operate on ALL trashed records regardless of page or filter.

  let searchQuery = $state('');
  /// The visible input text, bound so "Clear search" empties BOTH the
  /// filter and the box (the debounced onSearch only flows one way).
  let searchInput = $state('');
  let showRestoreAll = $state(false);
  let showRestoreByDate = $state(false);
  const retentionDays = $derived(settings.trashRetentionDays);
  let now = $state(Date.now());
  // The panel is mounted only while Trash is open. One shared clock keeps
  // every row current, and its teardown prevents timers leaking on exit.
  onMount(() => {
    const timer = setInterval(() => { now = Date.now(); }, 60_000);
    return () => clearInterval(timer);
  });

  /// The ONE polite live region for trash-local results (D9): per-row
  /// restore completions land here — never ALSO as a toast (no double
  /// announce). Actionable failures use toasts (role=alert after D9).
  let statusMessage = $state('');

  const filtered = $derived.by(() => {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return recordings.trashedList;
    return recordings.trashedList.filter(
      (r) =>
        r.filename.toLowerCase().includes(q) ||
        (r.patient_name ?? '').toLowerCase().includes(q),
    );
  });

  function announce(msg: string) {
    statusMessage = msg;
  }

  async function retryLoad() {
    await recordings.loadTrashed();
  }

  function clearSearch() {
    searchQuery = '';
    searchInput = '';
  }

  /// Per-row restore with focus management (D9): the invoking row
  /// disappears when the refresh lands, so focus moves to the NEXT row's
  /// Restore button — or the Trash heading when it was the last row —
  /// never to document.body.
  async function restoreRow(id: string) {
    // Pick the fallback BEFORE the await: the row may leave `filtered`.
    const ids = filtered.map((r) => r.id);
    const nextId = ids[ids.indexOf(id) + 1];
    try {
      const count = await recordings.restoreTrashed([id]);
      announce(
        count > 0
          ? 'Recording restored to Active.'
          : 'Recording was no longer in Trash.',
      );
      requestAnimationFrame(() => {
        const next =
          (nextId !== undefined
            ? document.querySelector(`[data-restore-btn="${nextId}"]`)
            : null) ?? document.querySelector('[data-trash-heading]');
        (next as HTMLElement | null)?.focus();
      });
    } catch (err) {
      console.error('Failed to restore recording:', err);
      toasts.error(`Couldn't restore recording: ${sanitizedErr(err)}`);
    }
  }

  /// "Purge Now" (manual, exact-set permanent purge). The button first
  /// fetches the BACKEND preview — the authoritative all-DB candidate set,
  /// never the loaded page, the search filter, or the badge count — then
  /// shows a dangerous confirmation over the FROZEN preview. Records that
  /// land in Trash while the dialog is open are NOT in the reviewed set
  /// and require a new click; the backend re-verifies every tombstone
  /// identity inside the deletion transaction regardless.
  let showPurge = $state(false);
  let purgePreview = $state<TrashPurgePreview | null>(null);
  /// Destructive-phase busy flag (set only AFTER consent); the preview
  /// phase uses `previewing` so the label never implies deletion started.
  let purgeBusy = $state(false);
  /// True while the NON-destructive preview fetch is in flight.
  let previewing = $state(false);

  const purgeDisabled = $derived(
    recordings.trashedTotal === 0 ||
      recordings.trashedLoading ||
      recordings.trashedError !== null ||
      recordings.restoring ||
      recordings.purging ||
      purgeBusy ||
      previewing,
  );

  async function startPurge() {
    if (purgeDisabled) return;
    previewing = true;
    try {
      const preview = await recordings.previewTrashPurge();
      if (preview.candidates.length === 0) {
        // Nothing eligible — never open a destructive dialog, never claim
        // an empty Trash while protected rows remain.
        announce(
          preview.protectedCount > 0
            ? 'Nothing in Trash can be purged right now. Recordings still encrypting were kept.'
            : 'Trash is already empty.',
        );
        return;
      }
      purgePreview = preview; // FROZEN — the dialog confirms exactly this set
      showPurge = true;
    } catch (err) {
      console.error('Failed to preview Trash purge:', err);
      toasts.error(`Couldn't load the purge preview: ${sanitizedErr(err)}`);
    } finally {
      previewing = false;
    }
  }

  async function confirmPurge() {
    const candidates = purgePreview?.candidates;
    // Close the dialog BEFORE the await: a double-click hits nothing, and
    // the store's in-flight guard rejects any racing second call.
    showPurge = false;
    if (!candidates || candidates.length === 0) {
      purgePreview = null;
      return;
    }
    purgeBusy = true;
    let succeeded = false;
    try {
      const result = await recordings.purgeTrashNow(candidates);
      succeeded = true;
      const noun = (n: number) => (n === 1 ? 'recording' : 'recordings');
      const parts = [`${result.purged} ${noun(result.purged)} permanently deleted.`];
      if (result.protected > 0) {
        parts.push(
          `${result.protected} ${noun(result.protected)} still encrypting ${result.protected === 1 ? 'was' : 'were'} kept — try again later.`,
        );
      }
      if (result.changed > 0) {
        parts.push(
          `${result.changed} ${noun(result.changed)} changed in Trash ${result.changed === 1 ? 'was' : 'were'} left alone.`,
        );
      }
      if (result.cleanupPending) {
        // Durable committed intent — the background drain WILL finish
        // these (unlink failures, ack failures, crashes all recover).
        parts.push('Some audio cleanup continues in the background.');
      }
      // Remaining-count (Codie S1): only from the store's POST-purge
      // refresh, and only when that refresh succeeded — a failed reload
      // must not invent a count (trashedError is set in that case).
      if (recordings.trashedError === null && recordings.trashedTotal > 0) {
        parts.push(
          `${recordings.trashedTotal} ${noun(recordings.trashedTotal)} remain in Trash.`,
        );
      }
      // ONE polite announcement (D9) — never also a toast.
      announce(parts.join(' '));
    } catch (err) {
      console.error('Failed to purge Trash:', err);
      toasts.error(`Couldn't purge Trash: ${sanitizedErr(err)}`);
    } finally {
      purgePreview = null;
      purgeBusy = false;
      if (succeeded) {
        // The purge button may now be disabled (Trash emptied) — focus()
        // on a disabled button is a no-op, so move focus to the heading.
        requestAnimationFrame(() => {
          (
            document.querySelector('[data-trash-heading]') as HTMLElement | null
          )?.focus();
        });
      }
    }
  }

  /// "Restore all" — operates on EVERY trashed recording (the dialog says
  /// so), not the loaded page or the active search filter. Duplicate
  /// submissions are one restore: the dialog closes before the await and
  /// the store's in-flight guard rejects any racing second call.
  async function confirmRestoreAll() {
    showRestoreAll = false;
    try {
      const count = await recordings.restoreAllFromTrash();
      const noun = count === 1 ? 'recording' : 'recordings';
      announce(`${count} ${noun} restored to Active.`);
    } catch (err) {
      console.error('Failed to restore all recordings:', err);
      toasts.error(`Couldn't restore recordings: ${sanitizedErr(err)}`);
    }
  }
</script>

<div class="trash-panel">
  <h3
    class="trash-heading"
    data-trash-heading
    tabindex="-1"
    id="trash-heading"
  >
    Trash
  </h3>
  <p class="trash-explain">
    Recordings can be restored for {retentionDays} days in Trash. After that, cleanup permanently deletes their audio, transcripts, SOAP notes and generated documents.
  </p>

  <div class="trash-toolbar">
    <span class="trash-count">
      {recordings.trashedTotal}
      {recordings.trashedTotal === 1 ? 'recording' : 'recordings'} in Trash
    </span>
    <div class="trash-actions">
      <button
        class="btn-restore-date"
        onclick={() => (showRestoreByDate = true)}
        disabled={recordings.trashedTotal === 0}
      >
        Restore by deletion date…
      </button>
      <button
        class="btn-restore-all"
        onclick={() => (showRestoreAll = true)}
        disabled={recordings.trashedTotal === 0}
      >
        Restore all…
      </button>
      <button
        class="btn-purge-now"
        onclick={startPurge}
        disabled={purgeDisabled}
      >
        {previewing ? 'Checking…' : purgeBusy || recordings.purging ? 'Purging…' : 'Purge Now'}
      </button>
    </div>
  </div>

  <SearchBar
    bind:value={searchInput}
    placeholder="Search Trash…"
    label="Search Trash"
    inputId="trash-search-input"
    onSearch={(q) => (searchQuery = q)}
  />

  {#if searchQuery.trim() !== ''}
    <!-- Static match count — deliberately NOT a live region (D9: one polite
         status region per view, reserved for previews/completions). -->
    <p class="match-line">
      {filtered.length} {filtered.length === 1 ? 'match' : 'matches'} ·
      {recordings.trashedTotal} in Trash
    </p>
  {/if}

  <div class="trash-status sr-only" role="status" aria-live="polite">
    {statusMessage}
  </div>

  <div class="trash-list">
    {#if recordings.trashedLoading}
      <div class="state-msg">
        <span>Loading Trash…</span>
      </div>

    {:else if recordings.trashedError !== null}
      <!-- A load failure must NEVER render as an empty trash. -->
      <div class="state-msg">
        <p>Couldn't load Trash. Try again.</p>
        <button class="btn-retry" onclick={retryLoad}>Retry</button>
      </div>

    {:else if recordings.trashedTotal === 0}
      <div class="state-msg">
        <div class="state-icon">🗑️</div>
        <p>Trash is empty.</p>
        <p class="hint">
          Recordings are eligible for permanent deletion after {retentionDays} days in Trash.
        </p>
      </div>

    {:else if filtered.length === 0}
      <div class="state-msg">
        <p>No recordings in Trash match your search.</p>
        <button class="btn-retry" onclick={clearSearch}>Clear search</button>
      </div>

    {:else}
      {#each filtered as item (item.id)}
        <TrashRecordingRow
          item={item}
          {retentionDays}
          {now}
          onRestore={() => restoreRow(item.id)}
          restoreDisabled={recordings.restoring}
        />
      {/each}
      {#if recordings.trashedHasMore}
        <div class="load-more">
          <button
            class="btn-load-more"
            onclick={() => recordings.loadMoreTrashed()}
            disabled={recordings.trashedLoadingMore}
          >
            {recordings.trashedLoadingMore ? 'Loading…' : 'Load more'}
          </button>
        </div>
      {/if}
    {/if}
  </div>
</div>

{#if showRestoreAll && recordings.trashedTotal > 0}
  {@const restoreAllN = recordings.trashedTotal}
  {@const restoreAllNoun = restoreAllN === 1 ? 'recording' : 'recordings'}
  <ConfirmDialog
    open={true}
    title={`Restore all ${restoreAllN} ${restoreAllNoun} from Trash?`}
    message="They will return to Active with their saved audio and documents. This restores every recording in Trash, not just the ones currently listed."
    confirmLabel={`Restore all ${restoreAllN}`}
    danger={false}
    onConfirm={confirmRestoreAll}
    onCancel={() => (showRestoreAll = false)}
  />
{/if}

{#if showRestoreByDate && recordings.trashedTotal > 0}
  <RestoreByDateDialog
    onClose={() => (showRestoreByDate = false)}
    onAnnounce={announce}
  />
{/if}

{#if showPurge && purgePreview && purgePreview.candidates.length > 0}
  {@const purgeN = purgePreview.candidates.length}
  {@const purgeNoun = purgeN === 1 ? 'recording' : 'recordings'}
  {@const protectedN = purgePreview.protectedCount}
  <ConfirmDialog
    open={true}
    title={`Permanently delete all ${purgeN} ${purgeNoun} in Trash?`}
    message={`This permanently deletes every recording in Trash — including any outside your current search and loaded pages — erasing their audio, transcripts, SOAP notes and generated documents forever. There is no undo.${protectedN > 0 ? ` ${protectedN} ${protectedN === 1 ? 'recording' : 'recordings'} still encrypting will be kept.` : ''}`}
    confirmLabel={`Purge ${purgeN} ${purgeNoun} now`}
    danger={true}
    onConfirm={confirmPurge}
    onCancel={() => {
      showPurge = false;
      purgePreview = null;
    }}
  />
{/if}

<style>
  .trash-panel {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .trash-heading {
    font-size: 15px;
    font-weight: 600;
    color: var(--text-primary);
    padding: 12px 12px 0;
  }

  .trash-heading:focus {
    outline: none;
  }

  .trash-explain {
    font-size: 12px;
    color: var(--text-muted);
    padding: 4px 12px 8px;
    margin: 0;
    line-height: 1.5;
  }

  .trash-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border);
  }

  .trash-count {
    font-size: 12px;
    color: var(--text-muted);
  }

  .btn-restore-all,
  .btn-restore-date {
    padding: 4px 10px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .btn-purge-now {
    padding: 4px 10px;
    font-size: 12px;
    font-weight: 600;
    color: var(--danger, #ef4444);
    background-color: transparent;
    border: 1px solid var(--danger, #ef4444);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .trash-actions {
    display: flex;
    gap: 6px;
  }

  .btn-restore-all:hover:not(:disabled),
  .btn-restore-date:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-purge-now:hover:not(:disabled) {
    /* Contrast-preserving hover (N2): NO background tint — the tinted bg
       dropped the 12px/600 label to 3.88:1 on the light theme (below the
       4.5:1 AA bar for non-large text). Underline alone signals the hover
       without changing text/background colors. */
    text-decoration: underline;
  }

  .btn-restore-all:disabled,
  .btn-restore-date:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-purge-now:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .match-line {
    font-size: 12px;
    color: var(--text-muted);
    padding: 6px 12px;
    margin: 0;
    border-bottom: 1px solid var(--border);
  }

  .trash-list {
    flex: 1;
    overflow-y: auto;
  }

  .state-msg {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 40px 20px;
    text-align: center;
    color: var(--text-muted);
  }

  .state-icon {
    font-size: 40px;
    margin-bottom: 8px;
  }

  .state-msg p {
    font-size: 14px;
    margin: 0;
  }

  .hint {
    font-size: 12px;
  }

  .btn-retry {
    margin-top: 6px;
    padding: 6px 16px;
    font-size: 13px;
    font-weight: 500;
    color: var(--accent);
    background-color: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .btn-retry:hover {
    background-color: var(--bg-hover);
    border-color: var(--accent);
  }

  .load-more {
    display: flex;
    justify-content: center;
    padding: 16px 12px;
  }

  .btn-load-more {
    padding: 8px 24px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
  }

  .btn-load-more:hover:not(:disabled) {
    background-color: var(--bg-hover);
    border-color: var(--accent);
    color: var(--text-primary);
  }

  .btn-load-more:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
