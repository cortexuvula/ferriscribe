# Rich Editor for SOAP / Referral / Letter — Design

**Date:** 2026-05-17
**Status:** Draft

## Goal

Replace the plain `<textarea>` editor for SOAP Note, Referral Letter, and Patient Letter tabs with a Tiptap-based rich-text editor that provides:

- Find & Replace (Cmd+F / Cmd+H, regex optional)
- Native OS spellcheck (browser default)
- Undo / Redo with full history
- Inline formatting: **bold**, *italic*, _underline_, bullet & ordered lists, headings (H1–H3)
- Right-click "Add word to medical vocabulary" affordance
- Toolbar with the formatting buttons + a Find button

Transcript tab remains a plain `<textarea>` — transcripts are raw STT output and shouldn't carry user-applied formatting.

## Why

The current `<textarea>` lacks Find & Replace inside its content (webview Cmd+F doesn't traverse textarea bodies) and offers no formatting controls. Clinicians editing SOAP / Referral / Letter want to highlight findings, structure plans as lists, and quickly fix typos across long notes. The training corpus pipeline (`generations.final_text`) benefits from capturing formatting choices as part of the training signal.

## Architecture

- **Editor library:** [Tiptap v2](https://tiptap.dev) (MIT, no telemetry). Built on ProseMirror.
- **Storage format:** Markdown, persisted in the existing `recordings.soap_note` / `referral` / `letter` TEXT columns. No schema migration. Existing plain-text content remains valid Markdown.
- **Serialization:** `tiptap-markdown` community extension (MIT) converts between ProseMirror state and Markdown on load/save.
- **Search compatibility:** FTS5 indexes the raw column content; since Markdown is plain text, search continues to work without modification.
- **Training corpus:** `save_recording_field` already writes the edited value into `generations.final_text` via `GenerationsRepo::update_final_text`. No backend change. The corpus accumulates Markdown `final_text` against plain-text `draft_text`; `edit_distance` between the two stays meaningful because Markdown adds minimal noise.
- **No PHI in logs.** Tiptap is purely client-side; we add no logging that includes note content.
- **No telemetry.** All Tiptap extensions used are open-source and offline. We pin specific versions in `package.json`.

### Tab branching

`EditorTab.svelte` accepts a `tabId` of `'transcript' | 'soap' | 'referral' | 'letter'`. The component branches:
- `tabId === 'transcript'` → render `<TextEditor>` (existing textarea, unchanged)
- Otherwise → render `<RichEditor>` (new Tiptap component)

Both components have identical `{ value, placeholder, readonly, onChange }` props, so the save/debounce logic in `EditorTab.svelte` stays unchanged.

### Find & Replace

A floating panel anchored to the editor:
- Cmd+F (or Ctrl+F on non-mac) opens the Find input
- Cmd+G / Cmd+Shift+G cycles next/prev
- Cmd+H opens the Replace row (or toggles)
- Toggles: Case-sensitive, Whole word, Regex (regex via try/catch — fail-soft)
- Replace, Replace All buttons
- Esc closes the panel

Implementation: a Tiptap extension that uses ProseMirror decorations to highlight matches in the document. Either build inline (~100 lines) or adopt `tiptap-extension-search-and-replace` (community, MIT).

### Spellcheck

Browser-native — Tiptap's contenteditable inherits `spellcheck="true"` by default. macOS WKWebView highlights misspelled words and provides the native context menu with "Add to Dictionary" / "Learn Spelling." On Windows WebView2, spellcheck works but without UI affordance for adding words. We add no custom spell-check engine in v1.

### Medical vocabulary integration

**Deferred from v1.** The existing `vocabulary` table is a substitution table (`find_text` → `replacement`) used by the STT pipeline. It is not a spellcheck wordlist. Wiring an "Add to vocabulary" affordance against it would write degenerate `{find_text=X, replacement=X}` rows — pollution without benefit.

The native OS context menu (macOS WKWebView "Learn Spelling") already lets the user teach the OS dictionary individual words without any code change. That covers the immediate ergonomic need. A real medical-dictionary integration is its own design and is out of scope.

### Toolbar

A small toolbar above the editor:
- **B** / *I* / U → Bold / Italic / Underline
- Bullet List / Ordered List
- H1 / H2 / H3 (cycle)
- Find (opens Find panel; Cmd+F shortcut)
- Undo / Redo (Cmd+Z / Cmd+Shift+Z native)

Buttons reflect active marks (highlighted when the cursor is in a bold range, etc.).

## File-level decomposition

### New files

- `src/lib/components/RichEditor.svelte` — Tiptap host, toolbar, find-panel; ~250–350 lines
- `src/lib/components/rich_editor/Toolbar.svelte` — formatting buttons; ~120 lines
- `src/lib/components/rich_editor/FindPanel.svelte` — find/replace UI; ~150 lines
- `src/lib/components/rich_editor/use_tiptap.svelte.ts` — Svelte runes wrapper around Tiptap editor instance lifecycle; ~80 lines
- `src/lib/components/rich_editor/find_extension.ts` — ProseMirror Find/Replace decorator if not using community extension; ~120 lines OR pulled from community package

### Modified files

- `src/lib/pages/EditorTab.svelte` — branch by `tabId`; render `<RichEditor>` for soap/referral/letter, `<TextEditor>` for transcript
- `package.json` — add `@tiptap/core`, `@tiptap/starter-kit`, `@tiptap/extension-underline`, `tiptap-markdown`, and `tiptap-extension-search-and-replace` (or DIY)

### Unchanged

- `src-tauri/src/commands/recordings_edit.rs` — save flow stays the same; debounced string write
- `crates/db/src/generations.rs` — corpus capture unchanged
- DB schema — no migration
- `TextEditor.svelte` — kept for the transcript tab

## Constraints

- **Hard constraints (CLAUDE.md):**
  - **No PHI in logs.** Editor body content must never appear in `console.log` / `console.error`. Only counts, lengths, edit-distance values OK.
  - **No telemetry / phone-home.** Verify each new npm dep is offline-only. Pin versions in `package.json`.
- **No DB schema change.**
- **No breaking change to `save_recording_field` signature.** Frontend still sends a plain string; the string just happens to be Markdown.
- **Transcript tab unchanged** (avoids regressing the largest editing surface).
- **Read-only mode preserved.** When `readonly=true`, the editor renders content without the toolbar and disables editing.
- **Existing plain-text content loads identically** — a recording saved before this change shows up correctly (Markdown is a superset of plain text).

## Non-goals (v1)

- Medical-dictionary spellcheck — out of scope; users rely on OS dictionary + the right-click "Add to Vocabulary" affordance.
- Track-changes / comments / annotations.
- Tables (Tiptap supports them; defer to a follow-up if requested).
- Collaborative editing.
- ProseMirror-JSON storage with a structured schema.
- Markdown extensions beyond CommonMark + GFM tables/lists (no math, no Mermaid).

## Open questions

None outstanding. Storage format = Markdown; corpus flow = unchanged; affected tabs = SOAP / Referral / Letter.

## Acceptance criteria

1. Existing plain-text notes load and render identically in the new editor (no visible diff on read).
2. Saving an unedited note round-trips Markdown identically (no spurious whitespace/format changes that would trigger a `final_text` update).
3. Find & Replace works with Cmd+F/Cmd+H; matches highlight; Replace All updates the document.
4. Bold/italic/underline/list/heading buttons apply and toggle correctly.
5. Switching tabs / recordings doesn't lose pending unsaved edits (existing debounce logic continues to work).
6. Switching to the Transcript tab renders the plain textarea unchanged.
7. `final_text` in the `generations` row updates on every debounced save (existing behavior continues).
8. `npm run check` and `npx vitest run` pass.
9. No new console logs contain note body content.
