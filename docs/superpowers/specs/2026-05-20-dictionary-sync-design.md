# User Dictionary Sync — Design

Date: 2026-05-20
Status: Approved, ready for implementation plan
Related: 2026-04-20-custom-vocabulary-design.md (vocab sync, which this mirrors)

## Goal

When a client is paired with an office server, dictionary CRUD operations
should route through the server over HTTP — the same way vocabulary already
does. The server becomes the canonical source of truth while paired;
unpaired clients keep using their local SQLite `user_dictionary` table.

## Motivation

The vocabulary feature already gives paired clients a shared, server-canonical
correction list. The per-user spellcheck dictionary (`user_dictionary`) has
identical sharing requirements — multiple clinicians on paired client
machines should see one shared "accepted spellings" list anchored to the
office server — but today each client keeps its own local copy.

## Constraints

- Local-only AI providers; no telemetry. (No change to that posture — this
  is the same intranet HTTP path vocabulary already uses.)
- No PHI in logs. The dictionary may hold patient-context-specific terms,
  so handlers must log lengths/counts only — never word values.
- Same `vocab_port` and same bearer/`TokenStore` as the existing vocab API:
  no new port, no protocol bump on discovery (mDNS / QR).

## Architecture

### Server side (office server)

`src-tauri/src/sharing_vocab_api.rs` already hosts an axum router on
`vocab_port` (default 11437), authenticated against `TokenStore`. Add three
handlers to that same router:

| Method | Path                          | Body                | Response       |
|--------|-------------------------------|---------------------|----------------|
| GET    | `/v1/user-dictionary`         | —                   | `Vec<String>`  |
| POST   | `/v1/user-dictionary`         | `{ "word": "..." }` | `bool` (true if a new row was inserted) |
| DELETE | `/v1/user-dictionary/{word}`  | —                   | 204 No Content |

- All three call `medical_db::user_dictionary::UserDictionaryRepo` against
  the office server's local SQLite DB.
- Handlers run blocking DB work inside `tokio::task::spawn_blocking`, same
  pattern as the existing vocab handlers.
- Authorization: reuse the `authorize(&state, &headers)` helper already in
  the file — single bearer check, no per-user scoping (mirrors vocab).
- Logging: `info!(word_len = ..., "user_dict_api: added")` /
  `info!("user_dict_api: deleted")` — never log the word itself.

### Client side (paired client)

New file: `src-tauri/src/user_dict_remote.rs`, parallel to `vocab_remote.rs`.

```rust
pub struct UserDictRemote<'a> {
    pub conn: &'a PairedConnection,
    pub bearer: String,
    pub client: Arc<reqwest::Client>,
}

impl<'a> UserDictRemote<'a> {
    pub fn from(conn, bearer, client) -> Option<Self> { /* requires vocab_port */ }
    fn base_url(&self) -> Option<String>;           // LAN preferred, Tailscale fallback
    pub async fn list(&self) -> AppResult<Vec<String>>;
    pub async fn add(&self, word: &str) -> AppResult<bool>;
    pub async fn remove(&self, word: &str) -> AppResult<bool>;
}
```

Same timeout (15 s), same bearer header pattern, same `check_status` error
mapping (404 → "office server too old", 401 → "re-pair", else
`"dictionary API: HTTP <status>"`).

`src-tauri/src/commands/user_dictionary.rs` gains a `paired_dict_target()`
helper identical in shape to `paired_vocab_target()` — checks for a paired
connection with a `vocab_port` and an available bearer. Each command
(`user_dict_list`, `user_dict_add`, `user_dict_remove`) gets one early
branch: if paired, build a `UserDictRemote` and call the HTTP method;
otherwise run the existing local DB path unchanged.

### Why the same `vocab_port` (not a new port)

- Same axum process, same `TokenStore`, same firewall/Tailscale rules
  already configured.
- The advertised-ports payload (mDNS TXT + QR code) already carries
  `vocab`, so clients automatically know how to reach the dictionary API
  through the same address — no discovery protocol change.
