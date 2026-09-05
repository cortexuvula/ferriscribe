//! Pairing flow: QR generation + client list/revoke (server side), plus the
//! client-side `pair_with_server` / `paired_endpoint` / `unpair` commands.

use medical_core::error::{AppError, AppResult};
use medical_sharing::qr::{PairPayload, PairPorts, encode};
use tauri::State;

use crate::state::AppState;

use super::{ClientDto, PairedConnection, paired_connection_path};

#[tauri::command]
pub async fn pairing_qr(state: State<'_, AppState>) -> AppResult<String> {
    let svc = state.sharing.read().await;
    let svc = svc
        .as_ref()
        .ok_or_else(|| AppError::Other("sharing not running".into()))?;
    let code = svc.pairing_state().issue_code().await;
    let cfg = svc.config();
    let lan = local_lan_address();
    let payload = PairPayload {
        host: cfg.friendly_name.clone(),
        lan,
        tailscale: tailscale_address().await,
        ports: PairPorts {
            ollama: cfg.ollama_proxy_port,
            whisper: cfg.whisper_proxy_port,
            pairing: cfg.pairing_port,
            lmstudio: cfg.lmstudio_proxy_port,
            omlx: cfg.omlx_proxy_port,
            vocab: Some(cfg.vocab_port),
        },
        code,
    };
    Ok(encode(&payload))
}

#[tauri::command]
pub async fn list_paired_clients(state: State<'_, AppState>) -> AppResult<Vec<ClientDto>> {
    let svc = state.sharing.read().await;
    let svc = svc
        .as_ref()
        .ok_or_else(|| AppError::Other("sharing not running".into()))?;
    let rows = svc
        .token_store()
        .list()
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|r| ClientDto {
            id: r.id,
            label: r.label,
        })
        .collect())
}

#[tauri::command]
pub async fn revoke_client(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let svc = state.sharing.read().await;
    let svc = svc
        .as_ref()
        .ok_or_else(|| AppError::Other("sharing not running".into()))?;
    svc.token_store()
        .revoke(id)
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(())
}

#[tauri::command]
pub async fn rename_client(state: State<'_, AppState>, id: i64, label: String) -> AppResult<()> {
    if label.trim().is_empty() {
        return Err(AppError::Other("label cannot be empty".into()));
    }
    let svc = state.sharing.read().await;
    let svc = svc
        .as_ref()
        .ok_or_else(|| AppError::Other("sharing not running".into()))?;
    svc.token_store()
        .update_label(id, &label)
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(())
}

#[tauri::command]
pub fn suggested_client_label() -> String {
    medical_sharing::suggested_label::suggested_client_label()
}

/// Outcome of a single pair-enroll attempt against one base URL.
///
/// `Connect` signals a transport-level failure (TCP refused, DNS unresolved,
/// timeout) — the caller may choose to retry against a different address.
/// `Final` signals a definitive outcome (HTTP rejection, malformed body,
/// missing token field) that should bubble up without retry.
enum PairAttemptError {
    Connect(reqwest::Error),
    Final(AppError),
}

/// Perform a single pair-enroll POST against `base` and return the parsed
/// JSON body on success. Discriminates connect-level failures (retryable)
/// from server-level rejections (not retryable). Private to this module.
async fn try_pair_at_base(
    http: &reqwest::Client,
    base: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, PairAttemptError> {
    let resp = http
        .post(format!("{base}/pair/enroll"))
        .timeout(std::time::Duration::from_secs(10))
        .json(body)
        .send()
        .await
        .map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                PairAttemptError::Connect(e)
            } else {
                PairAttemptError::Final(AppError::Other(e.to_string()))
            }
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = medical_core::http_error_body::read_error_body(resp, 200).await;
        return Err(PairAttemptError::Final(AppError::Other(format!(
            "server rejected pair: {status} {body_text}"
        ))));
    }

    resp.json()
        .await
        .map_err(|e| PairAttemptError::Final(AppError::Other(e.to_string())))
}

/// Pair with an office server: POST the enroll code, receive a bearer token,
/// persist the token in the OS keychain, and persist the non-secret endpoint
/// metadata to disk. Returns nothing to the frontend — no raw token is ever
/// sent to JS.
///
/// After persisting, the in-memory Ollama, LM Studio, oMLX, and remote-STT
/// providers are updated immediately so the "models visible" success message
/// in the UI is truthful without requiring an app restart.
///
/// If the client's current `ai_provider` doesn't answer through the office
/// server's proxies (e.g. a fresh install defaults to `"lmstudio"` but the
/// office runs Ollama only), the pair flow probes each advertised provider
/// proxy and switches to the first one that answers — pairing succeeds as
/// long as ANY of Ollama, LM Studio, or oMLX is available.
#[tauri::command]
pub async fn pair_with_server(
    state: State<'_, AppState>,
    lan: Option<String>,
    tailscale: Option<String>,
    ports: PairPorts,
    code: String,
    label: String,
) -> AppResult<()> {
    pair_with_server_inner(&state, lan, tailscale, ports, code, label).await
}

