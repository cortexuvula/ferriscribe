# Low-Priority Improvements — Implementation Plans

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Spec:** `docs/superpowers/specs/2026-07-08-low-priority-improvements-design.md`

---

# Item #9: Conditional Chip Poll

## Task 1: Gate the poll behind sync_condition_chips

**Files:**
- Modify: `src/lib/components/ConditionChips.svelte`

- [ ] **Step 1: Read the current poll setup**

Read `src/lib/components/ConditionChips.svelte`. Find the `onMount` block with `setInterval` and the `pollHandle` + `onDestroy` declarations.

- [ ] **Step 2: Add settings import**

Add to the script imports:
```typescript
  import { settings } from '../stores/settings.svelte';
```

- [ ] **Step 3: Replace unconditional poll with $effect**

Replace the `pollHandle = setInterval(refreshChips, 30_000)` line in `onMount` with an `$effect` that watches the sync setting:

```typescript
  // Only poll when sync is enabled — avoids pointless DB reads for users
  // who haven't opted into chip sync (the default).
  $effect(() => {
    if (settings.state.sync_condition_chips) {
      pollHandle = setInterval(refreshChips, 30_000);
      return () => { if (pollHandle) clearInterval(pollHandle); };
    }
  });
```

Remove the `setInterval` line from `onMount`. Keep the initial `refreshChips()` call in `onMount` (chips load regardless of sync).

The `$effect` automatically starts/stops the interval when `sync_condition_chips` changes. The cleanup function clears the interval when the setting is toggled off or the component is destroyed.

Remove the `onDestroy` pollHandle cleanup since the `$effect` handles it now. Keep `onDestroy` for `dirtyTimer` and `unlistenSSE` if they exist.

- [ ] **Step 4: Run type check + lint + tests**

Run: `npm run check`
Run: `npx eslint src/lib/components/ConditionChips.svelte`
Run: `npx vitest run src/lib/components/ConditionChips.test.ts`
Expected: 0 errors, all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib/components/ConditionChips.svelte
git commit -m "perf: only poll condition chips when sync is enabled

The 30s poll now only runs when sync_condition_chips is true. When sync
is off (the default), no interval runs — saves a pointless DB read every
30s for every user who hasn't opted into chip sync."
```

---

# Item #10: Keyboard Shortcuts

## Task 1: Add Space (record/stop) + Cmd+Enter (generate SOAP) shortcuts

**Files:**
- Modify: `src/lib/pages/RecordTab.svelte`

- [ ] **Step 1: Read the current RecordTab**

Read `src/lib/pages/RecordTab.svelte`. Find:
- The record/stop button and its click handler
- The generate SOAP button and its handler
- The `audio` store usage (audio.state.state for recording/idle/stopped)
- The onMount/onDestroy or existing lifecycle hooks

- [ ] **Step 2: Add keyboard handler**

In the script section, add a keydown handler:

```typescript
  function handleGlobalKeydown(e: KeyboardEvent) {
    // Don't fire when typing in an input, textarea, or contenteditable.
    const target = e.target as HTMLElement;
    if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable) {
      return;
    }

    // Space — toggle record/stop
    if (e.code === 'Space') {
      e.preventDefault();
      if (audio.state.state === 'recording') {
        audio.stop();
      } else if (audio.state.state === 'idle' || audio.state.state === 'stopped') {
        audio.start();
      }
      return;
    }

    // Cmd+Enter (Mac) or Ctrl+Enter — generate SOAP
    if ((e.metaKey || e.ctrlKey) && e.code === 'Enter') {
      e.preventDefault();
      // Only generate if there's a selected recording and we're not already generating
      if (audio.state.lastRecordingId && /* check not already generating */) {
        // Call the generate handler — read the actual function name from RecordTab
      }
    }
  }
```

**IMPORTANT:** Read the actual RecordTab to find the correct function names for starting/stopping recording and generating SOAP. The `audio.start()` and `audio.stop()` are on the audio store, but the generate function may be a local function like `handleGenerate` or `handleRegenerateSoap`. Also check if there's a `generating` state flag to prevent double-generate.

- [ ] **Step 3: Register the listener on mount**

```typescript
  onMount(() => {
    window.addEventListener('keydown', handleGlobalKeydown);
  });

  onDestroy(() => {
    window.removeEventListener('keydown', handleGlobalKeydown);
  });
```

If the component already has onMount/onDestroy, add the listener to the existing hooks.

- [ ] **Step 4: Add discoverability hints**

Find the record/stop button in the markup. Add a `title` attribute and a small hint:

For the record button, add to its title:
```svelte
title="Record (Space)"
```

For the generate button, add:
```svelte
title="Generate SOAP (Cmd+Enter)"
```

Optionally, add a tiny hint text below the record button:
```svelte
<span class="shortcut-hint">Space to record</span>
```

With CSS:
```css
.shortcut-hint {
  font-size: 9px;
  color: var(--text-muted, #666);
  opacity: 0.6;
}
```

- [ ] **Step 5: Run type check + lint**

Run: `npm run check`
Run: `npx eslint src/lib/pages/RecordTab.svelte`
Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/pages/RecordTab.svelte
git commit -m "feat(ui): keyboard shortcuts — Space for record/stop, Cmd+Enter for SOAP

Space toggles recording (push-to-talk pattern). Cmd+Enter generates SOAP
for the selected recording. Both are guarded against firing while typing
in inputs. Hints added to button titles."
```

---

## Self-Review

### Spec coverage
- ✅ #9 Conditional poll — gate behind sync_condition_chips via $effect
- ✅ #10 Shortcuts — Space (record/stop) + Cmd+Enter (generate SOAP)
- ✅ Input guard — check tagName before firing
- ✅ Discoverability — title attributes + hint text

### Known caveats
1. #10: The exact function names for generate SOAP must be read from RecordTab — the plan says to read the file first.
2. #10: Space key may conflict with scroll behavior — `e.preventDefault()` handles this.
3. #9: The `$effect` must be at the top level of the script, not inside onMount — Svelte 5 requirement.
