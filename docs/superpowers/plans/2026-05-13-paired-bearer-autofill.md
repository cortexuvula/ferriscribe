# Paired-Bearer Auto-Fill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the client successfully pairs with an office server, mirror the bearer into the per-service keychain slots (`stt_remote_api_key`, `ollama_api_key`, `lmstudio_api_key`) and populate the matching `AppConfig` host/port/mode fields, so the Settings UI, pre-flight probe, and Phase 2 endpointHealth polling all see the paired endpoint instead of empty defaults. Pair flow → fully-configured client, no manual settings work required.

**Architecture:** Backend pair/unpair handlers gain settings-population logic; two `test_*_connection` Tauri commands gain an optional `api_key` parameter; the frontend `endpointHealth.ts` store extends Phase 2 Task 1's STT key-fetch pattern to Ollama and LM Studio; the frontend `ClientPair.svelte` chains a `settings.load()` after the pair invoke. Pure-function settings-mutation helpers are extracted to make the changes unit-testable without an OS keychain. No new schema, no new Tauri commands.

**Tech Stack:** Rust (Tauri 2, `keyring` crate, `medical_db::settings::SettingsRepo`, `state.keys: KeyStorage`), TypeScript (Svelte 5 runes, `@tauri-apps/api/core::invoke`), Vitest + wiremock for tests. No new dependencies.

**Spec:** [`docs/superpowers/specs/2026-05-13-paired-bearer-autofill-design.md`](../specs/2026-05-13-paired-bearer-autofill-design.md)

---

## File Structure

**New files:**
- (none — all changes modify existing files)

**Modified files:**
- `src-tauri/src/commands/sharing/pairing.rs` — `pair_with_server` writes per-service keychain slots + populates AppConfig; `unpair` clears them.
- `src-tauri/src/commands/sharing/settings_helpers.rs` *(new)* — pure helpers `apply_paired_settings(cfg, host, ports)` and `reset_paired_settings(cfg)` that mutate `AppConfig`. Lets pair/unpair logic be unit-tested without touching DB or keychain.
- `src-tauri/src/commands/sharing/mod.rs` — `pub mod settings_helpers;` (gated `#[cfg(test)]` or `pub(super)` depending on test placement).
- `src-tauri/src/commands/providers.rs` — `test_ollama_connection` and `test_lmstudio_connection` gain `api_key: Option<String>` parameter; send `Authorization: Bearer …` when present; 401 → "Authentication failed — re-pair" message.
- `src/lib/stores/endpointHealth.ts` — `probeAi` fetches `ollama_api_key` / `lmstudio_api_key` from keychain and forwards as `apiKey` to the test commands.
- `src/lib/stores/endpointHealth.test.ts` — two new tests for the AI api-key fetch + auth path.
- `src/lib/components/settings/sharing/ClientPair.svelte` — after a successful `pair_with_server` invoke, chain `settings.load()` so the reactive `$settings` store and the Settings UI reflect the new values.

**Worktree:** Use `superpowers:using-git-worktrees` to create `.worktrees/paired-bearer-autofill` from master before Task 1. The user is on `master` at `1caf79b` (Phase 3 spec commit).

---

## Task 1: Extract pure settings-mutation helpers

**Files:**
- Create: `src-tauri/src/commands/sharing/settings_helpers.rs`
- Modify: `src-tauri/src/commands/sharing/mod.rs`

**Why:** Doing this first lets every downstream task that needs the AppConfig mutation logic use the helpers with full unit-test coverage. The helpers don't touch DB or keychain — they take `&mut AppConfig` and just set fields. Easy to TDD.

- [ ] **Step 1: Add the helpers module to `mod.rs`**

Open `src-tauri/src/commands/sharing/mod.rs`. Append:

```rust
pub mod settings_helpers;
```

- [ ] **Step 2: Write the failing test file**

Create `src-tauri/src/commands/sharing/settings_helpers.rs` with stubs + tests:

