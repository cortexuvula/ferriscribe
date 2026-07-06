# Condition Chip Reorder + Order Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users drag-and-drop to reorganize condition chips, with the new order syncing to server and clients via the existing LWW merge.

**Architecture:** Add a `sort_order: i32` column to `condition_chips`. The `reorder` repo method sets sequential order values and bumps `updated_at` (triggering LWW propagation). Drag-and-drop uses the native HTML DnD API. A new `reorder_condition_chips` Tauri command dispatches like add/remove (local-first, background sync push).

**Tech Stack:** Rust (rusqlite), Svelte 5 runes + native HTML Drag and Drop API, Tauri v2 commands.

**Spec:** `docs/superpowers/specs/2026-07-06-condition-chip-reorder-design.md`

---

## File Structure

### Modified files

| File | Change |
|------|--------|
| `crates/core/src/types/condition_chip.rs` | Add `sort_order: i32` to `ConditionChip` struct |
| `crates/db/src/condition_chips.rs` | Add `sort_order` to queries, add `reorder` method, update `add` for max+1, update `row_to_chip`, update tests |
| `crates/db/src/migrations/m011_condition_chip_order.rs` | New migration: `ALTER TABLE condition_chips ADD COLUMN sort_order` |
| `crates/db/src/migrations/mod.rs` | Register m011 |
| `src-tauri/src/commands/conditions.rs` | Add `reorder_condition_chips` command + register in `generate_handler!` |
| `src/lib/api/conditions.ts` | Add `reorderConditionChips` helper + `sort_order` to interface |
| `src/lib/components/ConditionChips.svelte` | Add drag-and-drop handlers + reorder call |
| `src/lib/components/ConditionChips.test.ts` | Update for sort_order + add drag-drop test |

---

## Task 1: Add `sort_order` to `ConditionChip` + migration m011

**Files:**
- Modify: `crates/core/src/types/condition_chip.rs`
- Create: `crates/db/src/migrations/m011_condition_chip_order.rs`
- Modify: `crates/db/src/migrations/mod.rs`

- [ ] **Step 1: Add `sort_order` to the struct**

