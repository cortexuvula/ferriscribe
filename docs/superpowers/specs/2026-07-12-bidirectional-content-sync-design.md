# Bidirectional Content Sync

**Date:** 2026-07-12
**Status:** Design — awaiting implementation plan
**Depends on:** existing sharing infrastructure (mDNS discovery, bearer-token pairing, vocab API on port 11437, condition chip sync pattern)

## Problem

Today, condition chips are the only patient-adjacent data that syncs between the office server and remote clients. Transcripts, SOAP notes, referral letters, peer discussions, and audio recordings created on a remote laptop exist only on that laptop. If the laptop is lost or damaged, the data is gone. The user needs full bidirectional content sync: recordings created or edited on the client appear on the server, and vice versa; deletions propagate both ways; offline clients re-sync on startup.

## Decisions (locked)

1. **Transport security:** Content sync requires Tailscale on both server and client. Opt-in feature — clinics without Tailscale are unaffected. Condition chip sync continues to work over LAN as before.
2. **Audio strategy:** Text content syncs bidirectionally. Audio files upload client → server (server is canonical archive). Clients fetch audio on-demand only when the user needs playback/export for a recording that originated on another machine.
3. **Merge model:** Per-field last-write-wins (LWW). Each text field carries its own timestamp. Conflict toasts inform the user when a field they were viewing/editing was updated on another machine.
4. **Deletion model:** Immediate propagation, local-only undo (matching the existing condition chip pattern). Tombstones purged after 30 days by a sweeper task.
5. **Sync protocol:** Timestamp-gated delta sync. The client tracks a cursor (last-seen server `updated_at`). Each cycle, the server returns only recordings modified since that cursor. Initial sync is a one-time full dump; subsequent syncs are lightweight deltas.

## Architecture

```
┌──────────────────┐                          ┌──────────────────┐
│  Remote Client    │    Tailscale (WireGuard)  │   Server         │
│  (laptop)         │◄─────────────────────────►│  (office PC)     │
│                   │   vocab port 11437         │                  │
│  RecordingsDB     │  Pull: GET /sync?since=X   │  RecordingsDB    │
│  (SQLCipher)      │◄───────────────────────────│  (SQLCipher)     │
│                   │  Push: POST /sync (deltas) │                  │
│                   │───────────────────────────►│                  │
│                   │  SSE: GET /events          │                  │
│                   │◄───────────────────────────│                  │
│                   │                            │                  │
│  audio/ (*.enc)   │  Audio upload (client→srv) │  audio/ (*.enc)  │
│                   │───────────────────────────►│                  │
│                   │  Audio fetch (on-demand)   │                  │
│                   │◄───────────────────────────│                  │
└──────────────────┘                            └──────────────────┘
```

The server is the canonical store. Both server and clients can create and edit recordings. New route groups are added to the existing vocab API server (`sharing_vocab_api.rs`), reusing the same bearer-token auth and broadcast channel pattern as condition chips.

## Opt-in gating

Content sync activates only when all three conditions hold:

1. **Setting enabled** — `AppConfig.sync_content: bool` (default `false`). Toggled in Settings → Sharing → Content Sync.
2. **Paired** — Machine is paired (server config exists OR paired connection exists).
3. **Tailscale active** — Content sync routes exclusively through the Tailscale endpoint (`paired_connection.tailscale`). If Tailscale is not configured or unreachable, content sync silently falls back to local-only. The existing condition chip sync continues to work over LAN.

A new helper `content_sync_target()` (mirroring `paired_conditions_target()`) checks all three gates. Every content sync command routes through it.

**Server-side defense-in-depth:** Content sync routes reject requests where the remote address is not a Tailscale IP (100.64.0.0/10 CGNAT range), in addition to the bearer-token check.

## What syncs

| Content | Direction | When |
|---------|-----------|------|
| transcript, soap_note, referral, letter, peer_discussion, chat | Bidirectional (delta) | On change + on startup + SSE-triggered |
| patient_name, tags, metadata | Bidirectional | Part of recording row |
| processing_status | Bidirectional | Per-field LWW |
| Audio WAV file | Client → Server | After recording completes (background upload) |
| Audio WAV file | Server → Client | On-demand only |
| Deletions (tombstones via `deleted_at`) | Bidirectional | Immediately on delete |

### Fields that do NOT sync

