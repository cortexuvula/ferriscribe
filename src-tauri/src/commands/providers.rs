use std::time::Duration;

use tracing::info;

use medical_core::error::{AppError, AppResult, OfflineReason, ServiceKind};
use medical_core::preflight::classify_reqwest_error;

use crate::state::{self, AppState};

/// Validate that `host` is a local/LAN endpoint per the endpoint policy,
/// unless the user has opted into public endpoints via `allow_public_endpoint`.
/// Called by every test/probe command before firing an outbound request, so a
/// crafted frontend payload can't make the app contact an arbitrary public host
/// (AGENTS.md: no hosted AI APIs, no telemetry).
async fn validate_probe_host(state: &AppState, host: &str) -> AppResult<()> {
    // Load allow_public_endpoint off the async worker — SQLite pool checkout
    // and any busy-wait must not stall the Tokio runtime.
    let db = std::sync::Arc::clone(&state.db);
    let allow_public = tokio::task::spawn_blocking(move || -> AppResult<bool> {
        let conn = db.conn()?;
        Ok(medical_db::settings::SettingsRepo::load_config(&conn)
            .map(|mut c| {
                c.migrate();
                c.allow_public_endpoint
            })
            .unwrap_or(false))
    })
    .await
    .map_err(crate::commands::join_err)??;

    let effective = if host.is_empty() { "localhost" } else { host };
    medical_core::endpoint_policy::validate_local_endpoint(effective, allow_public)
        .map_err(|e| AppError::invalid_endpoint_for(e, "probe_host"))?;
    Ok(())
}

/// Inner reachability check — exposed as a pure async fn so unit tests can
/// call it without constructing `tauri::State`. The Tauri command is a thin
/// wrapper around this.
///
/// Returns Ok(()) for any HTTP response *except* 401/403 — auth failures
/// surface as EndpointOffline so the polling pill reflects them. Network
/// errors (connect/timeout/DNS/TLS) flow through classify_reqwest_error.
async fn probe_endpoint_reachable_inner(
    service: ServiceKind,
    provider_name: String,
    host: String,
    port: u16,
    probe_path: String,
    api_key: Option<String>,
) -> AppResult<()> {
    let effective_host = if host.is_empty() {
        "localhost".to_string()
    } else {
        host
    };
    let base_url = format!("http://{}:{}", effective_host, port);
    let url = format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        probe_path.trim_start_matches('/'),
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| AppError::Config(format!("reachability client build: {e}")))?;

    let mut req = client.get(&url);
    if let Some(key) = api_key.as_deref().filter(|s| !s.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let response = req.send().await.map_err(|e| {
        let reason = classify_reqwest_error(&e).unwrap_or(OfflineReason::ConnectionRefused);
        AppError::EndpointOffline {
            service,
            endpoint: base_url.clone(),
            reason,
            provider_name: provider_name.clone(),
        }
    })?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(AppError::EndpointOffline {
            service,
            endpoint: base_url,
            reason: OfflineReason::ConnectionRefused,
            provider_name,
        });
    }

    // Any other HTTP status (200/3xx/404/5xx) = reachable.
    Ok(())
}

/// Lenient reachability probe for the background endpointHealth poller.
/// Returns Ok for any HTTP response except 401/403. Returns
/// `AppError::EndpointOffline` for network errors and auth failures.
///
/// Used by `src/lib/stores/endpointHealth.ts`. NOT used by Settings →
/// Test Connection buttons (those use the strict `test_*_connection` commands
/// which probe `/v1/models` and treat 404 as failure — appropriate for
/// explicit user-triggered "can list models?" checks).
#[tauri::command]
pub async fn probe_endpoint_reachable(
    state: tauri::State<'_, AppState>,
    service: ServiceKind,
    provider_name: String,
    host: String,
    port: u16,
    probe_path: String,
    api_key: Option<String>,
) -> AppResult<()> {
    validate_probe_host(&state, &host).await?;
    probe_endpoint_reachable_inner(service, provider_name, host, port, probe_path, api_key).await
}