```rust
//! Pure-function helpers that mutate AppConfig when pairing / unpairing.
//! Extracted so pair/unpair logic is unit-testable without DB or keychain.

use medical_core::types::settings::{AppConfig, SttMode};
use medical_sharing::PairPorts;

/// Apply the office server's resolved address + ports to AppConfig.
/// Preserves `cfg.ai_provider` — pair does NOT change which provider is active.
/// LM Studio fields are only touched when `ports.lmstudio` is Some.
pub fn apply_paired_settings(cfg: &mut AppConfig, host: &str, ports: &PairPorts) {
    cfg.stt_mode = SttMode::Remote;
    cfg.stt_remote_host = host.to_string();
    cfg.stt_remote_port = ports.whisper;
    cfg.ollama_host = host.to_string();
    cfg.ollama_port = ports.ollama;
    if let Some(lp) = ports.lmstudio {
        cfg.lmstudio_host = host.to_string();
        cfg.lmstudio_port = lp;
    }
}

/// Reset the AppConfig fields the pair flow populated, back to local defaults.
/// Preserves `cfg.ai_provider`.
pub fn reset_paired_settings(cfg: &mut AppConfig) {
    cfg.stt_mode = SttMode::Local;
    cfg.stt_remote_host = String::new();
    cfg.stt_remote_port = 8080;
    cfg.ollama_host = "localhost".into();
    cfg.ollama_port = 11434;
    cfg.lmstudio_host = "localhost".into();
    cfg.lmstudio_port = 1234;
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::settings::AppConfig;

    fn ports(lmstudio: Option<u16>) -> PairPorts {
        PairPorts {
            ollama: 11435,
            whisper: 8081,
            pairing: 11436,
            lmstudio,
            vocab: Some(11437),
        }
    }

    #[test]
    fn apply_paired_settings_populates_all_three_services_when_lmstudio_present() {
        let mut cfg = AppConfig::default();
        cfg.ai_provider = "lmstudio".into();
        apply_paired_settings(&mut cfg, "192.168.4.37", &ports(Some(1235)));

        assert_eq!(cfg.stt_mode, SttMode::Remote);
        assert_eq!(cfg.stt_remote_host, "192.168.4.37");
        assert_eq!(cfg.stt_remote_port, 8081);
        assert_eq!(cfg.ollama_host, "192.168.4.37");
        assert_eq!(cfg.ollama_port, 11435);
        assert_eq!(cfg.lmstudio_host, "192.168.4.37");
        assert_eq!(cfg.lmstudio_port, 1235);
        assert_eq!(cfg.ai_provider, "lmstudio", "ai_provider must be preserved");
    }

    #[test]
    fn apply_paired_settings_leaves_lmstudio_fields_alone_when_port_is_none() {
        let mut cfg = AppConfig::default();
        let original_lmstudio_host = cfg.lmstudio_host.clone();
        let original_lmstudio_port = cfg.lmstudio_port;

        apply_paired_settings(&mut cfg, "192.168.4.37", &ports(None));

        assert_eq!(cfg.lmstudio_host, original_lmstudio_host);
        assert_eq!(cfg.lmstudio_port, original_lmstudio_port);
        assert_eq!(cfg.stt_remote_host, "192.168.4.37");  // STT still applied
        assert_eq!(cfg.ollama_host, "192.168.4.37");      // Ollama still applied
    }

    #[test]
    fn apply_paired_settings_preserves_ai_provider() {
        let mut cfg = AppConfig::default();
        cfg.ai_provider = "ollama".into();
        apply_paired_settings(&mut cfg, "10.0.0.5", &ports(Some(1235)));
        assert_eq!(cfg.ai_provider, "ollama");

        let mut cfg2 = AppConfig::default();
        cfg2.ai_provider = "lmstudio".into();
        apply_paired_settings(&mut cfg2, "10.0.0.5", &ports(Some(1235)));
        assert_eq!(cfg2.ai_provider, "lmstudio");
    }

    #[test]
    fn reset_paired_settings_returns_to_local_defaults() {
        let mut cfg = AppConfig::default();
        cfg.ai_provider = "ollama".into();
        // Simulate a paired state.
        apply_paired_settings(&mut cfg, "192.168.4.37", &ports(Some(1235)));
        assert_eq!(cfg.stt_mode, SttMode::Remote);

        reset_paired_settings(&mut cfg);

        assert_eq!(cfg.stt_mode, SttMode::Local);
        assert_eq!(cfg.stt_remote_host, "");
        assert_eq!(cfg.stt_remote_port, 8080);
        assert_eq!(cfg.ollama_host, "localhost");
        assert_eq!(cfg.ollama_port, 11434);
        assert_eq!(cfg.lmstudio_host, "localhost");
        assert_eq!(cfg.lmstudio_port, 1234);
        assert_eq!(cfg.ai_provider, "ollama", "ai_provider must be preserved");
    }
}
```

- [ ] **Step 3: Build + run the new tests**

```bash
cargo build -p rust-medical-assistant 2>&1 | tail -3
cargo test -p rust-medical-assistant --lib commands::sharing::settings_helpers 2>&1 | tail -10
```

Expected: clean build; 4 tests pass.

If `PairPorts` isn't re-exported from `medical_sharing` at the crate root, find its path with `grep -n "pub struct PairPorts" crates/sharing/src/`. The current call sites use `medical_sharing::PairPorts` (see `pairing.rs:90`).

- [ ] **Step 4: Run the full Tauri-app suite for regression**

```bash
cargo test -p rust-medical-assistant --lib 2>&1 | tail -5
```

Expected: 61 + 4 = 65 tests pass (the existing 61 from Phase 1/2 plus the 4 new).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/sharing/settings_helpers.rs src-tauri/src/commands/sharing/mod.rs
git commit -m "feat(sharing): extract apply/reset paired-settings helpers

Pure-function helpers that mutate AppConfig for pair / unpair, with
4 unit tests covering: full apply, optional lmstudio absent, ai_provider
preservation across both apply and reset, and the round-trip to local
defaults. Foundation for Task 2 (pair handler) and Task 3 (unpair
handler).

