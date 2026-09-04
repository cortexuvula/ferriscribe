//! Boot-time and periodic maintenance sweeps, extracted from
//! `AppState::initialize` so they are unit-testable against an in-memory
//! database.
//!
//! All sweeps are best-effort — a failure logs a warning and never blocks
//! boot — and PHI-safe: tracing carries counts and IDs only, never
//! transcript/SOAP content or file contents.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use medical_db::Database;
use medical_db::recordings::RecordingsRepo;
use tracing::info;

/// Flip any recordings still marked Processing from the previous session
/// (crash, hard-quit, SIGKILL mid-pipeline) to Failed so the UI doesn't
/// show them spinning forever.
pub fn fail_stuck_processing_sweep(db: &Database) {
    if let Ok(conn) = db.conn() {
        match RecordingsRepo::fail_stuck_processing(
            &conn,
            "Processing interrupted — app was closed before the pipeline finished.",
        ) {
            Ok(0) => {}
            Ok(n) => info!("Marked {n} stuck Processing recording(s) as Failed on boot"),
            Err(e) => tracing::warn!("fail_stuck_processing on boot failed: {e}"),
        }
    }
}

/// Sweep: encrypt any recordings left pending by a crash. A row is flagged
/// `encryption_pending=1` by `stop_recording` right before it spawns the
/// background encrypt task; the task clears the flag when done. If the app
/// died in between, the WAV is still plaintext at rest — finish the
/// encryption here so no PHI audio is left unencrypted.
pub fn encryption_pending_sweep(db: &Database) {
    if let Ok(conn) = db.conn() {
        let pending = match RecordingsRepo::list_encryption_pending(&conn) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "encryption sweep: list_encryption_pending failed");
                Vec::new()
            }
        };
        if pending.is_empty() {
            return;
        }
        info!(
            count = pending.len(),
            "Encrypting pending recordings from previous session"
        );
        for (id, path) in &pending {
            // Guard against the crash-after-encrypt-but-before-clear-flag
            // window: if the file is already encrypted on disk (FE1 magic),
            // just clear the flag instead of re-encrypting — re-encrypting
            // ciphertext would corrupt the file.
            if medical_security::file_crypto::is_encrypted(Path::new(path)) {
                let _ = RecordingsRepo::set_encryption_done(&conn, id);
                tracing::debug!(
                    recording_id = %id,
                    "Pending recording already encrypted on disk; cleared flag"
                );
                continue;
            }
            match medical_security::file_crypto::encrypt_file_in_place(Path::new(path)) {
                Ok(()) => {
                    let _ = RecordingsRepo::set_encryption_done(&conn, id);
                    tracing::debug!(recording_id = %id, "Encrypted pending recording");
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        recording_id = %id,
                        "Failed to encrypt pending recording"
                    );
                }
            }
        }
    }
}

/// Sweep: encrypt any WAV in the recordings dir with NO database row.
///
/// The capture path creates the WAV the moment recording starts, but the
/// DB row (and its `encryption_pending` flag) only exists after
/// `stop_recording` — a crash or hard-quit mid-recording leaves a
/// plaintext PHI file that `encryption_pending_sweep` can never see (it
/// enumerates flagged ROWS). This sweep closes that window: every `.wav`
/// in the recordings dir whose filename doesn't match any row's stored
/// audio path gets encrypted in place.
///
/// Age guard: files modified in the last 10 minutes are skipped — they
/// may belong to a recording in progress (its row doesn't exist yet
/// either). They'll be picked up on the NEXT boot.
///
/// No row is ever created for these orphans: the recording was never
/// finalized, so there is no duration/transcript to show — encrypting
/// at rest (instead of deleting) preserves the audio for manual
/// recovery. PHI-safe: logs carry counts only.
pub fn orphaned_wav_sweep(db: &Database, recordings_dir: &Path) {
    let dir = match std::fs::read_dir(recordings_dir) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "orphan wav sweep: cannot read recordings dir");
            return;
        }
    };

    // Collect the audio paths the DB knows about (basename compare — the
    // stored paths may be absolute while we list the dir directly).
    let known: std::collections::HashSet<String> = {
        let conn = match db.conn() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "orphan wav sweep: cannot open DB");
                return;
            }
        };
        match conn
            .prepare("SELECT audio_path FROM recordings")
            .and_then(|mut stmt| {
                let mut out = std::collections::HashSet::new();
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                for path in rows.flatten() {
                    if let Some(name) = std::path::Path::new(&path).file_name() {
                        out.insert(name.to_string_lossy().into_owned());
                    }
                }
                Ok(out)
            }) {
            Ok(set) => set,
            Err(e) => {
                tracing::warn!(error = %e, "orphan wav sweep: audio_path query failed");
                return;
            }
        }
    };

    let now = std::time::SystemTime::now();
    let mut encrypted = 0usize;
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wav") {
            continue;
        }
        let name = match path.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => continue,
        };
        if known.contains(&name) {
            continue; // row exists — encryption_pending_sweep owns it
        }
        // Age guard: skip possibly-in-progress captures.
        let mtime = entry.metadata().and_then(|m| m.modified()).ok();
        if let Some(t) = mtime
            && now.duration_since(t).unwrap_or(Duration::ZERO) < Duration::from_secs(600)
        {
            continue;
        }
        // Already encrypted (FE1 magic)? Nothing to do.
        if medical_security::file_crypto::is_encrypted(&path) {
            continue;
        }
        match medical_security::file_crypto::encrypt_file_in_place(&path) {
            Ok(()) => encrypted += 1,
            Err(e) => tracing::warn!(error = %e, "orphan wav sweep: encrypt failed"),
        }
    }
    if encrypted > 0 {
        info!(count = encrypted, "Encrypted orphaned WAVs with no DB row");
    }
}

