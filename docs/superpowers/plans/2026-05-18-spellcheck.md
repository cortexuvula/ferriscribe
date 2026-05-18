# In-App Spellcheck with Custom Wordlist — Spec & Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the broken Tauri+WKWebView OS spellcheck with an in-app Hunspell-based spellchecker. Red underline misspelled words. Right-click shows custom menu with suggestions, "Add to dictionary," and "Ignore." Per-user dictionary persisted in SQLite. Cross-platform identical behaviour.

**Architecture:** `nspell` (Hunspell-compatible, MIT) loaded with `dictionary-en` (~600 KB compressed en-US, tri-licensed). Bundled with the app via Vite asset imports — no external fetches. Tiptap extension uses ProseMirror decorations for squiggles. Svelte component for the menu. New `user_dictionary` SQLite table + Rust Tauri commands.

**Tech Stack:** nspell, dictionary-en, Tiptap v2, Svelte 5 runes, SQLite via rusqlite, existing AppState/DbResult plumbing.

---

## Design summary

- **Browser-native spellcheck attribute set to `false`** on the contenteditable so we don't get duplicate squiggles. Our custom decoration draws the underline.
- **Dictionary loaded once per session.** Lazy — first edit triggers async load. Until loaded, no squiggles (silent degraded mode).
- **Per-paragraph debounced spellcheck.** Re-scan only the paragraph that was edited, not the whole document.
- **Custom wordlist as suppression list.** Words on the list never get flagged; "Add to dictionary" from the menu writes to it. No medical-dict pre-population in v1 — that's a follow-up.
- **In-memory + persisted.** Frontend keeps a `Set<string>` mirror of the wordlist for synchronous spellcheck; loaded on mount via Tauri command; mutations write through.
- **Session ignore list.** "Ignore" suppresses for the rest of the session, not persisted. (Distinct from "Add to dictionary.")
- **No PHI in logs.** Spellchecker logs at most counts ("scanned N paragraphs, M misspellings"). Never word content.

### Non-goals

- Multi-language dictionaries (English-only in v1).
- Grammar checking.
- A curated medical dictionary (hook is in place; populating it is a follow-up).
- Replacement of mark/format states under the squiggle (squiggles render OVER bold/italic without interference).

---

## File-level decomposition

### Backend (new)

- `crates/db/src/migrations/m00X_user_dictionary.rs` — new migration creating the table
- `crates/db/src/user_dictionary.rs` — `UserDictionaryRepo` with `list`, `add`, `remove`, `contains`
- `crates/db/src/lib.rs` — re-export module
- `src-tauri/src/commands/user_dictionary.rs` — Tauri commands `user_dict_list`, `user_dict_add`, `user_dict_remove`
- `src-tauri/src/commands/mod.rs` + `src-tauri/src/lib.rs` — register the new commands

### Frontend (new)

- `src/lib/components/rich_editor/spellcheck/spellchecker.ts` — wraps nspell, manages dictionary loading, exposes `check(word) -> boolean`, `suggest(word) -> string[]`, `addToUserDict(word)`, `ignoreInSession(word)`
- `src/lib/components/rich_editor/spellcheck/spellcheck_extension.ts` — Tiptap extension exporting a ProseMirror plugin with decorations + contextmenu handler
- `src/lib/components/rich_editor/spellcheck/SpellcheckMenu.svelte` — the custom contextmenu UI

### Frontend (modified)

- `src/lib/components/RichEditor.svelte` — mount the extension, render the menu, set `spellcheck: 'false'` on the editor
- `package.json` — add `nspell` and `dictionary-en` deps

### Tests

- `crates/db/src/user_dictionary.rs` — repo tests (list/add/remove/contains/dedup)
- `src/lib/components/rich_editor/spellcheck/spellchecker.test.ts` — wraps a tiny test dictionary, asserts `check`/`suggest`/`addToUserDict` behave

---

## Acceptance criteria

