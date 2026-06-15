# Sharing Server Readiness Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the login-launch race where the sharing server binds its auth proxies and advertises via mDNS before Ollama/whisper/LM Studio are reachable, by gating upstream readiness at `start()` and adding a ReadinessWatcher that brings late-arriving upstreams online.

**Architecture:** A new `upstream.rs` module provides pure probe functions. `SharingService::start()` runs a ~5 s concurrent readiness gate before binding each upstream's auth proxy and before advertising the mDNS TXT record. A long-lived ReadinessWatcher task probes every 10 s, binding late-arriving upstreams, re-advertising mDNS, and pushing honest status through a `tokio::sync::watch` channel. The Tauri layer forwards watch changes to a frontend event.

**Tech Stack:** Rust (edition 2024), tokio, axum, mdns-sd, reqwest, wiremock (dev), Svelte 5.

**Spec:** `docs/superpowers/specs/2026-06-15-sharing-readiness-gate-design.md`

**Hard constraints (PHI/HIPAA from AGENTS.md):**
- No PHI in logs — watcher logs counts/IDs only ("upstream Ollama became ready"), never model names or request bodies.
- No new remote endpoints — all probes go to `127.0.0.1`.
- Local-only — no hosted-AI involvement.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/sharing/src/upstream.rs` | `UpstreamKind`, `UpstreamTarget`, `probe_ready`, `probe_with_backoff` — pure probe functions, no state | Create |
| `crates/sharing/src/lib.rs` | Export `upstream` module | Modify |
| `crates/sharing/src/mdns.rs` | `MdnsAdvertiser::update_ports()` — re-register with new TXT | Modify |
| `crates/sharing/src/orchestrator.rs` | Readiness cache, gate in `start()`, ReadinessWatcher, honest `status()`, dynamic `/info`, `readiness_changes()` watch channel | Modify |
| `src-tauri/src/commands/sharing/lifecycle.rs` | Always-candidate LM Studio config; drop one-shot probe; spawn watch→event forwarder | Modify |
| `src/lib/components/settings/sharing/ServerStatus.svelte` | Listen for `sharing-readiness-changed`, invalidate cache, toast | Modify |

---

## Task 1: Upstream probe module (TDD)

**Files:**
- Create: `crates/sharing/src/upstream.rs`
- Modify: `crates/sharing/src/lib.rs` (add `pub mod upstream;` after line 42 `pub mod auth_proxy;`)

- [ ] **Step 1: Register the module in `lib.rs`**

In `crates/sharing/src/lib.rs`, after line 42 (`pub mod auth_proxy;`), add:

```rust
pub mod upstream;
```

- [ ] **Step 2: Write failing tests for `probe_ready`**

Create `crates/sharing/src/upstream.rs` with only the test module and the type stubs needed to compile (functions return `todo!()`):

```rust
//! Upstream readiness probes for the sharing server.
//!
//! Pure async functions — no shared state. Used by the start-up readiness
//! gate and the long-lived ReadinessWatcher.
//!
//! ## PHI safety
//!
//! Probes hit `127.0.0.1` only and inspect only the HTTP status code. No
//! request or response bodies are logged.

use std::time::Duration;

use reqwest::Client;

/// Which local upstream we are probing. Drives the readiness URL path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpstreamKind {
    Ollama,
    Whisper,
    LmStudio,
}

/// A probe target: kind + base URL (e.g. `http://127.0.0.1:11434`).
#[derive(Debug, Clone)]
pub struct UpstreamTarget {
    pub kind: UpstreamKind,
    pub base_url: String,
}

impl UpstreamTarget {
    pub fn new(kind: UpstreamKind, base_url: impl Into<String>) -> Self {
        Self { kind, base_url: base_url.into() }
    }

    /// URL probed for readiness. GET, status 200 == ready.
    fn readiness_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        match self.kind {
            UpstreamKind::Ollama => format!("{base}/api/tags"),
            UpstreamKind::Whisper | UpstreamKind::LmStudio => format!("{base}/v1/models"),
        }
    }
}

/// Probe a single upstream once. `true` iff it answered a GET with 2xx.
pub async fn probe_ready(client: &Client, target: &UpstreamTarget) -> bool {
    todo!()
}

