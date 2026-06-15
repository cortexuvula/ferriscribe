# Sharing Server: Readiness Gate with Self-Healing

**Date:** 2026-06-15
**Status:** Approved → implementation planning
**Scope:** `crates/sharing/`, `src-tauri/src/commands/sharing/lifecycle.rs`, `ServerStatus.svelte`

## Problem

When FerriScribe auto-launches at login (via the OS Login Items) and auto-resumes
office-server mode, the sharing server starts before the local AI/STT upstreams
(Ollama, whisper-server child, LM Studio) are guaranteed reachable. There is
**no upstream readiness gate anywhere** in the chain, producing three distinct
failure modes for **paired client devices** (laptops, iPads):

1. **LM Studio silently missing.** `lmstudio_running_port()` does a one-shot
   300 ms probe to `127.0.0.1:1234/v1/models` (`lifecycle.rs:260-272`). At login
   LM Studio is mid-boot, so the probe fails. The LM Studio auth proxy is never
   bound, `lmstudio_proxy_port` is `None`, and LM Studio is **omitted from the
   mDNS TXT record** (`orchestrator.rs:243-270`, `mdns.rs:107-109`). Clients
   never see LM Studio until the user manually Stops + Starts sharing.

2. **Ollama/whisper 502s on early requests.** The Ollama and whisper auth
   proxies bind their listener ports regardless of whether their upstream is
   ready (`orchestrator.rs:204-236`). A client request that arrives before the
   upstream is up makes the proxy's `reqwest::send()` fail, and it returns
   **HTTP 502 Bad Gateway** (`auth_proxy.rs:171-174`).

3. **`SharingStatus` lies.** `ollama_ok`/`whisper_ok`/`lmstudio_ok` are just
   copies of the `running` boolean (`orchestrator.rs:361-369`), so the degraded
   state is invisible to the UI and to polling clients.

The same race produces both symptoms; they differ only in which downstream
effect a client sees.

## Design choice

Strategy: **gate then self-heal** (approved).

Don't bind an upstream's proxy or advertise its port until that upstream
actually answers. A short **gate window** at start catches upstreams that are
nearly ready; a long-lived **ReadinessWatcher** brings late arrivals online
(e.g. LM Studio finishing its boot) and binds + re-advertises them.
`SharingStatus` reflects the watcher's real probe results.

Explicitly **out of scope**: per-request retry inside `auth_proxy` (the
"Both" option). If a gated-up upstream later dies mid-session, the proxy 502s
and the watcher flags it degraded in status, but the request is not retried.

## Components

### A. Upstream probe module — `crates/sharing/src/upstream.rs` (new)

```text
UpstreamKind        := Ollama | Whisper | LmStudio
UpstreamTarget      := UpstreamKind + base_url: String
probe_ready(client, &UpstreamTarget) -> bool
    Ollama   -> GET {base}/api/tags            (3s timeout) 200 == ready
    Whisper  -> GET {base}/v1/models           (3s)          200 == ready
    LmStudio -> GET {base}/v1/models           (3s)          200 == ready
probe_with_backoff(client, target, deadline) -> bool
    poll probe_ready() on a bounded backoff schedule until deadline
```

Pure async functions; no I/O state. Unit-testable with `wiremock` (ready /
not-ready / slow-then-ready).

### B. Readiness cache — on `SharingService`

```text
readiness: Arc<RwLock<HashMap<UpstreamKind, ProbeState>>>

ProbeState := {
    configured: bool,        // upstream is part of this server's config
    proxy_bound: bool,       // auth proxy currently listening for it
    last_probe_ok: bool,     // most recent probe succeeded
    last_probe_at: Instant,
}
```

Initialized in `SharingService::new()` from the config: `Ollama` and `Whisper`
always configured; `LmStudio` configured iff `lmstudio_internal_port.is_some()`.

### C. Revised `SharingService::start()` — gate window

```text
1. bind pairing (11436) + vocab (11437)             # no upstream dependency
2. start WhisperSupervisor child                    # as today
3. GATE (~5s, concurrent): probe_with_backoff each configured upstream
       each ready upstream -> bind its auth proxy, set proxy_bound=true
4. build mDNS TXT + /info snapshot from the READY subset -> advertise once
5. spawn ReadinessWatcher task                      # see D
6. return Ok   ->   *sharing_slot set   ->   sharing-server.json written
```

`start()` blocks up to ~5 s instead of <1 s. The "Start sharing" button shows
its existing spinner; at auto-resume (login) the window simply appears a few
seconds later. Acceptable per approval.

### D. ReadinessWatcher — self-heal + health

A single background task owned by `SharingService`, aborted in `stop()`.
Every **10 s** it probes **all configured upstreams**:

- **Not-yet-bound upstream became ready** → bind its auth proxy, set
  `proxy_bound=true`, rebuild mDNS TXT + `/info`, emit Tauri event
  `sharing-readiness-changed`. *(The LM Studio fix.)*
- **Bound upstream now unreachable** → set `last_probe_ok=false` (status
  reflects it; proxy stays bound and 502s, no per-request retry).
- **Everything healthy** → refresh `last_probe_at`.

Runs for the lifetime of the service, so it also catches an upstream that's
started manually later in the session — not just at login. Uses a
`tokio::sync::watch`/`Notify` + the 10 s tick; cooperatively checks a
`running` flag so `stop()` can terminate it.

### E. Honest `SharingStatus`

```text
ollama_ok   = running && readiness[Ollama].last_probe_ok
whisper_ok  = running && readiness[Whisper].last_probe_ok
lmstudio_ok = running && readiness[LmStudio].configured && readiness[LmStudio].last_probe_ok
```