1. Existing notes load and display squiggles under English misspellings (e.g. `teh`, `xxxyz`) after the dictionary loads.
2. Right-click on a squiggled word shows a menu with: up to 5 suggestions, "Add to dictionary," "Ignore," "Cancel."
3. Clicking a suggestion replaces the word in place and clears the squiggle.
4. "Add to dictionary" persists the word in SQLite; reopening the app keeps it in the dictionary; no squiggle appears for that word.
5. "Ignore" clears the squiggle for the rest of the session; reopening restores the squiggle.
6. Browser-native spellcheck (duplicate squiggles) does NOT appear.
7. Right-clicking a CORRECTLY spelled word falls through to the OS default context menu (so users still get Cut/Copy/Paste).
8. `cargo test -p medical-db --lib user_dictionary` passes.
9. `npx vitest run` passes (including new spellchecker tests).
10. `npm run check` passes.
11. No console logs include misspelled-word content. Counts/lengths only.
12. App startup is not slowed; dictionary loads lazily on first editor mount.

---

## Tasks

### Task 1: SQLite `user_dictionary` table + repo

**Files:**
- Create: `crates/db/src/migrations/m00X_user_dictionary.rs` (X = next migration number — discover by listing `crates/db/src/migrations/`)
- Create: `crates/db/src/user_dictionary.rs`
- Modify: `crates/db/src/lib.rs` (re-export + add migration to runner)

- [ ] **Step 1: Discover the next migration number**

Run: `ls crates/db/src/migrations/ | sort`
Identify the highest `m###_*.rs` number. New migration name: `m00<next>_user_dictionary.rs`.

- [ ] **Step 2: Write the migration**

Create the file with this content (adjust `m00X` to actual number):

```rust
//! User-dictionary table: per-user wordlist of accepted spellings.
//! Words on this list are not flagged by the in-app spellchecker.

use rusqlite::{Connection, Result};

pub fn up(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS user_dictionary (
            id          INTEGER PRIMARY KEY,
            word        TEXT NOT NULL,
            added_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_user_dictionary_word_nocase
          ON user_dictionary (LOWER(word));
        "#,
    )?;
    Ok(())
}
```

- [ ] **Step 3: Register the migration**

In `crates/db/src/lib.rs`, find the migration runner and add the new migration to its ordered list. Follow the existing pattern.

- [ ] **Step 4: Write `UserDictionaryRepo`**

Create `crates/db/src/user_dictionary.rs`:

```rust
//! Per-user dictionary of accepted spellings.

use crate::DbResult;
use rusqlite::{Connection, params};

pub struct UserDictionaryRepo;

impl UserDictionaryRepo {
    pub fn list(conn: &Connection) -> DbResult<Vec<String>> {
        let mut stmt = conn.prepare("SELECT word FROM user_dictionary ORDER BY LOWER(word)")?;
        let words = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(words)
    }

    /// Returns true if the word was newly added, false if it already existed.
    pub fn add(conn: &Connection, word: &str) -> DbResult<bool> {
        let trimmed = word.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        let changed = conn.execute(
            "INSERT OR IGNORE INTO user_dictionary (word) VALUES (?1)",
            params![trimmed],
        )?;
        Ok(changed > 0)
    }

    pub fn remove(conn: &Connection, word: &str) -> DbResult<bool> {
        let changed = conn.execute(
            "DELETE FROM user_dictionary WHERE LOWER(word) = LOWER(?1)",
            params![word],
        )?;
        Ok(changed > 0)
    }

    pub fn contains(conn: &Connection, word: &str) -> DbResult<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM user_dictionary WHERE LOWER(word) = LOWER(?1)",
            params![word],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{open_in_memory_for_test, run_migrations};

    fn fresh() -> Connection {
        let conn = open_in_memory_for_test();
        run_migrations(&conn).expect("migrate");
        conn
    }

    #[test]
    fn add_then_list_returns_word() {
        let conn = fresh();
        assert!(UserDictionaryRepo::add(&conn, "atenolol").unwrap());
        assert_eq!(UserDictionaryRepo::list(&conn).unwrap(), vec!["atenolol"]);
    }

    #[test]
    fn add_is_idempotent_case_insensitive() {
        let conn = fresh();
        assert!(UserDictionaryRepo::add(&conn, "Lisinopril").unwrap());
        assert!(!UserDictionaryRepo::add(&conn, "lisinopril").unwrap());
        assert_eq!(UserDictionaryRepo::list(&conn).unwrap().len(), 1);
    }

    #[test]
    fn contains_is_case_insensitive() {
        let conn = fresh();
        UserDictionaryRepo::add(&conn, "metformin").unwrap();
        assert!(UserDictionaryRepo::contains(&conn, "METFORMIN").unwrap());
        assert!(UserDictionaryRepo::contains(&conn, "metformin").unwrap());
        assert!(!UserDictionaryRepo::contains(&conn, "unknown").unwrap());
    }

    #[test]
    fn remove_returns_false_when_absent() {
        let conn = fresh();
        assert!(!UserDictionaryRepo::remove(&conn, "ghost").unwrap());
    }

    #[test]
    fn add_strips_whitespace_and_skips_empty() {
        let conn = fresh();
        assert!(!UserDictionaryRepo::add(&conn, "   ").unwrap());
        assert!(UserDictionaryRepo::add(&conn, "  word  ").unwrap());
        assert_eq!(UserDictionaryRepo::list(&conn).unwrap(), vec!["word"]);
    }
}
```

