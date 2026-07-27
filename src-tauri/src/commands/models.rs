//! Tauri commands for model management: list, download, delete.

use serde::Serialize;
use tauri::Emitter;

use medical_core::error::{AppError, AppResult};
use medical_stt_providers::models as stt_models;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
struct ModelDownloadProgress {
    model_id: String,
    downloaded_bytes: u64,
    total_bytes: u64,
}

/// List all available whisper models with download status.
#[tauri::command]
pub async fn list_whisper_models(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<stt_models::ModelInfo>> {
    Ok(stt_models::available_whisper_models(&state.data_dir))
}

/// List all available pyannote diarization models with download status.
#[tauri::command]
pub async fn list_pyannote_models(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<stt_models::ModelInfo>> {
    Ok(stt_models::available_pyannote_models(&state.data_dir))
}

/// Download a model by ID (whisper or pyannote).
///
/// Emits `model-download-progress` events with `{ model_id, downloaded_bytes, total_bytes }`.
#[tauri::command]
pub async fn download_model(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    model_id: String,
) -> AppResult<()> {
    let data_dir = state.data_dir.clone();

    // Search both whisper and pyannote model lists
    let all_whisper = stt_models::available_whisper_models(&data_dir);
    let all_pyannote = stt_models::available_pyannote_models(&data_dir);

    let (model, dest_path) = if let Some(m) = all_whisper.iter().find(|m| m.id == model_id) {
        let path = stt_models::whisper_model_path(&data_dir, &m.filename);
        (m.clone(), path)
    } else if let Some(m) = all_pyannote.iter().find(|m| m.id == model_id) {
        let path = stt_models::pyannote_model_path(&data_dir, &m.filename);
        (m.clone(), path)
    } else {
        return Err(AppError::SttProvider(format!(
            "Unknown model ID: {model_id}"
        )));
    };

    if dest_path.exists() {
        return Ok(()); // Already downloaded
    }

    let mid = model_id.clone();
    let app_clone = app.clone();

    stt_models::download_model(&model.download_url, &dest_path, move |downloaded, total| {
        let _ = app_clone.emit(
            "model-download-progress",
            ModelDownloadProgress {
                model_id: mid.clone(),
                downloaded_bytes: downloaded,
                total_bytes: total,
            },
        );
    })
    .await
    .map_err(|e| AppError::SttProvider(e.to_string()))?;

    // After downloading, reinitialize the STT provider so it picks up new models.
    // Load the full AppConfig so local/remote mode + remote host/port/key all flow through.
    // Wrapped in spawn_blocking so SQLite pool checkout never blocks the async worker.
    let config = {
        let db = std::sync::Arc::clone(&state.db);
        tokio::task::spawn_blocking(
            move || -> AppResult<medical_core::types::settings::AppConfig> {
                let conn = db.conn()?;
                let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)?;
                cfg.migrate();
                Ok(cfg)
            },
        )
        .await
        .map_err(crate::commands::join_err)??
    };
    // Re-load the paired RemoteEndpoint (if any) so the reinitialised STT
    // provider routes to the office server rather than silently falling back
    // to localhost.
    let paired = crate::state::load_paired_connection();
    let bearer = if paired.is_some() {
        crate::state::load_sharing_bearer()
    } else {
        None
    };
    let whisper_ep = if let Some(ref p) = paired {
        use medical_core::types::RemoteEndpoint;
        Some(RemoteEndpoint {
            lan: p.lan.clone(),
            tailscale: p.tailscale.clone(),
            port: p.ports.whisper,
            bearer: bearer.clone(),
        })
    } else {
        None
    };

    let crate::state::SttProviderHandles {
        provider: new_stt_provider,
        remote: new_remote_stt,
    } = crate::state::init_stt_providers_with_config(&state.data_dir, &config, whisper_ep);
    {
        let mut guard = state.stt_providers.lock().await;
        *guard = new_stt_provider;
    }
    // Replace the typed handle with the freshly built Arc so the handle and the
    // stt_providers slot point at the SAME instance.
    *state.remote_stt_provider.write().await = new_remote_stt;

    Ok(())
}

/// Delete a downloaded model to reclaim disk space.
#[tauri::command]
pub async fn delete_model(state: tauri::State<'_, AppState>, model_id: String) -> AppResult<()> {
    let data_dir = state.data_dir.clone();

    // Search both whisper and pyannote model lists
    let all_whisper = stt_models::available_whisper_models(&data_dir);
    let all_pyannote = stt_models::available_pyannote_models(&data_dir);

    let path = if let Some(m) = all_whisper.iter().find(|m| m.id == model_id) {
        stt_models::whisper_model_path(&data_dir, &m.filename)
    } else if let Some(m) = all_pyannote.iter().find(|m| m.id == model_id) {
        stt_models::pyannote_model_path(&data_dir, &m.filename)
    } else {
        return Err(AppError::SttProvider(format!(
            "Unknown model ID: {model_id}"
        )));
    };

    stt_models::delete_model(&path)
        .await
        .map_err(|e| AppError::SttProvider(e.to_string()))?;

    Ok(())
}
