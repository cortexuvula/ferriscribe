# Low-Priority Improvements — Design Spec

**Date:** 2026-07-08
**Status:** Approved
**Scope:** 2 independent improvements

---

## Item #9: Conditional Chip Poll

### Problem
The 30s poll calls `refreshChips()` unconditionally — even when sync is off and the machine isn't paired. Re-reads the local DB every 30s for no reason.

### Approach
Gate the `setInterval` behind the `sync_condition_chips` setting. Only poll when sync is enabled. Use an `$effect` that watches the setting and starts/stops the interval accordingly. When sync is off (the default), no poll runs at all.

### Details
- Replace the unconditional `setInterval` in `onMount` with an `$effect` that depends on `settings.state.sync_condition_chips`
- When the setting is true: start the 30s interval
- When the setting is false: clear the interval
- The initial `listConditionChips()` call in `onMount` stays unconditional (chips need to load regardless of sync)

---

## Item #10: Keyboard Shortcuts

### Problem
No keyboard shortcuts for common actions. Clinicians working fast would benefit from push-to-talk and quick-generate.

### Approach
Add a global keyboard handler on the Record tab:
- **Space** — toggle record/stop (push-to-talk pattern). Only fires when not typing in an input/textarea.
- **Cmd+Enter** — generate SOAP for the selected recording.

Guard against firing when the user is typing in an input, textarea, or contenteditable element (check `e.target.tagName`).

### Discoverability
- Add a small hint near the record button: "⌨ Space" in muted text
- Add `title` attributes to buttons mentioning the shortcut

### Files
- `src/lib/pages/RecordTab.svelte` — add keydown listener + hints
