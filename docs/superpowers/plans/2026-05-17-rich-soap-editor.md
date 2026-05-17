# Rich Editor for SOAP / Referral / Letter — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `<textarea>` for SOAP / Referral / Letter tabs with a Tiptap rich-text editor (Markdown-persisted), preserving the existing save flow, training corpus integration, FTS5 search, and PHI-free logging. Transcript tab unchanged.

**Architecture:** Tiptap v2 + `tiptap-markdown` for serialization. New `RichEditor.svelte` component slot-replaces `<TextEditor>` in `EditorTab.svelte` based on `tabId`. Toolbar and Find & Replace panel are sub-components. Existing save/debounce logic and `save_recording_field` command are untouched — frontend still sends a plain string (Markdown).

**Tech Stack:** Svelte 5 runes, Tiptap v2, `tiptap-markdown`, `tiptap-extension-search-and-replace`, Vite, Vitest.

---

## Task 1: Add Tiptap dependencies and create RichEditor skeleton

**Files:**
- Modify: `package.json` (add deps)
- Create: `src/lib/components/RichEditor.svelte`
- Create: `src/lib/components/rich_editor/.gitkeep` (placeholder so the directory exists)

- [ ] **Step 1: Add Tiptap dependencies**

In `package.json`, add to `dependencies`:

```json
"@tiptap/core": "^2.10.0",
"@tiptap/pm": "^2.10.0",
"@tiptap/starter-kit": "^2.10.0",
"@tiptap/extension-underline": "^2.10.0",
"tiptap-markdown": "^0.8.10",
"tiptap-extension-search-and-replace": "^0.0.7"
```

Pin to exact compatible minor versions to avoid surprises. (Use `npm view <pkg> version` to confirm the latest available at install time; the values above are the floor.)

- [ ] **Step 2: Install**

Run: `npm install`
Expected: clean install, lockfile updated, no audit critical.

- [ ] **Step 3: Verify no telemetry / phone-home in the new packages**

Run: `grep -rIni "fetch\|XMLHttpRequest\|navigator.sendBeacon" node_modules/{@tiptap,tiptap-markdown,tiptap-extension-search-and-replace} 2>/dev/null | head -20`

