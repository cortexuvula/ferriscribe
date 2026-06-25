<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Editor } from '@tiptap/core';
  import StarterKit from '@tiptap/starter-kit';
  import Underline from '@tiptap/extension-underline';
  import { Markdown } from 'tiptap-markdown';
  import { SearchAndReplace } from '@sereneinserenade/tiptap-search-and-replace';
  import Toolbar from './rich_editor/Toolbar.svelte';
  import FindPanel from './rich_editor/FindPanel.svelte';
  import { Spellcheck, type SpellcheckContextMenuRequest } from './rich_editor/spellcheck/spellcheck_extension';
  import SpellcheckMenu from './rich_editor/spellcheck/SpellcheckMenu.svelte';

  interface Props {
    value?: string;
    placeholder?: string;
    readonly?: boolean;
    onChange?: (v: string) => void;
  }

  const { value = '', placeholder = '', readonly = false, onChange = () => {} }: Props = $props();

  let editorEl: HTMLDivElement;
  // `editor` is $state so the reassignment in onMount flows to child components
  // (Toolbar) that consume it as a reactive prop.
  let editor = $state<Editor | null>(null);
  let findOpen = $state(false);
  let spellcheckRequest = $state<SpellcheckContextMenuRequest | null>(null);

  // Hoisted so onDestroy can detach the same handler instance.
  const onKeyDown = (e: KeyboardEvent) => {
    const metaOrCtrl = e.metaKey || e.ctrlKey;
    if (!metaOrCtrl) return;
    const key = e.key.toLowerCase();
    if (key === 'f' || key === 'h') {
      e.preventDefault();
      findOpen = true;
    }
  };

  onMount(() => {
    editor = new Editor({
      element: editorEl,
      extensions: [
        // History is enabled by default in StarterKit; no explicit config needed.
        StarterKit,
        Underline,
        Markdown.configure({ html: false, breaks: true }),
        SearchAndReplace.configure({
          searchResultClass: 'rich-editor-search-hit',
          disableRegex: false,
        }),
        Spellcheck.configure({
          onContextMenu: (req) => {
            spellcheckRequest = req;
          },
        }),
      ],
      editable: !readonly,
      content: value,
      editorProps: {
        attributes: {
          class: 'rich-editor-area',
          spellcheck: 'false',
        },
      },
      onUpdate: ({ editor }) => {
        // Read Markdown out via the Markdown extension storage.
        const md = (editor.storage as { markdown?: { getMarkdown: () => string } })
          .markdown?.getMarkdown() ?? editor.getText();
        onChange(md);
      },
    });
    editorEl.addEventListener('keydown', onKeyDown);
  });

  onDestroy(() => {
    editorEl?.removeEventListener('keydown', onKeyDown);
    editor?.destroy();
    editor = null;
  });

  // Update content when the value prop changes from outside (e.g. switching
  // recordings/tabs). Skip if the markdown round-trip matches what we already
  // have, to avoid clobbering the user's cursor mid-edit.
  $effect(() => {
    if (!editor) return;
    const current = (editor.storage as { markdown?: { getMarkdown: () => string } })
      .markdown?.getMarkdown() ?? '';
    if (value !== current) {
      editor.commands.setContent(value, false);
    }
  });

  // React to readonly toggle.
  $effect(() => {
    if (!editor) return;
    editor.setEditable(!readonly);
  });
</script>

<div class="rich-editor-wrapper">
  {#if !readonly}
    <Toolbar {editor} onFindClick={() => (findOpen = true)} />
  {/if}
  <div class="rich-editor-host">
    <FindPanel
      {editor}
      open={findOpen}
      {readonly}
      onClose={() => (findOpen = false)}
    />
    <div class="rich-editor" bind:this={editorEl} aria-label={placeholder || 'Editor'}></div>
    <SpellcheckMenu
      {editor}
      request={spellcheckRequest}
      onClose={() => (spellcheckRequest = null)}
    />
  </div>
</div>

<style>
  .rich-editor-wrapper {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-height: 0;
  }

  .rich-editor-host {
    position: relative;
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-height: 0;
  }

  .rich-editor {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-height: 0;
  }

  :global(.rich-editor-area) {
    flex: 1;
    width: 100%;
    height: 100%;
    padding: 16px;
    font-size: 14px;
    line-height: 1.6;
    background-color: var(--bg-primary);
    color: var(--text-primary);
    outline: none;
    overflow-y: auto;
  }

  :global(.rich-editor-area p) { margin: 0 0 0.5em 0; }
  :global(.rich-editor-area p:last-child) { margin-bottom: 0; }
  :global(.rich-editor-area ul),
  :global(.rich-editor-area ol) { margin: 0 0 0.5em 1.5em; }

  /* Search & Replace highlight. The package adds the base class to every
     match and `<base>-current` to the currently-selected one. */
  :global(.rich-editor-search-hit) {
    background-color: rgba(255, 215, 0, 0.4);
  }
  :global(.rich-editor-search-hit-current) {
    background-color: rgba(255, 165, 0, 0.7);
  }

  :global(.spellcheck-misspelled) {
    text-decoration: underline wavy var(--accent, #d33);
    text-decoration-skip-ink: none;
    text-underline-offset: 2px;
  }
</style>
