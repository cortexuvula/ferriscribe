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
    start_sharing_inner(&state, friendly_name.clone(), app_handle).await?;
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
/// `app_handle` is used to emit `sharing-readiness-changed` events to the
/// frontend when the ReadinessWatcher brings a late-arriving upstream online,
/// and by the vocab API to emit recording-refresh events. It is a required
/// (non-optional) parameter so the invariant is enforced at compile time.
pub async fn start_sharing_inner(
    state: &AppState,
    friendly_name: String,
    app_handle: tauri::AppHandle,
) -> AppResult<()> {
    // Serialize the WHOLE start against stop/other starts. The old slot
    // check alone let a Stop land mid-start (slot still empty), return Ok,
    // and the start then finished installing a running server the user had
    // stopped. Holding this across start() only delays concurrent
    // lifecycle ops — status polling reads the sharing slot, never this.
    let _lifecycle = state.sharing_lifecycle.lock().await;
    {
        let sharing_slot = state.sharing.read().await;
        if sharing_slot.is_some() {
            return Err(AppError::Other("sharing already running".into()));
        }
    }
    let cfg = build_sharing_config(friendly_name).await?;
    let service = Arc::new(SharingService::new(cfg).map_err(|e| AppError::Other(e.to_string()))?);
    // The ReadinessWatcher's probe client has no dependency on the started
    // service — build it before start() so its (rare) failure doesn't need
    // service cleanup.
    let watcher_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| AppError::Other(e.to_string()))?;
    // Bind ports + start whisper here, BEFORE taking the write lock. On error,
    // stop the service so the whisper child isn't orphaned.
    if let Err(e) = service.start().await {
        let _ = service.stop().await;
        return Err(AppError::Other(e.to_string()));
    }

    // Spawn the vocab CRUD API on the configured port. Failures here are
    // logged but don't abort sharing — clients on older versions don't
    // expect a vocab API anyway, so they degrade gracefully. The app handle
    // lets the vocab API emit Tauri events to this server's own frontend
    // when a remote client pushes recordings (so the Recordings view
    // refreshes).
    let vocab_handle = match crate::sharing_vocab_api::spawn(
        std::sync::Arc::clone(&state.db),
        service.token_store(),
        service.config().vocab_port,
        state.data_dir.clone(),
        app_handle.clone(),
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
    service.spawn_readiness_watcher(watcher_client);

    {
        let mut rx = service.readiness_changes();
        let handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            use tauri::Emitter;
            // Skip the initial value; only emit on actual changes.
            while rx.changed().await.is_ok() {
                let _ = handle.emit("sharing-readiness-changed", ());
            }
        });
    }

    // All fallible wiring runs BEFORE the state slots are assigned: a failure
    // here unwinds (stop service, abort the vocab API) instead of leaving a
    // dead service wedged in `state.sharing` — which would make every later
    // start fail with "sharing already running" while an orphaned vocab API
    // keeps serving PHI over HTTP until restart.
    if let Err(e) = wire_upstream_endpoints(state).await {
        if let Some(h) = vocab_handle {
            h.abort();
        }
        let _ = service.stop().await;
        return Err(e);
    }

    // Brief write lock: just the assignment. Everything above ran unlocked
    // and nothing after this point can fail.
    *state.sharing.write().await = Some(service.clone());
    *state.vocab_api.write().await = vocab_handle;

    Ok(())
}

