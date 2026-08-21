//! Append-only target agent (R3): runs on the backup TARGET machine
//! (e.g. the Tailscale peer), accepts snapshot pushes, and never lets the
//! source-side credential delete or overwrite anything.
//!
//! # Credential model — the actual enforcement
//!
//! Two bearer tokens:
//! - **append token** (held by machine #1): may upload files into a NEW
//!   snapshot, commit it, and read snapshots back. That is the entire
//!   scope — there is no route this token can reach that deletes or
//!   overwrites an existing snapshot. A fully compromised source (even
//!   root on its own box) physically cannot erase history, because the
//!   capability does not exist on the wire (R3 / acceptance criterion 2).
//! - **admin token** (lives ONLY on the target): additionally may prune
//!   old snapshots (`POST /v1/admin/prune`).
//!
//! Committed snapshots are additionally chmod'd read-only (0444 files,
//! 0555 dirs) as defense-in-depth against local misuse on the target.
//!
//! # PHI
//!
//! The agent stores and serves opaque snapshot ids and payload names;
//! receipts carry only counts/sizes. Logs record ids and statuses only.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tracing::info;

use crate::snapshot::{PAYLOAD_DIR, RECEIPT_FILE, SnapshotReceipt};

/// Default total-bytes cap (1 TiB) — bounded growth for the append-only
/// store. Override via `FERRISCRIBE_BACKUP_MAX_BYTES`.
pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
/// Default committed-snapshot cap. Override via
/// `FERRISCRIBE_BACKUP_MAX_SNAPSHOTS`.
pub const DEFAULT_MAX_SNAPSHOTS: usize = 1000;
/// Uncommitted (in-flight) snapshot dirs older than this are swept at
/// agent startup — a crashed push must not leak disk forever.
pub const STALE_INCOMING_AFTER: std::time::Duration =
    std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// Agent configuration: storage root + the two credentials + growth caps.
#[derive(Clone)]
pub struct AgentConfig {
    pub root: PathBuf,
    pub append_token: String,
    pub admin_token: String,
    /// Cap on total on-disk snapshot bytes (committed + incoming). Uploads
    /// and commits that would exceed it are rejected with 507.
    pub max_bytes: u64,
    /// Cap on the number of COMMITTED snapshots; the (N+1)-th commit is
    /// rejected with 507.
    pub max_snapshots: usize,
}

impl AgentConfig {
    /// Build from environment: tokens are required; caps fall back to
    /// [`DEFAULT_MAX_BYTES`] / [`DEFAULT_MAX_SNAPSHOTS`] unless
    /// `FERRISCRIBE_BACKUP_MAX_BYTES` / `FERRISCRIBE_BACKUP_MAX_SNAPSHOTS`
    /// are set.
    pub fn from_env(root: PathBuf) -> Result<Self, String> {
        let append_token = std::env::var("FERRISCRIBE_BACKUP_APPEND_TOKEN")
            .map_err(|_| "FERRISCRIBE_BACKUP_APPEND_TOKEN is required".to_string())?;
        let admin_token = std::env::var("FERRISCRIBE_BACKUP_ADMIN_TOKEN")
            .map_err(|_| "FERRISCRIBE_BACKUP_ADMIN_TOKEN is required".to_string())?;
        let max_bytes = match std::env::var("FERRISCRIBE_BACKUP_MAX_BYTES") {
            Ok(v) => v
                .parse()
                .map_err(|_| "FERRISCRIBE_BACKUP_MAX_BYTES must be a u64".to_string())?,
            Err(_) => DEFAULT_MAX_BYTES,
        };
        let max_snapshots = match std::env::var("FERRISCRIBE_BACKUP_MAX_SNAPSHOTS") {
            Ok(v) => v
                .parse()
                .map_err(|_| "FERRISCRIBE_BACKUP_MAX_SNAPSHOTS must be a usize".to_string())?,
            Err(_) => DEFAULT_MAX_SNAPSHOTS,
        };
        Ok(Self {
            root,
            append_token,
            admin_token,
            max_bytes,
            max_snapshots,
        })
    }
}