- An older office server (one that has `vocab_port` but no dictionary
  routes yet) returns 404 for `/v1/user-dictionary`, and `check_status`
  surfaces a clear upgrade message — same fallback the vocab client uses.

A dedicated `dict_port` was considered and rejected: it would add a config
field, a new bind, a new mDNS/QR entry, and an "older server lacks dict
port" branch on the client, all to solve a problem the 404 already solves.

## Data flow

### Read (spellchecker init)
1. Frontend `listUserDict()` → `user_dict_list` Tauri command.
2. `paired_dict_target()`:
   - `Some((conn, bearer))` → `UserDictRemote::list()` →
     `GET /v1/user-dictionary` → `Vec<String>` to frontend.
   - `None` → `UserDictionaryRepo::list(&local_conn)` (existing behavior).
3. Spellchecker's existing `.catch(() => [])` still handles transient
   network errors; this is a one-time init, not a per-word call.

### Add / remove
- `user_dict_add(word)` paired → `POST /v1/user-dictionary { word }` →
  server runs `UserDictionaryRepo::add`. Returns `bool`.
- `user_dict_remove(word)` paired → `DELETE /v1/user-dictionary/{word}`
  (path-encoded) → server runs `UserDictionaryRepo::remove`. Returns
  `bool`.
- Unpaired paths unchanged.

### Source-of-truth rule

While paired, the paired client never touches its own `user_dictionary`
table — the server's DB is canonical. Mirrors vocab exactly, including the
deliberate consequence that words added while paired aren't retained on
the local client DB if it later unpairs.

## Error handling

`UserDictRemote` reuses the same vocabulary error vocabulary:

- HTTP 404 → `"Office server does not support dictionary sync (update it to vX.Y.Z or later)."`
  The exact version string is filled in during implementation, once the
  shipping version is known.
- HTTP 401 → `"Office server rejected the bearer token. Try unpair → re-pair from this client."`
- Other non-2xx → `"dictionary API: HTTP <status>"`.
- reqwest / timeout errors → `AppError::Other("dict <op>: <err>")`.

Word encoding for the DELETE path: percent-encode the path segment (use
`urlencoding::encode` on the client; `axum::Path<String>` decodes on the
server). Case-insensitive matching is preserved by `UserDictionaryRepo::remove`,
so `Lisinopril` and `lisinopril` resolve to the same row regardless of
which form was DELETEd.

## Testing

- `UserDictionaryRepo` — existing unit tests cover case-insensitive
  add/remove/contains/list; no changes needed.
- `sharing_vocab_api.rs` handlers — follow whatever convention the
  existing vocab handlers use in that file. If they have unit tests, add
  equivalent ones for the dictionary routes; if not, do not introduce a
  test scaffold solely for this change.
- `user_dict_remote.rs` — covered indirectly through command tests. A
  small `mockito`/`wiremock` test is optional and only worth adding if
  `vocab_remote.rs` has one.
- `commands/user_dictionary.rs` — verify routing branch selection with a
  faked paired-connection state, mirroring whatever vocab command tests
  do today.
- `src/lib/api/userDictionary.test.ts` — unchanged; the frontend contract
  (the three Tauri command names and their argument shape) is identical.

## Out of scope (YAGNI)

- No bulk import/export, no `delete_all`, no batch sync. Surface stays
  three operations.
- No backfill of locally-added words to the server on first pair.
- No pull-down of server words to the local DB on unpair.
- No per-user scoping on the server (all bearers see the same shared
  dictionary, same as vocab).

These can be added later as separate features without disturbing this
design.

## Files touched

- `src-tauri/src/sharing_vocab_api.rs` — add three handlers + route
  registration.
- `src-tauri/src/user_dict_remote.rs` — new file.
- `src-tauri/src/lib.rs` — `mod user_dict_remote;` declaration.
- `src-tauri/src/commands/user_dictionary.rs` — add `paired_dict_target()`
  and route through `UserDictRemote` when paired.

No migrations. No new ports. No changes to mDNS / QR discovery payload.
No frontend changes.
