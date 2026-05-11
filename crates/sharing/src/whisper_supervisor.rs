//! whisper.cpp child process supervisor + on-demand binary download.
//!
//! # Platform support
//!
//! As of v1.7.6, whisper.cpp (now under the ggml-org GitHub organisation) ships
//! prebuilt server binaries **only for Windows x86_64** (inside `whisper-bin-x64.zip`).
//! macOS and Linux office-server admins must build `whisper-server` from source:
//!   <https://github.com/ggml-org/whisper.cpp#server>
//!
//! The supervisor returns [`WhisperError::UnsupportedPlatform`] when `url` is
//! `null` in the manifest (i.e. no prebuilt binary is available for this OS).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{info, warn};

const MANIFEST: &str = include_str!("../whisper-manifest.json");

#[derive(Debug, Deserialize)]
struct Manifest {
    /// Cache-busting key. Stored alongside the installed binary in
    /// `.whisper-manifest-version`; on startup the supervisor invalidates
    /// the cached binary whenever this string changes.
    version: String,
    binaries: std::collections::HashMap<String, BinaryEntry>,
}

/// A single platform entry in the manifest.
///
/// `url` and `archive` are `None` when whisper.cpp does not publish a prebuilt
/// server binary for that platform. The supervisor surfaces
/// [`WhisperError::UnsupportedPlatform`] in that case.
#[derive(Debug, Deserialize)]
struct BinaryEntry {
    url: Option<String>,
    sha256: Option<String>,
    archive: Option<String>,
    binary_name: String,
}