/// Actual on-disk usage: total bytes across ALL snapshot dirs (committed
/// AND incoming — an attacker who never commits must not be able to fill
/// the disk either) plus the count of committed snapshots. Measured from
/// the filesystem, never from client-declared receipt numbers, so lying
/// receipts cannot bypass the caps.
fn disk_usage(root: &Path) -> (u64, usize) {
    fn dir_bytes(dir: &Path) -> u64 {
        let mut total = 0u64;
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    total += dir_bytes(&path);
                } else if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            }
        }
        total
    }
    let mut bytes = 0u64;
    let mut committed = 0usize;
    if let Ok(rd) = std::fs::read_dir(root) {
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            bytes += dir_bytes(&path);
            if is_committed(&path) {
                committed += 1;
            }
        }
    }
    (bytes, committed)
}

/// Remove uncommitted snapshot dirs whose newest mtime is older than
/// `cutoff`. Agent-side housekeeping (the append credential has no say);
/// called at startup BEFORE the listener binds, so no push is in flight.
/// Returns the swept ids (for logging — counts only).
pub fn sweep_stale_incoming(root: &Path, cutoff: std::time::SystemTime) -> Vec<String> {
    let mut swept = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        return swept;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() || is_committed(&path) {
            continue;
        }
        // Newest mtime inside the dir (or the dir itself when empty).
        let newest = newest_mtime(&path).unwrap_or_else(std::time::SystemTime::now);
        if newest < cutoff && unfreeze_and_remove(&path).is_ok() {
            swept.push(
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
    }
    swept
}

fn newest_mtime(dir: &Path) -> Option<std::time::SystemTime> {
    let mut newest = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            if let Ok(t) = entry.metadata().and_then(|m| m.modified())
                && newest.is_none_or(|n| t > n)
            {
                newest = Some(t);
            }
        }
    }
    newest
}

/// What a request's bearer token is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Append,
    Admin,
}

fn auth_scope(headers: &HeaderMap, cfg: &AgentConfig, route: &'static str) -> Option<Scope> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?;
    let raw = value.to_str().ok()?;
    let token = raw.strip_prefix("Bearer ")?;
    if constant_time_eq(token, &cfg.admin_token) {
        Some(Scope::Admin)
    } else if constant_time_eq(token, &cfg.append_token) {
        Some(Scope::Append)
    } else {
        // Counts/routes only — never the token, never PHI (finding 8).
        tracing::warn!(route, "authentication failed");
        None
    }
}

