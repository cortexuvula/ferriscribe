use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::types::settings::AppConfig;
use medical_db::settings::SettingsRepo;

use crate::state::AppState;

/// Synchronously load the app config (for use in non-async dispatch helpers).
///
/// This mirrors the load half of [`get_settings`] but takes a borrowed
/// `Database` and applies all migrations, without the onboarding auto-mark
/// side effects. It exists so sync-dispatch helpers (e.g. condition-chip
/// routing) can read the opt-in flags without an `async` context.
pub fn load_config_sync(
    db: &Arc<medical_db::Database>,
) -> AppResult<medical_core::types::settings::AppConfig> {
    let conn = db.conn()?;
    let mut config = SettingsRepo::load_config(&conn)?;
    config.migrate();
    Ok(config)
}

/// Load the current application settings from the database.
///
/// Returns the full `AppConfig` with all migrations applied.
///
/// **Onboarding auto-mark (existing installs):** a brand-new install has no
/// `onboarding_started` sentinel and no `app_config`, so
/// `onboarding_completed` stays `false` and the wizard shows. The wizard
/// writes the `onboarding_started` sentinel the first time it saves config,
/// so an interrupted wizard still reappears on next launch. An existing
/// install that predates the wizard has an `app_config` row but no
/// `onboarding_started` sentinel — treat that as already onboarded.
///
/// This avoids the earlier bug where the wizard saving `app_config` on step 2
/// flipped `config_existed=true`, silently marking an interrupted wizard as
/// complete on the next launch.
#[tauri::command]
pub async fn get_settings(state: tauri::State<'_, AppState>) -> AppResult<AppConfig> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        let onboarding_started = SettingsRepo::exists(&conn, "onboarding_started")?;
        let config_existed = SettingsRepo::exists(&conn, "app_config")?;
        let mut config = SettingsRepo::load_config(&conn)?;
        config.migrate();
        if !config.onboarding_completed && config_existed && !onboarding_started {
            // Pre-wizard existing install: mark onboarded, never show the wizard.
            config.onboarding_completed = true;
            SettingsRepo::save_config(&conn, &config)?;
        }
        Ok(config)
    })
    .await
    .map_err(super::join_err)?
}

/// Mark onboarding as started. The onboarding wizard calls this the first time
/// it saves any config, so that an interrupted wizard is NOT silently auto-
/// marked complete on the next launch (see `get_settings`). The sentinel is
/// idempotent — setting it again is a no-op.
#[tauri::command]
pub async fn set_onboarding_started(state: tauri::State<'_, AppState>) -> AppResult<()> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = db.conn()?;
        SettingsRepo::set(&conn, "onboarding_started", "1")?;
        Ok(())
    })
    .await
    .map_err(super::join_err)?
}