If `open_in_memory_for_test` or `run_migrations` don't exist with those names, look in `crates/db/src/lib.rs` for the project's existing test-DB helper and use that name. Update the test imports.

- [ ] **Step 5: Run tests**

Run: `cargo test -p medical-db --lib user_dictionary`
Expected: 5/5 pass.

- [ ] **Step 6: Full workspace lib tests**

Run: `cargo test --workspace --lib`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/db/
git commit -m "feat(db): add user_dictionary table for in-app spellcheck wordlist"
```

---

### Task 2: Tauri commands for the user dictionary

**Files:**
- Create: `src-tauri/src/commands/user_dictionary.rs`
- Modify: `src-tauri/src/commands/mod.rs` (re-export)
- Modify: `src-tauri/src/lib.rs` (register commands)

- [ ] **Step 1: Write the commands module**

Create `src-tauri/src/commands/user_dictionary.rs`:

```rust
//! Tauri commands for the in-app spellchecker's per-user wordlist.

use medical_core::error::{AppError, AppResult};
use medical_db::user_dictionary::UserDictionaryRepo;

use crate::state::AppState;

#[tauri::command]
pub async fn user_dict_list(state: tauri::State<'_, AppState>) -> AppResult<Vec<String>> {
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    UserDictionaryRepo::list(&conn).map_err(|e| AppError::Database(e.to_string()))
}

#[tauri::command]
pub async fn user_dict_add(
    state: tauri::State<'_, AppState>,
    word: String,
) -> AppResult<bool> {
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    UserDictionaryRepo::add(&conn, &word).map_err(|e| AppError::Database(e.to_string()))
}

#[tauri::command]
pub async fn user_dict_remove(
    state: tauri::State<'_, AppState>,
    word: String,
) -> AppResult<bool> {
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    UserDictionaryRepo::remove(&conn, &word).map_err(|e| AppError::Database(e.to_string()))
}
```

- [ ] **Step 2: Wire commands into the Tauri runtime**

In `src-tauri/src/commands/mod.rs`, add `pub mod user_dictionary;`.

In `src-tauri/src/lib.rs`, find the `.invoke_handler(tauri::generate_handler![...])` block and add the three commands by their fully-qualified path (mirror the existing entries' style, e.g. `commands::user_dictionary::user_dict_list`).

- [ ] **Step 3: Verify build**

Run: `cargo build -p rust-medical-assistant`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/
git commit -m "feat(commands): add user_dict_list/add/remove Tauri commands"
```

---

### Task 3: Frontend spellchecker wrapper

