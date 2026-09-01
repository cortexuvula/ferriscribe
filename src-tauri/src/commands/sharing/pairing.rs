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
/// If the client's current `ai_provider` isn't served by this server (e.g. a
/// fresh install defaults to `"lmstudio"` but the office runs Ollama only),
/// the pair flow probes each advertised provider proxy and switches to the
/// first one that answers — pairing succeeds as long as ANY of Ollama, LM
/// Studio, or oMLX is available.
#[tauri::command]
pub async fn pair_with_server(
    state: State<'_, AppState>,
    lan: Option<String>,
    tailscale: Option<String>,
    ports: PairPorts,
    code: String,
    label: String,
) -> AppResult<()> {
    // The QR encodes BOTH LAN and Tailscale addresses; a remote client over
    // Tailscale cannot reach the office LAN IP. Try LAN first, and on a
    // connect-level failure (TCP refused, DNS unresolved, timeout) fall back
    // to Tailscale exactly once. HTTP-level rejections (4xx/5xx) are NOT
    // retried — those are real server-side responses, not connectivity.
    //
    // http_url brackets IPv6 literals — without it, an mDNS-discovered IPv6
    // address makes reqwest emit a generic "Builder error" with no URL context.
    let body = serde_json::json!({ "code": code, "label": label });

    // Track which host actually answered the pair handshake. The QR carries
    // both LAN and Tailscale, but a remote client may only reach the latter;
    // downstream AppConfig autofill must use the reachable address, not just
    // whichever one happened to be present in the QR.
    let (winning_host, v): (String, serde_json::Value) = match (lan.as_ref(), tailscale.as_ref()) {
        (Some(l), ts_opt) => {
            let lan_base = medical_core::types::http_url(l, ports.pairing);
            tracing::info!(host = %l, port = ports.pairing, "pair: trying LAN");
            match try_pair_at_base(&state.http_client, &lan_base, &body).await {
                Ok(v) => (l.clone(), v),
                Err(PairAttemptError::Connect(_)) => {
                    if let Some(ts) = ts_opt {
                        tracing::info!(
                            host = %ts,
                            port = ports.pairing,
                            "pair: LAN unreachable, falling back to Tailscale"
                        );
                        let ts_base = medical_core::types::http_url(ts, ports.pairing);
                        match try_pair_at_base(&state.http_client, &ts_base, &body).await {
                            Ok(v) => (ts.clone(), v),
                            Err(PairAttemptError::Connect(e)) => {
                                return Err(AppError::Other(e.to_string()));
                            }
                            Err(PairAttemptError::Final(e)) => return Err(e),
                        }
                    } else {
                        // No Tailscale fallback available — surface the
                        // LAN connect failure as a normal AppError.
                        return Err(AppError::Other(
                            "could not connect to server (LAN unreachable, no Tailscale address)"
                                .into(),
                        ));
                    }
                }
                Err(PairAttemptError::Final(e)) => return Err(e),
            }
        }
        (None, Some(ts)) => {
            let ts_base = medical_core::types::http_url(ts, ports.pairing);
            tracing::info!(host = %ts, port = ports.pairing, "pair: trying Tailscale");
            match try_pair_at_base(&state.http_client, &ts_base, &body).await {
                Ok(v) => (ts.clone(), v),
                Err(PairAttemptError::Connect(e)) => {
                    return Err(AppError::Other(e.to_string()));
                }
                Err(PairAttemptError::Final(e)) => return Err(e),
            }
        }
        (None, None) => {
            return Err(AppError::Other("no reachable address provided".into()));
        }
    };

    let token = v
        .get("token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::Other("server did not return a token".into()))?
        .to_string();

    // Store bearer token in OS keychain.
    keyring::Entry::new("rustMedicalAssistant", "sharing-bearer")
        .map_err(|e| AppError::Other(format!("keychain open: {e}")))?
        .set_password(&token)
        .map_err(|e| AppError::Other(format!("keychain write: {e}")))?;

    // Persist non-secret endpoint metadata.
    let conn = PairedConnection {
        lan: lan.clone(),
        tailscale: tailscale.clone(),
        ports: ports.clone(),
        label,
    };
    let json = serde_json::to_string(&conn)?;
    let path = paired_connection_path()?;
    std::fs::write(&path, json)?;

    // Update in-memory provider endpoints immediately so the "models visible"
    // success message in ClientPair.svelte is truthful without an app restart.
    let pair_cfg = crate::commands::load_app_config(&state.db, "pairing").await?;
    let allow_public = pair_cfg.allow_public_endpoint;
    let bearer = Some(token.clone());
    let eps = super::paired_endpoints(&conn, bearer);

    {
        let guard = state.ollama_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(eps.ollama, allow_public).await?;
        }
    }
    {
        let guard = state.lmstudio_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(eps.lmstudio, allow_public).await?;
        }
    }
    {
        let guard = state.omlx_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(eps.omlx, allow_public).await?;
        }
    }

    // STT requires more than set_endpoint: if the user was in Local mode at
    // app startup, state.remote_stt_provider is None and set_endpoint would
    // be a no-op. Persist `stt_mode = Remote` and rebuild the STT provider
    // so transcription routes through the office server's whisper proxy —
    // otherwise the user hits "Whisper model not found" because the local
    // provider is still the active one.
    {
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

        let stt_handles = crate::state::init_stt_providers_with_config(
            &state.data_dir,
            &cfg,
            eps.whisper.clone(),
        );
        {
            let mut guard = state.stt_providers.lock().await;
            *guard = stt_handles.provider;
        }
        *state.remote_stt_provider.write().await = stt_handles.remote;
    }

    // ── Availability-aware provider selection ──
    //
    // A fresh client defaults to ai_provider = "lmstudio". If the server
    // doesn't serve that provider (its proxy port is advertised only when
    // the upstream is actually ready), generation would point at a dead
    // endpoint even though the server happily serves Ollama or oMLX —
    // looking exactly like "the client won't connect". Check all three
    // providers: when the current one isn't served, probe each advertised
    // proxy with the fresh token and switch to the first that answers.
    let served = served_providers(&ports);
    let mut chosen_provider: Option<String> = None;
    let mut chosen_model: Option<String> = None;
    if !served.contains(&pair_cfg.ai_provider.as_str()) {
        for cand in served {
            let Some(proxy_port) = provider_proxy_port(&ports, cand) else {
                continue;
            };
            if !probe_provider_proxy(&state.http_client, &winning_host, proxy_port, cand, &token)
                .await
            {
                tracing::info!(
                    provider = cand,
                    "pair: provider proxy not answering; skipping"
                );
                continue;
            }
            chosen_provider = Some(cand.to_string());
            break;
        }
    }

    // Best-effort model selection for a switched provider: the old
    // provider's model name likely doesn't exist on the new one. Ask the
    // (already re-endpointed) provider for its list and take the first.
    if let Some(ref provider_id) = chosen_provider {
        let arc = {
            let registry = state.ai_providers.lock().await;
            registry.get_arc(provider_id)
        };
        if let Some(provider) = arc
            && let Ok(models) = provider.available_models().await
            && let Some(first) = models.first()
        {
            chosen_model = Some(first.id.clone());
        }
    }

    // ── Phase 3: per-service keychain mirror + AppConfig population ──
    //
    // The bearer above is stored at keyring "rustMedicalAssistant"/"sharing-bearer"
    // (used by the in-memory provider path). The rest of the app — Settings UI,
    // pre-flight, endpointHealth polling — reads from per-service keychain slots
    // and AppConfig host/port fields. Mirror the bearer here so paired clients
    // don't need to manually fill in Settings → Audio / Models.
    {
        use super::settings_helpers::apply_paired_settings;

        // 1. Use the host that actually answered the pair handshake above.
        //    The in-memory RemoteEndpoint still carries BOTH LAN and Tailscale
        //    and probes both at call time; but the static AppConfig field has
        //    to be a single address that the client can actually reach. Using
        //    `winning_host` ensures a remote-paired client doesn't get the
        //    server's unreachable LAN IP written into Settings (which would
        //    poison pre-flight checks and health polling that read AppConfig
        //    host fields directly).
        let host = winning_host;

        // 2. Write the bearer to per-service keychain slots via state.keys.
        //    Same KeyStorage abstraction the set_api_key Tauri command uses.
        for slot in &[
            "stt_remote_api_key",
            "ollama_api_key",
            "lmstudio_api_key",
            "omlx_api_key",
        ] {
            state
                .keys
                .store_key(slot, &token)
                .map_err(|e| AppError::Other(format!("autofill: store {slot}: {e}")))?;
        }

        // 3. Update AppConfig with the paired endpoint values (and the
        //    availability-selected provider, when a switch happened).
        //    Wrapped in spawn_blocking so the SQLite read-modify-write never
        //    blocks the async runtime worker.
        let db = std::sync::Arc::clone(&state.db);
        let host_for_db = host.clone();
        let ports_for_db = ports.clone();
        let provider_for_db = chosen_provider.clone();
        let model_for_db = chosen_model.clone();
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
                if let Some(m) = model_for_db {
                    cfg.ai_model = m;
                }
            }
            medical_db::settings::SettingsRepo::save_config(&conn, &cfg)
                .map_err(|e| AppError::Other(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(crate::commands::join_err)??;

        // 4. Flip the live registry's active provider to match, so generation
        //    uses the served provider immediately (no reinit needed).
        if let Some(ref p) = chosen_provider {
            let mut registry = state.ai_providers.lock().await;
            if registry.set_active(p) {
                tracing::info!(provider = %p, "pair: active provider switched to served provider");
            }
        }

        tracing::info!(
            host = %host,
            whisper_port = ports.whisper,
            ollama_port = ports.ollama,
            lmstudio_port = ?ports.lmstudio,
            omlx_port = ?ports.omlx,
            "pair: populated per-service api_keys and AppConfig host/ports"
        );
    }

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
    let path = if provider == "ollama" {
        "/api/tags"
    } else {
        "/v1/models"
    };
    let url = format!("http://{host}:{port}{path}");
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
    //! Tests for `try_pair_at_base`. The full `pair_with_server` command depends
    //! on Tauri `State` and the keychain, which are awkward to fake — but the
    //! retry-discrimination logic lives entirely in the helper, so unit-testing
    //! the helper covers the bug fix.
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
