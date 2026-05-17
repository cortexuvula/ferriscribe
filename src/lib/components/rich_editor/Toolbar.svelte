<!-- src/lib/components/rich_editor/Toolbar.svelte -->
<script lang="ts">
  import type { Editor } from '@tiptap/core';

  interface Props {
    editor: Editor | null;
    onFindClick?: () => void;
  }

  let { editor, onFindClick = () => {} }: Props = $props();

  // Reactive flag bag so buttons show "active" state. Tiptap doesn't expose
  // a built-in store; we poll via a tiny $effect on selection changes.
  let activeMarks = $state({
    bold: false,
    italic: false,
    underline: false,
    bulletList: false,
    orderedList: false,
    h1: false,
    h2: false,
    h3: false,
  });

  $effect(() => {
    if (!editor) return;
    const update = () => {
      activeMarks = {
        bold: editor.isActive('bold'),
        italic: editor.isActive('italic'),
        underline: editor.isActive('underline'),
        bulletList: editor.isActive('bulletList'),
        orderedList: editor.isActive('orderedList'),
        h1: editor.isActive('heading', { level: 1 }),
        h2: editor.isActive('heading', { level: 2 }),
        h3: editor.isActive('heading', { level: 3 }),
      };
    };
    editor.on('selectionUpdate', update);
    editor.on('transaction', update);
    update();
    return () => {
      editor.off('selectionUpdate', update);
      editor.off('transaction', update);
    };
  });

  function cmd(fn: (e: Editor) => void) {
    return () => {
      if (!editor) return;
      fn(editor);
      editor.commands.focus();
    };
  }
</script>

<div class="toolbar" role="toolbar" aria-label="Formatting">
  <button type="button" class:active={activeMarks.bold}
    aria-label="Bold" title="Bold (Cmd+B)"
    onclick={cmd((e) => e.chain().toggleBold().run())}><strong>B</strong></button>

  <button type="button" class:active={activeMarks.italic}
    aria-label="Italic" title="Italic (Cmd+I)"
    onclick={cmd((e) => e.chain().toggleItalic().run())}><em>I</em></button>

  <button type="button" class:active={activeMarks.underline}
    aria-label="Underline" title="Underline (Cmd+U)"
    onclick={cmd((e) => e.chain().toggleUnderline().run())}><u>U</u></button>

  <span class="sep" aria-hidden="true"></span>

  <button type="button" class:active={activeMarks.bulletList}
    aria-label="Bullet list" title="Bullet list"
    onclick={cmd((e) => e.chain().toggleBulletList().run())}>•&nbsp;List</button>

  <button type="button" class:active={activeMarks.orderedList}
    aria-label="Numbered list" title="Numbered list"
    onclick={cmd((e) => e.chain().toggleOrderedList().run())}>1.&nbsp;List</button>

  <span class="sep" aria-hidden="true"></span>

  <button type="button" class:active={activeMarks.h1}
    aria-label="Heading 1" title="Heading 1"
    onclick={cmd((e) => e.chain().toggleHeading({ level: 1 }).run())}>H1</button>

  <button type="button" class:active={activeMarks.h2}
    aria-label="Heading 2" title="Heading 2"
    onclick={cmd((e) => e.chain().toggleHeading({ level: 2 }).run())}>H2</button>

  <button type="button" class:active={activeMarks.h3}
    aria-label="Heading 3" title="Heading 3"
    onclick={cmd((e) => e.chain().toggleHeading({ level: 3 }).run())}>H3</button>

  <span class="sep" aria-hidden="true"></span>

  <button type="button" aria-label="Find and Replace" title="Find (Cmd+F)"
    onclick={onFindClick}>Find</button>
</div>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    background-color: var(--bg-secondary);
    flex-shrink: 0;
  }
  .toolbar button {
    background: transparent;
    color: var(--text-primary);
    border: 1px solid transparent;
    border-radius: 4px;
    padding: 4px 8px;
    font-size: 13px;
    cursor: pointer;
  }
  .toolbar button:hover { background-color: var(--bg-hover); }
  .toolbar button.active {
    background-color: var(--accent);
    color: var(--text-inverse);
  }
  .sep {
    display: inline-block;
    width: 1px;
    height: 18px;
    background-color: var(--border);
    margin: 0 4px;
  }
</style>
