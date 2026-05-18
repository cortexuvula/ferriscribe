<!--
  Custom contextmenu for misspelled words. Rendered by RichEditor in
  response to the Spellcheck extension's onContextMenu callback.

  PHI safety: word content is rendered to the DOM (it's user-visible text
  the user just typed) but never sent to console, logs, or telemetry.
-->
<script lang="ts">
  import type { Editor } from '@tiptap/core';
  import { getSpellchecker } from './spellchecker';
  import {
    requestSpellcheckRescan,
    type SpellcheckContextMenuRequest,
  } from './spellcheck_extension';

  interface Props {
    editor: Editor | null;
    request: SpellcheckContextMenuRequest | null;
    onClose: () => void;
  }

  let { editor, request, onClose }: Props = $props();

  const spell = getSpellchecker();

  // Recompute suggestions when the request changes.
  const suggestions = $derived(
    request ? spell.suggest(request.word, 5) : [],
  );

  function applySuggestion(s: string) {
    if (!editor || !request) return;
    editor
      .chain()
      .focus()
      .insertContentAt({ from: request.from, to: request.to }, s)
      .run();
    onClose();
  }

  async function addToDictionary() {
    if (!request) return;
    await spell.addToUserDict(request.word);
    // Re-scan so the squiggle clears on this and other occurrences.
    if (editor) requestSpellcheckRescan(editor.view);
    onClose();
  }

  function ignoreOnce() {
    if (!request) return;
    spell.ignoreInSession(request.word);
    if (editor) requestSpellcheckRescan(editor.view);
    onClose();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if request}
  <div
    class="spellcheck-menu"
    style="left: {request.clientX}px; top: {request.clientY}px"
    role="menu"
    aria-label="Spelling suggestions for {request.word}"
  >
    {#if suggestions.length === 0}
      <div class="empty">No suggestions</div>
    {:else}
      {#each suggestions as s (s)}
        <button type="button" role="menuitem" onclick={() => applySuggestion(s)}>
          {s}
        </button>
      {/each}
    {/if}
    <div class="sep" aria-hidden="true"></div>
    <button type="button" role="menuitem" onclick={addToDictionary}>
      Add &ldquo;{request.word}&rdquo; to dictionary
    </button>
    <button type="button" role="menuitem" onclick={ignoreOnce}>Ignore</button>
    <button type="button" role="menuitem" onclick={onClose}>Cancel</button>
  </div>
{/if}

<style>
  .spellcheck-menu {
    position: fixed;
    z-index: 100;
    min-width: 200px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.18);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .spellcheck-menu button {
    background: transparent;
    color: var(--text-primary);
    border: none;
    border-radius: 4px;
    padding: 6px 10px;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
    white-space: nowrap;
  }
  .spellcheck-menu button:hover {
    background-color: var(--bg-hover);
  }
  .empty {
    padding: 6px 10px;
    color: var(--text-muted);
    font-size: 13px;
    font-style: italic;
  }
  .sep {
    height: 1px;
    background-color: var(--border);
    margin: 4px 0;
  }
</style>
