# Training Corpus Review UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the side-by-side preview cards in Settings → Training Corpus with a master+detail layout that renders a line-level unified diff between draft and final SOAP notes, so the reviewer can actually see what changed before deciding Promote / Reject / Unpromote / Restore.

**Architecture:** Three new Svelte components (`ReviewLayout`, `MasterRow`, `DetailPane`) plus a pure-TS `diff.ts` helper using `jsdiff`. The three list components (`CandidatesList`, `PromotedList`, `RejectedList`) become thin wrappers that pass a `mode` prop to `ReviewLayout`. Backend gets one filter change so null-`final_text` rows are excluded from the Candidates queue (and counted out of the candidates count) but stay visible in Promoted/Rejected for audit. `GenerationCard.svelte` is deleted.

**Tech Stack:** Svelte 5 runes (`$state`, `$derived`, `$props`), TypeScript, `jsdiff` npm package, Vitest (logic only — no Svelte component testing framework in this repo, components verified via manual smoke per `CLAUDE.md`), Rust (`rusqlite`), `cargo test -p medical-db`.

**Spec:** [`docs/superpowers/specs/2026-05-14-training-corpus-review-ui-design.md`](../specs/2026-05-14-training-corpus-review-ui-design.md)

---

## File Structure

**New files:**
- `src/lib/components/settings/training_corpus/diff.ts` — pure helpers around `jsdiff` (`diffLines`, `firstChangeSnippet`)
- `src/lib/components/settings/training_corpus/diff.test.ts` — Vitest unit tests for the above
- `src/lib/components/settings/training_corpus/ReviewLayout.svelte` — master+detail container, owns selection + keyboard nav
- `src/lib/components/settings/training_corpus/MasterRow.svelte` — single row in the master column
- `src/lib/components/settings/training_corpus/DetailPane.svelte` — diff renderer with sticky header + footer

**Modified files:**
- `package.json` — add `diff` and `@types/diff` deps
- `crates/db/src/generations.rs` — `list_by_status` and `count_by_status` exclude rows with `final_text IS NULL` when `corpus_status='candidate'`
- `src/lib/components/settings/training_corpus/CandidatesList.svelte` — becomes a 3-line wrapper around `<ReviewLayout mode="candidate" ... />`
- `src/lib/components/settings/training_corpus/PromotedList.svelte` — becomes a wrapper around `<ReviewLayout mode="promoted" ... />`, preserves the existing Export toolbar
- `src/lib/components/settings/training_corpus/RejectedList.svelte` — becomes a wrapper around `<ReviewLayout mode="rejected" ... />`

**Deleted files:**
- `src/lib/components/settings/training_corpus/GenerationCard.svelte` — replaced by `MasterRow` + `DetailPane`

**Worktree:** Use `superpowers:using-git-worktrees` to create `.worktrees/training-corpus-review-ui` from `master` at the spec commit (`96de4dc`) before Task 1.

---

## Task 1: Add `jsdiff` dependency

**Files:**
- Modify: `package.json`, `package-lock.json`

**Why:** `diff.ts` depends on it; getting the dep installed first means subsequent tasks can import without setup churn.

- [ ] **Step 1: Install the package**

Run from the repo root:

```bash
npm install diff@^7
npm install --save-dev @types/diff@^7
```

- [ ] **Step 2: Verify the install**

Run:

```bash
node -e "const d = require('diff'); console.log(typeof d.diffLines)"
```

Expected output: `function`

- [ ] **Step 3: Verify TypeScript can resolve the types**

Run:

```bash
npm run check
```

