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

    async fn spawn_agent_with(max_bytes: u64, max_snapshots: usize) -> Env {
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
        spawn_agent_with(agent::DEFAULT_MAX_BYTES, agent::DEFAULT_MAX_SNAPSHOTS).await
    }

    fn build_fixture_snapshot(dest: &Path, salt: u8) -> SnapshotReceipt {
        let db_key = [salt; 32];
        let wrapping = [salt ^ 0xFF; 32];
        let src = tempfile::tempdir().expect("src");
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
        let recordings = src.path().join("recordings");
        std::fs::create_dir_all(&recordings).unwrap();
        let wav_key = medical_security::file_crypto::derive_file_key(&db_key);
        let blob = medical_security::file_crypto::encrypt_bytes_with_key(
            &wav_key,
            format!("RIFF patient audio {salt}").as_bytes(),
        )
        .unwrap();
        std::fs::write(recordings.join("r0.enc"), blob).unwrap();

        build_snapshot(&BuildOptions {
            db_path,
            recordings_dir: recordings,
            keystore_path: None,
            dest_dir: dest.to_path_buf(),
            db_key,
            wrapping_key: wrapping,
        })
        .expect("build snapshot")
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

        let pushed = client(&env)
            .push_snapshot(&built.path().join(&receipt.snapshot_id))
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
        c.push_snapshot(&built.path().join(&r1.snapshot_id))
            .await
            .expect("push 1");

        let id = r1.snapshot_id.clone();

        // 1. Re-committing an existing id → 409.
        let resp = reqwest::Client::new()
            .post(format!("{}/v1/snapshots/{id}/commit", env.base_url))
            .bearer_auth(&env.append)
            .json(&r1)
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
        c.push_snapshot(&built.path().join(&r1.snapshot_id))
            .await
            .unwrap();
        // Second snapshot with a distinct id (new timestamp/rand suffix).
        let r2 = build_fixture_snapshot(built.path(), 0x33);
        c.push_snapshot(&built.path().join(&r2.snapshot_id))
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
        let env = spawn_agent_with(1024 * 1024, agent::DEFAULT_MAX_SNAPSHOTS).await;
        let c = client(&env);

        // A normal (small) fixture snapshot fits under the 1 MiB cap.
        let built = tempfile::tempdir().expect("built");
        let r1 = build_fixture_snapshot(built.path(), 0x44);
        c.push_snapshot(&built.path().join(&r1.snapshot_id))
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
        let env = spawn_agent_with(agent::DEFAULT_MAX_BYTES, 1).await;
        let c = client(&env);
        let built = tempfile::tempdir().expect("built");
        let r1 = build_fixture_snapshot(built.path(), 0x55);
        c.push_snapshot(&built.path().join(&r1.snapshot_id))
            .await
            .expect("first push commits");

        // Second snapshot: uploads fine, commit must fail with 507.
        let r2 = build_fixture_snapshot(built.path(), 0x55);
        let err = c
            .push_snapshot(&built.path().join(&r2.snapshot_id))
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
        for rel in ["manifest.json.enc", "payload/f000000.bin"] {
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
}