Phase 3 of the server-down / paired-bearer effort (spec
docs/superpowers/specs/2026-05-13-paired-bearer-autofill-design.md)."
```

---

## Task 2: Pair handler — populate keychain + AppConfig

**Files:**
- Modify: `src-tauri/src/commands/sharing/pairing.rs` (the `pair_with_server` function, lines 87-209)

**Why:** The actual fix. After the existing in-memory provider update (which already works), write the bearer into the three per-service keychain slots and call the Task 1 helper to populate AppConfig. Then save the updated config. The downstream test commands and the frontend Settings UI will now see the paired endpoint.

- [ ] **Step 1: Add the new logic to `pair_with_server`**

In `src-tauri/src/commands/sharing/pairing.rs`, locate the closing brace of the STT block at line 206 (right before `Ok(())`). Insert the following BETWEEN line 206 (`}` ending the STT block) and line 208 (`Ok(())`):

```rust
    // ── Phase 3: per-service keychain mirror + AppConfig population ──
    //
    // The bearer above is stored at keyring "rustMedicalAssistant"/"sharing-bearer"
    // (used by the in-memory provider path). The rest of the app — Settings UI,
    // pre-flight, endpointHealth polling — reads from per-service keychain slots
    // and AppConfig host/port fields. Mirror the bearer here so paired clients
    // don't need to manually fill in Settings → Audio / Models.
    {
        use super::settings_helpers::apply_paired_settings;

        // 1. Pick the resolved host. Prefer LAN; fall back to Tailscale. The
        //    in-memory RemoteEndpoint will still try LAN-then-Tailscale at call
        //    time, but the static AppConfig field shows ONE address — LAN is
        //    more meaningful for the user reading the Settings UI than a
        //    Tailscale CGNAT address.
        let host = lan.clone()
            .or_else(|| tailscale.clone())
            .ok_or_else(|| "no reachable address for paired-settings autofill".to_string())?;

        // 2. Write the bearer to per-service keychain slots via state.keys.
        //    Same KeyStorage abstraction the set_api_key Tauri command uses.
        for slot in &["stt_remote_api_key", "ollama_api_key", "lmstudio_api_key"] {
            state.keys.store_key(slot, &token).map_err(|e| {
                format!("autofill: store {slot}: {e}")
            })?;
        }

        // 3. Update AppConfig with the paired endpoint values.
        let conn = state.db.conn().map_err(|e| e.to_string())?;
        let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| e.to_string())?;
        cfg.migrate();
        apply_paired_settings(&mut cfg, &host, &ports);
        medical_db::settings::SettingsRepo::save_config(&conn, &cfg)
            .map_err(|e| e.to_string())?;

        tracing::info!(
            host = %host,
            whisper_port = ports.whisper,
            ollama_port = ports.ollama,
            lmstudio_port = ?ports.lmstudio,
            "pair: populated per-service api_keys and AppConfig host/ports"
        );
    }
```

- [ ] **Step 2: Build**

```bash
cargo build -p rust-medical-assistant 2>&1 | tail -5
```

Expected: clean build. If `state.keys.store_key` complains, the field might be named differently — check `src-tauri/src/state.rs` for `pub keys:` and adjust.

- [ ] **Step 3: Test the test commands aren't broken by this change**

Existing tests don't directly cover `pair_with_server` (no Tauri-command unit tests existed before Phase 3). Run the full suite to confirm no regressions:

```bash
cargo test -p rust-medical-assistant --lib 2>&1 | tail -5
```

Expected: 65 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/sharing/pairing.rs
git commit -m "feat(sharing): pair_with_server populates per-service keychain + AppConfig

After the existing in-memory provider update, the pair handler now:
- Writes the bearer to keychain slots stt_remote_api_key,
  ollama_api_key, lmstudio_api_key (via state.keys.store_key).
- Calls apply_paired_settings to set stt_mode=Remote and populate
  host/port for STT / Ollama / LM Studio in AppConfig.
- Preserves cfg.ai_provider (user choice).

LAN address is preferred over Tailscale for the static AppConfig
field; the in-memory RemoteEndpoint still falls back to Tailscale
at request time.

Phase 3 Task 2."
```

---

## Task 3: Unpair handler — clear keychain + reset AppConfig

**Files:**
- Modify: `src-tauri/src/commands/sharing/pairing.rs` (the `unpair` function, lines 224-238)

**Why:** Symmetric to Task 2. When the user unpairs, the per-service api_keys must be deleted and the AppConfig fields reverted to local defaults. Otherwise stale paired state lingers and confuses subsequent runs.

- [ ] **Step 1: Modify `unpair`**

Replace the body of `unpair` (lines 224-238) with:

```rust
#[tauri::command]
pub async fn unpair(state: State<'_, AppState>) -> Result<(), String> {
    // Remove the sharing-bearer keychain entry (ignore NoEntry).
    if let Ok(entry) = keyring::Entry::new("rustMedicalAssistant", "sharing-bearer") {
        let _ = entry.delete_credential();
    }

    // Remove the metadata file (ignore not-found).
    let path = paired_connection_path()?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }

    // ── Phase 3: clear per-service keychain slots and reset AppConfig ──
    {
        use super::settings_helpers::reset_paired_settings;

        for slot in &["stt_remote_api_key", "ollama_api_key", "lmstudio_api_key"] {
            // Idempotent — ignore "not found" errors per the existing pattern.
            let _ = state.keys.remove_key(slot);
        }

        let conn = state.db.conn().map_err(|e| e.to_string())?;
        let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| e.to_string())?;
        cfg.migrate();
        reset_paired_settings(&mut cfg);
        medical_db::settings::SettingsRepo::save_config(&conn, &cfg)
            .map_err(|e| e.to_string())?;

        tracing::info!("unpair: cleared per-service api_keys and reset AppConfig");
    }

    Ok(())
}
```

