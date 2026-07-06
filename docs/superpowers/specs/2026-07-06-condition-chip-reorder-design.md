# Condition Chip Reorder + Order Sync — Design Spec

**Date:** 2026-07-06
**Status:** Approved (all design sections)
**Approach:** `sort_order` column + whole-list LWW propagation

## Goal

Let users drag-and-drop to reorganize the known condition chips, and have that ordering sync to the server and other remote clients via the existing condition chip sync infrastructure.

## Scope

**In scope:** Manual reordering of the chip presets via drag-and-drop, with the new order syncing through the existing two-way LWW merge.

**Out of scope:** Auto-sorting, grouping, or categorization. Chips remain a flat list.

## Requirements (from brainstorming)

1. **Reorder UI:** Drag-and-drop with the native HTML Drag and Drop API (no new dependency)
2. **Order sync model:** Whole-list order with single timestamp — ordering rides on the existing per-chip LWW merge (when a chip's `updated_at` wins, its `sort_order` comes with it)
3. **Concurrent reorders:** Last-writer-wins — whoever reordered most recently wins. Converges cleanly, no collisions.
4. **New chips:** Append to the end (`sort_order = max + 1`)

## Data Model

### Migration `m011_condition_chip_order`

```sql
ALTER TABLE condition_chips ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
```

Existing chips (seeded by m010) all get `sort_order = 0`. The `ORDER BY sort_order, LOWER(text)` query means same-order chips fall back to alphabetical — so the initial visual order is unchanged after migration. The first time a user reorders, chips get explicit sequential `sort_order` values.

### `ConditionChip` struct — add `sort_order: i32`

```rust
pub struct ConditionChip {
    pub id: String,
    pub text: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub sort_order: i32,  // NEW
}
```

The DTO (`ConditionChipDto`) and TypeScript interface gain the same field.

### `list_active` query change

```sql
-- Before: ORDER BY LOWER(text)
-- After:  ORDER BY sort_order, LOWER(text)
```

The `LOWER(text)` tiebreaker handles chips that haven't been explicitly ordered.

## Merge Algorithm for Ordering

Ordering piggybacks on the existing per-chip LWW merge — no separate merge path needed. `sort_order` is just another field on the chip. When a chip's `updated_at` wins (remote or local), ALL its fields come along, including `sort_order`.

### Walkthrough — reorder propagates

| Step | Machine A (reorders) | Machine B (hasn't synced) |
|------|---------------------|--------------------------|
| 1 | `reorder(["Diabetes","Asthma","Hypertension"])` → bumps `updated_at` on all 3, sets `sort_order` 0, 1, 2 | Unchanged |
| 2 | Pushes to server | — |
| 3 | — | Pulls from server → merge applies new `sort_order` (newer `updated_at` wins) |

### Concurrent reorders

A reorders at 10:00, B reorders at 10:05. On next sync, B's chips have newer `updated_at` → B's ordering wins. Converges to B's order. Predictable, no collision, no garbled interleaving.

### New chips

Added with `sort_order = max(existing) + 1` → appear at end. On sync, preserved (most recently updated on that chip).

## Repository Method

```rust
/// Reorder chips to match the given ordered list of IDs.
/// Sets sort_order = index for each, bumps updated_at on changed rows.
/// Returns the active list in the new order.
pub fn reorder(conn: &Connection, ordered_ids: &[String], now_iso: &str) -> DbResult<Vec<ConditionChip>>
```

```sql
UPDATE condition_chips SET sort_order = ?1, updated_at = ?2 WHERE id = ?3
```

The `updated_at` bump is what makes ordering propagate through the existing LWW merge.

Also update:
- `upsert` — include `sort_order` in the INSERT and ON CONFLICT DO UPDATE SET clauses
- `add` — set `sort_order = max(existing sort_order) + 1` for new chips (query `SELECT MAX(sort_order) FROM condition_chips WHERE deleted_at IS NULL`)
- `row_to_chip` — read the new column (5th column)

## Tauri Command

```rust
#[tauri::command]
pub async fn reorder_condition_chips(
    state: tauri::State<'_, AppState>,
    ordered_ids: Vec<String>,
) -> AppResult<Vec<ConditionChip>>
```

Follows the same dispatch pattern as add/remove: update local DB first (instant UI), then best-effort background sync push. The background push sends the full list including new `sort_order` values.

No new server endpoint needed — ordering rides on the existing `POST /v1/condition-chips/sync`.

## Frontend (Drag-and-Drop)

### `ConditionChips.svelte`

Each chip `<button>` gets `draggable="true"` with native HTML DnD event handlers:
- `ondragstart` — store the dragged chip's index
- `ondragover` — prevent default (enables drop), show visual indicator
- `ondrop` — reorder the local `chips` array, call `reorderConditionChips(orderedIds)`

State: `dragIndex` tracks the dragged chip. Visual feedback: dragged chip gets `opacity: 0.4`, drop target gets a left-border highlight.

The click-to-add behavior is preserved — HTML DnD natively distinguishes a drag (threshold movement) from a click.

New API helper: `reorderConditionChips(orderedIds: string[])` in `src/lib/api/conditions.ts`.

## Edge Cases

| Case | Behavior |
|------|----------|
| New chip added | `sort_order = max + 1` → end of list |
| Chip tombstoned | `sort_order` irrelevant — filtered out by `list_active` |
| Server reorders while client offline | On reconnect, merge applies server's order |
| Chip re-added after tombstone | Fresh row, `sort_order = max + 1` (end, not old position) |
| `reorder` with subset of IDs | Only listed chips reordered; others keep positions |
| `reorder` with unknown/stale ID | `UPDATE ... WHERE id = ?` affects 0 rows — ignored |

## Testing Strategy

### New repo tests (in `condition_chips.rs`)
- `reorder_updates_sort_order` — reorder 3 chips, verify `list_active` order
- `reorder_bumps_updated_at` — verify changed chips have newer timestamps
- `reorder_partial_list` — reorder some chips, unlisted keep positions
- `reorder_idempotent` — same list twice, same state
- `list_active_orders_by_sort_order` — `sort_order` takes precedence over alphabetical
- `merge_propagates_order` — local old order, remote new order (newer ts) → merge updates

### Existing tests — update for new field
- The 9 existing `condition_chips` tests need the `sort_order` column in the manual table creation and the `chip()` test helper constructor.

### Migration test
- Verify m011 adds the column without data loss.

### Frontend test
- Extend `ConditionChips.test.ts` with a drag-drop simulation (fire drag events, verify `reorderConditionChips` called with correct order).