In `crates/core/src/types/condition_chip.rs`, add `sort_order: i32` to the `ConditionChip` struct (after `deleted_at`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConditionChip {
    pub id: String,
    pub text: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub sort_order: i32,
}
```

Add `#[serde(default)]` on the `sort_order` field so old serialized chips (without the field) deserialize cleanly:

```rust
    #[serde(default)]
    pub sort_order: i32,
```

- [ ] **Step 2: Create migration m011**

Create `crates/db/src/migrations/m011_condition_chip_order.rs`:

```rust
use rusqlite::Connection;

use crate::DbResult;

/// Add `sort_order` column for user-defined chip ordering + drag-and-drop.
///
/// Existing chips get `sort_order = 0` (alphabetical tiebreaker in list_active
/// keeps the visual order stable). The first time a user reorders, chips get
/// explicit sequential sort_order values.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "ALTER TABLE condition_chips ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;",
    )?;
    Ok(())
}
```

- [ ] **Step 3: Register the migration**

In `crates/db/src/migrations/mod.rs`, add after `pub mod m010_condition_chips;`:

```rust
pub mod m011_condition_chip_order;
```

In the `all_migrations()` array, after the m010 entry, add:

```rust
        Migration { version: 11, name: "condition_chip_order", up: m011_condition_chip_order::up },
```

- [ ] **Step 4: Run migration tests**

Run: `cargo test -p medical-db --lib migrations`
Expected: all pass including m011.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/types/condition_chip.rs crates/db/src/migrations/m011_condition_chip_order.rs crates/db/src/migrations/mod.rs
git commit -m "feat(db): m011 migration — sort_order column on condition_chips"
```

---

## Task 2: Update `ConditionChipsRepo` for `sort_order` + add `reorder`

**Files:**
- Modify: `crates/db/src/condition_chips.rs`

This task updates all existing methods to handle the new column, adds the `reorder` method, and updates all tests.

- [ ] **Step 1: Update `row_to_chip` to read the 5th column**

In `crates/db/src/condition_chips.rs`, update `row_to_chip` (currently reads 4 columns: id, text, updated_at, deleted_at). Add `sort_order`:

```rust
fn row_to_chip(row: &Row) -> rusqlite::Result<ConditionChip> {
    Ok(ConditionChip {
        id: row.get(0)?,
        text: row.get(1)?,
        updated_at: row.get(2)?,
        deleted_at: row.get(3)?,
        sort_order: row.get(4)?,
    })
}
```

- [ ] **Step 2: Update `list_active` and `list_all` queries**

Change both SELECT queries to include `sort_order` and change `list_active` ordering:

For `list_active`:
```sql
SELECT id, text, updated_at, deleted_at, sort_order
FROM condition_chips
WHERE deleted_at IS NULL
ORDER BY sort_order, LOWER(text)
```

For `list_all`:
```sql
SELECT id, text, updated_at, deleted_at, sort_order
FROM condition_chips
```
(list_all keeps no specific ORDER BY — it returns rows in storage order for sync)

- [ ] **Step 3: Update `upsert` to include `sort_order`**

```rust
pub fn upsert(conn: &Connection, chip: &ConditionChip) -> DbResult<()> {
    conn.execute(
        "INSERT INTO condition_chips (id, text, updated_at, deleted_at, sort_order)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
            text = excluded.text,
            updated_at = excluded.updated_at,
            deleted_at = excluded.deleted_at,
            sort_order = excluded.sort_order",
        rusqlite::params![chip.id, chip.text, chip.updated_at, chip.deleted_at, chip.sort_order],
    )?;
    Ok(())
}
```

- [ ] **Step 4: Update `add` to set `sort_order = max + 1`**

In the `add` method, before creating the chip, query the max sort_order of active chips:

```rust
pub fn add(conn: &Connection, text: &str, now_iso: &str) -> DbResult<Vec<ConditionChip>> {
    let max_order: i32 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) FROM condition_chips WHERE deleted_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    let chip = ConditionChip {
        id: deterministic_id(text),
        text: text.trim().to_string(),
        updated_at: now_iso.to_string(),
        deleted_at: None,
        sort_order: max_order + 1,
    };
    Self::upsert(conn, &chip)?;
    Self::list_active(conn)
}
```

- [ ] **Step 5: Add the `reorder` method**

Add after `remove_by_text`:

```rust
/// Reorder chips to match the given ordered list of IDs.
/// Sets sort_order = index for each, bumps updated_at on all changed rows.
/// Chips not in the list keep their existing sort_order.
/// Returns the active list in the new order.
pub fn reorder(conn: &Connection, ordered_ids: &[String], now_iso: &str) -> DbResult<Vec<ConditionChip>> {
    for (index, id) in ordered_ids.iter().enumerate() {
        conn.execute(
            "UPDATE condition_chips
             SET sort_order = ?1, updated_at = ?2
             WHERE id = ?3",
            rusqlite::params![index as i32, now_iso, id],
        )?;
    }
    Self::list_active(conn)
}
```

- [ ] **Step 6: Update the test helper `chip()` to include `sort_order`**

In the `#[cfg(test)] mod tests` section, update the `chip()` helper:

```rust
fn chip(text: &str, updated_offset: i64, deleted: bool) -> ConditionChip {
    ConditionChip {
        id: deterministic_id(text),
        text: text.to_string(),
        updated_at: now(updated_offset),
        deleted_at: if deleted { Some(now(updated_offset)) } else { None },
        sort_order: 0,
    }
}
```

- [ ] **Step 7: Update all test table creation to include `sort_order`**

Every test creates the table manually. Update ALL occurrences of the CREATE TABLE in tests to:

```sql
CREATE TABLE condition_chips (
    id TEXT PRIMARY KEY, text TEXT NOT NULL,
    updated_at TEXT NOT NULL, deleted_at TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0
);
```

There are multiple tests that create this table — update ALL of them. Search for `CREATE TABLE condition_chips` in the test module and update each.

