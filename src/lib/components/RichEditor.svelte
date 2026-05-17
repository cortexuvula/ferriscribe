<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Editor } from '@tiptap/core';
  import StarterKit from '@tiptap/starter-kit';
  import Underline from '@tiptap/extension-underline';
  import { Markdown } from 'tiptap-markdown';

  interface Props {
    value?: string;
    placeholder?: string;
    readonly?: boolean;
    onChange?: (v: string) => void;
  }

  let { value = '', placeholder = '', readonly = false, onChange = () => {} }: Props = $props();

  let editorEl: HTMLDivElement;
  let editor: Editor | null = null;

  onMount(() => {
    editor = new Editor({
      element: editorEl,
      extensions: [
        // History is enabled by default in StarterKit; no explicit config needed.
        StarterKit,
        Underline,
        Markdown.configure({ html: false, breaks: true }),
      ],
      editable: !readonly,
      content: value,
      editorProps: {
        attributes: {
          class: 'rich-editor-area',
          spellcheck: 'true',
        },
      },
      onUpdate: ({ editor }) => {
        // Read Markdown out via the Markdown extension storage.
        const md = (editor.storage as { markdown?: { getMarkdown: () => string } })
          .markdown?.getMarkdown() ?? editor.getText();
        onChange(md);
      },
    });
  });

  onDestroy(() => {
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

<div class="rich-editor" bind:this={editorEl} aria-label={placeholder || 'Editor'}></div>

<style>
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
</style>
