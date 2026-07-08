# High-Priority Improvements — Design Spec

**Date:** 2026-07-08
**Status:** Approved (all design sections)
**Scope:** 4 independent improvements, implemented sequentially

## Overview

Four high-priority improvements identified during a review session. Each is an independent subsystem with its own design, implemented in risk order: #2 (toasts) → #3 (release fix) → #1 (SSE) → #4 (async encryption).

---

## Item #2: Conflict Feedback Toasts

### Problem
When the 30s poll detects that another machine reordered/changed chips, `refreshChips` silently overwrites local state. If the user just reordered locally, their change is clobbered with no indication.

### Approach
Track local mutations via a `dirtySince` timestamp. When the poll fires:
- **No local mutation in the last 5 seconds** → silent update (normal idle case)
- **Local mutation happened recently** → show a toast: "Condition chips updated from another machine" (auto-dismiss, info-level)

### Details
- `dirtySince` is a `$state<Date | null>` in `ConditionChips.svelte`, set on every local add/remove/reorder, cleared after 5s via `setTimeout`
- The toast uses the existing `toasts.add()` API with `autoDismiss: true`
- No action button needed — chips already updated via LWW, this is just awareness
- The toast is shown at most once per detected change (not on every poll tick)

### Files
- `src/lib/components/ConditionChips.svelte` — add `dirtySince` state, set it in add/remove/reorder handlers, check it in `refreshChips`
- `src/lib/stores/toasts.svelte.ts` — already has `add()` API, no changes needed

---

## Item #3: latest.json Release Order Fix

### Problem
The release workflow runs 3 parallel matrix jobs (Linux, Windows, macOS). Each uses `tauri-action` which auto-generates `latest.json` independently. The first job to finish publishes a partial manifest — clients see a new version but can't download their platform's binary yet.

### Approach
1. Set `updaterJson: false` on the `tauri-action` step to suppress per-job `latest.json` generation
2. Add a `manifest` job with `needs: release` (same pattern as the existing `prune` job) that runs after all platform builds complete
3. The manifest job generates a consolidated `latest.json` with all platform entries and uploads it to the release

### Manifest generation
A small inline Node.js script reads the release assets via `gh` CLI, finds the platform-specific assets + their `.sig` files, and constructs the JSON matching Tauri's expected format:

```json
{
  "version": "0.29.0",
  "notes": "...",
  "pub_date": "2026-07-08T...",
  "platforms": {
    "linux-x86_64": { "signature": "...", "url": "..." },
    "windows-x86_64": { "signature": "...", "url": "..." },
    "darwin-aarch64": { "signature": "...", "url": "..." }
  }
}
```

### Files
- `.github/workflows/release.yml` — add `updaterJson: false`, add `manifest` job

### Why this eliminates the race
`latest.json` only appears after every platform binary + signature is uploaded. Clients never see a partial manifest. The updater store's friendly error message (from v0.26.2) remains as a safety net.

---

## Item #1: SSE Realtime Chip Sync

### Problem
The 30s poll means up to 30 seconds of latency between machine A reordering and machine B seeing it.

### Approach
Add SSE (Server-Sent Events) push from the vocab API server. The client subscribes to a stream and re-pulls immediately when notified.

### Server side (`sharing_vocab_api.rs`)
- New endpoint: `GET /v1/condition-chips/events` — returns `text/event-stream`
- Add `tokio::sync::broadcast::Sender<()>` to `ApiState`
- When `condition_chips_sync_handler` completes a merge, call `sender.send(())` to notify subscribers
- The SSE handler receives from the broadcast channel and sends `data: changed\n\n`
- Client doesn't need chip data in the SSE event — just a "re-pull" signal

### Client side (`conditions_remote.rs` + background task)
- New `subscribe_events()` method on `ConditionsRemote` using `reqwest`'s `resp.bytes_stream()` for SSE line parsing
- A long-lived `tokio::spawn` task reads the stream and emits Tauri event `condition-chips-changed`
- Frontend listens via `listen('condition-chips-changed', ...)` and calls `refreshChips()` immediately