| Field | Reason |
|-------|--------|
| `audio_path` | Machine-specific absolute path; resolved locally by recording ID |
| `encryption_pending` | Machine-specific transient state |
| `id`, `filename`, `created_at` | Immutable, copied as-is on creation |
| `duration_seconds`, `file_size_bytes`, `stt_provider`, `ai_provider` | Set once at creation, copied as-is |

## Data model

### Migration m013 — Row-level `updated_at` on recordings

```sql
ALTER TABLE recordings ADD COLUMN updated_at TEXT;
UPDATE recordings SET updated_at = created_at WHERE updated_at IS NULL;
```

Any write to a recording row bumps `updated_at` to `now()`. This column drives delta filtering.

### Migration m014 — Per-field revision tracking

```sql
CREATE TABLE recording_field_revisions (
    recording_id  TEXT NOT NULL,
    field         TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    origin_device TEXT,
    PRIMARY KEY (recording_id, field),
    FOREIGN KEY (recording_id) REFERENCES recordings(id) ON DELETE CASCADE
);
CREATE INDEX idx_revisions_updated_at ON recording_field_revisions(updated_at);
```

Syncable fields: `transcript`, `soap_note`, `referral`, `letter`, `peer_discussion`, `chat`, `patient_name`, `tags`, `metadata`, `processing_status`.

`origin_device` enables human-readable conflict toasts ("SOAP note updated on Dr. Lee's laptop").

### Migration m015 — Sync state (cursor persistence)

```sql
CREATE TABLE sync_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

Keys: `content_sync_cursor` (max `updated_at` from last successful pull), `content_sync_last_pull` (ISO timestamp), `pending_audio_uploads` (JSON array of recording IDs).

### Audio file resolution

`audio_path` does not sync. Each machine resolves audio by recording ID: `{recordings_dir}/{recording_id}.enc`. The original capture filename is preserved in the `filename` column for display.

For audio transfer over Tailscale, the sender decrypts with its local FE1 key (in memory), streams plaintext bytes; the receiver re-encrypts with its own local FE1 key and writes to disk. Plaintext exists only momentarily in memory on each side; at rest it is always encrypted per-machine.

**Audio conflict policy:** Audio files do not participate in per-field LWW (they are binary blobs, not editable text). The policy is **first-upload wins**: if the server already has audio for a recording ID, subsequent `PUT /v1/content/audio/{id}` requests are rejected with `409 Conflict`. The server's copy is canonical. On-demand fetch always downloads the server's copy, overwriting any local file for that ID.

### Existing edit command integration

`save_recording_field` (the only text-edit path) is extended:
1. After writing the field, upsert `recording_field_revisions(recording_id, field, now(), machine_id)`.
2. Bump `recordings.updated_at = now()`.
3. If content sync is active, spawn a background push (fire-and-forget, like condition chips).

Every existing edit automatically participates in sync — no new edit pathways needed.

## Sync protocol

### New HTTP routes (vocab port 11437)

All routes require `Authorization: Bearer <token>`. Content sync routes additionally reject non-Tailscale IPs.

```
GET    /v1/content/sync?since=<ISO8601>&limit=200
       → 200 { recordings: [...], server_time, has_more }

POST   /v1/content/sync
       Body: { recordings: [...], deleted_ids: [...] }
       → 200 { recordings: [...], conflicts: [...] }

GET    /v1/content/sync/meta
       → 200 { server_time, recording_count, latest_updated_at }

GET    /v1/content/events          (SSE)
       → stream: data: connected / data: changed / data: deleted:{id}

GET    /v1/content/audio/{recording_id}
       → 200 audio/x-wav (streamed plaintext)

PUT    /v1/content/audio/{recording_id}
       Body: audio/x-wav (streamed plaintext)
       → 201 Created