fn platform_key() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macos-aarch64",
        ("macos", "x86_64") => "macos-x86_64",
        ("linux", "x86_64") => "linux-x86_64",
        ("windows", "x86_64") => "windows-x86_64",
        _ => "unsupported",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WhisperError {
    #[error("platform unsupported")]
    UnsupportedPlatform,
    #[error("download: {0}")]
    Download(String),
    #[error("hash mismatch (expected {expected}, got {got})")]
    HashMismatch { expected: String, got: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest: {0}")]
    Manifest(String),
}

pub type Result<T> = std::result::Result<T, WhisperError>;

pub struct WhisperSupervisor {
    binary_dir: PathBuf,
    model_path: PathBuf,
    port: u16,
    child: Mutex<Option<Child>>,
    stop: Arc<tokio::sync::Notify>,
    /// Set to `true` before `stop.notify_waiters()` so the supervisor loop
    /// sees the intent even if the notification is delivered before the
    /// `select!` future has registered a waiter.
    stopped: AtomicBool,
    /// Handle to the supervisor task so `stop()` can abort it as a safety net.
    supervisor_handle: Mutex<Option<JoinHandle<()>>>,
}

impl WhisperSupervisor {
    pub fn new(binary_dir: PathBuf, model_path: PathBuf, port: u16) -> Self {
        Self {
            binary_dir,
            model_path,
            port,
            child: Mutex::new(None),
            stop: Arc::new(tokio::sync::Notify::new()),
            stopped: AtomicBool::new(false),
            supervisor_handle: Mutex::new(None),
        }
    }

    pub async fn ensure_binary(&self) -> Result<PathBuf> {
        let manifest: Manifest =
            serde_json::from_str(MANIFEST).map_err(|e| WhisperError::Manifest(e.to_string()))?;
        let key = platform_key();
        let entry = manifest
            .binaries
            .get(key)
            .ok_or(WhisperError::UnsupportedPlatform)?;

        // A null `url` means whisper.cpp does not publish a prebuilt server binary
        // for this platform. Office-server admins must build from source:
        // https://github.com/ggml-org/whisper.cpp#server
        let url = entry.url.as_deref().ok_or(WhisperError::UnsupportedPlatform)?;
        let archive = entry.archive.as_deref().ok_or(WhisperError::UnsupportedPlatform)?;

        let bin_path = self.binary_dir.join(&entry.binary_name);
        let lock_path = self.binary_dir.join(".whisper-manifest-version");

        if bin_path.exists() {
            let cached = tokio::fs::read_to_string(&lock_path)
                .await
                .ok()
                .map(|s| s.trim().to_string());
            if cached.as_deref() == Some(manifest.version.trim()) {
                return Ok(bin_path);
            }
            warn!(
                "cached whisper-server was installed from manifest version {:?}; current is {:?}; replacing",
                cached.as_deref().unwrap_or("(none)"),
                manifest.version
            );
            let _ = tokio::fs::remove_file(&bin_path).await;
            let _ = tokio::fs::remove_file(&lock_path).await;
        }
        tokio::fs::create_dir_all(&self.binary_dir).await?;
        let bytes = reqwest::get(url)
            .await
            .map_err(|e| WhisperError::Download(e.to_string()))?
            .bytes()
            .await
            .map_err(|e| WhisperError::Download(e.to_string()))?;
        if let Some(expected) = &entry.sha256 {
            let got = hex::encode(Sha256::digest(&bytes));
            if &got != expected {
                return Err(WhisperError::HashMismatch {
                    expected: expected.clone(),
                    got,
                });
            }
        } else {
            warn!("sha256 not set for platform {}; skipping verification", key);
        }
        Self::extract_archive(&bytes, archive, &self.binary_dir, &entry.binary_name)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&bin_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&bin_path, perms)?;
        }
        let _ = tokio::fs::write(&lock_path, manifest.version.trim()).await;
        Ok(bin_path)
    }

    fn extract_archive(
        bytes: &[u8],
        archive_kind: &str,
        out_dir: &Path,
        binary_name: &str,
    ) -> Result<()> {
        match archive_kind {
            "zip" => extract_zip(bytes, out_dir, binary_name),
            "tar.gz" => extract_tar_gz(bytes, out_dir, binary_name),
            other => Err(WhisperError::Manifest(format!(
                "unsupported archive: {other}"
            ))),
        }
    }

    pub async fn start(self: &Arc<Self>) -> Result<()> {
        let bin = self.ensure_binary().await?;
        let child = self.spawn_once_at(&bin).await?;
        *self.child.lock().await = Some(child);
        let me = Arc::clone(self);
        let handle = tokio::spawn(async move {
            me.supervise().await;
        });
        *self.supervisor_handle.lock().await = Some(handle);
        Ok(())
    }

    async fn supervise(self: Arc<Self>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            // Belt-and-suspenders: if stop() was called before we even loop,
            // exit immediately rather than attempting another spawn.
            if self.stopped.load(Ordering::Relaxed) {
                return;
            }
            let mut guard = self.child.lock().await;
            let Some(mut c) = guard.take() else { return; };
            drop(guard);
            tokio::select! {
                _ = c.wait() => {
                    info!("whisper-server exited; restarting in {:?}", backoff);
                    // Wait for the backoff period, but bail immediately if stop fires.
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = self.stop.notified() => { return; }
                    }
                    // Re-check `stopped` after the backoff sleep — stop() may have
                    // been called while we were sleeping (notify_waiters was used
                    // as a best-effort signal only).
                    if self.stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                    let bin = match self.binary_dir.read_dir() {
                        Ok(_) => self.binary_dir.join(self.binary_name_for_platform()),
                        Err(_) => return,
                    };
                    if let Ok(child) = self.spawn_once_at(&bin).await {
                        *self.child.lock().await = Some(child);
                    } else {
                        return;
                    }
                }
                _ = self.stop.notified() => {
                    let _ = c.kill().await;
                    return;
                }
            }
        }
    }

    fn binary_name_for_platform(&self) -> &'static str {
        // Defaults that match the manifest.
        if cfg!(target_os = "windows") { "whisper-server.exe" } else { "whisper-server" }
    }

    async fn spawn_once_at(&self, bin: &Path) -> Result<Child> {
        // whisper.cpp's whisper-server has no `--api-key` flag and no built-in
        // auth. Auth is enforced one layer up by auth_proxy on
        // `whisper_proxy_port`; this child only listens on 127.0.0.1 so it is
        // unreachable from the LAN.
        let mut cmd = Command::new(bin);
        cmd.arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(self.port.to_string())
            .arg("-m")
            .arg(&self.model_path)
            // whisper.cpp's whisper-server defaults to `/inference` for its
            // POST endpoint; our RemoteSttProvider sends to the OpenAI-
            // compatible `/v1/audio/transcriptions` path. Pin the server to
            // that path so the proxy → whisper-server → response chain
            // works without per-deployment config. The request and response
            // shapes (multipart file upload, verbose_json) are already
            // compatible.
            .arg("--inference-path")
            .arg("/v1/audio/transcriptions")
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        // Forward stderr to tracing so dyld errors, port conflicts, and
        // model-load failures surface in the app log instead of vanishing
        // into a silent crashloop. whisper-server's stderr carries init
        // and request-routing diagnostics — not transcript content.
        if let Some(stderr) = child.stderr.take() {
            let stderr_task = tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    info!("whisper-server: {line}");
                }
            });
            tokio::spawn(async move {
                match stderr_task.await {
                    Ok(()) => tracing::debug!("whisper stderr-forwarding task exited normally"),
                    Err(e) if e.is_cancelled() => tracing::debug!("whisper stderr task cancelled"),
                    Err(e) if e.is_panic() => tracing::error!(error = %e, "whisper stderr task panicked; stderr output lost"),
                    Err(e) => tracing::error!(error = %e, "whisper stderr task failed"),
                }
            });
        }
        Ok(child)
    }

    pub async fn stop(&self) {
        // Set the flag BEFORE notifying so the supervise() loop sees it even
        // if it polls stop.notified() after the waiters snapshot is taken.
        self.stopped.store(true, Ordering::Relaxed);
        self.stop.notify_waiters();
        if let Some(mut c) = self.child.lock().await.take() {
            let _ = c.kill().await;
        }
        // Belt-and-suspenders: abort the supervisor task so pathological cases
        // (e.g. stuck in spawn_once_at) can't leave it running.
        if let Some(handle) = self.supervisor_handle.lock().await.take() {
            handle.abort();
        }
    }
}