/// Testable core of [`pair_with_server`]: takes `&AppState` directly so the
/// command-level tests can drive the full flow (handshake, endpoint wiring,
/// STT switch, provider selection, model refresh, persistence) against a
/// wiremock office server.
pub(super) async fn pair_with_server_inner(
    state: &AppState,
    lan: Option<String>,
    tailscale: Option<String>,
    ports: PairPorts,
    code: String,
    label: String,
) -> AppResult<()> {
    // ── Phase 1: handshake ──
    let body = serde_json::json!({ "code": code, "label": label });
    let (winning_host, v) = pair_handshake(
        &state.http_client,
        lan.as_deref(),
        tailscale.as_deref(),
        ports.pairing,
        &body,
    )
    .await?;
    let token = bearer_from_enroll_response(&v)?;

    // ── Phase 2: persist credentials + connection metadata ──
    store_sharing_bearer(&token)?;
    let conn = PairedConnection {
        lan,
        tailscale,
        ports: ports.clone(),
        label,
    };
    persist_connection_metadata(&conn)?;

    // ── Phase 3: re-point the live AI providers through the office proxies ──
    let (pair_cfg, eps) = wire_ai_provider_endpoints(state, &conn, &token).await?;

    // ── Phase 4: route STT through the office whisper proxy ──
    switch_stt_to_remote(state, eps.whisper.clone()).await?;

    // ── Phase 5: availability-aware provider + model selection ──
    let (current_answers, chosen_provider) =
        select_served_provider(state, &pair_cfg.ai_provider, &ports, &winning_host, &token).await;
    let effective_provider = chosen_provider
        .clone()
        .or_else(|| current_answers.then(|| pair_cfg.ai_provider.clone()));
    let chosen_model =
        refresh_model_choice(state, effective_provider.as_deref(), &pair_cfg.ai_model).await;

    // ── Phase 6: mirror the bearer into per-service slots + persist AppConfig ──
    persist_pair_settings(
        state,
        &winning_host,
        &ports,
        &token,
        chosen_provider.as_deref(),
        chosen_model.as_deref(),
    )
    .await
}

/// Phase 1: POST the enroll code, trying LAN first and falling back to
/// Tailscale exactly once on a connect-level failure (TCP refused, DNS
/// unresolved, timeout). HTTP-level rejections (4xx/5xx) are NOT retried —
/// those are real server-side responses, not connectivity.
///
/// Returns the host that actually answered (downstream AppConfig autofill
/// must use the reachable address) plus the raw enroll JSON. `http_url`
/// brackets IPv6 literals — without it, an mDNS-discovered IPv6 address
/// makes reqwest emit a generic "Builder error" with no URL context.
async fn pair_handshake(
    http: &reqwest::Client,
    lan: Option<&str>,
    tailscale: Option<&str>,
    pairing_port: u16,
    body: &serde_json::Value,
) -> AppResult<(String, serde_json::Value)> {
    match (lan, tailscale) {
        (Some(l), ts_opt) => {
            let lan_base = medical_core::types::http_url(l, pairing_port);
            tracing::info!(host = %l, port = pairing_port, "pair: trying LAN");
            match try_pair_at_base(http, &lan_base, body).await {
                Ok(v) => Ok((l.to_string(), v)),
                Err(PairAttemptError::Connect(_)) => {
                    if let Some(ts) = ts_opt {
                        tracing::info!(
                            host = %ts,
                            port = pairing_port,
                            "pair: LAN unreachable, falling back to Tailscale"
                        );
                        let ts_base = medical_core::types::http_url(ts, pairing_port);
                        match try_pair_at_base(http, &ts_base, body).await {
                            Ok(v) => Ok((ts.to_string(), v)),
                            Err(PairAttemptError::Connect(e)) => {
                                Err(AppError::Other(e.to_string()))
                            }
                            Err(PairAttemptError::Final(e)) => Err(e),
                        }
                    } else {
                        // No Tailscale fallback available — surface the
                        // LAN connect failure as a normal AppError.
                        Err(AppError::Other(
                            "could not connect to server (LAN unreachable, no Tailscale address)"
                                .into(),
                        ))
                    }
                }
                Err(PairAttemptError::Final(e)) => Err(e),
            }
        }
        (None, Some(ts)) => {
            let ts_base = medical_core::types::http_url(ts, pairing_port);
            tracing::info!(host = %ts, port = pairing_port, "pair: trying Tailscale");
            match try_pair_at_base(http, &ts_base, body).await {
                Ok(v) => Ok((ts.to_string(), v)),
                Err(PairAttemptError::Connect(e)) => Err(AppError::Other(e.to_string())),
                Err(PairAttemptError::Final(e)) => Err(e),
            }
        }
        (None, None) => Err(AppError::Other("no reachable address provided".into())),
    }
}