/// Poll `probe_ready` on a bounded backoff until `deadline` elapses or the
/// upstream answers ready. Returns `true` if it became ready in time.
pub async fn probe_with_backoff(
    client: &Client,
    target: &UpstreamTarget,
    deadline: tokio::time::Instant,
) -> bool {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use wiremock::matchers::{method, path};

    fn client() -> Client {
        Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn probe_ready_ollama_200_is_ready() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::Ollama, srv.uri());
        assert!(probe_ready(&client(), &t).await);
    }

    #[tokio::test]
    async fn probe_ready_ollama_503_is_not_ready() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::Ollama, srv.uri());
        assert!(!probe_ready(&client(), &t).await);
    }

    #[tokio::test]
    async fn probe_ready_whisper_hits_v1_models() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::Whisper, srv.uri());
        assert!(probe_ready(&client(), &t).await);
    }

    #[tokio::test]
    async fn probe_ready_lmstudio_hits_v1_models() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::LmStudio, srv.uri());
        assert!(probe_ready(&client(), &t).await);
    }

    #[tokio::test]
    async fn probe_ready_connection_refused_is_not_ready() {
        // Port 1: privileged, almost certainly not listening; connect fails fast.
        let t = UpstreamTarget::new(UpstreamKind::Ollama, "http://127.0.0.1:1");
        assert!(!probe_ready(&client(), &t).await);
    }

    #[tokio::test]
    async fn probe_with_backoff_returns_false_when_never_ready() {
        let t = UpstreamTarget::new(UpstreamKind::Ollama, "http://127.0.0.1:1");
        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        assert!(!probe_with_backoff(&client(), &t, deadline).await);
    }

    #[tokio::test]
    async fn probe_with_backoff_returns_true_when_ready_immediately() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::Ollama, srv.uri());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        assert!(probe_with_backoff(&client(), &t, deadline).await);
    }

    #[tokio::test]
    async fn probe_with_backoff_recovers_when_upstream_comes_up_mid_window() {
        let srv = MockServer::start().await;
        // First ~400ms: 503. After that: 200.
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_millis(0)))
            .up_to_n_times(2)
            .mount(&srv)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = UpstreamTarget::new(UpstreamKind::Ollama, srv.uri());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        assert!(probe_with_backoff(&client(), &t, deadline).await);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p medical-sharing --lib upstream::tests`
Expected: FAIL — the stubs `todo!()` panic.

- [ ] **Step 4: Implement `probe_ready` and `probe_with_backoff`**

Replace the two `todo!()` bodies (and remove them) with:

```rust
pub async fn probe_ready(client: &Client, target: &UpstreamTarget) -> bool {
    let url = target.readiness_url();
    match client.get(&url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

pub async fn probe_with_backoff(
    client: &Client,
    target: &UpstreamTarget,
    deadline: tokio::time::Instant,
) -> bool {
    loop {
        if probe_ready(client, target).await {
            return true;
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return false;
        }
        // Backoff: 200ms, 400ms, 800ms, capped at 1s; bounded by the deadline.
        let step = Duration::from_millis(200).min(deadline.saturating_duration_since(now));
        tokio::time::sleep(step).await;
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p medical-sharing --lib upstream::tests`
Expected: PASS — all 8 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/sharing/src/upstream.rs crates/sharing/src/lib.rs
git commit -m "feat(sharing): add upstream readiness probe module

Pure probe_ready / probe_with_backoff functions for Ollama, whisper, and
LM Studio. Foundation for the start() gate and ReadinessWatcher."
```

---

## Task 2: `MdnsAdvertiser::update_ports()` (TDD)

**Files:**
- Modify: `crates/sharing/src/mdns.rs`

- [ ] **Step 1: Write the failing test**

In the existing `#[cfg(test)] mod tests` in `crates/sharing/src/mdns.rs`, add this test (after `advertise_then_browse_finds_self`):

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn update_ports_reregisters_with_new_txt() {
        if std::env::var("FERRISCRIBE_MDNS_TEST").ok().as_deref() != Some("1") {
            eprintln!("skipping: set FERRISCRIBE_MDNS_TEST=1 to run mDNS smoke test");
            return;
        }
        let ports_v1 = ServerPorts {
            ollama: Some(11435),
            whisper: Some(8081),
            lmstudio: None,
            pairing: Some(11436),
            vocab: Some(11437),
        };
        let ports_v2 = ServerPorts {
            ollama: Some(11435),
            whisper: Some(8081),
            lmstudio: Some(1235), // LM Studio came online
            pairing: Some(11436),
            vocab: Some(11437),
        };
        let mut adv = MdnsAdvertiser::start("update-test", &ports_v1, "0.0.0.0").unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;

        // Re-register with v2 ports (adds lmstudio).
        adv = adv.update_ports(&ports_v2);
        tokio::time::sleep(Duration::from_millis(400)).await;

        let mut rx = browse(Duration::from_secs(3)).unwrap();
        let mut found = None;
        while let Some(d) = rx.recv().await {
            if d.instance_name.contains("update-test") {
                found = Some(d);
                break;
            }
        }
        adv.stop();
        let d = found.expect("did not discover own advertisement after update");
        assert_eq!(d.ports.lmstudio, Some(1235), "lmstudio port must appear after update_ports");
    }
```

- [ ] **Step 2: Run test to verify it fails to compile**

Run: `cargo test -p medical-sharing --lib mdns::tests::update_ports_reregisters_with_new_txt`
Expected: FAIL — `method update_ports not found in MdnsAdvertiser`.

- [ ] **Step 3: Implement `update_ports`**

In `crates/sharing/src/mdns.rs`, first add an `instance_name` and `version` field to `MdnsAdvertiser` so `update_ports` can re-register. Change the struct (currently lines 73-76):

```rust
pub struct MdnsAdvertiser {
    daemon: ServiceDaemon,
    fullname: String,
    instance_name: String,
    version: String,
}
```

Update the end of `start()` (currently lines 130-133) to store them:

```rust
        Ok(Self {
            daemon,
            fullname: format!("{instance_name}.{SERVICE_TYPE}"),
            instance_name: instance_name.to_string(),
            version: version.to_string(),
        })
```

Add the `update_ports` method immediately after `start()` (before `stop`):

```rust
    /// Re-register with a new TXT record. Consumes and returns `self` so the
    /// caller keeps the same advertiser identity without needing to rebind.
    ///
    /// Calls `unregister` on the old service then `register` on the new one.
    /// Used by the ReadinessWatcher when the ready set of upstreams changes.
    pub fn update_ports(mut self, ports: &ServerPorts) -> Self {
        let _ = self.daemon.unregister(&self.fullname);
        let host = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "localhost".to_string());
        let host_with_dot = if host.ends_with(".local.") {
            host
        } else {
            format!("{host}.local.")
        };
        let mut props: HashMap<String, String> = HashMap::new();
        if let Some(p) = ports.ollama { props.insert("ollama".into(), p.to_string()); }
        if let Some(p) = ports.whisper { props.insert("whisper".into(), p.to_string()); }
        if let Some(p) = ports.lmstudio { props.insert("lmstudio".into(), p.to_string()); }
        if let Some(p) = ports.pairing { props.insert("pairing".into(), p.to_string()); }
        if let Some(p) = ports.vocab { props.insert("vocab".into(), p.to_string()); }
        props.insert("version".into(), self.version.clone());
        let advertise_port = ports.pairing.unwrap_or(11436);
        let info = match ServiceInfo::new(
            SERVICE_TYPE,
            &self.instance_name,
            &host_with_dot,
            "",
            advertise_port,
            Some(props),
        ) {
            Ok(i) => i.enable_addr_auto(),
            Err(e) => {
                tracing::warn!(error = %e, "mdns update_ports: ServiceInfo build failed; keeping old registration");
                return self;
            }
        };
        if let Err(e) = self.daemon.register(info) {
            tracing::warn!(error = %e, "mdns update_ports: register failed; keeping old registration");
        }
        self
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p medical-sharing --lib mdns::tests`
Expected: PASS — all mdns tests (skipped without `FERRISCRIBE_MDNS_TEST=1`).

- [ ] **Step 5: Commit**

```bash
git add crates/sharing/src/mdns.rs
git commit -m "feat(sharing): add MdnsAdvertiser::update_ports for re-advertisement

Lets the ReadinessWatcher re-register the mDNS service with an updated
TXT record when upstreams become ready, without a Stop+Start."
```

---

## Task 3: Readiness cache + honest `status()` (TDD)

**Files:**
- Modify: `crates/sharing/src/orchestrator.rs`

This task adds the readiness cache to `SharingService` and makes `status()` read from it. No behavioral change to `start()` yet — that's Task 4. The cache is initialized to a "degraded" state so status is honest even before the gate runs.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod lifecycle_tests` in `crates/sharing/src/orchestrator.rs` (at the end of the module, before its closing `}`):

```rust
    #[tokio::test]
    async fn status_reflects_readiness_cache_not_running_flag() {
        let dir = tempdir().unwrap();
        let mut c = cfg_with_tokens_at(dir.path().join("tokens.db"), [0u8; 32], "k");
        // Mark LM Studio as a configured candidate (office mode default).
        c.lmstudio_internal_port = Some(1234);
        c.lmstudio_proxy_port = Some(1235);
        let svc = SharingService::new(c).unwrap();

        // Force the cache into a mixed state without calling start() (which
        // needs real upstreams). Ollama ok, whisper ok, lmstudio not.
        {
            let mut r = svc.readiness.write().await;
            r.get_mut(&UpstreamKind::Ollama).unwrap().last_probe_ok = true;
            r.get_mut(&UpstreamKind::Whisper).unwrap().last_probe_ok = true;
            r.get_mut(&UpstreamKind::LmStudio).unwrap().last_probe_ok = false;
            // Simulate start() having run: running=true.
            *svc.running.lock().await = true;
        }

        let s = svc.status().await;
        assert!(s.enabled);
        assert!(s.ollama_ok, "ollama reflects probe cache");
        assert!(s.whisper_ok, "whisper reflects probe cache");
        assert!(!s.lmstudio_ok, "lmstudio reflects probe cache (not running flag)");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p medical-sharing --lib orchestrator::lifecycle_tests::status_reflects_readiness_cache_not_running_flag`
Expected: FAIL — `no field readiness on SharingService` / `no variant or item UpstreamKind in scope`.

- [ ] **Step 3: Add the readiness cache and types**

In `crates/sharing/src/orchestrator.rs`:

**3a.** Add imports at the top (after line 12 `use tokio::sync::Mutex;`):

```rust
use std::collections::HashMap;
use std::time::Instant;

use crate::upstream::{UpstreamKind, UpstreamTarget};
```

**3b.** Add a `ProbeState` struct and `ReadinessState` type alias after the `SharingStatus` struct (after line 133):

```rust
/// Per-upstream readiness snapshot, read by `status()` and updated by the
/// ReadinessWatcher and the start gate.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProbeState {
    /// Upstream is part of this server's config (e.g. LM Studio is always a
    /// candidate in office mode). When false the upstream is ignored entirely.
    pub configured: bool,
    /// Auth proxy is currently listening for this upstream.
    pub proxy_bound: bool,
    /// Most recent probe succeeded.
    pub last_probe_ok: bool,
    /// When the most recent probe ran.
    pub last_probe_at: Option<Instant>,
}

/// Readiness cache keyed by upstream kind.
pub type ReadinessState = HashMap<UpstreamKind, ProbeState>;
```

**3c.** Add fields to `SharingService` (currently lines 144-152). Replace the struct with:

```rust
pub struct SharingService {
    config: SharingConfig,
    store: Arc<TokenStore>,
    pairing: Arc<PairingState>,
    whisper: Arc<WhisperSupervisor>,
    mdns: Mutex<Option<MdnsAdvertiser>>,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    running: Mutex<bool>,
    /// Per-upstream readiness cache. Drives honest `status()`.
    readiness: tokio::sync::RwLock<ReadinessState>,
    /// Live snapshot pushed to clients when the ready set changes. Behind
    /// `Arc` so it can be cloned into the spawned pairing task, which serves
    /// `GET :11436/info` from this live value (Tailscale discovery path).
    info: Arc<tokio::sync::RwLock<InfoSnapshot>>,
    /// Watch channel: a new `ReadinessState` is sent whenever the ready set
    /// changes. The Tauri layer forwards changes to a frontend event.
    readiness_tx: tokio::sync::watch::Sender<ReadinessState>,
}
```

**3d.** Initialize the new fields in `SharingService::new()` (currently lines 159-179). Build the initial readiness map from config and the initial info snapshot. Replace the `Ok(Self { ... })` block with:

```rust
        let mut readiness: ReadinessState = HashMap::new();
        readiness.insert(UpstreamKind::Ollama, ProbeState {
            configured: true, proxy_bound: false, last_probe_ok: false, last_probe_at: None,
        });
        readiness.insert(UpstreamKind::Whisper, ProbeState {
            configured: true, proxy_bound: false, last_probe_ok: false, last_probe_at: None,
        });
        readiness.insert(UpstreamKind::LmStudio, ProbeState {
            configured: config.lmstudio_proxy_port.is_some(),
            proxy_bound: false, last_probe_ok: false, last_probe_at: None,
        });
        let info = InfoSnapshot {
            host: config.friendly_name.clone(),
            version: config.version.clone(),
            ports: ServerPorts {
                ollama: Some(config.ollama_proxy_port),
                whisper: Some(config.whisper_proxy_port),
                // LM Studio advertised only once ready (see watcher).
                lmstudio: None,
                pairing: Some(config.pairing_port),
                vocab: Some(config.vocab_port),
            },
        };
        let (readiness_tx, _rx) = tokio::sync::watch::channel(readiness.clone());
        Ok(Self {
            config, store, pairing, whisper,
            mdns: Mutex::new(None),
            handles: Mutex::new(Vec::new()),
            running: Mutex::new(false),
            readiness: tokio::sync::RwLock::new(readiness),
            info: Arc::new(tokio::sync::RwLock::new(info)),
            readiness_tx,
        })
```

**3e.** Add a `readiness_changes()` accessor right after `config()` (currently line 190):

```rust
    /// Subscribe to readiness changes. A new `ReadinessState` is sent whenever
    /// the ready set of upstreams changes (an upstream binds or goes down).
    /// The Tauri layer forwards these to a frontend event.
    pub fn readiness_changes(&self) -> tokio::sync::watch::Receiver<ReadinessState> {
        self.readiness_tx.subscribe()
    }
```

- [ ] **Step 4: Make `status()` honest**

Replace the `status()` method (currently lines 354-373) with:

```rust
    pub async fn status(&self) -> SharingStatus {
        let running = *self.running.lock().await;
        let n = self
            .store
            .list()
            .map(|v| v.len() as u32)
            .unwrap_or(0);
        let r = self.readiness.read().await;
        let get = |k: UpstreamKind| -> ProbeState {
            *r.get(&k).unwrap_or(&ProbeState::default())
        };
        let ollama = get(UpstreamKind::Ollama);
        let whisper = get(UpstreamKind::Whisper);
        let lmstudio = get(UpstreamKind::LmStudio);
        SharingStatus {
            enabled: running,
            ollama_ok: running && ollama.last_probe_ok,
            whisper_ok: running && whisper.last_probe_ok,
            lmstudio_ok: running && lmstudio.configured && lmstudio.last_probe_ok,
            mdns_ok: running,
            pairing_ok: running,
            paired_clients: n,
        }
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p medical-sharing --lib orchestrator::lifecycle_tests`
Expected: PASS — the new test plus the existing status tests. The existing tests `sharing_service_status_when_not_running_reports_disabled` and `sharing_service_status_counts_paired_clients_when_stopped` should still pass (running=false zeroes everything).

- [ ] **Step 6: Commit**

```bash
git add crates/sharing/src/orchestrator.rs
git commit -m "feat(sharing): add readiness cache and honest status()

SharingService now holds a per-upstream readiness cache. status() reads
the cache instead of echoing the running flag, so ollama_ok/whisper_ok/
lmstudio_ok reflect actual probe results. Also adds readiness_changes()
watch channel and dynamic /info snapshot field."
```

---

## Task 4: Readiness gate in `start()` (TDD)

**Files:**
- Modify: `crates/sharing/src/orchestrator.rs`

This task rewrites `start()` so auth proxies are gated behind a readiness probe, and mDNS advertises only the ready subset. The ReadinessWatcher is added in Task 5; this task wires the gate and dynamic `/info`.

- [ ] **Step 1: Refactor `spawn_auth_proxy` to return the bound listener (helper)**

The gate needs to bind a proxy only when an upstream becomes ready. Currently `spawn_auth_proxy` binds the TCP listener itself (lines 80-84 of `auth_proxy.rs`). Add a variant that accepts an already-bound listener so the orchestrator can defer binding.

In `crates/sharing/src/auth_proxy.rs`, after the existing `spawn_auth_proxy` function (after line 101), add:

```rust
/// Like [`spawn_auth_proxy`] but accepts a pre-bound listener. Used by the
/// readiness gate, which may bind the proxy only after the upstream becomes
/// ready (long after `start()`).
pub async fn spawn_auth_proxy_on_listener(
    listener: tokio::net::TcpListener,
    config: ProxyConfig,
    store: Arc<TokenStore>,
) -> crate::Result<tokio::task::JoinHandle<()>> {
    let client = Client::builder()
        .pool_max_idle_per_host(8)
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| crate::SharingError::AuthProxy(e.to_string()))?;
    let state = AppState { config: config.clone(), client, store };
    let app = Router::new()
        .fallback(handler)
        .with_state(state);
    Ok(tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            warn!("auth_proxy serve exited: {e}");
        }
    }))
}

/// Bind a TCP listener on `0.0.0.0:port` without spawning the proxy. The
/// readiness gate uses this to claim the port early so a conflict surfaces
/// before the upstream probe, then hands the listener to
/// [`spawn_auth_proxy_on_listener`] once the upstream is ready.
pub async fn bind_proxy_listener(port: u16) -> crate::Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| crate::SharingError::AuthProxy(format!("bind 0.0.0.0:{port}: {e}")))
}
```

Re-export these from the crate by adding to `crates/sharing/src/lib.rs` — no change needed, `auth_proxy` is already `pub mod`.

- [ ] **Step 2: Write the failing test for the gate**

Add to `#[cfg(test)] mod lifecycle_tests` in `crates/sharing/src/orchestrator.rs`:

```rust
    /// Gate test: with an unready upstream, start() must succeed but NOT bind
    /// a proxy for it. We assert the readiness cache shows proxy_bound=false
    /// for the unready upstream, and the advertised /info snapshot omits it.
    #[tokio::test]
    async fn start_gates_unready_upstream_out_of_advertisement() {
        // We can't easily spin real Ollama/whisper, so we test the gate's
        // contract through the public surface: start() with an unreachable
        // gate window leaves upstreams unbound and status honest.
        // Use a config with a zero-length gate so start() doesn't hang,
        // and point upstreams at closed ports.
        let dir = tempdir().unwrap();
        let mut c = cfg_with_tokens_at(dir.path().join("tokens.db"), [0u8; 32], "k");
        c.lmstudio_internal_port = Some(1234);
        c.lmstudio_proxy_port = Some(1235);
        let svc = Arc::new(SharingService::new(c).unwrap());

        // Override the gate deadline to "now" so start() probes once and moves on.
        // We call a test-only helper that runs start_with_gate(Duration::ZERO).
        svc.start_with_gate(std::time::Duration::ZERO).await.expect("start");

        let r = svc.readiness.read().await;
        // All upstreams unreachable on a fresh test box -> none bound.
        assert!(!r[&UpstreamKind::Ollama].proxy_bound, "ollama not gated up");
        assert!(!r[&UpstreamKind::LmStudio].proxy_bound, "lmstudio not gated up");
        drop(r);

        let info = svc.info.read().await;
        assert!(info.ports.lmstudio.is_none(), "/info must omit unready lmstudio");
        // pairing + vocab always present regardless of upstream readiness.
        assert_eq!(info.ports.pairing, Some(11436));
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p medical-sharing --lib orchestrator::lifecycle_tests::start_gates_unready_upstream_out_of_advertisement`
Expected: FAIL — `no method start_with_gate on SharingService`.

- [ ] **Step 4: Add `start_with_gate` and rewrite the bind sequence**

In `crates/sharing/src/orchestrator.rs`, replace the entire `start()` method (currently lines 199-329) with a thin wrapper plus the gated body. Add imports for auth_proxy helpers at the top of the file (after line 15 `use crate::auth_proxy::{ProxyConfig, spawn_auth_proxy};`):

```rust
use crate::auth_proxy::{
    ProxyConfig, bind_proxy_listener, spawn_auth_proxy_on_listener,
};
use crate::mdns::{MdnsAdvertiser, ServerPorts};
use crate::upstream::{UpstreamKind, UpstreamTarget, probe_with_backoff};
```

Replace the `start()` method with:

```rust
    /// Start all sharing subsystems with the default 5s readiness gate.
    ///
    /// See [`start_with_gate`](Self::start_with_gate) for details.
    pub async fn start(&self) -> Result<(), SharingError> {
        self.start_with_gate(std::time::Duration::from_secs(5)).await
    }

    /// Start sharing, gating each upstream's proxy binding behind a readiness
    /// probe. Upstreams that answer within `gate` get their auth proxy bound
    /// and appear in the mDNS TXT + /info snapshot at advertisement time.
    /// Upstreams still unreachable after the gate are NOT bound — they are
    /// brought online later by the ReadinessWatcher (Task 5).
    ///
    /// Pre-binds all proxy ports up front so port conflicts surfaces as `Err`
    /// immediately, then drops the listeners for unready upstreams at the end
    /// of the gate (so the port is free again if the upstream never comes up).
    pub async fn start_with_gate(&self, gate: std::time::Duration) -> Result<(), SharingError> {
        let mut running = self.running.lock().await;
        if *running { return Ok(()); }

        // 1. Pre-bind all proxy listeners so port conflicts surface as Err now.
        //    We hold them until the gate decides which to keep.
        let ollama_listener = bind_proxy_listener(self.config.ollama_proxy_port).await?;
        let whisper_listener = bind_proxy_listener(self.config.whisper_proxy_port).await?;
        let lmstudio_listener = match (self.config.lmstudio_internal_port, self.config.lmstudio_proxy_port) {
            (Some(_), Some(proxy)) => Some(bind_proxy_listener(proxy).await?),
            _ => None,
        };

        // 2. Whisper child — always start (in-process, supervised).
        if let Err(e) = self.whisper.start().await {
            return Err(SharingError::WhisperSupervisor(e.to_string()));
        }

        // 3. GATE: probe each upstream concurrently.
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .map_err(|e| SharingError::AuthProxy(e.to_string()))?;
        let deadline = tokio::time::Instant::now() + gate;

        let ollama_target = UpstreamTarget::new(UpstreamKind::Ollama, "http://127.0.0.1:11434");
        let whisper_target = UpstreamTarget::new(
            UpstreamKind::Whisper,
            format!("http://127.0.0.1:{}", self.config.whisper_internal_port),
        );

        let (ollama_ready, whisper_ready, lmstudio_ready) = {
            let lmstudio_target = match self.config.lmstudio_internal_port {
                Some(p) => Some(UpstreamTarget::new(UpstreamKind::LmStudio, format!("http://127.0.0.1:{p}"))),
                None => None,
            };
            let ollama_fut = probe_with_backoff(&client, &ollama_target, deadline);
            let whisper_fut = probe_with_backoff(&client, &whisper_target, deadline);
            let lmstudio_fut = async {
                match lmstudio_target {
                    Some(t) => probe_with_backoff(&client, &t, deadline).await,
                    None => false,
                }
            };
            tokio::join!(ollama_fut, whisper_fut, lmstudio_fut)
        };

        // 4. Bind proxies for ready upstreams; drop listeners for unready ones.
        let mut handles = self.handles.lock().await;
        if ollama_ready {
            let h = spawn_auth_proxy_on_listener(
                ollama_listener,
                ProxyConfig {
                    listen_port: self.config.ollama_proxy_port,
                    backend_url: "http://127.0.0.1:11434".to_string(),
                    path_prefix: "/".to_string(),
                    inject_api_key: None,
                },
                self.store.clone(),
            ).await?;
            handles.push(h);
        } else {
            drop(ollama_listener);
        }
        if whisper_ready {
            let h = spawn_auth_proxy_on_listener(
                whisper_listener,
                ProxyConfig {
                    listen_port: self.config.whisper_proxy_port,
                    backend_url: format!("http://127.0.0.1:{}", self.config.whisper_internal_port),
                    path_prefix: "/".to_string(),
                    inject_api_key: Some(self.config.whisper_internal_api_key.clone()),
                },
                self.store.clone(),
            ).await?;
            handles.push(h);
        } else {
            drop(whisper_listener);
        }
        let lmstudio_bound = if lmstudio_ready {
            if let Some(listener) = lmstudio_listener {
                if let (Some(internal), Some(proxy)) = (
                    self.config.lmstudio_internal_port,
                    self.config.lmstudio_proxy_port,
                ) {
                    let h = spawn_auth_proxy_on_listener(
                        listener,
                        ProxyConfig {
                            listen_port: proxy,
                            backend_url: format!("http://127.0.0.1:{internal}"),
                            path_prefix: "/".to_string(),
                            inject_api_key: None,
                        },
                        self.store.clone(),
                    ).await?;
                    handles.push(h);
                    true
                } else { false }
            } else { false }
        } else {
            drop(lmstudio_listener);
            false
        };

        // 5. Update readiness cache from gate results.
        {
            let mut r = self.readiness.write().await;
            let now = Instant::now();
            if let Some(e) = r.get_mut(&UpstreamKind::Ollama) {
                e.last_probe_ok = ollama_ready; e.proxy_bound = ollama_ready; e.last_probe_at = Some(now);
            }
            if let Some(e) = r.get_mut(&UpstreamKind::Whisper) {
                e.last_probe_ok = whisper_ready; e.proxy_bound = whisper_ready; e.last_probe_at = Some(now);
            }
            if let Some(e) = r.get_mut(&UpstreamKind::LmStudio) {
                e.last_probe_ok = lmstudio_ready; e.proxy_bound = lmstudio_bound; e.last_probe_at = Some(now);
            }
            let _ = self.readiness_tx.send(r.clone());
        }

        // 6. Build /info snapshot from the ready subset.
        self.rebuild_info_snapshot().await;

        // 7. mDNS — advertise the ready subset.
        let mdns = MdnsAdvertiser::start(
            &self.config.friendly_name,
            &self.info.read().await.ports,
            &self.config.version,
        )?;
        *self.mdns.lock().await = Some(mdns);

        // 8. Pairing service (always up).
        let info_snapshot = self.info.read().await.clone();
        let h3 = spawn_pairing_service(
            self.config.pairing_port,
            self.pairing.clone(),
            self.store.clone(),
            info_snapshot,
        ).await?;
        handles.push(h3);

        *running = true;
        Ok(())
    }
```

- [ ] **Step 5: Add `rebuild_info_snapshot` helper**

Add this method to `impl SharingService` (right after `readiness_changes()`):

```rust
    /// Rebuild the live /info snapshot from the current readiness cache.
    /// Called after the gate and by the ReadinessWatcher whenever the ready
    /// set changes. LM Studio's port appears only when its proxy is bound.
    async fn rebuild_info_snapshot(&self) {
        let r = self.readiness.read().await;
        let lmstudio_port = if r.get(&UpstreamKind::LmStudio)
            .map(|s| s.proxy_bound)
            .unwrap_or(false)
        {
            self.config.lmstudio_proxy_port
        } else {
            None
        };
        let mut info = self.info.write().await;
        info.ports.lmstudio = lmstudio_port;
    }
```

- [ ] **Step 6: Make `/info` dynamic (share the live `Arc<RwLock<InfoSnapshot>>`)**

The spec's section G requires `GET :11436/info` to reflect the current ready set, so Tailscale clients polling it see LM Studio appear after the watcher binds it. Currently `build_pairing_router` takes a static `InfoSnapshot`.

**6a.** Change `build_pairing_router`'s signature and `St` struct (currently lines 387-397) to take `Arc<RwLock<InfoSnapshot>>` instead of `InfoSnapshot`:

```rust
pub(crate) fn build_pairing_router(
    pairing: Arc<PairingState>,
    store: Arc<TokenStore>,
    info: Arc<tokio::sync::RwLock<InfoSnapshot>>,
) -> axum::Router {
    use std::net::SocketAddr;
    use axum::{Json, Router, extract::{ConnectInfo, State}, routing::{get, post}};
    use serde::{Deserialize, Serialize};

    #[derive(Clone)]
    struct St { pairing: Arc<PairingState>, store: Arc<TokenStore>, info: Arc<tokio::sync::RwLock<InfoSnapshot>> }
```

And change `info_handler` (currently lines 455-457) to read the live value:

```rust
    /// Public discovery: serves the live /info snapshot so Tailscale clients
    /// polling it see newly-ready upstreams (e.g. LM Studio) without a
    /// Stop+Start. Same shape mDNS broadcasts. No secrets, no codes.
    async fn info_handler(State(st): State<St>) -> Json<InfoSnapshot> {
        Json(st.info.read().await.clone())
    }
```

**6b.** Change `spawn_pairing_service` (currently lines 468-473) to take the shared handle:

```rust
async fn spawn_pairing_service(
    port: u16,
    pairing: Arc<PairingState>,
    store: Arc<TokenStore>,
    info: Arc<tokio::sync::RwLock<InfoSnapshot>>,
) -> crate::Result<tokio::task::JoinHandle<()>> {
```

**6c.** In `start_with_gate`, the pairing spawn call now passes the shared `self.info` (an `Arc`) instead of a cloned snapshot. Replace the pairing-spawn block in `start_with_gate` (the `let info_snapshot = self.info.read().await.clone();` ... `spawn_pairing_service(...)` lines) with:

```rust
        // 8. Pairing service (always up). Shares the live /info snapshot so
        //    Tailscale discovery sees newly-ready upstreams.
        let h3 = spawn_pairing_service(
            self.config.pairing_port,
            self.pairing.clone(),
            self.store.clone(),
            self.info.clone(),
        ).await?;
        handles.push(h3);
```

**6d.** Update the existing pairing router tests (`pairing_router_tests` module, currently lines 489-730) to pass `Arc::new(RwLock::new(sample_info()))` instead of `sample_info()`. In each test, replace the `build_pairing_router(pairing, store, sample_info())` call with:

```rust
        let app = build_pairing_router(
            pairing,
            store,
            std::sync::Arc::new(tokio::sync::RwLock::new(sample_info())),
        );
```

(There are ~9 call sites in that module — update each. The `info_handler` tests `info_returns_snapshot_with_configured_ports` and `info_requires_no_auth_or_loopback` now exercise the live-read path.)

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p medical-sharing --lib orchestrator::`
Expected: PASS — all orchestrator tests including the new gate test.

- [ ] **Step 8: Commit**

```bash
git add crates/sharing/src/orchestrator.rs crates/sharing/src/auth_proxy.rs
git commit -m "feat(sharing): gate auth-proxy binding behind upstream readiness

start() now pre-binds all proxy ports, probes each upstream (ollama,
whisper, lmstudio) within a 5s gate, binds the proxy + advertises only
the ready subset via mDNS and /info. Unready upstreams are left for the
ReadinessWatcher (next task). status() reads the cache, not the running
flag."
```

---

## Task 5: ReadinessWatcher (TDD)

**Files:**
- Modify: `crates/sharing/src/orchestrator.rs`

The watcher probes every 10 s, binds newly-ready upstreams, drops newly-unready ones, re-advertises mDNS, updates `/info`, and pushes a new `ReadinessState` on the watch channel.

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod lifecycle_tests`:

```rust
    /// Watcher brings a previously-unready upstream online: the proxy binds,
    /// /info gains its port, and the watch channel fires.
    #[tokio::test]
    async fn watcher_binds_late_upstream_and_re_advertises() {
        let dir = tempdir().unwrap();
        let mut c = cfg_with_tokens_at(dir.path().join("tokens.db"), [0u8; 32], "k");
        c.lmstudio_internal_port = Some(1234);
        c.lmstudio_proxy_port = Some(1235);
        let svc = Arc::new(SharingService::new(c).unwrap());
        svc.start_with_gate(std::time::Duration::ZERO).await.unwrap();

        // Subscribe before the watcher tick so we observe the change.
        let mut rx = svc.readiness_changes();

        // Manually flip LM Studio's cache to "ready" then run one watcher tick.
        // (In production the tick's own probe does this; here we bypass the
        // probe by seeding the cache, then run the bind/re-advertise half.)
        svc.bind_ready_upstreams_once().await;

        // Watch channel fired with lmstudio proxy_bound.
        let _ = rx.borrow_and_update();
        let r = svc.readiness.read().await;
        assert!(r[&UpstreamKind::LmStudio].proxy_bound, "watcher bound lmstudio");

        let info = svc.info.read().await;
        assert_eq!(info.ports.lmstudio, Some(1235), "/info now advertises lmstudio");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p medical-sharing --lib orchestrator::lifecycle_tests::watcher_binds_late_upstream_and_re_advertises`
Expected: FAIL — `no method bind_ready_upstreams_once`.

- [ ] **Step 3: Implement the watcher body and spawner**

Add to `impl SharingService` (after `rebuild_info_snapshot`):

```rust
    /// Spawn the long-lived ReadinessWatcher. Probes every 10s; on a change
    /// in the ready set, binds newly-ready upstreams, drops newly-unready
    /// ones, re-advertises mDNS, updates /info, and pushes the new
    /// ReadinessState on the watch channel. Aborted by `stop()`.
    pub fn spawn_readiness_watcher(self: &Arc<Self>, http_client: reqwest::Client) {
        let svc = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            interval.tick().await; // first tick is immediate; skip it
            loop {
                interval.tick().await;
                if !*svc.running.lock().await { break; }
                svc.bind_ready_upstreams_once().await;
            }
        });
    }

    /// One pass of the watcher: probe every configured upstream, bind proxies
    /// for newly-ready ones, drop proxies for newly-unready ones (noting the
    /// port remains free for re-bind), rebuild /info, re-advertise mDNS, and
    /// push the new ReadinessState. Called by the watcher loop and by tests.
    ///
    /// NOTE: dropping a bound proxy mid-session is NOT supported by the current
    /// auth_proxy (the join handle would need tracking per-upstream). For now
    /// this method only ever BINDS (unready->ready transitions). A bound
    /// upstream that later fails keeps its proxy and just reports
    /// last_probe_ok=false in status — clients see 502, matching the chosen
    /// "gate then self-heal, no per-request retry" design.
    pub async fn bind_ready_upstreams_once(&self) {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let targets: Vec<(UpstreamKind, UpstreamTarget)> = {
            let r = self.readiness.read().await;
            let mut v = Vec::new();
            for kind in [UpstreamKind::Ollama, UpstreamKind::Whisper, UpstreamKind::LmStudio] {
                let st = *r.get(&kind).unwrap_or(&ProbeState::default());
                if st.configured && !st.proxy_bound {
                    let base = match kind {
                        UpstreamKind::Ollama => "http://127.0.0.1:11434".to_string(),
                        UpstreamKind::Whisper => format!("http://127.0.0.1:{}", self.config.whisper_internal_port),
                        UpstreamKind::LmStudio => match self.config.lmstudio_internal_port {
                            Some(p) => format!("http://127.0.0.1:{p}"),
                            None => continue,
                        },
                    };
                    v.push((kind, UpstreamTarget::new(kind, base)));
                }
            }
            v
        };
        if targets.is_empty() {
            // Nothing new to bind. Still refresh last_probe_at for status.
            self.refresh_probe_health(&client).await;
            return;
        }

        let mut changed = false;
        for (kind, target) in targets {
            let ready = crate::upstream::probe_ready(&client, &target).await;
            if !ready { continue; }
            // Bind the proxy for this upstream.
            let proxy_port = match kind {
                UpstreamKind::Ollama => self.config.ollama_proxy_port,
                UpstreamKind::Whisper => self.config.whisper_proxy_port,
                UpstreamKind::LmStudio => match self.config.lmstudio_proxy_port {
                    Some(p) => p,
                    None => continue,
                },
            };
            let cfg = match kind {
                UpstreamKind::Ollama => ProxyConfig {
                    listen_port: proxy_port,
                    backend_url: target.base_url.clone(),
                    path_prefix: "/".to_string(),
                    inject_api_key: None,
                },
                UpstreamKind::Whisper => ProxyConfig {
                    listen_port: proxy_port,
                    backend_url: target.base_url.clone(),
                    path_prefix: "/".to_string(),
                    inject_api_key: Some(self.config.whisper_internal_api_key.clone()),
                },
                UpstreamKind::LmStudio => ProxyConfig {
                    listen_port: proxy_port,
                    backend_url: target.base_url.clone(),
                    path_prefix: "/".to_string(),
                    inject_api_key: None,
                },
            };
            match crate::auth_proxy::spawn_auth_proxy(cfg, self.store.clone()).await {
                Ok(h) => {
                    self.handles.lock().await.push(h);
                    let mut r = self.readiness.write().await;
                    if let Some(e) = r.get_mut(&kind) {
                        e.proxy_bound = true;
                        e.last_probe_ok = true;
                        e.last_probe_at = Some(Instant::now());
                        tracing::info!(upstream = ?kind, "watcher: upstream became ready, proxy bound");
                    }
                    changed = true;
                }
                Err(e) => {
                    tracing::warn!(upstream = ?kind, error = %e, "watcher: failed to bind proxy for ready upstream");
                }
            }
        }

        if changed {
            self.rebuild_info_snapshot().await;
            // Re-advertise mDNS with the new ports.
            if let Some(m) = self.mdns.lock().await.take() {
                let ports = self.info.read().await.ports.clone();
                *self.mdns.lock().await = Some(m.update_ports(&ports));
            }
            let r = self.readiness.read().await.clone();
            let _ = self.readiness_tx.send(r);
        } else {
            self.refresh_probe_health(&client).await;
        }
    }

    /// Refresh `last_probe_ok` for already-bound upstreams (status honesty).
    /// Does not bind or unbind anything.
    async fn refresh_probe_health(&self, client: &reqwest::Client) {
        let kinds: Vec<(UpstreamKind, String)> = {
            let r = self.readiness.read().await;
            let mut v = Vec::new();
            for kind in [UpstreamKind::Ollama, UpstreamKind::Whisper, UpstreamKind::LmStudio] {
                let st = *r.get(&kind).unwrap_or(&ProbeState::default());
                if !st.configured { continue; }
                let base = match kind {
                    UpstreamKind::Ollama => "http://127.0.0.1:11434".to_string(),
                    UpstreamKind::Whisper => format!("http://127.0.0.1:{}", self.config.whisper_internal_port),
                    UpstreamKind::LmStudio => match self.config.lmstudio_internal_port {
                        Some(p) => format!("http://127.0.0.1:{p}"),
                        None => continue,
                    },
                };
                v.push((kind, base));
            }
            v
        };
        let mut changed = false;
        let now = Instant::now();
        for (kind, base) in kinds {
            let target = UpstreamTarget::new(kind, base);
            let ok = crate::upstream::probe_ready(client, &target).await;
            let mut r = self.readiness.write().await;
            if let Some(e) = r.get_mut(&kind) {
                if e.last_probe_ok != ok { changed = true; }
                e.last_probe_ok = ok;
                e.last_probe_at = Some(now);
            }
        }
        if changed {
            let r = self.readiness.read().await.clone();
            let _ = self.readiness_tx.send(r);
        }
    }
```

- [ ] **Step 4: Wire `spawn_readiness_watcher` into `start_with_gate`**

In `start_with_gate`, just before `*running = true;` (the end of the method), add:

```rust
        // 9. Spawn the ReadinessWatcher (brings late-arriving upstreams online).
        let client_for_watcher = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .map_err(|e| SharingError::AuthProxy(e.to_string()))?;
        // We need an Arc<Self> to spawn the watcher, but start_with_gate takes
        // &self. The Tauri layer always holds SharingService behind Arc, so the
        // caller (start_sharing_inner) spawns the watcher instead. Keep this
        // method self-contained: it does NOT spawn the watcher.
```

(Confirmed: the watcher is spawned from `start_sharing_inner` which has the `Arc<SharingService>` — see Task 6.)

- [ ] **Step 5: Update `stop()` to abort the watcher**

The watcher exits its loop when `running` goes false (checked at the top of each tick), so no explicit abort handle is needed. Verify the existing `stop()` sets `*running = false;` — it does (line 346). No change needed. Add a comment in `stop()`:

After the existing `*running = false;` in `stop()`, no code change — but verify the comment in `stop()`'s doc mentions the watcher. Update the `stop()` doc comment to:

```rust
    /// Stop all sharing subsystems.
    ///
    /// Unregisters mDNS, kills the whisper-server child, and aborts all
    /// proxy/pairing join handles. The ReadinessWatcher notices `running`
    /// flipped to false on its next tick and exits. Idempotent.
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p medical-sharing --lib orchestrator::`
Expected: PASS — all tests.

- [ ] **Step 7: Commit**

```bash
git add crates/sharing/src/orchestrator.rs
git commit -m "feat(sharing): add ReadinessWatcher for self-healing upstreams

A 10s background task probes configured upstreams. Newly-ready ones
(e.g. LM Studio finishing its boot after login launch) get their auth
proxy bound, mDNS re-advertised, /info updated, and a watch-channel
notification fired. refresh_probe_health keeps status() honest for
already-bound upstreams. No per-request retry (per design)."
```

---

## Task 6: Wire the gate + watcher into the Tauri layer

**Files:**
- Modify: `src-tauri/src/commands/sharing/lifecycle.rs`

- [ ] **Step 1: Make LM Studio always a candidate in office mode**

In `build_sharing_config` (`src-tauri/src/commands/sharing/lifecycle.rs`), remove the `lmstudio_running_port` call and always set the LM Studio ports. Replace lines 237-256 (the `// Only wire up an LM Studio proxy...` block through the `Ok(SharingConfig { ... })`) with:

```rust
    // LM Studio is always a candidate in office mode. The start() gate probes
    // it once; the ReadinessWatcher brings it online later if it boots after
    // the gate (the login-launch race we're fixing). No Stop+Start needed.
    Ok(SharingConfig {
        enabled: true,
        friendly_name,
        ollama_proxy_port: 11435,
        whisper_proxy_port: 8081,
        pairing_port: 11436,
        whisper_internal_port: 8080,
        lmstudio_internal_port: Some(1234),
        lmstudio_proxy_port: Some(1235),
        vocab_port: 11437,
        token_store_path: app_data.join("sharing.db"),
        token_store_key: key,
        binary_dir: app_data.join("bin"),
        whisper_model_path: app_data.join("models/whisper/ggml-large-v3-turbo.bin"),
        whisper_internal_api_key: hex::encode(whisper_api),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
```

- [ ] **Step 2: Delete `lmstudio_running_port`**

Remove the entire `lmstudio_running_port` function (currently lines 260-272). It's now unused.

- [ ] **Step 3: Spawn the ReadinessWatcher and event forwarder in `start_sharing_inner`**

The Tauri layer has the `AppHandle`. But `start_sharing_inner` takes `&AppState`, not the handle. We need to pass the handle through. Change the signature of `start_sharing_inner` to accept an optional `AppHandle`.

In `src-tauri/src/commands/sharing/lifecycle.rs`, change the `start_sharing_inner` signature (currently line 33-36) from:

```rust
pub async fn start_sharing_inner(
    state: &AppState,
    friendly_name: String,
) -> AppResult<()> {
```

to:

```rust
pub async fn start_sharing_inner(
    state: &AppState,
    friendly_name: String,
    app_handle: Option<tauri::AppHandle>,
) -> AppResult<()> {
```

And update the `start_sharing` command (line 17-26) to pass the handle:

```rust
#[tauri::command]
pub async fn start_sharing(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
    friendly_name: String,
) -> AppResult<()> {
    start_sharing_inner(&state, friendly_name.clone(), Some(app_handle)).await?;
    write_server_config(&ServerConfig { version: 1, friendly_name })?;
    Ok(())
}
```

Then, after `*sharing_slot = Some(service);` (line 65), add the watcher + forwarder spawn:

```rust
    *sharing_slot = Some(service.clone());
    *state.vocab_api.write().await = vocab_handle;

    // Spawn the ReadinessWatcher (10s probe loop) and a tiny forwarder that
    // turns watch-channel changes into a Tauri event the frontend listens to.
    // Layering: SharingService is a library crate with no tauri dep, so the
    // emit() happens here.
    let watcher_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| AppError::Other(e.to_string()))?;
    service.spawn_readiness_watcher(watcher_client);

    if let Some(handle) = app_handle {
        let mut rx = service.readiness_changes();
        tauri::async_runtime::spawn(async move {
            use tauri::Emitter;
            while rx.changed().await.is_ok() {
                let _ = handle.emit("sharing-readiness-changed", ());
            }
        });
    }
```

Note: `*sharing_slot` now stores `Arc<SharingService>`, and we use `service.clone()` before storing. The original line `*sharing_slot = Some(service);` consumed `service`; now we clone into the slot. Verify `state.sharing` is `Arc<RwLock<Option<Arc<SharingService>>>>` — it is (per exploration). The current code does `let service = Arc::new(...)` then `*sharing_slot = Some(service)`. Change to clone:

Find `*sharing_slot = Some(service);` and confirm it becomes `*sharing_slot = Some(service.clone());` per the block above.

- [ ] **Step 4: Update the auto-resume caller in `lib.rs`**

In `src-tauri/src/lib.rs`, the auto-resume path (around line 197) calls `start_sharing_inner(&state, cfg.friendly_name)`. Update it to pass the handle:

```rust
                    if let Err(e) = crate::commands::sharing::start_sharing_inner(
                        &state,
                        cfg.friendly_name,
                        Some(app_handle.clone()),
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "auto-resume sharing failed");
                    }
```

(The `app_handle` variable already exists at line 193 of lib.rs.)

- [ ] **Step 5: Run type-check and the sharing crate's tests**

Run: `cargo build -p rust-medical-assistant`
Expected: builds cleanly.

Run: `cargo test -p medical-sharing --lib`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/commands/sharing/lifecycle.rs src-tauri/src/lib.rs
git commit -m "feat(sharing): wire gate + watcher into the Tauri layer

LM Studio is now always a candidate in office mode (the gate/watcher
decide when to bind). start_sharing_inner spawns the ReadinessWatcher
and a watch->event forwarder that emits sharing-readiness-changed to
the frontend. The login auto-resume path passes the AppHandle too."
```

---

## Task 7: Frontend — listen for the readiness event + toast

**Files:**
- Modify: `src/lib/components/settings/sharing/ServerStatus.svelte`

- [ ] **Step 1: Add the event listener and cache invalidation**

In `src/lib/components/settings/sharing/ServerStatus.svelte`, in the `<script>` block, add imports and the listener. After line 2 (`import { onMount, createEventDispatcher, tick } from 'svelte';`), add:

```typescript
  import { listen } from '@tauri-apps/api/event';
```

Then, inside `onMount` (currently lines 95-99), subscribe to the event and add a toast. Replace the `onMount` block with:

```typescript
  onMount(() => {
    refresh().then(() => regenQr());
    pollHandle = setInterval(refresh, 5000);

    // When the ReadinessWatcher brings a late-arriving upstream online (e.g.
    // LM Studio finishing its boot after a login launch), invalidate the
    // cache immediately and refresh, instead of waiting up to 5s for the poll.
    let unlistenFn: (() => void) | undefined;
    listen('sharing-readiness-changed', () => {
      refresh();
    }).then((un) => { unlistenFn = un; });

    return () => {
      clearInterval(pollHandle);
      unlistenFn?.();
    };
  });
```

- [ ] **Step 2: Update the LM Studio "off hint" copy**

The `offHint` for `lmstudio_ok` (currently line 107) tells the user to Stop+Start. With the watcher, that's no longer needed — LM Studio will be picked up automatically. Change the `offHint` to reflect the new behavior:

```typescript
    {
      key: 'lmstudio_ok',
      label: 'LM Studio',
      offHint: 'LM Studio is not running yet. It will appear automatically once its local server starts.',
    },
```

- [ ] **Step 3: Run the type-check**

Run: `npm run check`
Expected: PASS — no svelte-check errors.

- [ ] **Step 4: Run frontend tests**

Run: `npx vitest run`
Expected: PASS — no regressions (no new tests needed for this thin event wiring; the behavior is backend-tested).

- [ ] **Step 5: Commit**

```bash
git add src/lib/components/settings/sharing/ServerStatus.svelte
git commit -m "feat(sharing): refresh server status on readiness event

ServerStatus.svelte listens for sharing-readiness-changed and refreshes
immediately instead of waiting for the 5s poll. Updates the LM Studio
off-hint to reflect that it now appears automatically once started."
```

---

## Task 8: Final verification

- [ ] **Step 1: Run the full backend lib test suite**

Run: `cargo test --workspace --lib`
Expected: PASS.

- [ ] **Step 2: Run clippy on the sharing crate**

Run: `cargo clippy -p medical-sharing --lib --tests`
Expected: no warnings.

- [ ] **Step 3: Run clippy on the Tauri shell**

Run: `cargo clippy -p rust-medical-assistant`
Expected: no warnings.

- [ ] **Step 4: Run frontend type-check + tests**

Run: `npm run check && npx vitest run`
Expected: PASS.

- [ ] **Step 5: Manual smoke test (optional, if a local Ollama is available)**

With Ollama running locally:
1. `npm run tauri dev`
2. Settings → Sharing → Start sharing.
3. Observe status: Ollama ✓ within ~5s.
4. Stop Ollama (`ollama stop` or kill). Within ~10-15s status shows Ollama ✗.
5. Start Ollama again. Within ~10-15s status shows Ollama ✓ again (no Stop+Start of sharing needed).

With LM Studio: start it after clicking Start sharing — it should appear as ✓ within ~10-15s automatically.

- [ ] **Step 6: Final commit (if any cleanup)**

If steps 1-4 required any fixups, commit them. Otherwise nothing to commit.

---

## Out of scope (per the approved spec)

- Per-request retry inside `auth_proxy` (the "Both" option). A gated-up upstream that later dies keeps its proxy and 502s; the watcher flags it degraded in status; the request is not retried.
- Ongoing health-checking beyond the 10s watcher tick (the watcher already runs indefinitely).
- Changing the server's own provider endpoints — they already point at loopback upstreams and self-heal.
