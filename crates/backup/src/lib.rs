//! Encrypted off-machine backup for FerriScribe.
//!
//! Design contract (see `.zcode/plans/plan-ferriscribe-off-machine-backup.md`):
//!
//! - **R1 Key escrow** — snapshots are encrypted under a *backup wrapping
//!   key* whose off-machine copies are two independent, individually
//!   verifiable escrow artifacts: a printable recovery sheet and an
//!   offline USB file ([`escrow`]). The SQLCipher DB key itself travels
//!   inside every snapshot, wrapped under the snapshot key, so ciphertext
//!   + escrow = recoverable.
//! - **R3 Append-only** — the target-side agent ([`agent`], added in a
//!   follow-up commit) exposes no delete/overwrite route to the push
//!   credential, so a compromised source cannot erase its own history.
//! - **R4 Tested restore** — [`drill`] restores the latest snapshot into a
//!   temp directory, opens the restored SQLCipher DB with the recovered
//!   key, decrypts a sample recording, and diffs record counts.
//! - **R5 Integrity + authenticity** — every snapshot carries an HMAC-SHA256
//!   tag over the receipt's canonical fields plus every payload hash;
//!   verification fails closed ([`snapshot::verify_snapshot`]).
//!
//! # PHI rules
//!
//! Snapshot filenames are opaque (`snap-<timestamp>-<rand>`, `fNNNNNN.bin`).
//! The plaintext receipt carries only counts, sizes, and the HMAC tag — no
//! paths, no patient data. Original relative paths (which may embed patient
//! names in recording filenames) live only inside the *encrypted* manifest.

pub mod agent;
pub mod client;
pub mod drill;
pub mod escrow;
pub mod job;
pub mod keys;
pub mod schedule;
pub mod snapshot;
pub mod status;

