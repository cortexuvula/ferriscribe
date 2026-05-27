# medical-sharing

LAN and Tailscale sharing for FerriScribe. Run AI inference and speech-to-text
on a powerful office server; clinicians connect from laptops over the local
network or a Tailscale mesh.

> **PHI safety.** No patient data ever crosses these modules. Audio bytes pass
> through the auth proxy as opaque body bytes. Nothing in this crate writes
> transcripts, SOAP notes, medications, or allergies to logs or stdout.

## How it fits in the workspace

```
medical-core          medical-security
      \                    /
       \                  /
        medical-sharing          <-- you are here
              |
          src-tauri        (Tauri commands wire settings UI to SharingService)
```

| Dependency | What we use |
|---|---|
| `medical-core` | Shared error types, version constants, provider enums |
| `medical-security` | SQLCipher key derivation for the token store |
| `src-tauri` (downstream) | Tauri commands that call `SharingService::start/stop/status` and expose paired-client management to the Svelte frontend |

## Architecture at a glance

```
+--------------------- office server (this crate) --------------------+
|                                                                      |
|  SharingService (orchestrator)                                       |
|   |-- auth_proxy :11435  --> Ollama 127.0.0.1:11434                  |
|   |-- auth_proxy :8081   --> whisper-server 127.0.0.1:8080           |
|   |-- auth_proxy :1235   --> LM Studio 127.0.0.1:1234  (optional)    |
|   |-- pairing svc :11436 --> /pair/enroll, /info, /pair/clients      |
|   |-- mDNS advertiser      _ferriscribe._tcp.local.                  |
|   |-- WhisperSupervisor    downloads + respawn whisper-server         |
|   +-- TokenStore (SQLCipher)  bearer tokens hashed at rest            |
|                                                                      |
+----------------------------------------------------------------------+
         ^                              ^
         | bearer token                 | bearer token
    clinic-laptop                  clinic-tablet
```

## Modules

| Module | Purpose |
|---|---|
| `orchestrator` | `SharingService` -- the public face. `start()` boots all subsystems; `stop()` tears them down. `SharingConfig` and `SharingStatus` live here. |
| `auth_proxy` | Bearer-validated reverse proxy. One instance fronts Ollama, a second fronts whisper.cpp, an optional third fronts LM Studio. |
| `pairing` | One-shot 6-digit enrollment codes that exchange for long-lived per-client tokens. |
| `token_store` | Per-client token CRUD backed by a SQLCipher-encrypted SQLite file. Tokens are SHA-256-hashed before persistence; the raw value is returned exactly once at issue time. |
| `mdns` | mDNS advertiser (`_ferriscribe._tcp.local.`) and browser for server discovery on the LAN. |
| `qr` | Encode/decode the `ferriscribe://pair?...` URL that the QR code carries. |
| `service_installer` | Per-platform persistent Ollama service writers -- launchd (macOS), systemd (Linux), Scheduled Task (Windows). |
| `whisper_supervisor` | Downloads the whisper-server binary (Windows prebuilt; manual build for macOS/Linux), spawns the child process, and restarts on crash with exponential backoff. |
| `tailscale` | Parses `tailscale status --json` output to extract the machine's Tailscale DNS name. |
| `suggested_label` | Sanitises the OS hostname into a safe default client label (never PHI). |

## How it works

### Server mode (office server)

1. **`SharingService::start()`** binds all listeners synchronously so port
   conflicts surface immediately as errors.
2. Three **auth proxy** instances start (Ollama, whisper, optionally LM Studio).
   Each strips the inbound `Authorization: Bearer <client-token>`, validates it
   against the token store, and forwards the request to the loopback-only
   backend. The whisper proxy additionally injects a static API key so
   whisper-server sees its expected `--api-key` value.
3. The **whisper supervisor** downloads the platform binary from the manifest
   (if a prebuilt exists), verifies SHA-256, extracts it, and spawns
   `whisper-server --host 127.0.0.1`. If the child crashes, the supervisor
   restarts it with exponential backoff (1s -> 60s cap).
4. The **mDNS advertiser** broadcasts `_ferriscribe._tcp.local.` with TXT
   records for each service port so clients on the LAN can discover the server.
5. The **pairing HTTP service** exposes `/pair/enroll` (public, exchanges a
   6-digit code for a bearer token), `/info` (public discovery snapshot), and
   loopback-only admin endpoints (`/pair/clients`, `/pair/revoke/:id`).
6. The **service installer** writes a launchd plist, systemd user unit, or
   Windows Scheduled Task so Ollama survives reboots without the FerriScribe
   app running.

### Client mode (laptop / tablet)

1. **mDNS browse** discovers servers on the LAN. For Tailscale-only networks,
   the client probes known Tailscale DNS names on the pairing port's `/info`
   endpoint.
2. The server admin generates a **6-digit pairing code** (or QR encoding
   `ferriscribe://pair?...`). The client submits the code to `/pair/enroll`
   along with a human-readable label.
3. The pairing service validates the code (one-shot, 10-minute TTL), issues a
   long-lived bearer token from the token store, and returns it.
4. All subsequent STT and AI requests from the client include
   `Authorization: Bearer <token>`, which the auth proxy validates before
   forwarding to the backend.