**Files:**
- Modify: `package.json` (add deps)
- Create: `src/lib/components/rich_editor/spellcheck/spellchecker.ts`
- Create: `src/lib/components/rich_editor/spellcheck/spellchecker.test.ts`

- [ ] **Step 1: Add deps**

In `package.json` dependencies:

```json
"nspell": "^2.1.5",
"dictionary-en": "^4.0.0"
```

Run: `npm install`
Expected: clean install. Note any audit findings in the commit message.

- [ ] **Step 2: Write the wrapper**

Create `src/lib/components/rich_editor/spellcheck/spellchecker.ts`:

```ts
// Spellchecker wrapper. Loads the en-US Hunspell dictionary once via Vite
// asset imports (bundled at build time — no runtime network requests, no PHI
// leakage, complies with the local-only constraint), and combines its
// suggestions with a per-user wordlist persisted in SQLite + an in-memory
// session ignore set.
import { invoke } from '@tauri-apps/api/core';
import nspell from 'nspell';
// Vite resolves these at build time and inlines them as URLs.
// dictionary-en ships the Hunspell .aff/.dic files via its package exports.
import affUrl from 'dictionary-en/index.aff?url';
import dicUrl from 'dictionary-en/index.dic?url';

export interface Spellchecker {
  /** Returns true once the dictionary has loaded. */
  readonly ready: boolean;
  /** Returns true when the word is in the dictionary OR user wordlist
   *  OR session ignore list. */
  check(word: string): boolean;
  /** Up to N suggestions for a misspelled word (empty if no suggestions). */
  suggest(word: string, max?: number): string[];
  /** Add to user dictionary (persisted). Returns true if newly added. */
  addToUserDict(word: string): Promise<boolean>;
  /** Add to session ignore (not persisted). */
  ignoreInSession(word: string): void;
}

class SpellcheckerImpl implements Spellchecker {
  private nspell: ReturnType<typeof nspell> | null = null;
  private userWords = new Set<string>(); // lowercased
  private sessionIgnored = new Set<string>(); // lowercased
  private loadingPromise: Promise<void> | null = null;

  get ready(): boolean {
    return this.nspell !== null;
  }

  async load(): Promise<void> {
    if (this.loadingPromise) return this.loadingPromise;
    this.loadingPromise = (async () => {
      const [affRes, dicRes, userListRaw] = await Promise.all([
        fetch(affUrl).then((r) => r.text()),
        fetch(dicUrl).then((r) => r.text()),
        invoke<string[]>('user_dict_list').catch(() => [] as string[]),
      ]);
      this.nspell = nspell(affRes, dicRes);
      for (const w of userListRaw) this.userWords.add(w.toLowerCase());
    })();
    return this.loadingPromise;
  }

  check(word: string): boolean {
    if (!this.nspell) return true; // pre-load: don't flag anything
    const lower = word.toLowerCase();
    if (this.userWords.has(lower)) return true;
    if (this.sessionIgnored.has(lower)) return true;
    return this.nspell.correct(word);
  }

  suggest(word: string, max = 5): string[] {
    if (!this.nspell) return [];
    return this.nspell.suggest(word).slice(0, max);
  }

  async addToUserDict(word: string): Promise<boolean> {
    const newlyAdded = await invoke<boolean>('user_dict_add', { word });
    this.userWords.add(word.toLowerCase());
    return newlyAdded;
  }

  ignoreInSession(word: string): void {
    this.sessionIgnored.add(word.toLowerCase());
  }
}

let singleton: SpellcheckerImpl | null = null;

/**
 * Get the lazily-initialized singleton spellchecker. The first call starts
 * the async dictionary load; subsequent calls return the same instance.
 * Use `.ready` to gate UI that depends on the dictionary being loaded.
 */
export function getSpellchecker(): Spellchecker & { load(): Promise<void> } {
  if (!singleton) singleton = new SpellcheckerImpl();
  return singleton as unknown as Spellchecker & { load(): Promise<void> };
}
```

- [ ] **Step 3: Write the test**

