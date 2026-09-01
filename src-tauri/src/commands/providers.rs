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
    let config = crate::commands::load_app_config(&state.db, "provider").await?;

    // Re-load paired endpoint so reinit also re-wires endpoints.
    let paired = state::load_paired_connection();
    let bearer = if paired.is_some() {
        state::load_sharing_bearer()
    } else {
        None
    };
    let eps = if let Some(ref p) = paired {
        crate::commands::sharing::paired_endpoints(p, bearer)
    } else {
        crate::commands::sharing::PairedEndpoints {
            ollama: None,
            lmstudio: None,
            omlx: None,
            whisper: None,
        }
    };

    // Rebuild AI providers with current config (includes LM Studio host/port).
    let mut ai_handles = state::init_ai_providers(&config, eps.ollama, eps.lmstudio, eps.omlx);

    // Restore the user's active provider preference from saved settings
    // so reinit doesn't silently switch to a random provider.
    ai_handles.registry.set_active(&config.ai_provider);

    let available = ai_handles.registry.list_available();

    // Destructure before any partial moves so all fields remain accessible.
    let state::AiProviderHandles {
        registry,
        ollama: new_ollama,
        lmstudio: new_lmstudio,
        omlx: new_omlx,
    } = ai_handles;
    {
        let mut guard = state.ai_providers.lock().await;
        *guard = registry;
    }

    // Rebuild STT provider based on current config (mode + whisper model + remote host/port/key).
    let stt_handles = state::init_stt_providers_with_config(&state.data_dir, &config, eps.whisper);
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
    *state.omlx_provider.write().await = new_omlx;
    *state.remote_stt_provider.write().await = new_remote_stt;

    info!(providers = ?available, "Providers reinitialized");

    Ok(available)
}

/// Per-service knobs for [`test_models_endpoint`]. The test-connection
/// commands differ only in these; everything else (bearer header, offline
/// error classification, 401/403 wording, model counting, success message
/// shape) is shared.
struct ProbeSpec {
    /// Human-readable service name for error messages ("LM Studio",
    /// "the Whisper server", "Ollama").
    service: &'static str,
    /// Endpoint path: "/v1/models" (OpenAI-compatible) or "/api/tags" (Ollama).
    path: &'static str,
    /// JSON array key holding the model listing: "data" or "models".
    array_key: &'static str,
    /// Per-request timeout.
    timeout: Duration,
    /// Success wording: Ollama reports "installed", others "available".
    installed_wording: bool,
    /// Error constructor — selects the `AppError` category surfaced to the UI.
    err: fn(String) -> AppError,
}

