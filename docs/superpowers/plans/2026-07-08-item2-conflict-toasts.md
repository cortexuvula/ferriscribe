# Item #2: Conflict Feedback Toasts — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Show a toast when another machine changes condition chips while the user has local pending changes.

**Architecture:** Track a `dirtySince` timestamp in `ConditionChips.svelte`. Set it on local mutations (add/remove/reorder). When the poll detects a remote change AND `dirtySince` is within 5 seconds, show an info toast. Clear `dirtySince` after 5s.

**Tech Stack:** Svelte 5 runes, existing `toasts` store.

**Spec:** `docs/superpowers/specs/2026-07-08-high-priority-improvements-design.md` (Item #2)

---

## Task 1: Add dirtySince tracking + toast on remote change

**Files:**
- Modify: `src/lib/components/ConditionChips.svelte`

- [ ] **Step 1: Read the current component**

Read `src/lib/components/ConditionChips.svelte` fully. Find:
- The `refreshChips` function (the poll callback)
- The `addNewCondition`, `removeCondition`, and `handleDrop` functions (local mutations)
- The imports at the top

- [ ] **Step 2: Add imports + dirtySince state**

In the `<script>` section, add the toast import after the existing imports:

```typescript
  import { toasts } from '../stores/toasts.svelte';
```

Add the `dirtySince` state after the other `$state` declarations (near `loaded`, `adding`, etc.):

```typescript
  // Tracks when the user last made a local chip change. When the poll detects
  // a remote change within 5s of this, we show a toast (possible conflict).
  let dirtySince = $state<number | null>(null);
  let dirtyTimer: ReturnType<typeof setTimeout> | null = null;

  function markDirty() {
    dirtySince = Date.now();
    if (dirtyTimer) clearTimeout(dirtyTimer);
    dirtyTimer = setTimeout(() => { dirtySince = null; }, 5000);
  }
```

- [ ] **Step 3: Call markDirty() in local mutation handlers**

Add `markDirty()` at the start of the mutation in these functions:
- `addNewCondition` — after the dedup check, before `chips = await addConditionChip(trimmed)`
- `removeCondition` — before `chips = await removeConditionChip(conditionText)`
- In `handleDrop` — after the optimistic `chips = reordered`, before `await reorderConditionChips(orderedIds)`

For example, in `addNewCondition`:
```typescript
    try {
      markDirty();
      chips = await addConditionChip(trimmed);
    } catch (e) {
```

- [ ] **Step 4: Show toast when poll detects change while dirty**

In `refreshChips`, modify the change-detection block. Currently it checks if the list changed and updates silently. Add a toast when `dirtySince` is recent:

```typescript
  async function refreshChips() {
    try {
      const result = await listConditionChips();
      // Only update if the list actually changed (avoid unnecessary re-renders).
      if (
        result.length !== chips.length ||
        result.some((c, i) => c.id !== chips[i]?.id || c.sort_order !== chips[i]?.sort_order)
      ) {
        // If the user made a local change recently, this remote update may
        // have overwritten it. Show a toast so the user knows.
        if (dirtySince !== null) {
          toasts.add({
            message: 'Condition chips updated from another machine',
            type: 'info',
            autoDismiss: true,
          });
        }
        chips = result;
      }
    } catch (e) {
      console.error('Failed to load condition chips:', e);
    }
    loaded = true;
  }
```

Check the `toasts.add()` API — the `type` field may be named differently. Read `src/lib/stores/toasts.svelte.ts` to verify the exact interface (it may use `variant` or no type field at all). Match the existing API.

- [ ] **Step 5: Clean up dirtyTimer on destroy**

In the existing `onDestroy` callback (already present for the poll handle), add:

```typescript
  onDestroy(() => {
    if (pollHandle) clearInterval(pollHandle);
    if (dirtyTimer) clearTimeout(dirtyTimer);
  });
```

- [ ] **Step 6: Run type check**

Run: `npm run check`
Expected: 0 errors.

- [ ] **Step 7: Run lint**

Run: `npx eslint src/lib/components/ConditionChips.svelte`
Expected: 0 errors.

- [ ] **Step 8: Run tests**

Run: `npx vitest run src/lib/components/ConditionChips.test.ts`
Expected: all pass.

- [ ] **Step 9: Commit**

```bash
git add src/lib/components/ConditionChips.svelte
git commit -m "feat(ui): toast when condition chips change from another machine

Tracks local mutations via dirtySince timestamp. When the 30s poll
detects a remote change within 5s of a local mutation, shows an info
toast so the user knows their view was updated."
```

---

## Self-Review

### Spec coverage
- ✅ dirtySince timestamp set on add/remove/reorder — Step 3
- ✅ 5s window for dirty — Step 2 (setTimeout)
- ✅ Toast on remote change while dirty — Step 4
- ✅ No toast when idle (dirtySince is null) — Step 4 (if check)
- ✅ autoDismiss — Step 4

### Type consistency
- `dirtySince: number | null` (Date.now() returns number)
- `markDirty()` called consistently in all 3 mutation handlers
- `toasts.add()` API — MUST verify exact interface in Step 4
