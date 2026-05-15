# Svelte Stores → Runes Migration — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Migrate all 14 stores in `src/lib/stores/` from `writable()`-based modules to class-based `$state` runes in `.svelte.ts` modules, updating all 175+ consumer touchpoints + 3 test files. Spec: `docs/superpowers/specs/2026-05-15-stores-to-runes-design.md`.

**Worktree:** `.worktrees/stores-to-runes` on branch `stores-to-runes`.

**Baseline:** 233 vitest tests pass. svelte-check 0/0/0/477. cargo workspace lib all green.

---

## Universal recipe (applies in every task)

**Per store file:**
1. Rename `src/lib/stores/X.ts` → `src/lib/stores/X.svelte.ts` (use `git mv`).
2. Remove `import { writable } from 'svelte/store';`. Remove the `createXStore` factory.
3. Define a class with `$state` fields for the previously-writable value(s) and regular methods for the helpers.
4. Export `const x = new XStore();` at the end (same export name as before).
5. **State property naming (canonical):**
   - Primitive value: `current` (theme, settingsNav.activeTab, recordSidebar.<single thing>)
   - Object/record: `state` (pipeline, audio, settings, endpointOffline, recordings.selected, chat)
   - Array: `list` (toasts, recordings, contextTemplates)
   - Map/Record-keyed-by-id: `byId` or `entries` (per-store judgment)
6. Internal non-reactive state (Promises, counters, listener handles): keep as `private` fields.

**Per consumer (.svelte or .ts file):**
1. `$store.field` → `store.<property>.field` (e.g., `$settings.theme` → `settings.state.theme`).
2. `$store` standalone (e.g., `$toasts.length`) → `store.<property>.length`.
3. Import path stays extension-less: `from '../stores/foo'` works for `foo.svelte.ts`.

**Per test file:**
1. Remove `import { get } from 'svelte/store';` if present.
2. `get(store).field` → `store.<property>.field`.
3. `store.subscribe(...)` → direct property reads at assertion sites.

**Verification per task:**
- `npx vitest run` — should match baseline (233) except for the 3 test files we're updating (those may shift counts as expected).
- `npm run check` — 0 errors, 0 warnings.
- `git grep -rE '\$<storeName>\b' src/` for each migrated store — should be empty.
- `git grep "from 'svelte/store'" src/lib/stores/` — only un-migrated stores should match.

---

## Task 1: Validator — 5 small stores

**Stores:** `theme`, `settingsNav`, `recordSidebar`, `chat`, `endpointOffline`.

These are the lowest-risk because they have few consumers and either use the method API only (no `$store` auto-sub) or have isolated consumers. This task validates the migration pattern.

### Per-store guidance

#### `theme.ts` (3 consumers, 0 `$theme` auto-sub)

Property: `current: Theme`.

Class shape:
```ts
type Theme = 'light' | 'dark';

class ThemeStore {
  current = $state<Theme>('dark');

  set(value: Theme) {
    if (typeof document !== 'undefined') {
      document.documentElement.setAttribute('data-theme', value);
    }
    this.current = value;
  }

  toggle() {
    const next: Theme = this.current === 'dark' ? 'light' : 'dark';
    this.set(next);
  }
}

export const theme = new ThemeStore();
```

Consumers: `theme.set(x)` and `theme.toggle()` stay. If a consumer auto-subscribed via `$theme`, change to `theme.current`.

#### `settingsNav.ts` (2 consumers, 0 `$` auto-sub)

Read the file first to determine its public API. Expose the previously-writable as `current` (likely a tab name string).

#### `recordSidebar.ts` (1 consumer, 0 `$` auto-sub, **has tests**)

Read the file + `recordSidebar.test.ts`. Migrate the file, then update the test to use direct property access. Test count may stay the same or shift slightly.

#### `chat.ts` (1 consumer, 3 `$chat` auto-subs)

Read the file. Expose as `state` (it likely wraps an object).

#### `endpointOffline.ts` (4 consumers, 0 `$` auto-sub, **has tests**)

Read the file + `endpointOffline.test.ts`. Migrate + update test.

### Steps

