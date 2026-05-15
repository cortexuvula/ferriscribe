# Svelte Stores → Runes Migration — Design

**Date:** 2026-05-15
**Branch:** `stores-to-runes`
**Predecessor:** AppError migration (`ca15dcd`)

## Goal

Migrate all 14 stores in `src/lib/stores/` from the legacy `svelte/store` `writable()`/`derived()` API to class-based `$state`/`$derived` runes in `.svelte.ts` modules. Behavior-preserving — same exported names, same methods, same reactive semantics, equivalent test surface.

## Scope

**14 store files** (all currently `.ts`, all import `writable` from `svelte/store`):

| Store | Lines | Consumers | $store refs |
|-------|-------|-----------|-------------|
| settings | 145 | 18 | 85 |
| pipeline | 240 | 3 | 25 |
| audio | 233 | 6 | 22 |
| rsvp | 76 | 6 | 11 |
| contextTemplates | 24 | 4 | 11 |
| generation | 41 | 2 | 10 |
| recordings | 103 | 5 | 6 |
| chat | 156 | 1 | 3 |
| toasts | 52 | 6 | 2 |
| theme | 28 | 3 | 0 (method-only) |
| settingsNav | 28 | 2 | 0 (method-only) |
| recordSidebar | 65 | 1 | 0 (method-only) |
| endpointHealth | 212 | 2 | 0 (method-only) |
| endpointOffline | 52 | 4 | 0 (method-only) |

**Total touchpoints:** 14 stores + 175 `$store.field` references + 3 test files (`endpointHealth.test.ts`, `endpointOffline.test.ts`, `recordSidebar.test.ts`) + 22 Svelte components touched.

## Migration recipe

### Per-store transform

**Before (theme.ts):**
```ts
import { writable } from 'svelte/store';
type Theme = 'light' | 'dark';

function createThemeStore() {
  const { subscribe, set, update } = writable<Theme>('dark');
  return {
    subscribe,
    set(theme: Theme) { /* side-effect */ set(theme); },
    toggle() { update((current) => /* ... */); },
  };
}
export const theme = createThemeStore();
```

**After (theme.svelte.ts):**
```ts
type Theme = 'light' | 'dark';

class ThemeStore {
  current = $state<Theme>('dark');

  set(value: Theme) {
    /* side-effect */
    this.current = value;
  }

  toggle() {
    const next: Theme = this.current === 'dark' ? 'light' : 'dark';
    /* side-effect */
    this.current = next;
  }
}

export const theme = new ThemeStore();
```

**Rules:**
1. **File rename:** every `src/lib/stores/X.ts` → `src/lib/stores/X.svelte.ts`.
2. **Drop `subscribe` from the public API.** Consumers will read state via property access (`theme.current`) instead of `$theme`.
3. **Promote each writable's value into a public `$state` property** on the class. Use the existing semantic name where possible. Conventions for the property name:
   - If the store wrapped a primitive (`Theme`, `string`): use a descriptive name like `current`, `value`, or the most natural one. Use `theme.current` / `toasts.list` / `recordSidebar.state` / `endpointOffline.state` etc.
   - If the store wrapped a record/object/state: expose as `state` (e.g., `pipeline.state`, `audio.state`).
   - If the store wrapped an array: expose as `list` or `items` (e.g., `toasts.list`, `recordings.list`).
   - **Document the chosen name per-store in the plan** so consumers know what to change.
4. **Helper methods become regular methods on the class.** Mutate `this.<field>` directly (or reassign nested state when needed).
5. **`derived()` chains** (if any) become `$derived` properties on the same class or a separate `$derived.by(() => …)` block.
6. **Reactive `$effect`-like update logic** that used `subscribe` callbacks can become explicit method calls (when invoked by a single trigger) or methods that are called from `onMount` in a component.

### Per-consumer transform

For each Svelte component / TypeScript file that uses a store:

1. **Import path unchanged.** `import { settings } from '../stores/settings'` still resolves — Vite finds `settings.svelte.ts` automatically. (If TypeScript complains about resolution, fix via `tsconfig.json` rather than touching imports.)
2. **`$store.field` → `store.<chosen-name>.field`**. e.g. `$settings.input_device` → `settings.state.input_device`.
3. **`$store` as a whole** (e.g., `$toasts.map(...)`) → `store.<chosen-name>.map(...)`. e.g., `$toasts` → `toasts.list`.
4. **`subscribe`/`set`/`update` callback usage** disappears. If a component had `$: { ... reactive ... using $store ... }`, replace with `$effect(() => { ... store.state.x ... })`.