/// Rebuild AI + STT provider registries (e.g. after LM Studio host/port changes).
///
/// Returns the list of available AI provider names after reinitialization.
#[tauri::command]
pub async fn reinit_providers(state: tauri::State<'_, AppState>) -> AppResult<Vec<String>> {
    // Load saved settings for provider config (host, port, active provider, whisper model).
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

    // Re-load paired endpoint so reinit also re-wires endpoints.
    let paired = state::load_paired_connection();
    let bearer = if paired.is_some() {
        state::load_sharing_bearer()
    } else {
        None
    };
    let (ollama_ep, lmstudio_ep, whisper_ep) = if let Some(ref p) = paired {
        use medical_core::types::RemoteEndpoint;
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
                bearer: bearer.clone(),
            }),
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
    let state::AiProviderHandles {
        registry,
        ollama: new_ollama,
        lmstudio: new_lmstudio,
    } = ai_handles;
    {
        let mut guard = state.ai_providers.lock().await;
        *guard = registry;
    }

    // Rebuild STT provider based on current config (mode + whisper model + remote host/port/key).
    let stt_handles = state::init_stt_providers_with_config(&state.data_dir, &config, whisper_ep);
    let state::SttProviderHandles {
        provider: new_stt_provider,
        remote: new_remote_stt,
    } = stt_handles;
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
/// timeout. If `api_key` is present and non-empty, an `Authorization: Bearer …`
/// header is sent. Returns a success message with the model count, or an error.
#[tauri::command]
pub async fn test_lmstudio_connection(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
    api_key: Option<String>,
) -> AppResult<String> {
    validate_probe_host(&state, &host).await?;
    let effective_host = if host.is_empty() {
        "localhost".to_string()
    } else {
        host
    };
    let url = format!("http://{}:{}/v1/models", effective_host, port);

    info!(url = %url, "Testing LM Studio connection");

    let mut req = state.http_client.get(&url).timeout(Duration::from_secs(5));
    if let Some(key) = api_key.as_deref().filter(|s| !s.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let response = req.send().await.map_err(|e| {
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
            Some(OfflineReason::DnsFailure) => {
                AppError::AiProvider(format!("Cannot resolve hostname '{}'", effective_host))
            }
            Some(OfflineReason::TlsFailure) => AppError::AiProvider(format!(
                "TLS handshake failed at {}:{}",
                effective_host, port
            )),
            None => AppError::AiProvider(format!("Connection failed: {e}")),
        }
    })?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(AppError::AiProvider(
            "Authentication failed \u{2014} verify the API key, or if this is a paired client, \
             re-pair the office server (Settings \u{2192} Sharing \u{2192} Unpair, then scan a fresh code)."
                .to_string(),
        ));
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = medical_core::http_error_body::read_error_body(response, 200).await;
        return Err(AppError::AiProvider(format!(
            "Server returned HTTP {status}: {body}"
        )));
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
    validate_probe_host(&state, &host).await?;
    let effective_host = if host.is_empty() {
        "localhost".to_string()
    } else {
        host
    };
    let url = format!("http://{}:{}/v1/models", effective_host, port);

    info!(url = %url, "Testing Whisper server connection");

    let mut req = state.http_client.get(&url).timeout(Duration::from_secs(10));
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
            Some(OfflineReason::DnsFailure) => {
                AppError::SttProvider(format!("Cannot resolve hostname '{}'", effective_host))
            }
            Some(OfflineReason::TlsFailure) => AppError::SttProvider(format!(
                "TLS handshake failed at {}:{}",
                effective_host, port
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
        return Err(AppError::SttProvider(format!(
            "Server returned HTTP {status}: {body}"
        )));
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
/// timeout. If `api_key` is present and non-empty, an `Authorization: Bearer …`
/// header is sent. Returns a success message including the installed-model count,
/// or a user-readable error.
#[tauri::command]
pub async fn test_ollama_connection(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
    api_key: Option<String>,
) -> AppResult<String> {
    validate_probe_host(&state, &host).await?;
    let effective_host = if host.is_empty() {
        "localhost".to_string()
    } else {
        host
    };
    let url = format!("http://{}:{}/api/tags", effective_host, port);

    info!(url = %url, "Testing Ollama connection");

    let mut req = state.http_client.get(&url).timeout(Duration::from_secs(5));
    if let Some(key) = api_key.as_deref().filter(|s| !s.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let response = req.send().await.map_err(|e| {
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
            Some(OfflineReason::DnsFailure) => {
                AppError::AiProvider(format!("Cannot resolve hostname '{}'", effective_host))
            }
            Some(OfflineReason::TlsFailure) => AppError::AiProvider(format!(
                "TLS handshake failed at {}:{}",
                effective_host, port
            )),
            None => AppError::AiProvider(format!("Connection failed: {e}")),
        }
    })?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(AppError::AiProvider(
            "Authentication failed \u{2014} verify the API key, or if this is a paired client, \
             re-pair the office server (Settings \u{2192} Sharing \u{2192} Unpair, then scan a fresh code)."
                .to_string(),
        ));
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = medical_core::http_error_body::read_error_body(response, 200).await;
        return Err(AppError::AiProvider(format!(
            "Server returned HTTP {status}: {body}"
        )));
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

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use medical_core::error::AppError;

    use super::probe_endpoint_reachable_inner;

    #[tokio::test]
    async fn probe_endpoint_reachable_returns_ok_on_any_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let parsed: reqwest::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let result = probe_endpoint_reachable_inner(
            medical_core::error::ServiceKind::RemoteStt,
            "Whisper STT".to_string(),
            host,
            port,
            "/v1/models".to_string(),
            None,
        )
        .await;

        assert!(result.is_ok(), "200 should be Ok; got {result:?}");
    }

    #[tokio::test]
    async fn probe_endpoint_reachable_returns_ok_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(404).set_body_string("File Not Found"))
            .mount(&server)
            .await;

        let parsed: reqwest::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let result = probe_endpoint_reachable_inner(
            medical_core::error::ServiceKind::RemoteStt,
            "Whisper STT".to_string(),
            host,
            port,
            "/v1/models".to_string(),
            None,
        )
        .await;

        assert!(
            result.is_ok(),
            "404 means 'server alive, route absent' — must be Ok for reachability; got {result:?}"
        );
    }

    #[tokio::test]
    async fn probe_endpoint_reachable_returns_endpoint_offline_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let parsed: reqwest::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let result = probe_endpoint_reachable_inner(
            medical_core::error::ServiceKind::RemoteStt,
            "Whisper STT".to_string(),
            host,
            port,
            "/v1/models".to_string(),
            None,
        )
        .await;

        let err = result.expect_err("401 must surface as Err so the pill reflects auth issues");
        assert!(
            matches!(err, AppError::EndpointOffline { .. }),
            "auth failure must produce EndpointOffline; got {err:?}"
        );
    }

    #[tokio::test]
    async fn probe_endpoint_reachable_forwards_bearer_when_provided() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer secret-token",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let parsed: reqwest::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let result = probe_endpoint_reachable_inner(
            medical_core::error::ServiceKind::RemoteStt,
            "Whisper STT".to_string(),
            host,
            port,
            "/v1/models".to_string(),
            Some("secret-token".to_string()),
        )
        .await;

        assert!(
            result.is_ok(),
            "authenticated 200 should be Ok; got {result:?}"
        );
    }

    #[tokio::test]
    async fn probe_endpoint_reachable_returns_endpoint_offline_on_connect_refused() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let result = probe_endpoint_reachable_inner(
            medical_core::error::ServiceKind::RemoteStt,
            "Whisper STT".to_string(),
            "127.0.0.1".to_string(),
            port,
            "/v1/models".to_string(),
            None,
        )
        .await;

        let err = result.expect_err("connect refused must error");
        assert!(matches!(err, AppError::EndpointOffline { .. }));
    }
}
