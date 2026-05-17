<!-- src/lib/components/rich_editor/FindPanel.svelte -->
<script lang="ts">
  import type { Editor } from '@tiptap/core';

  interface Props {
    editor: Editor | null;
    open: boolean;
    readonly?: boolean;
    onClose: () => void;
  }

  let { editor, open, readonly = false, onClose }: Props = $props();

  let findText = $state('');
  let replaceText = $state('');
  let caseSensitive = $state(false);
  let showReplace = $state(false);
  let findInput: HTMLInputElement | null = $state(null);

  $effect(() => {
    if (open && findInput) {
      findInput.focus();
      findInput.select();
    }
  });

  // When the panel is closed, suppress collapse of replace row so the next
  // open starts from a clean state. Also clear the search term to remove
  // highlight decorations.
  $effect(() => {
    if (!editor) return;
    if (!open) {
      editor.commands.setSearchTerm('');
      editor.commands.setReplaceTerm('');
      return;
    }
    // Push current search term into the extension on change.
    editor.commands.setSearchTerm(findText);
    editor.commands.setReplaceTerm(replaceText);
    editor.commands.setCaseSensitive(caseSensitive);
  });

  // If we're in readonly, never show the Replace row.
  $effect(() => {
    if (readonly && showReplace) showReplace = false;
  });

  function next() {
    if (!editor || !findText) return;
    editor.commands.nextSearchResult();
  }
  function prev() {
    if (!editor || !findText) return;
    editor.commands.previousSearchResult();
  }
  function replaceOne() {
    if (!editor || !findText || readonly) return;
    editor.commands.replace();
  }
  function replaceAll() {
    if (!editor || !findText || readonly) return;
    editor.commands.replaceAll();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (e.shiftKey) prev(); else next();
    }
  }
</script>

{#if open}
  <div class="find-panel" role="dialog" aria-label="Find and Replace">
    <div class="row">
      <input
        bind:this={findInput}
        bind:value={findText}
        type="text"
        placeholder="Find"
        aria-label="Find text"
        onkeydown={onKey}
      />
      <button type="button" aria-label="Previous match" title="Previous match (Shift+Enter)" onclick={prev}>↑</button>
      <button type="button" aria-label="Next match" title="Next match (Enter)" onclick={next}>↓</button>
      <label class="opt" title="Match case">
        <input type="checkbox" bind:checked={caseSensitive} /> Aa
      </label>
      {#if !readonly}
        <button type="button" aria-label="Toggle replace row" title="Toggle replace"
          onclick={() => (showReplace = !showReplace)}>↕</button>
      {/if}
      <button type="button" aria-label="Close find panel" title="Close (Esc)" onclick={onClose}>✕</button>
    </div>
    {#if showReplace && !readonly}
      <div class="row">
        <input
          bind:value={replaceText}
          type="text"
          placeholder="Replace with"
          aria-label="Replace text"
          onkeydown={onKey}
        />
        <button type="button" onclick={replaceOne}>Replace</button>
        <button type="button" onclick={replaceAll}>All</button>
      </div>
    {/if}
  </div>
{/if}

<style>
  .find-panel {
    position: absolute;
    top: 8px;
    right: 16px;
    z-index: 10;
    background: var(--bg-secondary);
    border: 1px solid var(--border-primary);
    border-radius: 6px;
    padding: 6px 8px;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .row { display: flex; align-items: center; gap: 4px; }
  .row input[type="text"] {
    background: var(--bg-primary);
    color: var(--text-primary);
    border: 1px solid var(--border-primary);
    border-radius: 4px;
    padding: 3px 6px;
    font-size: 13px;
    width: 180px;
  }
  .row button {
    background: transparent;
    color: var(--text-primary);
    border: 1px solid var(--border-primary);
    border-radius: 4px;
    padding: 2px 8px;
    font-size: 12px;
    cursor: pointer;
  }
  .row button:hover { background-color: var(--bg-tertiary); }
  .opt { font-size: 12px; display: flex; align-items: center; gap: 2px; }
</style>
