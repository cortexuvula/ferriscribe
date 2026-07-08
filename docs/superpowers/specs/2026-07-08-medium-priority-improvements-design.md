# Medium-Priority Improvements — Design Spec

**Date:** 2026-07-08
**Status:** Approved (all design sections)
**Scope:** 4 independent improvements

---

## Item #5: Reduce Command Boilerplate

### Problem
36 occurrences of `.map_err(|e| AppError::Other(format!("Task join error: {e}")))` across command files. Each `spawn_blocking(...).await` requires manual JoinError→AppError wrapping.

### Approach
1. Add `TaskJoin(String)` variant to `AppError` in `crates/core/src/error.rs`
2. Implement `From<tokio::task::JoinError> for AppError`
3. Replace all `.map_err(|e| AppError::Other(format!("Task join error: {e}")))?` with just `?` — the `From` impl handles conversion automatically
4. The 2 transcription cases that extract the error string for `mark_recording_failed` keep manual handling (they need the string content, not just propagation)

### Scope
- `crates/core/src/error.rs` — add variant + From impl
- ~10 files in `src-tauri/src/commands/` — mechanical find-replace of the map_err pattern
- No behavior change — same error message text

---

## Item #6: Sync Round-Trip Integration Test

### Problem
All condition chip merge tests are single-DB unit tests. No test exercises the full sync round-trip across two independent databases.

### Approach
Create `crates/db/tests/condition_chips_sync.rs` that:
1. Creates two in-memory databases (`Database::open_in_memory()`) — "machine A" and "machine B"
2. Adds chips independently on both (simulating offline use)
3. Simulates the sync round-trip: A → server → B (merge_incoming on B), then B → server → A (merge_incoming on A)
4. Asserts both machines converge to identical active chip sets + ordering
5. Tests tombstone propagation across the round-trip

### Test scenarios
- Both add unique chips → converge to union
- Both add same chip → converge to one copy
- A removes (tombstone), B still has → converges to removed
- A reorders, B has old order → converges to A's order

---

## Item #7: Dependabot Triage

### Problem
6 open PRs, none ecosystem-blocked per AGENTS.md. Need individual triage.

### Triage plan

**Close + add ignore rules (high-risk, not worth migration now):**
- `rand 0.8→0.10` — 4 crates including security, major API change
- `zip 2→4` — two majors, sharing-critical packaging
- `rubato 0.16→3.0` — three majors, STT resampler

**Try-merge (single-crate, testable):**
- `sha2 0.10→0.11` — security crate, run build+tests, merge if green
- `rodio 0.19→0.22` — audio playback, run build, merge if green

**Merge after verification:**
- npm patch group (#52) — verify tiptap-markdown minor doesn't break editor, merge if green

### Dependabot config update
Add ignore rules for rand, zip, rubato major bumps in `.github/dependabot.yml`

---

## Item #8: Split Sharing.svelte

### Problem
107 lines mixing mode selector, sync toggle, and sub-component routing.

### Approach
Extract two new section components into `src/lib/components/settings/sharing/`:
- `SharingModes.svelte` — the off/server/client radio block + `refresh()` bootstrap + `sharingOn`/`pairedTo` state. Exposes `mode`, `sharingOn`, and `pairedTo` via callback props so the parent can route sub-components.
- `ConditionChipSync.svelte` — the `sync_condition_chips` toggle. Receives `visible` as a prop (true when sharingOn || pairedTo).

`Sharing.svelte` shrinks to ~25 lines: holds the `mode`/`sharingOn`/`pairedTo` state (lifted from the modes component via callbacks), imports both new sections + existing ServerWizard/ServerStatus/ClientPair, routes sub-components based on mode.

### Cross-cutting
Update AGENTS.md "Known deferred debt" to:
- Remove the stale General.svelte note (already split, 33 lines now)
- This is a no-op for Sharing (it's small enough to not warrant a debt note)

### Pattern
Follows the General.svelte split pattern exactly — parent is a thin orchestrator, sections are siblings in a subdirectory.
