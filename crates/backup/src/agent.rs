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

use crate::snapshot::{RECEIPT_FILE, SnapshotReceipt};

/// Agent configuration: storage root + the two credentials.
#[derive(Clone)]
pub struct AgentConfig {
    pub root: PathBuf,
    pub append_token: String,
    pub admin_token: String,
}

/// What a request's bearer token is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Append,
    Admin,
}

fn auth_scope(headers: &HeaderMap, cfg: &AgentConfig) -> Option<Scope> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?;
    let raw = value.to_str().ok()?;
    let token = raw.strip_prefix("Bearer ")?;
    if constant_time_eq(token, &cfg.admin_token) {
        Some(Scope::Admin)
    } else if constant_time_eq(token, &cfg.append_token) {
        Some(Scope::Append)
    } else {
        None
    }
}

/// Constant-time-ish comparison (length check + byte fold) so a token
/// probe can't shortcut on prefix matches. Not a cryptographic CT guard,
/// but raises the bar over `==` on a LAN-facing service.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Build the agent router (public so tests can drive it in-process).
pub fn router(cfg: AgentConfig) -> Router {
    Router::new()
        .route("/v1/snapshots", get(list_snapshots))
        .route("/v1/snapshots/{id}/file", get(get_file).put(put_file))
        .route("/v1/snapshots/{id}/commit", post(commit_snapshot))
        .route("/v1/admin/prune", post(prune))
        .with_state(cfg)
}

/// Serve until the process is stopped. Binds `addr` (typically a
/// Tailscale IP — see the README deployment notes).
pub async fn serve(cfg: AgentConfig, addr: SocketAddr) -> Result<(), std::io::Error> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "ferriscribe-backup agent listening");
    axum::serve(listener, router(cfg)).await
}

// ── handlers ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct FilePathQuery {
    path: String,
}

async fn put_file(
    State(cfg): State<AgentConfig>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(q): Query<FilePathQuery>,
    body: axum::body::Bytes,
) -> Result<StatusCode, (StatusCode, String)> {
    let scope = auth_scope(&headers, &cfg).ok_or(unauthorized())?;
    let _ = scope; // both scopes may upload
    validate_snapshot_id(&id)?;
    let rel = validate_rel_path(&q.path)?;

    let snap_dir = cfg.root.join(&id);
    if is_committed(&snap_dir) {
        return Err(conflict("snapshot already committed — append-only"));
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
    State(cfg): State<AgentConfig>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(receipt): Json<SnapshotReceipt>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    auth_scope(&headers, &cfg).ok_or(unauthorized())?;
    validate_snapshot_id(&id)?;
    if receipt.snapshot_id != id {
        return Err(bad_request("receipt id does not match URL id"));
    }
    let snap_dir = cfg.root.join(&id);
    if is_committed(&snap_dir) {
        return Err(conflict("snapshot already committed — append-only"));
    }
    if !snap_dir.is_dir() {
        return Err(bad_request("no files uploaded for this snapshot"));
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
    State(cfg): State<AgentConfig>,
    headers: HeaderMap,
) -> Result<Json<Vec<SnapshotReceipt>>, (StatusCode, String)> {
    auth_scope(&headers, &cfg).ok_or(unauthorized())?;
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
    receipts.sort_by(|a, b| b.created_at.cmp(&a.created_at)); // newest first
    Ok(Json(receipts))
}

async fn get_file(
    State(cfg): State<AgentConfig>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(q): Query<FilePathQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    auth_scope(&headers, &cfg).ok_or(unauthorized())?;
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
    State(cfg): State<AgentConfig>,
    headers: HeaderMap,
    Query(q): Query<PruneQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let scope = auth_scope(&headers, &cfg).ok_or(unauthorized())?;
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
    committed.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
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
                let _ = std::fs::set_permissions(&path, writable_dir());
            }
        }
        let _ = std::fs::set_permissions(d, writable_dir());
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
fn writable_dir() -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    std::fs::Permissions::from_mode(0o755)
}

#[cfg(not(unix))]
fn set_readonly_file(_path: &Path) {}
#[cfg(not(unix))]
fn set_readonly_dir(_path: &Path) {}
#[cfg(not(unix))]
fn writable_dir() -> std::fs::Permissions {
    std::fs::Permissions::new()
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
}