- [ ] Read the 5 store files + 2 test files + identify all consumers via `git grep`.
- [ ] Per-store: apply the universal recipe.
- [ ] Per-consumer: update `$store` references (the chat one has 3 sites; others are method-only).
- [ ] `npx vitest run` — expect 233 passed (or close — confirm baseline if test counts shift).
- [ ] `npm run check` — 0 errors.
- [ ] Verify: `git grep -rE '\$(theme|settingsNav|recordSidebar|chat|endpointOffline)\b' src/` — empty result.
- [ ] Verify: `git grep "from 'svelte/store'" src/lib/stores/{theme,settingsNav,recordSidebar,chat,endpointOffline}.*` — empty.
- [ ] Commit:
  ```
  refactor(stores): migrate theme/settingsNav/recordSidebar/chat/endpointOffline to runes

  5 small stores converted from writable() factories to class-based $state
  runes in .svelte.ts modules. Same exported names, same methods, no
  behavior changes.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 2: `generation` + `contextTemplates` + `toasts`

**Total:** ~23 touchpoints. All three are simple in shape but each has 4-6 consumers.

### Per-store guidance

#### `generation.ts` (2 consumers, 10 `$generation` auto-subs)

Read. Property name: `state` (it likely wraps a generation status object).

#### `contextTemplates.ts` (4 consumers, 11 `$contextTemplates` auto-subs)

Read. Property name: `list` (it's an array of templates per the name + spec table).

#### `toasts.ts` (6 consumers, 2 `$toasts` auto-subs)

Property name: `list: Toast[]`.

Reference target class shape (the existing factory does counter + setTimeout):
```ts
class ToastStore {
  list = $state<Toast[]>([]);
  private counter = 0;

  add(toast: Omit<Toast, 'id'>) {
    const id = `toast-${++this.counter}`;
    this.list = [...this.list, { ...toast, id }];
    if (toast.autoDismiss) {
      setTimeout(() => this.dismiss(id), 8000);
    }
    return id;
  }

  dismiss(id: string) {
    this.list = this.list.filter((t) => t.id !== id);
  }

  error(message: string) { return this.add({ message, type: 'error', autoDismiss: false }); }
  success(message: string) { return this.add({ message, type: 'success', autoDismiss: true }); }
}
```

Note: assign `this.list = [...this.list, x]` instead of `.push()` because `$state` tracks reassignment but not mutation of arrays in older Svelte 5 versions. Verify by reading whether Svelte 5 in this project's `package.json` supports `.push()` (it should — Svelte 5 makes `$state` arrays proxied — but the assignment pattern is safer).

### Steps

- [ ] Apply universal recipe to all 3 stores.
- [ ] Update all 23 consumer sites.
- [ ] `npx vitest run`, `npm run check`.
- [ ] Verify auto-subs gone for these 3 stores.
- [ ] Commit:
  ```
  refactor(stores): migrate generation/contextTemplates/toasts to runes
  ```

---

## Task 3: `recordings` + `pipeline`

**Total:** ~31 touchpoints. `pipeline` imports `recordings` — migrate `recordings` first within the task.

### Per-store guidance

#### `recordings.ts` (5 consumers, 6 `$recordings` auto-subs)

Read carefully — likely exports both a list-shaped state AND helper methods (`selectRecording`, etc.). Spec calls for `list` as the property name. Verify the file's exports first — it likely exports `recordings` plus named functions like `selectRecording`. Preserve all named exports.

#### `pipeline.ts` (3 consumers, 25 `$pipeline.current` auto-subs)

Read in full (240 lines). The factory registers Tauri `listen(...)` event handlers. Migration must preserve:
1. Initial state (`{ current: null, active: {} }`)
2. Event listener registration (likely in the factory itself — moves to the class `constructor` OR to an `init()` method called from a component's `onMount`)
3. The `current` derived view (most recent pipeline) — currently a property of the wrapped object; becomes a `$state` field on the class.

Property name: `state` (object with `current: PipelineEntry | null` and `active: Record<...>`).

If event registration is moved to the constructor, ensure it doesn't fire before tauri is ready — verify by reading how the existing factory schedules listen registration. The class constructor runs at import time; if the factory previously deferred listen() to first call, preserve that lazy init pattern via an `ensureListening` private method called from the public methods.

### Steps

- [ ] Read `recordings.ts` + `pipeline.ts` thoroughly.
- [ ] Migrate `recordings` first.
- [ ] Migrate `pipeline` second (it imports `recordings`; that import path is unchanged but the API access changes).
- [ ] Update all 31 consumer sites.
- [ ] `npx vitest run`, `npm run check`.
- [ ] Verify auto-subs gone.
- [ ] Commit:
  ```
  refactor(stores): migrate recordings + pipeline to runes
  ```

---

## Task 4: `rsvp` + `audio` + `endpointHealth`

**Total:** ~33 touchpoints (rsvp 11, audio 22). `endpointHealth` has 0 auto-subs but has tests.

### Per-store guidance

#### `rsvp.ts` (6 consumers, 11 `$rsvp` auto-subs — split between `.picker` (6) and `.reader` (5))

Property name: `state` (wraps `{ picker, reader, ... }`).

#### `audio.ts` (6 consumers, 22 `$audio.state` auto-subs)

Already has `.state` in its existing API! Read the file to confirm. Property name: `state` (a record).

#### `endpointHealth.ts` (2 consumers, 0 auto-sub, **has tests**: `endpointHealth.test.ts` with 17 tests)

Read both files. The test currently uses `import { writable } from 'svelte/store'` per the earlier grep — verify and adapt. Property name: TBD by inspecting the file (likely an object exposing per-endpoint health, so `state`).

### Steps

- [ ] Read all 3 store files + endpointHealth test.
- [ ] Migrate each.
- [ ] Update 33 consumer sites.
- [ ] Update `endpointHealth.test.ts`.
- [ ] `npx vitest run` — expect 233 passed (test counts may stay constant since test logic is unchanged).
- [ ] `npm run check` — 0 errors.
- [ ] Commit:
  ```
  refactor(stores): migrate rsvp/audio/endpointHealth to runes
  ```

---

## Task 5: `settings` (the largest blast radius)

**Total:** 85 `$settings.field` references across 18 consumers. Save queue + load guard must be preserved exactly.

### Migration details

Read `settings.ts` (145 lines). It uses:
- `writable<AppConfig>(defaults)` as the inner state
- `loaded: boolean` flag (private)
- `saveQueue: Promise<void>` chain (private)
- Public methods: `subscribe`, `load`, `save`, `updateField`

Target class shape:
```ts
class SettingsStore {
  state = $state<AppConfig>({ ...defaults });
  private loaded = false;
  private saveQueue: Promise<void> = Promise.resolve();

