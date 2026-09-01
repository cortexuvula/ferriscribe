use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use tracing::{info, instrument};

use medical_core::error::{AppError, AppResult};
use medical_core::types::settings::ContextTemplate;
use medical_db::settings::SettingsRepo;

use crate::state::{self, AppState};
use crate::templates_remote::TemplatesRemote;

/// Returns `Some((conn, bearer))` when this client is paired with an office
/// server that exposes the templates API. Context-templates commands route
/// through HTTP in that case; otherwise they operate on local SettingsRepo.
fn paired_templates_target() -> Option<(crate::commands::sharing::PairedConnection, String)> {
    let conn = state::load_paired_connection()?;
    conn.ports.vocab?;
    let bearer = state::load_sharing_bearer()?;
    Some((conn, bearer))
}

/// Sort templates alphabetically by name (case-insensitive).
pub fn sort_templates(templates: &mut [ContextTemplate]) {
    templates.sort_by_key(|a| a.name.to_lowercase());
}

/// Insert or replace a template in-place.  Case-sensitive name match.
/// The vec is re-sorted afterwards.
pub fn upsert_into(
    templates: &mut Vec<ContextTemplate>,
    name: String,
    body: String,
) -> ContextTemplate {
    let entry = ContextTemplate {
        name: name.clone(),
        body: body.clone(),
    };
    if let Some(existing) = templates.iter_mut().find(|t| t.name == name) {
        existing.body = body;
    } else {
        templates.push(entry.clone());
    }
    sort_templates(templates);
    entry
}

/// Rename a template.  Errors if `old_name` does not exist, or if `new_name`
/// is different and already exists.
pub fn rename_in(
    templates: &mut [ContextTemplate],
    old_name: &str,
    new_name: String,
) -> AppResult<ContextTemplate> {
    if old_name == new_name {
        return templates
            .iter()
            .find(|t| t.name == old_name)
            .cloned()
            .ok_or_else(|| AppError::Config(format!("Template '{old_name}' not found")));
    }
    if templates.iter().any(|t| t.name == new_name) {
        return Err(AppError::Config(format!(
            "A template named '{new_name}' already exists"
        )));
    }
    let idx = templates
        .iter()
        .position(|t| t.name == old_name)
        .ok_or_else(|| AppError::Config(format!("Template '{old_name}' not found")))?;
    templates[idx].name = new_name;
    let renamed = templates[idx].clone();
    sort_templates(templates);
    Ok(renamed)
}

/// Remove a template by name.
pub fn delete_in(templates: &mut Vec<ContextTemplate>, name: &str) -> AppResult<()> {
    let idx = templates
        .iter()
        .position(|t| t.name == name)
        .ok_or_else(|| AppError::Config(format!("Template '{name}' not found")))?;
    templates.remove(idx);
    Ok(())
}

/// Accepted JSON shapes for import.  Tried in order; the first match wins.
#[derive(Deserialize)]
#[serde(untagged)]
enum ImportShape {
    /// { "custom_context_templates": { "Name": "body", ... } }
    Wrapped {
        custom_context_templates: BTreeMap<String, String>,
    },
    /// Bare array of objects
    BareArray(Vec<ContextTemplate>),
    /// Bare { "Name": "body", ... } dict
    BareDict(BTreeMap<String, String>),
}

/// Parse any accepted JSON shape into a list of templates.
pub fn parse_import_json(content: &str) -> AppResult<Vec<ContextTemplate>> {
    let shape: ImportShape = serde_json::from_str(content)?;
    Ok(match shape {
        ImportShape::Wrapped {
            custom_context_templates,
        } => custom_context_templates
            .into_iter()
            .map(|(name, body)| ContextTemplate { name, body })
            .collect(),
        ImportShape::BareArray(list) => list,
        ImportShape::BareDict(map) => map
            .into_iter()
            .map(|(name, body)| ContextTemplate { name, body })
            .collect(),
    })
}

/// Apply imported entries into an existing template list.  Skips empty-name
/// or empty-body entries.  Existing names are overwritten (upsert).  Returns
/// the count actually applied.
pub fn apply_import(templates: &mut Vec<ContextTemplate>, imported: Vec<ContextTemplate>) -> u32 {
    let mut count = 0u32;
    for entry in imported {
        let name = entry.name.trim().to_string();
        let body = entry.body.trim().to_string();
        if name.is_empty() || body.is_empty() {
            continue;
        }
        upsert_into(templates, name, body);
        count += 1;
    }
    count
}

/// Serialise templates to the canonical wrapped JSON form used for export.
pub fn export_json(templates: &[ContextTemplate]) -> AppResult<String> {
    let map: BTreeMap<&str, &str> = templates
        .iter()
        .map(|t| (t.name.as_str(), t.body.as_str()))
        .collect();
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "custom_context_templates": map,
    }))?;
    Ok(json)
}