```

### Wire format

```json
{
  "recordings": [
    {
      "id": "uuid",
      "filename": "Recording_2026-07-12_09-30-00.wav",
      "created_at": "2026-07-12T16:30:00Z",
      "updated_at": "2026-07-12T16:35:00Z",
      "deleted_at": null,
      "patient_name": "John Doe",
      "duration_seconds": 42.5,
      "fields": {
        "transcript": { "value": "...", "updated_at": "2026-07-12T16:31:00Z", "origin_device": "srv" },
        "soap_note":  { "value": "...", "updated_at": "2026-07-12T16:35:00Z", "origin_device": "laptop1" }
      }
    }
  ],
  "server_time": "2026-07-12T16:36:00Z"
}
```

Only fields that changed are included in `fields` (sparse). Unchanged fields are omitted.

### Pull protocol

```
1. Client reads cursor from sync_state
2. GET /v1/content/sync?since=<cursor>
3. Server: SELECT ... FROM recordings WHERE updated_at > ? ORDER BY updated_at LIMIT 200
4. Server includes per-field revisions for matching recordings
5. Client runs merge_incoming locally
6. Client stores new cursor = max(updated_at), or server_time if empty
7. If has_more=true, repeat from step 2
```

### Push protocol

```
1. Client collects locally modified recordings since last push
2. POST /v1/content/sync with sparse recording + revision data
3. Server runs merge_incoming for each recording/field
4. Server returns conflicts (fields where server had newer data)
5. Client merges server's response locally
6. Server broadcasts SSE "changed" to wake other clients
```

### Merge algorithm (per-field LWW, symmetric)

```
fn merge_recording(local, remote):
    for each field in remote.fields:
        local_rev = local_revisions[field]
        remote_rev = remote.fields[field]

        if remote_rev.updated_at > local_rev.updated_at:
            local[field] = remote_rev.value
            local_revisions[field] = remote_rev
        elif remote_rev.updated_at < local_rev.updated_at:
            conflicts.push({ field, local_ts, remote_ts })
        # equal timestamps: keep local, no conflict

    # Deletion: earliest deleted_at wins; later un-delete wins
    if remote.deleted_at && (!local.deleted_at || remote.deleted_at < local.deleted_at):
        local.deleted_at = remote.deleted_at
    if !remote.deleted_at && local.deleted_at && remote.updated_at > local.deleted_at:
        local.deleted_at = null

    local.updated_at = max(local.updated_at, remote.updated_at)
```

### New recording propagation

When a client records a new encounter:
1. Row created locally with `id`, `created_at`, transcript, etc. `updated_at = created_at`.
2. Audio file: `{recordings_dir}/{id}.enc`.
3. Background: `PUT /v1/content/audio/{id}` uploads decrypted audio bytes.
4. Background: `POST /v1/content/sync` pushes the new recording row.
5. Server broadcasts SSE `data: changed`.

### SSE event types

| Event | Trigger | Client action |
|-------|---------|---------------|
| `data: changed` | Any recording field updated | Pull deltas |
| `data: deleted:{id}` | Recording soft-deleted | Mark local as deleted (with toast) |
| `data: connected` | Initial SSE connection | No action (pull already ran on startup) |

## Sync lifecycle

### Startup sync

```
1. AppState::initialize checks content_sync_target()
2. If active → spawn background "initial sync" task:
   a. GET /v1/content/sync/meta → server's latest_updated_at
   b. If server latest > local cursor → paginate full pull (200-row batches)
   c. merge_incoming for each batch
   d. Push local recordings newer than server's last known state
   e. Emit Tauri event 'content-sync-complete'
3. SSE subscriber task starts (or reconnects)
4. Periodic push/pull tasks start
```

The initial pull is idempotent — if interrupted, it resumes from the stored cursor on next launch.

### Write triggers

| Trigger | What happens | Latency |
|---------|-------------|---------|
| Field edit (`save_recording_field`) | Write local → bump `updated_at` + revision → background push | ~2s (debounced) |
| New recording (`stop_recording`) | Insert row + revision → background push + audio upload | After encryption |
| Delete (`delete_recording`) | Soft-delete local → background push with tombstone | Immediate |

Background pushes are fire-and-forget with one retry. The periodic sync catches up any failures.

### Pull triggers

| Trigger | What happens |
|---------|-------------|
| SSE `changed` event | Immediate delta pull |
| SSE `deleted:{id}` event | Mark local as deleted, emit toast |
| App startup | Full delta pull |
| Periodic poll (60s) | Safety-net delta pull |
| Manual "Sync now" button | Full delta pull |

The 60s poll is slower than condition chips' 30s because recording data is heavier. SSE is the primary realtime path.

### Conflict detection & toasts

When a pull brings in a recording where a field's remote `updated_at` is newer than local:

```
1. merge_incoming updates the local field
2. Check: is this recording currently open in the editor?
   YES → check if local has unsaved edits (dirtySince within last 10s)
     YES → toast: "SOAP note updated on {origin_device}. Your unsaved edit was preserved."
     NO  → reload editor content, toast: "SOAP note updated on {origin_device}"
   NO  → silently update the store