/// Bearer-token comparison over SHA-256 digests: both sides hash to a
/// fixed 32 bytes first, so there is no length leak and no early exit —
/// the fold runs over equal-length digests regardless of input (finding 9).
fn constant_time_eq(a: &str, b: &str) -> bool {
    use sha2::{Digest, Sha256};
    let da: [u8; 32] = Sha256::digest(a.as_bytes()).into();
    let db: [u8; 32] = Sha256::digest(b.as_bytes()).into();
    da.iter()
        .zip(db.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Router state: config plus per-snapshot in-process locks. The lock
/// serializes PUTs against the commit's validate→freeze window — without
/// it, a racing upload could slip unlisted bytes past the envelope check
/// into a just-committed snapshot (verify still fails closed on restore,
/// but a compromised source could weaponize the race to make snapshots
/// unverifiable). One agent process ⇒ an in-process mutex suffices.
#[derive(Clone)]
pub struct AgentState {
    pub cfg: AgentConfig,
    locks: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    >,
}

impl AgentState {
    fn new(cfg: AgentConfig) -> Self {
        Self {
            cfg,
            locks: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Acquire this snapshot's lock. Blocks until any concurrent PUT or
    /// commit for the SAME id finishes; never touches other snapshots.
    /// tokio::sync::Mutex so the guard may be held across the handler's
    /// filesystem awaits (a std guard would make the handler !Send).
    fn lock_snapshot(&self, id: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        let lock = {
            let mut map = self.locks.lock().expect("lock map not poisoned");
            map.entry(id.to_string()).or_default().clone()
        };
        // Guard against unbounded growth: prune lock entries when the map
        // grows past a generous bound (entries are only needed during
        // active uploads to one id).
        {
            let mut map = self.locks.lock().expect("lock map not poisoned");
            if map.len() > 10_000 {
                map.clear();
            }
        }
        lock
    }
}

/// Build the agent router (public so tests can drive it in-process).
pub fn router(cfg: AgentConfig) -> Router {
    Router::new()
        .route("/v1/snapshots", get(list_snapshots))
        .route("/v1/snapshots/{id}/file", get(get_file).put(put_file))
        .route("/v1/snapshots/{id}/commit", post(commit_snapshot))
        .route("/v1/admin/prune", post(prune))
        .with_state(AgentState::new(cfg))
}

/// Serve until the process is stopped. Binds `addr` (typically a
/// Tailscale IP — see the README deployment notes).
pub async fn serve(cfg: AgentConfig, addr: SocketAddr) -> Result<(), std::io::Error> {
    // Housekeeping BEFORE binding: sweep crashed in-flight uploads so a
    // dead push can't leak disk under the byte cap forever. Startup-only
    // (no request can be in flight yet).
    let swept = sweep_stale_incoming(
        &cfg.root,
        std::time::SystemTime::now()
            .checked_sub(STALE_INCOMING_AFTER)
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
    );
    if !swept.is_empty() {
        info!(count = swept.len(), "swept stale in-flight snapshot dirs");
    }
    std::fs::create_dir_all(&cfg.root)?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, max_bytes = cfg.max_bytes, max_snapshots = cfg.max_snapshots, "ferriscribe-backup agent listening");
    axum::serve(listener, router(cfg)).await
}

// ── handlers ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct FilePathQuery {
    path: String,
}

async fn put_file(
    State(state): State<AgentState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(q): Query<FilePathQuery>,
    body: axum::body::Bytes,
) -> Result<StatusCode, (StatusCode, String)> {
    let cfg = &state.cfg;
    let scope = auth_scope(&headers, cfg, "put_file").ok_or(unauthorized())?;
    let _ = scope; // both scopes may upload
    validate_snapshot_id(&id)?;
    let rel = validate_rel_path(&q.path)?;

    // Serialize against a concurrent commit of the SAME id: hold the
    // per-snapshot lock across the committed-check and the write.
    let snapshot_lock = state.lock_snapshot(&id);
    let _guard = snapshot_lock.lock().await;

    let snap_dir = cfg.root.join(&id);
    if is_committed(&snap_dir) {
        return Err(conflict("snapshot already committed — append-only"));
    }
    // Bounded growth (R3): reject uploads that would push actual on-disk
    // bytes past the cap. Measured from the filesystem, never from
    // client-declared numbers. (Small race window between concurrent
    // PUTs is acceptable — the commit-time check re-verifies.)
    let (used_bytes, _) = disk_usage(&cfg.root);
    if used_bytes + body.len() as u64 > cfg.max_bytes {
        tracing::warn!(
            used_bytes,
            body_len = body.len(),
            max_bytes = cfg.max_bytes,
            "upload rejected: byte cap would be exceeded"
        );
        return Err(insufficient_storage("byte cap would be exceeded"));
    }
    let dest = snap_dir.join(&rel);
    if dest.exists() {
        // Overwriting within an uncommitted snapshot is allowed (retries),
        // but never after commit — handled above.
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(internal)?;
    }
    tokio::fs::write(&dest, &body).await.map_err(internal)?;
    Ok(StatusCode::CREATED)
}

async fn commit_snapshot(
    State(state): State<AgentState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(receipt): Json<SnapshotReceipt>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let cfg = &state.cfg;
    auth_scope(&headers, cfg, "commit_snapshot").ok_or(unauthorized())?;
    validate_snapshot_id(&id)?;
    if receipt.snapshot_id != id {
        return Err(bad_request("receipt id does not match URL id"));
    }

    // Hold the SAME per-snapshot lock as put_file across the entire
    // validate→freeze window — no upload can slip past the envelope
    // check into the committed tree.
    let snapshot_lock = state.lock_snapshot(&id);
    let _guard = snapshot_lock.lock().await;

    let snap_dir = cfg.root.join(&id);
    if is_committed(&snap_dir) {
        return Err(conflict("snapshot already committed — append-only"));
    }
    if !snap_dir.is_dir() {
        return Err(bad_request("no files uploaded for this snapshot"));
    }

    // Bounded growth (R3): re-check both caps against actual disk usage
    // at commit time — the moment the snapshot becomes immutable history.
    let (used_bytes, committed_count) = disk_usage(&cfg.root);
    if used_bytes > cfg.max_bytes {
        tracing::warn!(
            used_bytes,
            max_bytes = cfg.max_bytes,
            "commit rejected: byte cap exceeded"
        );
        return Err(insufficient_storage("byte cap exceeded"));
    }
    if committed_count + 1 > cfg.max_snapshots {
        tracing::warn!(
            committed_count,
            max_snapshots = cfg.max_snapshots,
            "commit rejected: snapshot-count cap exceeded"
        );
        return Err(insufficient_storage(
            "committed-snapshot count cap exceeded — prune old snapshots on the target",
        ));
    }

    // Envelope validation (finding 3a): the receipt must describe exactly
    // the files the target actually holds. The target is key-free — it
    // cannot (and must not) verify the HMAC — but count/size agreement
    // catches truncated or corrupt pushes here, and closes the
    // lying-receipt bypass against the byte cap. Receipt totals count
    // PAYLOAD files only (manifest.json.enc is not a manifest entry).
    let payload_dir = snap_dir.join(PAYLOAD_DIR);
    let mut uploaded_count: u64 = 0;
    let mut uploaded_bytes: u64 = 0;
    match std::fs::read_dir(&payload_dir) {
        Ok(rd) => {
            for entry in rd.flatten() {
                if let Ok(meta) = entry.metadata()
                    && meta.is_file()
                {
                    uploaded_count += 1;
                    uploaded_bytes += meta.len();
                }
            }
        }
        Err(_) => {
            return Err(bad_request("no payload files uploaded for this snapshot"));
        }
    }
    if uploaded_count != receipt.file_count || uploaded_bytes != receipt.total_bytes {
        tracing::warn!(
            uploaded_count,
            receipt_file_count = receipt.file_count,
            uploaded_bytes,
            receipt_total_bytes = receipt.total_bytes,
            "commit rejected: receipt does not match received files"
        );
        return Err(bad_request(
            "receipt file_count/total_bytes do not match the uploaded payload files",
        ));
    }

    // Write the receipt, then freeze: mark everything read-only and lay
    // down the commit marker. Readers only ever list marker-bearing dirs.
    let receipt_path = snap_dir.join(RECEIPT_FILE);
    let bytes =
        serde_json::to_vec_pretty(&receipt).map_err(|e| internal(std::io::Error::other(e)))?;
    tokio::fs::write(&receipt_path, bytes)
        .await
        .map_err(internal)?;
    freeze_tree(&snap_dir).map_err(internal)?;
    tokio::fs::write(snap_dir.join(".committed"), b"1")
        .await
        .map_err(internal)?;
    set_readonly_file(&snap_dir.join(".committed"));
    set_readonly_dir(&snap_dir);
    info!(snapshot_id = %id, "snapshot committed (append-only)");
    Ok((StatusCode::CREATED, format!("committed {id}")))
}

async fn list_snapshots(
    State(state): State<AgentState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SnapshotReceipt>>, (StatusCode, String)> {
    let cfg = &state.cfg;
    auth_scope(&headers, cfg, "list_snapshots").ok_or(unauthorized())?;
    let mut receipts = Vec::new();
    let mut rd = tokio::fs::read_dir(&cfg.root).await.map_err(internal)?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let dir = entry.path();
        if !is_committed(&dir) {
            continue;
        }
        if let Ok(bytes) = tokio::fs::read(dir.join(RECEIPT_FILE)).await
            && let Ok(receipt) = serde_json::from_slice::<SnapshotReceipt>(&bytes)
        {
            receipts.push(receipt);
        }
    }
    receipts.sort_by_key(|r| std::cmp::Reverse(r.created_at)); // newest first
    Ok(Json(receipts))
}

async fn get_file(
    State(state): State<AgentState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(q): Query<FilePathQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let cfg = &state.cfg;
    auth_scope(&headers, cfg, "get_file").ok_or(unauthorized())?;
    validate_snapshot_id(&id)?;
    let rel = validate_rel_path_read(&q.path)?;
    let snap_dir = cfg.root.join(&id);
    if !is_committed(&snap_dir) {
        return Err(not_found("snapshot not found"));
    }
    let bytes = tokio::fs::read(snap_dir.join(&rel))
        .await
        .map_err(|_| not_found("file not found"))?;
    Ok((StatusCode::OK, bytes))
}

#[derive(Deserialize)]
struct PruneQuery {
    keep: usize,
}

/// Admin-only retention: keep the newest `keep` committed snapshots,
/// delete older ones. This is the ONLY deletion path in the entire
/// system, and it is unreachable with the append token.
async fn prune(
    State(state): State<AgentState>,
    headers: HeaderMap,
    Query(q): Query<PruneQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let cfg = &state.cfg;
    let scope = auth_scope(&headers, cfg, "prune").ok_or(unauthorized())?;
    if scope != Scope::Admin {
        return Err(forbidden("prune requires the target-side admin token"));
    }
    let mut committed: Vec<(chrono::DateTime<chrono::Utc>, PathBuf)> = Vec::new();
    let mut rd = tokio::fs::read_dir(&cfg.root).await.map_err(internal)?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let dir = entry.path();
        if !is_committed(&dir) {
            continue;
        }
        match tokio::fs::read(dir.join(RECEIPT_FILE)).await {
            Ok(bytes) => {
                if let Ok(receipt) = serde_json::from_slice::<SnapshotReceipt>(&bytes) {
                    committed.push((receipt.created_at, dir));
                }
            }
            Err(_) => continue,
        }
    }
    committed.sort_by_key(|a| std::cmp::Reverse(a.0)); // newest first
    let mut pruned = Vec::new();
    for (_, dir) in committed.iter().skip(q.keep) {
        unfreeze_and_remove(dir).map_err(internal)?;
        pruned.push(
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
    }
    info!(count = pruned.len(), keep = q.keep, "pruned old snapshots");
    Ok((
        StatusCode::OK,
        format!("pruned {} snapshot(s)", pruned.len()),
    ))
}

// ── helpers ──────────────────────────────────────────────────────────────

fn is_committed(snap_dir: &Path) -> bool {
    snap_dir.join(".committed").exists()
}

fn validate_snapshot_id(id: &str) -> Result<(), (StatusCode, String)> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(bad_request("invalid snapshot id"))
    }
}

