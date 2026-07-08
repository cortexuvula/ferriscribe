# Medium-Priority Improvements — Implementation Plans

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 4 independent improvements: reduce boilerplate (#5), add sync round-trip test (#6), triage dependabot (#7), split Sharing.svelte (#8).

**Spec:** `docs/superpowers/specs/2026-07-08-medium-priority-improvements-design.md`

---

# Item #5: Reduce Command Boilerplate

**Goal:** Eliminate 36 repetitions of `.map_err(|e| AppError::Other(format!("Task join error: {e}")))` by adding `From<JoinError>`.

## Task 1: Add TaskJoin variant + From impl

**Files:**
- Modify: `crates/core/src/error.rs`

- [ ] **Step 1: Add the variant**

Read `crates/core/src/error.rs`. Find the `AppError` enum (around line 75). Add a new variant after `Other`:

```rust
    #[error("background task failed: {0}")]
    TaskJoin(String),
```

- [ ] **Step 2: Add the From impl**

After the enum definition (after the existing `From` impls for `Io` and `Serialization`), add:

```rust
impl From<tokio::task::JoinError> for AppError {
    fn from(e: tokio::task::JoinError) -> Self {
        AppError::TaskJoin(e.to_string())
    }
}
```

Check if `tokio` is already a dependency of `medical-core`. Run: `grep 'tokio' crates/core/Cargo.toml`. If not, add `tokio = { workspace = true }` to the dependencies (or just the `tokio` feature needed for `JoinError` — it's in the core `tokio` crate).

Actually, to avoid adding a tokio dependency to `medical-core` (which is a pure types crate), use a blanket string conversion instead. The `From<JoinError>` impl would require tokio in core. Better approach: make a helper function in `src-tauri` instead.

**REVISED approach (no medical-core change):** Add a helper function in `src-tauri/src/commands/mod.rs`:

```rust
/// Convert a tokio JoinError into an AppError. Used by all spawn_blocking
/// call sites to avoid repeating the format! boilerplate.
pub fn join_err(e: tokio::task::JoinError) -> AppError {
    AppError::Other(format!("Task join error: {e}"))
}
```

Then replace all `.map_err(|e| AppError::Other(format!("Task join error: {e}")))` with `.map_err(join_err)` across the command files. This is a pure mechanical refactor — no new dependencies, no medical-core change.

- [ ] **Step 3: Add the helper to mod.rs**

In `src-tauri/src/commands/mod.rs`, add near the existing helpers (`unwrap_app_error_message`, etc.):

```rust
use medical_core::error::AppError;

/// Convert a tokio JoinError into an AppError. Used by spawn_blocking call
/// sites to avoid repeating the format! boilerplate 36 times.
pub fn join_err(e: tokio::task::JoinError) -> AppError {
    AppError::Other(format!("Task join error: {e}"))
}
```

- [ ] **Step 4: Run sed to replace the pattern across all command files**

Run this to find all occurrences and verify count:
```bash
grep -rn 'map_err(|e| AppError::Other(format!("Task join error' src-tauri/src/commands/ | wc -l
```

Then replace each one. The pattern to find:
```
.map_err(|e| AppError::Other(format!("Task join error: {e}")))
```
Replace with:
```
.map_err(crate::commands::join_err)
```

Or if the file already imports from `crate::commands`, use the shorter form.

This is a mechanical change — use `sed` or manual edits file by file. After each file, verify it compiles.

- [ ] **Step 5: Build + test**

Run: `cargo build -p rust-medical-assistant 2>&1 | tail -10`
Run: `cargo test --workspace --lib 2>&1 | tail -10`
Expected: compiles, all tests pass. No behavior change.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: extract join_err helper to eliminate spawn_blocking boilerplate

Replaces 36 repetitions of .map_err(|e| AppError::Other(format!(\"Task join error: {e}\"))) 
with .map_err(crate::commands::join_err). Pure mechanical refactor, no behavior change."
```

---

# Item #6: Sync Round-Trip Integration Test

**Goal:** Test that two independent DBs converge after a sync round-trip.

## Task 1: Write the round-trip test

**Files:**
- Create: `crates/db/tests/condition_chips_sync.rs`

- [ ] **Step 1: Create the test file**

Create `crates/db/tests/condition_chips_sync.rs`:

```rust
//! Integration test: condition chip sync round-trip between two independent
//! databases (simulating two machines).

use medical_db::condition_chips::ConditionChipsRepo;
use medical_db::Database;
use medical_core::types::condition_chip::ConditionChip;

fn now(offset_secs: i64) -> String {
    let base = chrono::DateTime::parse_from_rfc3339("2026-07-08T10:00:00Z").unwrap();
    let t = base + chrono::Duration::seconds(offset_secs);
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Simulate a sync round-trip: A pushes its full list, server (B) merges,
/// returns active list, A merges the result back.
fn sync_roundtrip(
    db_a: &medical_db::PooledConnection,
    db_b: &medical_db::PooledConnection,
) -> (Vec<ConditionChip>, Vec<ConditionChip>) {
    // A pushes to B (server)
    let a_all = ConditionChipsRepo::list_all(db_a).unwrap();
    let b_result = ConditionChipsRepo::merge_incoming(db_b, &a_all).unwrap();

    // B pushes back to A
    let b_all = ConditionChipsRepo::list_all(db_b).unwrap();
    let a_result = ConditionChipsRepo::merge_incoming(db_a, &b_all).unwrap();

    (a_result, b_result)
}

#[test]
fn both_add_unique_chips_converge() {
    let db_a = Database::open_in_memory().unwrap();
    let db_b = Database::open_in_memory().unwrap();
    let conn_a = db_a.conn().unwrap();
    let conn_b = db_b.conn().unwrap();

    // A adds Hypertension, B adds Diabetes (offline)
    ConditionChipsRepo::add(&conn_a, "Hypertension", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn_b, "Diabetes", &now(0)).unwrap();

    let (a_result, b_result) = sync_roundtrip(&conn_a, &conn_b);

    // Both should have both chips
    assert_eq!(a_result.len(), 2, "A should have both chips");
    assert_eq!(b_result.len(), 2, "B should have both chips");
    let a_texts: Vec<&str> = a_result.iter().map(|c| c.text.as_str()).collect();
    let b_texts: Vec<&str> = b_result.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(a_texts, b_texts, "Both machines should converge to same set");
}

#[test]
fn both_add_same_chip_converge_to_one() {
    let db_a = Database::open_in_memory().unwrap();
    let db_b = Database::open_in_memory().unwrap();
    let conn_a = db_a.conn().unwrap();
    let conn_b = db_b.conn().unwrap();

    ConditionChipsRepo::add(&conn_a, "Asthma", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn_b, "Asthma", &now(1)).unwrap();

    let (a_result, b_result) = sync_roundtrip(&conn_a, &conn_b);

    assert_eq!(a_result.len(), 1, "Should converge to one Asthma chip");
    assert_eq!(b_result.len(), 1);
    assert_eq!(a_result[0].text, "Asthma");
}

#[test]
fn tombstone_propagates_across_roundtrip() {
    let db_a = Database::open_in_memory().unwrap();
    let db_b = Database::open_in_memory().unwrap();
    let conn_a = db_a.conn().unwrap();
    let conn_b = db_b.conn().unwrap();

    // Both start with COPD
    ConditionChipsRepo::add(&conn_a, "COPD", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn_b, "COPD", &now(0)).unwrap();

    // A removes it (tombstone at t=10)
    ConditionChipsRepo::remove_by_text(&conn_a, "COPD", &now(10)).unwrap();

    // Sync roundtrip
    let (a_result, b_result) = sync_roundtrip(&conn_a, &conn_b);

    assert!(a_result.is_empty(), "A should have no active chips");
    assert!(b_result.is_empty(), "B should have no active chips (tombstone propagated)");
}

#[test]
fn reorder_propagates_across_roundtrip() {
    let db_a = Database::open_in_memory().unwrap();
    let db_b = Database::open_in_memory().unwrap();
    let conn_a = db_a.conn().unwrap();
    let conn_b = db_b.conn().unwrap();

    // Both start with Alpha, Beta (in that order)
    ConditionChipsRepo::add(&conn_a, "Alpha", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn_a, "Beta", &now(0)).unwrap();

    // Sync initial state so B has both
    let a_all = ConditionChipsRepo::list_all(&conn_a).unwrap();
    ConditionChipsRepo::merge_incoming(&conn_b, &a_all).unwrap();

    // A reorders to Beta, Alpha
    let beta_id = medical_core::types::condition_chip::deterministic_id("Beta");
    let alpha_id = medical_core::types::condition_chip::deterministic_id("Alpha");
    ConditionChipsRepo::reorder(&conn_a, &[beta_id, alpha_id], &now(100)).unwrap();

    // Sync roundtrip
    let (a_result, b_result) = sync_roundtrip(&conn_a, &conn_b);

    let a_texts: Vec<&str> = a_result.iter().map(|c| c.text.as_str()).collect();
    let b_texts: Vec<&str> = b_result.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(a_texts, vec!["Beta", "Alpha"], "A should have reordered list");
    assert_eq!(b_texts, vec!["Beta", "Alpha"], "B should converge to A's order");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p medical-db --test condition_chips_sync`
Expected: all 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/db/tests/condition_chips_sync.rs
git commit -m "test(db): condition chip sync round-trip integration test

Tests convergence across two independent in-memory databases: unique
chip union, same-chip dedup, tombstone propagation, and reorder
propagation. Catches integration bugs the single-DB unit tests miss."
```

---

# Item #7: Dependabot Triage

## Task 1: Close high-risk PRs + add ignore rules

- [ ] **Step 1: Close rand PR**

```bash
gh pr close 37 --comment "Closing: rand 0.8→0.10 is a two-major bump affecting 4 crates including medical-security (key generation). The Rng trait API changed significantly. This needs a dedicated migration effort, not an auto-merge."
```

- [ ] **Step 2: Close zip PR**

```bash
gh pr close 38 --comment "Closing: zip 2→4 is two breaking majors. medical-sharing's packaging code needs API migration verification. Too risky for auto-merge."
```

- [ ] **Step 3: Close rubato PR**

```bash
gh pr close 36 --comment "Closing: rubato 0.16→3.0 is three major versions. The STT resampler wrapper needs API migration. Not worth the risk for a single-crate dependency."
```

- [ ] **Step 4: Add ignore rules to dependabot.yml**

In `.github/dependabot.yml`, add to the cargo section:

```yaml
    ignore:
      # Major bumps requiring dedicated migration effort
      - dependency-name: "rand"
        update-types: ["version-update:semver-major"]
      - dependency-name: "zip"
        update-types: ["version-update:semver-major"]
      - dependency-name: "rubato"
        update-types: ["version-update:semver-major"]
```

- [ ] **Step 5: Commit dependabot config**

```bash
git add .github/dependabot.yml
git commit -m "chore: ignore rand/zip/rubato major bumps in dependabot"
```

## Task 2: Try-merge sha2 and rodio

- [ ] **Step 1: Test sha2 bump locally**

Check out the sha2 branch, build, test:
```bash
gh pr checkout 39
cargo build --workspace 2>&1 | tail -10
cargo test --workspace --lib 2>&1 | tail -10
```
If green, merge. If it fails, close with a comment explaining the API breakage.

- [ ] **Step 2: Test rodio bump locally**

Same process for rodio (PR #35).

- [ ] **Step 3: Merge npm patch group (PR #52)**

```bash
gh pr checkout 52
npm ci && npm run check && npx vitest run 2>&1 | tail -10
```
If green, merge. Verify the tiptap-markdown bump specifically by checking if the editor renders correctly.

---

# Item #8: Split Sharing.svelte

## Task 1: Extract section components

**Files:**
- Create: `src/lib/components/settings/sharing/SharingModes.svelte`
- Create: `src/lib/components/settings/sharing/ConditionChipSync.svelte`
- Modify: `src/lib/components/settings/Sharing.svelte`

- [ ] **Step 1: Read the current Sharing.svelte**

Read `src/lib/components/settings/Sharing.svelte` fully.

- [ ] **Step 2: Create SharingModes.svelte**

Extract the mode selector + refresh logic. The component receives no props but emits state changes via callbacks:

```svelte
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';

  type Mode = 'off' | 'server' | 'client';

  let {
    onModeChange,
    onStatusChange,
  }: {
    onModeChange: (mode: Mode) => void;
    onStatusChange: (sharingOn: boolean, pairedTo: string | null) => void;
  } = $props();

  let mode = $state<Mode>('off');
  let sharingOn = $state(false);
  let pairedTo = $state<string | null>(null);

  async function refresh() {
    try {
      const status = await invoke<{ enabled: boolean }>('sharing_status');
      sharingOn = !!status.enabled;
    } catch { sharingOn = false; }
    try {
      const paired = await invoke<{ label: string } | null>('paired_endpoint');
      pairedTo = paired?.label ?? null;
    } catch { pairedTo = null; }
    if (sharingOn) mode = 'server';
    else if (pairedTo) mode = 'client';
    else mode = 'off';
    onModeChange(mode);
    onStatusChange(sharingOn, pairedTo);
  }

  onMount(refresh);

  function selectMode(m: Mode) {
    mode = m;
    onModeChange(m);
  }
</script>

<div class="modes">
  <label class:disabled={sharingOn}>
    <input type="radio" checked={mode === 'off'} disabled={sharingOn} onchange={() => selectMode('off')} />
    Off
  </label>
  <label class:disabled={sharingOn}>
    <input type="radio" checked={mode === 'server'} disabled={sharingOn} onchange={() => selectMode('server')} />
    Server
  </label>
  <label>
    <input type="radio" checked={mode === 'client'} disabled={sharingOn} onchange={() => selectMode('client')} />
    Client
  </label>
</div>

{#if sharingOn}
  <p class="hint">Stop sharing first (in the panel below) before switching modes.</p>
{/if}

<style>
  .modes { display: flex; gap: 1rem; margin: 0.5rem 0; }
  label { display: flex; align-items: center; gap: 4px; cursor: pointer; }
  label.disabled { opacity: 0.5; cursor: not-allowed; }
</style>
```

- [ ] **Step 3: Create ConditionChipSync.svelte**

Extract the sync toggle:

```svelte
<script lang="ts">
  import { settings } from '../../../stores/settings.svelte';
  import { syncConditionChips } from '../../../api/conditions';

  let { visible }: { visible: boolean } = $props();
</script>

{#if visible}
  <label class="form-row" style="margin-top: 1rem;">
    <input
      type="checkbox"
      checked={settings.state.sync_condition_chips ?? false}
      onchange={async (e) => {
        const checked = (e.target as HTMLInputElement).checked;
        settings.updateField('sync_condition_chips', checked);
        if (checked) {
          try { await syncConditionChips(); }
          catch (err) { console.error('Initial condition chip sync failed:', err); }
        }
      }}
    />
    <span>
      Sync known condition chips with the server
      <p class="hint">
        When enabled, your condition chip presets sync two-way between this
        machine and the server. Other clients' changes appear on reconnect.
        Off by default — each machine keeps its own list.
      </p>
    </span>
  </label>
{/if}
```

- [ ] **Step 4: Rewrite Sharing.svelte as thin orchestrator**

Replace Sharing.svelte with:

```svelte
<script lang="ts">
  import SharingModes from './sharing/SharingModes.svelte';
  import ConditionChipSync from './sharing/ConditionChipSync.svelte';
  import ServerWizard from './sharing/ServerWizard.svelte';
  import ServerStatus from './sharing/ServerStatus.svelte';
  import ClientPair from './sharing/ClientPair.svelte';

  type Mode = 'off' | 'server' | 'client';
  let mode = $state<Mode>('off');
  let sharingOn = $state(false);
  let pairedTo = $state<string | null>(null);
</script>

<div class="sharing">
  <h2>Sharing across machines</h2>
  <p class="hint">
    Run FerriScribe's heavy AI on one office computer and connect from your
    laptop or other clinicians' machines.
  </p>

  <SharingModes
    onModeChange={(m) => (mode = m)}
    onStatusChange={(on, paired) => { sharingOn = on; pairedTo = paired; }}
  />

  <ConditionChipSync visible={sharingOn || pairedTo !== null} />

  {#if mode === 'server' && !sharingOn}
    <ServerWizard />
  {:else if mode === 'server' && sharingOn}
    <ServerStatus />
  {:else if mode === 'client'}
    <ClientPair />
  {/if}
</div>

<style>
  .sharing { display: flex; flex-direction: column; gap: 0.5rem; }
  .hint { font-size: 0.85rem; color: var(--text-muted); margin-bottom: 0.5rem; }
</style>
```

**IMPORTANT:** The `onStatusChange` callback replaces the direct invoke calls that were in Sharing.svelte. The ServerWizard/ServerStatus components previously used an `ondone`/`onstopped` callback prop for refresh — check if those are still needed and wire them through the modes component. Read the original Sharing.svelte to verify the callback wiring for ServerWizard and ServerStatus.

- [ ] **Step 5: Run type check + lint + tests**

Run: `npm run check`
Run: `npx eslint src/lib/components/settings/Sharing.svelte src/lib/components/settings/sharing/SharingModes.svelte src/lib/components/settings/sharing/ConditionChipSync.svelte`
Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/components/settings/Sharing.svelte src/lib/components/settings/sharing/SharingModes.svelte src/lib/components/settings/sharing/ConditionChipSync.svelte
git commit -m "refactor: split Sharing.svelte into SharingModes + ConditionChipSync sections

Follows the General.svelte split pattern. Parent is now a thin orchestrator
(~25 lines) that routes sub-components based on mode."
```

---

## Self-Review

### Spec coverage
- ✅ #5 join_err helper — Task 1 (revised to avoid medical-core dep change)
- ✅ #6 Round-trip test — Task 1 (4 test scenarios)
- ✅ #7 Dependabot triage — Tasks 1-2 (close + ignore + try-merge)
- ✅ #8 Sharing split — Task 1 (2 new components + thin parent)

### Type consistency
- `join_err(e: tokio::task::JoinError) -> AppError` — consistent signature
- `SharingModes` callbacks: `onModeChange(mode)` + `onStatusChange(sharingOn, pairedTo)` — must match parent's usage
- `ConditionChipSync` prop: `visible: boolean` — matches parent's `sharingOn || pairedTo !== null`

### Known caveats
1. #5: The `join_err` helper avoids adding tokio to medical-core. Some command files may import `AppError` directly and need `use crate::commands::join_err;` added.
2. #8: ServerWizard/ServerStatus previously had `ondone`/`onstopped` callbacks for refresh — these need to be wired through SharingModes. Verify the original Sharing.svelte callback wiring.
3. #7: sha2 and rodio bumps may not compile — have a close-with-comment plan ready.