/// Extract the bearer token from the enroll response.
fn bearer_from_enroll_response(v: &serde_json::Value) -> AppResult<String> {
    v.get("token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .ok_or_else(|| AppError::Other("server did not return a token".into()))
}

/// Phase 2b: persist the non-secret connection metadata next to the config.
fn persist_connection_metadata(conn: &PairedConnection) -> AppResult<()> {
    let json = serde_json::to_string(conn)?;
    let path = paired_connection_path()?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Phase 3: load the pair-time config, build the office endpoints, and
/// re-point the live Ollama / LM Studio / oMLX providers through the
/// proxies immediately — so the "models visible" success message in
/// ClientPair.svelte is truthful without an app restart. Returns the
/// config (caller needs the current provider/model) and the endpoints
/// (the whisper one feeds the STT switch).
async fn wire_ai_provider_endpoints(
    state: &AppState,
    conn: &PairedConnection,
    token: &str,
) -> AppResult<(
    medical_core::types::settings::AppConfig,
    super::PairedEndpoints,
)> {
    let pair_cfg = crate::commands::load_app_config(&state.db, "pairing").await?;
    let allow_public = pair_cfg.allow_public_endpoint;
    let eps = super::paired_endpoints(conn, Some(token.to_string()));

    {
        let guard = state.ollama_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(eps.ollama.clone(), allow_public).await?;
        }
    }
    {
        let guard = state.lmstudio_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(eps.lmstudio.clone(), allow_public).await?;
        }
    }
    {
        let guard = state.omlx_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(eps.omlx.clone(), allow_public).await?;
        }
    }

    Ok((pair_cfg, eps))
}

/// Phase 4: STT requires more than set_endpoint — if the user was in Local
/// mode at app startup, `state.remote_stt_provider` is None and
/// set_endpoint would be a no-op. Persist `stt_mode = Remote` and rebuild
/// the STT providers so transcription routes through the office server's
/// whisper proxy — otherwise the user hits "Whisper model not found"
/// because the local provider is still the active one.
async fn switch_stt_to_remote(
    state: &AppState,
    whisper_ep: Option<medical_core::types::RemoteEndpoint>,
) -> AppResult<()> {
    use medical_core::types::settings::SttMode;
    let db = std::sync::Arc::clone(&state.db);
    let cfg = tokio::task::spawn_blocking(
        move || -> AppResult<medical_core::types::settings::AppConfig> {
            let conn = db.conn().map_err(|e| AppError::Other(e.to_string()))?;
            let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
                .map_err(|e| AppError::Other(e.to_string()))?;
            cfg.migrate();
            if cfg.stt_mode != SttMode::Remote {
                cfg.stt_mode = SttMode::Remote;
                medical_db::settings::SettingsRepo::save_config(&conn, &cfg)
                    .map_err(|e| AppError::Other(e.to_string()))?;
                tracing::info!("pair: switched stt_mode to Remote");
            }
            Ok(cfg)
        },
    )
    .await
    .map_err(crate::commands::join_err)??;

    let stt_handles =
        crate::state::init_stt_providers_with_config(&state.data_dir, &cfg, whisper_ep);
    {
        let mut guard = state.stt_providers.lock().await;
        *guard = stt_handles.provider;
    }
    *state.remote_stt_provider.write().await = stt_handles.remote;
    *state.local_stt_provider.write().await = stt_handles.local;
    Ok(())
}

/// Phase 5a: availability-aware provider selection.
///
/// A fresh client defaults to ai_provider = "lmstudio". If the server
/// doesn't serve that provider, generation would point at a dead endpoint
/// even though the server happily serves Ollama or oMLX — looking exactly
/// like "the client won't connect".
///
/// Advertisement alone is not trusted: the QR encodes the server's static
/// config ports (LM Studio / oMLX may be listed without their proxies
/// bound), and Ollama's proxy port is advertised unconditionally. So the
/// CURRENT provider is always probed through the just-established proxies
/// — if it answers, it is kept (respects an explicit user choice). Only
/// when it doesn't answer (or isn't advertised at all) are the other
/// providers probed, switching to the first that answers. If nothing
/// answers, the current setting stands and pairing still succeeds.
///
/// Returns `(current_answers, chosen_switch)`.
async fn select_served_provider(
    state: &AppState,
    current: &str,
    ports: &PairPorts,
    winning_host: &str,
    token: &str,
) -> (bool, Option<String>) {
    let current_answers = match provider_proxy_port(ports, current) {
        Some(port) => {
            probe_provider_proxy(&state.http_client, winning_host, port, current, token).await
        }
        None => false,
    };

    if current_answers {
        tracing::info!(
            provider = %current,
            "pair: current provider answered through the office proxy; keeping it"
        );
        return (true, None);
    }

    for cand in served_providers(ports) {
        if cand == current {
            continue; // already probed above and it didn't answer
        }
        let Some(proxy_port) = provider_proxy_port(ports, cand) else {
            continue;
        };
        if !probe_provider_proxy(&state.http_client, winning_host, proxy_port, cand, token).await {
            tracing::info!(
                provider = cand,
                "pair: provider proxy not answering; skipping"
            );
            continue;
        }
        return (false, Some(cand.to_string()));
    }
    tracing::info!("pair: no advertised provider proxy answered; keeping current provider setting");
    (false, None)
}