### Token lifecycle

| Event | What happens |
|---|---|
| Pairing | `TokenStore::issue(label)` generates 32 random bytes, base64url-encodes them, stores only the SHA-256 hash. The raw token is returned exactly once. |
| Each proxied request | `TokenStore::validate(token)` hashes the presented token and looks it up. On hit, `touch(id)` updates `last_seen_at`. |
| Admin revokes client | `TokenStore::revoke(id)` sets `revoked_at`. The token immediately fails validation. Revocation is idempotent. |
| Rename client | `TokenStore::update_label(id, new)` trims whitespace, rejects empty strings, truncates to 80 Unicode chars. |

## Examples

### Pairing flow

```rust
use medical_sharing::pairing::PairingState;
use medical_sharing::token_store::TokenStore;
use std::sync::Arc;

// Server side: issue a code (display it or encode as QR)
let pairing = PairingState::new(store.clone());
let code = pairing.issue_code().await;   // e.g. "042917"

// Client side: submit the code
let token = pairing.enroll("042917", "clinic-laptop").await?;
// `token` is the bearer for all subsequent requests
```

### How STT requests are proxied

```
clinic-laptop                          office server
    |                                       |
    | POST /v1/audio/transcriptions         |
    | Authorization: Bearer <client-token>  |
    | Content-Type: multipart/form-data     |
    | (audio bytes in body)                 |
    |-------------------------------------->|  :8081 (whisper auth proxy)
    |                                       |
    |                   validate bearer     |
    |                   strip Authorization |
    |                   inject api-key      |
    |                                       |
    |                   POST /v1/audio/...  |
    |                   Authorization: ...  |-->|  :8080 (whisper-server, loopback)
    |                                       |
    |<--------------------------------------|  streaming response
```

### QR code URL format

The `ferriscribe://pair?...` URL carries everything a client needs to connect:

```
ferriscribe://pair?code=042917&host=Clinic+Server&lan=192.168.1.42
    &op=11435&wp=8081&pp=11436&ts=clinic.tail-abc.ts.net&vp=11437
```

| Param | Meaning |
|---|---|
| `host` | Friendly server name |
| `lan` | LAN IPv4 (optional) |
| `ts` | Tailscale DNS name (optional) |
| `op` | Ollama proxy port |
| `wp` | Whisper proxy port |
| `pp` | Pairing service port |
| `lp` | LM Studio proxy port (optional) |
| `vp` | Vocab sync port (optional) |
| `code` | 6-digit pairing code |

## Gotchas

### `x-auth-reason` header contract

When the auth proxy rejects a request with 401, it includes an
`x-auth-reason` response header. The `stt-providers` crate inspects this
header to distinguish "token expired/revoked" (trigger re-pair) from "server
unreachable" (retry or switch provider). Current values:

- `missing-bearer` -- no `Authorization` header at all
- `unknown-token` -- token hash not found or already revoked

### whisper-server on macOS/Linux

Prebuilt whisper-server binaries are only published for Windows x86_64. On
macOS and Linux, the office-server admin must build `whisper-server` from
source following <https://github.com/ggml-org/whisper.cpp#server> and place
it where the manifest expects it. The supervisor returns
`WhisperError::UnsupportedPlatform` when no prebuilt is available.

### Pairing traffic is plain HTTP

The `/pair/enroll` endpoint and the pairing code exchange happen over
unencrypted HTTP on the LAN. On trusted office networks this is acceptable --
the 6-digit code has a 10-minute TTL and one-shot semantics. On guest
networks or untrusted Wi-Fi, **use Tailscale** so the pairing traffic is
encrypted end-to-end via WireGuard.

### Service installer platform differences

| Platform | Mechanism | File location |
|---|---|---|
| macOS | launchd LaunchAgent | `~/Library/LaunchAgents/com.ferriscribe.ollama.plist` |
| Linux | systemd user unit | `~/.config/systemd/user/ferriscribe-ollama.service` |
| Windows | Scheduled Task | `FerriScribe Ollama` (via `schtasks`) |

The installer skips installation if something is already listening on
Ollama's default port (11434), to avoid conflicting with an externally
managed Ollama (Homebrew, Ollama.app, etc.).

### LM Studio proxy is opportunistic

The LM Studio auth proxy is only wired up when LM Studio's local server is
detected at `start()` time. If the user starts LM Studio *after* enabling
sharing, they must Stop and Start sharing again for clients to see LM Studio
models.

### Token store key management

The token store is encrypted with SQLCipher. The 32-byte key is derived by
`medical-security` and stored in the OS keychain. Opening the store with the
wrong key does not immediately fail -- the first query will error instead.

## Testing

```bash
# Unit tests (no network required)
cargo test -p medical-sharing --lib

# mDNS smoke test (requires LAN access, opt-in)
FERRISCRIBE_MDNS_TEST=1 cargo test -p medical-sharing --lib -- advertise_then_browse
```

Most tests use `wiremock` for HTTP mocking and `tempfile::tempdir()` for
isolated token store databases. The mDNS test is gated behind
`FERRISCRIBE_MDNS_TEST=1` because it broadcasts on the real network.