- [ ] **Step 8: Add new reorder + sort_order tests**

Add these tests to the `#[cfg(test)] mod tests` section:

```rust
#[test]
fn reorder_updates_sort_order() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn().unwrap();
    conn.execute_batch(
        "CREATE TABLE condition_chips (
            id TEXT PRIMARY KEY, text TEXT NOT NULL,
            updated_at TEXT NOT NULL, deleted_at TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0
        );"
    ).unwrap();

    // Add 3 chips — they get sort_order 0, 1, 2.
    let list = ConditionChipsRepo::add(&conn, "Alpha", &now(0)).unwrap();
    let list = ConditionChipsRepo::add(&conn, "Beta", &now(1)).unwrap();
    let list = ConditionChipsRepo::add(&conn, "Gamma", &now(2)).unwrap();
    assert_eq!(list.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), vec!["Alpha", "Beta", "Gamma"]);

    // Reorder: Gamma first, Alpha second, Beta third.
    let gamma_id = deterministic_id("Gamma");
    let alpha_id = deterministic_id("Alpha");
    let beta_id = deterministic_id("Beta");
    let reordered = ConditionChipsRepo::reorder(&conn, &[gamma_id, alpha_id, beta_id], &now(100)).unwrap();
    assert_eq!(
        reordered.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
        vec!["Gamma", "Alpha", "Beta"],
        "list_active should reflect new sort_order"
    );
}

#[test]
fn reorder_bumps_updated_at() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn().unwrap();
    conn.execute_batch(
        "CREATE TABLE condition_chips (
            id TEXT PRIMARY KEY, text TEXT NOT NULL,
            updated_at TEXT NOT NULL, deleted_at TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0
        );"
    ).unwrap();

    ConditionChipsRepo::add(&conn, "Alpha", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn, "Beta", &now(0)).unwrap();

    let alpha_id = deterministic_id("Alpha");
    let beta_id = deterministic_id("Beta");
    ConditionChipsRepo::reorder(&conn, &[beta_id, alpha_id], &now(100)).unwrap();

    // Verify updated_at was bumped on reordered chips.
    let all = ConditionChipsRepo::list_all(&conn).unwrap();
    for chip in &all {
        assert_eq!(chip.updated_at, now(100), "updated_at should be bumped by reorder");
    }
}

#[test]
fn reorder_partial_list_keeps_unlisted_positions() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn().unwrap();
    conn.execute_batch(
        "CREATE TABLE condition_chips (
            id TEXT PRIMARY KEY, text TEXT NOT NULL,
            updated_at TEXT NOT NULL, deleted_at TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0
        );"
    ).unwrap();

    ConditionChipsRepo::add(&conn, "Alpha", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn, "Beta", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn, "Gamma", &now(0)).unwrap();

    // Reorder only Alpha and Beta — Gamma keeps its sort_order.
    let alpha_id = deterministic_id("Alpha");
    let beta_id = deterministic_id("Beta");
    let reordered = ConditionChipsRepo::reorder(&conn, &[beta_id, alpha_id], &now(100)).unwrap();

    // Beta=0, Alpha=1, Gamma=2 (unchanged)
    assert_eq!(
        reordered.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
        vec!["Beta", "Alpha", "Gamma"]
    );
}

#[test]
fn merge_propagates_order() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn().unwrap();
    conn.execute_batch(
        "CREATE TABLE condition_chips (
            id TEXT PRIMARY KEY, text TEXT NOT NULL,
            updated_at TEXT NOT NULL, deleted_at TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0
        );"
    ).unwrap();

    // Local has Alpha(0), Beta(1) at t=0.
    ConditionChipsRepo::add(&conn, "Alpha", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn, "Beta", &now(0)).unwrap();

    // Remote has the same chips but reordered (Beta=0, Alpha=1) at t=100.
    let remote = vec![
        ConditionChip { id: deterministic_id("Beta"), text: "Beta".into(), updated_at: now(100), deleted_at: None, sort_order: 0 },
        ConditionChip { id: deterministic_id("Alpha"), text: "Alpha".into(), updated_at: now(100), deleted_at: None, sort_order: 1 },
    ];
    let merged = ConditionChipsRepo::merge_incoming(&conn, &remote).unwrap();

    // Merge should apply remote's sort_order (newer updated_at wins).
    assert_eq!(
        merged.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
        vec!["Beta", "Alpha"],
        "merge should propagate remote's ordering"
    );
}

#[test]
fn add_appends_to_end_of_sorted_list() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn().unwrap();
    conn.execute_batch(
        "CREATE TABLE condition_chips (
            id TEXT PRIMARY KEY, text TEXT NOT NULL,
            updated_at TEXT NOT NULL, deleted_at TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0
        );"
    ).unwrap();

    ConditionChipsRepo::add(&conn, "Alpha", &now(0)).unwrap();
    ConditionChipsRepo::add(&conn, "Beta", &now(0)).unwrap();
    let after_gamma = ConditionChipsRepo::add(&conn, "Gamma", &now(0)).unwrap();

    // Gamma should be at the end (sort_order = max + 1 = 2).
    assert_eq!(after_gamma.last().unwrap().text, "Gamma");
}
```

