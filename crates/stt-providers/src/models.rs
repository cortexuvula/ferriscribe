//! Model metadata, download/delete, and path helpers.
//!
//! Manages the on-disk model catalog for Whisper (ggml `.bin` files) and
//! pyannote (ONNX files). Models are stored under `{app_data_dir}/models/`
//! with separate subdirectories for `whisper/` and `pyannote/`.
//!
//! # Downloads
//!
//! Downloads use a write-to-`.tmp`-then-rename strategy for crash safety:
//! if the process is interrupted, the partial `.tmp` file is left behind
//! but the target model path is never corrupted.
//!
//! # Model Catalog
//!
//! - **Whisper**: `base` (~148 MB), `base-q5_1` (~60 MB), `small` (~488 MB),
//!   `small-q5_1` (~190 MB), `medium` (~1.5 GB), `large-v3-turbo` (~1.6 GB),
//!   `large-v3-turbo-q5_0` (~574 MB)
//! - **Pyannote**: `segmentation-3.0.onnx` (~6 MB), `wespeaker_en_voxceleb_CAM++.onnx` (~28 MB)
//!
//! The `q5` variants are the same models with 5-bit quantization: a third
//! to an eighth of the download and much faster to load, at a small
//! accuracy cost — the turbo q5_0 in particular keeps large-v3-turbo
//! accuracy at ~⅓ the size.

use crate::SttError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// WhisperModelId
// ---------------------------------------------------------------------------

/// Identifies a Whisper model variant by name.
///
/// Maps to ggml filenames: `Base` → `ggml-base.bin`, etc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhisperModelId {
    /// Whisper Base — fastest, lowest accuracy (~148 MB).
    Base,
    /// Whisper Base Q5_1 — quantized base (~60 MB).
    BaseQ5_1,
    /// Whisper Small — balanced speed and accuracy (~488 MB).
    Small,
    /// Whisper Small Q5_1 — quantized small (~190 MB).
    SmallQ5_1,
    /// Whisper Medium — high accuracy (~1.5 GB).
    Medium,
    /// Whisper Large-v3-Turbo — best accuracy (~1.6 GB).
    LargeV3Turbo,
    /// Whisper Large-v3-Turbo Q5_0 — quantized turbo, the size/speed
    /// sweet spot (~574 MB).
    LargeV3TurboQ5_0,
}

impl WhisperModelId {
    /// Return the string identifier for this model (e.g. `"base"`, `"large-v3-turbo"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            WhisperModelId::Base => "base",
            WhisperModelId::BaseQ5_1 => "base-q5_1",
            WhisperModelId::Small => "small",
            WhisperModelId::SmallQ5_1 => "small-q5_1",
            WhisperModelId::Medium => "medium",
            WhisperModelId::LargeV3Turbo => "large-v3-turbo",
            WhisperModelId::LargeV3TurboQ5_0 => "large-v3-turbo-q5_0",
        }
    }

    /// Parse a string identifier into a `WhisperModelId`. Returns `None` for
    /// unrecognized strings.
    ///
    /// Not implemented as `std::str::FromStr` because this returns `Option`,
    /// not `Result`. Named `from_str` for call-site readability.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "base" => Some(WhisperModelId::Base),
            "base-q5_1" => Some(WhisperModelId::BaseQ5_1),
            "small" => Some(WhisperModelId::Small),
            "small-q5_1" => Some(WhisperModelId::SmallQ5_1),
            "medium" => Some(WhisperModelId::Medium),
            "large-v3-turbo" => Some(WhisperModelId::LargeV3Turbo),
            "large-v3-turbo-q5_0" => Some(WhisperModelId::LargeV3TurboQ5_0),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// ModelInfo
// ---------------------------------------------------------------------------