Expected: no new errors. (Existing pre-existing warnings, if any, can remain — we're only checking that the `diff` types resolve.)

- [ ] **Step 4: Commit**

```bash
git add package.json package-lock.json
git commit -m "deps: add jsdiff for training-corpus review UI"
```

---

## Task 2: Pure-TS `diff.ts` helpers (TDD)

**Files:**
- Create: `src/lib/components/settings/training_corpus/diff.ts`
- Create: `src/lib/components/settings/training_corpus/diff.test.ts`

**Why:** Diff logic is the only non-trivial piece of new code. Pure functions, no Svelte runtime dependency — perfect for TDD. The components that consume this in later tasks can rely on it being correct.

- [ ] **Step 1: Write the failing test file**

Create `src/lib/components/settings/training_corpus/diff.test.ts`:

```ts
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
npx vitest run src/lib/components/settings/training_corpus/diff.test.ts
```

Expected: all tests fail because `diff.ts` doesn't exist.

- [ ] **Step 3: Implement `diff.ts`**

Create `src/lib/components/settings/training_corpus/diff.ts`:

```ts
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
npx vitest run src/lib/components/settings/training_corpus/diff.test.ts
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib/components/settings/training_corpus/diff.ts src/lib/components/settings/training_corpus/diff.test.ts
git commit -m "feat(training-corpus): add jsdiff-based diff helpers"
```

---

## Task 3: Backend — exclude null-final candidates (TDD)

**Files:**
- Modify: `crates/db/src/generations.rs` (functions `list_by_status` around line 186 and `count_by_status` around line 221)

**Why:** Per the spec, null-`final_text` rows are not useful training pairs and must be filtered out of the Candidates queue. They stay visible in Promoted/Rejected so audit history is preserved.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block at the bottom of `crates/db/src/generations.rs`. Look at the existing helper functions in that file (`fresh_conn`, `insert_finalized_candidate`, etc.) and follow the same pattern. Append after the existing tests:

```rust
#[test]
fn list_by_status_candidate_excludes_null_final_text() {
    let conn = fresh_conn();
    // One candidate with final_text present:
    let with_final = Uuid::new_v4();
    conn.execute(
        "INSERT INTO generations
           (id, recording_id, output_type, created_at, ai_provider, ai_model,
            input_transcript, draft_text, final_text, corpus_status, regeneration_seq)
         VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'qwen3.6',
                 'transcript', 'draft body', 'final body', 'candidate', 1)",
        params![with_final.to_string(), Uuid::new_v4().to_string()],
    ).unwrap();
    // One candidate with final_text NULL (the case we filter out):
    let without_final = Uuid::new_v4();
    conn.execute(
        "INSERT INTO generations
           (id, recording_id, output_type, created_at, ai_provider, ai_model,
            input_transcript, draft_text, final_text, corpus_status, regeneration_seq)
         VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'qwen3.6',
                 'transcript', 'draft body', NULL, 'candidate', 1)",
        params![without_final.to_string(), Uuid::new_v4().to_string()],
    ).unwrap();

    let (items, total) =
        GenerationsRepo::list_by_status(&conn, "candidate", 10, 0).unwrap();
    assert_eq!(total, 1, "total should reflect only candidates with final_text");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, with_final);
}

#[test]
fn list_by_status_promoted_still_includes_null_final_text() {
    let conn = fresh_conn();
    let id = Uuid::new_v4();
    conn.execute(
        "INSERT INTO generations
           (id, recording_id, output_type, created_at, ai_provider, ai_model,
            input_transcript, draft_text, final_text, corpus_status, regeneration_seq)
         VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'qwen3.6',
                 'transcript', 'draft body', NULL, 'promoted', 1)",
        params![id.to_string(), Uuid::new_v4().to_string()],
    ).unwrap();

    let (items, total) =
        GenerationsRepo::list_by_status(&conn, "promoted", 10, 0).unwrap();
    assert_eq!(total, 1);
    assert_eq!(items.len(), 1);
}

#[test]
fn count_by_status_excludes_null_final_text_from_candidates() {
    let conn = fresh_conn();
    // Two candidates: one with final_text, one without.
    conn.execute(
        "INSERT INTO generations
           (id, recording_id, output_type, created_at, ai_provider, ai_model,
            input_transcript, draft_text, final_text, corpus_status, regeneration_seq)
         VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'qwen3.6',
                 'transcript', 'draft body', 'final body', 'candidate', 1)",
        params![Uuid::new_v4().to_string(), Uuid::new_v4().to_string()],
    ).unwrap();
    conn.execute(
        "INSERT INTO generations
           (id, recording_id, output_type, created_at, ai_provider, ai_model,
            input_transcript, draft_text, final_text, corpus_status, regeneration_seq)
         VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'qwen3.6',
                 'transcript', 'draft body', NULL, 'candidate', 1)",
        params![Uuid::new_v4().to_string(), Uuid::new_v4().to_string()],
    ).unwrap();

    let (c, _p, _r, _e) = GenerationsRepo::count_by_status(&conn).unwrap();
    assert_eq!(c, 1, "candidate count must match list_by_status filtering");
}
```

(If the test helper names differ from what's shown — check the existing `mod tests` block at the top and copy the actual idiom used by neighbouring tests, e.g. `fresh_conn` may be `setup_conn` or similar.)

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p medical-db generations:: -- --nocapture
```

Expected: the three new tests fail. The first two fail because the current `list_by_status` returns null-final rows when status='candidate' (total=2, len=2). The third fails because `count_by_status` reports c=2.

- [ ] **Step 3: Update `list_by_status` to filter**

In `crates/db/src/generations.rs`, replace the body of `list_by_status` (around lines 186–217). The change is to both the count query and the row query — add `AND (corpus_status != 'candidate' OR final_text IS NOT NULL)`:

```rust
pub fn list_by_status(
    conn: &Connection,
    status: &str,
    limit: u32,
    offset: u32,
) -> DbResult<(Vec<Generation>, u32)> {
    let limit = limit.min(200);

    let total: u32 = conn.query_row(
        "SELECT count(*) FROM generations
         WHERE corpus_status = ?
           AND (corpus_status != 'candidate' OR final_text IS NOT NULL)",
        params![status],
        |r| r.get(0),
    )?;

    let mut stmt = conn.prepare(
        "SELECT id, recording_id, output_type, created_at, finalized_at,
                ai_provider, ai_model, prompt_template_name,
                input_transcript, input_context_json,
                draft_text, final_text,
                corpus_status, corpus_curated_at,
                edit_distance, edit_ratio, regeneration_seq
         FROM generations
         WHERE corpus_status = ?
           AND (corpus_status != 'candidate' OR final_text IS NOT NULL)
         ORDER BY created_at DESC
         LIMIT ? OFFSET ?",
    )?;
    let rows = stmt
        .query_map(params![status, limit, offset], Self::row_to_generation)?
        .filter_map(|r| r.ok())
        .collect();
    Ok((rows, total))
}
```

- [ ] **Step 4: Update `count_by_status` to match**

In the same file, replace `count_by_status` (around lines 221–239) so the `candidate` branch applies the same filter:

```rust
pub fn count_by_status(conn: &Connection) -> DbResult<(u32, u32, u32, u32)> {
    let mut stmt = conn.prepare(
        "SELECT
            SUM(CASE WHEN corpus_status='candidate' AND final_text IS NOT NULL THEN 1 ELSE 0 END) AS c,
            SUM(CASE WHEN corpus_status='promoted'  THEN 1 ELSE 0 END) AS p,
            SUM(CASE WHEN corpus_status='rejected'  THEN 1 ELSE 0 END) AS r,
            SUM(CASE WHEN corpus_status='excluded'  THEN 1 ELSE 0 END) AS e
         FROM generations",
    )?;
    let (c, p, r, e) = stmt.query_row([], |row| {
        Ok((
            row.get::<_, Option<u32>>(0)?.unwrap_or(0),
            row.get::<_, Option<u32>>(1)?.unwrap_or(0),
            row.get::<_, Option<u32>>(2)?.unwrap_or(0),
            row.get::<_, Option<u32>>(3)?.unwrap_or(0),
        ))
    })?;
    Ok((c, p, r, e))
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
cargo test -p medical-db generations:: -- --nocapture
```

Expected: all generations tests pass, including the three new ones AND all existing ones (the existing `list_by_status_returns_candidates_newest_first` etc. tests should be unaffected because their fixtures use non-null `final_text` — but verify).

- [ ] **Step 6: Commit**

```bash
git add crates/db/src/generations.rs
git commit -m "feat(generations): exclude null-final candidates from corpus queue"
```

---

## Task 4: `MasterRow.svelte` (presentational, no test)

**Files:**
- Create: `src/lib/components/settings/training_corpus/MasterRow.svelte`

**Why:** Compact row component used by `ReviewLayout`. Pure presentation — takes a `Generation` prop, renders metadata + first-change snippet, fires `onclick`. No internal state, no async, so no unit test needed (the diff helper it depends on is already tested in Task 2; visual correctness is verified by manual smoke in Task 8).

- [ ] **Step 1: Create the file**

Create `src/lib/components/settings/training_corpus/MasterRow.svelte`:

```svelte
<script lang="ts">
  import { firstChangeSnippet } from './diff';

  type Generation = {
    id: string;
    recording_id: string;
    created_at: string;
    draft_text: string;
    final_text: string | null;
    ai_model: string;
    edit_ratio: number | null;
    regeneration_seq: number;
  };

  type Props = {
    generation: Generation;
    selected: boolean;
    onclick: () => void;
  };
  let { generation, selected, onclick }: Props = $props();

  function editChip(): { label: string; cls: string } {
    if (generation.final_text === null) return { label: 'no save', cls: 'chip-red' };
    const r = generation.edit_ratio;
    if (r === null) return { label: 'computing…', cls: 'chip-gray' };
    if (r < 0.15) return { label: 'light', cls: 'chip-green' };
    if (r < 0.4) return { label: 'moderate', cls: 'chip-yellow' };
    return { label: 'heavy', cls: 'chip-orange' };
  }

  function shortDate(iso: string): string {
    const d = new Date(iso);
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
      + ', '
      + d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  }

  let chip = $derived(editChip());
  let snippet = $derived(firstChangeSnippet(generation.draft_text, generation.final_text));
</script>

<button class="master-row" class:selected type="button" {onclick}>
  <div class="row-head">
    <span class="date">{shortDate(generation.created_at)}</span>
    <span class="chip {chip.cls}">{chip.label}</span>
  </div>
  <div class="model">{generation.ai_model}</div>
  {#if snippet}
    <div class="snippet">
      {#if snippet.removed !== null}<div class="snip-removed">− {snippet.removed}</div>{/if}
      {#if snippet.added !== null}<div class="snip-added">+ {snippet.added}</div>{/if}
    </div>
  {/if}
</button>

<style>
  .master-row {
    display: block;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    border-left: 3px solid transparent;
    padding: 0.5rem 0.75rem;
    cursor: pointer;
    font: inherit;
    color: inherit;
  }
  .master-row:hover { background: rgba(255,255,255,0.03); }
  .master-row.selected {
    background: rgba(59,130,246,0.10);
    border-left-color: #3b82f6;
  }
  .row-head { display: flex; justify-content: space-between; align-items: center; }
  .date { font-size: 0.78rem; font-weight: 600; color: var(--foreground, #cbd2da); }
  .model { font-family: var(--font-mono, monospace); font-size: 0.7rem; color: var(--muted-foreground, #9aa4b2); margin-top: 0.15rem; }
  .snippet {
    margin-top: 0.4rem;
    background: var(--muted, rgba(0,0,0,0.25));
    border-radius: 3px;
    padding: 0.3rem 0.4rem;
    font-family: var(--font-mono, monospace);
    font-size: 0.72rem;
    line-height: 1.4;
  }
  .snip-removed { color: #fca5a5; }
  .snip-added { color: #86efac; }
  .chip { font-size: 0.68rem; padding: 0.05rem 0.45rem; border-radius: 9px; }
  .chip-green { background: #0a3b2a; color: #34d399; }
  .chip-yellow { background: #3b2a0a; color: #fbbf24; }
  .chip-orange { background: #3b1d0a; color: #fb923c; }
  .chip-red { background: #3b1d1d; color: #fca5a5; }
  .chip-gray { background: #1f2937; color: #9ca3af; }
</style>
```

- [ ] **Step 2: Verify it type-checks**

Run:

```bash
npm run check
```

Expected: no new errors in `MasterRow.svelte`. (Pre-existing warnings elsewhere are OK.)

- [ ] **Step 3: Commit**

```bash
git add src/lib/components/settings/training_corpus/MasterRow.svelte
git commit -m "feat(training-corpus): MasterRow component for review master column"
```

---

## Task 5: `DetailPane.svelte` (presentational, no test)

**Files:**
- Create: `src/lib/components/settings/training_corpus/DetailPane.svelte`

**Why:** Renders the line-level diff with the action buttons. Like `MasterRow`, it's pure presentation built on the already-tested `diff.ts` — no unit test needed; correctness verified in manual smoke (Task 8).

- [ ] **Step 1: Create the file**

Create `src/lib/components/settings/training_corpus/DetailPane.svelte`:

```svelte
<script lang="ts">
  import { diffLines } from './diff';

  type Generation = {
    id: string;
    recording_id: string;
    created_at: string;
    draft_text: string;
    final_text: string | null;
    ai_model: string;
    edit_ratio: number | null;
    regeneration_seq: number;
  };

  type Action = 'promote' | 'reject' | 'unpromote' | 'restore';
  type Mode = 'candidate' | 'promoted' | 'rejected';

  type Props = {
    generation: Generation | null;
    mode: Mode;
    loading: boolean;
    position: { index: number; total: number } | null;
    onAction: (id: string, action: Action) => void;
  };
  let { generation, mode, loading, position, onAction }: Props = $props();

  function fullDate(iso: string): string {
    return new Date(iso).toLocaleString();
  }

  function chip(g: Generation): { label: string; cls: string } {
    const r = g.edit_ratio;
    const pct = r === null ? null : Math.round(r * 100);
    if (g.final_text === null) return { label: 'no save', cls: 'chip-red' };
    if (r === null) return { label: 'computing…', cls: 'chip-gray' };
    if (r < 0.15) return { label: `light edit · ${pct}% changed`, cls: 'chip-green' };
    if (r < 0.4) return { label: `moderate edit · ${pct}% changed`, cls: 'chip-yellow' };
    return { label: `heavy edit · ${pct}% changed`, cls: 'chip-orange' };
  }

  // Compute the diff for the currently selected generation. final_text being
  // null shouldn't happen in `candidate` mode after Task 3, but `promoted`
  // and `rejected` may still have it — show draft only in that case.
  let diff = $derived.by(() => {
    if (!generation) return [];
    if (generation.final_text === null) return [];
    return diffLines(generation.draft_text, generation.final_text);
  });
</script>

{#if !generation}
  <div class="empty">Nothing to review.</div>
{:else}
  <div class="detail" class:dimmed={loading}>
    <header class="head">
      <div class="head-meta">
        <div class="date">{fullDate(generation.created_at)}</div>
        <div class="meta">
          <span class="model">{generation.ai_model}</span>
          {#if generation.regeneration_seq > 1}
            <span class="regen">#{generation.regeneration_seq}</span>
          {/if}
          {@const c = chip(generation)}
          <span class="chip {c.cls}">{c.label}</span>
        </div>
      </div>
      <div class="actions">
        {#if mode === 'candidate'}
          <button class="btn promote" disabled={loading} onclick={() => onAction(generation.id, 'promote')}>Promote (P)</button>
          <button class="btn reject" disabled={loading} onclick={() => onAction(generation.id, 'reject')}>Reject (R)</button>
        {:else if mode === 'promoted'}
          <button class="btn neutral" disabled={loading} onclick={() => onAction(generation.id, 'unpromote')}>Unpromote (U)</button>
        {:else if mode === 'rejected'}
          <button class="btn neutral" disabled={loading} onclick={() => onAction(generation.id, 'restore')}>Restore (R)</button>
        {/if}
      </div>
    </header>

    <div class="body">
      {#if generation.final_text === null}
        <pre class="draft-only">{generation.draft_text}</pre>
      {:else}
        {#each diff as line, i (i)}
          <div class="line line-{line.kind}">
            <span class="sign">{line.kind === 'add' ? '+' : line.kind === 'remove' ? '−' : ' '}</span><span class="text">{line.text}</span>
          </div>
        {/each}
      {/if}
    </div>

    {#if position}
      <footer class="foot">
        <span>{mode === 'candidate' ? 'Candidate' : mode === 'promoted' ? 'Promoted' : 'Rejected'} <strong>{position.index + 1} of {position.total}</strong></span>
      </footer>
    {/if}
  </div>
{/if}

<style>
  .empty { padding: 2rem; text-align: center; color: var(--muted-foreground, #9aa4b2); }
  .detail { display: flex; flex-direction: column; height: 100%; }
  .detail.dimmed .body { opacity: 0.55; pointer-events: none; }
  .head {
    display: flex; justify-content: space-between; align-items: flex-start; gap: 1rem;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid var(--border, #1c2128);
    background: var(--muted, rgba(255,255,255,0.02));
    position: sticky; top: 0; z-index: 1;
  }
  .date { font-size: 0.85rem; font-weight: 600; }
  .meta { display: flex; gap: 0.5rem; align-items: center; margin-top: 0.2rem; font-size: 0.75rem; color: var(--muted-foreground, #9aa4b2); }
  .model { font-family: var(--font-mono, monospace); }
  .regen { background: #3b2a0a; color: #fbbf24; padding: 0.05rem 0.4rem; border-radius: 3px; font-size: 0.7rem; }
  .chip { font-size: 0.7rem; padding: 0.05rem 0.45rem; border-radius: 9px; }
  .chip-green { background: #0a3b2a; color: #34d399; }
  .chip-yellow { background: #3b2a0a; color: #fbbf24; }
  .chip-orange { background: #3b1d0a; color: #fb923c; }
  .chip-red { background: #3b1d1d; color: #fca5a5; }
  .chip-gray { background: #1f2937; color: #9ca3af; }
  .actions { display: flex; gap: 0.4rem; flex-shrink: 0; }
  .btn {
    padding: 0.3rem 0.8rem; border-radius: 4px; border: 1px solid; cursor: pointer; font-size: 0.8rem;
    background: transparent;
  }
  .btn:disabled { opacity: 0.5; cursor: default; }
  .btn.promote { background: #059669; color: white; border-color: #059669; }
  .btn.reject { color: #dc2626; border-color: #dc2626; }
  .btn.neutral { color: var(--foreground, #cbd2da); border-color: var(--border, #6b7280); }

  .body {
    flex: 1; overflow-y: auto;
    padding: 0.75rem 1rem;
    font-family: var(--font-mono, monospace);
    font-size: 0.78rem;
    line-height: 1.55;
  }
  .line { white-space: pre-wrap; word-break: break-word; }
  .line-context { color: var(--foreground, #cbd2da); }
  .line-add { background: rgba(34,197,94,0.10); color: #86efac; }
  .line-remove { background: rgba(239,68,68,0.10); color: #fca5a5; }
  .sign { display: inline-block; width: 1ch; opacity: 0.7; user-select: none; }
  .draft-only { white-space: pre-wrap; font-family: var(--font-mono, monospace); font-size: 0.78rem; }

  .foot {
    padding: 0.4rem 1rem; border-top: 1px solid var(--border, #1c2128);
    font-size: 0.72rem; color: var(--muted-foreground, #9aa4b2);
    background: var(--muted, rgba(255,255,255,0.02));
  }
</style>
```

- [ ] **Step 2: Verify it type-checks**

Run:

```bash
npm run check
```

Expected: no new errors in `DetailPane.svelte`.

- [ ] **Step 3: Commit**

```bash
git add src/lib/components/settings/training_corpus/DetailPane.svelte
git commit -m "feat(training-corpus): DetailPane component renders unified diff"
```

---

## Task 6: `ReviewLayout.svelte` — the master+detail container

**Files:**
- Create: `src/lib/components/settings/training_corpus/ReviewLayout.svelte`

**Why:** This is where the master and detail compose. It owns the data fetch, selection, pagination, and keyboard nav (J/K with cross-page support, and the mode-specific action keys). It's parameterized by `mode` so all three tabs use the same component.

- [ ] **Step 1: Create the file**

Create `src/lib/components/settings/training_corpus/ReviewLayout.svelte`:

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import MasterRow from './MasterRow.svelte';
  import DetailPane from './DetailPane.svelte';

  type Generation = {
    id: string;
    recording_id: string;
    created_at: string;
    draft_text: string;
    final_text: string | null;
    ai_model: string;
    edit_ratio: number | null;
    regeneration_seq: number;
  };
  type Page = { items: Generation[]; total: number };
  type Mode = 'candidate' | 'promoted' | 'rejected';
  type Action = 'promote' | 'reject' | 'unpromote' | 'restore';

  type Props = {
    mode: Mode;
    onchange?: () => void;
  };
  let { mode, onchange }: Props = $props();

  let items: Generation[] = $state([]);
  let total = $state(0);
  let offset = $state(0);
  let selectedId: string | null = $state(null);
  let loading = $state(false);
  let error: string | null = $state(null);
  const PAGE_SIZE = 50;

  let cursorIndex = $derived(
    selectedId ? items.findIndex((g) => g.id === selectedId) : -1
  );
  let selected = $derived(items.find((g) => g.id === selectedId) ?? null);
  let position = $derived(
    cursorIndex >= 0 && total > 0
      ? { index: offset + cursorIndex, total }
      : null
  );

  async function load(opts?: { keepSelection?: boolean }) {
    const prevSelectedId = selectedId;
    const prevCursor = cursorIndex;
    loading = true;
    error = null;
    try {
      const page = await invoke<Page>('training_corpus_list', {
        status: mode,
        limit: PAGE_SIZE,
        offset,
      });
      items = page.items;
      total = page.total;
    } catch (e) {
      error = String(e);
      items = [];
      total = 0;
    } finally {
      loading = false;
    }

    if (items.length === 0 && offset > 0 && total > 0) {
      offset = Math.max(0, offset - PAGE_SIZE);
      await load(opts);
      return;
    }

    if (opts?.keepSelection && prevSelectedId && items.some((g) => g.id === prevSelectedId)) {
      selectedId = prevSelectedId;
    } else if (items.length > 0) {
      const idx = Math.min(Math.max(prevCursor, 0), items.length - 1);
      selectedId = items[idx].id;
    } else {
      selectedId = null;
    }
  }

  async function act(id: string, action: Action) {
    const new_status =
      action === 'promote' ? 'promoted' :
      action === 'reject' ? 'rejected' :
      'candidate';
    try {
      await invoke('training_corpus_set_status', { id, newStatus: new_status });
      await load();
      onchange?.();
    } catch (e) {
      error = String(e);
    }
  }

  async function goNext() {
    if (cursorIndex < items.length - 1) {
      selectedId = items[cursorIndex + 1].id;
    } else if (offset + items.length < total) {
      offset += PAGE_SIZE;
      await load();
    }
  }

  async function goPrev() {
    if (cursorIndex > 0) {
      selectedId = items[cursorIndex - 1].id;
    } else if (offset > 0) {
      offset = Math.max(0, offset - PAGE_SIZE);
      await load();
      if (items.length > 0) selectedId = items[items.length - 1].id;
    }
  }

  function onKey(ev: KeyboardEvent) {
    if (loading || items.length === 0) return;
    // Don't intercept when the user is typing in an input/textarea.
    const target = ev.target as HTMLElement | null;
    if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) return;

    const key = ev.key.toLowerCase();
    if (key === 'j') { goNext(); ev.preventDefault(); }
    else if (key === 'k') { goPrev(); ev.preventDefault(); }
    else if (key === 's' && mode === 'candidate') { goNext(); ev.preventDefault(); }
    else if (key === 'p' && mode === 'candidate' && selected) { act(selected.id, 'promote'); ev.preventDefault(); }
    else if (key === 'r' && mode === 'candidate' && selected) { act(selected.id, 'reject'); ev.preventDefault(); }
    else if (key === 'u' && mode === 'promoted' && selected) { act(selected.id, 'unpromote'); ev.preventDefault(); }
    else if (key === 'r' && mode === 'rejected' && selected) { act(selected.id, 'restore'); ev.preventDefault(); }
  }

  onMount(() => load());
</script>

<svelte:window onkeydown={onKey} />

<div class="layout">
  {#if error}
    <div class="error">{error}</div>
  {/if}

  {#if !loading && items.length === 0}
    <div class="empty">
      {#if mode === 'candidate'}
        No candidates. Generate a SOAP note with capture enabled to populate this list.
      {:else if mode === 'promoted'}
        No promoted candidates yet. Promote a candidate to add it to the training corpus.
      {:else}
        No rejected candidates.
      {/if}
    </div>
  {:else}
    <div class="grid">
      <aside class="master" aria-label="Candidate list">
        {#each items as g (g.id)}
          <MasterRow
            generation={g}
            selected={g.id === selectedId}
            onclick={() => (selectedId = g.id)}
          />
        {/each}
        {#if total > PAGE_SIZE}
          <nav class="pagination">
            <button disabled={offset === 0} onclick={() => { offset = Math.max(0, offset - PAGE_SIZE); load(); }}>← Prev</button>
            <span>{offset + 1}–{Math.min(offset + PAGE_SIZE, total)} of {total}</span>
            <button disabled={offset + PAGE_SIZE >= total} onclick={() => { offset += PAGE_SIZE; load(); }}>Next →</button>
          </nav>
        {/if}
      </aside>
      <section class="detail-col" aria-label="Selected candidate detail">
        <DetailPane
          generation={selected}
          mode={mode}
          loading={loading}
          position={position}
          onAction={act}
        />
      </section>
    </div>
    <p class="kbd-hint">
      <kbd>J</kbd>/<kbd>K</kbd> navigate
      {#if mode === 'candidate'}· <kbd>P</kbd> promote · <kbd>R</kbd> reject · <kbd>S</kbd> skip{/if}
      {#if mode === 'promoted'}· <kbd>U</kbd> unpromote{/if}
      {#if mode === 'rejected'}· <kbd>R</kbd> restore{/if}
    </p>
  {/if}
</div>

<style>
  .layout { display: flex; flex-direction: column; gap: 0.4rem; }
  .error { padding: 0.5rem; background: #fee; color: #991b1b; border-radius: 4px; }
  .empty { padding: 1rem; color: var(--muted-foreground, #888); }
  .grid {
    display: grid; grid-template-columns: 240px 1fr;
    border: 1px solid var(--border, #1c2128);
    border-radius: 6px;
    min-height: 420px;
    overflow: hidden;
  }
  .master {
    border-right: 1px solid var(--border, #1c2128);
    overflow-y: auto;
    max-height: 70vh;
  }
  .detail-col { overflow: hidden; }
  .pagination { display: flex; gap: 0.5rem; align-items: center; padding: 0.5rem; font-size: 0.75rem; }
  .pagination button { padding: 0.25rem 0.6rem; cursor: pointer; }
  .kbd-hint { font-size: 0.8rem; color: var(--muted-foreground, #888); margin: 0.4rem 0 0 0; }
  kbd {
    background: var(--muted, #f5f5f5); border: 1px solid var(--border, #ccc);
    border-bottom-width: 2px; padding: 0.1rem 0.35rem; border-radius: 3px;
    font-family: var(--font-mono, monospace); font-size: 0.8rem;
  }
</style>
```

- [ ] **Step 2: Verify it type-checks**

Run:

```bash
npm run check
```

Expected: no new errors in `ReviewLayout.svelte`.

- [ ] **Step 3: Commit**

```bash
git add src/lib/components/settings/training_corpus/ReviewLayout.svelte
git commit -m "feat(training-corpus): ReviewLayout owns master+detail and keyboard nav"
```

---

## Task 7: Wire the three list components and delete `GenerationCard`

**Files:**
- Modify: `src/lib/components/settings/training_corpus/CandidatesList.svelte`
- Modify: `src/lib/components/settings/training_corpus/PromotedList.svelte`
- Modify: `src/lib/components/settings/training_corpus/RejectedList.svelte`
- Delete: `src/lib/components/settings/training_corpus/GenerationCard.svelte`

**Why:** This is the integration step where the new layout becomes live. `CandidatesList` and `RejectedList` shrink to a single line; `PromotedList` keeps its Export toolbar wrapper but delegates the queue rendering.

- [ ] **Step 1: Replace `CandidatesList.svelte`**

Overwrite `src/lib/components/settings/training_corpus/CandidatesList.svelte` with:

```svelte
<script lang="ts">
  import ReviewLayout from './ReviewLayout.svelte';
  let { onchange }: { onchange: () => void } = $props();
</script>

<ReviewLayout mode="candidate" {onchange} />
```

- [ ] **Step 2: Replace `RejectedList.svelte`**

Overwrite `src/lib/components/settings/training_corpus/RejectedList.svelte` with:

```svelte
<script lang="ts">
  import ReviewLayout from './ReviewLayout.svelte';
  let { onchange }: { onchange: () => void } = $props();
</script>

<ReviewLayout mode="rejected" {onchange} />
```

- [ ] **Step 3: Replace `PromotedList.svelte` while keeping the Export toolbar**

Overwrite `src/lib/components/settings/training_corpus/PromotedList.svelte` with:

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import ReviewLayout from './ReviewLayout.svelte';
  import ExportDialog from './ExportDialog.svelte';

  type Generation = {
    id: string;
    ai_model: string;
  };

  let { onchange }: { onchange: () => void } = $props();

  // The toolbar shows distinct models from the promoted set. We do a single
  // separate fetch here (paginated, model field only) — this is cheap and
  // keeps ReviewLayout free of toolbar concerns.
  let allPromoted: Generation[] = $state([]);
  let total = $state(0);
  let showExport = $state(false);
  let successMessage: string | null = $state(null);

  async function loadModels() {
    try {
      const page = await invoke<{ items: Generation[]; total: number }>(
        'training_corpus_list',
        { status: 'promoted', limit: 200, offset: 0 },
      );
      allPromoted = page.items;
      total = page.total;
    } catch {
      allPromoted = [];
      total = 0;
    }
  }

  function distinctModels(): string[] {
    return Array.from(new Set(allPromoted.map((g) => g.ai_model))).sort();
  }

  function onChildChange() {
    loadModels();
    onchange?.();
  }

  onMount(loadModels);
</script>

<div class="promoted-wrap">
  <div class="promoted-toolbar">
    <button onclick={() => (showExport = true)} disabled={total === 0}>
      Export training corpus…
    </button>
    {#if successMessage}<span class="success">{successMessage}</span>{/if}
  </div>

  <ReviewLayout mode="promoted" onchange={onChildChange} />
</div>

{#if showExport}
  <ExportDialog
    promotedCount={total}
    availableModels={distinctModels()}
    onclose={() => (showExport = false)}
    onsuccess={(dir, pairs, warnings) => {
      showExport = false;
      successMessage =
        `Exported ${pairs} pair${pairs === 1 ? '' : 's'} to ${dir}` +
        (warnings > 0 ? ` (${warnings} redaction warning${warnings === 1 ? '' : 's'} — see manifest.json)` : '');
    }}
  />
{/if}

<style>
  .promoted-wrap { display: flex; flex-direction: column; gap: 0.5rem; }
  .promoted-toolbar { display: flex; align-items: center; gap: 0.75rem; padding: 0.25rem 0; }
  .success { font-size: 0.85rem; color: #166534; background: #dcfce7; padding: 0.3rem 0.6rem; border-radius: 4px; flex: 1; }
</style>
```

- [ ] **Step 4: Delete `GenerationCard.svelte`**

Run:

```bash
git rm src/lib/components/settings/training_corpus/GenerationCard.svelte
```

- [ ] **Step 5: Verify nothing else imports `GenerationCard`**

Run:

```bash
grep -rn "GenerationCard" src/ 2>/dev/null
```

Expected: no matches. (The wrappers from Steps 1–3 no longer import it; nothing else should either.)

- [ ] **Step 6: Verify everything still type-checks and tests still pass**

Run:

```bash
npm run check
npx vitest run
```

Expected: no new TypeScript errors; all existing + new tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/lib/components/settings/training_corpus/
git commit -m "feat(training-corpus): wire ReviewLayout into all three tabs; remove GenerationCard"
```

---

## Task 8: Manual smoke test

**Files:** none (verification only)

**Why:** Per `CLAUDE.md`: "For UI or frontend changes, start the dev server and use the feature in a browser before reporting the task as complete." No Svelte component test framework is in this repo, so manual smoke is how visual + interaction correctness is verified.

- [ ] **Step 1: Start the dev environment**

Run in one terminal:

```bash
npm run tauri dev
```

Wait for the Tauri window to open.

- [ ] **Step 2: Populate the corpus if it's empty**

If you don't already have generations: record a short consultation, generate a SOAP note (capture must be enabled — see the Training Corpus settings panel), edit a word or two, save. Repeat 2–3 times so there are multiple candidates to walk through. (If the user/tester already has a populated dev DB, this step can be skipped.)

- [ ] **Step 3: Verify Candidates tab**

Open Settings → Training Corpus → Candidates. Confirm each item below.

- [ ] Master column shows date, model, `light` / `moderate` / `heavy` chip, and a `−`/`+` snippet for each row.
- [ ] First row is selected by default (blue left border + tinted background).
- [ ] Detail pane shows: date+time in header, model + chip with "N% changed", Promote and Reject buttons top-right.
- [ ] Diff body is line-level: context lines neutral, removed lines red-tinted with `−`, added lines green-tinted with `+`.
- [ ] Click another master row → detail pane updates.
- [ ] Press `J` → selection moves down; `K` → up. Wrapping at top/bottom is a no-op (or pages if there are more pages).
- [ ] Press `P` → row is promoted, leaves the candidate queue, selection moves to the next item, header count "X candidates" decrements.
- [ ] Press `R` → row is rejected, same behavior.
- [ ] Press `S` → selection advances without acting.

- [ ] **Step 4: Verify Promoted tab**

- [ ] Same master+detail layout.
- [ ] "Export training corpus…" toolbar button is still present above the layout.
- [ ] Detail pane shows only "Unpromote (U)" — no Promote / Reject.
- [ ] Press `U` → row is unpromoted (returns to Candidates), promoted count decrements.

- [ ] **Step 5: Verify Rejected tab**

- [ ] Same layout, "Restore (R)" button in detail pane.
- [ ] Press `R` → row is restored.

- [ ] **Step 6: Verify pagination crossing**

If you don't have 50+ candidates, this is informational only; otherwise:
- [ ] J at the bottom of page 1 advances to page 2 and selects the first item of the new page.
- [ ] K at the top of page 2 rewinds to page 1 and selects the last item.

- [ ] **Step 7: Verify the null-final filter**

If you have a generation with no saved final (unusual — would happen if a draft was generated but never saved):
- [ ] It does NOT appear in Candidates.
- [ ] If it had been previously promoted/rejected, it DOES appear in those tabs with the "no save" chip and a draft-only body (no diff).

- [ ] **Step 8: Verify no PHI in logs**

In the running dev terminal, watch the stdout output while clicking/keying through. Per the project's hard constraint, none of the SOAP content (draft text, final text, diff hunks, snippets) may appear in `tracing::*`, `println!`, `console.log`, or `eprintln!`. The new code in this plan doesn't introduce any such logging — but confirm by visual inspection of the terminal during smoke.

- [ ] **Step 9: Final commit if any cleanup needed**

If the smoke test surfaced anything that needs fixing, fix it now and commit:

```bash
git add <files>
git commit -m "fix(training-corpus): <what>"
```

If nothing needs fixing, no commit is required — proceed to merge.

---

## Self-review notes

- **Spec coverage:** Every section in the spec is implemented. Backend filter → Task 3. `diff.ts` → Task 2. `MasterRow` → Task 4. `DetailPane` → Task 5. `ReviewLayout` → Task 6. Wiring → Task 7. Manual smoke → Task 8.
- **Component tests:** The spec called for Vitest component tests; this plan reduces those to manual smoke because the repo has no `@testing-library/svelte`. The logic-bearing code (`diff.ts`) is fully unit-tested. This is a deliberate scope-trim — adding the test library would be its own task and is out of scope for this plan. Spec's "Testing" section should be considered partially superseded here.
- **Keyboard nav extension:** The spec adds cross-page J/K. Task 6 implements this in `goNext` / `goPrev`.
- **No PHI leak:** No `console.log`, `tracing`, or `println!` of generation content introduced. Errors logged are stringified `e` from `invoke`, which contains DB error messages, not generation content.
- **No new providers:** Local-only AI constraint untouched.
- **Type consistency:** `Generation` type duplicated identically across `MasterRow`, `DetailPane`, `ReviewLayout`, and the wrapper components (matches the existing codebase style — same duplication is in the current `CandidatesList` / `PromotedList` / `RejectedList`). Action union is consistent: `'promote' | 'reject' | 'unpromote' | 'restore'`. Mode union is consistent: `'candidate' | 'promoted' | 'rejected'`.
