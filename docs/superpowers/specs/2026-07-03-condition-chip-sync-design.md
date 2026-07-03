# Condition Chip Sync — Design Spec

**Date:** 2026-07-03
**Status:** Approved (all design sections)
**Approach:** A — Dedicated `condition_chips` table

## Goal

Add an opt-in setting that synchronizes the "known condition" chip presets between the office server and remote clients. Two-way merge with per-item last-write-wins (LWW) and tombstones. Default off — each machine keeps its own list unless the user opts in.

## Scope

**In scope:** `AppConfig.custom_conditions` — the practice-wide quick-add chip presets (Hypertension, Diabetes, etc.). These are clinician convenience shortcuts, not patient-specific PHI.

**Out of scope:** Per-recording `PatientContext.conditions` (the actual conditions attached to each SOAP encounter). Those stay local per recording.

## Requirements (from brainstorming)

1. **Sync model:** Two-way merge (both server and clients can add/remove)
2. **Conflict rule:** Per-item last-write-wins with tombstones (soft-deletes)
3. **Sync trigger:** Real-time — push on edit, pull on connect (app launch + reconnect)
4. **Setting:** Opt-in, defaults to `false` (`sync_condition_chips` in `AppConfig`)
5. **Scope:** Chip presets only, not per-recording conditions

## Data Model

### New table (migration `m010_condition_chips`)

```sql
CREATE TABLE IF NOT EXISTS condition_chips (
    id          TEXT PRIMARY KEY,      -- deterministic UUID v5 from normalized text
    text        TEXT NOT NULL,         -- display text, e.g. "Hypertension"
    updated_at  TEXT NOT NULL,         -- ISO 8601 UTC — the LWW clock
    deleted_at  TEXT                   -- tombstone (NULL = active)
);
CREATE INDEX idx_condition_chips_active ON condition_chips(text) WHERE deleted_at IS NULL;
```

### Key decisions

- **Deterministic ID (UUID v5):** Normalization is `text.trim().to_lowercase()`, then UUID v5 with a fixed namespace UUID. "Hypertension", "hypertension ", and "HYPERTENSION" all produce the same `id`. This is what makes two-way merge work — same condition on two machines maps to the same row.

- **Soft-delete tombstones (`deleted_at`):** Removing a chip sets `deleted_at = now()` rather than deleting the row. On sync, a tombstone with a newer `updated_at` wins over an older active row, preventing "ghost chip" reappearance.

- **Migration of existing `custom_conditions`:** On first run after upgrade, the migration reads `AppConfig.custom_conditions` and inserts each as an active row with `updated_at = now()`. The old field stays in `AppConfig` (inert, for backward compat) but is no longer read. The migration does NOT clear `custom_conditions` — leaving it populated provides a rollback path if the feature is reverted.

- **Tombstone pruning:** Tombstones older than 30 days are pruned by a best-effort sweep on each sync. If a pruned tombstone's condition is re-added later, it creates a fresh active row.

### `ConditionChip` struct (`crates/core/src/types/condition_chip.rs`)

New file. The struct is defined in `medical-core` so both the DB crate (repo) and the Tauri layer (commands/API DTOs) can reference it without circular dependencies.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConditionChip {
    pub id: String,
    pub text: String,
    pub updated_at: String,       // ISO 8601 UTC
    pub deleted_at: Option<String>,
}
```

### `ConditionChipsRepo` (`crates/db/src/condition_chips.rs`)

- `list_active(conn) -> Vec<ConditionChip>` — active rows, ordered by text
- `upsert(conn, chip)` — insert or update by id
- `soft_delete(conn, id)` — set `deleted_at = now()`, bump `updated_at`
- `merge_incoming(conn, remote_chips) -> Vec<ConditionChip>` — core LWW merge, returns resolved active list
- `prune_tombstones(conn, older_than: Duration)` — remove stale tombstones
- `deterministic_id(text: &str) -> String` — UUID v5 from normalized text

## Merge Algorithm

Runs identically on server and client (same function, different DB instances). The merge is idempotent and commutative — both sides converge to identical state after one round-trip.

```
fn merge_incoming(remote_chips):
    for R in remote_chips:
        local = find local chip with same id as R
        if local does not exist:
            INSERT R as-is
        else if R.updated_at > local.updated_at:
            REPLACE local with R
        else if R.updated_at < local.updated_at:
            local wins — do nothing
        else (timestamps equal):
            keep whichever has deleted_at set (deleted wins on exact tie)
    return list_active()
