// @vitest-environment jsdom
// src/lib/components/rich_editor/markdown_roundtrip.test.ts
// Tiptap's Editor constructor touches `document`, so this suite must run in jsdom.
// The repo-wide default vitest environment is `node`; mirror the per-file
// override pattern used by src/lib/stores/recordSidebar.test.ts.
import { describe, it, expect } from 'vitest';
import { Editor } from '@tiptap/core';
import StarterKit from '@tiptap/starter-kit';
import Underline from '@tiptap/extension-underline';
import { Markdown } from 'tiptap-markdown';

function makeEditor(initial = ''): Editor {
  return new Editor({
    extensions: [
      // History is enabled by default in StarterKit; mirror RichEditor.svelte's
      // bare usage (StarterKit.configure({ history: true }) is rejected by the
      // current @tiptap/starter-kit types).
      StarterKit,
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