Create `src/lib/components/rich_editor/spellcheck/spellchecker.test.ts`:

```ts
// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';

// We can't easily mock the real dictionary in unit tests. Instead, verify
// that the wrapper behaves correctly when the underlying nspell is stubbed.
// Real-dictionary smoke-test is left to integration (running the app).

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'user_dict_list') return ['atenolol'];
    if (cmd === 'user_dict_add') return true;
    return null;
  }),
}));

vi.mock('dictionary-en/index.aff?url', () => ({ default: '/test.aff' }));
vi.mock('dictionary-en/index.dic?url', () => ({ default: '/test.dic' }));

vi.mock('nspell', () => ({
  default: () => ({
    correct: (w: string) => ['cat', 'dog', 'patient'].includes(w),
    suggest: (w: string) => (w === 'paitent' ? ['patient', 'patent'] : []),
  }),
}));

// Stub global fetch since the wrapper fetches the dict URLs.
beforeEach(() => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => ({ text: async () => '' })),
  );
});

describe('Spellchecker wrapper', () => {
  it('returns true before load (degraded mode)', async () => {
    const { getSpellchecker } = await import('./spellchecker');
    const s = getSpellchecker();
    expect(s.ready).toBe(false);
    expect(s.check('xxxyz')).toBe(true);
  });

  it('flags unknown words and returns suggestions after load', async () => {
    const mod = await import('./spellchecker');
    // Re-import isolated: vitest module cache may persist; reset by clearing modules.
    vi.resetModules();
    const { getSpellchecker } = await import('./spellchecker');
    const s = getSpellchecker() as ReturnType<typeof getSpellchecker> & { load: () => Promise<void> };
    await s.load();
    expect(s.ready).toBe(true);
    expect(s.check('cat')).toBe(true);
    expect(s.check('paitent')).toBe(false);
    expect(s.suggest('paitent')).toEqual(['patient', 'patent']);
  });

  it('accepts words from the persisted user dictionary', async () => {
    vi.resetModules();
    const { getSpellchecker } = await import('./spellchecker');
    const s = getSpellchecker() as ReturnType<typeof getSpellchecker> & { load: () => Promise<void> };
    await s.load();
    expect(s.check('atenolol')).toBe(true); // present in mocked user_dict_list
  });

  it('addToUserDict persists and unflags the word', async () => {
    vi.resetModules();
    const { getSpellchecker } = await import('./spellchecker');
    const s = getSpellchecker() as ReturnType<typeof getSpellchecker> & { load: () => Promise<void> };
    await s.load();
    expect(s.check('lisinopril')).toBe(false);
    await s.addToUserDict('lisinopril');
    expect(s.check('lisinopril')).toBe(true);
  });

  it('ignoreInSession unflags the word for the session', async () => {
    vi.resetModules();
    const { getSpellchecker } = await import('./spellchecker');
    const s = getSpellchecker() as ReturnType<typeof getSpellchecker> & { load: () => Promise<void> };
    await s.load();
    expect(s.check('xxxyz')).toBe(false);
    s.ignoreInSession('xxxyz');
    expect(s.check('xxxyz')).toBe(true);
  });
});
```

- [ ] **Step 4: Run tests**

Run: `npx vitest run src/lib/components/rich_editor/spellcheck/`
Expected: 5/5 pass.

Adjust expectations if module caching makes assertions order-dependent. If a test reveals genuinely broken wrapper logic, fix the wrapper. Document any expectation adjustments in the commit body.

- [ ] **Step 5: Full vitest**

Run: `npx vitest run`
Expected: prior 238 + new 5 = 243/243.

- [ ] **Step 6: Commit**

```bash
git add package.json package-lock.json src/lib/components/rich_editor/spellcheck/
git commit -m "feat(editor): spellchecker wrapper with nspell + dictionary-en + user wordlist"
```

---

### Task 4: Tiptap extension with decorations and contextmenu

