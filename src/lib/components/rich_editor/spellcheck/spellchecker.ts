// Spellchecker wrapper. Loads the en-US Hunspell dictionary once via Vite
// asset imports (bundled at build time — no runtime network requests, no PHI
// leakage, complies with the local-only constraint), and combines its
// suggestions with a per-user wordlist persisted in SQLite + an in-memory
// session ignore set.
import nspell from 'nspell';
// Vite resolves these at build time and inlines them as URLs.
// dictionary-en ships the Hunspell .aff/.dic files via its package exports.
import affUrl from 'dictionary-en/index.aff?url';
import dicUrl from 'dictionary-en/index.dic?url';
import { listUserDict, addUserDict, removeUserDict } from '../../../api/userDictionary';

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
  /** Remove from user dictionary (persisted). Returns true if a row was deleted. */
  removeFromUserDict(word: string): Promise<boolean>;
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
        listUserDict().catch(() => [] as string[]),
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
    const newlyAdded = await addUserDict(word);
    this.userWords.add(word.toLowerCase());
    return newlyAdded;
  }

  async removeFromUserDict(word: string): Promise<boolean> {
    const removed = await removeUserDict(word);
    this.userWords.delete(word.toLowerCase());
    return removed;
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