The function signature changes from `unpair() -> Result<(), String>` to `unpair(state: State<'_, AppState>) -> Result<(), String>`. The frontend invoke call site doesn't change — Tauri auto-injects `State`.

- [ ] **Step 2: Build**

```bash
cargo build -p rust-medical-assistant 2>&1 | tail -5
```

Expected: clean. If `state.keys.remove_key` returns a different type than expected, check `crates/security/src/key_storage.rs:136` — it returns `SecurityResult<bool>` (bool indicating whether the entry existed). The `let _ =` swallows both the Result and the bool.

- [ ] **Step 3: Full test suite**

```bash
cargo test -p rust-medical-assistant --lib 2>&1 | tail -5
```

Expected: 65 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/sharing/pairing.rs
git commit -m "feat(sharing): unpair clears per-service keychain + resets AppConfig

Symmetric to Task 2. After the existing keychain/file cleanup, unpair
now:
- Removes stt_remote_api_key, ollama_api_key, lmstudio_api_key from
  KeyStorage (idempotent — missing entries are ignored).
- Calls reset_paired_settings to revert stt_mode=Local, clear remote
  hosts/ports back to defaults.
- Preserves cfg.ai_provider.

Adds State<AppState> to the function signature so it can reach
state.keys and state.db. Tauri auto-injects, callers unchanged.

Phase 3 Task 3."
```

---

## Task 4: `test_ollama_connection` + `test_lmstudio_connection` accept `api_key`

**Files:**
- Modify: `src-tauri/src/commands/providers.rs` (the two functions, around lines 81–135 and 216–265)

**Why:** Today the AI test commands have no `api_key` parameter — Phase 2 endpointHealth polling hits the auth proxy at ports 11435 / 1235 without a Bearer and gets 401. Same pattern Phase 1 already established for `test_stt_remote_connection`: optional `api_key` parameter; send Bearer when present; 401 → "Authentication failed" message.

- [ ] **Step 1: Add the failing test for `test_ollama_connection` Bearer forwarding**

Tests for the providers commands. Find any existing test module in `src-tauri/src/commands/providers.rs` (if absent, add one at the bottom). Add:

```rust
#[cfg(test)]
mod offline_tests {
    use super::*;
    use wiremock::matchers::{method, path, header};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Existing tests in this module (if any) stay. New tests below.

    #[tokio::test]
    async fn test_ollama_connection_sends_bearer_when_api_key_provided() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .and(header("authorization", "Bearer secret-token-xyz"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"models":[]}"#))
            .mount(&server)
            .await;

        // Construct a minimal AppState that owns an http_client. The existing
        // test infrastructure in src-tauri may have a helper for this; if not,
        // build one inline:
        let state = /* test AppState — see Phase 1's preflight_tests for the
                       build_test_app_state-style helper */;

        let url = server.uri();
        // The host:port the command expects:
        let parsed: url::Url = url.parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let result = test_ollama_connection(
            tauri::State::from(&state),  // adapt to actual State construction
            host,
            port,
            Some("secret-token-xyz".to_string()),
        ).await;

        assert!(result.is_ok(), "Bearer-authenticated probe should succeed; got {result:?}");
    }

    #[tokio::test]
    async fn test_ollama_connection_returns_auth_failed_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let state = /* same test AppState */;
        let parsed: url::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let result = test_ollama_connection(
            tauri::State::from(&state),
            host,
            port,
            None,
        ).await;

