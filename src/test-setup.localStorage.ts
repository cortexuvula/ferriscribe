// Setup file: provides a full localStorage polyfill for Node 25+.
//
// Node 25 ships a built-in global `localStorage` backed by
// `--localstorage-file`, but when that flag is absent the object is a stub
// that lacks `.clear()`, `.getItem()`, `.setItem()`, etc.  Vitest's jsdom
// environment does NOT override this stub because `localStorage` is not in its
// "KEYS" allowlist for global promotion.
//
// This file is listed under `setupFiles` in vitest.config.ts for tests that
// need localStorage (e.g. recordSidebar.test.ts).  It replaces the stub with a
// fully-functional in-memory implementation so that `localStorage.clear()`,
// `vi.spyOn(Storage.prototype, ...)`, etc. all work correctly.

const _store: Record<string, string> = {};

class InMemoryStorage {
  get length() {
    return Object.keys(_store).length;
  }
  getItem(key: string): string | null {
    return Object.prototype.hasOwnProperty.call(_store, key) ? _store[key] : null;
  }
  setItem(key: string, value: string): void {
    _store[key] = String(value);
  }
  removeItem(key: string): void {
    delete _store[key];
  }
  clear(): void {
    const keys = Object.keys(_store);
    for (const k of keys) delete _store[k];
  }
  key(index: number): string | null {
    return Object.keys(_store)[index] ?? null;
  }
}

const impl = new InMemoryStorage();

Object.defineProperty(globalThis, 'localStorage', {
  value: impl,
  configurable: true,
  writable: true,
});

Object.defineProperty(globalThis, 'Storage', {
  value: InMemoryStorage,
  configurable: true,
  writable: true,
});