### Per-test transform

Tests using `get(store)` from `svelte/store`:
- Replace `import { get } from 'svelte/store';` with direct property access.
- `get(store).field` → `store.<chosen-name>.field`.
- `store.subscribe(callback)` for testing → replace with explicit access at the assertion site (each `$state` read is reactive; mock-friendly).

If a test relies on `flushSync` or specific reactivity timing, ensure equivalent behavior with `tick()` from Svelte (if needed).

### File extension policy

- Renamed files use `.svelte.ts`.
- Imports stay extension-less (`from '../stores/foo'`).
- If svelte-check or vitest can't resolve a renamed file, add the `.svelte` extension to the import as a last resort.

## Out of scope

- The legacy `writable`-based exports in third-party libraries (e.g., `@tauri-apps/api`) — those stay.
- New tests for migrated stores. The 3 existing test files (endpointHealth, endpointOffline, recordSidebar) cover their stores; we update those, but don't add new ones.
- Cross-store dependency restructuring. If `pipeline` imports `recordings`, that import stays intact post-migration.
- Performance optimizations beyond what runes already buy (fine-grained reactivity).

## Acceptance criteria

- `npx vitest run` → all 233 tests pass (or new counts if the 3 store-test files yield different totals after rewrite — confirm with implementer).
- `npm run check` (svelte-check) → 0 errors, 0 warnings, identical file count to baseline (477).
- No `writable`/`derived`/`readable` imports from `svelte/store` remain in `src/lib/stores/` — verify with `grep -r "from 'svelte/store'" src/lib/stores/`.
- No `$<storeName>` auto-subscriptions remain in `src/lib/` — verify with `grep -rE '\$(settings|recordings|pipeline|toasts|chat|audio|endpointHealth|endpointOffline|rsvp|generation|theme|recordSidebar|settingsNav|contextTemplates)\b' src/ --include="*.svelte" --include="*.ts"` returning empty (excluding the new stores' own internal `$state` declarations).
- `cargo test --workspace --lib` → backend tests still pass (sanity).
- All 14 stores renamed to `.svelte.ts`.
- Behavior preservation: settings save chain still works, pipeline event handling still works, toasts auto-dismiss timer still works, etc.

## Risk register

- **Reactivity-timing differences:** `writable` propagates updates synchronously to subscribers; `$state` updates are batched in microtasks. Most code is robust to this, but: anywhere a test or component relies on observing an intermediate value during a multi-step update, the behavior may differ. Mitigation: read each store's `update()` carefully before transforming; if it mutates and reads back in one step, that may need a refactor.
- **`pipeline` imports `recordings`:** migrate `recordings` before or in the same task as `pipeline`.
- **`settings.save()` chain:** the existing implementation uses a `saveQueue: Promise<void>` to serialize. The class version must preserve this; declare `private saveQueue: Promise<void> = Promise.resolve();` as a regular field (not `$state` — it's an internal Promise, not a reactive value).
- **Tauri event listeners:** stores like `pipeline` register `listen(...)` event handlers in their factory. The class equivalent runs the listener registration in a method called from a component's `onMount`. Or — simpler — register in the constructor. Either is fine; pick one and apply consistently.
- **Auto-subscription `$store` in templates:** every `$xxx` use must change. The mechanical scan via `git grep` should catch them all, but consumers like `EditorTab.svelte` (uses many `$settings.xxx`) need full re-verification.

## Why this matters

- Consistency with the rest of the codebase post-runes-migration in Svelte 5.
- Better TypeScript inference (`settings.state.allow_public_endpoint: boolean` directly typed without `Readable<AppConfig>` indirection).
- Removes the cognitive split between "runes for components, stores for cross-component" — everything is now `$state`-based.
- Future test ergonomics: direct property access in vitest, no `get(store)` boilerplate.

## Task structure

Implementation broken into 5 mechanical batches + final verification (see plan). Order optimized to migrate dependencies before dependents and small validators before big-blast-radius migrations.