        let err = result.expect_err("401 must surface as Err");
        let msg = err.to_string();
        assert!(
            msg.contains("Authentication failed"),
            "expected user-friendly auth message; got: {msg}"
        );
    }

    // Repeat both tests for test_lmstudio_connection: same shape but
    // path("/v1/models") instead of path("/api/tags").
    #[tokio::test]
    async fn test_lmstudio_connection_sends_bearer_when_api_key_provided() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer secret-token-xyz"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"data":[]}"#))
            .mount(&server)
            .await;
        let state = /* same */;
        let parsed: url::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let result = test_lmstudio_connection(
            tauri::State::from(&state),
            host,
            port,
            Some("secret-token-xyz".to_string()),
        ).await;

        assert!(result.is_ok(), "Bearer-authenticated probe should succeed; got {result:?}");
    }

    #[tokio::test]
    async fn test_lmstudio_connection_returns_auth_failed_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let state = /* same */;
        let parsed: url::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let result = test_lmstudio_connection(
            tauri::State::from(&state),
            host,
            port,
            None,
        ).await;

        let err = result.expect_err("401 must surface as Err");
        assert!(err.to_string().contains("Authentication failed"));
    }
}
```

**Implementor note:** the placeholder `/* test AppState */` blocks need a concrete AppState constructor. Look at `src-tauri/src/commands/generation/test_helpers.rs::build_test_state_with_recording` (Phase 1) for the pattern. AppState construction for these test commands only needs `state.http_client` and `state.keys` — the rest can be defaults / mocks. If building a full AppState is too heavyweight for unit tests, refactor the inner request logic into a helper that takes `(host, port, api_key) -> Result<String, AppError>` and test that directly without the State.

- [ ] **Step 2: Run the tests to confirm they fail (function signature mismatch)**

```bash
cargo test -p rust-medical-assistant --lib commands::providers::offline_tests 2>&1 | tail -15
```

Expected: build error — `test_ollama_connection` and `test_lmstudio_connection` don't have the `api_key` parameter yet.

- [ ] **Step 3: Add the `api_key` parameter to `test_ollama_connection`**

Find `test_ollama_connection` in `src-tauri/src/commands/providers.rs` (around line 216). Modify the signature and the request building:

```rust
#[tauri::command]
pub async fn test_ollama_connection(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
    api_key: Option<String>,      // NEW
) -> AppResult<String> {
    let effective_host = if host.is_empty() { "localhost".to_string() } else { host };
    let url = format!("http://{}:{}/api/tags", effective_host, port);

    info!(url = %url, "Testing Ollama connection");

    let mut req = state.http_client
        .get(&url)
        .timeout(Duration::from_secs(5));
    if let Some(key) = api_key.as_deref().filter(|s| !s.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let response = req.send().await.map_err(|e| {
        // …existing classify_reqwest_error path (unchanged from Phase 1 Task 4)…
    })?;

    // NEW: 401/403 surface the same "Authentication failed" wording as STT.
    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(AppError::AiProvider(
            "Authentication failed \u{2014} verify the API key, or if this is a paired client, \
             re-pair the office server (Settings \u{2192} Sharing \u{2192} Unpair, then scan a fresh code)."
                .to_string(),
        ));
    }

    // …rest of existing code unchanged…
}
```

Insert the `api_key` parameter and the auth-header logic; insert the 401/403 check after `response` is bound but BEFORE the existing `if !response.status().is_success()` block (so 401/403 gets the friendly message instead of the generic "Server returned HTTP 401" path).

- [ ] **Step 4: Same change to `test_lmstudio_connection`**

Find `test_lmstudio_connection` (around line 81). Apply the identical shape: add `api_key: Option<String>` parameter; conditionally add `Authorization: Bearer …` header; add the 401/403 friendly-message branch.

- [ ] **Step 5: Run the tests; they should pass**

```bash
cargo test -p rust-medical-assistant --lib commands::providers::offline_tests 2>&1 | tail -15
```

Expected: 4 tests pass.

- [ ] **Step 6: Update existing callers of `test_ollama_connection` / `test_lmstudio_connection` to pass `api_key: None` (or the keychain value)**

The frontend Settings → Models "Test Connection" button currently invokes these commands. Find the caller:

```bash
grep -rn "test_ollama_connection\|test_lmstudio_connection" src/lib/ 2>&1 | head -10
```

For each caller (likely in `src/lib/components/settings/Models.svelte`), pass the api_key from keychain. The Settings → Models component already has the host/port from `$settings`. Add the api_key fetch:

```ts
// In the "Test Connection" handler for Ollama:
let apiKey: string | undefined = undefined;
try {
  const key = await invoke<string | null>('get_api_key', {
    provider: 'ollama_api_key',
  });
  if (key) apiKey = key;
} catch {
  // Keychain unavailable — try without auth.
}
const result = await invoke<string>('test_ollama_connection', {
  host: $settings.ollama_host,
  port: $settings.ollama_port,
  apiKey,
});
```

Same pattern for LM Studio with `lmstudio_api_key`.

If `Models.svelte` doesn't currently invoke these commands directly (e.g. it uses an API wrapper in `src/lib/api/`), update the wrapper instead. Let `npm run check` guide you to any callers that need updating — TypeScript will surface signature mismatches.

- [ ] **Step 7: Build + svelte-check + full backend tests**

```bash
cargo build -p rust-medical-assistant 2>&1 | tail -3
cargo test -p rust-medical-assistant --lib 2>&1 | tail -5
npm run check 2>&1 | tail -10
```

Expected: clean build; 69 backend tests pass (65 + 4 new); 0 svelte-check errors.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/commands/providers.rs src/lib/components/settings/Models.svelte
git commit -m "feat(providers): test_ollama/lmstudio_connection accept api_key

Mirrors what test_stt_remote_connection has had since Phase 1: optional
api_key parameter, Authorization: Bearer header when present, 401/403
surfaces the 'Authentication failed — re-pair' message instead of a
generic 'Server returned HTTP 401'.

Settings → Models's Test Connection buttons fetch the per-provider
api_key from keychain (ollama_api_key / lmstudio_api_key) before
invoking. 4 wiremock-backed tests cover Bearer forwarding + 401
handling for both providers.

Phase 3 Task 4."
```

---

## Task 5: `endpointHealth.ts` — fetch AI api_keys

**Files:**
- Modify: `src/lib/stores/endpointHealth.ts` (the `probeAi` function)
- Modify: `src/lib/stores/endpointHealth.test.ts` (add 2 new tests + update existing)

**Why:** Phase 2 Task 1 already added `get_api_key` fetch for the STT probe. Mirror that for the AI probe so the polling pill stops reporting offline when paired.

- [ ] **Step 1: Add failing tests for the AI api_key fetch**

