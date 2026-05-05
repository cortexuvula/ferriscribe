use std::sync::Arc;

use medical_sharing::mdns::DiscoveredServer;
use medical_sharing::qr::{PairPayload, PairPorts, encode};
use medical_sharing::{SharingConfig, SharingService, SharingStatus};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct SharingStatusDto {
    pub enabled: bool,
    pub ollama_ok: bool,
    pub whisper_ok: bool,
    pub mdns_ok: bool,
    pub pairing_ok: bool,
    pub paired_clients: u32,
}

impl From<SharingStatus> for SharingStatusDto {
    fn from(s: SharingStatus) -> Self {
        Self {
            enabled: s.enabled,
            ollama_ok: s.ollama_ok,
            whisper_ok: s.whisper_ok,
            mdns_ok: s.mdns_ok,
            pairing_ok: s.pairing_ok,
            paired_clients: s.paired_clients,
        }
    }
}

/// Non-secret connection metadata persisted across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedConnection {
    pub lan: Option<String>,
    pub tailscale: Option<String>,
    pub ports: PairPorts,
    pub label: String,
}

fn paired_connection_path() -> Result<std::path::PathBuf, String> {
    let app_data = dirs::data_dir()
        .ok_or_else(|| "no app data dir".to_string())?
        .join("rust-medical-assistant");
    std::fs::create_dir_all(&app_data).map_err(|e| e.to_string())?;
    Ok(app_data.join("sharing-paired.json"))
}

#[tauri::command]
pub async fn start_sharing(
    state: State<'_, AppState>,
    friendly_name: String,
) -> Result<(), String> {
    // Acquire the write lock BEFORE binding ports / spawning proxies so that a
    // concurrent stop_sharing cannot return Ok while we are mid-start and leave
    // the service running with no future cleanup path.
    let mut sharing_slot = state.sharing.write().await;
    if sharing_slot.is_some() {
        return Err("sharing already running".to_string());
    }
    let cfg = build_sharing_config(&state, friendly_name)
        .await
        .map_err(|e| e.to_string())?;
    let service = Arc::new(SharingService::new(cfg).map_err(|e| e.to_string())?);
    service.start().await.map_err(|e| e.to_string())?;
    *sharing_slot = Some(service);

    // Wire up the persistent Ollama service the wizard promises ("FerriScribe
    // will configure persistent Ollama..."). Idempotent: skip when already
    // installed; surface install failures via warn rather than aborting
    // sharing so that a missing ollama binary or write-protected
    // LaunchAgents directory doesn't block the in-process services.
    use medical_sharing::service_installer::{
        install_persistent_ollama, ollama_service_state, ServiceState,
    };
    if matches!(ollama_service_state(), ServiceState::Missing) {
        // install_persistent_ollama logs its own outcome (installed vs.
        // skipped because port 11434 is already bound externally). Only log
        // here on hard failure.
        if let Err(e) = install_persistent_ollama() {
            tracing::warn!("persistent ollama install failed: {e}");
        }
    }

    // Heavy-box routing: this machine IS the office server, so route AI/STT
    // calls to the upstream services on localhost directly — no proxy hop, no
    // bearer needed. Ports are the upstream ports (Ollama 11434, LM Studio 1234,
    // whisper.cpp 8080), NOT the proxy ports (11435 / 8081).
    use medical_core::types::RemoteEndpoint;
    let local_ollama = Some(RemoteEndpoint {
        lan: Some("127.0.0.1".to_string()),
        tailscale: None,
        port: 11434,
        bearer: None,
    });
    let local_lmstudio = Some(RemoteEndpoint {
        lan: Some("127.0.0.1".to_string()),
        tailscale: None,
        port: 1234,
        bearer: None,
    });
    let local_whisper = Some(RemoteEndpoint {
        lan: Some("127.0.0.1".to_string()),
        tailscale: None,
        port: 8080,
        bearer: None,
    });

    {
        let guard = state.ollama_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(local_ollama).await;
        }
    }
    {
        let guard = state.lmstudio_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(local_lmstudio).await;
        }
    }
    {
        let guard = state.remote_stt_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(local_whisper).await;
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn stop_sharing(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(s) = state.sharing.write().await.take() {
        s.stop().await.map_err(|e| e.to_string())?;
    }

    // Restore provider endpoints to pre-sharing configuration.
    // If this machine is also paired as a client to another server, restore the
    // paired endpoint; otherwise revert to None (local-only mode).
    let paired = crate::state::load_paired_connection();
    let bearer = if paired.is_some() { crate::state::load_sharing_bearer() } else { None };

    use medical_core::types::RemoteEndpoint;
    let (ollama_ep, lmstudio_ep, whisper_ep) = if let Some(ref p) = paired {
        (
            Some(RemoteEndpoint { lan: p.lan.clone(), tailscale: p.tailscale.clone(), port: p.ports.ollama, bearer: bearer.clone() }),
            p.ports.lmstudio.map(|lp| RemoteEndpoint { lan: p.lan.clone(), tailscale: p.tailscale.clone(), port: lp, bearer: bearer.clone() }),
            Some(RemoteEndpoint { lan: p.lan.clone(), tailscale: p.tailscale.clone(), port: p.ports.whisper, bearer }),
        )
    } else {
        (None, None, None)
    };

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
    {
        let guard = state.remote_stt_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(whisper_ep).await;
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn sharing_status(state: State<'_, AppState>) -> Result<SharingStatusDto, String> {
    if let Some(s) = state.sharing.read().await.as_ref() {
        Ok(s.status().await.into())
    } else {
        Ok(SharingStatusDto {
            enabled: false,
            ollama_ok: false,
            whisper_ok: false,
            mdns_ok: false,
            pairing_ok: false,
            paired_clients: 0,
        })
    }
}

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
        },
        code,
    };
    Ok(encode(&payload))
}