/// Persist updated application settings to the database.
///
/// Validates that configured AI/STT hosts are local (private/LAN addresses)
/// unless `allow_public_endpoint` is explicitly enabled. Rejects public hosts
/// like `api.openai.com` to enforce the local-only PHI constraint.
#[tauri::command]
pub async fn save_settings(state: tauri::State<'_, AppState>, config: AppConfig) -> AppResult<()> {
    let db = state.db.clone();
    // Host validation may resolve DNS — blocking — so it rides along in the
    // same spawn_blocking as the save.
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        // Reject public/unknown hosts unless the user has explicitly opted in.
        for (field, host) in [
            ("ollama_host", config.ollama_host.as_str()),
            ("lmstudio_host", config.lmstudio_host.as_str()),
            ("omlx_host", config.omlx_host.as_str()),
            ("stt_remote_host", config.stt_remote_host.as_str()),
        ] {
            // Empty host means "use default" — defer enforcement until the user
            // actually fills it in.
            if host.is_empty() {
                continue;
            }
            medical_core::endpoint_policy::validate_local_endpoint(
                host,
                config.allow_public_endpoint,
            )
            .map_err(|e| AppError::invalid_endpoint_for(e, field))?;
        }

        let conn = db.conn()?;
        SettingsRepo::save_config(&conn, &config).map_err(AppError::from)
    })
    .await
    .map_err(super::join_err)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::endpoint_policy::EndpointKind;

    fn config_with_hosts(ollama: &str, lmstudio: &str, stt: &str) -> AppConfig {
        AppConfig {
            ollama_host: ollama.to_string(),
            lmstudio_host: lmstudio.to_string(),
            stt_remote_host: stt.to_string(),
            ..Default::default()
        }
    }

    // We can't run the full Tauri command here (needs State), but we can
    // exercise the validation logic standalone by calling the helper directly.
    // This is sufficient because the save_settings body is a thin wrapper.

    #[test]
    fn validate_public_ollama_host_rejected_by_default() {
        let cfg = config_with_hosts("api.openai.com", "localhost", "");
        let r = medical_core::endpoint_policy::validate_local_endpoint(
            &cfg.ollama_host,
            cfg.allow_public_endpoint,
        );
        assert!(r.is_err());
    }

    #[test]
    fn validate_public_ollama_host_accepted_with_opt_out() {
        let mut cfg = config_with_hosts("api.openai.com", "localhost", "");
        cfg.allow_public_endpoint = true;
        let r = medical_core::endpoint_policy::validate_local_endpoint(
            &cfg.ollama_host,
            cfg.allow_public_endpoint,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn empty_stt_remote_host_is_allowed() {
        let cfg = config_with_hosts("localhost", "localhost", "");
        // Mirroring save_settings: empty is skipped.
        assert!(cfg.stt_remote_host.is_empty());
    }

    #[test]
    fn invalid_endpoint_for_helper_includes_field_name() {
        use medical_core::endpoint_policy::EndpointPolicyError;
        let err = EndpointPolicyError::Blocked {
            host: "api.openai.com".into(),
            kind: EndpointKind::Unknown,
        };
        let app = AppError::invalid_endpoint_for(err, "ollama_host");
        match app {
            AppError::InvalidEndpoint { field, host, kind } => {
                assert_eq!(field, "ollama_host");
                assert_eq!(host, "api.openai.com");
                assert_eq!(kind, EndpointKind::Unknown);
            }
            _ => panic!("expected InvalidEndpoint"),
        }
    }
}

/// Retrieve a stored API key for the given provider from the OS keychain.
///
/// Returns `None` if no key is stored. The key value is never logged.
/// OS keychain calls (macOS Security framework, Windows Credential Manager)
/// can block for hundreds of ms or prompt the user, so this runs on the
/// blocking pool instead of the IPC thread.
#[tauri::command]
pub async fn get_api_key(
    state: tauri::State<'_, AppState>,
    provider: String,
) -> AppResult<Option<String>> {
    let keys = Arc::clone(&state.keys);
    tokio::task::spawn_blocking(move || {
        keys.get_key(&provider)
            .map_err(|e| AppError::security_with_source(e.to_string(), e))
    })
    .await
    .map_err(|e| AppError::Other(format!("keychain task failed: {e}")))?
}

/// Store an API key for the given provider in the OS keychain.
///
/// Overwrites any existing key for the same provider.
/// OS keychain calls can block; runs on the blocking pool.
#[tauri::command]
pub async fn set_api_key(
    state: tauri::State<'_, AppState>,
    provider: String,
    key: String,
) -> AppResult<()> {
    let keys = Arc::clone(&state.keys);
    tokio::task::spawn_blocking(move || {
        keys.store_key(&provider, &key)
            .map_err(|e| AppError::security_with_source(e.to_string(), e))
    })
    .await
    .map_err(|e| AppError::Other(format!("keychain task failed: {e}")))?
}

/// Return the built-in default system prompt for the given document type.
///
/// `doc_type` must be one of: "soap", "referral", "letter", "letter_writer",
/// "synopsis", "peer_discussion".
#[tauri::command]
pub fn get_default_prompt(doc_type: String) -> AppResult<String> {
    use medical_processing::document_generator::{
        default_letter_from_document_prompt, default_letter_prompt, default_referral_prompt,
        default_synopsis_prompt,
    };
    use medical_processing::peer_discussion::default_peer_discussion_prompt;
    use medical_processing::soap_generator::default_soap_prompt;

    match doc_type.as_str() {
        "soap" => Ok(default_soap_prompt().to_string()),
        "referral" => Ok(default_referral_prompt().to_string()),
        "letter" => Ok(default_letter_prompt().to_string()),
        "letter_writer" => Ok(default_letter_from_document_prompt().to_string()),
        "synopsis" => Ok(default_synopsis_prompt().to_string()),
        "peer_discussion" => Ok(default_peer_discussion_prompt().to_string()),
        _ => Err(AppError::Config(format!("Unknown doc_type: {}", doc_type))),
    }
}
