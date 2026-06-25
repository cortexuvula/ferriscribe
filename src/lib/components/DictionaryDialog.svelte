<script lang="ts">
  import { getSpellchecker } from './rich_editor/spellcheck/spellchecker';
  import { requestSpellcheckRescan } from './rich_editor/spellcheck/spellcheck_extension';
  import { listUserDict } from '../api/userDictionary';
  import { toasts } from '../stores/toasts.svelte';
  import { onEscape } from '../actions/onEscape';

  interface Props {
    open: boolean;
    onclose: () => void;
  }

  let { open, onclose }: Props = $props();

  let words = $state<string[]>([]);
  let loading = $state(false);
  let searchText = $state('');
  let newWord = $state('');
  let addError = $state('');

  // Escape-to-close is handled by the onEscape action (see <svelte:window>
  // below). The open guard prevents close when the dialog is hidden.

  async function loadWords() {
    loading = true;
    try {
      words = await listUserDict();
    } catch (err) {
      console.error('Failed to load user dictionary:', err);
      toasts.error(`Failed to load dictionary: ${err}`);
    } finally {
      loading = false;
    }
  }

  async function handleAdd() {
    const trimmed = newWord.trim();
    if (!trimmed) return;
    addError = '';
    try {
      const added = await getSpellchecker().addToUserDict(trimmed);
      if (!added) {
        addError = `"${trimmed}" is already in the dictionary.`;
        return;
      }
      newWord = '';
      await loadWords();
      requestSpellcheckRescan();
    } catch (err) {
      console.error('Failed to add word:', err);
      addError = String(err) || 'Failed to add word.';
    }
  }

  async function handleRemove(word: string) {
    try {
      await getSpellchecker().removeFromUserDict(word);
      await loadWords();
      requestSpellcheckRescan();
    } catch (err) {
      console.error('Failed to remove word:', err);
      toasts.error(`Failed to remove word: ${err}`);
    }
  }

  function onAddKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleAdd();
    }
  }

  $effect(() => {
    if (open) loadWords();
  });

  const filtered = $derived(
    searchText.trim()
      ? words.filter((w) => w.toLowerCase().includes(searchText.trim().toLowerCase()))
      : words,
  );
</script>

<svelte:window use:onEscape={() => open && onclose()} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="dict-overlay" onclick={onclose}>
    <div class="dict-dialog" onclick={(e) => e.stopPropagation()}>
      <div class="dict-header">
        <h2>Manage Dictionary</h2>
        <button class="btn-close" aria-label="Close" onclick={onclose}>&times;</button>
      </div>

      <div class="dict-add">
        <input
          class="add-input"
          type="text"
          placeholder="Add a word…"
          bind:value={newWord}
          onkeydown={onAddKeydown}
        />
        <button class="btn-add" onclick={handleAdd} disabled={!newWord.trim()}>+ Add</button>
      </div>
      {#if addError}
        <div class="dict-error">{addError}</div>
      {/if}

      <div class="dict-toolbar">
        <input
          class="search-input"
          type="text"
          placeholder="Search dictionary…"
          bind:value={searchText}
        />
      </div>

      <div class="dict-body">
        {#if loading}
          <p class="state-text">Loading…</p>
        {:else if filtered.length === 0}
          <p class="state-text">
            {words.length === 0
              ? 'No words in the dictionary yet. Add one above or right-click a misspelled word in the editor.'
              : 'No matches.'}
          </p>
        {:else}
          <ul class="word-list">
            {#each filtered as word (word)}
              <li class="word-row">
                <span class="word">{word}</span>
                <button
                  class="btn-remove"
                  aria-label={`Remove ${word}`}
                  onclick={() => handleRemove(word)}
                >Remove</button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>

      <div class="dict-footer">
        <span class="footer-count">
          {filtered.length} shown{searchText ? ` of ${words.length}` : ''}
        </span>
      </div>
    </div>
  </div>
{/if}

<style>
  .dict-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }
  .dict-dialog {
    background: var(--bg-secondary, #1e1e1e);
    color: var(--text-primary, #e0e0e0);
    border-radius: 8px;
    width: 90vw;
    max-width: 560px;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }

  .dict-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 14px 20px;
    border-bottom: 1px solid var(--border-color, #333);
    flex: 0 0 auto;
  }
  .dict-header h2 { margin: 0; font-size: 1.1rem; font-weight: 600; }
  .btn-close {
    background: none;
    border: none;
    color: var(--text-secondary, #aaa);
    font-size: 1.4rem;
    line-height: 1;
    padding: 4px 8px;
    cursor: pointer;
    border-radius: 4px;
  }
  .btn-close:hover { background: rgba(255, 255, 255, 0.08); }

  .dict-add {
    display: flex;
    gap: 8px;
    padding: 12px 20px 8px;
    flex: 0 0 auto;
  }
  .add-input {
    flex: 1 1 auto;
    min-width: 0;
    padding: 6px 10px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: var(--bg-primary, #111);
    color: var(--text-primary, #e0e0e0);
    font-size: 0.9rem;
  }
  .btn-add {
    flex: 0 0 auto;
    padding: 6px 14px;
    border-radius: 4px;
    border: none;
    background: var(--accent-color, #4a9eff);
    color: white;
    cursor: pointer;
    white-space: nowrap;
    font-size: 0.9rem;
  }
  .btn-add:disabled { opacity: 0.5; cursor: not-allowed; }
  .btn-add:not(:disabled):hover { filter: brightness(1.1); }

  .dict-error {
    color: #ff6b6b;
    padding: 0 20px 8px;
    font-size: 0.85rem;
  }

  .dict-toolbar {
    padding: 4px 20px 10px;
    border-bottom: 1px solid var(--border-color, #333);
    flex: 0 0 auto;
  }
  .search-input {
    width: 100%;
    padding: 6px 10px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: var(--bg-primary, #111);
    color: var(--text-primary, #e0e0e0);
    font-size: 0.9rem;
    box-sizing: border-box;
  }

  .dict-body {
    flex: 1 1 auto;
    overflow-y: auto;
    min-height: 0;
    padding: 6px 20px 12px;
  }
  .state-text {
    text-align: center;
    color: var(--text-secondary, #888);
    padding: 24px 8px;
    font-size: 0.9rem;
  }
  .word-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .word-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 6px 4px;
    border-bottom: 1px solid var(--border-color, #222);
    gap: 12px;
  }
  .word {
    font-family: 'SF Mono', Menlo, Consolas, monospace;
    font-size: 0.9rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .btn-remove {
    flex: 0 0 auto;
    padding: 3px 10px;
    border-radius: 3px;
    border: 1px solid #ff6b6b44;
    background: transparent;
    color: #ff6b6b;
    cursor: pointer;
    font-size: 0.78rem;
  }
  .btn-remove:hover { background: rgba(255, 107, 107, 0.08); }

  .dict-footer {
    padding: 10px 20px;
    border-top: 1px solid var(--border-color, #333);
    flex: 0 0 auto;
  }
  .footer-count {
    font-size: 0.82rem;
    color: var(--text-secondary, #888);
  }
</style>