- [ ] **Step 9: Run all condition_chips tests**

Run: `cargo test -p medical-db --lib condition_chips`
Expected: all existing 9 tests + 5 new tests pass (14 total).

- [ ] **Step 10: Commit**

```bash
git add crates/db/src/condition_chips.rs
git commit -m "feat(db): sort_order on condition_chips + reorder method + tests"
```

---

## Task 3: Add `reorder_condition_chips` Tauri command

**Files:**
- Modify: `src-tauri/src/commands/conditions.rs`
- Modify: `src-tauri/src/lib.rs` (register in `generate_handler!`)

- [ ] **Step 1: Add the command to `conditions.rs`**

Add this command after `sync_condition_chips_cmd` (follows the same dispatch pattern as add/remove):

```rust
/// Reorder condition chips. Updates local DB immediately, then pushes to
/// server if sync is enabled. The ordered_ids define the new sequence.
#[tauri::command]
#[instrument(skip(state), name = "conditions::reorder")]
pub async fn reorder_condition_chips(
    state: tauri::State<'_, AppState>,
    ordered_ids: Vec<String>,
) -> AppResult<Vec<ConditionChip>> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    // 1. Update local DB immediately (instant UI).
    let db = Arc::clone(&state.db);
    let local_list = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::reorder(&conn, &ordered_ids, &now)
            .map_err(AppError::from)
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    // 2. Best-effort background sync — push full list with new sort_order values.
    if let Some((conn, bearer)) = paired_conditions_target(&state)
        && let Some(remote) = crate::conditions_remote::ConditionsRemote::from(
            &conn,
            Some(bearer),
            state.http_client.clone(),
        )
    {
        let db2 = Arc::clone(&state.db);
        tokio::spawn(async move {
            match tokio::task::spawn_blocking(move || {
                let conn = db2.conn()?;
                medical_db::condition_chips::ConditionChipsRepo::list_all(&conn)
                    .map_err(AppError::from)
            }).await {
                Ok(Ok(all_chips)) => {
                    match remote.sync(all_chips).await {
                        Ok(_) => tracing::debug!("condition chip sync push (reorder) succeeded"),
                        Err(e) => tracing::warn!(error = %e, "condition chip sync push (reorder) failed"),
                    }
                }
                _ => tracing::warn!("failed to load chips for sync push (reorder)"),
            }
        });
    }

    Ok(local_list)
}
```

- [ ] **Step 2: Register in `generate_handler!`**

In `src-tauri/src/lib.rs`, in the `tauri::generate_handler![...]` macro, after `commands::conditions::sync_condition_chips_cmd`, add:

```rust
        commands::conditions::reorder_condition_chips,
```

- [ ] **Step 3: Build to verify**