Frontend `ServerStatus.svelte` already polls `sharing_status` every 5 s → real
green/red per service surfaces automatically.

### F. mDNS re-advertisement

`MdnsAdvertiser::update_ports(&new_ports)` — unregister current service info,
re-register with the new TXT record. Called by the watcher only when the ready
set changes (rare). Re-registration is the well-supported `mdns-sd` path
(`unregister` → `register`).

### G. Dynamic `/info`

`spawn_pairing_service` takes an `Arc<RwLock<InfoSnapshot>>`; `info_handler`
reads the live value; the watcher updates it alongside mDNS. Keeps Tailscale
discovery probing (`GET :11436/info`) consistent with mDNS.

### H. Readiness-change notifications (layering-correct)

`crates/sharing/` is a pure library crate with no `tauri` dependency, so the
watcher must **not** hold an `AppHandle` or emit Tauri events directly. Instead:

- `SharingService::start()` spawns the watcher with a
  `tokio::sync::watch::Sender<ReadinessSnapshot>` (a serializable summary of
  `readiness` + `running`). The watcher sends a new snapshot whenever the
  ready set changes.
- `SharingService::readiness_changes() -> watch::Receiver<ReadinessSnapshot>`
  hands out a receiver.
- `lifecycle.rs` (`start_sharing_inner`) spawns a tiny forwarder task:
  `while rx.changed().await is Ok { app_handle.emit("sharing-readiness-changed", ()) }`.
  This keeps the Tauri dependency in the Tauri layer.
- The frontend listens for `sharing-readiness-changed`, invalidates its
  `sharing_status` cache, and shows a toast.

This is unit-testable without tauri (the test just reads the watch receiver).

## Data flow (login-launch, LM Studio slow to boot)

```text
T=0   start()
T=0   bind pairing/vocab, start whisper supervisor
T=0   gate: probe all
        ollama  up @ T=2s   -> bind proxy 11435, mark ready
        whisper up @ T=3s   -> bind proxy 8081,  mark ready
        lmstudio down @ T=5s -> SKIP (not bound, not advertised)
T=5   advertise mDNS TXT { ollama, whisper, pairing, vocab }  (no lmstudio)
T=5   spawn ReadinessWatcher
T=5   start() returns Ok -> sharing-server.json written

clients see ollama+whisper now; never see a 502 (both gated up)

T=40  LM Studio finishes booting
T=50  watcher tick probes lmstudio -> ready
        -> bind proxy 1235, set proxy_bound
        -> update mDNS TXT { ollama, whisper, lmstudio, pairing, vocab }
        -> update /info snapshot
        -> emit sharing-readiness-changed
clients (re-discovering via mDNS) now see LM Studio
```

## Files touched

| File | Change |
|---|---|
| `crates/sharing/src/upstream.rs` **(new)** | `UpstreamKind`, `UpstreamTarget`, `probe_ready`, `probe_with_backoff` + tests |
| `crates/sharing/src/orchestrator.rs` | gate logic in `start()`; readiness cache; `ReadinessWatcher`; honest `status()`; dynamic `/info`; `readiness_changes()` watch channel |
| `crates/sharing/src/mdns.rs` | `MdnsAdvertiser::update_ports()` |
| `crates/sharing/src/lib.rs` | export `upstream` module |
| `src-tauri/src/commands/sharing/lifecycle.rs` | drop one-shot `lmstudio_running_port`; **configure LM Studio as an always-candidate** (ports 1234/1235) so the gate/watcher can detect it; spawn the `watch` → `app_handle.emit()` forwarder |
| `src/lib/components/settings/sharing/ServerStatus.svelte` | listen for `sharing-readiness-changed` + toast; invalidate status cache |

## Testing

- `probe_ready()` unit tests with `wiremock` (ready / not-ready / slow-then-ready).
- `probe_with_backoff()` honours the deadline.
- `start()` gate test: two mock upstreams — one ready immediately, one ready
  after ~1 s → assert proxy binding order + that the mDNS TXT reflects the
  ready subset at advertisement time.
- Watcher test: start with no upstream ready → bring one up on a mock → assert
  proxy binds, mDNS re-advertises, readiness cache flips, event fires.
- `status()` reflects the probe cache, not just `running`.
- Update existing `sharing_service_status_*` tests for the new field semantics.
- `MdnsAdvertiser::update_ports()` smoke test (guarded by
  `FERRISCRIBE_MDNS_TEST=1`, like the existing advertiser test).

## Constraints honored

- **No PHI in logs.** Watcher logs counts/IDs only ("upstream Ollama became
  ready", "re-advertised 4 ports") — never model names, request bodies, or
  transcripts.
- **No new remote endpoints.** All probes are to `127.0.0.1`.
- **Local-only.** No hosted-AI involvement; the gate merely waits for the
  user's own local Ollama / whisper / LM Studio.

## Open questions resolved

- Gate window: **5 s** (default).
- Watcher interval: **10 s**.
- Per-request proxy retry: **no** (gate + watcher only).
- LM Studio config: in office mode, always set `lmstudio_internal_port=Some(1234)`
  and `lmstudio_proxy_port=Some(1235)` (i.e. LM Studio is always a *candidate*).
  The one-shot `lmstudio_running_port` probe is removed from
  `build_sharing_config`. The gate at `start()` decides whether to bind the
  proxy *now*; the watcher brings it up later if LM Studio boots after the gate.
  Clients only see LM Studio once it's actually ready — no Stop+Start needed.
- Watcher → frontend notifications: `tokio::sync::watch` channel out of the
  library crate; Tauri event emission happens in `lifecycle.rs` (layering).