**Files:**
- Create: `src/lib/components/rich_editor/spellcheck/spellcheck_extension.ts`
- Create: `src/lib/components/rich_editor/spellcheck/SpellcheckMenu.svelte`

- [ ] **Step 1: Write the Tiptap extension**

Create `src/lib/components/rich_editor/spellcheck/spellcheck_extension.ts`:

```ts
import { Extension } from '@tiptap/core';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import { Decoration, DecorationSet } from '@tiptap/pm/view';
import type { EditorView } from '@tiptap/pm/view';
import { getSpellchecker } from './spellchecker';

const SPELLCHECK_PLUGIN_KEY = new PluginKey('spellcheck');

export interface SpellcheckContextMenuRequest {
  word: string;
  from: number; // ProseMirror position of word start
  to: number;
  clientX: number;
  clientY: number;
}

export interface SpellcheckOptions {
  /** Called when the user right-clicks a misspelled word. The host renders
   *  the menu at the given client coordinates and calls back into the
   *  editor via the commands below. */
  onContextMenu: (req: SpellcheckContextMenuRequest, view: EditorView) => void;
}

// Word boundary regex. \p{L} + \p{N} captures Unicode letters and numbers.
const WORD_RE = /[\p{L}\p{N}'-]+/gu;

function scanDecorations(doc: import('@tiptap/pm/model').Node): DecorationSet {
  const spell = getSpellchecker();
  if (!spell.ready) return DecorationSet.empty;
  const decos: Decoration[] = [];
  doc.descendants((node, pos) => {
    if (!node.isText || !node.text) return;
    const text = node.text;
    WORD_RE.lastIndex = 0;
    let match: RegExpExecArray | null;
    while ((match = WORD_RE.exec(text)) !== null) {
      const word = match[0];
      if (word.length < 2) continue;
      if (spell.check(word)) continue;
      const start = pos + match.index;
      const end = start + word.length;
      decos.push(
        Decoration.inline(start, end, { class: 'spellcheck-misspelled' }),
      );
    }
  });
  return DecorationSet.create(doc, decos);
}

export const Spellcheck = Extension.create<SpellcheckOptions>({
  name: 'spellcheck',

  addOptions(): SpellcheckOptions {
    return { onContextMenu: () => {} };
  },

  addProseMirrorPlugins() {
    const opts = this.options;
    // Kick off async dictionary load and re-scan when ready.
    const spell = getSpellchecker();
    if (!spell.ready) {
      // Cast: `load` exists on the impl, not the public interface.
      (spell as unknown as { load: () => Promise<void> }).load().then(() => {
        // After load, request a re-scan by sending a no-op transaction
        // through every active editor view we know of. ProseMirror plugin
        // state.apply will re-decorate. We use a global flag the plugin
        // checks on every transaction.
        DICT_LOADED = true;
      });
    } else {
      DICT_LOADED = true;
    }

    return [
      new Plugin({
        key: SPELLCHECK_PLUGIN_KEY,
        state: {
          init: (_cfg, state) => scanDecorations(state.doc),
          apply(tr, oldSet, oldState, newState) {
            if (tr.docChanged || DICT_JUST_LOADED()) {
              return scanDecorations(newState.doc);
            }
            return oldSet.map(tr.mapping, tr.doc);
          },
        },
        props: {
          decorations(state) {
            return this.getState(state) ?? DecorationSet.empty;
          },
          handleDOMEvents: {
            contextmenu(view, event) {
              const me = event as MouseEvent;
              const pos = view.posAtCoords({ left: me.clientX, top: me.clientY });
              if (!pos) return false;
              const word = wordAtPos(view, pos.pos);
              if (!word) return false;
              const spell = getSpellchecker();
              if (spell.check(word.text)) return false; // not misspelled — let OS menu show
              opts.onContextMenu(
                {
                  word: word.text,
                  from: word.from,
                  to: word.to,
                  clientX: me.clientX,
                  clientY: me.clientY,
                },
                view,
              );
              event.preventDefault();
              return true;
            },
          },
        },
      }),
    ];
  },
});

// Trivial signal: flip a module-level boolean when the dictionary loads.
// The plugin's `apply` is called on every transaction, so we observe the
// flip there. After the first scan we clear the just-loaded flag.
let DICT_LOADED = false;
let SEEN_LOADED = false;
function DICT_JUST_LOADED(): boolean {
  if (DICT_LOADED && !SEEN_LOADED) {
    SEEN_LOADED = true;
    return true;
  }
  return false;
}

function wordAtPos(
  view: EditorView,
  pos: number,
): { text: string; from: number; to: number } | null {
  const $pos = view.state.doc.resolve(pos);
  const node = $pos.parent;
  if (!node.isTextblock) return null;
  const text = node.textContent;
  const offsetInBlock = pos - $pos.start();
  WORD_RE.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = WORD_RE.exec(text)) !== null) {
    const start = match.index;
    const end = start + match[0].length;
    if (offsetInBlock >= start && offsetInBlock <= end) {
      return {
        text: match[0],
        from: $pos.start() + start,
        to: $pos.start() + end,
      };
    }
  }
  return null;
}
```