Run: `cargo build -p rust-medical-assistant 2>&1 | tail -10`
Expected: compiles without errors.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/conditions.rs src-tauri/src/lib.rs
git commit -m "feat(commands): reorder_condition_chips Tauri command"
```

---

## Task 4: Frontend — API helper + drag-and-drop in ConditionChips.svelte

**Files:**
- Modify: `src/lib/api/conditions.ts`
- Modify: `src/lib/components/ConditionChips.svelte`

- [ ] **Step 1: Update the TS interface and add the reorder helper**

In `src/lib/api/conditions.ts`, add `sort_order` to the `ConditionChip` interface:

```typescript
export interface ConditionChip {
  id: string;
  text: string;
  updated_at: string;
  deleted_at: string | null;
  sort_order: number;
}
```

Add the reorder helper function:

```typescript
export async function reorderConditionChips(orderedIds: string[]): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('reorder_condition_chips', { orderedIds });
}
```

- [ ] **Step 2: Read the current ConditionChips.svelte**

Read `src/lib/components/ConditionChips.svelte` fully to understand the current markup structure — specifically the chip `<button>` elements that will become draggable.

- [ ] **Step 3: Add drag-and-drop state and handlers to the script**

In `ConditionChips.svelte`, add to the script section:

```typescript
  import { reorderConditionChips } from '../api/conditions';
```

Add drag state:

```typescript
  let dragIndex = $state<number | null>(null);
  let dragOverIndex = $state<number | null>(null);
```

Add drag handlers:

```typescript
  function handleDragStart(e: DragEvent, index: number) {
    dragIndex = index;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = 'move';
    }
  }

  function handleDragOver(e: DragEvent, index: number) {
    e.preventDefault();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect = 'move';
    }
    dragOverIndex = index;
  }

  async function handleDrop(e: DragEvent, dropIndex: number) {
    e.preventDefault();
    if (dragIndex === null || dragIndex === dropIndex) {
      dragIndex = null;
      dragOverIndex = null;
      return;
    }
    // Reorder the chips array.
    const reordered = [...chips];
    const [moved] = reordered.splice(dragIndex, 1);
    reordered.splice(dropIndex, 0, moved);
    chips = reordered;

    // Get the ordered IDs and call the backend.
    const orderedIds = reordered;
    dragIndex = null;
    dragOverIndex = null;
    try {
      // The backend returns the full active list — update our state.
      // We need to map text to IDs, but our chips array is text strings.
      // The reorder command takes IDs, so we need to track IDs.
      // Actually — we should store chips as ConditionChip objects, not just text.
    } catch (e) {
      console.error('Failed to reorder condition chips:', e);
    }
  }

  function handleDragEnd() {
    dragIndex = null;
    dragOverIndex = null;
  }
```

**IMPORTANT:** The current component stores `chips` as `string[]` (just text). But `reorderConditionChips` needs IDs. You must change `chips` from `string[]` to `ConditionChip[]` (the full objects). Read the actual component before making changes.

Here's the restructured script section:

```typescript
  let chips = $state<ConditionChip[]>([]);
  let loaded = $state(false);

  // Build default chip objects (for display when not loaded or empty).
  const DEFAULT_CHIPS: ConditionChip[] = DEFAULT_CONDITIONS.map((text, i) => ({
    id: '',  // defaults have no real ID — they're display-only fallbacks
    text,
    updated_at: '',
    deleted_at: null,
    sort_order: i,
  }));

  let displayChips = $derived(loaded && chips.length > 0 ? chips : DEFAULT_CHIPS);

  onMount(async () => {
    try {
      chips = await listConditionChips();
    } catch (e) {
      console.error('Failed to load condition chips:', e);
    }
    loaded = true;
  });

  async function addNewCondition() {
    const trimmed = newCondition.trim();
    if (!trimmed) return;
    if (displayChips.some((c) => c.text.toLowerCase() === trimmed.toLowerCase())) {
      newCondition = '';
      adding = false;
      return;
    }
    try {
      chips = await addConditionChip(trimmed);
    } catch (e) {
      console.error('Failed to add condition chip:', e);
    }
    newCondition = '';
    adding = false;
  }

  async function removeCondition(conditionText: string) {
    try {
      chips = await removeConditionChip(conditionText);
    } catch (e) {
      console.error('Failed to remove condition chip:', e);
    }
  }

  // Drag-and-drop handlers
  let dragIndex = $state<number | null>(null);
  let dragOverIndex = $state<number | null>(null);

  function handleDragStart(_e: DragEvent, index: number) {
    dragIndex = index;
  }

  function handleDragOver(e: DragEvent, index: number) {
    e.preventDefault();
    dragOverIndex = index;
  }

  async function handleDrop(e: DragEvent, dropIndex: number) {
    e.preventDefault();
    if (dragIndex === null || dragIndex === dropIndex) {
      dragIndex = null;
      dragOverIndex = null;
      return;
    }
    // Only reorder if we have real chip IDs (loaded from backend).
    if (!loaded || chips.length === 0) {
      dragIndex = null;
      dragOverIndex = null;
      return;
    }
    const reordered = [...chips];
    const [moved] = reordered.splice(dragIndex, 1);
    reordered.splice(dropIndex, 0, moved);
    chips = reordered;  // optimistic UI update

    const orderedIds = reordered.map((c) => c.id);
    dragIndex = null;
    dragOverIndex = null;
    try {
      chips = await reorderConditionChips(orderedIds);
    } catch (e) {
      console.error('Failed to reorder condition chips:', e);
    }
  }

  function handleDragEnd() {
    dragIndex = null;
    dragOverIndex = null;
  }
