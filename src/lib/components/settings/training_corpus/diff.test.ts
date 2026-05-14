import { describe, it, expect } from 'vitest';
import { diffLines, firstChangeSnippet } from './diff';

describe('diffLines', () => {
  it('returns all context lines when inputs are identical', () => {
    const out = diffLines('foo\nbar', 'foo\nbar');
    expect(out.every((l) => l.kind === 'context')).toBe(true);
    expect(out.map((l) => l.text)).toEqual(['foo', 'bar']);
  });

  it('normalizes trailing newlines so they do not produce spurious diffs', () => {
    const out = diffLines('foo\nbar\n', 'foo\nbar');
    expect(out.every((l) => l.kind === 'context')).toBe(true);
  });

  it('marks added lines on pure insertion', () => {
    const out = diffLines('', 'new line');
    expect(out).toEqual([{ kind: 'add', text: 'new line' }]);
  });

  it('marks removed lines on pure deletion', () => {
    const out = diffLines('old line', '');
    expect(out).toEqual([{ kind: 'remove', text: 'old line' }]);
  });

  it('produces a mixed diff for a one-line edit', () => {
    const out = diffLines('a\nold\nc', 'a\nnew\nc');
    expect(out).toEqual([
      { kind: 'context', text: 'a' },
      { kind: 'remove', text: 'old' },
      { kind: 'add', text: 'new' },
      { kind: 'context', text: 'c' },
    ]);
  });
});

describe('firstChangeSnippet', () => {
  it('returns null when inputs are identical', () => {
    expect(firstChangeSnippet('foo', 'foo')).toBeNull();
  });

  it('returns null when final is null', () => {
    expect(firstChangeSnippet('foo', null)).toBeNull();
  });

  it('returns the first remove+add pair', () => {
    const snip = firstChangeSnippet('a\nold\nc', 'a\nnew\nc');
    expect(snip).toEqual({ removed: 'old', added: 'new' });
  });

  it('returns only the added side on pure insertion', () => {
    expect(firstChangeSnippet('', 'hello')).toEqual({ removed: null, added: 'hello' });
  });

  it('returns only the removed side on pure deletion', () => {
    expect(firstChangeSnippet('hello', '')).toEqual({ removed: 'hello', added: null });
  });

  it('truncates long snippet text to 60 chars with an ellipsis', () => {
    const long = 'x'.repeat(120);
    const snip = firstChangeSnippet('', long);
    expect(snip!.added).toMatch(/^x{60}…$/);
  });
});
