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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Errors from the whisper-server supervisor.
#[derive(Debug, thiserror::Error)]
pub enum WhisperError {
    /// No prebuilt binary is available for this OS/arch. The admin must build
    /// whisper-server from source.
    #[error("platform unsupported")]
    UnsupportedPlatform,
    /// HTTP download failed.
    #[error("download: {0}")]
    Download(String),
    /// SHA-256 of the downloaded archive didn't match the manifest.
    #[error("hash mismatch (expected {expected}, got {got})")]
    HashMismatch { expected: String, got: String },
    /// Filesystem I/O error during extraction or chmod.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Manifest parsing or archive extraction error.
    #[error("manifest: {0}")]
    Manifest(String),
}

/// Convenience alias for `Result<T, WhisperError>`.
pub type Result<T> = std::result::Result<T, WhisperError>;

/// Child process supervisor for whisper-server.
///
/// Manages the full lifecycle: binary download (with SHA-256 verification),
/// process spawning, stderr forwarding, and crash recovery with exponential
/// backoff (1s -> 60s cap).
///
/// The child always binds to `127.0.0.1` -- auth is enforced one layer up
/// by the auth proxy.
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
    /// Create a new supervisor.
    ///
    /// Does not download or spawn anything -- call [`start`](Self::start) for that.
    ///
    /// - `binary_dir`: where to cache the downloaded whisper-server binary.
    /// - `model_path`: path to the `.bin` model file passed via `-m`.
    /// - `port`: loopback port for whisper-server (`--port`).
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

    /// Download (or reuse cached) whisper-server binary for the current platform.
    ///
    /// Checks the manifest version against a lock file to invalidate stale
    /// caches. Returns [`WhisperError::UnsupportedPlatform`] when no prebuilt
    /// binary exists for this OS/arch.
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
        let url = entry
            .url
            .as_deref()
            .ok_or(WhisperError::UnsupportedPlatform)?;
        let archive = entry
            .archive
            .as_deref()
            .ok_or(WhisperError::UnsupportedPlatform)?;

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

        let bin_path = self
            .download_and_verify(url, archive, entry.sha256.as_deref(), &entry.binary_name)
            .await?;

        let _ = tokio::fs::write(&lock_path, manifest.version.trim()).await;
        Ok(bin_path)
    }

    /// Download an archive from `url`, optionally verify its SHA-256 against
    /// `expected_sha256`, extract `binary_name` into `self.binary_dir`, and
    /// (on Unix) chmod 0755. Returns the path to the extracted binary.
    ///
    /// Extracted into a `pub(crate)` helper so unit tests can supply a
    /// wiremock URL + a controlled archive body. The lock-file write that
    /// records the manifest version stays in `ensure_binary` — this helper
    /// is unaware of the manifest.
    pub(crate) async fn download_and_verify(
        &self,
        url: &str,
        archive: &str,
        expected_sha256: Option<&str>,
        binary_name: &str,
    ) -> Result<PathBuf> {
        tokio::fs::create_dir_all(&self.binary_dir).await?;
        // Refuse to download and execute a binary without a SHA-256 to verify
        // against. A compromised manifest or MITM on the download path would
        // otherwise execute arbitrary code on the office server.
        let expected = expected_sha256.ok_or_else(|| {
            WhisperError::Download(format!(
                "sha256 hash missing for binary {binary_name}; refusing to download without verification"
            ))
        })?;
        let bytes = reqwest::get(url)
            .await
            .map_err(|e| WhisperError::Download(e.to_string()))?
            .bytes()
            .await
            .map_err(|e| WhisperError::Download(e.to_string()))?;
        let got = hex::encode(Sha256::digest(&bytes));
        if got != expected {
            return Err(WhisperError::HashMismatch {
                expected: expected.to_string(),
                got,
            });
        }
        // The extract + chmod are synchronous CPU/disk work (full archive
        // decompression, metadata read, permission set). Running them inline
        // stalls the tokio worker thread for the whole duration — noticeable
        // on a large whisper-server binary. Offload to the blocking pool,
        // matching how the STT crate handles CPU-heavy inference
        // (stt-providers/src/local_provider.rs:113).
        let out_dir = self.binary_dir.clone();
        let archive = archive.to_string();
        let binary_name = binary_name.to_string();
        let bin_path = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
            Self::extract_archive(&bytes, &archive, &out_dir, &binary_name)?;
            let bin_path = out_dir.join(&binary_name);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&bin_path)?.permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&bin_path, perms)?;
            }
            Ok(bin_path)
        })
        .await
        .map_err(|e| WhisperError::Download(format!("extract task failed: {e}")))??;
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

    /// Ensure the binary exists, spawn the child process, and start the
    /// supervisor loop that restarts on crash.
    ///
    /// Before spawning, probes the configured port: if a healthy
    /// whisper-server is already listening (e.g. a leftover from a previous
    /// app session), it is **reused** and no new process is spawned — this
    /// prevents the zombie accumulation where each app launch stacked another
    /// 2 GB whisper-server. If something unhealthy holds the port, it is
    /// killed first.
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        let bin = self.ensure_binary().await?;

        // Probe the port before spawning. Three outcomes:
        //  1. Healthy whisper-server already up → reuse it, skip spawn.
        //  2. Something unhealthy on the port → kill it, then spawn.
        //  3. Nothing on the port → spawn as usual.
        if self.is_port_healthy().await {
            info!(
                port = self.port,
                "whisper-server already healthy on port; reusing, no spawn"
            );
            // We have no Child handle for a reused process, so we can't
            // supervise/kill it later. That's acceptable: a reused instance
            // is unmanaged and will be killed on next start's reclaim path
            // if it becomes unhealthy. The supervisor loop is skipped.
            //
            // Mark stopped=true so a stale supervise() handle (if any)
            // doesn't try to restart into a duplicate.
            self.stopped.store(true, Ordering::Relaxed);
            return Ok(());
        }
        // Something may be holding the port but not healthy. Kill any
        // process bound to it so our spawn succeeds and we don't create
        // yet another zombie.
        self.reclaim_port().await;

        let child = self.spawn_once_at(&bin).await?;
        *self.child.lock().await = Some(child);
        // Clear the stopped flag (it may have been set by a previous start()
        // that reused an existing instance).
        self.stopped.store(false, Ordering::Relaxed);
        let me = Arc::clone(self);
        let handle = tokio::spawn(async move {
            me.supervise().await;
        });
        *self.supervisor_handle.lock().await = Some(handle);
        Ok(())
    }

    /// True iff a healthy whisper-server answers GET /health on our port.
    async fn is_port_healthy(&self) -> bool {
        let url = format!("http://127.0.0.1:{}/health", self.port);
        match reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(2))
            .build()
        {
            Ok(client) => client
                .get(&url)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Kill any process bound to our port. Best-effort; uses `lsof` on Unix
    /// and `netstat`+`taskkill` on Windows. Logs the count of killed PIDs
    /// (PHI-safe — no process details).
    async fn reclaim_port(&self) {
        let pids = pids_on_port(self.port).await;
        if pids.is_empty() {
            return;
        }
        info!(
            port = self.port,
            count = pids.len(),
            "reclaiming port from stale processes"
        );
        for pid in &pids {
            let _ = kill_pid(*pid).await;
        }
        // Give the OS a moment to release the port after the kills.
        tokio::time::sleep(Duration::from_millis(500)).await;
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
            let Some(mut c) = guard.take() else {
                return;
            };
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
        if cfg!(target_os = "windows") {
            "whisper-server.exe"
        } else {
            "whisper-server"
        }
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
        // Forward stderr to tracing so dynd errors, port conflicts, and
        // model-load failures surface in the app log instead of vanishing
        // into a silent crashloop. PHI guard: only allowlist known-safe
        // diagnostic prefixes are logged; whisper-server processes PHI audio
        // and depending on build/version could emit recognized text to stderr,
        // so we must not forward arbitrary lines verbatim (AGENTS.md line 6).
        if let Some(stderr) = child.stderr.take() {
            let stderr_task = tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let lower = line.to_ascii_lowercase();
                    // Safe diagnostic prefixes: model loading, system info,
                    // network/listen state. Anything else (which could include
                    // transcribed segments) is dropped silently.
                    let safe = lower.starts_with("ggml")
                        || lower.starts_with("whisper")
                        || lower.starts_with("load")
                        || lower.starts_with("system_info")
                        || lower.starts_with("server")
                        || lower.starts_with("listening")
                        || lower.starts_with("port")
                        || lower.contains("model loaded")
                        || lower.contains("error")
                        || lower.contains("warning")
                        || lower.contains("init");
                    if safe {
                        info!("whisper-server: {line}");
                    } else {
                        // Non-diagnostic line; log only its length, never content.
                        tracing::debug!(
                            len = line.len(),
                            "whisper-server stderr line (not logged)"
                        );
                    }
                }
            });
            tokio::spawn(async move {
                match stderr_task.await {
                    Ok(()) => tracing::debug!("whisper stderr-forwarding task exited normally"),
                    Err(e) if e.is_cancelled() => tracing::debug!("whisper stderr task cancelled"),
                    Err(e) if e.is_panic() => {
                        tracing::error!(error = %e, "whisper stderr task panicked; stderr output lost")
                    }
                    Err(e) => tracing::error!(error = %e, "whisper stderr task failed"),
                }
            });
        }
        Ok(child)
    }

    /// Kill the child process and stop the supervisor loop.
    ///
    /// Sets a stopped flag before notifying waiters, so the supervise loop
    /// exits cleanly even if it's mid-backoff. As a safety net, also aborts
    /// the supervisor task handle.
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