/// Shared body of the test-connection commands: GET the service's model
/// listing (with a Bearer key when supplied), classify transport failures
/// into actionable messages, and count the advertised models.
async fn test_models_endpoint(
    http_client: &reqwest::Client,
    host: &str,
    port: u16,
    api_key: Option<&str>,
    spec: &ProbeSpec,
) -> AppResult<String> {
    let effective_host = if host.is_empty() { "localhost" } else { host };
    let url = format!("http://{effective_host}:{port}{}", spec.path);

    info!(url = %url, "Testing {} connection", spec.service);

    let mut req = http_client.get(&url).timeout(spec.timeout);
    if let Some(key) = api_key.filter(|s| !s.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let response = req.send().await.map_err(|e| {
        use medical_core::error::OfflineReason;
        use medical_core::preflight::classify_reqwest_error;
        let err = spec.err;
        match classify_reqwest_error(&e) {
            Some(OfflineReason::ConnectionRefused) => err(format!(
                "Connection refused — is {} running at {}:{}?",
                spec.service, effective_host, port
            )),
            Some(OfflineReason::Timeout) => err(format!(
                "Connection timed out — check that {}:{} is reachable",
                effective_host, port
            )),
            Some(OfflineReason::DnsFailure) => {
                err(format!("Cannot resolve hostname '{effective_host}'"))
            }
            Some(OfflineReason::TlsFailure) => {
                err(format!("TLS handshake failed at {effective_host}:{port}"))
            }
            None => err(format!("Connection failed: {e}")),
        }
    })?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err((spec.err)(
            "Authentication failed \u{2014} verify the API key, or if this is a paired client, \
             re-pair the office server (Settings \u{2192} Sharing \u{2192} Unpair, then scan a fresh code)."
                .to_string(),
        ));
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = medical_core::http_error_body::read_error_body(response, 200).await;
        return Err((spec.err)(format!("Server returned HTTP {status}: {body}")));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| (spec.err)(format!("Invalid response from server: {e}")))?;

    let model_count = body
        .get(spec.array_key)
        .and_then(|d| d.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    let noun = if spec.installed_wording {
        "installed"
    } else {
        "available"
    };
    Ok(format!(
        "Connected — {model_count} model{} {noun}",
        if model_count == 1 { "" } else { "s" }
    ))
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
    test_models_endpoint(
        &state.http_client,
        &host,
        port,
        api_key.as_deref(),
        &ProbeSpec {
            service: "LM Studio",
            path: "/v1/models",
            array_key: "data",
            timeout: Duration::from_secs(5),
            installed_wording: false,
            err: |m| AppError::ai_provider(m),
        },
    )
    .await
}

/// Test connectivity to an oMLX server (OpenAI-compatible).
///
/// Makes a GET request to `http://{host}:{port}/v1/models` with a 5-second
/// timeout. If `api_key` is present and non-empty, an
/// `Authorization: Bearer …` header is sent. Returns a success message with
/// the model count, or an error.
#[tauri::command]
pub async fn test_omlx_connection(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
    api_key: Option<String>,
) -> AppResult<String> {
    validate_probe_host(&state, &host).await?;
    test_models_endpoint(
        &state.http_client,
        &host,
        port,
        api_key.as_deref(),
        &ProbeSpec {
            service: "oMLX",
            path: "/v1/models",
            array_key: "data",
            timeout: Duration::from_secs(5),
            installed_wording: false,
            err: |m| AppError::ai_provider(m),
        },
    )
    .await
}

/// Test connectivity to a remote Whisper server (OpenAI-compatible).
///
/// Makes a GET request to `http://{host}:{port}/v1/models` with a 10-second
/// timeout. If `api_key` is present and non-empty, an
/// `Authorization: Bearer …` header is sent.
#[tauri::command]
pub async fn test_stt_remote_connection(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
    api_key: Option<String>,
) -> AppResult<String> {
    validate_probe_host(&state, &host).await?;
    test_models_endpoint(
        &state.http_client,
        &host,
        port,
        api_key.as_deref(),
        &ProbeSpec {
            service: "the Whisper server",
            path: "/v1/models",
            array_key: "data",
            timeout: Duration::from_secs(10),
            installed_wording: false,
            err: |m| AppError::stt_provider(m),
        },
    )
    .await
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
    test_models_endpoint(
        &state.http_client,
        &host,
        port,
        api_key.as_deref(),
        &ProbeSpec {
            service: "Ollama",
            path: "/api/tags",
            array_key: "models",
            timeout: Duration::from_secs(5),
            installed_wording: true,
            err: |m| AppError::ai_provider(m),
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use medical_core::error::AppError;

    use super::{ProbeSpec, probe_endpoint_reachable_inner, test_models_endpoint};

    fn omlx_spec() -> ProbeSpec {
        ProbeSpec {
            service: "oMLX",
            path: "/v1/models",
            array_key: "data",
            timeout: Duration::from_secs(5),
            installed_wording: false,
            err: |m| AppError::ai_provider(m),
        }
    }

    #[tokio::test]
    async fn test_models_endpoint_omlx_counts_models_from_data_array() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "qwen3-8b"}, {"id": "llama-8b"}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let parsed: reqwest::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let client = reqwest::Client::new();
        let msg = test_models_endpoint(&client, &host, port, None, &omlx_spec())
            .await
            .expect("mocked /v1/models should succeed");
        assert_eq!(msg, "Connected — 2 models available");
        server.verify().await;
    }

    #[tokio::test]
    async fn test_models_endpoint_omlx_connection_refused_errors() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let client = reqwest::Client::new();
        let result = test_models_endpoint(&client, "127.0.0.1", port, None, &omlx_spec()).await;
        let err = result.expect_err("dead port must error");
        assert!(
            matches!(
                err,
                AppError::AiProvider { .. } | AppError::EndpointOffline { .. }
            ),
            "transport failure must surface as a provider error; got {err:?}"
        );
    }

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