/// Metadata about a downloadable model (Whisper or pyannote).
///
/// Used by the Settings UI to list available models with their download status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier (e.g. `"base"`, `"pyannote-segmentation"`).
    pub id: String,
    /// On-disk filename within the model subdirectory.
    pub filename: String,
    /// Expected file size in bytes.
    pub size_bytes: u64,
    /// URL to download the model from (Hugging Face or GitHub releases).
    pub download_url: String,
    /// Human-readable description shown in the Settings UI.
    pub description: String,
    /// Whether the model file already exists on disk.
    pub downloaded: bool,
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Return the root models directory: `{app_data_dir}/models`.
pub fn models_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models")
}

/// Return the Whisper models directory: `{app_data_dir}/models/whisper`.
pub fn whisper_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models").join("whisper")
}

/// Return the pyannote models directory: `{app_data_dir}/models/pyannote`.
pub fn pyannote_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models").join("pyannote")
}

/// Return the full path to a Whisper model file: `{whisper_dir}/{filename}`.
pub fn whisper_model_path(app_data_dir: &Path, filename: &str) -> PathBuf {
    whisper_dir(app_data_dir).join(filename)
}

/// Return the full path to a pyannote model file: `{pyannote_dir}/{filename}`.
pub fn pyannote_model_path(app_data_dir: &Path, filename: &str) -> PathBuf {
    pyannote_dir(app_data_dir).join(filename)
}

// ---------------------------------------------------------------------------
// Filename mapping
// ---------------------------------------------------------------------------