Append to `src/lib/stores/endpointHealth.test.ts`:

```ts
  it('fetches ollama_api_key from keychain and forwards it to the Ollama probe', async () => {
    settings.set({
      ai_provider: 'ollama',
      lmstudio_host: '',
      lmstudio_port: 1234,
      ollama_host: '192.168.1.10',
      ollama_port: 11435,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_mode: 'local',
    } as any);

    invokeMock.mockImplementation((cmd: string, _args: any) => {
      if (cmd === 'get_api_key') return Promise.resolve('ollama-secret-token');
      if (cmd === 'test_ollama_connection') return Promise.resolve('Connected — 3 models available');
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('get_api_key', {
      provider: 'ollama_api_key',
    });
    expect(invokeMock).toHaveBeenCalledWith('test_ollama_connection', {
      host: '192.168.1.10',
      port: 11435,
      apiKey: 'ollama-secret-token',
    });
    const state = get(endpointHealth);
    expect(state.ai).toBe('online');
  });

  it('fetches lmstudio_api_key from keychain and forwards it to the LM Studio probe', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '192.168.1.10',
      lmstudio_port: 1235,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_mode: 'local',
    } as any);

    invokeMock.mockImplementation((cmd: string, _args: any) => {
      if (cmd === 'get_api_key') return Promise.resolve('lmstudio-secret-token');
      if (cmd === 'test_lmstudio_connection') return Promise.resolve('Connected');
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('get_api_key', {
      provider: 'lmstudio_api_key',
    });
    expect(invokeMock).toHaveBeenCalledWith('test_lmstudio_connection', {
      host: '192.168.1.10',
      port: 1235,
      apiKey: 'lmstudio-secret-token',
    });
    const state = get(endpointHealth);
    expect(state.ai).toBe('online');
  });

  it('AI probe continues without auth if keychain fetch fails', async () => {
    settings.set({
      ai_provider: 'ollama',
      lmstudio_host: '',
      lmstudio_port: 1234,
      ollama_host: '192.168.1.10',
      ollama_port: 11435,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_mode: 'local',
    } as any);

    invokeMock.mockImplementation((cmd: string, _args: any) => {
      if (cmd === 'get_api_key') return Promise.reject(new Error('keychain locked'));
      if (cmd === 'test_ollama_connection') return Promise.resolve('Connected');
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('test_ollama_connection', {
      host: '192.168.1.10',
      port: 11435,
      apiKey: undefined,
    });
    const state = get(endpointHealth);
    expect(state.ai).toBe('online');
  });
```

- [ ] **Step 2: Run the tests; expect failures**

```bash
npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -25
```

Expected: 3 new tests fail — `probeAi` doesn't fetch the api_key yet.

You may also see existing tests that previously asserted `apiKey` was absent from `test_ollama_connection` / `test_lmstudio_connection` calls now failing if they used strict `expect.toHaveBeenCalledWith`. Update those by adding `apiKey: undefined` to the expected args.

- [ ] **Step 3: Modify `probeAi` to fetch the api_key**

Open `src/lib/stores/endpointHealth.ts`. The current `probeAi` function (in `createEndpointHealthStore`):

```ts
async function probeAi(cfg: AppConfig): Promise<ServiceStatus> {
  const provider = cfg.ai_provider;
  if (provider === 'ollama') {
    if (isLoopbackHost(cfg.ollama_host)) return 'skipped';
    try {
      await invoke('test_ollama_connection', {
        host: cfg.ollama_host,
        port: cfg.ollama_port,
      });
      return 'online';
    } catch {
      return 'offline';
    }
  }
  if (provider === 'lmstudio') {
    if (isLoopbackHost(cfg.lmstudio_host)) return 'skipped';
    try {
      await invoke('test_lmstudio_connection', {
        host: cfg.lmstudio_host,
        port: cfg.lmstudio_port,
      });
      return 'online';
    } catch {
      return 'offline';
    }
  }
  return 'skipped';
}
```

Replace it with:

```ts
async function probeAi(cfg: AppConfig): Promise<ServiceStatus> {
  const provider = cfg.ai_provider;
  if (provider === 'ollama') {
    if (isLoopbackHost(cfg.ollama_host)) return 'skipped';
    let apiKey: string | undefined = undefined;
    try {
      const key = await invoke<string | null>('get_api_key', {
        provider: 'ollama_api_key',
      });
      if (key) apiKey = key;
    } catch {
      // Keychain unavailable or no key stored — continue without auth.
    }
    try {
      await invoke('test_ollama_connection', {
        host: cfg.ollama_host,
        port: cfg.ollama_port,
        apiKey,
      });
      return 'online';
    } catch {
      return 'offline';
    }
  }
  if (provider === 'lmstudio') {
    if (isLoopbackHost(cfg.lmstudio_host)) return 'skipped';
    let apiKey: string | undefined = undefined;
    try {
      const key = await invoke<string | null>('get_api_key', {
        provider: 'lmstudio_api_key',
      });
      if (key) apiKey = key;
    } catch {
      // Keychain unavailable or no key stored — continue without auth.
    }
    try {
      await invoke('test_lmstudio_connection', {
        host: cfg.lmstudio_host,
        port: cfg.lmstudio_port,
        apiKey,
      });
      return 'online';
    } catch {
      return 'offline';
    }
  }
  return 'skipped';
}
```