/// Load `AppConfig` from the DB on a blocking worker (see
/// [`crate::commands::load_app_config`]).
async fn load_config(
    db: &Arc<medical_db::Database>,
) -> AppResult<medical_core::types::settings::AppConfig> {
    crate::commands::load_app_config(db, "context template").await
}

/// Save `AppConfig` to the DB inside `spawn_blocking`.
async fn save_config(
    db: &Arc<medical_db::Database>,
    config: &medical_core::types::settings::AppConfig,
) -> AppResult<()> {
    // SettingsRepo::save_config is synchronous; clone the config into an
    // owned Arc so the move closure can take it across the thread boundary
    // without borrowing.
    let config = Arc::new(config.clone());
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = db.conn()?;
        SettingsRepo::save_config(&conn, &config).map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)?
}

#[tauri::command]
pub async fn list_context_templates(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<ContextTemplate>> {
    if let Some((conn, bearer)) = paired_templates_target() {
        let remote = TemplatesRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired templates target unavailable".into()))?;
        return remote.list().await;
    }
    let config = load_config(&state.db).await?;
    let mut templates = config.custom_context_templates;
    sort_templates(&mut templates);
    Ok(templates)
}

#[tauri::command]
pub async fn upsert_context_template(
    state: tauri::State<'_, AppState>,
    name: String,
    body: String,
) -> AppResult<ContextTemplate> {
    let name = name.trim().to_string();
    let body = body.trim().to_string();
    if name.is_empty() {
        return Err(AppError::Config(
            "Template name cannot be empty".to_string(),
        ));
    }
    if body.is_empty() {
        return Err(AppError::Config(
            "Template body cannot be empty".to_string(),
        ));
    }
    if let Some((conn, bearer)) = paired_templates_target() {
        let remote = TemplatesRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired templates target unavailable".into()))?;
        let entry = remote.upsert(&name, &body).await?;
        info!(
            name_len = entry.name.chars().count(),
            "Upserted context template (remote)"
        );
        return Ok(entry);
    }
    let mut config = load_config(&state.db).await?;
    let result = upsert_into(&mut config.custom_context_templates, name, body);
    save_config(&state.db, &config).await?;
    info!(
        name_len = result.name.chars().count(),
        "Upserted context template"
    );
    Ok(result)
}

#[tauri::command]
pub async fn rename_context_template(
    state: tauri::State<'_, AppState>,
    old_name: String,
    new_name: String,
) -> AppResult<ContextTemplate> {
    let new_name = new_name.trim().to_string();
    if new_name.is_empty() {
        return Err(AppError::Config(
            "Template name cannot be empty".to_string(),
        ));
    }
    if let Some((conn, bearer)) = paired_templates_target() {
        let remote = TemplatesRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired templates target unavailable".into()))?;
        let entry = remote.rename(&old_name, &new_name).await?;
        info!(
            name_len = entry.name.chars().count(),
            "Renamed context template (remote)"
        );
        return Ok(entry);
    }
    let mut config = load_config(&state.db).await?;
    let result = rename_in(&mut config.custom_context_templates, &old_name, new_name)?;
    save_config(&state.db, &config).await?;
    info!(
        name_len = result.name.chars().count(),
        "Renamed context template"
    );
    Ok(result)
}

#[tauri::command]
pub async fn delete_context_template(
    state: tauri::State<'_, AppState>,
    name: String,
) -> AppResult<()> {
    if let Some((conn, bearer)) = paired_templates_target() {
        let remote = TemplatesRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired templates target unavailable".into()))?;
        remote.delete(&name).await?;
        info!("Deleted context template (remote)");
        return Ok(());
    }
    let mut config = load_config(&state.db).await?;
    delete_in(&mut config.custom_context_templates, &name)?;
    save_config(&state.db, &config).await?;
    info!("Deleted context template");
    Ok(())
}

#[tauri::command]
#[instrument(skip(state))]
pub async fn import_context_templates_json(
    state: tauri::State<'_, AppState>,
    file_path: String,
) -> AppResult<u32> {
    let file_path = crate::commands::validate_user_path(&file_path)?;
    let content = tokio::fs::read_to_string(&file_path).await?;
    let imported = parse_import_json(&content)?;
    if let Some((conn, bearer)) = paired_templates_target() {
        let remote = TemplatesRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired templates target unavailable".into()))?;
        let mut count = 0u32;
        for entry in imported {
            let name = entry.name.trim();
            let body = entry.body.trim();
            if name.is_empty() || body.is_empty() {
                continue;
            }
            remote.upsert(name, body).await?;
            count += 1;
        }
        info!(count, path = %file_path.display(), "Imported context templates (remote)");
        return Ok(count);
    }
    let mut config = load_config(&state.db).await?;
    let count = apply_import(&mut config.custom_context_templates, imported);
    save_config(&state.db, &config).await?;
    info!(count, path = %file_path.display(), "Imported context templates");
    Ok(count)
}

#[tauri::command]
#[instrument(skip(state))]
pub async fn export_context_templates_json(
    state: tauri::State<'_, AppState>,
    file_path: String,
) -> AppResult<u32> {
    let file_path = crate::commands::validate_user_path(&file_path)?;
    let templates: Vec<ContextTemplate> = if let Some((conn, bearer)) = paired_templates_target() {
        let remote = TemplatesRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired templates target unavailable".into()))?;
        remote.list().await?
    } else {
        let config = load_config(&state.db).await?;
        config.custom_context_templates
    };
    let count = templates.len() as u32;
    let json = export_json(&templates)?;
    tokio::fs::write(&file_path, json).await?;
    info!(count, path = %file_path.display(), "Exported context templates");
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<ContextTemplate> {
        vec![
            ContextTemplate {
                name: "Follow-up".into(),
                body: "Follow-up visit.".into(),
            },
            ContextTemplate {
                name: "Telehealth".into(),
                body: "Video consult.".into(),
            },
        ]
    }

    #[test]
    fn sort_is_case_insensitive_alphabetical() {
        let mut v = vec![
            ContextTemplate {
                name: "zebra".into(),
                body: "z".into(),
            },
            ContextTemplate {
                name: "Apple".into(),
                body: "a".into(),
            },
            ContextTemplate {
                name: "banana".into(),
                body: "b".into(),
            },
        ];
        sort_templates(&mut v);
        assert_eq!(v[0].name, "Apple");
        assert_eq!(v[1].name, "banana");
        assert_eq!(v[2].name, "zebra");
    }

    #[test]
    fn upsert_inserts_new() {
        let mut v = Vec::new();
        let result = upsert_into(&mut v, "New".into(), "body".into());
        assert_eq!(result.name, "New");
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn upsert_replaces_existing_body() {
        let mut v = sample();
        upsert_into(&mut v, "Follow-up".into(), "updated body".into());
        assert_eq!(v.len(), 2);
        let fu = v.iter().find(|t| t.name == "Follow-up").unwrap();
        assert_eq!(fu.body, "updated body");
    }

    #[test]
    fn rename_changes_name_and_re_sorts() {
        let mut v = sample();
        let renamed = rename_in(&mut v, "Follow-up", "Zebra-visit".into()).unwrap();
        assert_eq!(renamed.name, "Zebra-visit");
        assert_eq!(v.last().unwrap().name, "Zebra-visit");
    }

    #[test]
    fn rename_to_existing_name_errors() {
        let mut v = sample();
        let err = rename_in(&mut v, "Follow-up", "Telehealth".into()).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn rename_identity_returns_template() {
        let mut v = sample();
        let r = rename_in(&mut v, "Follow-up", "Follow-up".into()).unwrap();
        assert_eq!(r.name, "Follow-up");
    }

    #[test]
    fn rename_missing_errors() {
        let mut v = sample();
        let err = rename_in(&mut v, "Missing", "Other".into()).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn delete_removes_entry() {
        let mut v = sample();
        delete_in(&mut v, "Follow-up").unwrap();
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn delete_missing_errors() {
        let mut v = Vec::new();
        assert!(delete_in(&mut v, "Missing").is_err());
    }

    #[test]
    fn parse_wrapped_json() {
        let json = r#"{"custom_context_templates": {"A": "body A", "B": "body B"}}"#;
        let parsed = parse_import_json(json).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn parse_bare_dict() {
        let json = r#"{"A": "body A", "B": "body B"}"#;
        let parsed = parse_import_json(json).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn parse_bare_array() {
        let json = r#"[{"name": "A", "body": "body A"}, {"name": "B", "body": "body B"}]"#;
        let parsed = parse_import_json(json).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn parse_invalid_errors() {
        assert!(parse_import_json("not json").is_err());
    }

    #[test]
    fn apply_import_skips_empty_and_trims() {
        let mut v: Vec<ContextTemplate> = Vec::new();
        let imported = vec![
            ContextTemplate {
                name: "  Keep  ".into(),
                body: "  yes  ".into(),
            },
            ContextTemplate {
                name: "".into(),
                body: "skip".into(),
            },
            ContextTemplate {
                name: "skip".into(),
                body: "".into(),
            },
        ];
        let count = apply_import(&mut v, imported);
        assert_eq!(count, 1);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "Keep");
        assert_eq!(v[0].body, "yes");
    }

    #[test]
    fn apply_import_upserts_existing() {
        let mut v = sample();
        let imported = vec![ContextTemplate {
            name: "Follow-up".into(),
            body: "new body".into(),
        }];
        let count = apply_import(&mut v, imported);
        assert_eq!(count, 1);
        assert_eq!(v.len(), 2);
        let fu = v.iter().find(|t| t.name == "Follow-up").unwrap();
        assert_eq!(fu.body, "new body");
    }

    #[test]
    fn export_produces_wrapped_form_and_round_trips() {
        let v = sample();
        let json = export_json(&v).expect("serialize");
        assert!(json.contains("custom_context_templates"));
        let reparsed = parse_import_json(&json).unwrap();
        assert_eq!(reparsed.len(), 2);
    }
}