/// Phase 5b: best-effort model validation for whichever provider
/// generation will use after this pair — switched OR kept. A kept provider
/// can carry a stale model name (e.g. a placeholder saved by an older
/// build whose model fetch failed); sending it to the server 404s every
/// generation. Returns the replacement model when the saved one isn't
/// offered.
async fn refresh_model_choice(
    state: &AppState,
    effective_provider: Option<&str>,
    saved_model: &str,
) -> Option<String> {
    let provider_id = effective_provider?;
    let arc = {
        let registry = state.ai_providers.lock().await;
        registry.get_arc(provider_id)
    };
    let provider = arc?;
    match provider.available_models().await {
        Ok(models) => {
            let ids: Vec<String> = models.into_iter().map(|m| m.id).collect();
            refreshed_model(saved_model, &ids)
        }
        // The error carries the provider name and endpoint URL (no model
        // content) — worth a trace, since it means the model refresh was
        // skipped and a stale name survives.
        Err(e) => {
            tracing::warn!(
                provider = %provider_id,
                error = %e,
                "pair: model list unavailable; kept the saved model"
            );
            None
        }
    }
}

/// Phase 6: mirror the bearer into the per-service keychain slots and
/// persist the AppConfig host/port fields (plus the availability-selected
/// provider/model). The rest of the app — Settings UI, pre-flight,
/// endpointHealth polling — reads from those slots and fields, so paired
/// clients don't need to manually fill in Settings → Audio / Models.
/// Finally flips the live registry's active provider so generation uses
/// the served provider immediately (no reinit needed).
async fn persist_pair_settings(
    state: &AppState,
    winning_host: &str,
    ports: &PairPorts,
    token: &str,
    chosen_provider: Option<&str>,
    chosen_model: Option<&str>,
) -> AppResult<()> {
    use super::settings_helpers::apply_paired_settings;

    // The in-memory RemoteEndpoint still carries BOTH LAN and Tailscale and
    // probes both at call time; but the static AppConfig field has to be a
    // single address the client can actually reach. Using `winning_host`
    // ensures a remote-paired client doesn't get the server's unreachable
    // LAN IP written into Settings (which would poison pre-flight checks
    // and health polling that read AppConfig host fields directly).
    for slot in &[
        "stt_remote_api_key",
        "ollama_api_key",
        "lmstudio_api_key",
        "omlx_api_key",
    ] {
        state
            .keys
            .store_key(slot, token)
            .map_err(|e| AppError::Other(format!("autofill: store {slot}: {e}")))?;
    }

    // Wrapped in spawn_blocking so the SQLite read-modify-write never
    // blocks the async runtime worker.
    let db = std::sync::Arc::clone(&state.db);
    let host_for_db = winning_host.to_string();
    let ports_for_db = ports.clone();
    let provider_for_db = chosen_provider.map(str::to_string);
    let model_for_db = chosen_model.map(str::to_string);
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = db.conn().map_err(|e| AppError::Other(e.to_string()))?;
        let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| AppError::Other(e.to_string()))?;
        cfg.migrate();
        apply_paired_settings(&mut cfg, &host_for_db, &ports_for_db);
        if let Some(p) = provider_for_db {
            tracing::info!(
                from = %cfg.ai_provider, to = %p,
                "pair: current provider not served by server; switching"
            );
            cfg.ai_provider = p;
        }
        // Written independently of a provider switch: a kept provider can
        // still need its stale/placeholder model corrected.
        if let Some(m) = model_for_db {
            tracing::info!(
                from = %cfg.ai_model, to = %m,
                "pair: saved model not offered by the serving provider; replacing"
            );
            cfg.ai_model = m;
        }
        medical_db::settings::SettingsRepo::save_config(&conn, &cfg)
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    })
    .await
    .map_err(crate::commands::join_err)??;

    if let Some(p) = chosen_provider {
        let mut registry = state.ai_providers.lock().await;
        if registry.set_active(p) {
            tracing::info!(provider = %p, "pair: active provider switched to served provider");
        }
    }

    tracing::info!(
        host = %winning_host,
        whisper_port = ports.whisper,
        ollama_port = ports.ollama,
        lmstudio_port = ?ports.lmstudio,
        omlx_port = ?ports.omlx,
        "pair: populated per-service api_keys and AppConfig host/ports"
    );
    Ok(())
}

/// Provider ids the office server can serve, derived from the pairing
/// ports. Ollama's proxy port is a required field (always advertised); the
/// LM Studio / oMLX ports appear only when those upstreams are actually
/// ready (readiness-gated advertisement), so their presence is a real
/// availability signal. Order is the switch preference when the client's
/// current provider isn't served: Ollama first (the office wizard installs
/// it persistently), then LM Studio, then oMLX.
fn served_providers(ports: &PairPorts) -> Vec<&'static str> {
    let mut v = vec!["ollama"];
    if ports.lmstudio.is_some() {
        v.push("lmstudio");
    }
    if ports.omlx.is_some() {
        v.push("omlx");
    }
    v
}

/// Write the sharing bearer to the OS keychain slot that
/// `state::load_sharing_bearer` reads at startup and re-init.
#[cfg(not(test))]
fn store_sharing_bearer(token: &str) -> AppResult<()> {
    keyring::Entry::new("rustMedicalAssistant", "sharing-bearer")
        .map_err(|e| AppError::Other(format!("keychain open: {e}")))?
        .set_password(token)
        .map_err(|e| AppError::Other(format!("keychain write: {e}")))
}

