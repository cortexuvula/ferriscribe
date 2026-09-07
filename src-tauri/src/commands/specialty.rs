//! Specialty prompt-pack commands: list discovered packs for Settings, and
//! resolve a pack's prompt artifact at generation time.
//!
//! Packs are LOCAL files only — bundled packs are compiled into
//! `medical-processing`; user packs live under `<data_dir>/specialties/`.
//! Nothing here performs network I/O, and only pack ids/versions are ever
//! logged (never prompt content).

use medical_core::error::{AppError, AppResult};
use medical_core::types::settings::AppConfig;
use medical_processing::specialty::{
    DocType, SpecialtyPackInfo, list_packs, resolve_pack_artifact,
};

use crate::state::AppState;

use super::generation::MAX_CONTEXT_CHARS;

/// The app-data directory holding user-installed packs.
fn user_packs_dir(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("specialties")
}

/// Every discovered pack (bundled + user), plus per-directory load errors,
/// for the Settings → Prompts specialty picker. Broken pack directories are
/// surfaced with their `error` — never silently skipped.
#[tauri::command]
pub async fn list_specialty_packs(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<SpecialtyPackInfo>> {
    let data_dir = state.data_dir.clone();
    tokio::task::spawn_blocking(move || {
        let dir = user_packs_dir(&data_dir);
        Ok(list_packs(Some(&dir)))
    })
    .await
    .map_err(crate::commands::join_err)?
}

/// Validate a config's `specialty` selection before saving.
///
/// Only a CHANGED selection is validated: if the save switches the specialty
/// to an id that resolves to no discovered pack (bundled or user), it is
/// rejected with an actionable message. A selection that is UNCHANGED from
/// the stored config always passes — a pack that vanished (or a folder that
/// is transiently unreadable) must never brick unrelated settings saves;
/// generation already falls back to the built-in prompts, and the Prompts
/// pane surfaces the missing pack.
pub(crate) fn validate_specialty_selection(
    previous_specialty: Option<&str>,
    config: &AppConfig,
    data_dir: &std::path::Path,
) -> AppResult<()> {
    let Some(id) = config
        .specialty
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(());
    };
    if previous_specialty.map(str::trim) == Some(id) {
        return Ok(());
    }
    let dir = user_packs_dir(data_dir);
    let packs = list_packs(Some(&dir));
    if packs.iter().any(|p| p.error.is_none() && p.id == id) {
        return Ok(());
    }
    Err(AppError::InvalidInput(format!(
        "Unknown specialty pack \"{id}\". Pick an available specialty in Settings → Prompts."
    )))
}

/// Resolve the selected specialty pack's prompt body for one document type,
/// ready to hand to the generation prompt builders (which assemble it with
/// the compiled-in safety block).
///
/// `Ok(None)` = no specialty selected, the id resolves to nothing, or the
/// pack does not provide this doc type — the caller falls back to the
/// built-in prompt (per-artifact fallback, never a partial output).
///
/// Runs the directory scan on the blocking pool (small file reads). The
/// artifact passes the same size cap every custom prompt gets — user pack
/// files are user input flowing into a system prompt.
pub(super) async fn resolve_specialty_prompt(
    state: &AppState,
    config: &AppConfig,
    doc: DocType,
) -> AppResult<Option<String>> {
    let Some(id) = config
        .specialty
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };

    let data_dir = state.data_dir.clone();
    let id = id.to_string();
    // Kept out of the move closure so the size-cap error can name the pack.
    let id_for_error = id.clone();
    tokio::task::spawn_blocking(move || {
        let dir = user_packs_dir(&data_dir);
        resolve_pack_artifact(Some(&id), doc, Some(&dir))
    })
    .await
    .map_err(|e| AppError::Other(format!("specialty pack resolution task failed: {e}")))?
    .map(move |body| -> AppResult<String> {
        if body.len() > MAX_CONTEXT_CHARS {
            return Err(AppError::InvalidInput(format!(
                "Specialty pack \"{id_for_error}\" {} prompt too large: {} chars, \
                 limit is {MAX_CONTEXT_CHARS}.",
                doc.as_str(),
                body.len(),
            )));
        }
        Ok(body)
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write_pack(root: &Path, id: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("manifest.json"),
            format!(
                r#"{{ "schema_version": 1, "id": "{id}", "name": "{id}", "version": "1.0.0",
                     "description": "test pack", "prompts": {{ "soap": "soap_prompt.md" }} }}"#
            ),
        )
        .expect("write manifest");
        std::fs::write(dir.join("soap_prompt.md"), format!("{id} body")).expect("write artifact");
    }

    #[test]
    fn validate_accepts_unset_bundled_and_user_ids() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut config = AppConfig::default();

        // Unset / empty always valid (no previous selection).
        validate_specialty_selection(None, &config, tmp.path()).expect("unset valid");
        config.specialty = Some("  ".into());
        validate_specialty_selection(None, &config, tmp.path()).expect("whitespace valid");

        // Bundled ids validate with no user dir at all.
        config.specialty = Some("family-medicine".into());
        validate_specialty_selection(None, &config, tmp.path()).expect("bundled id valid");

        // User-installed id validates too (packs live under the
        // data_dir/specialties subdirectory).
        write_pack(&tmp.path().join("specialties"), "my-specialty");
        config.specialty = Some("my-specialty".into());
        validate_specialty_selection(None, &config, tmp.path()).expect("user id valid");

        // Changing from one valid id to another valid id passes.
        config.specialty = Some("family-medicine".into());
        validate_specialty_selection(Some("my-specialty"), &config, tmp.path())
            .expect("valid switch valid");
    }

    #[test]
    fn validate_rejects_changed_to_unknown_ids() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut config = AppConfig::default();

        config.specialty = Some("ghost-pack".into());
        let err = validate_specialty_selection(None, &config, tmp.path())
            .expect_err("changed to unknown id");
        assert!(
            err.to_string().contains("ghost-pack") && err.to_string().contains("Settings"),
            "error must name the id and where to fix it: {err}"
        );

        // A pack dir that exists but is broken (no manifest) is NOT a valid
        // selection — the id resolves to nothing usable.
        std::fs::create_dir_all(tmp.path().join("specialties").join("broken")).expect("mkdir");
        config.specialty = Some("broken".into());
        assert!(validate_specialty_selection(None, &config, tmp.path()).is_err());
    }

    /// The lockout regression: an UNCHANGED selection must pass even when
    /// its pack has vanished — unrelated settings saves (theme, hosts, …)
    /// must not be blocked by a stale specialty id. Generation falls back
    /// to the built-in prompts and the Prompts pane surfaces the gap.
    #[test]
    fn validate_unchanged_selection_passes_when_pack_vanished() {
        let tmp = tempfile::tempdir().expect("tmp");
        let config = AppConfig {
            specialty: Some("vanished-pack".into()),
            ..AppConfig::default()
        };
        validate_specialty_selection(Some("vanished-pack"), &config, tmp.path())
            .expect("unchanged selection must not block the save");
    }
}