```

### Scenario table

| Scenario | Machine A | Machine B | Result |
|----------|-----------|-----------|--------|
| Both add "Diabetes" independently | `{Diabetes, t=10:00}` | `{Diabetes, t=10:05}` | One row, B's timestamp wins |
| A adds "Asthma", B hasn't synced | `{Asthma, t=10:00}` | (nothing) | B gets Asthma on next pull |
| A removes "COPD" (was active) | tombstone t=11:00 | active t=09:00 | Removed everywhere (11:00 > 09:00) |
| Both remove "COPD" | tombstone t=11:00 | tombstone t=11:05 | Both deleted, B's timestamp wins |
| A removes "COPD", B re-adds after | tombstone t=11:00 | active t=12:00 | Re-added everywhere (12:00 > 11:00) |

### Clock source

`chrono::Utc::now()` formatted as ISO 8601 with milliseconds. String comparison of ISO 8601 timestamps is chronological, so LWW is a simple string `cmp`. Wall clock is sufficient for a single-practice deployment where clock skew is seconds.

## API Layer

### New HTTP endpoints (server, `sharing_vocab_api.rs`, port 11437, bearer-authed)

| Method | Route | Body | Returns | Purpose |
|--------|-------|------|---------|---------|
| `GET` | `/v1/condition-chips` | — | `Vec<ConditionChipDto>` | Pull all active chips |
| `POST` | `/v1/condition-chips/sync` | `Vec<ConditionChipDto>` | `Vec<ConditionChipDto>` | Push local list, get merged list back |

`ConditionChipDto` = `{ id: String, text: String, updated_at: String, deleted_at: Option<String> }`

**Why `POST /sync` (full-list round-trip) instead of per-operation endpoints:** Simpler than per-operation endpoints (which need ordering/retry), naturally handles offline edits — a client that added 3 and removed 2 while disconnected sends its current list on reconnect, merge reconciles in one round-trip.

### Client-side dispatch (`commands/conditions.rs`)

Every chip operation checks: is sync enabled AND are we paired?

```rust
fn list_chips(state) -> Vec<ConditionChip>:
    if sync_enabled(state) && paired(state):
        ConditionsRemote::from(paired_target())?.list().await
    else:
        ConditionChipsRepo::list_active(&local_db)

fn sync_chips(state, local_chips) -> Vec<ConditionChip>:
    if sync_enabled(state) && paired(state):
        ConditionsRemote::from(paired_target())?.sync(local_chips).await
    else:
        local_chips