/// Test double: never touches the developer's real keychain. The token's
/// effect is asserted through the per-service `state.keys` mirror instead
/// (same KeyStorage abstraction the production flow writes).
#[cfg(test)]
fn store_sharing_bearer(_token: &str) -> AppResult<()> {
    Ok(())
}

/// The advertised proxy port for a provider id, or `None` when the server
/// doesn't serve it.
fn provider_proxy_port(ports: &PairPorts, provider: &str) -> Option<u16> {
    match provider {
        "ollama" => Some(ports.ollama),
        "lmstudio" => ports.lmstudio,
        "omlx" => ports.omlx,
        _ => None,
    }
}

/// Build the probe URL for one of the server's provider auth-proxies.
/// Goes through [`medical_core::types::http_url`] so IPv6 literals get
/// bracketed — a raw `http://{host}:{port}` format makes reqwest fail URL
/// parsing with an opaque "Builder error" (the same trap the pair handshake
/// above documents).
fn provider_probe_url(host: &str, port: u16, provider: &str) -> String {
    let path = if provider == "ollama" {
        "/api/tags"
    } else {
        "/v1/models"
    };
    format!("{}{path}", medical_core::types::http_url(host, port))
}

/// Probe one of the server's provider auth-proxies with the freshly issued
/// token. `true` iff it answered 2xx — i.e. the provider is actually served
/// and reachable through the exact path generation will use. Probes hit the
/// user-configured office server only (the host that just answered the pair
/// handshake) and carry no PHI.
async fn probe_provider_proxy(
    http: &reqwest::Client,
    host: &str,
    port: u16,
    provider: &str,
    token: &str,
) -> bool {
    let url = provider_probe_url(host, port, provider);
    match http
        .get(&url)
        .timeout(std::time::Duration::from_secs(3))
        .bearer_auth(token)
        .send()
        .await
    {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Decide whether the pair flow must replace the saved `ai_model` for the
/// provider generation will use after pairing (switched OR kept): the model
/// must actually be offered by that provider. Returns the replacement (first
/// offered model) when a refresh is needed, else `None`.
///
/// The list comes straight from the server (`available_models` errors rather
/// than synthesizing ids), so any name it contains is real — including a
/// model that happens to be called "llama3" or "default". Filtering by name
/// here would shadow such a model and override a user's explicit choice.
fn refreshed_model(saved: &str, offered: &[String]) -> Option<String> {
    let first = offered.first()?;
    if offered.iter().any(|id| id == saved) {
        None // an explicitly chosen, offered model — respect it
    } else {
        Some(first.clone())
    }
}

/// Returns the saved paired-connection metadata, or `None` if not paired.
#[tauri::command]
pub async fn paired_endpoint() -> AppResult<Option<PairedConnection>> {
    let path = paired_connection_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(&path)?;
    let conn: PairedConnection = serde_json::from_str(&json)?;
    Ok(Some(conn))
}

/// Remove the keychain entry and the on-disk metadata. Idempotent.
/// Also clears the per-service keychain slots and resets AppConfig fields
/// the pair flow populated (Phase 3).
#[tauri::command]
pub async fn unpair(state: State<'_, AppState>) -> AppResult<()> {
    // Remove the sharing-bearer keychain entry (ignore NoEntry).
    if let Ok(entry) = keyring::Entry::new("rustMedicalAssistant", "sharing-bearer") {
        let _ = entry.delete_credential();
    }

    // Remove the metadata file (ignore not-found).
    let path = paired_connection_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }

    // ── Phase 3: clear per-service keychain slots and reset AppConfig ──
    {
        use super::settings_helpers::reset_paired_settings;

        for slot in &[
            "stt_remote_api_key",
            "ollama_api_key",
            "lmstudio_api_key",
            "omlx_api_key",
        ] {
            // Idempotent — ignore "not found" errors per the existing pattern.
            let _ = state.keys.remove_key(slot);
        }

        let db = std::sync::Arc::clone(&state.db);
        tokio::task::spawn_blocking(move || -> AppResult<()> {
            let conn = db.conn().map_err(|e| AppError::Other(e.to_string()))?;
            let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
                .map_err(|e| AppError::Other(e.to_string()))?;
            cfg.migrate();
            reset_paired_settings(&mut cfg);
            medical_db::settings::SettingsRepo::save_config(&conn, &cfg)
                .map_err(|e| AppError::Other(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(crate::commands::join_err)??;

        tracing::info!("unpair: cleared per-service api_keys and reset AppConfig");
    }

    Ok(())
}

fn local_lan_address() -> Option<String> {
    use std::net::UdpSocket;
    // Standard "connect to a public IP, read our outbound IP" trick. Doesn't actually transmit.
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    s.local_addr().ok().map(|a| a.ip().to_string())
}

async fn tailscale_address() -> Option<String> {
    // Delegate to the library helper so the shell-out lives in one place
    // (the orchestrator uses the same function to populate InfoSnapshot).
    medical_sharing::tailscale::self_dns_name().await
}

/// Self-heal: backfill a missing `tailscale` field on an existing paired
/// connection by probing the server's `/info` endpoint over LAN.
///
/// Clients that paired over mDNS before v0.30.5 have `tailscale: null` in
/// their `sharing-paired.json`, which silently blocks content sync (the
/// Tailscale-only transport gate fails). Instead of requiring a manual
/// unpair + re-pair, this function probes the server's pairing-port `/info`
/// endpoint (which now reports the server's Tailscale DNS name) and
/// persists it.
///
/// Trigger condition: paired, `tailscale` is `None`, `lan` is present, and
/// `ports.vocab` is present (so the server is new enough to have a vocab /
/// content-sync port). Skips silently otherwise.
///
/// All failure paths return `Ok(())` — this is best-effort and must never
/// block app startup or the downstream sync attempt. The manual re-pair
/// path remains the fallback if the probe can't reach the server.
pub async fn backfill_tailscale() -> AppResult<()> {
    let Some(conn) = crate::state::load_paired_connection() else {
        return Ok(()); // Not paired — nothing to backfill.
    };
    // Already have a Tailscale address → no-op.
    if conn.tailscale.is_some() {
        return Ok(());
    }
    // Need a LAN host to probe and a vocab port (new enough server).
    let Some(lan) = &conn.lan else {
        return Ok(());
    };
    let Some(vocab_port) = conn.ports.vocab else {
        return Ok(());
    };

    tracing::info!(
        lan = %lan,
        vocab_port,
        "backfill: probing server /info to fetch its Tailscale DNS name"
    );

    // Short-timeout probe matching the discovery pattern. The pairing port
    // (11436) hosts the unauthenticated /info endpoint.
    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "backfill: failed to build HTTP client");
            return Ok(());
        }
    };

    let pairing_port = conn.ports.pairing; // 11436 (required field, not Option)
    let base = medical_core::types::endpoint::http_url(lan, pairing_port);
    let url = format!("{base}/info");

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "backfill: /info probe failed (non-fatal)");
            return Ok(());
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "backfill: /info returned non-200");
        return Ok(());
    }

    // Minimal deserialization — we only need the tailscale field. Older
    // servers omit it entirely (serde default → None).
    #[derive(serde::Deserialize)]
    struct InfoTailscale {
        #[serde(default)]
        tailscale: Option<String>,
    }
    let info: InfoTailscale = match resp.json().await {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(error = %e, "backfill: failed to parse /info JSON");
            return Ok(());
        }
    };

    let Some(ts_name) = info.tailscale else {
        tracing::info!(
            "backfill: server /info did not report a Tailscale name (pre-0.30.5 server?)"
        );
        return Ok(());
    };

    // Persist the updated connection.
    let mut updated = conn.clone();
    updated.tailscale = Some(ts_name.clone());
    let json = serde_json::to_string(&updated)?;
    let path = paired_connection_path()?;
    std::fs::write(&path, json)?;
    tracing::info!(
        tailscale = %ts_name,
        "backfill: successfully saved Tailscale address to paired connection"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Tests for `try_pair_at_base` and the pure pair-flow helpers. The
    //! full command flow has its own coverage in `command_tests` below
    //! (wiremock office server + mocked keychain + redirected app-data
    //! dir); this module keeps the unit tests for the pieces.
    use super::*;

    /// Bind a TCP listener to grab an ephemeral port, then drop the listener so
    /// the port is free again. The OS will return ECONNREFUSED for the next
    /// connect against that port — exactly the "LAN unreachable" condition we
    /// need to exercise.
    fn closed_port_url() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn try_pair_at_base_returns_connect_error_for_closed_port() {
        // A closed port surfaces as a connect-level reqwest error — the kind
        // that should trigger Tailscale fallback. If this discrimination
        // breaks, the user-facing bug returns (LAN failure becomes a fatal
        // error instead of a fallback).
        let http = reqwest::Client::new();
        let base = closed_port_url();
        let body = serde_json::json!({ "code": "x", "label": "y" });

        let result = try_pair_at_base(&http, &base, &body).await;
        match result {
            Err(PairAttemptError::Connect(_)) => {} // expected — retryable
            Err(PairAttemptError::Final(e)) => {
                panic!("expected Connect (retryable), got Final: {e:?}");
            }
            Ok(_) => panic!("expected error from closed port"),
        }
    }

    fn ports(lmstudio: Option<u16>, omlx: Option<u16>) -> PairPorts {
        PairPorts {
            ollama: 11435,
            whisper: 8081,
            pairing: 11436,
            lmstudio,
            omlx,
            vocab: Some(11437),
        }
    }

    #[test]
    fn served_providers_reflects_advertised_ports() {
        // Ollama-only server (LM Studio / oMLX never came ready).
        assert_eq!(served_providers(&ports(None, None)), vec!["ollama"]);
        // All three advertised.
        assert_eq!(
            served_providers(&ports(Some(1235), Some(8001))),
            vec!["ollama", "lmstudio", "omlx"]
        );
        // oMLX-only (Apple Silicon office without LM Studio).
        assert_eq!(
            served_providers(&ports(None, Some(8001))),
            vec!["ollama", "omlx"]
        );
    }

    #[test]
    fn provider_proxy_port_maps_ids_to_advertised_ports() {
        let p = ports(Some(1235), Some(8001));
        assert_eq!(provider_proxy_port(&p, "ollama"), Some(11435));
        assert_eq!(provider_proxy_port(&p, "lmstudio"), Some(1235));
        assert_eq!(provider_proxy_port(&p, "omlx"), Some(8001));
        assert_eq!(provider_proxy_port(&p, "unknown"), None);
        let none = ports(None, None);
        assert_eq!(provider_proxy_port(&none, "lmstudio"), None);
        assert_eq!(provider_proxy_port(&none, "omlx"), None);
    }

    /// Regression: an unbracketed IPv6 host used to make reqwest fail URL
    /// parsing, so the availability probe silently never succeeded.
    #[test]
    fn provider_probe_url_brackets_ipv6_literals() {
        assert_eq!(
            provider_probe_url("fe80::1", 8001, "omlx"),
            "http://[fe80::1]:8001/v1/models"
        );
        assert_eq!(
            provider_probe_url("2001:db8::a:1", 11435, "ollama"),
            "http://[2001:db8::a:1]:11435/api/tags"
        );
        // Plain hostnames and IPv4 are untouched.
        assert_eq!(
            provider_probe_url("clinic.local", 1235, "lmstudio"),
            "http://clinic.local:1235/v1/models"
        );
        assert_eq!(
            provider_probe_url("192.168.1.9", 11435, "ollama"),
            "http://192.168.1.9:11435/api/tags"
        );
    }

    #[test]
    fn refreshed_model_replaces_model_not_offered_by_server() {
        let offered = vec![
            "mlx-community--Qwen2.5-0.5B-Instruct-4bit".to_string(),
            "Ornith-1.5-35B-A3B-MLX-4bit".to_string(),
        ];
        // Regression: a kept provider with a placeholder saved as ai_model
        // 404s every generation after re-pairing.
        assert_eq!(
            refreshed_model("default", &offered),
            Some("mlx-community--Qwen2.5-0.5B-Instruct-4bit".to_string())
        );
        // A model left over from another server/provider isn't offered.
        assert_eq!(
            refreshed_model("llama3:8b", &offered),
            Some("mlx-community--Qwen2.5-0.5B-Instruct-4bit".to_string())
        );
    }

    // A model that happens to share a name with the old fallback ids is a
    // REAL server model — filtering by name would shadow it and override
    // the user's explicit choice.
    #[test]
    fn refreshed_model_respects_offered_model_named_like_old_placeholder() {
        let offered = vec!["llama3".to_string(), "llama3.1:8b".to_string()];
        assert_eq!(refreshed_model("llama3", &offered), None);
    }

    #[test]
    fn refreshed_model_keeps_valid_saved_choice() {
        let offered = vec![
            "mlx-community--Qwen2.5-0.5B-Instruct-4bit".to_string(),
            "Ornith-1.5-35B-A3B-MLX-4bit".to_string(),
        ];
        assert_eq!(
            refreshed_model("Ornith-1.5-35B-A3B-MLX-4bit", &offered),
            None,
            "an explicitly chosen offered model must be respected"
        );
    }

    #[test]
    fn refreshed_model_no_ops_when_nothing_offered() {
        // Defensive: available_models errors on an empty list, so this only
        // guards the pure function's contract.
        assert_eq!(refreshed_model("anything", &[]), None);
    }

    #[tokio::test]
    async fn probe_provider_proxy_hits_provider_specific_path_with_bearer() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer fixture-auth-value"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "qwen3-8b"}]
            })))
            .expect(1)
            .mount(&srv)
            .await;
        let parsed: reqwest::Url = srv.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let http = reqwest::Client::new();
        assert!(probe_provider_proxy(&http, &host, port, "omlx", "fixture-auth-value").await);
        srv.verify().await;
    }
}