/// Errors surfaced by the backup tooling.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("keychain error: {0}")]
    Keychain(#[from] medical_security::keychain::KeychainError),
    #[error("crypto error: {0}")]
    Crypto(#[from] medical_security::file_crypto::FileCryptoError),
    #[error("database error: {0}")]
    Db(#[from] medical_db::DbError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("snapshot verification failed: {0}")]
    Verification(String),
    /// Installation/deployment failures (sidecar staging, scheduling,
    /// task join errors) — nothing to do with key escrow.
    #[error("setup error: {0}")]
    Setup(String),
    #[error("escrow error: {0}")]
    Escrow(String),
    #[error("missing key material: {0}")]
    MissingKey(String),
    #[error("snapshot format error: {0}")]
    Format(String),
}

/// Result alias for the backup crate.
pub type BackupResult<T> = Result<T, BackupError>;

/// Raw rusqlite errors (backup opens its own connections alongside the
/// pooled ones from `medical_db`).
impl From<rusqlite::Error> for BackupError {
    fn from(e: rusqlite::Error) -> Self {
        BackupError::Db(medical_db::DbError::Sqlite(e))
    }
}

/// End-to-end transport tests: real agent (in-process, random port) +
/// real HTTP client, against real snapshots built from a SQLCipher
/// fixture. These encode the R3 contract — the source-side credential
/// physically cannot delete or overwrite history.
#[cfg(test)]
mod transport_tests {
    use super::*;
    use agent::{AgentConfig, router};
    use client::BackupClient;
    use snapshot::{BuildOptions, SnapshotReceipt, build_snapshot, verify_snapshot};
    use std::path::Path;

    use medical_core::types::recording::Recording;
    use medical_db::recordings::RecordingsRepo;
    use uuid::Uuid;

    struct Env {
        base_url: String,
        append: String,
        admin: String,
        _root: tempfile::TempDir,
    }

    async fn spawn_agent_with(max_bytes: u64, max_snapshots: usize, keep_n: Option<usize>) -> Env {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir_all(root.path()).unwrap();
        let append = format!("append-{}", Uuid::new_v4());
        let admin = format!("admin-{}", Uuid::new_v4());
        let app = router(AgentConfig {
            root: root.path().to_path_buf(),
            append_token: append.clone(),
            admin_token: admin.clone(),
            max_bytes,
            max_snapshots,
            keep_n,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        Env {
            base_url: format!("http://{addr}"),
            append,
            admin,
            _root: root,
        }
    }

    async fn spawn_agent() -> Env {
        spawn_agent_with(agent::DEFAULT_MAX_BYTES, agent::DEFAULT_MAX_SNAPSHOTS, None).await
    }

    fn build_fixture_snapshot(dest: &Path, salt: u8) -> SnapshotReceipt {
        build_fixture_snapshot_staged(dest, salt, snapshot::StagingMode::Hardlink, None).0
    }

    /// Stream-staged fixture build that also returns the recordings dir
    /// (kept alive via a leaked TempDir — test-scoped, disposable) so
    /// incremental tests can add recordings between builds and pass the
    /// dir to push for source streaming.
    fn build_fixture_snapshot_staged(
        dest: &Path,
        salt: u8,
        staging: snapshot::StagingMode,
        existing_recordings: Option<&Path>,
    ) -> (SnapshotReceipt, std::path::PathBuf) {
        let db_key = [salt; 32];
        let wrapping = [salt ^ 0xFF; 32];
        let src: &'static tempfile::TempDir =
            Box::leak(Box::new(tempfile::tempdir().expect("src")));
        let db_path = src.path().join("medical.db");
        let database = medical_db::Database::open(&db_path, Some(db_key)).expect("db");
        {
            let conn = database.conn().expect("conn");
            RecordingsRepo::insert(
                &conn,
                &Recording::new(
                    format!("r{salt}.enc"),
                    src.path().join(format!("r{salt}.enc")),
                ),
            )
            .expect("insert");
        }
        drop(database);
        // Real incremental semantics need the recordings dir to be STABLE
        // across builds (immutable ciphertext ⇒ stable hash). A fresh
        // GCM nonce per call would otherwise make every build look like a
        // new recording.
        let recordings: std::path::PathBuf = match existing_recordings {
            Some(dir) => dir.to_path_buf(),
            None => {
                let recordings = src.path().join("recordings");
                std::fs::create_dir_all(&recordings).unwrap();
                let wav_key = medical_security::file_crypto::derive_file_key(&db_key);
                let blob = medical_security::file_crypto::encrypt_bytes_with_key(
                    &wav_key,
                    format!("RIFF patient audio {salt}").as_bytes(),
                )
                .unwrap();
                std::fs::write(recordings.join("r0.enc"), blob).unwrap();
                recordings
            }
        };

        let recordings = recordings.clone();
        let receipt = build_snapshot(&BuildOptions {
            db_path,
            recordings_dir: recordings.clone(),
            keystore_path: None,
            dest_dir: dest.to_path_buf(),
            db_key,
            wrapping_key: wrapping,
            staging,
        })
        .expect("build snapshot");
        (receipt, recordings)
    }

    fn client(env: &Env) -> BackupClient {
        BackupClient::new(&env.base_url, &env.append)
    }

    fn wrapping_for(salt: u8) -> [u8; 32] {
        [salt ^ 0xFF; 32]
    }

    #[tokio::test]
    async fn push_list_pull_roundtrip_verifies_end_to_end() {
        let env = spawn_agent().await;
        let built = tempfile::tempdir().expect("built");
        let receipt = build_fixture_snapshot(built.path(), 0x11);

        let (pushed, _stats) = client(&env)
            .push_snapshot(
                &built.path().join(&receipt.snapshot_id),
                None,
                &wrapping_for(0x11),
            )
            .await
            .expect("push");
        assert_eq!(pushed.snapshot_id, receipt.snapshot_id);

        let listed = client(&env).list_snapshots().await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].snapshot_id, receipt.snapshot_id);

        let pulled_dir = tempfile::tempdir().expect("pulled");
        let local = client(&env)
            .pull_snapshot(None, pulled_dir.path(), &wrapping_for(0x11))
            .await
            .expect("pull");
        assert!(local.ends_with(&receipt.snapshot_id));
        // The pulled copy must pass full verification — this is the path
        // a clean-machine restore would use.
        assert!(verify_snapshot(&local, &wrapping_for(0x11)).is_ok());
    }

    /// Acceptance criterion 2, restated as code: a client holding ONLY
    /// the append token cannot delete or overwrite a committed snapshot,
    /// no matter what it sends.
    #[tokio::test]
    async fn append_only_holds_against_source_credential() {
        let env = spawn_agent().await;
        let c = client(&env);

        // Push two snapshots (same salt is fine — separate builds).
        let built = tempfile::tempdir().expect("built");
        let r1 = build_fixture_snapshot(built.path(), 0x22);
        c.push_snapshot(
            &built.path().join(&r1.snapshot_id),
            None,
            &wrapping_for(0x22),
        )
        .await
        .expect("push 1");

        let id = r1.snapshot_id.clone();

        // 1. Re-committing an existing id → 409 (replaying the original
        // CAS commit body, blobs and all).
        let mut replay = serde_json::to_value(&r1).unwrap();
        let mut blobs: Vec<String> = std::fs::read_dir(built.path().join(&id).join("payload"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        blobs.sort();
        replay["blobs"] = serde_json::json!(blobs);
        let resp = reqwest::Client::new()
            .post(format!("{}/v1/snapshots/{id}/commit", env.base_url))
            .bearer_auth(&env.append)
            .json(&replay)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 409, "re-commit must conflict");

        // 2. Uploading a file into a committed snapshot → 409.
        let resp = reqwest::Client::new()
            .put(format!(
                "{}/v1/snapshots/{id}/file?path=payload/x.bin",
                env.base_url
            ))
            .bearer_auth(&env.append)
            .body(Vec::from(b"overwrite attempt".as_slice()))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            409,
            "write into committed snapshot must conflict"
        );

        // 3. No DELETE route exists at all.
        for path in [
            format!("/v1/snapshots/{id}"),
            format!("/v1/snapshots/{id}/file?path=payload/f000000.bin"),
            "/v1/snapshots".to_string(),
        ] {
            let resp = reqwest::Client::new()
                .delete(format!("{}{path}", env.base_url))
                .bearer_auth(&env.append)
                .send()
                .await
                .unwrap();
            assert!(
                resp.status() == 404 || resp.status() == 405,
                "DELETE {path} must not exist, got {}",
                resp.status()
            );
        }

        // 4. Pruning with the append token → 403, snapshot survives.
        let resp = reqwest::Client::new()
            .post(format!("{}/v1/admin/prune?keep=0", env.base_url))
            .bearer_auth(&env.append)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403, "append token must not reach prune");
        let listed = c.list_snapshots().await.expect("list");
        assert_eq!(listed.len(), 1, "snapshot must still exist");

        // 5. Bad token → 401.
        let resp = reqwest::Client::new()
            .get(format!("{}/v1/snapshots", env.base_url))
            .bearer_auth("wrong-token")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    /// The admin token (target-side only) CAN prune — retention is the
    /// one sanctioned deletion path, keeping the newest N.
    #[tokio::test]
    async fn admin_prune_keeps_newest_snapshots() {
        let env = spawn_agent().await;
        let c = client(&env);
        let built = tempfile::tempdir().expect("built");
        let r1 = build_fixture_snapshot(built.path(), 0x33);
        c.push_snapshot(
            &built.path().join(&r1.snapshot_id),
            None,
            &wrapping_for(0x33),
        )
        .await
        .unwrap();
        // Second snapshot with a distinct id (new timestamp/rand suffix).
        let r2 = build_fixture_snapshot(built.path(), 0x33);
        c.push_snapshot(
            &built.path().join(&r2.snapshot_id),
            None,
            &wrapping_for(0x33),
        )
        .await
        .unwrap();
        assert_eq!(c.list_snapshots().await.unwrap().len(), 2);

        let resp = reqwest::Client::new()
            .post(format!("{}/v1/admin/prune?keep=1", env.base_url))
            .bearer_auth(&env.admin)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let listed = c.list_snapshots().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].snapshot_id, r2.snapshot_id, "newest survives");
    }

    /// Finding 2 (bounded growth): uploads past the byte cap are rejected
    /// with 507 while already-committed snapshots survive.
    #[tokio::test]
    async fn quota_rejects_uploads_over_byte_cap() {
        let env = spawn_agent_with(1024 * 1024, agent::DEFAULT_MAX_SNAPSHOTS, None).await;
        let c = client(&env);

        // A normal (small) fixture snapshot fits under the 1 MiB cap.
        let built = tempfile::tempdir().expect("built");
        let r1 = build_fixture_snapshot(built.path(), 0x44);
        c.push_snapshot(
            &built.path().join(&r1.snapshot_id),
            None,
            &wrapping_for(0x44),
        )
        .await
        .expect("first push fits");
        assert_eq!(c.list_snapshots().await.unwrap().len(), 1);

        // A 2 MiB upload must be rejected at PUT time (507), even though
        // it never commits — an attacker must not fill the disk with
        // in-flight garbage either.
        let resp = reqwest::Client::new()
            .put(format!(
                "{}/v1/snapshots/snap-flood-0001/file?path=payload/f000000.bin",
                env.base_url
            ))
            .bearer_auth(&env.append)
            .body(vec![0u8; 2 * 1024 * 1024])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 507, "over-cap upload rejected");

        // The committed snapshot is untouched.
        let listed = c.list_snapshots().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].snapshot_id, r1.snapshot_id);
    }

    /// Finding 2: the (N+1)-th COMMIT is rejected when the snapshot-count
    /// cap is reached; existing history survives.
    #[tokio::test]
    async fn quota_rejects_commit_over_count_cap() {
        let env = spawn_agent_with(agent::DEFAULT_MAX_BYTES, 1, None).await;
        let c = client(&env);
        let built = tempfile::tempdir().expect("built");
        let r1 = build_fixture_snapshot(built.path(), 0x55);
        c.push_snapshot(
            &built.path().join(&r1.snapshot_id),
            None,
            &wrapping_for(0x55),
        )
        .await
        .expect("first push commits");

        // Second snapshot: uploads fine, commit must fail with 507.
        let r2 = build_fixture_snapshot(built.path(), 0x55);
        let err = c
            .push_snapshot(
                &built.path().join(&r2.snapshot_id),
                None,
                &wrapping_for(0x55),
            )
            .await
            .expect_err("count cap must reject the second commit");
        assert!(
            err.to_string().contains("507"),
            "expected 507 in error, got: {err}"
        );
        assert_eq!(c.list_snapshots().await.unwrap().len(), 1);
    }

    /// Finding 3a: the target validates the receipt envelope (file count
    /// + byte total) against the files it ACTUALLY received — a truncated
    /// or corrupt push, or a lying receipt, commits to nothing.
    #[tokio::test]
    async fn commit_rejects_receipt_not_matching_uploaded_files() {
        let env = spawn_agent().await;
        let built = tempfile::tempdir().expect("built");
        let receipt = build_fixture_snapshot(built.path(), 0x66);
        let dir = built.path().join(&receipt.snapshot_id);
        let http = reqwest::Client::new();

        // Upload only SOME of the real files (the fixture has more)…
        let first_payload = {
            let mut names: Vec<String> = std::fs::read_dir(dir.join("payload"))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names[0].clone()
        };
        for rel in ["manifest.json.enc", &format!("payload/{first_payload}")] {
            let bytes = std::fs::read(dir.join(rel)).expect("file");
            let resp = http
                .put(format!(
                    "{}/v1/snapshots/{}/file?path={rel}",
                    env.base_url, receipt.snapshot_id
                ))
                .bearer_auth(&env.append)
                .body(bytes)
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 201, "upload {rel}");
        }
        // …so the HONEST receipt must be rejected: its totals describe
        // files the target never received (count/bytes mismatch).
        let resp = http
            .post(format!(
                "{}/v1/snapshots/{}/commit",
                env.base_url, receipt.snapshot_id
            ))
            .bearer_auth(&env.append)
            .json(&receipt)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            400,
            "under-uploaded payload must not commit: {}",
            resp.text().await.unwrap_or_default()
        );

        // A doctored receipt (inflated byte total) is rejected too.
        let mut lying = receipt.clone();
        lying.total_bytes += 1;
        let resp = http
            .post(format!(
                "{}/v1/snapshots/{}/commit",
                env.base_url, receipt.snapshot_id
            ))
            .bearer_auth(&env.append)
            .json(&lying)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "lying receipt rejected");

        // Nothing committed; the target stays clean for a retry.
        assert!(client(&env).list_snapshots().await.unwrap().is_empty());
    }
    /// Per-snapshot locking smoke test (review round 2): many concurrent
    /// PUTs to the same snapshot + a racing commit must either all land
    /// before the freeze or be rejected — the committed tree can never
    /// contain bytes that bypassed the envelope check.
    #[tokio::test]
    async fn concurrent_puts_and_commit_are_serialized() {
        let env = spawn_agent().await;
        let built = tempfile::tempdir().expect("built");
        let receipt = build_fixture_snapshot(built.path(), 0x97);
        let dir = built.path().join(&receipt.snapshot_id);

        // Upload the CAS pieces concurrently: the manifest via the
        // per-snapshot file route, every payload file as a blob (payload
        // files are hash-named in v3, so the name IS the blob key).
        let http = reqwest::Client::new();
        let mut handles = Vec::new();
        let mut blob_hashes = Vec::new();
        for name in std::fs::read_dir(dir.join("payload"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        {
            blob_hashes.push(name.clone());
            let url = format!("{}/v1/blobs/{name}", env.base_url);
            let token = env.append.clone();
            let bytes = std::fs::read(dir.join("payload").join(&name)).unwrap();
            handles.push(tokio::spawn(async move {
                reqwest::Client::new()
                    .put(&url)
                    .bearer_auth(&token)
                    .body(bytes)
                    .send()
                    .await
                    .unwrap()
                    .status()
            }));
        }
        {
            let url = format!(
                "{}/v1/snapshots/{}/file?path=manifest.json.enc",
                env.base_url, receipt.snapshot_id
            );
            let token = env.append.clone();
            let bytes = std::fs::read(dir.join("manifest.json.enc")).unwrap();
            handles.push(tokio::spawn(async move {
                reqwest::Client::new()
                    .put(&url)
                    .bearer_auth(&token)
                    .body(bytes)
                    .send()
                    .await
                    .unwrap()
                    .status()
            }));
        }
        for h in handles {
            assert_eq!(h.await.unwrap(), 201, "every upload lands");
        }

        // Commit after the storm: CAS envelope must be complete.
        let mut body = serde_json::to_value(&receipt).unwrap();
        blob_hashes.sort();
        body["blobs"] = serde_json::json!(blob_hashes);
        let resp = http
            .post(format!(
                "{}/v1/snapshots/{}/commit",
                env.base_url, receipt.snapshot_id
            ))
            .bearer_auth(&env.append)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201, "commit succeeds after concurrent puts");

        // And the committed snapshot pulls + verifies end-to-end.
        let pulled = tempfile::tempdir().expect("pulled");
        let local = client(&env)
            .pull_snapshot(None, pulled.path(), &wrapping_for(0x97))
            .await
            .expect("pull verifies");
        assert!(local.ends_with(&receipt.snapshot_id));
    }
    /// Automatic retention: with KEEP_N=2, committing a 3rd snapshot
    /// prunes the oldest — deletion happens with the TARGET's authority
    /// right after commit; the append credential never deletes anything.
    #[tokio::test]
    async fn auto_prune_keeps_newest_n_after_commit() {
        let env = spawn_agent_with(
            agent::DEFAULT_MAX_BYTES,
            agent::DEFAULT_MAX_SNAPSHOTS,
            Some(2),
        )
        .await;
        let c = client(&env);
        let built = tempfile::tempdir().expect("built");

        let r1 = build_fixture_snapshot(built.path(), 0xA1);
        c.push_snapshot(
            &built.path().join(&r1.snapshot_id),
            None,
            &wrapping_for(0xA1),
        )
        .await
        .expect("push 1");
        let r2 = build_fixture_snapshot(built.path(), 0xA1);
        c.push_snapshot(
            &built.path().join(&r2.snapshot_id),
            None,
            &wrapping_for(0xA1),
        )
        .await
        .expect("push 2");
        // Distinct wrapping keys per snapshot would fail verify — same key
        // (0xA1 salt) keeps artifacts coherent; ids differ per build.
        assert_eq!(c.list_snapshots().await.unwrap().len(), 2);

        // Third commit trips auto-prune: oldest (r1) is deleted, newest 2 stay.
        let r3 = build_fixture_snapshot(built.path(), 0xA1);
        c.push_snapshot(
            &built.path().join(&r3.snapshot_id),
            None,
            &wrapping_for(0xA1),
        )
        .await
        .expect("push 3");
        let listed = c.list_snapshots().await.unwrap();
        assert_eq!(listed.len(), 2, "auto-prune trimmed to newest 2");
        let ids: Vec<&str> = listed.iter().map(|r| r.snapshot_id.as_str()).collect();
        assert!(!ids.contains(&r1.snapshot_id.as_str()), "oldest pruned");
        assert!(ids.contains(&r2.snapshot_id.as_str()));
        assert!(ids.contains(&r3.snapshot_id.as_str()));
    }

    // ── content-addressed blob store (CAS / snapshot format v3) ──────────

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(bytes);
        hex::encode(h.finalize())
    }

    async fn put_blob(env: &Env, hash: &str, body: Vec<u8>) -> reqwest::StatusCode {
        reqwest::Client::new()
            .put(format!("{}/v1/blobs/{hash}", env.base_url))
            .bearer_auth(&env.append)
            .body(body)
            .send()
            .await
            .unwrap()
            .status()
    }

    async fn head_blob(env: &Env, hash: &str) -> reqwest::StatusCode {
        reqwest::Client::new()
            .head(format!("{}/v1/blobs/{hash}", env.base_url))
            .bearer_auth(&env.append)
            .send()
            .await
            .unwrap()
            .status()
    }

    fn blob_on_disk(env: &Env, hash: &str) -> bool {
        env._root
            .path()
            .join(agent::BLOBS_DIR)
            .join(&hash[..2])
            .join(hash)
            .is_file()
    }

    /// A fabricated v3 receipt for CAS commit tests — the agent validates
    /// id match + caps only (it is key-free by design).
    fn cas_receipt(id: &str, blobs: &[String]) -> serde_json::Value {
        serde_json::json!({
            "snapshot_id": id,
            "version": 3,
            "created_at": chrono::Utc::now().to_rfc3339(),
            "file_count": blobs.len() as u64,
            "total_bytes": 42u64 * blobs.len() as u64,
            "recording_count": 0u64,
            "hmac_tag": "00".repeat(32),
            "blobs": blobs,
        })
    }

    async fn put_manifest_file(env: &Env, id: &str) {
        let status = reqwest::Client::new()
            .put(format!(
                "{}/v1/snapshots/{id}/file?path=manifest.json.enc",
                env.base_url
            ))
            .bearer_auth(&env.append)
            .body(b"opaque-encrypted-manifest-bytes".to_vec())
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 201, "manifest upload");
    }

    async fn commit_cas(env: &Env, body: &serde_json::Value) -> reqwest::StatusCode {
        reqwest::Client::new()
            .post(format!(
                "{}/v1/snapshots/{}/commit",
                env.base_url,
                body["snapshot_id"].as_str().unwrap()
            ))
            .bearer_auth(&env.append)
            .json(body)
            .send()
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn blob_put_head_get_roundtrip_is_write_once() {
        let env = spawn_agent().await;
        let content = b"ciphertext-bytes-of-a-recording".to_vec();
        let hash = sha256_hex(&content);

        // Unknown before upload.
        assert_eq!(head_blob(&env, &hash).await, 404);
        // Upload: streamed, hash-validated, created.
        assert_eq!(put_blob(&env, &hash, content.clone()).await, 201);
        assert_eq!(head_blob(&env, &hash).await, 204);
        // Write-once: the same blob PUTs as a no-op (200), never a rewrite.
        assert_eq!(put_blob(&env, &hash, content.clone()).await, 200);
        // "Overwriting" an existing blob with DIFFERENT bytes is a no-op
        // too — the stored bytes win, the body is never even hashed.
        assert_eq!(
            put_blob(&env, &hash, b"tampered-different-bytes".to_vec()).await,
            200,
            "existing blob is never rewritten"
        );
        // The anti-poisoning guard applies to NEW keys: bytes that don't
        // hash to the claimed key are rejected and nothing is stored.
        let fresh = sha256_hex(b"a-fresh-blob-key");
        assert_eq!(
            put_blob(&env, &fresh, b"bytes-that-do-not-match".to_vec()).await,
            400,
            "hash mismatch must be rejected for a new blob"
        );
        assert!(!blob_on_disk(&env, &fresh), "rejected blob leaves no file");
        // GET returns exactly the stored bytes.
        let bytes = reqwest::Client::new()
            .get(format!("{}/v1/blobs/{hash}", env.base_url))
            .bearer_auth(&env.append)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(&bytes[..], &content[..]);
        // A malformed key is rejected outright.
        assert_eq!(put_blob(&env, "not-a-hash", content.clone()).await, 400);
    }

    #[tokio::test]
    async fn cas_commit_requires_every_referenced_blob() {
        let env = spawn_agent().await;
        let content = b"some-blob".to_vec();
        let present = sha256_hex(&content);
        let missing = sha256_hex(b"never-uploaded");
        assert_eq!(put_blob(&env, &present, content).await, 201);
        put_manifest_file(&env, "snap-cas-1").await;

        // A referenced-but-absent blob fails the commit; nothing lands.
        let body = cas_receipt("snap-cas-1", &[present.clone(), missing.clone()]);
        assert_eq!(commit_cas(&env, &body).await, 400);
        assert!(!env._root.path().join("snap-cas-1/.committed").exists());

        // With every blob present the commit succeeds and lays down the
        // plaintext reference index.
        let body = cas_receipt("snap-cas-1", std::slice::from_ref(&present));
        assert_eq!(commit_cas(&env, &body).await, 201);
        let idx: Vec<String> = serde_json::from_str(
            &std::fs::read_to_string(
                env._root
                    .path()
                    .join("snap-cas-1")
                    .join(agent::BLOBS_IDX_FILE),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(idx, vec![present]);
    }

    #[tokio::test]
    async fn blob_gc_keeps_referenced_blobs_and_removes_orphans() {
        // Two CAS snapshots sharing one blob; prune to the newest — the
        // shared blob survives (still referenced), the oldest snapshot's
        // private blob is garbage-collected.
        let env = spawn_agent().await;
        let shared = sha256_hex(b"shared-blob");
        let old_private = sha256_hex(b"old-private-blob");
        let new_private = sha256_hex(b"new-private-blob");
        for (hash, body) in [
            (&shared, b"shared-blob".to_vec()),
            (&old_private, b"old-private-blob".to_vec()),
            (&new_private, b"new-private-blob".to_vec()),
        ] {
            assert_eq!(put_blob(&env, hash, body).await, 201);
        }

        put_manifest_file(&env, "snap-old").await;
        let old_receipt = cas_receipt("snap-old", &[shared.clone(), old_private.clone()]);
        assert_eq!(commit_cas(&env, &old_receipt).await, 201);
        // Age the first snapshot so prune's newest-first ordering is
        // unambiguous even within the same second.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        put_manifest_file(&env, "snap-new").await;
        let new_receipt = cas_receipt("snap-new", &[shared.clone(), new_private.clone()]);
        assert_eq!(commit_cas(&env, &new_receipt).await, 201);

        // Admin prune to the newest 1 → oldest snapshot + its orphan blob
        // go; the shared blob stays.
        let status = reqwest::Client::new()
            .post(format!("{}/v1/admin/prune?keep=1", env.base_url))
            .bearer_auth(&env.admin)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 200);
        assert!(
            !env._root.path().join("snap-old").exists(),
            "old snapshot pruned"
        );
        assert!(blob_on_disk(&env, &shared), "shared blob survives GC");
        assert!(
            !blob_on_disk(&env, &old_private),
            "orphaned blob removed by GC"
        );
        assert!(blob_on_disk(&env, &new_private));
    }

    /// Acceptance criterion 2: a second backup with no new recordings
    /// re-uploads ZERO recording blobs — every recording HEAD says
    /// present; only the always-new DB + wrapped-key blobs transfer.
    #[tokio::test]
    async fn second_cas_push_uploads_no_recording_blobs() {
        let env = spawn_agent().await;
        let c = client(&env);
        let built = tempfile::tempdir().expect("built");

        let (r1, recordings) =
            build_fixture_snapshot_staged(built.path(), 0xB1, snapshot::StagingMode::Stream, None);
        let (s1, stats1) = c
            .push_snapshot(
                &built.path().join(&r1.snapshot_id),
                Some(&recordings),
                &wrapping_for(0xB1),
            )
            .await
            .expect("push 1");
        assert_eq!(s1.snapshot_id, r1.snapshot_id);
        // First push carries everything: recording blob + DB + wrapped key.
        assert_eq!(stats1.uploaded, 3, "initial full: all blobs new");
        assert_eq!(stats1.skipped, 0);

        // Same sources, second build: the recording blob is byte-identical
        // (immutable ciphertext) so its hash — and the HEAD — match.
        let (r2, _) = build_fixture_snapshot_staged(
            built.path(),
            0xB1,
            snapshot::StagingMode::Stream,
            Some(&recordings),
        );
        let (_, stats2) = c
            .push_snapshot(
                &built.path().join(&r2.snapshot_id),
                Some(&recordings),
                &wrapping_for(0xB1),
            )
            .await
            .expect("push 2");
        assert_eq!(
            stats2.skipped, 1,
            "the recording blob is already on the target"
        );
        assert_eq!(
            stats2.uploaded, 2,
            "only the always-new DB + wrapped-key blobs transfer"
        );

        // And the pulled copy verifies end-to-end (R5).
        let pulled = tempfile::tempdir().expect("pulled");
        let local = c
            .pull_snapshot(Some(&r2.snapshot_id), pulled.path(), &wrapping_for(0xB1))
            .await
            .expect("pull verifies");
        assert!(local.ends_with(&r2.snapshot_id));
    }

    /// Acceptance criterion 3: one new recording → exactly that
    /// recording's blob uploads, plus the always-new small blobs.
    #[tokio::test]
    async fn cas_push_with_one_new_recording_uploads_only_its_blob() {
        let env = spawn_agent().await;
        let c = client(&env);
        let built = tempfile::tempdir().expect("built");

        let (r1, recordings) =
            build_fixture_snapshot_staged(built.path(), 0xB2, snapshot::StagingMode::Stream, None);
        c.push_snapshot(
            &built.path().join(&r1.snapshot_id),
            Some(&recordings),
            &wrapping_for(0xB2),
        )
        .await
        .expect("push 1");

        // A new recording lands between backups.
        let wav_key = medical_security::file_crypto::derive_file_key(&[0xB2u8; 32]);
        let blob = medical_security::file_crypto::encrypt_bytes_with_key(
            &wav_key,
            b"RIFF a brand new patient recording",
        )
        .unwrap();
        std::fs::write(recordings.join("r1.enc"), blob).unwrap();

        let (r2, _) = build_fixture_snapshot_staged(
            built.path(),
            0xB2,
            snapshot::StagingMode::Stream,
            Some(&recordings),
        );
        let (_, stats2) = c
            .push_snapshot(
                &built.path().join(&r2.snapshot_id),
                Some(&recordings),
                &wrapping_for(0xB2),
            )
            .await
            .expect("push 2");
        // New: the new recording + fresh DB + fresh wrapped key.
        assert_eq!(stats2.uploaded, 3, "one new recording blob + DB + key");
        // Old recording: skipped.
        assert_eq!(stats2.skipped, 1);
    }

    /// The full scheduled unit over CAS, exactly as launchd / the app's
    /// "Back up now" run it: build (Stream) -> push missing blobs ->
    /// commit -> re-pull -> drill -> status. Two consecutive runs — the
    /// second exercises the incremental path end-to-end.
    #[tokio::test]
    async fn job_with_target_runs_end_to_end_over_cas() {
        use job::{BackupTarget, JobConfig};

        let env = spawn_agent().await;
        let data = tempfile::tempdir().expect("data");
        let db_key = [0xC4u8; 32];
        let wrapping = [0xC5u8; 32];

        // Fixture DB + one encrypted recording (mirrors job.rs's fixture).
        let db_path = data.path().join("medical.db");
        {
            let database = medical_db::Database::open(&db_path, Some(db_key)).unwrap();
            let conn = database.conn().unwrap();
            medical_db::recordings::RecordingsRepo::insert(
                &conn,
                &Recording::new("a.enc".to_string(), data.path().join("a.enc")),
            )
            .unwrap();
        }
        let recordings = data.path().join("recordings");
        std::fs::create_dir_all(&recordings).unwrap();
        let wav_key = medical_security::file_crypto::derive_file_key(&db_key);
        std::fs::write(
            recordings.join("a.enc"),
            medical_security::file_crypto::encrypt_bytes_with_key(&wav_key, b"RIFF audio").unwrap(),
        )
        .unwrap();

        for run in 1..=2 {
            let cfg = JobConfig {
                data_dir: data.path().to_path_buf(),
                db_path: db_path.clone(),
                recordings_dir: recordings.clone(),
                keystore_path: None,
                target: Some(BackupTarget {
                    url: env.base_url.clone(),
                    token: env.append.clone(),
                }),
                keep_local: 1,
            };
            let outcome =
                tokio::task::spawn_blocking(move || job::run_backup_job(&cfg, db_key, wrapping))
                    .await
                    .expect("job task");
            assert!(
                outcome.success(),
                "run {run} failed: {:?} - {:?}",
                outcome.status.failure,
                outcome.events
            );
            assert!(outcome.status.drill_passed, "run {run} drilled");
            assert_eq!(
                outcome.status.pushed_to.as_deref(),
                Some(env.base_url.as_str())
            );
        }

        // Both runs succeeded; local retention kept exactly one snapshot
        // and the target holds both committed snapshots.
        let backups = data.path().join("backups");
        let dirs: Vec<_> = std::fs::read_dir(&backups)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        assert_eq!(dirs.len(), 1, "keep_local=1: exactly one local snapshot");
        assert_eq!(client(&env).list_snapshots().await.unwrap().len(), 2);
    }
}