  async load(): Promise<void> {
    try {
      const config = await getSettings();
      Object.assign(this.state, config);
      // Or: this.state = config; — preserve reference semantics by checking how callers read.
      this.loaded = true;
    } catch (err) {
      console.error('Failed to load settings:', err);
    }
  }

  async save(config: AppConfig): Promise<void> { /* same logic, this.state = config */ }

  async updateField<K extends keyof AppConfig>(key: K, value: AppConfig[K]): Promise<void> {
    if (!this.loaded) { /* warn + return */ }
    const next = { ...this.state, [key]: value };
    this.state = next;
    const prev = this.saveQueue;
    this.saveQueue = (async () => {
      await prev.catch(() => {});
      try {
        await saveSettings(next);
      } catch (err) {
        console.error('Save failed:', err);
        try {
          const latest = await getSettings();
          this.state = latest;
        } catch (_reloadErr) { /* ignore */ }
        throw err;
      }
    })();
    return this.saveQueue;
  }
}

export const settings = new SettingsStore();
```

Key invariants:
- `this.loaded` guard before save
- `saveQueue` chain — sequential saves, failed save reloads backend
- Optimistic local update before save fires

### Consumer updates

All 85 `$settings.field` sites → `settings.state.field`. The implementer should use `git grep -rn '\$settings\.' src/ --include="*.svelte" --include="*.ts"` to enumerate, then update each with Edit (or `sed -i` for safety).

### Steps

- [ ] Read `settings.ts` carefully.
- [ ] Migrate to `settings.svelte.ts` per the recipe above.
- [ ] Enumerate the 85 sites: `git grep -rn '\$settings\.' src/`.
- [ ] Update each `$settings.field` → `settings.state.field`. (Also handle bare `$settings` standalone usage.)
- [ ] `npx vitest run` → 233 passed.
- [ ] `npm run check` → 0 errors.
- [ ] Verify: `git grep -rE '\$settings\b' src/ --include="*.svelte" --include="*.ts"` → empty.
- [ ] Commit:
  ```
  refactor(stores): migrate settings to runes (the big one)

  85 $settings.* references → settings.state.* across 18 consumers.
  Save queue chain and loaded guard preserved exactly.
  ```

---

## Task 6: Final verification

- [ ] `npx vitest run` → 233 passed (or new totals if 3 store tests legitimately shifted).
- [ ] `npm run check` → 0 errors, 0 warnings, 477 files.
- [ ] `cargo test --workspace --lib` → 14 lib suites all `ok` (sanity).
- [ ] `git grep -rE '\$(settings|recordings|pipeline|toasts|chat|audio|endpointHealth|endpointOffline|rsvp|generation|theme|recordSidebar|settingsNav|contextTemplates)\b' src/ --include="*.svelte" --include="*.ts"` → empty.
- [ ] `git grep "from 'svelte/store'" src/lib/stores/` → empty.
- [ ] `ls src/lib/stores/*.ts | wc -l` → only test files left (3 test files; 14 stores renamed to .svelte.ts).
- [ ] `git log --oneline master..HEAD` shows ~8 commits.
- [ ] Dispatch final whole-branch code reviewer.
- [ ] Present merge options.

After all tasks: use superpowers:finishing-a-development-branch.