3. Emit Tauri event 'recording-updated' { id, changed_fields }
```

### Tombstone sweeper

Server-side background task runs every 24h:

```
1. SELECT id, audio_path FROM recordings
   WHERE deleted_at IS NOT NULL AND deleted_at < datetime('now', '-30 days')
2. For each: delete audio file (if exists), DELETE FROM recordings,
   DELETE FROM recording_field_revisions
3. Log only counts — never recording IDs or content
```

Clients learn about permanent deletions via the next pull — the server stops returning those recordings. The client's local copy remains soft-deleted with its own 30-day window.

### Reconnection handling

```
1. SSE task detects disconnect → exponential backoff (5s→30s cap)
2. On reconnect → emit 'data: connected'
3. SSE task triggers immediate delta pull
4. Cursor ensures everything missed during outage is caught
```

## Frontend integration

### New AppConfig field

```rust
pub sync_content: bool,  // default: false
```

### Settings UI

`src/lib/components/settings/sharing/ContentSync.svelte` — mirrors `ConditionChipSync.svelte`:

- Checkbox: "Sync patient content via Tailscale"
- Description: "Syncs transcripts, SOAP notes, letters, and peer discussions between this machine and the server over your encrypted Tailscale connection."
- Warning: "Requires Tailscale on both machines"
- Status: "Last synced: X minutes ago" + "Sync now" button

**Visibility**: Only shown when `(sharingOn || pairedTo)` AND a Tailscale endpoint is detected. If paired but no Tailscale, shows disabled state: "Content sync requires Tailscale. Configure Tailscale to enable."

**On enable**: Calls `invoke('sync_content_now')` for immediate bidirectional sync, starts SSE subscriber.

### Recordings store changes

New state:
```typescript
syncActive: boolean
syncing: boolean
lastSyncedAt: Date | null
syncConflicts: ConflictToast[]
```

New methods:
- `syncNow()` — invokes `sync_content_now`, refreshes list on completion
- `handleRemoteUpdate(updatedRecording)` — merges into store, triggers conflict toast if open recording affected

### Tauri event listeners

```typescript
listen('recording-updated', (e) => handleRemoteUpdate(e.payload));
listen('recording-deleted-remote', (e) => {
    recordings.removeById(e.payload.id, { silent: true });
    toast.info(`Recording deleted on ${e.payload.origin}`);
});
listen('content-sync-complete', () => {
    recordings.load();
    recordings.lastSyncedAt = new Date();
});
invoke('subscribe_content_sync');
```

### Conflict toast

When a remote update affects the currently-open recording:
- No unsaved local edits → editor silently reloads newer content, toast: "SOAP note updated on {origin_device}"
- Unsaved local edits → toast with choice: "View" (discard local, load remote) or "Dismiss" (keep local, push on next save)

### Audio fetch UI

"Fetch Audio from Server" button on the transcript tab toolbar, shown only when content sync is active AND local audio file doesn't exist AND recording originated on another machine. Flow: `invoke('fetch_audio_from_server')` → backend downloads, re-encrypts, writes locally → "Export Audio" then works normally.

### Recording card sync badge

Subtle cloud icon (☁) with checkmark when audio is on the server, grayed-out when audio is local-only.

### What stays unchanged

- Recording list, search, card display
- All editing flows (transcript, SOAP, letters, peer discussion)
- Condition chip sync
- Audio capture, export
- Delete + undo (local behavior unchanged; sync propagation transparent)

## Error handling

### Network failures

| Scenario | Behavior |
|----------|----------|
| Push fails | Local write already committed. One retry. Periodic 60s poll reconciles. No data loss. |
| Pull fails | Cursor not advanced. Retried on next SSE event or poll. |
| Audio upload fails | Text still syncs. `pending_audio_uploads` tracks IDs. Retried on next cycle. |
| Audio fetch fails | Toast: "Audio not available on server." Text unaffected. |
| SSE disconnects | Exponential backoff (5s→30s). On reconnect, immediate delta pull. |

### Clock skew

1. **Server-time anchoring** — Server returns `server_time` on every response. If client clock delta exceeds ±60 seconds, client logs a warning (count only) and uses `server_time` as reference. Prevents a fast-clock client from always winning LWW.
2. **Resolution** — ISO 8601 UTC, millisecond precision. Edits are typically seconds-to-minutes apart.
3. **Tie-breaking** — On equal timestamps, lexicographically larger `origin_device` ID wins (deterministic).

### Race conditions

| Race | Resolution |
|------|-----------|
| User edits field while push in flight | Push carries pre-edit value. On completion, new push with latest. Worst case: 2s where old value briefly on server. |
| Two clients push simultaneously | Both hit server merge. Second sees first's changes. Server returns conflicts to loser. Both converge on next pull. |
| New recording during initial sync | Pull doesn't touch local-newer rows. Push sends new recordings after pull completes. |
| Client deletes while server edits | Delete vs edit LWW. Newer timestamp wins. |

### Large initial sync

1. **Pagination** — `limit=200`, `has_more` flag drives subsequent requests. Each batch merged independently.
2. **Background** — UI shows "Syncing…" but remains usable. New edits pushed after pull.
3. **No audio during initial sync** — Only text transfers. Audio fetched on-demand.

## HIPAA compliance

| Concern | Mitigation |
|---------|-----------|
| PHI over network | Tailscale wire encryption (WireGuard). Tailscale-gated at both ends. Server rejects non-CGNAT IPs. |
| PHI in logs | Counts, IDs, lengths only — never content. `tracing::info!("synced {} recordings", count)`. |
| PHI at rest | Audio re-encrypted with receiver's FE1 key. Text in SQLCipher DB (already encrypted). |
| No external services | Sync stays on existing vocab port. No new remote endpoints. No telemetry. Tailscale is peer-to-peer mesh, not a hosted service. |
| Access control | Existing bearer-token auth. Only paired clients sync. Server admin can revoke tokens. |

## Testing strategy

### Unit tests (`crates/db/tests/`)

- `recording_sync_merge.rs` — merge algorithm: per-field LWW, tie-breaking, deletion races, clock skew fallback
- `recording_field_revisions.rs` — revision upsert, cascade delete, delta query

### Integration tests (`crates/db/tests/`)

- Full sync cycle: create on A → push → pull on B → verify all fields match
- Concurrent edit: A edits SOAP, B edits transcript → both changes preserved
- Conflict: A and B edit same field → LWW resolves, loser gets conflict
- Delete propagation: A deletes → B receives tombstone
- Tombstone sweeper: create → delete → advance clock → verify purge

### Frontend tests (`src/lib/stores/*.test.ts`)

- `handleRemoteUpdate` — open recording gets conflict toast, closed recording silent update
- `syncNow` — invokes command, refreshes list

## Implementation scope

### New files

- `crates/db/src/migrations/m013_recording_updated_at.rs`
- `crates/db/src/migrations/m014_field_revisions.rs`
- `crates/db/src/migrations/m015_sync_state.rs`
- `crates/db/src/recording_sync.rs` — merge algorithm, delta queries, cursor management
- `src-tauri/src/content_remote.rs` — HTTP client (mirrors `conditions_remote.rs`)
- `src-tauri/src/commands/content_sync.rs` — Tauri commands (pull, push, subscribe, audio fetch/upload)
- `src/lib/components/settings/sharing/ContentSync.svelte`
- `src/lib/api/contentSync.ts`

### Modified files

- `crates/core/src/settings.rs` — add `sync_content: bool`
- `crates/db/src/migrations/mod.rs` — register m013/m014/m015
- `crates/db/src/mod.rs` — export recording_sync module
- `crates/db/src/recordings.rs` — add `updated_at` to Recording struct, update queries
- `crates/core/src/types/recording.rs` — add `updated_at` field
- `src-tauri/src/sharing_vocab_api.rs` — add `/v1/content/*` routes, new broadcast channel
- `src-tauri/src/commands/recordings_edit.rs` — bump `updated_at` + revision on save, add `peer_discussion` to editable fields
- `src-tauri/src/commands/audio.rs` — rename audio to `{id}.enc`, trigger background upload on sync
- `src-tauri/src/commands/recordings.rs` — propagate deletion via sync on soft-delete
- `src-tauri/src/state.rs` — startup sync task, tombstone sweeper
- `src-tauri/src/lib.rs` — register new commands
- `src/lib/stores/recordings.svelte.ts` — sync state, `handleRemoteUpdate`, `syncNow`
- `src/lib/pages/RecordingsTab.svelte` — event listeners
- `src/lib/pages/EditorTab.svelte` — conflict toast integration, "Fetch Audio" button
- `src/lib/components/RecordingCard.svelte` — sync badge
- `src/lib/components/settings/Sharing.svelte` — include ContentSync sub-component
- `src/lib/types/index.ts` — add sync-related types