/// Map a model ID string to its ggml filename. Returns `None` for unknown IDs.
///
/// | ID | Filename |
/// |---|---|
/// | `"base"` | `ggml-base.bin` |
/// | `"base-q5_1"` | `ggml-base-q5_1.bin` |
/// | `"small"` | `ggml-small.bin` |
/// | `"small-q5_1"` | `ggml-small-q5_1.bin` |
/// | `"medium"` | `ggml-medium.bin` |
/// | `"large-v3-turbo"` | `ggml-large-v3-turbo.bin` |
/// | `"large-v3-turbo-q5_0"` | `ggml-large-v3-turbo-q5_0.bin` |
pub fn whisper_model_filename(model_id: &str) -> Option<&'static str> {
    match model_id {
        "base" => Some("ggml-base.bin"),
        "base-q5_1" => Some("ggml-base-q5_1.bin"),
        "small" => Some("ggml-small.bin"),
        "small-q5_1" => Some("ggml-small-q5_1.bin"),
        "medium" => Some("ggml-medium.bin"),
        "large-v3-turbo" => Some("ggml-large-v3-turbo.bin"),
        "large-v3-turbo-q5_0" => Some("ggml-large-v3-turbo-q5_0.bin"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Available models
// ---------------------------------------------------------------------------

/// List all available Whisper models with their download status.
///
/// Checks each model's expected path under `app_data_dir` and sets `downloaded: true`
/// if the file exists.
pub fn available_whisper_models(app_data_dir: &Path) -> Vec<ModelInfo> {
    let models_raw = [
        (
            "base",
            "ggml-base.bin",
            147_951_465u64,
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            "Whisper Base (~148 MB) — fast, lower accuracy",
        ),
        (
            "base-q5_1",
            "ggml-base-q5_1.bin",
            59_707_625u64,
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base-q5_1.bin",
            "Whisper Base Q5_1 (~60 MB) — fastest, quantized",
        ),
        (
            "small",
            "ggml-small.bin",
            487_601_905u64,
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            "Whisper Small (~488 MB) — balanced speed and accuracy",
        ),
        (
            "small-q5_1",
            "ggml-small-q5_1.bin",
            190_085_487u64,
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin",
            "Whisper Small Q5_1 (~190 MB) — balanced, quantized",
        ),
        (
            "medium",
            "ggml-medium.bin",
            1_533_774_081u64,
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
            "Whisper Medium (~1.5 GB) — high accuracy",
        ),
        (
            "large-v3-turbo",
            "ggml-large-v3-turbo.bin",
            1_622_081_537u64,
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
            "Whisper Large-v3-Turbo (~1.6 GB) — best accuracy",
        ),
        (
            "large-v3-turbo-q5_0",
            "ggml-large-v3-turbo-q5_0.bin",
            574_041_195u64,
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
            "Whisper Large-v3-Turbo Q5_0 (~574 MB) — best accuracy, quantized",
        ),
    ];

    models_raw
        .iter()
        .map(|(id, filename, size_bytes, url, description)| {
            let path = whisper_model_path(app_data_dir, filename);
            ModelInfo {
                id: id.to_string(),
                filename: filename.to_string(),
                size_bytes: *size_bytes,
                download_url: url.to_string(),
                description: description.to_string(),
                downloaded: path.exists(),
            }
        })
        .collect()
}

/// List all available pyannote models (segmentation + embedding) with download status.
pub fn available_pyannote_models(app_data_dir: &Path) -> Vec<ModelInfo> {
    let models_raw = [
        (
            "pyannote-segmentation",
            "segmentation-3.0.onnx",
            5_983_836u64,
            "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/segmentation-3.0.onnx",
            "Pyannote segmentation 3.0 (~6 MB) — voice activity detection",
        ),
        (
            "pyannote-embedding",
            "wespeaker_en_voxceleb_CAM++.onnx",
            29_292_684u64,
            "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/wespeaker_en_voxceleb_CAM++.onnx",
            "WeSpeaker CAM++ (~28 MB) — speaker embedding extraction",
        ),
    ];

    models_raw
        .iter()
        .map(|(id, filename, size_bytes, url, description)| {
            let path = pyannote_model_path(app_data_dir, filename);
            ModelInfo {
                id: id.to_string(),
                filename: filename.to_string(),
                size_bytes: *size_bytes,
                download_url: url.to_string(),
                description: description.to_string(),
                downloaded: path.exists(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Check required models
// ---------------------------------------------------------------------------

/// Check which models are missing and return human-readable descriptions of each.
///
/// Checks the requested Whisper model plus both pyannote models (segmentation
/// and embedding). Used by the Settings UI to show a "missing models" warning.
pub fn check_required_models(app_data_dir: &Path, whisper_model_id: &str) -> Vec<String> {
    let mut missing = Vec::new();

    // Check requested whisper model
    if let Some(filename) = whisper_model_filename(whisper_model_id) {
        let path = whisper_model_path(app_data_dir, filename);
        if !path.exists() {
            missing.push(format!(
                "Whisper model '{}' ({})",
                whisper_model_id, filename
            ));
        }
    }

    // Pyannote stub models (diarization — currently not available but reserved)
    let pyannote_stubs = [
        ("segmentation-3.0.onnx", "Pyannote segmentation model"),
        (
            "wespeaker_en_voxceleb_CAM++.onnx",
            "Pyannote speaker embedding model",
        ),
    ];
    for (filename, description) in &pyannote_stubs {
        let path = pyannote_model_path(app_data_dir, filename);
        if !path.exists() {
            missing.push(description.to_string());
        }
    }

    missing
}

// ---------------------------------------------------------------------------
// Download / delete
// ---------------------------------------------------------------------------

/// Download a model file with progress reporting.
///
/// Downloads to a `.tmp` file first, then atomically renames to `dest_path`.
/// This prevents partial downloads from corrupting an existing model file.
///
/// The `on_progress` callback receives `(downloaded_bytes, total_bytes)` and
/// is called after each chunk is written to disk. Model URLs publish no
/// SHA-256 digest, so hash verification is not possible here — the shared
/// `medical_core::net` helper still provides the connect/total timeouts
/// (the default reqwest client has none, so a stalled CDN would otherwise
/// hang the download forever).
pub async fn download_model<F>(url: &str, dest_path: &Path, on_progress: F) -> Result<(), SttError>
where
    F: Fn(u64, u64) + Send + 'static,
{
    // Ensure parent directory exists
    if let Some(parent) = dest_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            SttError::ModelDownload(format!("Failed to create model directory: {e}"))
        })?;
    }

    let tmp_path = dest_path.with_extension("tmp");

    medical_core::net::download_file(url, &tmp_path, None, on_progress)
        .await
        .map_err(|e| SttError::ModelDownload(format!("Failed to download from {url}: {e}")))?;

    // Atomic rename
    tokio::fs::rename(&tmp_path, dest_path).await.map_err(|e| {
        SttError::ModelDownload(format!(
            "Failed to rename {} -> {}: {e}",
            tmp_path.display(),
            dest_path.display()
        ))
    })?;

    Ok(())
}

/// Delete a downloaded model file from disk.
///
/// Returns `SttError::ModelNotFound` if the file doesn't exist.
pub async fn delete_model(path: &Path) -> Result<(), SttError> {
    if !path.exists() {
        return Err(SttError::ModelNotFound(format!(
            "Model file not found: {}",
            path.display()
        )));
    }

    tokio::fs::remove_file(path).await.map_err(|e| {
        SttError::ModelDownload(format!("Failed to delete {}: {e}", path.display()))
    })?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn whisper_model_filenames() {
        assert_eq!(whisper_model_filename("base"), Some("ggml-base.bin"));
        assert_eq!(
            whisper_model_filename("base-q5_1"),
            Some("ggml-base-q5_1.bin")
        );
        assert_eq!(whisper_model_filename("small"), Some("ggml-small.bin"));
        assert_eq!(
            whisper_model_filename("small-q5_1"),
            Some("ggml-small-q5_1.bin")
        );
        assert_eq!(whisper_model_filename("medium"), Some("ggml-medium.bin"));
        assert_eq!(
            whisper_model_filename("large-v3-turbo"),
            Some("ggml-large-v3-turbo.bin")
        );
        assert_eq!(
            whisper_model_filename("large-v3-turbo-q5_0"),
            Some("ggml-large-v3-turbo-q5_0.bin")
        );
        assert_eq!(whisper_model_filename("unknown"), None);
    }

    #[test]
    fn path_resolution() {
        let base = Path::new("/tmp/app_data");
        assert_eq!(models_dir(base), Path::new("/tmp/app_data/models"));
        assert_eq!(whisper_dir(base), Path::new("/tmp/app_data/models/whisper"));
        assert_eq!(
            pyannote_dir(base),
            Path::new("/tmp/app_data/models/pyannote")
        );
        assert_eq!(
            whisper_model_path(base, "ggml-base.bin"),
            Path::new("/tmp/app_data/models/whisper/ggml-base.bin")
        );
        assert_eq!(
            pyannote_model_path(base, "segmentation.onnx"),
            Path::new("/tmp/app_data/models/pyannote/segmentation.onnx")
        );
    }

    #[test]
    fn available_models_list() {
        // Use a path that definitely does not exist so downloaded = false for all
        let base = Path::new("/tmp/__nonexistent_ferriscribe_test_dir__");
        let models = available_whisper_models(base);
        assert_eq!(models.len(), 7);
        assert!(models.iter().all(|m| !m.downloaded));
        // Every catalog id must resolve to a filename — a missing match arm
        // silently falls back to large-v3-turbo at provider build time.
        for m in &models {
            assert!(
                whisper_model_filename(&m.id).is_some(),
                "catalog id {} has no filename mapping",
                m.id
            );
        }
    }

    #[test]
    fn check_missing_models() {
        let base = Path::new("/tmp/__nonexistent_ferriscribe_test_dir__");
        let missing = check_required_models(base, "base");
        // whisper base + 2 pyannote stubs = 3 missing
        assert_eq!(missing.len(), 3);
    }

    #[test]
    fn whisper_model_id_roundtrip() {
        let ids = [
            WhisperModelId::Base,
            WhisperModelId::BaseQ5_1,
            WhisperModelId::Small,
            WhisperModelId::SmallQ5_1,
            WhisperModelId::Medium,
            WhisperModelId::LargeV3Turbo,
            WhisperModelId::LargeV3TurboQ5_0,
        ];
        for id in &ids {
            let s = id.as_str();
            let roundtripped = WhisperModelId::from_str(s).expect("from_str failed");
            assert_eq!(*id, roundtripped);
        }
        assert!(WhisperModelId::from_str("nonexistent").is_none());
    }
}