- [ ] **Step 2: Write the menu component**

Create `src/lib/components/rich_editor/spellcheck/SpellcheckMenu.svelte`:

```svelte
<script lang="ts">
  import type { Editor } from '@tiptap/core';
  import { getSpellchecker } from './spellchecker';
  import type { SpellcheckContextMenuRequest } from './spellcheck_extension';

  interface Props {
    editor: Editor | null;
    request: SpellcheckContextMenuRequest | null;
    onClose: () => void;
  }

  let { editor, request, onClose }: Props = $props();

  const spell = getSpellchecker();

  // Recompute suggestions when the request changes.
  const suggestions = $derived(
    request ? spell.suggest(request.word, 5) : [],
  );

  function applySuggestion(s: string) {
    if (!editor || !request) return;
    editor
      .chain()
      .focus()
      .insertContentAt({ from: request.from, to: request.to }, s)
      .run();
    onClose();
  }

  async function addToDictionary() {
    if (!request) return;
    await spell.addToUserDict(request.word);
    // Force a re-scan: insert a no-op transaction.
    editor?.chain().focus().run();
    onClose();
  }

  function ignoreOnce() {
    if (!request) return;
    spell.ignoreInSession(request.word);
    editor?.chain().focus().run();
    onClose();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if request}
  <div
    class="spellcheck-menu"
    style="left: {request.clientX}px; top: {request.clientY}px"
    role="menu"
    aria-label="Spelling suggestions for {request.word}"
  >
    {#if suggestions.length === 0}
      <div class="empty">No suggestions</div>
    {:else}
      {#each suggestions as s}
        <button type="button" role="menuitem" onclick={() => applySuggestion(s)}>
          {s}
        </button>
      {/each}
    {/if}
    <div class="sep" aria-hidden="true"></div>
    <button type="button" role="menuitem" onclick={addToDictionary}>
      Add &ldquo;{request.word}&rdquo; to dictionary
    </button>
    <button type="button" role="menuitem" onclick={ignoreOnce}>Ignore</button>
    <button type="button" role="menuitem" onclick={onClose}>Cancel</button>
  </div>
{/if}

<style>
  .spellcheck-menu {
    position: fixed;
    z-index: 100;
    min-width: 200px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.18);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .spellcheck-menu button {
    background: transparent;
    color: var(--text-primary);
    border: none;
    border-radius: 4px;
    padding: 6px 10px;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
    white-space: nowrap;
  }
  .spellcheck-menu button:hover { background-color: var(--bg-hover); }
  .empty {
    padding: 6px 10px;
    color: var(--text-muted, var(--text-secondary));
    font-size: 13px;
    font-style: italic;
  }
  .sep {
    height: 1px;
    background-color: var(--border);
    margin: 4px 0;
  }
</style>
```

- [ ] **Step 3: Verify build**

Run: `npm run check`
Expected: 0 errors, 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add src/lib/components/rich_editor/spellcheck/spellcheck_extension.ts \
        src/lib/components/rich_editor/spellcheck/SpellcheckMenu.svelte