/// Post-start wiring for the office server's own AI/STT providers: installs
/// the persistent Ollama service (best-effort), then points the providers at
/// the local upstream ports. Runs before `state.sharing` is assigned so a
/// failure unwinds the whole start (see `start_sharing_inner`).
async fn wire_upstream_endpoints(state: &AppState) -> AppResult<()> {
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
    // bearer needed. Ports are the upstream ports (Ollama 11434, LM Studio
    // 1234, oMLX 8000, whisper.cpp 8080), NOT the proxy ports
    // (11435 / 1235 / 8001 / 8081).
    let allow_public = crate::commands::load_app_config(&state.db, "sharing start")
        .await?
        .allow_public_endpoint;
    use medical_core::types::RemoteEndpoint;
    let endpoint = |port: u16| {
        Some(RemoteEndpoint {
            lan: Some("127.0.0.1".to_string()),
            tailscale: None,
            port,
            bearer: None,
        })
    };
    let local_ollama = endpoint(11434);
    let local_lmstudio = endpoint(1234);
    let local_omlx = endpoint(8000);
    let local_whisper = endpoint(8080);

    {
        let provider = { state.ollama_provider.read().await.clone() };
        if let Some(p) = provider
            && let Err(e) = p.set_endpoint(local_ollama, allow_public).await
        {
            return Err(e);
        }
    }
    {
        let provider = { state.lmstudio_provider.read().await.clone() };
        if let Some(p) = provider
            && let Err(e) = p.set_endpoint(local_lmstudio, allow_public).await
        {
            return Err(e);
        }
    }
    {
        let provider = { state.omlx_provider.read().await.clone() };
        if let Some(p) = provider
            && let Err(e) = p.set_endpoint(local_omlx, allow_public).await
        {
            return Err(e);
        }
    }
    {
        let provider = { state.remote_stt_provider.read().await.clone() };
        if let Some(p) = provider
            && let Err(e) = p.set_endpoint(local_whisper, allow_public).await
        {
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
    // Wait out an in-flight start (see start_sharing_inner): a Stop clicked
    // during a slow start now blocks until the service registers, then
    // cleanly stops it — instead of racing to an empty slot and returning
    // Ok while the start installs a running server behind us.
    let _lifecycle = state.sharing_lifecycle.lock().await;
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
    let allow_public = crate::commands::load_app_config(&state.db, "sharing stop")
        .await?
        .allow_public_endpoint;
    let paired = crate::state::load_paired_connection();
    let bearer = if paired.is_some() {
        crate::state::load_sharing_bearer()
    } else {
        None
    };

    let eps = if let Some(ref p) = paired {
        super::paired_endpoints(p, bearer)
    } else {
        super::PairedEndpoints {
            ollama: None,
            lmstudio: None,
            omlx: None,
            whisper: None,
        }
    };

    {
        let provider = { state.ollama_provider.read().await.clone() };
        if let Some(p) = provider {
            p.set_endpoint(eps.ollama, allow_public).await?;
        }
    }
    {
        let provider = { state.lmstudio_provider.read().await.clone() };
        if let Some(p) = provider {
            p.set_endpoint(eps.lmstudio, allow_public).await?;
        }
    }
    {
        let provider = { state.omlx_provider.read().await.clone() };
        if let Some(p) = provider {
            p.set_endpoint(eps.omlx, allow_public).await?;
        }
    }
    {
        let provider = { state.remote_stt_provider.read().await.clone() };
        if let Some(p) = provider {
            p.set_endpoint(eps.whisper, allow_public).await?;
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
            omlx_ok: false,
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

    let app_data = super::app_data_dir()?;
    let mut whisper_api = [0u8; 16];
    rand::rng().fill_bytes(&mut whisper_api);
    // LM Studio and oMLX are always candidates in office mode. The start()
    // gate probes each once; the ReadinessWatcher brings them online later
    // if they boot after the gate (the login-launch race we're fixing). No
    // Stop+Start needed.
    Ok(SharingConfig {
        friendly_name,
        ollama_proxy_port: 11435,
        whisper_proxy_port: 8081,
        pairing_port: 11436,
        whisper_internal_port: 8080,
        lmstudio_internal_port: Some(1234),
        lmstudio_proxy_port: Some(1235),
        omlx_internal_port: Some(8000),
        omlx_proxy_port: Some(8001),
        vocab_port: 11437,
        token_store_path: app_data.join("sharing.db"),
        token_store_key: key,
        binary_dir: app_data.join("bin"),
        whisper_model_path: app_data.join("models/whisper/ggml-large-v3-turbo.bin"),
        whisper_internal_api_key: hex::encode(whisper_api),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}