- [ ] **Step 4: Run all endpointHealth tests**

```bash
npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -20
```

Expected: all tests pass (existing 14 + 3 new = 17 tests).

If existing tests now fail because the `test_ollama_connection` / `test_lmstudio_connection` call now sends `apiKey: undefined` and the test asserted those args strictly, update those tests' expected args to include `apiKey: undefined`.

- [ ] **Step 5: Full vitest + svelte-check**

```bash
npx vitest run 2>&1 | tail -5
npm run check 2>&1 | tail -10
```

Expected: 169 + 3 = 172 tests pass; 0 svelte-check errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/stores/endpointHealth.ts src/lib/stores/endpointHealth.test.ts
git commit -m "feat(endpointHealth): probeAi fetches per-provider api_key

Mirrors Phase 2 Task 1's STT pattern: invoke('get_api_key',
{provider: 'ollama_api_key'|'lmstudio_api_key'}) before the probe;
keychain failure → continue without auth. Adds 3 tests covering
both providers' happy path and the keychain-failure fallback.

After Task 4's auth_key parameter on the test commands and Task 2's
pair-handler keychain populate, paired clients now show green pill
instead of offline.

Phase 3 Task 5."
```

---

## Task 6: ClientPair.svelte — refresh settings after pair / unpair

**Files:**
- Modify: `src/lib/components/settings/sharing/ClientPair.svelte` (around line 129 where `pair_with_server` is invoked, and wherever `unpair` is invoked)

**Why:** After Task 2 saves new AppConfig, the frontend reactive `settings` store still has the old (empty) values until something calls `settings.load()`. Without this, the Settings UI doesn't reflect the pair-time autofill, and the user sees the still-blank API Key field. Also: endpointHealth's `settings.subscribe` won't fire its immediate-re-probe because the store isn't notified.

- [ ] **Step 1: Find the pair / unpair call sites**

```bash
grep -n "pair_with_server\|unpair" src/lib/components/settings/sharing/ClientPair.svelte
```

Expected: at least two lines — one invoking `pair_with_server`, one invoking `unpair`.

- [ ] **Step 2: Add `settings.load()` to the pair success path**

In `ClientPair.svelte`, locate the line that invokes `pair_with_server`:

```svelte
      await invoke('pair_with_server', {
        // …existing args (lan, tailscale, ports, code, label)…
      });
```

Right after the `await invoke('pair_with_server', …)` resolves (still inside the success branch of whatever try/catch surrounds it), add:

```svelte
      await settings.load();
```

The `settings` import is `import { settings } from '../../../stores/settings';` (the relative path from `ClientPair.svelte`). Check the file's existing imports — if `settings` isn't already imported, add it. `settings.load()` is async and returns a Promise; await it so any downstream code that reads `$settings` sees the new values.

- [ ] **Step 3: Add `settings.load()` to the unpair success path**

Same file. Find the `invoke('unpair', …)` call. Right after it resolves, add:

```svelte
      await settings.load();
```

- [ ] **Step 4: Build + tests**

```bash
npm run check 2>&1 | tail -10
npx vitest run 2>&1 | tail -5
```

Expected: 0 svelte-check errors; 172 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib/components/settings/sharing/ClientPair.svelte
git commit -m "feat(sharing-ui): reload settings after pair / unpair

After invoking pair_with_server (which now mutates AppConfig in
Task 2) or unpair (which resets AppConfig in Task 3), the frontend
must call settings.load() so the reactive \$settings store and
the Settings UI reflect the new values. Without this, endpointHealth's
settings.subscribe doesn't fire and the Settings → Audio / Models
fields stay blank.

Phase 3 Task 6."
```

---

## Task 7: Manual QA + version bump 0.10.60

**Files:**
- Modify: `src-tauri/Cargo.toml` (version)
- Modify: `package.json` (version)
- Modify: `src-tauri/tauri.conf.json` (version)

**Why:** Last step — verification + release prep.

- [ ] **Step 1: Final automated sweep**

```bash
cargo test --workspace --lib 2>&1 | tail -5
```

Expected: 599 backend tests pass workspace-wide (no regression on the lower crates). Record the count.

```bash
npx vitest run 2>&1 | tail -10
```

Expected: ~172 frontend tests pass.

```bash
npm run check 2>&1 | tail -10
```

Expected: 0 svelte-check errors; the pre-existing ExportDialog warning is acceptable.

- [ ] **Step 2: Verify no stale references**

```bash
grep -rn 'test_ollama_connection\|test_lmstudio_connection' src/ src-tauri/src/ 2>&1 | grep -v test | head -10
```

Confirm every call site of these commands now passes `apiKey` (either a real value or `undefined`).

- [ ] **Step 3: Version bump**

Current: `0.10.59`. Bump to `0.10.60` (patch — additive fix).

```bash
# src-tauri/Cargo.toml
```

Change `version = "0.10.59"` → `version = "0.10.60"`.

```bash
# package.json
```

Change `"version": "0.10.59"` → `"version": "0.10.60"`.

```bash
# src-tauri/tauri.conf.json
```

