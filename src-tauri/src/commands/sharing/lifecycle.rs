//! Office-server lifecycle: start / stop / status. Owns the heavy
//! `start_sharing_inner` body and the `build_sharing_config` builder.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_sharing::{SharingConfig, SharingService};
use tauri::State;

use crate::state::AppState;

use super::{ServerConfig, SharingStatusDto, delete_server_config, write_server_config};

#[tauri::command]
pub async fn start_sharing(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
    friendly_name: String,
) -> AppResult<()> {
    start_sharing_inner(&state, friendly_name.clone(), Some(app_handle)).await?;
    // Persist after a successful start so a crash mid-start doesn't leave a
    // stale config that would auto-resume into a half-built service.
    write_server_config(&ServerConfig {
        version: 1,
        friendly_name,
    })?;
    Ok(())
}

/// Body of the `start_sharing` command, factored out so the app-startup
/// auto-resume hook can call exactly the same logic without going through
/// the Tauri command dispatcher. Does NOT persist `sharing-server.json` —
/// that's the caller's concern (auto-resume reads it; the Tauri command
/// writes it).
///
/// `app_handle`, when `Some`, is used to emit `sharing-readiness-changed`
/// events to the frontend when the ReadinessWatcher brings a late-arriving
/// upstream online. The auto-resume path passes the handle too.
pub async fn start_sharing_inner(
    state: &AppState,
    friendly_name: String,
    app_handle: Option<tauri::AppHandle>,
) -> AppResult<()> {
    // Acquire the write lock BEFORE binding ports / spawning proxies so that a
    // concurrent stop_sharing cannot return Ok while we are mid-start and leave
    // the service running with no future cleanup path. We only hold the write
    // lock briefly to check + assign — NOT across the multi-second start(),
    // so sharing_status polling and stop() aren't frozen during startup.
    {
        let sharing_slot = state.sharing.read().await;
        if sharing_slot.is_some() {
            return Err(AppError::Other("sharing already running".into()));
        }
    }
    let cfg = build_sharing_config(friendly_name).await?;
    let service = Arc::new(SharingService::new(cfg).map_err(|e| AppError::Other(e.to_string()))?);
    // Bind ports + start whisper here, BEFORE taking the write lock. On error,
    // stop the service so the whisper child isn't orphaned.
    if let Err(e) = service.start().await {
        let _ = service.stop().await;
        return Err(AppError::Other(e.to_string()));
    }

    // Spawn the vocab CRUD API on the configured port. Failures here are
    // logged but don't abort sharing — clients on older versions don't
    // expect a vocab API anyway, so they degrade gracefully.
    let vocab_handle = match crate::sharing_vocab_api::spawn(
        std::sync::Arc::clone(&state.db),
        service.token_store(),
        service.config().vocab_port,
        state.data_dir.clone(),
    )
    .await
    {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::warn!("vocab API failed to start: {e}");
            None
        }
    };

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
            // Skip the initial value; only emit on actual changes.
            while rx.changed().await.is_ok() {
                let _ = handle.emit("sharing-readiness-changed", ());
            }
        });
    }

    // Brief write lock: just the assignment. Everything above ran unlocked.
    *state.sharing.write().await = Some(service.clone());
    *state.vocab_api.write().await = vocab_handle;

    // Wire up the persistent Ollama service the wizard promises ("FerriScribe
    // will configure persistent Ollama..."). Idempotent: skip when already
    // installed; surface install failures via warn rather than aborting
    // sharing so that a missing ollama binary or write-protected
    // LaunchAgents directory doesn't block the in-process services.
    use medical_sharing::service_installer::{
        ServiceState, install_persistent_ollama, ollama_service_state,
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
        let conn = state
            .db
            .conn()
            .map_err(|e| AppError::Other(e.to_string()))?;
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
        let provider = { state.ollama_provider.read().await.clone() };
        if let Some(p) = provider
            && let Err(e) = p.set_endpoint(local_ollama, allow_public).await
        {
            let _ = service.stop().await;
            return Err(e);
        }
    }
    {
        let provider = { state.lmstudio_provider.read().await.clone() };
        if let Some(p) = provider
            && let Err(e) = p.set_endpoint(local_lmstudio, allow_public).await
        {
            let _ = service.stop().await;
            return Err(e);
        }
    }
    {
        let provider = { state.remote_stt_provider.read().await.clone() };
        if let Some(p) = provider
            && let Err(e) = p.set_endpoint(local_whisper, allow_public).await
        {
            let _ = service.stop().await;
            return Err(e);
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn stop_sharing(state: State<'_, AppState>) -> AppResult<()> {
    stop_sharing_inner(&state).await
}

/// Core stop logic, factored out so the app-close / window-close handler can
/// call it without the Tauri command wrapper.
pub async fn stop_sharing_inner(state: &AppState) -> AppResult<()> {
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
        let conn = state
            .db
            .conn()
            .map_err(|e| AppError::Other(e.to_string()))?;
        let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| AppError::Other(e.to_string()))?;
        cfg.migrate();
        cfg.allow_public_endpoint
    };
    let paired = crate::state::load_paired_connection();
    let bearer = if paired.is_some() {
        crate::state::load_sharing_bearer()
    } else {
        None
    };

    use medical_core::types::RemoteEndpoint;
    let (ollama_ep, lmstudio_ep, whisper_ep) = if let Some(ref p) = paired {
        (
            Some(RemoteEndpoint {
                lan: p.lan.clone(),
                tailscale: p.tailscale.clone(),
                port: p.ports.ollama,
                bearer: bearer.clone(),
            }),
            p.ports.lmstudio.map(|lp| RemoteEndpoint {
                lan: p.lan.clone(),
                tailscale: p.tailscale.clone(),
                port: lp,
                bearer: bearer.clone(),
            }),
            Some(RemoteEndpoint {
                lan: p.lan.clone(),
                tailscale: p.tailscale.clone(),
                port: p.ports.whisper,
                bearer,
            }),
        )
    } else {
        (None, None, None)
    };

    {
        let provider = { state.ollama_provider.read().await.clone() };
        if let Some(p) = provider {
            p.set_endpoint(ollama_ep, allow_public).await?;
        }
    }
    {
        let provider = { state.lmstudio_provider.read().await.clone() };
        if let Some(p) = provider {
            p.set_endpoint(lmstudio_ep, allow_public).await?;
        }
    }
    {
        let provider = { state.remote_stt_provider.read().await.clone() };
        if let Some(p) = provider {
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

async fn build_sharing_config(friendly_name: String) -> AppResult<SharingConfig> {
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
}