/// Relative path inside a snapshot for WRITES: no traversal, no absolute,
/// bounded charset, and not the commit marker or the receipt (the receipt
/// is written only by the commit handler).
fn validate_rel_path(rel: &str) -> Result<PathBuf, (StatusCode, String)> {
    validate_rel_path_impl(rel, false)
}

/// Relative path for READS: same constraints, but `receipt.json` is
/// readable (pulls and listing fetch it).
fn validate_rel_path_read(rel: &str) -> Result<PathBuf, (StatusCode, String)> {
    validate_rel_path_impl(rel, true)
}

fn validate_rel_path_impl(rel: &str, allow_receipt: bool) -> Result<PathBuf, (StatusCode, String)> {
    let path = Path::new(rel);
    let ok = !rel.is_empty()
        && rel.len() <= 128
        && !path.is_absolute()
        && rel
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
        && !path.components().any(|c| c.as_os_str() == "..")
        && (allow_receipt || rel != RECEIPT_FILE)
        && rel != ".committed";
    if ok {
        Ok(path.to_path_buf())
    } else {
        Err(bad_request("invalid file path"))
    }
}

fn freeze_tree(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            freeze_tree(&path)?;
            set_readonly_dir(&path);
        } else {
            set_readonly_file(&path);
        }
    }
    Ok(())
}