#[cfg(test)]
mod command_tests {
    //! Full-flow coverage for `pair_with_server_inner` against a wiremock
    //! office server: handshake, keychain mirror, provider availability
    //! switch, model refresh, and AppConfig persistence. Machine-global
    //! side effects are contained — the OS-keychain bearer write is a
    //! #[cfg(test)] no-op (`store_sharing_bearer`), and the
    //! paired-connection file lands in the per-process test tempdir
    //! (`super::super::test_app_data_dir`).
    use super::super::{paired_connection_path, test_app_data_guard};
    use super::*;
    use crate::commands::generation::test_helpers::build_test_state_with_provider;
    use medical_core::types::settings::{AppConfig, SttMode};
    use medical_db::settings::SettingsRepo;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Neutral fixture token the mock enroll endpoint hands out.
    const FIXTURE_TOKEN: &str = "pair-flow-fixture-42";

    /// Bind an ephemeral port and drop the listener — connects against it
    /// fail instantly with ECONNREFUSED, standing in for an unbound proxy.
    fn closed_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        port
    }

    /// A wiremock "office server": /pair/enroll hands out the fixture
    /// token, /v1/models serves `models`. The caller must keep the
    /// returned server alive for the duration of the test (a dropped
    /// MockServer can unmount its mocks mid-request).
    async fn office_server(models: &[&str]) -> (MockServer, u16) {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/pair/enroll"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token": FIXTURE_TOKEN
            })))
            .mount(&srv)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": models.iter().map(|m| serde_json::json!({ "id": m })).collect::<Vec<_>>()
            })))
            .mount(&srv)
            .await;
        let port = srv
            .uri()
            .parse::<reqwest::Url>()
            .expect("url")
            .port()
            .expect("port");
        (srv, port)
    }

    fn ports(pairing: u16, lmstudio: Option<u16>, omlx: Option<u16>) -> PairPorts {
        PairPorts {
            ollama: closed_port(), // never answers in these scenarios
            whisper: 8081,
            pairing,
            lmstudio,
            omlx,
            vocab: None,
        }
    }

    /// State whose registry holds ONE oMLX provider statically pointed at
    /// the mock office server (so post-pair model refreshes hit its
    /// /v1/models), plus the config fields the flow reads.
    async fn client_state(
        srv_port: u16,
        ai_provider: &str,
        ai_model: &str,
    ) -> crate::state::AppState {
        let mut config = AppConfig::default();
        config.ai_provider = ai_provider.to_string();
        config.ai_model = ai_model.to_string();
        let base = format!("http://127.0.0.1:{srv_port}");
        let provider = medical_ai_providers::omlx::OmlxProvider::new(
            Some(base.as_str()),
            false,
            None,
            medical_ai_providers::http_client::RetryConfig {
                max_retries: 0,
                ..Default::default()
            },
        )
        .expect("build omlx provider");
        let (mut state, _recording_id) = build_test_state_with_provider(
            config,
            "transcript text",
            std::sync::Arc::new(provider) as std::sync::Arc<dyn medical_core::traits::AiProvider>,
        )
        .await;
        // The pair flow initializes STT providers under data_dir; point it
        // at a real tempdir so those reads don't fail with NotFound.
        let dir = tempfile::tempdir().expect("stt tempdir");
        state.data_dir = dir.path().to_path_buf();
        std::mem::forget(dir);
        state
    }

    async fn load_cfg(state: &crate::state::AppState) -> AppConfig {
        let conn = state.db.conn().expect("conn");
        let mut cfg = SettingsRepo::load_config(&conn).expect("load config");
        cfg.migrate();
        cfg
    }

    #[tokio::test]
    async fn pair_switches_to_the_only_answering_provider_and_picks_its_model() {
        let _guard = test_app_data_guard().await;
        let (_server, srv_port) = office_server(&["Ornith-1.5-35B", "Qwen-4B"]).await;
        let state = client_state(srv_port, "lmstudio", "llama3:8b").await;

        // Only the oMLX proxy answers; LM Studio's port is closed.
        pair_with_server_inner(
            &state,
            Some("127.0.0.1".into()),
            None,
            ports(srv_port, Some(closed_port()), Some(srv_port)),
            "123456".into(),
            "Test Client".into(),
        )
        .await
        .expect("pair succeeds");

        let cfg = load_cfg(&state).await;
        assert_eq!(
            cfg.ai_provider, "omlx",
            "switched to the answering provider"
        );
        assert_eq!(cfg.ai_model, "Ornith-1.5-35B", "first offered model picked");
        assert_eq!(cfg.omlx_host, "127.0.0.1");
        assert_eq!(cfg.omlx_port, srv_port);
        assert_eq!(
            cfg.stt_mode,
            SttMode::Remote,
            "STT routed through the office"
        );

        // Bearer mirrored into the per-service keychain slots (state.keys
        // is the same file-backed KeyStorage the flow writes).
        assert_eq!(
            state.keys.get_key("omlx_api_key").expect("key read"),
            Some(FIXTURE_TOKEN.to_string())
        );
        assert!(paired_connection_path().expect("path").exists());
    }

    #[tokio::test]
    async fn pair_keeps_answering_provider_and_replaces_stale_saved_model() {
        let _guard = test_app_data_guard().await;
        let (_server, srv_port) = office_server(&["Ornith-1.5-35B", "Qwen-4B"]).await;
        // Saved model is the old placeholder id — not in the server's list.
        let state = client_state(srv_port, "omlx", "default").await;

        pair_with_server_inner(
            &state,
            Some("127.0.0.1".into()),
            None,
            ports(srv_port, None, Some(srv_port)),
            "123456".into(),
            "Test Client".into(),
        )
        .await
        .expect("pair succeeds");

        let cfg = load_cfg(&state).await;
        assert_eq!(cfg.ai_provider, "omlx", "answering provider kept");
        assert_eq!(
            cfg.ai_model, "Ornith-1.5-35B",
            "stale model replaced with the first offered one"
        );
    }

    #[tokio::test]
    async fn pair_respects_saved_model_when_the_server_actually_offers_it() {
        // Regression pin (2026-09-02): a REAL server model that happens to
        // share a name with the old placeholder ids must not be shadowed.
        let _guard = test_app_data_guard().await;
        let (_server, srv_port) = office_server(&["default", "Ornith-1.5-35B"]).await;
        let state = client_state(srv_port, "omlx", "default").await;

        pair_with_server_inner(
            &state,
            Some("127.0.0.1".into()),
            None,
            ports(srv_port, None, Some(srv_port)),
            "123456".into(),
            "Test Client".into(),
        )
        .await
        .expect("pair succeeds");

        let cfg = load_cfg(&state).await;
        assert_eq!(
            cfg.ai_model, "default",
            "an offered model is respected, even one named like the old placeholder"
        );
    }
}
