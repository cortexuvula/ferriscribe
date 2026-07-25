//! Context template CRUD handlers for the `/v1/context-templates` routes.
//!
//! Templates live inside AppConfig.custom_context_templates (Vec<ContextTemplate>)
//! in the SQLCipher settings row, so each mutation is read-modify-write of
//! the whole settings blob. SettingsRepo is sync; we wrap in spawn_blocking
//! to keep the axum runtime free.

use std::sync::Arc;

use axum::Json;
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use medical_core::types::settings::ContextTemplate;
use medical_db::settings::SettingsRepo;
use serde::Deserialize;
use tracing::{debug, info, warn};

use super::{ApiState, authorize};

#[derive(Deserialize)]
pub(super) struct TemplateUpsertBody {
    name: String,
    body: String,
}

#[derive(Deserialize)]
pub(super) struct TemplateRenameBody {
    old_name: String,
    new_name: String,
}

#[derive(Deserialize)]
pub(super) struct TemplateDeleteBody {
    name: String,
}

fn ctx_templates_load_sorted(
    db: &medical_db::Database,
) -> Result<Vec<ContextTemplate>, medical_core::error::AppError> {
    let conn = db.conn()?;
    let mut cfg = SettingsRepo::load_config(&conn)?;
    cfg.migrate();
    let mut t = cfg.custom_context_templates;
    t.sort_by_key(|a| a.name.to_lowercase());
    Ok(t)
}

pub(super) async fn templates_list_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ContextTemplate>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let list = tokio::task::spawn_blocking(move || ctx_templates_load_sorted(&db))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            warn!("templates list failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    debug!(count = list.len(), "vocab_api: templates list");
    Ok(Json(list))
}

pub(super) async fn templates_upsert_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(body): Json<TemplateUpsertBody>,
) -> Result<Json<ContextTemplate>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let name = body.name.trim().to_string();
    let body_text = body.body.trim().to_string();
    if name.is_empty() || body_text.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = Arc::clone(&state.db);
    let entry = tokio::task::spawn_blocking(
        move || -> Result<ContextTemplate, medical_core::error::AppError> {
            let conn = db.conn()?;
            let mut cfg = SettingsRepo::load_config(&conn)?;
            cfg.migrate();
            let entry = ContextTemplate {
                name: name.clone(),
                body: body_text.clone(),
            };
            if let Some(existing) = cfg
                .custom_context_templates
                .iter_mut()
                .find(|t| t.name == name)
            {
                existing.body = body_text;
            } else {
                cfg.custom_context_templates.push(entry.clone());
            }
            cfg.custom_context_templates
                .sort_by_key(|a| a.name.to_lowercase());
            SettingsRepo::save_config(&conn, &cfg)?;
            Ok(entry)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("templates upsert failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    info!(name_len = entry.name.len(), "vocab_api: template upserted");
    Ok(Json(entry))
}

pub(super) async fn templates_rename_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(body): Json<TemplateRenameBody>,
) -> Result<Json<ContextTemplate>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let new_name = body.new_name.trim().to_string();
    if new_name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let old_name = body.old_name;
    let db = Arc::clone(&state.db);
    let entry = tokio::task::spawn_blocking(
        move || -> Result<ContextTemplate, medical_core::error::AppError> {
            let conn = db.conn()?;
            let mut cfg = SettingsRepo::load_config(&conn)?;
            cfg.migrate();
            if old_name == new_name {
                return cfg
                    .custom_context_templates
                    .iter()
                    .find(|t| t.name == old_name)
                    .cloned()
                    .ok_or(medical_core::error::AppError::Other(format!(
                        "'{old_name}' not found"
                    )));
            }
            if cfg
                .custom_context_templates
                .iter()
                .any(|t| t.name == new_name)
            {
                return Err(medical_core::error::AppError::Other(format!(
                    "'{new_name}' already exists"
                )));
            }
            let idx = cfg
                .custom_context_templates
                .iter()
                .position(|t| t.name == old_name)
                .ok_or(medical_core::error::AppError::Other(format!(
                    "'{old_name}' not found"
                )))?;
            cfg.custom_context_templates[idx].name = new_name.clone();
            let renamed = cfg.custom_context_templates[idx].clone();
            cfg.custom_context_templates
                .sort_by_key(|a| a.name.to_lowercase());
            SettingsRepo::save_config(&conn, &cfg)?;
            Ok(renamed)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("templates rename: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(entry))
}

pub(super) async fn templates_delete_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(body): Json<TemplateDeleteBody>,
) -> Result<StatusCode, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let name = body.name;
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> Result<(), medical_core::error::AppError> {
        let conn = db.conn()?;
        let mut cfg = SettingsRepo::load_config(&conn)?;
        cfg.migrate();
        let idx = cfg
            .custom_context_templates
            .iter()
            .position(|t| t.name == name)
            .ok_or(medical_core::error::AppError::Other(format!(
                "'{name}' not found"
            )))?;
        cfg.custom_context_templates.remove(idx);
        SettingsRepo::save_config(&conn, &cfg)?;
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("templates delete: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::NO_CONTENT)
}
