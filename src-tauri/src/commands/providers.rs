use std::time::Duration;

use tracing::info;

use medical_core::error::{AppError, AppResult};

use crate::state::{self, AppState};

/// Rebuild AI + STT provider registries (e.g. after LM Studio host/port changes).
///
/// Returns the list of available AI provider names after reinitialization.
#[tauri::command]
pub async fn reinit_providers(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<String>> {
    // Load saved settings for provider config (host, port, active provider, whisper model)
    let config = {
        let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| AppError::Database(e.to_string()))?;
        cfg.migrate();
        cfg
    };

    // Re-load paired endpoint so reinit also re-wires endpoints.
    let paired = state::load_paired_connection();
    let bearer = if paired.is_some() { state::load_sharing_bearer() } else { None };
    let (ollama_ep, lmstudio_ep, whisper_ep) = if let Some(ref p) = paired {
        use medical_core::types::RemoteEndpoint;
        (
            Some(RemoteEndpoint { lan: p.lan.clone(), tailscale: p.tailscale.clone(), port: p.ports.ollama, bearer: bearer.clone() }),
            p.ports.lmstudio.map(|lp| RemoteEndpoint { lan: p.lan.clone(), tailscale: p.tailscale.clone(), port: lp, bearer: bearer.clone() }),
            Some(RemoteEndpoint { lan: p.lan.clone(), tailscale: p.tailscale.clone(), port: p.ports.whisper, bearer: bearer.clone() }),
        )
    } else {
        (None, None, None)
    };

    // Rebuild AI providers with current config (includes LM Studio host/port).
    let mut ai_handles = state::init_ai_providers(&config, ollama_ep, lmstudio_ep);

    // Restore the user's active provider preference from saved settings
    // so reinit doesn't silently switch to a random provider.
    ai_handles.registry.set_active(&config.ai_provider);

    let available = ai_handles.registry.list_available();

    // Destructure before any partial moves so all fields remain accessible.
    let state::AiProviderHandles { registry, ollama: new_ollama, lmstudio: new_lmstudio } = ai_handles;
    {
        let mut guard = state.ai_providers.lock().await;
        *guard = registry;
    }

    // Rebuild STT provider based on current config (mode + whisper model + remote host/port/key).
    let stt_handles = state::init_stt_providers_with_config(&state.data_dir, &config, whisper_ep);
    let state::SttProviderHandles { provider: new_stt_provider, remote: new_remote_stt } = stt_handles;
    {
        let mut guard = state.stt_providers.lock().await;
        *guard = new_stt_provider;
    }

    // Replace the typed handles with the freshly built Arcs so the handles
    // and the registry point at the SAME Arc instances.  Any subsequent
    // set_endpoint call (e.g. from start_sharing / pair_with_server) now
    // mutates the provider that is actually in the request path.
    *state.ollama_provider.write().await = new_ollama;
    *state.lmstudio_provider.write().await = new_lmstudio;
    *state.remote_stt_provider.write().await = new_remote_stt;

    info!(providers = ?available, "Providers reinitialized");

    Ok(available)
}

/// Test connectivity to an LM Studio server.
///
/// Makes a GET request to `http://{host}:{port}/v1/models` with a 5-second
/// timeout. Returns a success message with the model count, or an error.
#[tauri::command]
pub async fn test_lmstudio_connection(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
) -> AppResult<String> {
    let effective_host = if host.is_empty() { "localhost".to_string() } else { host };
    let url = format!("http://{}:{}/v1/models", effective_host, port);

    info!(url = %url, "Testing LM Studio connection");

    let response = state.http_client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| {
            use medical_core::error::OfflineReason;
            use medical_core::preflight::classify_reqwest_error;
            match classify_reqwest_error(&e) {
                Some(OfflineReason::ConnectionRefused) => AppError::AiProvider(format!(
                    "Connection refused — is LM Studio running at {}:{}?",
                    effective_host, port
                )),
                Some(OfflineReason::Timeout) => AppError::AiProvider(format!(
                    "Connection timed out — check that {}:{} is reachable",
                    effective_host, port
                )),
                Some(OfflineReason::DnsFailure) => AppError::AiProvider(format!(
                    "Cannot resolve hostname '{}'", effective_host
                )),
                Some(OfflineReason::TlsFailure) => AppError::AiProvider(format!(
                    "TLS handshake failed at {}:{}", effective_host, port
                )),
                None => AppError::AiProvider(format!("Connection failed: {e}")),
            }
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = medical_core::http_error_body::read_error_body(response, 200).await;
        return Err(AppError::AiProvider(format!("Server returned HTTP {status}: {body}")));
    }

    // Parse the OpenAI-compatible models response to count models
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::AiProvider(format!("Invalid response from server: {e}")))?;

    let model_count = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    Ok(format!(
        "Connected — {} model{} available",
        model_count,
        if model_count == 1 { "" } else { "s" }
    ))
}