fn extract_zip(bytes: &[u8], out_dir: &Path, binary_name: &str) -> Result<()> {
    let cursor = std::io::Cursor::new(bytes);
    let mut zip =
        zip::ZipArchive::new(cursor).map_err(|e| WhisperError::Manifest(e.to_string()))?;
    for i in 0..zip.len() {
        let mut file = zip
            .by_index(i)
            .map_err(|e| WhisperError::Manifest(e.to_string()))?;
        let name = file.name().to_string();
        if Path::new(&name).file_name().and_then(|s| s.to_str()) == Some(binary_name) {
            let mut buf = Vec::with_capacity(file.size() as usize);
            std::io::Read::read_to_end(&mut file, &mut buf)?;
            std::fs::write(out_dir.join(binary_name), buf)?;
            return Ok(());
        }
    }
    Err(WhisperError::Manifest(format!(
        "binary {binary_name} not found in zip"
    )))
}

fn extract_tar_gz(bytes: &[u8], out_dir: &Path, binary_name: &str) -> Result<()> {
    let cursor = std::io::Cursor::new(bytes);
    let gz = flate2::read::GzDecoder::new(cursor);
    let mut ar = tar::Archive::new(gz);
    for entry in ar.entries()? {
        let mut e = entry?;
        let path = e.path()?.to_path_buf();
        if path.file_name().and_then(|s| s.to_str()) == Some(binary_name) {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut e, &mut buf)?;
            std::fs::write(out_dir.join(binary_name), buf)?;
            return Ok(());
        }
    }
    Err(WhisperError::Manifest(format!(
        "binary {binary_name} not found in tar.gz"
    )))
}