Expected: any matches must be local-DOM uses (e.g. ProseMirror's internal DOM transactions). No `https://*.tiptap.dev` or analytics-style endpoints. Document the audit result in the commit message if any results are surprising.

- [ ] **Step 4: Create RichEditor.svelte skeleton**

Write `src/lib/components/RichEditor.svelte`:

```svelte
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
        StarterKit.configure({ history: true }),
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
```

- [ ] **Step 5: Wire RichEditor into EditorTab.svelte**

In `src/lib/pages/EditorTab.svelte`:

1. Add to imports (near line 5):
   ```ts
   import RichEditor from '../components/RichEditor.svelte';
   ```
2. Replace the single render line (around line 181) with a branch:
   ```svelte
   {#if tabId === 'transcript'}
     <TextEditor value={content} placeholder="No content…" onChange={onEditorChange} />
   {:else}
     <RichEditor value={content} placeholder="No content…" onChange={onEditorChange} />
   {/if}
   ```

- [ ] **Step 6: Verify build**

Run: `npm run check`
Expected: 0 errors, 0 warnings.

Run: `npx vitest run`
Expected: 233/233 (or current passing count) — no regressions.

- [ ] **Step 7: Manual smoke check**

Run: `npm run tauri dev` (briefly) — open a recording, switch to SOAP tab, confirm the new editor renders and shows existing plain-text content. Type a few characters; confirm the save indicator goes "saving" → "saved." Switch to Transcript — confirm it still uses the textarea. Close dev.

If this step is impractical in a non-interactive setting, mark it as a manual-verification note in the commit message.

- [ ] **Step 8: Commit**

```bash
git add package.json package-lock.json src/lib/components/RichEditor.svelte src/lib/components/rich_editor/.gitkeep src/lib/pages/EditorTab.svelte
git commit -m "feat(editor): add Tiptap RichEditor skeleton for SOAP/Referral/Letter

Initial Tiptap v2 integration with StarterKit + Underline + Markdown.
EditorTab.svelte branches by tabId: SOAP/Referral/Letter use the new
RichEditor; Transcript continues with the plain TextEditor. Existing
save/debounce flow unchanged — onChange still emits a plain string,
which is now Markdown.

No DB migration. Existing plain-text notes are valid Markdown and
load identically.
"
```

---

## Task 2: Verify Markdown round-trip behavior with existing content

**Files:**
- Create: `src/lib/components/rich_editor/markdown_roundtrip.test.ts`

**Why this task is a test-only deliverable:** Tiptap-Markdown round-trips MOSTLY preserve plain text, but a known edge is that adjacent paragraphs serialize with double-newlines, and a single-newline-in-source becomes a paragraph break unless `breaks: true` is set (which we did in Task 1). We codify the expectation in a unit test so future package upgrades surface regressions.

- [ ] **Step 1: Write the round-trip test**

```ts
// src/lib/components/rich_editor/markdown_roundtrip.test.ts
import { describe, it, expect } from 'vitest';
import { Editor } from '@tiptap/core';
import StarterKit from '@tiptap/starter-kit';
import Underline from '@tiptap/extension-underline';
import { Markdown } from 'tiptap-markdown';

function makeEditor(initial = ''): Editor {
  return new Editor({
    extensions: [
      StarterKit.configure({ history: true }),
      Underline,
      Markdown.configure({ html: false, breaks: true }),
    ],
    content: initial,
  });
}

function md(ed: Editor): string {
  return (ed.storage as { markdown: { getMarkdown: () => string } }).markdown.getMarkdown();
}

describe('Markdown round-trip', () => {
  it('preserves plain prose', () => {
    const input = 'The patient presents with a cough and a fever.';
    const ed = makeEditor(input);
    expect(md(ed).trim()).toBe(input);
    ed.destroy();
  });

  it('preserves multi-paragraph plain text', () => {
    const input = 'Subjective:\n\nThe patient reports fatigue.\n\nObjective:\n\nBP 120/80.';
    const ed = makeEditor(input);
    expect(md(ed).trim()).toBe(input.trim());
    ed.destroy();
  });

  it('preserves bullet lists', () => {
    const input = '- item one\n- item two\n- item three';
    const ed = makeEditor(input);
    const out = md(ed).trim();
    expect(out).toContain('- item one');
    expect(out).toContain('- item two');
    expect(out).toContain('- item three');
    ed.destroy();
  });

  it('preserves bold and italic marks', () => {
    const input = 'The **pain** is *worse* in the morning.';
    const ed = makeEditor(input);
    const out = md(ed).trim();
    expect(out).toBe(input);
    ed.destroy();
  });

  it('returns empty string for empty editor', () => {
    const ed = makeEditor('');
    expect(md(ed).trim()).toBe('');
    ed.destroy();
  });
});
```

- [ ] **Step 2: Run the test**

Run: `npx vitest run src/lib/components/rich_editor/markdown_roundtrip.test.ts`
Expected: 5/5 pass.

If any test fails (a plausible outcome on tiptap-markdown edge cases), adjust expectations to reflect the ACTUAL serialization behavior — the goal is to lock in current behavior, not prescribe a specific output format. Note any unexpected behavior in the commit message.

- [ ] **Step 3: Full test run**

Run: `npx vitest run`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/lib/components/rich_editor/markdown_roundtrip.test.ts
git commit -m "test(editor): lock in tiptap-markdown round-trip behavior

Five tests covering plain prose, multi-paragraph layout, bullet lists,
inline bold/italic marks, and the empty editor case. These pin current
serialization shape so future tiptap-markdown upgrades surface
regressions explicitly."
```

---

## Task 3: Toolbar with formatting buttons

**Files:**
- Create: `src/lib/components/rich_editor/Toolbar.svelte`
- Modify: `src/lib/components/RichEditor.svelte` (render Toolbar, expose editor instance to it)

- [ ] **Step 1: Create Toolbar.svelte**

```svelte
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
    border-bottom: 1px solid var(--border-primary);
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
  .toolbar button:hover { background-color: var(--bg-tertiary); }
  .toolbar button.active {
    background-color: var(--accent-primary, #4a90e2);
    color: var(--text-on-accent, white);
  }
  .sep {
    display: inline-block;
    width: 1px;
    height: 18px;
    background-color: var(--border-primary);
    margin: 0 4px;
  }
</style>
```

- [ ] **Step 2: Wire Toolbar into RichEditor.svelte**

In `RichEditor.svelte`:

1. Add to imports:
   ```ts
   import Toolbar from './rich_editor/Toolbar.svelte';
   ```
2. Replace the single `<div class="rich-editor" ...>` with a wrapper:
   ```svelte
   <div class="rich-editor-wrapper">
     {#if !readonly}
       <Toolbar {editor} onFindClick={() => { /* wired in Task 4 */ }} />
     {/if}
     <div class="rich-editor" bind:this={editorEl} aria-label={placeholder || 'Editor'}></div>
   </div>
   ```
3. Add to the style block:
   ```css
   .rich-editor-wrapper {
     flex: 1;
     display: flex;
     flex-direction: column;
     overflow: hidden;
     min-height: 0;
   }
   ```

- [ ] **Step 3: Verify build**

Run: `npm run check`
Expected: 0 errors, 0 warnings.

- [ ] **Step 4: Manual smoke**

Briefly: open a SOAP note, click Bold → type → confirm bold marker appears in saved Markdown (`**word**`). Cmd+B native shortcut already works because Tiptap's StarterKit binds it.

- [ ] **Step 5: Commit**

```bash
git add src/lib/components/rich_editor/Toolbar.svelte src/lib/components/RichEditor.svelte
git commit -m "feat(editor): add formatting toolbar to RichEditor

Bold, italic, underline, bullet/ordered lists, H1–H3, plus a Find
button stub (wired in the next task). Buttons reflect active marks
via Tiptap selection events. Toolbar hidden in readonly mode."
```

---

## Task 4: Find & Replace panel

**Files:**
- Create: `src/lib/components/rich_editor/FindPanel.svelte`
- Modify: `src/lib/components/RichEditor.svelte` (mount panel, wire shortcuts + Find button)

- [ ] **Step 1: Add the SearchAndReplace extension to RichEditor**

In `RichEditor.svelte`, near the existing imports:

```ts
import { SearchAndReplace } from 'tiptap-extension-search-and-replace';
```

Add to the `extensions` array in the Editor constructor (after Markdown):

```ts
SearchAndReplace.configure({
  searchResultClass: 'rich-editor-search-hit',
  disableRegex: false,
}),
```

Add a CSS rule in the existing `<style>` block:

```css
:global(.rich-editor-search-hit) {
  background-color: rgba(255, 215, 0, 0.4);
}
:global(.rich-editor-search-hit-active) {
  background-color: rgba(255, 165, 0, 0.7);
}
```

- [ ] **Step 2: Create FindPanel.svelte**

```svelte
<!-- src/lib/components/rich_editor/FindPanel.svelte -->
<script lang="ts">
  import type { Editor } from '@tiptap/core';

  interface Props {
    editor: Editor | null;
    open: boolean;
    onClose: () => void;
  }

  let { editor, open, onClose }: Props = $props();

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

  $effect(() => {
    if (!editor) return;
    // Push current search term into the extension on change.
    editor.commands.setSearchTerm(findText);
    editor.commands.setReplaceTerm(replaceText);
    editor.commands.setCaseSensitive(caseSensitive);
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
    if (!editor || !findText) return;
    editor.commands.replace();
  }
  function replaceAll() {
    if (!editor || !findText) return;
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
      <button type="button" aria-label="Previous match" onclick={prev}>↑</button>
      <button type="button" aria-label="Next match" onclick={next}>↓</button>
      <label class="opt"><input type="checkbox" bind:checked={caseSensitive} /> Aa</label>
      <button type="button" aria-label="More options"
        onclick={() => (showReplace = !showReplace)}>↕</button>
      <button type="button" aria-label="Close find panel" onclick={onClose}>✕</button>
    </div>
    {#if showReplace}
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
```

- [ ] **Step 3: Wire FindPanel in RichEditor.svelte**

Add to imports:

```ts
import FindPanel from './rich_editor/FindPanel.svelte';
```

Add state:

```ts
let findOpen = $state(false);
```

Update the markup wrapper to position panel as overlay:

```svelte
<div class="rich-editor-wrapper">
  {#if !readonly}
    <Toolbar {editor} onFindClick={() => (findOpen = true)} />
  {/if}
  <div class="rich-editor-host">
    <FindPanel {editor} open={findOpen} onClose={() => (findOpen = false)} />
    <div class="rich-editor" bind:this={editorEl} aria-label={placeholder || 'Editor'}></div>
  </div>
</div>
```

Add to style block:

```css
.rich-editor-host {
  position: relative;
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  min-height: 0;
}
```

Add a global Cmd+F / Cmd+H keyboard handler. In `onMount`, after the editor is created:

```ts
const onKey = (e: KeyboardEvent) => {
  const metaOrCtrl = e.metaKey || e.ctrlKey;
  if (metaOrCtrl && e.key.toLowerCase() === 'f') {
    e.preventDefault();
    findOpen = true;
  } else if (metaOrCtrl && e.key.toLowerCase() === 'h') {
    e.preventDefault();
    findOpen = true;
    // showReplace toggle lives inside FindPanel; ergonomically the Find
    // input takes focus and the user can ↕ for Replace. Cmd+H opens panel.
  }
};
editorEl.addEventListener('keydown', onKey);
```

And in `onDestroy`:

```ts
editorEl?.removeEventListener('keydown', onKey);
```

(Hoist `onKey` to an outer scope so it's reachable in onDestroy.)

- [ ] **Step 4: Verify build**

Run: `npm run check`
Expected: 0 errors, 0 warnings.

- [ ] **Step 5: Manual smoke**

Open a SOAP note with multi-line content. Cmd+F → type a word that appears → confirm highlights. Press Enter → next match. Shift+Enter → previous. Toggle ↕ → Replace row appears. Replace, Replace All work. Esc closes.

- [ ] **Step 6: Commit**

```bash
git add src/lib/components/rich_editor/FindPanel.svelte src/lib/components/RichEditor.svelte
git commit -m "feat(editor): add Find & Replace panel to RichEditor

Cmd+F opens the find panel; Enter / Shift+Enter cycle matches; toggle
opens Replace with Replace / Replace All. Uses
tiptap-extension-search-and-replace. Esc closes the panel. Hidden in
readonly mode."
```

---

## Task 5: Final polish + acceptance verification

**Files:**
- Modify: `src/lib/components/RichEditor.svelte` (small cleanups)

- [ ] **Step 1: Verify all spec acceptance criteria**

Walk through each item from the spec's "Acceptance criteria" list, manually or via test:

1. Existing plain-text notes load and render identically — **manual: open a pre-existing recording, no visible diff**
2. Saving an unedited note round-trips Markdown identically — **manual: open then close without typing, verify no `final_text` update and no `saving` indicator fires**
3. Find & Replace works with Cmd+F/Cmd+H — **manual (Task 4 smoke covers this)**
4. Bold/italic/underline/list/heading buttons apply and toggle correctly — **manual (Task 3 smoke)**
5. Switching tabs / recordings doesn't lose pending unsaved edits — **manual: type, switch tab before debounce fires, switch back; confirm content preserved (it should be — `value` flows from the store which already holds optimistic update)**
6. Transcript tab still uses plain textarea — **manual: switch to Transcript, confirm textarea**
7. `final_text` updates on every debounced save — **inspect `generations` table after editing**
8. `npm run check` passes — automated below
9. No new console logs contain note body content — `grep -n "console.log\|console.error\|console.warn" src/lib/components/RichEditor.svelte src/lib/components/rich_editor/*` should show no calls that pass `value`, `content`, `findText`, or `replaceText` directly. Document the check result.

- [ ] **Step 2: Type-check + tests**

Run: `npm run check`
Expected: 0 errors, 0 warnings.

Run: `npx vitest run`
Expected: full suite passes (the new markdown-roundtrip tests add 5 to the count).

- [ ] **Step 3: PHI-log audit**

Run:

```bash
grep -nE "console\.(log|error|warn)" src/lib/components/RichEditor.svelte src/lib/components/rich_editor/
```

Expected: empty (no console calls) OR only calls that log non-content values (counts, lengths, event types).

- [ ] **Step 4: Verify the dependencies are pinned and the lockfile is committed**

Run: `git status` — confirm `package-lock.json` is staged from Task 1's commit. If not, add it now.

- [ ] **Step 5: Commit any final polish**

Only if Step 1's acceptance walk surfaced fixable issues. Otherwise skip.

```bash
git commit -m "chore(editor): final polish for rich editor v1"  # only if there are changes
```

If no changes are needed, skip the commit.

---

## Out of scope (deferred follow-ups)

1. **Medical-dictionary spellcheck.** The existing `vocabulary` table is a substitution table, not a wordlist. Real medical spellcheck needs its own design (Hunspell-compatible dict files, ProseMirror decorations to render underlines, an "Add word" UI). Not in v1.
2. **Tables.** Tiptap supports tables via `@tiptap/extension-table`. Add when a clinician asks for them.
3. **Track changes / comments.** Requires storing decorations alongside content — bigger storage redesign.
4. **Auto-save status integration.** The existing "Saving…/Saved" badge in `EditorTab.svelte` already drives off `save_recording_field`, so it works unchanged. Adding inline "modified since last save" decoration is a possible polish.
5. **Cross-tab keyboard shortcuts (e.g. Cmd+B in Transcript)** — Transcript stays a textarea; native bold-key doesn't apply there.
