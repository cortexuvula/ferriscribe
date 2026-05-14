import { diffLines as jsDiffLines } from 'diff';

export type DiffLine =
  | { kind: 'context'; text: string }
  | { kind: 'add'; text: string }
  | { kind: 'remove'; text: string };

const SNIPPET_MAX = 60;

function trimTrailing(s: string): string {
  return s.replace(/\s+$/u, '');
}

function truncate(s: string): string {
  return s.length <= SNIPPET_MAX ? s : s.slice(0, SNIPPET_MAX) + '…';
}

export function diffLines(draft: string, final: string): DiffLine[] {
  const a = trimTrailing(draft);
  const b = trimTrailing(final);
  const parts = jsDiffLines(a, b, { newlineIsToken: false });
  const out: DiffLine[] = [];
  for (const part of parts) {
    // jsdiff returns blocks where `.value` is one or more lines joined by \n.
    // Split into individual lines and drop the trailing empty line that comes
    // from a terminal \n in the block.
    const lines = part.value.split('\n');
    if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
    const kind: DiffLine['kind'] = part.added ? 'add' : part.removed ? 'remove' : 'context';
    for (const text of lines) out.push({ kind, text });
  }
  return out;
}

export function firstChangeSnippet(
  draft: string,
  final: string | null,
): { removed: string | null; added: string | null } | null {
  if (final === null) return null;
  const lines = diffLines(draft, final);
  let removed: string | null = null;
  let added: string | null = null;
  for (const line of lines) {
    if (line.kind === 'remove' && removed === null) removed = truncate(line.text);
    else if (line.kind === 'add' && added === null) added = truncate(line.text);
    if (removed !== null && added !== null) break;
  }
  if (removed === null && added === null) return null;
  return { removed, added };
}
