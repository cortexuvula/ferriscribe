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
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

const MANIFEST: &str = include_str!("../whisper-manifest.json");

/// Upper bound on a decompressed whisper-server binary during extraction.
/// Guards against zip/gzip bombs in a corrupted or tampered archive; the
/// shipped whisper-server binaries are well under this size.
const MAX_EXTRACTED_BINARY_BYTES: u64 = 512 * 1024 * 1024;

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
        // Shared verified-download helper: explicit connect/total timeouts
        // (a stalled CDN must not hang start() forever — the default reqwest
        // client has none) + SHA-256 verification before extraction.
        let bytes = medical_core::net::download_bytes(url, Some(expected))
            .await
            .map_err(|e| match e {
                medical_core::net::DownloadError::HashMismatch { expected, got } => {
                    WhisperError::HashMismatch { expected, got }
                }
                other => WhisperError::Download(other.to_string()),
            })?;
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
        // Only kill processes that ARE whisper-server (name-filtered): an
        // unrelated local service on the port must never be force-killed —
        // and if one holds the port, the subsequent spawn will fail loudly
        // instead of silently murdering it.
        let pids = whisper_pids_on_port(self.port, self.binary_name_for_platform()).await;
        if pids.is_empty() {
            if !pids_on_port(self.port).await.is_empty() {
                warn!(
                    port = self.port,
                    "port is held by a non-whisper process; refusing to kill it — configure a different whisper port"
                );
            }
            return;
        }
        info!(
            port = self.port,
            count = pids.len(),
            "reclaiming port from stale whisper-server processes"
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
                    // Respawn with backoff. A failed respawn is logged and
                    // retried (backoff climbs to the 60 s cap) rather than
                    // silently ending supervision — a single transient spawn
                    // failure must not permanently kill local whisper with
                    // no trace in the log.
                    loop {
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
                        if let Err(e) = self.binary_dir.read_dir() {
                            warn!(
                                error = %e,
                                "whisper binary dir unreadable; cannot respawn yet"
                            );
                            continue;
                        }
                        let bin = self.binary_dir.join(self.binary_name_for_platform());
                        match self.spawn_once_at(&bin).await {
                            Ok(child) => {
                                *self.child.lock().await = Some(child);
                                break;
                            }
                            Err(e) => {
                                error!(
                                    error = %e,
                                    retry_in_secs = backoff.as_secs(),
                                    "whisper-server respawn failed; will retry"
                                );
                            }
                        }
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
        let file = zip
            .by_index(i)
            .map_err(|e| WhisperError::Manifest(e.to_string()))?;
        let name = file.name().to_string();
        if Path::new(&name).file_name().and_then(|s| s.to_str()) == Some(binary_name) {
            // The entry's declared size is untrusted zip metadata: refuse
            // oversized entries up front, then bound the actual decompressed
            // stream too, so a corrupted or tampered archive can't OOM the
            // app via a huge allocation. The archive's SHA-256 is verified
            // before extraction runs; this is belt-and-suspenders.
            if file.size() > MAX_EXTRACTED_BINARY_BYTES {
                return Err(WhisperError::Manifest(format!(
                    "zip entry '{name}' declares {} bytes; over the {}-byte extraction cap",
                    file.size(),
                    MAX_EXTRACTED_BINARY_BYTES
                )));
            }
            let mut buf = Vec::with_capacity(file.size() as usize);
            let mut limited = std::io::Read::take(file, MAX_EXTRACTED_BINARY_BYTES + 1);
            let n = std::io::Read::read_to_end(&mut limited, &mut buf)?;
            if n as u64 > MAX_EXTRACTED_BINARY_BYTES {
                return Err(WhisperError::Manifest(format!(
                    "zip entry '{name}' decompressed to {n} bytes; over the {}-byte extraction cap",
                    MAX_EXTRACTED_BINARY_BYTES
                )));
            }
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
        let e = entry?;
        let path = e.path()?.to_path_buf();
        if path.file_name().and_then(|s| s.to_str()) == Some(binary_name) {
            // Bound the decompressed size — same gzip-bomb defense as the
            // zip path.
            let mut buf = Vec::new();
            let mut limited = std::io::Read::take(e, MAX_EXTRACTED_BINARY_BYTES + 1);
            let n = std::io::Read::read_to_end(&mut limited, &mut buf)?;
            if n as u64 > MAX_EXTRACTED_BINARY_BYTES {
                return Err(WhisperError::Manifest(format!(
                    "tar entry '{binary_name}' decompressed to {n} bytes; over the {}-byte extraction cap",
                    MAX_EXTRACTED_BINARY_BYTES
                )));
            }
            std::fs::write(out_dir.join(binary_name), buf)?;
            return Ok(());
        }
    }
    Err(WhisperError::Manifest(format!(
        "binary {binary_name} not found in tar.gz"
    )))
}

/// Return PIDs of processes listening on `port` whose command line contains
/// `name_filter` (a binary-name fragment like "whisper-server"). Uses
/// `lsof` on Unix, `netstat`+`tasklist` on Windows. Best-effort — returns
/// empty on any error.
///
/// The filter is load-bearing: an innocent local service can legitimately
/// occupy the port (8080 is a common dev port), and `reclaim_port` KILLS
/// whatever it is handed — an unfiltered list would force-kill unrelated
/// processes.
async fn whisper_pids_on_port(port: u16, name_filter: &str) -> Vec<u32> {
    // lsof -t with the `-a` intersection: port+LISTEN+command-name match.
    // (`-c` matches against the whole command name.)
    let output = if cfg!(target_os = "windows") {
        // netstat gives no process names; filter via tasklist per PID.
        let all = pids_on_port(port).await;
        return filter_pids_by_name_windows(&all, name_filter).await;
    } else {
        let port_arg = format!(":{port}");
        tokio::process::Command::new("lsof")
            .args(["-ti", &port_arg, "-sTCP:LISTEN", "-a", "-c", name_filter])
            .output()
            .await
    };
    let out = match output {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pids: Vec<u32> = text
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// Windows: netstat knows no process names — ask `tasklist` per PID and
/// keep only the ones whose image name contains the filter. Best-effort:
/// PIDs that can't be queried are dropped (never killed blind).
async fn filter_pids_by_name_windows(pids: &[u32], name_filter: &str) -> Vec<u32> {
    let mut kept = Vec::new();
    for pid in pids {
        let Ok(out) = tokio::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .await
        else {
            continue;
        };
        let text = String::from_utf8_lossy(&out.stdout);
        if text.to_lowercase().contains(&name_filter.to_lowercase()) {
            kept.push(*pid);
        }
    }
    kept
}

/// Unfiltered port listeners (any process). Used only by the health
/// re-check logging path; the kill path goes through
/// [`whisper_pids_on_port`].
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
    let mut pids = if cfg!(target_os = "windows") {
        parse_netstat_listening_pids(&text, port)
    } else {
        text.lines()
            .filter_map(|l| l.trim().parse::<u32>().ok())
            .collect()
    };
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// Extract PIDs of processes LISTENING on exactly `port` from `netstat -ano`
/// output. Strictly column-based: only the LOCAL address column's port is
/// matched (exact string compare after the last `:`, so `:80` never matches
/// `:8080`), and only rows in the LISTENING state — a substring scan over
/// whole lines would otherwise match REMOTE endpoints of unrelated outbound
/// connections and force-kill innocent processes (`kill_pid` uses
/// `taskkill /F`). Pure so it is unit-testable off-Windows.
fn parse_netstat_listening_pids(text: &str, port: u16) -> Vec<u32> {
    let port_str = port.to_string();
    let mut pids = Vec::new();
    for line in text.lines() {
        // Rows: `TCP  <local>  <remote>  <state>  <pid>` (state omitted on
        // some rows — those are not listeners and are skipped by the column
        // count check).
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() != 5 || cols[0] != "TCP" {
            continue;
        }
        let (local, state, pid) = (cols[1], cols[3], cols[4]);
        if state != "LISTENING" {
            continue;
        }
        // Local may be `127.0.0.1:8080` or IPv6 `[::]:8080` — the port is
        // always after the last colon.
        if local.rsplit_once(':').map(|(_, p)| p) != Some(port_str.as_str()) {
            continue;
        }
        if let Ok(pid) = pid.parse::<u32>() {
            pids.push(pid);
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

    #[test]
    fn netstat_parser_matches_only_exact_listening_port() {
        let crlf = "\r\n";
        let text = concat!(
            "\r\n\r\n Active Connections\r\n\r\n",
            "  Proto  Local Address          Foreign Address        State           PID\r\n",
            // The listener we want.
            "  TCP    0.0.0.0:8080           0.0.0.0:0              LISTENING       4242\r\n",
            // Same port on IPv6 — same PID, must parse fine.
            "  TCP    [::]:8080              [::]:0                 LISTENING       4242\r\n",
            // Substring trap: port 80 must NOT match :8080, and vice versa.
            "  TCP    127.0.0.1:80            0.0.0.0:0              LISTENING       5150\r\n",
            // Outbound connection whose REMOTE endpoint is :8080 — must be
            // excluded (this is the force-kill-innocent-process bug).
            "  TCP    192.168.1.9:53214       93.184.216.34:8080     ESTABLISHED     7300\r\n",
            // Established with LOCAL :8080 — excluded by the state filter.
            "  TCP    127.0.0.1:8080          127.0.0.1:53215        ESTABLISHED     8100\r\n",
            // TIME_WAIT rows carry no PID column (4 cols) — skipped.
            "  TCP    127.0.0.1:8080          127.0.0.1:53216        TIME_WAIT\r\n",
        );
        assert_eq!(parse_netstat_listening_pids(text, 8080), vec![4242]);
        assert_eq!(parse_netstat_listening_pids(text, 80), vec![5150]);
        assert_eq!(parse_netstat_listening_pids(text, 8081), Vec::<u32>::new());
        let _ = crlf;
    }
}
