//! Source-side client: push a built snapshot to the target agent, and
//! pull one back (restore-on-clean-machine, or the weekly drill pulling
//! from the target to test the FULL path).
//!
//! v3 (CAS) pushes upload each unique blob exactly once (`HEAD` first,
//! `PUT` only the absent ones — the target store is content-addressed),
//! then commit with the receipt + plaintext blob-hash list. Legacy (v2)
//! snapshot dirs are still pushed per-file over the original routes.

use std::path::{Path, PathBuf};

use crate::BackupResult;
use crate::snapshot::{self, MANIFEST_FILE, PAYLOAD_DIR, RECEIPT_FILE, SnapshotReceipt};

/// What a CAS push actually transferred, for job events and the
/// incremental-backup acceptance tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PushStats {
    /// Blobs the target did not hold and were uploaded.
    pub uploaded: u64,
    /// Blobs already on the target (HEAD said present) — skipped.
    pub skipped: u64,
}

/// HTTP client for the backup target agent.
pub struct BackupClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl BackupClient {
    pub fn new(base_url: impl Into<String>, append_token: impl Into<String>) -> Self {
        // NO total request timeout: a single blob PUT can legitimately run
        // for hours on a relayed Tailscale link (the initial full push is
        // ~33 GB). Stalled DOWNLOADS/reads are bounded by the connect +
        // read timeouts; a SLOW upload survives, and a fully dead upload
        // peer is bounded only by the server closing the request — the
        // old blanket 300 s timeout aborted any blob larger than the link
        // could carry in five minutes, which was strictly worse.
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .read_timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("reqwest client builds");
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: append_token.into(),
            http,
        }
    }

    /// Push a built snapshot directory to the agent, then commit it. The
    /// snapshot id and receipt come from the local `receipt.json`.
    /// Re-pushing an already-committed id fails with a 409 from the target
    /// (append-only).
    ///
    /// v3 (CAS): every unique blob is uploaded at most once (HEAD first);
    /// `recordings_dir` resolves stream-staged entries that have no local
    /// payload file, and `wrapping_key` decrypts the manifest to learn
    /// the blob list (also sent with the commit — the agent is key-free;
    /// that list is its ONLY way to know GC reachability). v2 dirs keep
    /// the legacy per-file push and ignore both.
    pub async fn push_snapshot(
        &self,
        snapshot_dir: &Path,
        recordings_dir: Option<&Path>,
        wrapping_key: &[u8; 32],
    ) -> BackupResult<(SnapshotReceipt, PushStats)> {
        let receipt: SnapshotReceipt =
            serde_json::from_slice(&std::fs::read(snapshot_dir.join(RECEIPT_FILE))?)?;

        // The manifest is needed in both branches: the blob list for a CAS
        // push, and the commit payload. Decrypt once.
        let manifest_bytes = std::fs::read(snapshot_dir.join(MANIFEST_FILE))?;
        let manifest = if receipt.version >= 3 {
            let aes_key = crate::keys::snapshot_aes_key(wrapping_key);
            let plain =
                medical_security::file_crypto::decrypt_bytes_with_key(&aes_key, &manifest_bytes)
                    .map_err(|e| {
                        crate::BackupError::Verification(format!("manifest decrypt failed: {e}"))
                    })?;
            Some(
                serde_json::from_slice::<snapshot::SnapshotManifest>(&plain)
                    .map_err(|e| crate::BackupError::Format(format!("manifest parse: {e}")))?,
            )
        } else {
            None
        };

        let stats = match &manifest {
            Some(m) => {
                self.push_cas(snapshot_dir, &receipt, recordings_dir, m)
                    .await?
            }
            None => self.push_legacy(snapshot_dir, &receipt).await?,
        };

        // Commit — the agent freezes the snapshot read-only.
        let mut body = serde_json::to_value(&receipt)?;
        if let Some(m) = &manifest {
            let mut hashes: Vec<String> = m.entries.iter().map(|e| e.sha256.clone()).collect();
            hashes.sort();
            hashes.dedup();
            body["blobs"] = serde_json::json!(hashes);
        }
        let resp = self
            .http
            .post(format!(
                "{}/v1/snapshots/{}/commit",
                self.base_url, receipt.snapshot_id
            ))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(crate::BackupError::Verification(format!(
                "commit failed: HTTP {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        Ok((receipt, stats))
    }

    /// Legacy (v2) per-file push: manifest + every payload file.
    async fn push_legacy(
        &self,
        snapshot_dir: &Path,
        receipt: &SnapshotReceipt,
    ) -> BackupResult<PushStats> {
        // manifest.json.enc at the snapshot root…
        self.put_file(
            &receipt.snapshot_id,
            MANIFEST_FILE,
            &std::fs::read(snapshot_dir.join(MANIFEST_FILE))?,
        )
        .await?;
        // …and every payload file.
        let payload_dir = snapshot_dir.join(PAYLOAD_DIR);
        let mut names: Vec<String> = std::fs::read_dir(&payload_dir)?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        names.sort();
        let mut uploaded = 0u64;
        for name in names {
            let bytes = std::fs::read(payload_dir.join(&name))?;
            self.put_file(
                &receipt.snapshot_id,
                &format!("{PAYLOAD_DIR}/{name}"),
                &bytes,
            )
            .await?;
            uploaded += 1;
        }
        Ok(PushStats {
            uploaded,
            skipped: 0,
        })
    }

    /// CAS (v3) push: manifest via the snapshot file route, then each
    /// referenced blob once. Stream-staged recordings (no local payload
    /// file) are streamed from `recordings_dir`.
    async fn push_cas(
        &self,
        snapshot_dir: &Path,
        receipt: &SnapshotReceipt,
        recordings_dir: Option<&Path>,
        manifest: &snapshot::SnapshotManifest,
    ) -> BackupResult<PushStats> {
        self.put_file(
            &receipt.snapshot_id,
            MANIFEST_FILE,
            &std::fs::read(snapshot_dir.join(MANIFEST_FILE))?,
        )
        .await?;

        let payload_dir = snapshot_dir.join(PAYLOAD_DIR);
        let mut stats = PushStats::default();
        let mut seen = std::collections::HashSet::new();
        for entry in &manifest.entries {
            if !seen.insert(entry.sha256.clone()) {
                continue; // two manifest entries may share one blob
            }
            // Staged locally, or stream from the recordings source?
            let staged = payload_dir.join(&entry.sha256);
            let source: PathBuf = if staged.is_file() {
                staged
            } else if let Some(dir) = recordings_dir
                && let Some(name) = entry.relative_path.strip_prefix("recordings/")
            {
                let p = dir.join(name);
                if !p.is_file() {
                    // PHI rule: the recording filename may embed a patient
                    // name and lives only inside the ENCRYPTED manifest —
                    // errors carry the blob hash prefix, never the name.
                    return Err(crate::BackupError::Verification(format!(
                        "stream-staged blob source missing for blob {}",
                        &entry.sha256[..8.min(entry.sha256.len())]
                    )));
                }
                p
            } else {
                return Err(crate::BackupError::Verification(format!(
                    "no local payload for blob {} and no recordings_dir to stream it from \
                     (was this snapshot built in Stream mode?)",
                    &entry.sha256[..8.min(entry.sha256.len())]
                )));
            };
            if self.head_blob(&entry.sha256).await? {
                stats.skipped += 1;
            } else {
                self.put_blob(&entry.sha256, &source).await?;
                stats.uploaded += 1;
            }
        }
        Ok(stats)
    }

    async fn put_file(&self, id: &str, path: &str, body: &[u8]) -> BackupResult<()> {
        let resp = self
            .http
            .put(format!(
                "{}/v1/snapshots/{id}/file?path={path}",
                self.base_url
            ))
            .bearer_auth(&self.token)
            .body(body.to_vec())
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(crate::BackupError::Verification(format!(
                "upload of {path} failed: HTTP {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// Does the target already hold this blob?
    async fn head_blob(&self, hash: &str) -> BackupResult<bool> {
        let resp = self
            .http
            .head(format!("{}/v1/blobs/{hash}", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await?;
        match resp.status() {
            reqwest::StatusCode::NO_CONTENT => Ok(true),
            reqwest::StatusCode::NOT_FOUND => Ok(false),
            other => Err(crate::BackupError::Verification(format!(
                "blob existence check failed: HTTP {other}"
            ))),
        }
    }

    /// Stream a blob from `source` — the file is never buffered whole
    /// (blobs can be ~1 GB).
    async fn put_blob(&self, hash: &str, source: &Path) -> BackupResult<()> {
        let file = tokio::fs::File::open(source).await?;
        let stream = tokio_util::io::ReaderStream::with_capacity(file, 256 * 1024);
        let resp = self
            .http
            .put(format!("{}/v1/blobs/{hash}", self.base_url))
            .bearer_auth(&self.token)
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(crate::BackupError::Verification(format!(
                "blob upload failed: HTTP {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// List committed snapshots on the target (newest first).
    pub async fn list_snapshots(&self) -> BackupResult<Vec<SnapshotReceipt>> {
        let resp = self
            .http
            .get(format!("{}/v1/snapshots", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(crate::BackupError::Verification(format!(
                "list failed: HTTP {}",
                resp.status()
            )));
        }
        Ok(resp.json().await?)
    }

    /// Pull a snapshot (or the latest, when `id` is `None`) into
    /// `dest_dir/<snapshot-id>/` and verify it end-to-end (R5) before
    /// trusting a single byte's placement. Returns the local path.
    pub async fn pull_snapshot(
        &self,
        id: Option<&str>,
        dest_dir: &Path,
        wrapping_key: &[u8; 32],
    ) -> BackupResult<PathBuf> {
        let receipts = self.list_snapshots().await?;
        let receipt = match id {
            Some(wanted) => receipts
                .iter()
                .find(|r| r.snapshot_id == wanted)
                .ok_or_else(|| {
                    crate::BackupError::Verification(format!(
                        "snapshot {wanted} not found on target"
                    ))
                })?,
            None => receipts.first().ok_or_else(|| {
                crate::BackupError::Verification("target has no snapshots".into())
            })?,
        };
        let local_dir = dest_dir.join(&receipt.snapshot_id);
        std::fs::create_dir_all(local_dir.join(PAYLOAD_DIR))?;

        // Receipt from the target.
        let receipt_bytes = self.get_file(&receipt.snapshot_id, RECEIPT_FILE).await?;
        std::fs::write(local_dir.join(RECEIPT_FILE), &receipt_bytes)?;

        // Manifest + every payload blob. The manifest is encrypted on the
        // wire (it contains PHI-sensitive paths); decrypt locally with the
        // wrapping key to learn which blobs to fetch.
        let manifest_bytes = self.get_file(&receipt.snapshot_id, MANIFEST_FILE).await?;
        std::fs::write(local_dir.join(MANIFEST_FILE), &manifest_bytes)?;
        let aes_key = crate::keys::snapshot_aes_key(wrapping_key);
        let manifest_plain =
            medical_security::file_crypto::decrypt_bytes_with_key(&aes_key, &manifest_bytes)
                .map_err(|e| {
                    crate::BackupError::Verification(format!("manifest decrypt failed: {e}"))
                })?;
        let manifest: snapshot::SnapshotManifest = serde_json::from_slice(&manifest_plain)
            .map_err(|e| crate::BackupError::Format(format!("manifest parse: {e}")))?;

        let mut fetched = std::collections::HashSet::new();
        for entry in &manifest.entries {
            let dest = local_dir.join(PAYLOAD_DIR).join(&entry.opaque_name);
            if receipt.version >= 3 {
                // CAS: one shared blob on the target serves every entry
                // referencing it (opaque_name == hash). Dedup by hash only
                // in this branch — a v2 snapshot may hold two entries with
                // identical bytes under DIFFERENT opaque names, and each
                // private copy must still be fetched.
                if !fetched.insert(entry.sha256.clone()) {
                    continue;
                }
                self.get_blob(&entry.sha256, &dest).await?;
            } else {
                let bytes = self
                    .get_file(
                        &receipt.snapshot_id,
                        &format!("{PAYLOAD_DIR}/{}", entry.opaque_name),
                    )
                    .await?;
                std::fs::write(&dest, bytes)?;
            }
        }

        // Fail closed: the pulled snapshot must verify before use.
        snapshot::verify_snapshot(&local_dir, wrapping_key)?;
        Ok(local_dir)
    }

    /// Stream a blob down to `dest` — never buffered whole.
    async fn get_blob(&self, hash: &str, dest: &Path) -> BackupResult<()> {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;

        let resp = self
            .http
            .get(format!("{}/v1/blobs/{hash}", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(crate::BackupError::Verification(format!(
                "blob download failed: HTTP {}",
                resp.status()
            )));
        }
        let mut file = tokio::fs::File::create(dest).await?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                crate::BackupError::Verification(format!("blob download aborted: {e}"))
            })?;
            file.write_all(&chunk).await?;
        }
        file.sync_all().await?;
        Ok(())
    }

    async fn get_file(&self, id: &str, path: &str) -> BackupResult<Vec<u8>> {
        let resp = self
            .http
            .get(format!(
                "{}/v1/snapshots/{id}/file?path={path}",
                self.base_url
            ))
            .bearer_auth(&self.token)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(crate::BackupError::Verification(format!(
                "download of {path} failed: HTTP {}",
                resp.status()
            )));
        }
        Ok(resp.bytes().await?.to_vec())
    }
}
