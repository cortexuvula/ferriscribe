//! Office-server lifecycle: start / stop / status. Owns the heavy
//! `start_sharing_inner` body and the `build_sharing_config` builder.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_sharing::{SharingConfig, SharingService};
use tauri::State;

use crate::state::AppState;

use super::{
    delete_server_config, write_server_config, ServerConfig, SharingStatusDto,
};

#[tauri::command]
pub async fn start_sharing(
    state: State<'_, AppState>,
    friendly_name: String,
) -> AppResult<()> {
    start_sharing_inner(&state, friendly_name.clone()).await?;
    // Persist after a successful start so a crash mid-start doesn't leave a
    // stale config that would auto-resume into a half-built service.
    write_server_config(&ServerConfig { version: 1, friendly_name })?;
    Ok(())
}

/// Body of the `start_sharing` command, factored out so the app-startup
/// auto-resume hook can call exactly the same logic without going through
/// the Tauri command dispatcher. Does NOT persist `sharing-server.json` —
/// that's the caller's concern (auto-resume reads it; the Tauri command
/// writes it).
pub async fn start_sharing_inner(
    state: &AppState,
    friendly_name: String,
) -> AppResult<()> {
    // Acquire the write lock BEFORE binding ports / spawning proxies so that a
    // concurrent stop_sharing cannot return Ok while we are mid-start and leave
    // the service running with no future cleanup path.
    let mut sharing_slot = state.sharing.write().await;
    if sharing_slot.is_some() {
        return Err(AppError::Other("sharing already running".into()));
    }
    let cfg = build_sharing_config(state, friendly_name).await?;
    let service = Arc::new(SharingService::new(cfg).map_err(|e| AppError::Other(e.to_string()))?);
    service.start().await.map_err(|e| AppError::Other(e.to_string()))?;

    // Spawn the vocab CRUD API on the configured port. Failures here are
    // logged but don't abort sharing — clients on older versions don't
    // expect a vocab API anyway, so they degrade gracefully.
    let vocab_handle = match crate::sharing_vocab_api::spawn(
        std::sync::Arc::clone(&state.db),
        service.token_store(),
        service.config().vocab_port,
    )
    .await
    {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::warn!("vocab API failed to start: {e}");
            None
        }
    };

    *sharing_slot = Some(service);
    *state.vocab_api.write().await = vocab_handle;

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
    let allow_public = {
        let conn = state.db.conn().map_err(|e| AppError::Other(e.to_string()))?;
        let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| AppError::Other(e.to_string()))?;
        cfg.migrate();
        cfg.allow_public_endpoint
    };
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
            p.set_endpoint(local_ollama, allow_public).await?;
        }
    }
    {
        let guard = state.lmstudio_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(local_lmstudio, allow_public).await?;
        }
    }
    {
        let guard = state.remote_stt_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(local_whisper, allow_public).await?;
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn stop_sharing(state: State<'_, AppState>) -> AppResult<()> {
    // Clear the auto-resume marker first so an explicit Stop wins over an
    // unrelated startup race (e.g. user stops sharing immediately on launch
    // before the resume hook fires).
    delete_server_config();
    if let Some(s) = state.sharing.write().await.take() {
        s.stop().await.map_err(|e| AppError::Other(e.to_string()))?;
    }
    if let Some(h) = state.vocab_api.write().await.take() {
        h.abort();
    }

    // Restore provider endpoints to pre-sharing configuration.
    // If this machine is also paired as a client to another server, restore the
    // paired endpoint; otherwise revert to None (local-only mode).
    let allow_public = {
        let conn = state.db.conn().map_err(|e| AppError::Other(e.to_string()))?;
        let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| AppError::Other(e.to_string()))?;
        cfg.migrate();
        cfg.allow_public_endpoint
    };
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
            p.set_endpoint(ollama_ep, allow_public).await?;
        }
    }
    {
        let guard = state.lmstudio_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(lmstudio_ep, allow_public).await?;
        }
    }
    {
        let guard = state.remote_stt_provider.read().await;
        if let Some(ref p) = *guard {
            p.set_endpoint(whisper_ep, allow_public).await?;
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn sharing_status(state: State<'_, AppState>) -> AppResult<SharingStatusDto> {
    if let Some(s) = state.sharing.read().await.as_ref() {
        Ok(s.status().await.into())
    } else {
        Ok(SharingStatusDto {
            enabled: false,
            ollama_ok: false,
            whisper_ok: false,
            lmstudio_ok: false,
            mdns_ok: false,
            pairing_ok: false,
            paired_clients: 0,
        })
    }
}

async fn build_sharing_config(
    state: &AppState,
    friendly_name: String,
) -> AppResult<SharingConfig> {
    use medical_security::keychain;
    use rand::RngCore;

    // Reuse the SQLCipher DB key as the sharing-store key — same keychain
    // entry, no new secret to manage.
    let key = keychain::get_db_key()
        .map_err(|e| AppError::Other(format!("Keychain access denied: {e}. Sharing requires keychain access — quit and reopen FerriScribe, then approve the keychain prompt.")))?
        .ok_or_else(|| {
            AppError::Other("FerriScribe's database hasn't been initialized yet. Restart the app and try again.".into())
        })?;

    let app_data = dirs::data_dir()
        .ok_or_else(|| AppError::Other("no app data dir".into()))?
        .join("rust-medical-assistant");
    std::fs::create_dir_all(&app_data)?;
    let mut whisper_api = [0u8; 16];
    rand::thread_rng()
        .try_fill_bytes(&mut whisper_api)
        .map_err(|e| AppError::Other(e.to_string()))?;
    // Only wire up an LM Studio proxy when LM Studio's local server is
    // actually running. If the user starts LM Studio after Start sharing,
    // they'll need to Stop + Start sharing to wire up the proxy.
    let lmstudio_internal = lmstudio_running_port(&state.http_client).await;
    Ok(SharingConfig {
        enabled: true,
        friendly_name,
        ollama_proxy_port: 11435,
        whisper_proxy_port: 8081,
        pairing_port: 11436,
        whisper_internal_port: 8080,
        lmstudio_internal_port: lmstudio_internal,
        lmstudio_proxy_port: lmstudio_internal.map(|_| 1235),
        vocab_port: 11437,
        token_store_path: app_data.join("sharing.db"),
        token_store_key: key,
        binary_dir: app_data.join("bin"),
        whisper_model_path: app_data.join("models/whisper/ggml-large-v3-turbo.bin"),
        whisper_internal_api_key: hex::encode(whisper_api),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn lmstudio_running_port(client: &reqwest::Client) -> Option<u16> {
    let resp = client
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
