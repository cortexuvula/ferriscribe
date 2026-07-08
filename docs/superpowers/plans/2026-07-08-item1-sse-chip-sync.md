# Item #1: SSE Realtime Chip Sync — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Replace the 30s poll with push-based SSE so chip changes appear within seconds across machines.

**Architecture:** Add a `tokio::sync::broadcast` channel to the server's `ApiState`. When the sync handler completes a merge, broadcast a notification. Add an SSE endpoint that streams these notifications. The client subscribes via a long-lived streaming reqwest connection and emits a Tauri event the frontend listens for. Keep the 30s poll as a safety net.

**Tech Stack:** Axum 0.8 SSE, reqwest streaming, tokio broadcast, Tauri events.

**Spec:** `docs/superpowers/specs/2026-07-08-high-priority-improvements-design.md` (Item #1)

---

## File Structure

### Modified files

| File | Change |
|------|--------|
| `src-tauri/src/sharing_vocab_api.rs` | Add broadcast channel to ApiState, SSE endpoint, trigger on sync |
| `src-tauri/src/conditions_remote.rs` | Add `subscribe_events()` streaming method |
| `src-tauri/src/commands/conditions.rs` | Add `subscribe_condition_chips` command |
| `src-tauri/src/lib.rs` | Register new command |
| `src/lib/components/ConditionChips.svelte` | Listen for `condition-chips-changed` event |

---

## Task 1: Add broadcast channel to server ApiState + SSE endpoint

**Files:**
- Modify: `src-tauri/src/sharing_vocab_api.rs`

- [ ] **Step 1: Read the current file**

Read `src-tauri/src/sharing_vocab_api.rs` fully. Find:
- The `ApiState` struct definition
- The `spawn` function that builds the Router
- The `condition_chips_sync_handler` function
- The imports section

- [ ] **Step 2: Add broadcast channel to ApiState**

Add a broadcast sender to `ApiState`. The channel carries `()` — just a "something changed" signal, no data:

```rust
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct ApiState {
    db: Arc<Database>,
    tokens: Arc<TokenStore>,
    chips_changed_tx: broadcast::Sender<()>,
}
```

In the `spawn` function, create the channel before building `ApiState`:

```rust
let (chips_changed_tx, _) = broadcast::channel::<()>(16);
let state = ApiState {
    db: Arc::clone(&db),
    tokens: Arc::clone(&tokens),
    chips_changed_tx: chips_changed_tx.clone(),
};
```

- [ ] **Step 3: Trigger broadcast in sync handler**

In `condition_chips_sync_handler`, after the merge succeeds and before returning, trigger the broadcast:

```rust
    // Notify SSE subscribers that chips changed.
    let _ = state.chips_changed_tx.send(());
```

This is best-effort — `send` returns Err if there are no receivers, which is fine (no subscribers = no notification needed).

- [ ] **Step 4: Add the SSE endpoint**

Add a new route to the Router (alongside the other condition-chips routes):

```rust
        .route("/v1/condition-chips/events", get(condition_chips_events_handler))
```

Add the handler:

```rust
use axum::response::sse::{Event, Sse};
use futures_util::Stream;

/// GET /v1/condition-chips/events — SSE stream of chip-change notifications.
///
/// Sends a "changed" event whenever the sync handler completes a merge.
/// The client uses this as a "re-pull" signal — it calls list_condition_chips
/// on each event.
async fn condition_chips_events_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let mut rx = state.chips_changed_tx.subscribe();

    let stream = async_stream::stream! {
        // Send an initial event so the client knows the connection is live.
        yield Ok(Event::data("connected"));
        loop {
            match rx.recv().await {
                Ok(()) => yield Ok(Event::data("changed")),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream))
}
```

**IMPORTANT:** This uses `async_stream` and `futures_util`. Check if they're already dependencies. If not, add them to `src-tauri/Cargo.toml`:
- `async-stream` or `async-stream` — check workspace deps
- `futures-util` — likely already available

Read `src-tauri/Cargo.toml` to check. The `axum` SSE support is built-in (no extra feature flag needed for 0.8).

- [ ] **Step 5: Build to verify**

Run: `cargo build -p rust-medical-assistant 2>&1 | tail -20`
Expected: compiles. Fix any missing imports or dependencies.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/sharing_vocab_api.rs src-tauri/Cargo.toml
git commit -m "feat(sharing): SSE endpoint for condition chip change notifications"
```

---

## Task 2: Add SSE subscription client + Tauri command

**Files:**
- Modify: `src-tauri/src/conditions_remote.rs`
- Modify: `src-tauri/src/commands/conditions.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add subscribe_events to ConditionsRemote**

Read `src-tauri/src/conditions_remote.rs` fully. Add a new method that opens a streaming GET and yields "changed" signals:

```rust
use futures_util::StreamExt;

/// Subscribe to SSE change notifications. Returns a stream of () items,
/// each representing a "chips changed" signal from the server.
///
/// The caller should call `list_condition_chips` (pull + merge) on each
/// item to stay in sync.
pub async fn subscribe_events(
    &self,
) -> AppResult<impl futures_util::Stream<Item = ()>> {
    let url = format!(
        "{}/v1/condition-chips/events",
        self.base_url().ok_or_else(|| {
            AppError::Other("no vocab base URL for conditions remote".into())
        })?
    );

    let resp = self
        .client
        .get(&url)
        .timeout(Duration::from_secs(300)) // long timeout for SSE
        .bearer_auth(&self.bearer)
        .send()
        .await
        .map_err(|e| AppError::Other(format!("SSE connect: {e}")))?;

    if !resp.status().is_success() {
        return Err(AppError::Other(format!(
            "SSE connect failed: {}",
            resp.status()
        )));
    }

    // Parse SSE lines from the response body stream.
    let byte_stream = resp.bytes_stream();
    let event_stream = byte_stream
        .filter_map(|chunk| async move {
            match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    // SSE events are "data: <content>\n\n"
                    for line in text.lines() {
                        if line.starts_with("data: changed") {
                            return Some(());
                        }
                    }
                    None
                }
                Err(_) => None,
            }
        });

    Ok(event_stream)
}
```

**Note:** The SSE parsing is intentionally simple — we only look for `data: changed` lines. The `data: connected` initial event is ignored (it's just a connection acknowledgment).

- [ ] **Step 2: Add the subscribe command**

In `src-tauri/src/commands/conditions.rs`, add a command that starts the SSE subscription and emits a Tauri event on each notification:

```rust
/// Start listening for condition chip change notifications from the server.
/// Emits a `condition-chips-changed` Tauri event whenever the server notifies
/// of a change. The frontend calls this when sync is enabled + paired.
#[tauri::command]
#[instrument(skip(app, state), name = "conditions::subscribe")]
pub async fn subscribe_condition_chips(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> AppResult<()> {
    // Only subscribe if sync is enabled + paired.
    let Some((conn, bearer)) = paired_conditions_target(&state) else {
        return Ok(());
    };
    let Some(remote) = crate::conditions_remote::ConditionsRemote::from(
        &conn,
        Some(bearer),
        state.http_client.clone(),
    ) else {
        return Ok(());
    };

    // Clone the connection data for the spawned task (avoids borrow issues).
    // The remote borrows conn — we need to move conn into the task.
    // Actually, ConditionsRemote borrows &PairedConnection which can't be moved
    // into a 'static task. We need to reconstruct inside the task.
    let http_client = state.http_client.clone();
    let bearer = bearer;
    let conn = conn;

    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(5);
        loop {
            let remote = match crate::conditions_remote::ConditionsRemote::from(
                &conn,
                Some(bearer.clone()),
                http_client.clone(),
            ) {
                Some(r) => r,
                None => {
                    tracing::warn!("SSE subscription: remote unavailable, retrying");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    continue;
                }
            };

            match remote.subscribe_events().await {
                Ok(mut stream) => {
                    tracing::info!("SSE subscription connected");
                    backoff = Duration::from_secs(5); // reset backoff on success
                    while let Some(()) = stream.next().await {
                        // Emit Tauri event — frontend listens for this.
                        let _ = app.emit("condition-chips-changed", ());
                    }
                    tracing::info!("SSE stream ended, reconnecting");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "SSE subscription failed, reconnecting");
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });

    Ok(())
}
```

**IMPORTANT:** This command spawns an infinite reconnection loop. The task runs forever (until the app exits). The `app.emit("condition-chips-changed", ())` is what the frontend listens for.

Add necessary imports at the top of `conditions.rs`:
```rust
use std::time::Duration;
use futures_util::StreamExt;
use tauri::Emitter;
```

- [ ] **Step 3: Register the command**

In `src-tauri/src/lib.rs`, add to `generate_handler!`:
```rust
        commands::conditions::subscribe_condition_chips,
```

- [ ] **Step 4: Build to verify**

Run: `cargo build -p rust-medical-assistant 2>&1 | tail -20`
Expected: compiles. Fix any issues.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/conditions_remote.rs src-tauri/src/commands/conditions.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat(sync): SSE client + subscribe command for realtime chip sync"
```

---

## Task 3: Frontend — listen for SSE events + call subscribe on mount

**Files:**
- Modify: `src/lib/components/ConditionChips.svelte`

- [ ] **Step 1: Add Tauri event listener**

In `ConditionChips.svelte`, import the Tauri event listener:

```typescript
  import { listen } from '@tauri-apps/api/event';
```

In `onMount`, after starting the poll, also start the SSE subscription and listen for the event:

```typescript
  onMount(async () => {
    await refreshChips();
    pollHandle = setInterval(refreshChips, 30_000);

    // Listen for SSE push notifications (realtime sync).
    unlistenSSE = await listen('condition-chips-changed', () => {
      refreshChips();
    });

    // Start the SSE subscription on the backend (no-op if not paired).
    try {
      await invoke('subscribe_condition_chips');
    } catch (e) {
      console.error('Failed to start chip sync subscription:', e);
    }
  });
```

Add the state + cleanup:

```typescript
  let unlistenSSE: (() => void) | null = null;
```

In `onDestroy`:
```typescript
  onDestroy(() => {
    if (pollHandle) clearInterval(pollHandle);
    if (dirtyTimer) clearTimeout(dirtyTimer);
    if (unlistenSSE) unlistenSSE();
  });
```

Add the `invoke` import if not already present:
```typescript
  import { invoke } from '@tauri-apps/api/core';
```

- [ ] **Step 2: Run type check**

Run: `npm run check`
Expected: 0 errors.

- [ ] **Step 3: Run lint + tests**

Run: `npx eslint src/lib/components/ConditionChips.svelte`
Run: `npx vitest run src/lib/components/ConditionChips.test.ts`
Expected: 0 errors, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/lib/components/ConditionChips.svelte
git commit -m "feat(frontend): listen for SSE chip change events for realtime sync"
```

---

## Self-Review

### Spec coverage
- ✅ Broadcast channel in ApiState — Task 1
- ✅ SSE endpoint — Task 1
- ✅ Trigger on sync handler — Task 1
- ✅ Client subscribe_events() — Task 2
- ✅ Tauri command + reconnection loop — Task 2
- ✅ Frontend listen + subscribe on mount — Task 3
- ✅ Keep 30s poll as safety net — Task 3 (pollHandle unchanged)

### Known caveats
1. `async_stream` and `futures_util` may need to be added to Cargo.toml — check in Task 1 Step 4.
2. The `PairedConnection` borrow issue in the spawned task — the command reconstructs `ConditionsRemote` inside the loop to avoid lifetime issues.
3. The SSE stream parsing is simple line-matching — doesn't handle multi-line SSE data fields, but our events are single-line (`data: changed`).
4. The `subscribe_condition_chips` command is fire-and-forget — it spawns a forever-loop task. There's no explicit stop mechanism beyond app exit. This is acceptable since the task is cheap and the poll is the safety net.