#[derive(Debug, Serialize)]
pub struct ClientDto {
    pub id: i64,
    pub label: String,
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
pub async fn discover_servers(timeout_ms: u64) -> Result<Vec<DiscoveredServer>, String> {
    let mut rx =
        medical_sharing::mdns::browse(std::time::Duration::from_millis(timeout_ms))
            .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(d) = rx.recv().await {
        out.push(d);
    }
    Ok(out)
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
    {
        let guard = state.remote_stt_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(whisper_ep).await;
        }
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

async fn build_sharing_config(
    _state: &AppState,
    friendly_name: String,
) -> Result<SharingConfig, String> {
    use medical_security::keychain;
    use rand::RngCore;

    // Reuse the SQLCipher DB key as the sharing-store key — same keychain
    // entry, no new secret to manage.
    let key = keychain::get_db_key()
        .map_err(|e| format!("Keychain access denied: {e}. Sharing requires keychain access — quit and reopen FerriScribe, then approve the keychain prompt."))?
        .ok_or_else(|| {
            "FerriScribe's database hasn't been initialized yet. Restart the app and try again.".to_string()
        })?;

    let app_data = dirs::data_dir()
        .ok_or_else(|| "no app data dir".to_string())?
        .join("rust-medical-assistant");
    std::fs::create_dir_all(&app_data).map_err(|e| e.to_string())?;
    let mut whisper_api = [0u8; 16];
    rand::thread_rng()
        .try_fill_bytes(&mut whisper_api)
        .map_err(|e| e.to_string())?;
    // Only wire up an LM Studio proxy when LM Studio's local server is
    // actually running. If the user starts LM Studio after Start sharing,
    // they'll need to Stop + Start sharing to wire up the proxy.
    let lmstudio_internal = lmstudio_running_port().await;
    Ok(SharingConfig {
        enabled: true,
        friendly_name,
        ollama_proxy_port: 11435,
        whisper_proxy_port: 8081,
        pairing_port: 11436,
        whisper_internal_port: 8080,
        lmstudio_internal_port: lmstudio_internal,
        lmstudio_proxy_port: lmstudio_internal.map(|_| 1235),
        token_store_path: app_data.join("sharing.db"),
        token_store_key: key,
        binary_dir: app_data.join("bin"),
        whisper_model_path: app_data.join("models/whisper/ggml-large-v3-turbo.bin"),
        whisper_internal_api_key: hex::encode(whisper_api),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
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

async fn lmstudio_running_port() -> Option<u16> {
    let resp = reqwest::Client::new()
        .get("http://127.0.0.1:1234/v1/models")
        .timeout(std::time::Duration::from_millis(300))
        .send()
        .await
        .ok()?;
    if resp.status().is_success() {
        Some(1234)
    } else {
        None
    }
}