/// One tick of the daily retention sweeper. Two idempotent, PHI-safe phases
/// (logs carry counts/ids only):
///
/// 1. Tombstone purge (server only): permanently delete recordings
///    soft-deleted >30 days ago, after cleaning up their RAG vectors and
///    audio files. Server-only because durable deletion is the server's
///    policy; clients keep their soft-deletes for local undo.
/// 2. Retention sweep (per-machine): if the clinician configured a
///    retention window, move older visible recordings into the trash
///    (from which phase 1 will eventually purge them on the server).
///
/// `is_server` is a parameter (not re-read from disk here) so tests can
/// exercise both roles without touching the on-disk server config; the
/// spawned loop in [`spawn_retention_sweeper`] re-reads it every tick so a
/// machine that starts acting as the server mid-session is picked up
/// without a restart.
pub fn retention_sweep_tick(db: &Database, is_server: bool) {
    let Ok(conn) = db.conn() else {
        return;
    };

    // ── Phase 1: tombstone purge (server only) ─────────────────────────
    if is_server {
        // Get the IDs of recordings about to be purged, so we can also
        // clean up their RAG vectors.
        let to_purge =
            match RecordingsRepo::list_soft_deleted_older_than(&conn, 30, chrono::Utc::now()) {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::warn!(error = %e, "tombstone sweeper: list failed");
                    Vec::new()
                }
            };

        if !to_purge.is_empty() {
            // Clean up RAG vectors for each purged recording.
            use medical_db::vectors::VectorsRepo;
            for (id, _audio_path) in &to_purge {
                if let Err(e) = VectorsRepo::delete_by_document(&conn, &id.to_string()) {
                    tracing::warn!(
                        recording_id = %id,
                        error = %e,
                        "tombstone sweeper: failed to delete RAG vectors"
                    );
                }
            }

            // Best-effort delete of audio files. Tolerate missing files.
            for (id, audio_path) in &to_purge {
                if !audio_path.is_empty()
                    && let Err(e) = std::fs::remove_file(audio_path)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(recording_id = %id, error = %e, "tombstone sweeper: failed to delete audio file");
                }
            }

            // Now permanently delete the recording rows. This must go
            // through the repo: a raw DELETE fires the FTS delete-trigger
            // against rows that soft_delete already de-indexed, which fails
            // with SQLITE_CORRUPT — the reason the 30-day durable-deletion
            // policy never actually deleted rows before this fix.
            //
            // The ledger variant records each purged id in
            // `purged_recordings` inside the same transaction, so
            // `merge_incoming` can later refuse stale copies of these
            // recordings pushed by machines that missed the deletion.
            // Id + timestamp only — no PHI.
            let ids: Vec<uuid::Uuid> = to_purge.iter().map(|(id, _)| *id).collect();
            match RecordingsRepo::purge_soft_deleted_with_ledger(&conn, &ids) {
                Ok(purged) => {
                    tracing::info!(
                        purged = purged.len(),
                        vectors_cleaned = to_purge.len(),
                        ledger_count = purged.len(),
                        "tombstone sweeper purged soft-deleted recordings + RAG vectors + audio files"
                    );
                }
                Err(e) => tracing::warn!(error = %e, "tombstone sweeper failed"),
            }
        }
    }

    // ── Phase 2: per-machine retention sweep ───────────────────────────
    // Runs on every machine (server or client) — it only moves old visible
    // recordings into the trash.
    match medical_db::settings::SettingsRepo::load_config(&conn) {
        Ok(cfg) => {
            if let Some(days) = cfg.retention_days.filter(|d| *d > 0) {
                match RecordingsRepo::retention_soft_delete_older_than(
                    &conn,
                    days,
                    chrono::Utc::now(),
                ) {
                    Ok(trashed) if !trashed.is_empty() => {
                        tracing::info!(
                            count = trashed.len(),
                            "retention sweep: moved recordings to trash"
                        );
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "retention sweep failed"),
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "retention sweep: failed to load settings"),
    }
}

/// Spawn the periodic sweeper: first tick 5 minutes after boot, then daily.
/// Machines that are powered off overnight (most clinician laptops) never
/// accumulate 24h of uptime, so sleeping a full day BEFORE the first tick
/// meant the retention sweep never fired on them at all; the short initial
/// delay catches every launch while keeping the daily cadence afterwards.
pub fn spawn_retention_sweeper(db: Arc<Database>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(300)).await;
        loop {
            tracing::info!("running tombstone sweeper");
            let is_server = crate::state::load_server_config().is_some();
            retention_sweep_tick(&db, is_server);
            // Daily cadence between sweeps after the boot-time first tick
            // (the 30-day window dwarfs the interval).
            tokio::time::sleep(Duration::from_secs(86400)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::recording::{ProcessingStatus, Recording};
    use medical_core::types::settings::AppConfig;
    use medical_db::settings::SettingsRepo;
    use uuid::Uuid;

    /// Insert a recording whose `created_at` is `days` days in the past.
    /// The connection must be scoped and dropped by the caller before any
    /// sweep runs: the in-memory pool is `max_size=1`, so a second
    /// concurrent checkout (the sweep's own `db.conn()`) would block until
    /// timeout and the sweep would silently no-op.
    fn seed_days_old(
        conn: &rusqlite::Connection,
        days: i64,
        filename: &str,
        audio_path: std::path::PathBuf,
    ) -> Recording {
        let mut rec = Recording::new(filename, audio_path);
        rec.created_at = chrono::Utc::now() - chrono::TimeDelta::days(days);
        RecordingsRepo::insert(conn, &rec).expect("insert fixture recording");
        rec
    }

    fn set_retention_days(conn: &rusqlite::Connection, days: Option<u32>) {
        let mut cfg = AppConfig::default();
        cfg.retention_days = days;
        SettingsRepo::save_config(conn, &cfg).expect("save config");
    }

    fn deleted_at_raw(conn: &rusqlite::Connection, id: Uuid) -> Option<String> {
        conn.query_row(
            "SELECT deleted_at FROM recordings WHERE id = ?1",
            [id.to_string()],
            |row| row.get::<_, Option<String>>(0),
        )
        .expect("query deleted_at")
    }

    fn row_exists(conn: &rusqlite::Connection, id: Uuid) -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM recordings WHERE id = ?1",
            [id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .expect("count rows")
            > 0
    }

    #[test]
    fn retention_sweep_client_trashes_old_but_never_purges() {
        let db = Database::open_in_memory().expect("db");
        let (old, fresh) = {
            let conn = db.conn().expect("conn");
            set_retention_days(&conn, Some(90));
            let old = seed_days_old(&conn, 100, "old-visit.wav", "/audio/old.wav".into());
            let fresh = seed_days_old(&conn, 10, "fresh-visit.wav", "/audio/fresh.wav".into());
            (old, fresh)
        };

        // Client tick: phase 2 only (is_server = false).
        retention_sweep_tick(&db, false);

        let conn = db.conn().expect("conn");
        assert!(
            deleted_at_raw(&conn, old.id).is_some(),
            "old recording trashed"
        );
        assert!(
            deleted_at_raw(&conn, fresh.id).is_none(),
            "fresh recording untouched"
        );
        assert!(row_exists(&conn, old.id), "clients never purge rows");
        let ledger_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM purged_recordings", [], |row| {
                row.get(0)
            })
            .expect("count ledger");
        assert_eq!(ledger_rows, 0, "clients never write the purge ledger");
    }

    #[test]
    fn retention_sweep_server_purges_old_tombstones_and_audio() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let audio_path = tmp.path().join("purged-visit.wav");
        std::fs::write(&audio_path, b"RIFF fake wav bytes").expect("write audio");

        let db = Database::open_in_memory().expect("db");
        let (rec, visible) = {
            let conn = db.conn().expect("conn");
            set_retention_days(&conn, None); // phase 2 disabled; phase 1 only
            let rec = seed_days_old(&conn, 100, "purged-visit.wav", audio_path.clone());
            let visible = seed_days_old(&conn, 100, "kept-visit.wav", "/audio/kept.wav".into());
            // Seed an aged tombstone with a single UPDATE on the still-
            // visible row (the same statement shape `soft_delete` uses) —
            // updating `deleted_at` again AFTER soft_delete fires the FTS
            // trigger against an already de-indexed row and fails with
            // SQLITE_CORRUPT.
            let past = (chrono::Utc::now() - chrono::TimeDelta::days(40))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            conn.execute(
                "UPDATE recordings SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
                rusqlite::params![past, rec.id.to_string()],
            )
            .expect("seed aged tombstone");
            (rec, visible)
        };

        retention_sweep_tick(&db, true);

        let conn = db.conn().expect("conn");
        assert!(!row_exists(&conn, rec.id), "old tombstone purged");
        assert!(
            !audio_path.exists(),
            "audio file removed with the purged row"
        );
        assert!(row_exists(&conn, visible.id), "visible row kept");
        // The purge was ledgered so a stale peer copy can't resurrect it;
        // the kept visible row must NOT be ledgered.
        let ledger_at: Option<String> = conn
            .query_row(
                "SELECT purged_at FROM purged_recordings WHERE id = ?1",
                [rec.id.to_string()],
                |row| row.get(0),
            )
            .expect("query ledger");
        assert!(ledger_at.is_some(), "server purge must write the ledger");
        let ledgered_visible: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM purged_recordings WHERE id = ?1",
                [visible.id.to_string()],
                |row| row.get(0),
            )
            .expect("count ledger for kept row");
        assert_eq!(ledgered_visible, 0, "kept rows are never ledgered");
    }

    #[test]
    fn retention_sweep_without_window_trashes_nothing() {
        let db = Database::open_in_memory().expect("db");
        let old = {
            let conn = db.conn().expect("conn");
            set_retention_days(&conn, None);
            seed_days_old(&conn, 400, "ancient-visit.wav", "/audio/ancient.wav".into())
        };

        retention_sweep_tick(&db, false);

        let conn = db.conn().expect("conn");
        assert!(
            deleted_at_raw(&conn, old.id).is_none(),
            "no retention window configured — nothing trashed"
        );
    }

    #[test]
    fn stuck_processing_recording_is_marked_failed() {
        let db = Database::open_in_memory().expect("db");
        let rec = {
            let conn = db.conn().expect("conn");
            let rec = seed_days_old(&conn, 0, "stuck.wav", "/audio/stuck.wav".into());
            // Seed a Processing status directly into the JSON column (the
            // same shape `fail_stuck_processing` matches on).
            let status_json = serde_json::to_string(&ProcessingStatus::Processing {
                started_at: chrono::Utc::now(),
            })
            .expect("serialize status");
            conn.execute(
                "UPDATE recordings SET processing_status = ?1 WHERE id = ?2",
                rusqlite::params![status_json, rec.id.to_string()],
            )
            .expect("set Processing");
            rec
        };

        fail_stuck_processing_sweep(&db);

        let conn = db.conn().expect("conn");
        let after = RecordingsRepo::get_by_id(&conn, &rec.id).expect("reload");
        assert!(
            matches!(after.status, ProcessingStatus::Failed { .. }),
            "stuck recording flipped to Failed, got {:?}",
            after.status
        );
    }

    #[test]
    fn encryption_sweep_clears_flag_when_file_already_encrypted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // FE1 magic + junk: `is_encrypted` only inspects the prefix, so no
        // key material is needed to exercise the already-encrypted path.
        let audio_path = tmp.path().join("already-enc.wav");
        std::fs::write(&audio_path, b"FE1\x00\x01\x02ciphertext-bytes").expect("write fake enc");

        let db = Database::open_in_memory().expect("db");
        {
            let conn = db.conn().expect("conn");
            let rec = seed_days_old(&conn, 0, "pending.wav", audio_path.clone());
            conn.execute(
                "UPDATE recordings SET encryption_pending = 1 WHERE id = ?1",
                rusqlite::params![rec.id.to_string()],
            )
            .expect("flag pending");
        }

        encryption_pending_sweep(&db);

        let conn = db.conn().expect("conn");
        let pending = RecordingsRepo::list_encryption_pending(&conn).expect("list pending");
        assert!(pending.is_empty(), "flag cleared without re-encrypting");
    }

    /// Build a WAV fixture with a backdated mtime so the age guard lets
    /// the sweep see it.
    fn write_aged_wav(dir: &Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"RIFF fake plaintext wav bytes").expect("write wav");
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let ft = filetime::FileTime::from_system_time(past);
        filetime::set_file_mtime(&path, ft).expect("backdate mtime");
        path
    }

    // Mid-recording crash: the WAV exists, no DB row does — the file is
    // plaintext PHI invisible to encryption_pending_sweep (which only
    // enumerates flagged rows).
    //
    // The encryption assertion is conditional on crypto being AVAILABLE:
    // headless CI has no OS keyring, so `encrypt_file_in_place` fails with
    // a keychain error and the sweep (best-effort by design, like
    // encryption_pending_sweep in the same environment) leaves the file
    // plaintext. There we assert the weaker invariant — the sweep ran
    // without panicking and didn't delete or corrupt the orphan.
    #[test]
    fn orphaned_wav_sweep_encrypts_rowless_wavs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let orphan = write_aged_wav(tmp.path(), "crash-mid-recording.wav");

        let db = Database::open_in_memory().expect("db");

        // Probe the keychain through the hang guard FIRST: the probe, the
        // sweep, or both would otherwise sit forever on a securityd access
        // prompt the harness can't dismiss (this exact test was one of the
        // three that hung the workspace gate for ~35 min). When the probe
        // times out, don't even run the sweep — its internal encrypt would
        // block on the same call.
        let probe = crate::testutil::with_keychain_guard(|| {
            let probe_dir = tempfile::tempdir().expect("probe dir");
            let scratch = probe_dir.path().join("probe.txt");
            std::fs::write(&scratch, b"probe").expect("probe");
            medical_security::file_crypto::encrypt_file_in_place(&scratch)
        });
        let Some(probe_result) = probe else {
            assert!(
                orphan.exists(),
                "sweep must never delete the orphan, even when skipped"
            );
            return;
        };
        let crypto_available = probe_result.is_ok();

        orphaned_wav_sweep(&db, tmp.path());

        if crypto_available {
            assert!(
                medical_security::file_crypto::is_encrypted(&orphan),
                "row-less WAV must be encrypted at rest when crypto is available"
            );
        } else {
            assert!(
                orphan.exists(),
                "sweep must never delete the orphan, even when it can't encrypt"
            );
        }
    }

    #[test]
    fn orphaned_wav_sweep_leaves_known_and_fresh_files_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // Known: a row references this file (encryption_pending_sweep owns it).
        let known = tmp.path().join("has-row.wav");
        std::fs::write(&known, b"RIFF plaintext but owned").expect("write known");

        // Fresh: modified "just now" — could be an in-progress capture.
        let fresh = tmp.path().join("in-progress.wav");
        std::fs::write(&fresh, b"RIFF possibly still recording").expect("write fresh");

        let db = Database::open_in_memory().expect("db");
        {
            let conn = db.conn().expect("conn");
            seed_days_old(&conn, 0, "has-row.wav", known.clone());
        }

        orphaned_wav_sweep(&db, tmp.path());

        assert!(
            !medical_security::file_crypto::is_encrypted(&known),
            "row-backed WAV is the pending-sweep's business, not ours"
        );
        assert!(
            !medical_security::file_crypto::is_encrypted(&fresh),
            "recently-modified WAV may be an active capture — skip it"
        );
    }
}