fn unfreeze_and_remove(dir: &Path) -> std::io::Result<()> {
    fn unfreeze(d: &Path) -> std::io::Result<()> {
        for entry in std::fs::read_dir(d)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                unfreeze(&path)?;
                let _ = std::fs::set_permissions(&path, writable_dir(&path)?);
            }
        }
        let _ = std::fs::set_permissions(d, writable_dir(d)?);
        Ok(())
    }
    unfreeze(dir)?;
    std::fs::remove_dir_all(dir)
}

#[cfg(unix)]
fn set_readonly_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444));
}

#[cfg(unix)]
fn set_readonly_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o555));
}

#[cfg(unix)]
fn writable_dir(_path: &Path) -> std::io::Result<std::fs::Permissions> {
    use std::os::unix::fs::PermissionsExt;
    Ok(std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_readonly_file(_path: &Path) {}
#[cfg(not(unix))]
fn set_readonly_dir(_path: &Path) {}
// Windows Permissions only exposes the readonly bit — clone the
// directory's current permissions with readonly cleared. (Permissions
// has no portable constructor; that was the Windows build failure.)
#[cfg(not(unix))]
fn writable_dir(path: &Path) -> std::io::Result<std::fs::Permissions> {
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_readonly(false);
    Ok(perms)
}

fn unauthorized() -> (StatusCode, String) {
    (
        StatusCode::UNAUTHORIZED,
        "invalid or missing bearer token".into(),
    )
}
fn forbidden(msg: &str) -> (StatusCode, String) {
    (StatusCode::FORBIDDEN, msg.into())
}
fn conflict(msg: &str) -> (StatusCode, String) {
    (StatusCode::CONFLICT, msg.into())
}
fn insufficient_storage(msg: &str) -> (StatusCode, String) {
    (StatusCode::INSUFFICIENT_STORAGE, msg.into())
}
fn bad_request(msg: &str) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, msg.into())
}
fn not_found(msg: &str) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, msg.into())
}
fn internal(e: std::io::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_validation_blocks_traversal_and_marker_files() {
        assert!(validate_rel_path("payload/f000000.bin").is_ok());
        assert!(validate_rel_path("manifest.json.enc").is_ok());
        assert!(validate_rel_path("../escape").is_err());
        assert!(validate_rel_path("/absolute").is_err());
        assert!(validate_rel_path("payload/../../x").is_err());
        assert!(
            validate_rel_path(RECEIPT_FILE).is_err(),
            "receipt is commit-owned"
        );
        assert!(validate_rel_path(".committed").is_err());
        assert!(validate_rel_path("payload/a b.bin").is_err(), "no spaces");
    }

    #[test]
    fn snapshot_id_validation() {
        assert!(validate_snapshot_id("snap-20260820T010203Z-ab12cd").is_ok());
        assert!(validate_snapshot_id("../etc").is_err());
        assert!(validate_snapshot_id("a b").is_err());
        assert!(validate_snapshot_id("").is_err());
    }

    #[test]
    fn sweep_removes_stale_incoming_but_never_committed() {
        let root = tempfile::TempDir::new().unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 60 * 24 * 8);

        // A committed snapshot (marker + receipt).
        let committed = root.path().join("snap-committed");
        std::fs::create_dir_all(committed.join("payload")).unwrap();
        std::fs::write(committed.join(".committed"), b"1").unwrap();
        std::fs::write(committed.join("receipt.json"), b"{}").unwrap();
        // Age it past the cutoff — committed history is never swept.
        set_mtime_old(&committed.join(".committed"), old);

        // A crashed in-flight upload, stale (every path inside aged —
        // the sweep judges by the NEWEST mtime, dirs included).
        let incoming = root.path().join("snap-crashed");
        std::fs::create_dir_all(incoming.join("payload")).unwrap();
        std::fs::write(incoming.join("payload/f000000.bin"), b"bytes").unwrap();
        set_mtime_old(&incoming, old);
        set_mtime_old(&incoming.join("payload"), old);
        set_mtime_old(&incoming.join("payload/f000000.bin"), old);

        // A FRESH in-flight upload — recent, must survive.
        let fresh = root.path().join("snap-inprogress");
        std::fs::create_dir_all(fresh.join("payload")).unwrap();
        std::fs::write(fresh.join("payload/f000000.bin"), b"bytes").unwrap();

        let swept = sweep_stale_incoming(
            root.path(),
            std::time::SystemTime::now() - STALE_INCOMING_AFTER,
        );
        assert_eq!(swept, vec!["snap-crashed".to_string()]);
        assert!(committed.join(".committed").exists(), "committed survives");
        assert!(fresh.join("payload/f000000.bin").exists(), "fresh survives");
        assert!(!incoming.exists(), "stale incoming removed");
    }

    fn set_mtime_old(path: &Path, t: std::time::SystemTime) {
        // Read-only handle works for directories too (futimens checks
        // ownership, not fd access mode; the test owns everything).
        let f = std::fs::File::open(path).unwrap();
        f.set_modified(t).unwrap();
    }
}
