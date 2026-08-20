//! Source-side client: push a built snapshot to the target agent, and
//! pull one back (restore-on-clean-machine, or the weekly drill pulling
//! from the target to test the FULL path).

use std::path::{Path, PathBuf};

use crate::BackupResult;
use crate::snapshot::{self, MANIFEST_FILE, PAYLOAD_DIR, RECEIPT_FILE, SnapshotReceipt};

/// HTTP client for the backup target agent.
pub struct BackupClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl BackupClient {
    pub fn new(base_url: impl Into<String>, append_token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: append_token.into(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("reqwest client builds"),
        }
    }

    /// Push every file of a built snapshot directory to the agent, then
    /// commit it. The snapshot id and receipt come from the local
    /// `receipt.json`. Re-pushing an already-committed id fails with a
    /// 409 from the target (append-only).
    pub async fn push_snapshot(&self, snapshot_dir: &Path) -> BackupResult<SnapshotReceipt> {
        let receipt: SnapshotReceipt =
            serde_json::from_slice(&std::fs::read(snapshot_dir.join(RECEIPT_FILE))?)?;

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
        for name in names {
            let bytes = std::fs::read(payload_dir.join(&name))?;
            self.put_file(
                &receipt.snapshot_id,
                &format!("{PAYLOAD_DIR}/{name}"),
                &bytes,
            )
            .await?;
        }

        // Commit — the agent freezes the snapshot read-only.
        let resp = self
            .http
            .post(format!(
                "{}/v1/snapshots/{}/commit",
                self.base_url, receipt.snapshot_id
            ))
            .bearer_auth(&self.token)
            .json(&receipt)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(crate::BackupError::Verification(format!(
                "commit failed: HTTP {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        Ok(receipt)
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

        // Manifest + every payload file listed in it. The manifest is
        // encrypted on the wire (it contains PHI-sensitive paths); decrypt
        // locally with the wrapping key to learn which files to fetch.
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
        for entry in &manifest.entries {
            let bytes = self
                .get_file(
                    &receipt.snapshot_id,
                    &format!("{PAYLOAD_DIR}/{}", entry.opaque_name),
                )
                .await?;
            std::fs::write(local_dir.join(PAYLOAD_DIR).join(&entry.opaque_name), bytes)?;
        }

        // Fail closed: the pulled snapshot must verify before use.
        snapshot::verify_snapshot(&local_dir, wrapping_key)?;
        Ok(local_dir)
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