git commit -m "feat(editor): Tiptap spellcheck extension + custom suggestion menu"
```

---

### Task 5: Wire spellcheck into RichEditor + global styles

**Files:**
- Modify: `src/lib/components/RichEditor.svelte` (add extension + menu)
- Modify: `src/app.css` OR add scoped global style in `RichEditor.svelte` for the `.spellcheck-misspelled` decoration

- [ ] **Step 1: Add the misspelling underline CSS**

The decoration emits `class="spellcheck-misspelled"` on inline ranges. Since it's a global class applied inside the Tiptap editor, the cleanest place is a `:global` rule in `RichEditor.svelte`'s `<style>` block:

```css
:global(.spellcheck-misspelled) {
  text-decoration: underline wavy var(--accent, #d33);
  text-decoration-skip-ink: none;
  text-underline-offset: 2px;
}
```

- [ ] **Step 2: Mount the extension + menu in RichEditor.svelte**

In the Tiptap `extensions` array, add:

```ts
Spellcheck.configure({
  onContextMenu: (req) => {
    spellcheckRequest = req;
  },
}),
```

Add the import:

```ts
import { Spellcheck, type SpellcheckContextMenuRequest } from './rich_editor/spellcheck/spellcheck_extension';
import SpellcheckMenu from './rich_editor/spellcheck/SpellcheckMenu.svelte';
```

Add the state binding:

```ts
let spellcheckRequest = $state<SpellcheckContextMenuRequest | null>(null);
```

Change `spellcheck: 'true'` in `editorProps.attributes` to `spellcheck: 'false'` (our extension replaces browser native).

In the markup, alongside `<FindPanel>`, add:

```svelte
<SpellcheckMenu
  {editor}
  request={spellcheckRequest}
  onClose={() => (spellcheckRequest = null)}
/>
```

- [ ] **Step 3: Click-outside dismiss**

The `SpellcheckMenu` Escape handler is via `svelte:window onkeydown`. Add a click-outside handler too: in `SpellcheckMenu.svelte`, listen for `mousedown` on `svelte:window` and call `onClose()` if the click is outside the menu element. Use `bind:this` on the menu div.

- [ ] **Step 4: Verify build + tests**

Run: `npm run check` → 0/0.
Run: `npx vitest run` → 243/243.

- [ ] **Step 5: Commit**

```bash
git add src/lib/components/RichEditor.svelte src/lib/components/rich_editor/spellcheck/SpellcheckMenu.svelte
git commit -m "feat(editor): mount spellcheck extension and contextmenu in RichEditor"
```

---

### Task 6: Acceptance walk + version bump

**Steps:**

- [ ] **Step 1: Walk acceptance criteria**

For each of the 12 items, classify as Auto (testable) or Manual (requires running the app). Run the auto checks; document the manual ones for user-driven smoke testing.

- [ ] **Step 2: PHI-log audit**

```bash
grep -nE "console\.(log|error|warn)" src/lib/components/rich_editor/spellcheck/ src/lib/components/RichEditor.svelte
```

Expected: empty or counts-only.

- [ ] **Step 3: Type-check + full tests**

Run: `npm run check` → 0/0.
Run: `npx vitest run` → 243/243.
Run: `cargo test --workspace --lib` → all green.

- [ ] **Step 4: Bump version**

Bump `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` to 0.10.78. Run `cargo build -p rust-medical-assistant` to update Cargo.lock.

- [ ] **Step 5: Commit + tag**

```bash
git add Cargo.lock package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json
git commit -m "chore: bump 0.10.78 — in-app spellcheck with custom wordlist"
git push origin master
git tag v0.10.78
git push origin v0.10.78
```

---

## Out of scope (deferred)

- Multi-language dictionaries
- Curated medical dictionary preload
- Grammar checking
- Auto-correction
- Performance optimization for very large documents (the per-paragraph scan is fine for typical SOAP notes)