### Connection lifecycle
- The SSE subscription starts when the condition chip component mounts (same time as the initial pull + poll). It only connects if sync is enabled AND paired.
- On unpair/disconnect: the streaming response errors out naturally, the task logs and enters backoff
- On network drop: reqwest stream errors → task logs, waits 5s (exponential backoff up to 30s), reconnects
- On component destroy: the task is cancelled (drop the JoinHandle)
- **Keep the 30s poll as safety net** — if SSE works, the poll is a no-op (no changes detected). Belt and suspenders.

### Why SSE over WebSocket
SSE is simpler: unidirectional server→client, plain HTTP, no framing protocol. Chip sync only needs server→client push; the client already pushes via HTTP POST.

### Files
- `src-tauri/src/sharing_vocab_api.rs` — add broadcast channel to ApiState, SSE endpoint, trigger on sync
- `src-tauri/src/conditions_remote.rs` — add `subscribe_events()` streaming method
- `src-tauri/src/commands/conditions.rs` — add command to start/stop SSE subscription
- `src/lib/components/ConditionChips.svelte` — listen for `condition-chips-changed` event

---

## Item #4: Async Encryption (Non-Blocking Stop)

### Problem
`stop_recording` blocks for 1-2 seconds while encrypting the WAV (AES-256-GCM + fsync on a 30-50MB file). The user can't start a new recording until it returns.

### Critical constraint
The transcription pipeline reads the WAV immediately after `stop_recording` returns. If encryption is mid-rename, the reader sees a half-written file ("no RIFF tag found"). The current await exists to prevent this race.

### Approach — background encryption with reader-coordination flag
1. `stop_recording` saves the WAV, inserts the recording row with `encryption_pending = true`, spawns background encryption task, and returns immediately (~50ms)
2. Background task calls `encrypt_file_in_place`, then updates the row: `encryption_pending = false`
3. The reader (`open_recording_wav`) already handles both encrypted and plaintext files — it checks `FE1` magic and branches accordingly

### Why this is safe
Encryption is atomic at the filesystem level (rename is atomic on all platforms). The reader checks for `FE1` magic:
- Before rename: file is plaintext → reader gets `NotEncrypted` → reads plaintext ✓
- After rename: file is ciphertext → reader decrypts ✓
- No window where reader sees a corrupted file — rename either completed or it didn't

### New DB column (migration m012)
`encryption_pending BOOLEAN NOT NULL DEFAULT 0` on the `recordings` table. Tracks encryption state independently from the recording's processing `status`.

### Background task reliability
If the app crashes between insert and encryption, the recording stays plaintext with `encryption_pending = true`. On next app launch, a sweep checks for `encryption_pending = true` rows and encrypts them (same pattern as `fail_stuck_processing`).

### Files
- `crates/db/src/migrations/m012_encryption_pending.rs` — add column
- `crates/db/src/migrations/mod.rs` — register m012
- `crates/db/src/recordings.rs` — add `set_encryption_pending` method
- `src-tauri/src/commands/audio.rs` — change `stop_recording` to spawn background encryption instead of awaiting
- `src-tauri/src/lib.rs` or `src-tauri/src/state.rs` — add encryption sweep on startup

### Implementation order
This item is done LAST (after #2, #3, #1) because it touches the most sensitive code path (audio recording + PHI encryption).

---

## Testing Strategy

### Item #2 (toasts)
- Unit test: `dirtySince` is set on add/remove/reorder, cleared after 5s
- Frontend test: toast appears when poll detects change while dirty, no toast when idle

### Item #3 (release)
- Verify the manifest job produces valid JSON with all 3 platforms
- Manual: trigger a release tag, verify `latest.json` appears only after all assets

### Item #1 (SSE)
- Server unit test: broadcast sender triggers on sync handler
- Client integration test: SSE stream receives "changed" event after a sync
- Connection lifecycle: test reconnection after simulated network drop

### Item #4 (async encryption)
- DB migration test: m012 adds column without data loss
- Reader test: `open_recording_wav` correctly reads both `encryption_pending=true` (plaintext) and `encryption_pending=false` (ciphertext) files
- Integration test: `stop_recording` returns immediately, background task encrypts, reader can transcribe
- Sweep test: on startup, `encryption_pending=true` rows get encrypted