Change `"version": "0.10.59"` → `"version": "0.10.60"`.

Verify:

```bash
grep -E '0\.10\.5[09]|0\.10\.60' src-tauri/Cargo.toml package.json src-tauri/tauri.conf.json
```

Expected: all three show `0.10.60`.

- [ ] **Step 4: Final build after bump**

```bash
cargo build -p rust-medical-assistant 2>&1 | tail -3
```

Expected: clean. `Cargo.lock` updates to `0.10.60`.

- [ ] **Step 5: Commit + tag**

```bash
git add src-tauri/Cargo.toml package.json src-tauri/tauri.conf.json Cargo.lock
git commit -m "chore: bump 0.10.60 — paired-bearer auto-fill (Phase 3)

When a client successfully pairs with an office server, the pair flow
now writes the bearer into per-service keychain slots (stt_remote_api_key,
ollama_api_key, lmstudio_api_key) and populates the matching AppConfig
host/port/mode fields. Unpair clears the same. test_ollama_connection
and test_lmstudio_connection gain an api_key parameter mirroring
test_stt_remote_connection. endpointHealth's AI probe fetches the
per-provider key from keychain.

Action required for existing paired clients: unpair-and-re-pair once
after upgrading to v0.10.60 to populate the new keychain slots and
AppConfig fields."

git tag v0.10.60
```

(Do NOT push the tag — the user pushes when manual QA passes.)

- [ ] **Step 6: Print the manual QA checklist verbatim**

```
Manual QA — paired-bearer auto-fill (v0.10.60):

Pre-requisite: have the Mac office server running on Tailscale at the
address shown in its Settings → Sharing panel.

1. On the Windows client, open Settings → Sharing → Unpair (if currently paired).
2. Open Settings → Audio: confirm host is blank, port is 8080 (default),
   stt_mode shows Local, API Key field is empty.
3. Open Settings → Sharing → scan QR / enter pair code from the Mac.
4. After pairing succeeds:
   - Settings → Audio: host is the Mac's LAN address, port is 8081, 
     stt_mode flipped to Remote, API Key field is populated (long random string).
   - Settings → Models: host is the Mac's LAN address, ports are 11435 (Ollama) /
     1235 (LM Studio), API Keys are populated.
   - Status bar pill (Phase 2) shows green within ~10s.
5. Record a short test consultation. Transcription completes (no 401).
   Generate SOAP — no auth error.
6. Click Unpair in Settings → Sharing.
   - Settings → Audio reverts to host="", port=8080, mode=Local, API Key empty.
   - Settings → Models reverts ollama_host/port and lmstudio_host/port to localhost defaults.
   - Status bar pill disappears within 10s.
   - ai_provider field is unchanged (preserves whichever you had: ollama or lmstudio).
```

## Self-Review

**Spec coverage:**
- AC#1 (pair writes bearer to three keychain slots) → Task 2 ✓
- AC#2 (pair updates AppConfig fields, preserves ai_provider) → Task 2 ✓
- AC#3 (unpair clears the slots + resets AppConfig) → Task 3 ✓
- AC#4 (test_ollama/lmstudio_connection accept api_key + 401 message) → Task 4 ✓
- AC#5 (endpointHealth's probeAi fetches per-provider api_key) → Task 5 ✓
- AC#6 (Settings UI reflects populated fields after pair) → Task 6 ✓
- AC#7 (manual QA passes) → Task 7 ✓
- AC#8 (existing tests pass) → all tasks include the regression check ✓
- AC#9 (cargo test / vitest green) → Task 7 ✓
- AC#10 (svelte-check produces no new errors) → Task 7 ✓

**Placeholder scan:** Task 4 Step 1 has a `/* test AppState */` placeholder in the test code. That's intentional — the implementor reads the existing helper in `src-tauri/src/commands/generation/test_helpers.rs` (Phase 1) and adapts. The placeholder is flagged with the Implementor note explaining the alternative (refactor inner logic into a pure helper if AppState construction is heavy).

**Type consistency:** `apply_paired_settings(cfg, host, ports)` and `reset_paired_settings(cfg)` signatures match between Task 1 (declaration), Task 2 (pair-handler call), and Task 3 (unpair-handler call). `PairPorts` struct fields (`whisper`, `ollama`, `pairing`, `lmstudio: Option<u16>`, `vocab: Option<u16>`) match the existing `crates/sharing/src/qr.rs` definition. Keychain slot names (`stt_remote_api_key`, `ollama_api_key`, `lmstudio_api_key`) match between Tasks 2, 3, and 5.

**Known under-specifications** (flagged in-line):
- Task 4 Step 1: the test AppState constructor pattern — adapt from Phase 1's `build_test_state_with_recording`. If too heavy, factor the inner request logic into a pure helper.
- Task 4 Step 6: caller-update path depends on whether `Models.svelte` invokes directly or through a wrapper.

---

Plan complete and saved to `docs/superpowers/plans/2026-05-13-paired-bearer-autofill.md`. Two execution options:

**1. Subagent-Driven (recommended)** — fresh subagent per task with two-stage review (spec + code quality). Matches the convention from Phases 1 + 2.

**2. Inline Execution** — execute tasks in this session with checkpoints.

Which approach?