```

### `ConditionsRemote` (`conditions_remote.rs`)

Mirrors `templates_remote.rs`: same `from()` constructor gating on `conn.ports.vocab`, same bearer auth, same 404-fallback for old servers (returns `None` → local-only).

### Sync flow (real-time)

1. **App launch / reconnect:** Frontend calls `list_chips` → if sync on + paired, pulls server list → `merge_incoming(server_list)` locally → returns merged active chips → UI renders.

2. **User adds/removes a chip:** Frontend calls `add_condition_chip(text)` or `remove_condition_chip(id)` Tauri command → updates local DB immediately (instant UI) → if sync on + paired, fires `sync_chips(local_chips)` in background → server merges → client merges response → silent UI refresh if changed.

3. **Another client's change:** Surfaces on next pull (app launch, reconnect, manual refresh). No websocket/SSE push — consistent with how vocabulary/templates work.

## Opt-in Setting

New `AppConfig` field:
```rust
/// When true, condition chip presets sync two-way between this machine and
/// the paired server. Defaults to false — each machine keeps its own list.
#[serde(default)]
pub sync_condition_chips: bool,
```

Added to the `AppConfig` struct in `crates/core/src/types/settings.rs`. The TS `AppConfig` interface (`src/lib/types/index.ts`) and frontend defaults (`settings.svelte.ts`) are updated to match.

### Settings UI toggle (`Sharing.svelte`)

Rendered below the existing mode selector, greyed out when sharing is off. Hint text explains two-way merge. When toggled on while sharing is active, triggers an immediate pull.

## Frontend Integration

### `ConditionChips.svelte` changes

- **Read:** `$derived` from local state populated by calling `list_chips` on mount + after each operation. No longer reads `settings.state.custom_conditions`.
- **Add:** calls `invoke('add_condition_chip', { text })` — returns updated list, refreshes chip row.
- **Remove:** calls `invoke('remove_condition_chip', { id })` — same return-and-refresh pattern.
- **Clicking a chip** (to add to textarea): unchanged — still calls `onAdd(condition)`.

### `AppConfig.custom_conditions` retirement

The old field stays in the struct (serde keeps deserializing) but is no longer read by any code path after migration seeds the new table. Not removed from the struct to avoid breaking old config snapshots — it becomes inert.

## Error Handling

| Failure | Behavior | User sees |
|---------|----------|-----------|
| Server unreachable (push on edit) | Local DB already updated. Background sync fails silently, logs warning. Next sync reconciles. | Nothing — works instantly |
| Server unreachable (pull on launch) | Falls back to local list. Silent. | Local chips load normally |
| 404 (old server, no route) | `ConditionsRemote::from()` returns `None` → local-only | Chips work locally; no sync |
| 401 (token revoked) | Treated as "not paired" → local-only | Same as unpaired state |
| Malformed JSON from server | Error caught → local-only | Local chips; sync silently off |

**Core principle:** sync failures never block the UI. A chip add/remove always succeeds locally first; sync is best-effort, always.

## PHI / HIPAA Compliance

- Condition chip presets are practice-wide convenience shortcuts, not patient-specific PHI. A clinician could theoretically type patient-identifying text, so:
- **Transport:** rides on existing `vocab_port` HTTP with bearer auth — same envelope as vocabulary/templates. No new ports or auth surface.
- **Logging:** log counts and lengths only, never chip text. Sync log lines: `chips_synced=14 tombstones=2`.
- **No telemetry:** stays within LAN/Tailscale mesh — no external endpoints.

## Testing Strategy

### `ConditionChipsRepo` unit tests (`crates/db/src/condition_chips.rs`)
- `merge_incoming` — all 5 scenarios from the merge table
- Deterministic ID generation (case/whitespace-insensitive)
- Tombstone wins over older active
- Re-add after tombstone (newer timestamp resurrects)
- Idempotency: merging same list twice = identical state
- `prune_tombstones` removes old entries, keeps recent
- Migration seeding: existing `custom_conditions` → active rows

### API handler tests
- Auth required (401 without bearer, 200 with)
- `POST /sync` returns merged list

### `ConditionsRemote` tests
- `from()` returns None when `conn.ports.vocab` is None (old server)
- Graceful error handling on connection failure

### Dispatch tests (`commands/conditions.rs`)
- Sync off → local only (no HTTP attempt)
- Sync on + unpaired → local only
- Sync on + paired → routes to remote

### Settings backward-compat test (`settings.rs`)
- `sync_condition_chips` defaults to `false` when absent from old config JSON

### Frontend test (`ConditionChips.test.ts`)
- Renders chips from `list_chips` invoke result
- Add/remove calls correct invoke commands
- Falls back gracefully when invoke fails