```

- [ ] **Step 4: Update the markup for drag-and-drop**

The chip button elements need `draggable`, `ondragstart`, `ondragover`, `ondrop`, `ondragend` attributes. The `{#each}` loop now iterates `displayChips` (objects, not strings). Read the current markup and adapt.

The chip wrapper/button gets:
```svelte
{#each displayChips as chip, i (chip.text)}
  <div
    class="condition-chip-wrapper"
    draggable={loaded}
    ondragstart={(e) => handleDragStart(e, i)}
    ondragover={(e) => handleDragOver(e, i)}
    ondrop={(e) => handleDrop(e, i)}
    ondragend={handleDragEnd}
    style:opacity={dragIndex === i ? '0.4' : '1'}
    style:border-left={dragOverIndex === i && dragIndex !== null ? '2px solid var(--accent)' : ''}
  >
    <!-- existing chip button + remove button -->
    <button class="condition-chip" onclick={() => onAdd(chip.text)}>
      {chip.text}
    </button>
    {#if loaded}
      <button class="chip-remove" onclick={() => removeCondition(chip.text)}>×</button>
    {/if}
  </div>
{/each}
```

**IMPORTANT:** Read the actual markup and adapt — don't blindly replace. The existing markup has specific classes and structure. Add the DnD attributes to the existing elements, change `condition` to `chip.text`, and add the conditional rendering for loaded vs default.

- [ ] **Step 5: Add DnD cursor styling**

In the `<style>` section, add:

```css
.condition-chip-wrapper[draggable='true'] {
  cursor: grab;
}
.condition-chip-wrapper[draggable='true']:active {
  cursor: grabbing;
}
```

- [ ] **Step 6: Run type check**

Run: `npm run check`
Expected: 0 errors.

- [ ] **Step 7: Commit**

```bash
git add src/lib/api/conditions.ts src/lib/components/ConditionChips.svelte
git commit -m "feat(frontend): drag-and-drop reorder for condition chips"
```

---

## Task 5: Update frontend test for sort_order + drag-drop

**Files:**
- Modify: `src/lib/components/ConditionChips.test.ts`

- [ ] **Step 1: Update existing tests to include sort_order in mock data**

In `src/lib/components/ConditionChips.test.ts`, update all mock `ConditionChip` objects to include `sort_order: 0` (or appropriate index). For example:

```typescript
mockListConditionChips.mockResolvedValue([
  { id: '1', text: 'Custom Condition', updated_at: '', deleted_at: null, sort_order: 0 },
]);
```

Also add `reorderConditionChips` to the mock:

```typescript
const mockReorderConditionChips = vi.fn();
// In the vi.mock factory:
reorderConditionChips: mockReorderConditionChips,
```

- [ ] **Step 2: Add a drag-drop reorder test**

```typescript
it('calls reorderConditionChips when chips are dragged to a new position', async () => {
  mockListConditionChips.mockResolvedValue([
    { id: 'id-1', text: 'Alpha', updated_at: '', deleted_at: null, sort_order: 0 },
    { id: 'id-2', text: 'Beta', updated_at: '', deleted_at: null, sort_order: 1 },
  ]);
  mockReorderConditionChips.mockResolvedValue([
    { id: 'id-2', text: 'Beta', updated_at: '', deleted_at: null, sort_order: 0 },
    { id: 'id-1', text: 'Alpha', updated_at: '', deleted_at: null, sort_order: 1 },
  ]);
  render(ConditionChips, { props: { onAdd: () => {} } });
  await waitFor(() => expect(screen.getByText('Beta')).toBeTruthy());

  // Simulate dragging Alpha onto Beta's position.
  const chipWrappers = document.querySelectorAll('[draggable]');
  expect(chipWrappers.length).toBeGreaterThanOrEqual(2);
  fireEvent.dragStart(chipWrappers[0]);
  fireEvent.dragOver(chipWrappers[1]);
  fireEvent.drop(chipWrappers[1]);

  await waitFor(() => {
    expect(mockReorderConditionChips).toHaveBeenCalledWith(['id-2', 'id-1']);
  });
});
```

- [ ] **Step 3: Run the tests**

Run: `npx vitest run src/lib/components/ConditionChips.test.ts`
Expected: all tests pass (existing + new).

- [ ] **Step 4: Commit**

```bash
git add src/lib/components/ConditionChips.test.ts
git commit -m "test(frontend): condition chip reorder drag-and-drop test"
```

---

## Task 6: Full integration verification

- [ ] **Step 1: Run Rust tests**

Run: `cargo test --workspace --lib 2>&1 | tail -20`
Expected: all pass (the pre-existing flaky file_crypto test may fail under parallel load — that's unrelated, run it in isolation to confirm).

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace 2>&1 | grep -E '^(error|warning)' | head -10`
Expected: no output (clean).

- [ ] **Step 3: Run frontend tests**

Run: `npx vitest run 2>&1 | tail -10`
Expected: all pass.

- [ ] **Step 4: Run type check**

Run: `npm run check`
Expected: 0 errors.

- [ ] **Step 5: Commit any remaining fixes**

```bash
git add -A
git commit -m "test: full integration verification for chip reorder"
```

---

## Self-Review

### Spec coverage
- ✅ Data model (sort_order column + struct field) — Task 1
- ✅ Migration m011 — Task 1
- ✅ Merge algorithm (rides on existing LWW via updated_at) — Task 2 (merge_propagates_order test)
- ✅ `reorder` repo method — Task 2
- ✅ `list_active` ordering change — Task 2
- ✅ `add` appends to end (max+1) — Task 2
- ✅ Tauri command `reorder_condition_chips` — Task 3
- ✅ Frontend drag-and-drop — Task 4
- ✅ Frontend API helper — Task 4
- ✅ Testing (6 new repo tests, drag-drop frontend test) — Tasks 2, 5
- ✅ Integration verification — Task 6

### Type consistency
- `sort_order: i32` used consistently across Rust struct, SQL column, TS interface
- `reorderConditionChips(orderedIds: string[])` — consistent between API helper and component
- `reorder_condition_chips` Tauri command takes `ordered_ids: Vec<String>` — matches the TS helper's `orderedIds`

### Known caveats
1. The existing 9 condition_chips tests need their table creation SQL and `chip()` helper updated for the new column — Task 2 Step 7 handles this explicitly.
2. The `ConditionChips.svelte` component changes from storing text strings to `ConditionChip[]` objects — this is a structural change that must be handled carefully in Task 4.
3. The drag-drop test uses `querySelectorAll('[draggable]')` which depends on the actual markup — may need adjustment.