/// Test connectivity to a remote Whisper server (OpenAI-compatible).
///
/// Makes a GET request to `http://{host}:{port}/v1/models` with a 5-second
/// connect timeout and 10-second overall timeout. If `api_key` is present and
/// non-empty, an `Authorization: Bearer …` header is sent.
#[tauri::command]
pub async fn test_stt_remote_connection(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
    api_key: Option<String>,
) -> AppResult<String> {
    let effective_host = if host.is_empty() { "localhost".to_string() } else { host };
    let url = format!("http://{}:{}/v1/models", effective_host, port);

    info!(url = %url, "Testing Whisper server connection");

    let mut req = state.http_client
        .get(&url)
        .timeout(Duration::from_secs(10));
    if let Some(key) = api_key.as_deref().filter(|s| !s.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let response = req.send().await.map_err(|e| {
        use medical_core::error::OfflineReason;
        use medical_core::preflight::classify_reqwest_error;
        match classify_reqwest_error(&e) {
            Some(OfflineReason::ConnectionRefused) => AppError::SttProvider(format!(
                "Connection refused — is the Whisper server running at {}:{}?",
                effective_host, port
            )),
            Some(OfflineReason::Timeout) => AppError::SttProvider(format!(
                "Connection timed out — check that {}:{} is reachable",
                effective_host, port
            )),
            Some(OfflineReason::DnsFailure) => AppError::SttProvider(format!(
                "Cannot resolve hostname '{}'", effective_host
            )),
            Some(OfflineReason::TlsFailure) => AppError::SttProvider(format!(
                "TLS handshake failed at {}:{}", effective_host, port
            )),
            None => AppError::SttProvider(format!("Connection failed: {e}")),
        }
    })?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(AppError::SttProvider(
            "Authentication failed \u{2014} verify the API key, or if this is a paired client, \
             re-pair the office server (Settings \u{2192} Sharing \u{2192} Unpair, then scan a fresh code)."
                .to_string(),
        ));
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = medical_core::http_error_body::read_error_body(response, 200).await;
        return Err(AppError::SttProvider(format!("Server returned HTTP {status}: {body}")));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::SttProvider(format!("Invalid response from server: {e}")))?;

    let model_count = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    Ok(format!(
        "Connected — {} model{} available",
        model_count,
        if model_count == 1 { "" } else { "s" }
    ))
}

/// Test connectivity to an Ollama server.
///
/// Makes a GET request to `http://{host}:{port}/api/tags` with a 5-second
/// timeout. Returns a success message including the installed-model count,
/// or a user-readable error.
#[tauri::command]
pub async fn test_ollama_connection(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
) -> AppResult<String> {
    let effective_host = if host.is_empty() { "localhost".to_string() } else { host };
    let url = format!("http://{}:{}/api/tags", effective_host, port);

    info!(url = %url, "Testing Ollama connection");

    let response = state.http_client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| {
            use medical_core::error::OfflineReason;
            use medical_core::preflight::classify_reqwest_error;
            match classify_reqwest_error(&e) {
                Some(OfflineReason::ConnectionRefused) => AppError::AiProvider(format!(
                    "Connection refused — is Ollama running at {}:{}?",
                    effective_host, port
                )),
                Some(OfflineReason::Timeout) => AppError::AiProvider(format!(
                    "Connection timed out — check that {}:{} is reachable",
                    effective_host, port
                )),
                Some(OfflineReason::DnsFailure) => AppError::AiProvider(format!(
                    "Cannot resolve hostname '{}'", effective_host
                )),
                Some(OfflineReason::TlsFailure) => AppError::AiProvider(format!(
                    "TLS handshake failed at {}:{}", effective_host, port
                )),
                None => AppError::AiProvider(format!("Connection failed: {e}")),
            }
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = medical_core::http_error_body::read_error_body(response, 200).await;
        return Err(AppError::AiProvider(format!("Server returned HTTP {status}: {body}")));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::AiProvider(format!("Invalid response from server: {e}")))?;

    let model_count = body
        .get("models")
        .and_then(|m| m.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    Ok(format!(
        "Connected — {} model{} installed",
        model_count,
        if model_count == 1 { "" } else { "s" }
    ))
}
