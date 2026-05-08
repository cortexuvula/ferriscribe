//! Pairing flow: QR generation + client list/revoke (server side), plus the
//! client-side `pair_with_server` / `paired_endpoint` / `unpair` commands.

use medical_sharing::qr::{encode, PairPayload, PairPorts};
use tauri::State;

use crate::state::AppState;

use super::{paired_connection_path, ClientDto, PairedConnection};

#[tauri::command]
pub async fn pairing_qr(state: State<'_, AppState>) -> Result<String, String> {
    let svc = state.sharing.read().await;
    let svc = svc.as_ref().ok_or("sharing not running")?;
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
            vocab: Some(cfg.vocab_port),
        },
        code,
    };
    Ok(encode(&payload))
}

#[tauri::command]
pub async fn list_paired_clients(state: State<'_, AppState>) -> Result<Vec<ClientDto>, String> {
    let svc = state.sharing.read().await;
    let svc = svc.as_ref().ok_or("sharing not running")?;
    let rows = svc.token_store().list().map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| ClientDto {
            id: r.id,
            label: r.label,
        })
        .collect())
}

#[tauri::command]
pub async fn revoke_client(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let svc = state.sharing.read().await;
    let svc = svc.as_ref().ok_or("sharing not running")?;
    svc.token_store().revoke(id).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn rename_client(
    state: State<'_, AppState>,
    id: i64,
    label: String,
) -> Result<(), String> {
    if label.trim().is_empty() {
        return Err("label cannot be empty".into());
    }
    let svc = state.sharing.read().await;
    let svc = svc.as_ref().ok_or("sharing not running")?;
    svc.token_store()
        .update_label(id, &label)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn suggested_client_label() -> String {
    medical_sharing::suggested_label::suggested_client_label()
}

/// Pair with an office server: POST the enroll code, receive a bearer token,
/// persist the token in the OS keychain, and persist the non-secret endpoint
/// metadata to disk. Returns nothing to the frontend — no raw token is ever
/// sent to JS.
///
/// After persisting, the in-memory Ollama, LM Studio, and remote-STT providers
/// are updated immediately so the "models visible" success message in the UI is
/// truthful without requiring an app restart.
#[tauri::command]
pub async fn pair_with_server(
    state: State<'_, AppState>,
    lan: Option<String>,
    tailscale: Option<String>,
    ports: PairPorts,
    code: String,
    label: String,
) -> Result<(), String> {
    // Prefer LAN address; fall back to Tailscale. http_url brackets IPv6
    // literals — without it, an mDNS-discovered IPv6 address makes reqwest
    // emit a generic "Builder error" with no URL context.
    let base = if let Some(ref l) = lan {
        medical_core::types::http_url(l, ports.pairing)
    } else if let Some(ref ts) = tailscale {
        medical_core::types::http_url(ts, ports.pairing)
    } else {
        return Err("no reachable address provided".into());
    };

    let body = serde_json::json!({ "code": code, "label": label });
    let resp = reqwest::Client::new()
        .post(format!("{base}/pair/enroll"))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("server rejected pair: {}", resp.status()));
    }

    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let token = v
        .get("token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "server did not return a token".to_string())?
        .to_string();

    // Store bearer token in OS keychain.
    keyring::Entry::new("rustMedicalAssistant", "sharing-bearer")
        .map_err(|e| format!("keychain open: {e}"))?
        .set_password(&token)
        .map_err(|e| format!("keychain write: {e}"))?;

    // Persist non-secret endpoint metadata.
    let conn = PairedConnection { lan: lan.clone(), tailscale: tailscale.clone(), ports: ports.clone(), label };
    let json = serde_json::to_string(&conn).map_err(|e| e.to_string())?;
    let path = paired_connection_path()?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;

    // Update in-memory provider endpoints immediately so the "models visible"
    // success message in ClientPair.svelte is truthful without an app restart.
    use medical_core::types::RemoteEndpoint;
    let bearer = Some(token);

    let ollama_ep = Some(RemoteEndpoint {
        lan: lan.clone(),
        tailscale: tailscale.clone(),
        port: ports.ollama,
        bearer: bearer.clone(),
    });
    let lmstudio_ep = ports.lmstudio.map(|lp| RemoteEndpoint {
        lan: lan.clone(),
        tailscale: tailscale.clone(),
        port: lp,
        bearer: bearer.clone(),
    });
    let whisper_ep = Some(RemoteEndpoint {
        lan: lan.clone(),
        tailscale: tailscale.clone(),
        port: ports.whisper,
        bearer: bearer.clone(),
    });

    {
        let guard = state.ollama_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(ollama_ep).await;
        }
    }
    {
        let guard = state.lmstudio_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(lmstudio_ep).await;
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
        let conn = state.db.conn().map_err(|e| e.to_string())?;
        let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| e.to_string())?;
        cfg.migrate();
        if cfg.stt_mode != SttMode::Remote {
            cfg.stt_mode = SttMode::Remote;
            medical_db::settings::SettingsRepo::save_config(&conn, &cfg)
                .map_err(|e| e.to_string())?;
            tracing::info!("pair: switched stt_mode to Remote");
        }
        let stt_handles = crate::state::init_stt_providers_with_config(
            &state.data_dir,
            &cfg,
            whisper_ep.clone(),
        );
        {
            let mut guard = state.stt_providers.lock().await;
            *guard = stt_handles.provider;
        }
        *state.remote_stt_provider.write().await = stt_handles.remote;
    }

    Ok(())
}

/// Returns the saved paired-connection metadata, or `None` if not paired.
#[tauri::command]
pub async fn paired_endpoint() -> Result<Option<PairedConnection>, String> {
    let path = paired_connection_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let conn: PairedConnection = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    Ok(Some(conn))
}

/// Remove the keychain entry and the on-disk metadata. Idempotent.
#[tauri::command]
pub async fn unpair() -> Result<(), String> {
    // Remove keychain entry (ignore NoEntry).
    if let Ok(entry) = keyring::Entry::new("rustMedicalAssistant", "sharing-bearer") {
        let _ = entry.delete_credential();
    }

    // Remove the metadata file (ignore not-found).
    let path = paired_connection_path()?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
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
    let out = tokio::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    medical_sharing::tailscale::parse_self_dns_name(&out.stdout)
}