/// Return PIDs of processes listening on `port`. Uses `lsof` on Unix,
/// `netstat` on Windows. Best-effort — returns empty on any error.
async fn pids_on_port(port: u16) -> Vec<u32> {
    let output = if cfg!(target_os = "windows") {
        tokio::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .output()
            .await
    } else {
        tokio::process::Command::new("lsof")
            .args(["-ti", &format!(":{port}"), "-sTCP:LISTEN"])
            .output()
            .await
    };
    let out = match output {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pids = Vec::new();
    if cfg!(target_os = "windows") {
        // netstat -ano output lines look like:
        //   TCP    127.0.0.1:8080     0.0.0.0:0     LISTENING    1234
        for line in text.lines() {
            if !line.contains(&format!(":{port}")) {
                continue;
            }
            if let Some(pid) = line
                .split_whitespace()
                .last()
                .and_then(|s| s.parse::<u32>().ok())
            {
                pids.push(pid);
            }
        }
    } else {
        for line in text.lines() {
            if let Ok(pid) = line.trim().parse::<u32>() {
                pids.push(pid);
            }
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// Kill a process by PID. Uses `kill` on Unix, `taskkill` on Windows.
async fn kill_pid(pid: u32) -> std::io::Result<()> {
    if cfg!(target_os = "windows") {
        tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
            .await?;
    } else {
        tokio::process::Command::new("kill")
            .arg(pid.to_string())
            .output()
            .await?;
    }
    Ok(())
}

impl Drop for WhisperSupervisor {
    fn drop(&mut self) {
        // Best-effort kill of the tracked child if stop() was never called
        // (e.g. app crash, force-quit). We can't await in Drop, so use the
        // synchronous start_kill (SIGKILL on Unix, TerminateProcess on
        // Windows). This prevents orphaned whisper-server zombies.
        // NOTE: this only kills the process WE spawned. A reused instance
        // (from start()'s probe path) has no Child handle and is intentionally
        // left running.
        if let Ok(mut guard) = self.child.try_lock()
            && let Some(child) = guard.as_mut()
        {
            let _ = child.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use wiremock::matchers::{method as http_method, path as http_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use zip::write::SimpleFileOptions;

    /// Build an in-memory zip archive containing exactly one file named
    /// `binary_name` with the given body.
    fn build_zip_with(binary_name: &str, body: &[u8]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file(binary_name, SimpleFileOptions::default())
                .expect("start_file");
            std::io::Write::write_all(&mut w, body).expect("write");
            w.finish().expect("finish");
        }
        buf.into_inner()
    }

    /// Same but with only `other.txt` — used for the "binary missing" tests.
    fn build_zip_without_target() -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file("other.txt", SimpleFileOptions::default())
                .expect("start_file");
            std::io::Write::write_all(&mut w, b"decoy").expect("write");
            w.finish().expect("finish");
        }
        buf.into_inner()
    }

    fn build_tar_gz_with(binary_name: &str, body: &[u8]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        let gz = GzEncoder::new(Vec::new(), Compression::default());
        let mut tar = tar::Builder::new(gz);
        let mut header = tar::Header::new_gnu();
        header.set_path(binary_name).expect("set_path");
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, body).expect("append");
        let gz = tar.into_inner().expect("into_inner");
        gz.finish().expect("finish")
    }

    fn build_tar_gz_without_target() -> Vec<u8> {
        build_tar_gz_with("other.txt", b"decoy")
    }

    fn fresh_supervisor() -> (Arc<WhisperSupervisor>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let supervisor = Arc::new(WhisperSupervisor::new(
            dir.path().to_path_buf(),
            dir.path().join("model.bin"),
            0,
        ));
        (supervisor, dir)
    }

    #[test]
    fn extract_zip_extracts_named_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = build_zip_with("whisper-server", b"fake-binary-content");
        extract_zip(&bytes, dir.path(), "whisper-server").expect("extract_zip");
        let out = std::fs::read(dir.path().join("whisper-server")).expect("read");
        assert_eq!(out, b"fake-binary-content");
    }

    #[test]
    fn extract_zip_errors_when_binary_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = build_zip_without_target();
        let r = extract_zip(&bytes, dir.path(), "whisper-server");
        assert!(matches!(r, Err(WhisperError::Manifest(_))));
    }

    #[test]
    fn extract_tar_gz_extracts_named_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = build_tar_gz_with("whisper-server", b"tar-content");
        extract_tar_gz(&bytes, dir.path(), "whisper-server").expect("extract_tar_gz");
        let out = std::fs::read(dir.path().join("whisper-server")).expect("read");
        assert_eq!(out, b"tar-content");
    }

    #[test]
    fn extract_tar_gz_errors_when_binary_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = build_tar_gz_without_target();
        let r = extract_tar_gz(&bytes, dir.path(), "whisper-server");
        assert!(matches!(r, Err(WhisperError::Manifest(_))));
    }

    #[test]
    fn extract_archive_unknown_kind_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let r = WhisperSupervisor::extract_archive(b"", "rar", dir.path(), "whisper-server");
        match r {
            Err(WhisperError::Manifest(msg)) => {
                assert!(
                    msg.contains("unsupported archive"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Err(Manifest); got {other:?}"),
        }
    }

    #[tokio::test]
    async fn download_and_verify_succeeds_with_correct_sha256() {
        let (supervisor, _dir) = fresh_supervisor();
        let zip_bytes = build_zip_with("whisper-server", b"hello-binary");
        let expected = hex::encode(Sha256::digest(&zip_bytes));

        let server = MockServer::start().await;
        Mock::given(http_method("GET"))
            .and(http_path("/binary.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_bytes.clone()))
            .mount(&server)
            .await;

        let url = format!("{}/binary.zip", server.uri());
        let bin = supervisor
            .download_and_verify(&url, "zip", Some(&expected), "whisper-server")
            .await
            .expect("download_and_verify");
        let out = std::fs::read(&bin).expect("read");
        assert_eq!(out, b"hello-binary");
    }

    #[tokio::test]
    async fn download_and_verify_rejects_hash_mismatch() {
        let (supervisor, _dir) = fresh_supervisor();
        let zip_bytes = build_zip_with("whisper-server", b"actual-content");
        let wrong_sha = "0".repeat(64); // 64-char zero hash — guaranteed mismatch

        let server = MockServer::start().await;
        Mock::given(http_method("GET"))
            .and(http_path("/binary.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_bytes.clone()))
            .mount(&server)
            .await;

        let url = format!("{}/binary.zip", server.uri());
        let r = supervisor
            .download_and_verify(&url, "zip", Some(&wrong_sha), "whisper-server")
            .await;
        match r {
            Err(WhisperError::HashMismatch { expected, got }) => {
                assert_eq!(expected, wrong_sha);
                assert_eq!(got, hex::encode(Sha256::digest(&zip_bytes)));
            }
            other => panic!("expected Err(HashMismatch); got {other:?}"),
        }
    }

    #[tokio::test]
    async fn download_and_verify_rejects_missing_sha256() {
        // A manifest entry must not be downloaded without a SHA-256 to verify
        // against — executing an unverified binary is a code-execution risk.
        let (supervisor, _dir) = fresh_supervisor();
        let zip_bytes = build_zip_with("whisper-server", b"untrusted");

        let server = MockServer::start().await;
        Mock::given(http_method("GET"))
            .and(http_path("/binary.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_bytes.clone()))
            .mount(&server)
            .await;

        let url = format!("{}/binary.zip", server.uri());
        let r = supervisor
            .download_and_verify(&url, "zip", None, "whisper-server")
            .await;
        match r {
            Err(WhisperError::Download(msg)) => {
                assert!(
                    msg.contains("sha256"),
                    "expected sha256-related message, got: {msg}"
                );
            }
            other => panic!("expected Err(Download) for missing sha256; got {other:?}"),
        }
    }
}
