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

use medical_db::ContentSyncRepo;
use medical_db::Database;
use medical_db::recordings::RecordingsRepo;
use tracing::info;
use uuid::Uuid;

/// Remove one audio file, shredding plaintext first when needed.
///
/// Encrypted artifacts (FE1-magic `.wav`, sync-transport `.enc`) hold only
/// ciphertext — a plain unlink suffices. Legacy plaintext WAVs are shredded
/// (zero-fill + fsync + unlink, `file_crypto::shred_and_unlink`) so
/// unencrypted PHI isn't left recoverable on disk, matching the
/// screen-capture precedent for discarding plaintext PHI files.
///
/// Returns the underlying io error so callers can classify: `NotFound`
/// means already gone, anything else is retryable via the
/// `pending_file_removals` queue.
fn remove_audio_file(path: &str) -> std::io::Result<()> {
    let path = Path::new(path);
    if medical_security::file_crypto::is_encrypted(path) {
        std::fs::remove_file(path)
    } else {
        medical_security::file_crypto::shred_and_unlink(path)
    }
}

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

/// Sweep: delete stale utterance WAVs from the Translate tab's temp dir.
///
/// Translation utterances are throwaway by design — `capture_stop` deletes
/// the WAV the moment its samples are read — but a crash or hard-quit
/// mid-utterance leaves one behind forever. Unlike the recordings-dir
/// orphans (which get encrypted for possible recovery), these have no DB
/// row, no transcript, and no recovery value, so they are shredded and
/// unlinked (plaintext PHI — never a plain remove_file).
///
/// Age guard: files modified in the last 10 minutes are skipped — they may
/// belong to an in-progress capture on a very fast app restart. They'll be
/// picked up on the NEXT boot. PHI-safe: logs carry counts only.
pub fn translation_wav_sweep(translation_dir: &Path) {
    let dir = match std::fs::read_dir(translation_dir) {
        Ok(d) => d,
        Err(_) => return, // dir doesn't exist yet — nothing ever captured
    };

    let now = std::time::SystemTime::now();
    let mut removed = 0usize;
    for entry in dir.flatten() {
        let path = entry.path();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let name = match name {
            Some(n) => n,
            None => continue,
        };
        // Only our own utterance captures — never touch anything else a
        // user may have put in this directory.
        if !name.starts_with("utterance-")
            || path.extension().and_then(|e| e.to_str()) != Some("wav")
        {
            continue;
        }
        let mtime = entry.metadata().and_then(|m| m.modified()).ok();
        if let Some(t) = mtime
            && now.duration_since(t).unwrap_or(Duration::ZERO) < Duration::from_secs(600)
        {
            continue;
        }
        // Plaintext PHI — shred before unlink (a plain remove_file leaves
        // the audio recoverable on disk); a file raced away between the
        // directory read and here still counts as gone.
        match medical_security::file_crypto::shred_and_unlink(&path) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => removed += 1,
            Err(e) => tracing::warn!(error = %e, "translation wav sweep: delete failed"),
        }
    }
    if removed > 0 {
        info!(count = removed, "Deleted stale translation utterance WAVs");
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
/// 1. Tombstone purge (EVERY machine, not just the server): permanently
///    delete recordings soft-deleted >30 days ago. PURGE-FIRST ordering:
///    the ledger purge transaction runs before any artifact cleanup, and
///    RAG vectors + audio files are removed ONLY for ids the transaction
///    confirmed purged — a restore landing between the listing and the
///    purge leaves its row active, so the purge skips it and its artifacts
///    survive (pinned by `restore_between_listing_and_purge_leaves_row_active_with_audio`).
///    Runs on clients too: standalone machines must honor the 30-day
///    window, and the `purged_recordings` ledger keeps paired machines
///    safe on every side (`merge_incoming` refuses stale copies).
/// 2. Retention sweep (per-machine): if the clinician configured a
///    retention window, move older visible recordings into the trash
///    (from which phase 1 will eventually purge them).
pub fn retention_sweep_tick(db: &Database) {
    let Ok(conn) = db.conn() else {
        return;
    };

    // Retry file removals a previous purge failed to complete (the rows are
    // gone, so this queue is their only recovery path).
    pending_file_removals_sweep(&conn);

    // ── Phase 1: tombstone purge (every machine) ─────────────────────
    let to_purge = match RecordingsRepo::list_soft_deleted_older_than(&conn, 30, chrono::Utc::now())
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "tombstone sweeper: list failed");
            Vec::new()
        }
    };
    if !to_purge.is_empty() {
        purge_tombstone_batch(&conn, &to_purge);
    }

    // ── Phase 2: per-machine retention sweep ───────────────────────────
    // Runs on every machine — it only moves old visible recordings into
    // the trash.
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

/// Purge the given aged-tombstone candidates and clean up their artifacts,
/// purge-first: the `purged_recordings`-ledger transaction runs BEFORE any
/// artifact deletion, and RAG vectors + audio files are removed ONLY for
/// the ids it CONFIRMED purged.
///
/// The candidates come from `list_soft_deleted_older_than` moments earlier;
/// a restore may land in that window. The purge transaction's
/// `deleted_at IS NOT NULL` guard skips such a row (rows=0 → not
/// confirmed), so its audio and vectors are never deleted out from under a
/// live recording. Missing audio files are tolerated. Audio removal itself
/// shreds legacy plaintext WAVs before unlinking (encrypted artifacts get
/// a plain unlink — they hold only ciphertext), and a non-NotFound removal
/// failure enqueues the path in the `pending_file_removals` retry queue:
/// the row is already deleted, so no later tombstone listing can re-see
/// the id.
///
/// Separate from `retention_sweep_tick` so the restore-race test can drive
/// the exact production sequence with a stale listing.
fn purge_tombstone_batch(
    conn: &medical_db::Connection,
    candidates: &[(uuid::Uuid, String)],
) -> Vec<uuid::Uuid> {
    let ids: Vec<uuid::Uuid> = candidates.iter().map(|(id, _)| *id).collect();
    // Permanently delete the rows FIRST. This must go through the repo: a
    // raw DELETE fires the FTS delete-trigger against rows that
    // soft_delete already de-indexed, which fails with SQLITE_CORRUPT.
    // The ledger variant records each purged id in `purged_recordings`
    // inside the same transaction, so `merge_incoming` can later refuse
    // stale copies of these recordings. Id + timestamp only — no PHI.
    let confirmed = match RecordingsRepo::purge_soft_deleted_with_ledger(conn, &ids) {
        Ok(purged) => purged,
        Err(e) => {
            tracing::warn!(error = %e, "tombstone sweeper failed");
            return Vec::new();
        }
    };

    // Artifact cleanup for the CONFIRMED ids only.
    use medical_db::vectors::VectorsRepo;
    let mut audio_removed = 0usize;
    let mut failed_removals: Vec<String> = Vec::new();
    for (id, audio_path) in candidates {
        if !confirmed.contains(id) {
            continue; // restored (or otherwise revived) mid-sweep — not ours
        }
        if let Err(e) = VectorsRepo::delete_by_document(conn, &id.to_string()) {
            tracing::warn!(
                recording_id = %id,
                error = %e,
                "tombstone sweeper: failed to delete RAG vectors"
            );
        }
        if audio_path.is_empty() {
            continue;
        }
        match remove_audio_file(audio_path) {
            Ok(()) => audio_removed += 1,
            // Tolerate missing files (already cleaned up, pulled machine
            // whose audio lives on the partner).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                // The row is already deleted, so no future tombstone
                // listing can re-see this id — enqueue the path for retry
                // by `pending_file_removals_sweep` (boot + every tick).
                tracing::warn!(
                    recording_id = %id,
                    error = %e,
                    "tombstone sweeper: audio removal failed — queued for retry"
                );
                failed_removals.push(audio_path.clone());
            }
        }
    }
    if !failed_removals.is_empty() {
        // Best-effort persistence: a failure here leaves the files in place
        // (warned above) — never aborts the already-committed purge.
        match ContentSyncRepo::get_pending_file_removals(conn) {
            Ok(mut queue) => {
                for path in &failed_removals {
                    if !queue.contains(path) {
                        queue.push(path.clone());
                    }
                }
                if let Err(e) = ContentSyncRepo::set_pending_file_removals(conn, &queue) {
                    tracing::warn!(
                        error = %e,
                        count = failed_removals.len(),
                        "tombstone sweeper: persisting file-removal retry queue failed"
                    );
                }
            }
            Err(e) => tracing::warn!(
                error = %e,
                count = failed_removals.len(),
                "tombstone sweeper: reading file-removal retry queue failed"
            ),
        }
    }
    tracing::info!(
        purged = confirmed.len(),
        audio_removed,
        "tombstone sweeper purged soft-deleted recordings + RAG vectors + audio files"
    );
    confirmed
}

/// Retry audio-file removals queued by a failed purge (see
/// [`purge_tombstone_batch`]). Success or NotFound drops a path — the
/// latter means the file was cleaned up by other means; any other error
/// keeps it for the next tick. Runs at boot and at the top of every
/// retention tick. PHI-safe: logs carry counts only, never paths.
pub fn pending_file_removals_sweep(conn: &medical_db::Connection) {
    let mut queue = match ContentSyncRepo::get_pending_file_removals(conn) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!(error = %e, "file removal retry sweep: queue read failed");
            return;
        }
    };
    if queue.is_empty() {
        return;
    }
    let mut removed = 0usize;
    let before = queue.len();
    queue.retain(|path| match remove_audio_file(path) {
        Ok(()) => {
            removed += 1;
            false
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            removed += 1;
            false
        }
        Err(e) => {
            tracing::warn!(error = %e, "file removal retry failed — kept for next sweep");
            true
        }
    });
    if queue.len() != before
        && let Err(e) = ContentSyncRepo::set_pending_file_removals(conn, &queue)
    {
        tracing::warn!(error = %e, "file removal retry sweep: queue persist failed");
    }
    if removed > 0 {
        info!(count = removed, "Removed previously-failed audio file(s)");
    }
}

/// Delete `.enc` sync artifacts in the recordings dir that no row's
/// `audio_path` references.
///
/// `{uuid}.enc` files are written exclusively by the audio sync paths (the
/// server-side PUT handler and the client-side fetch), each of which sets
/// the row's `audio_path` immediately after — so a rowless `.enc` is a
/// crash leftover (process died between the atomic rename and the DB
/// update) whose row can never reference it again. Unlike the rowless-WAV
/// sweep, which ENCRYPTS for possible manual recovery, these are deleted:
/// they are decryptable PHI (the key is on this machine) that nothing else
/// would ever clean. The audio GET handler resolves via the row's
/// `audio_path`, so a rowless file is never servable — deleting it cannot
/// break a live recording; a live row whose fetch crashed mid-way simply
/// re-fetches.
///
/// Only files whose stem parses as a UUID (our naming convention — never a
/// user-named file) are touched, and only when modified more than 10
/// minutes ago (an in-flight fetch/upload legitimately writes the file
/// before its row update lands). PHI-safe: logs carry counts only.
pub fn orphaned_enc_sweep(db: &Database, recordings_dir: &Path) {
    let dir = match std::fs::read_dir(recordings_dir) {
        Ok(d) => d,
        Err(_) => return, // dir doesn't exist yet — nothing to sweep
    };

    // Basenames every row references (basename compare — stored paths may
    // be absolute while we list the dir directly). Tombstoned rows count
    // too: their audio is the 30-day purge's business, not ours.
    let known: std::collections::HashSet<String> = {
        let conn = match db.conn() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "orphan enc sweep: cannot open DB");
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
                tracing::warn!(error = %e, "orphan enc sweep: audio_path query failed");
                return;
            }
        }
    };

    let now = std::time::SystemTime::now();
    let mut removed = 0usize;
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("enc") {
            continue;
        }
        // Only our own sync artifacts — `{uuid}.enc`. A user-named `.enc`
        // file is never touched.
        let stem_is_uuid = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .is_some();
        if !stem_is_uuid {
            continue;
        }
        let name = match path.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => continue,
        };
        if known.contains(&name) {
            continue; // a row (live or trashed) references this file
        }
        // Age guard: an in-flight fetch/upload may have written the file
        // before its row update landed.
        let mtime = entry.metadata().and_then(|m| m.modified()).ok();
        if let Some(t) = mtime
            && now.duration_since(t).unwrap_or(Duration::ZERO) < Duration::from_secs(600)
        {
            continue;
        }
        // Ciphertext at rest — a plain unlink is sufficient.
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(error = %e, "orphan enc sweep: delete failed"),
        }
    }
    if removed > 0 {
        info!(
            count = removed,
            "Deleted orphaned .enc audio artifacts with no DB row"
        );
    }
}

/// Sweep: shred stale screenshot-OCR capture PNGs from the macOS private
/// capture dir (`data_dir/capture-tmp`).
///
/// macOS is the one platform where captured pixels transit disk (the
/// `screencapture` tool has no stdout mode); the capture path pre-creates
/// the file 0600 and shreds + unlinks it before OCR, but a process crash
/// mid-selection — or the capture-timeout error path — can strand one. The
/// PNG is patient data: it is shredded (never a plain unlink) and deleted;
/// there is no recovery value in keeping it.
///
/// Only files our own capture path names (`capture-*.png`) are touched, and
/// only when modified more than 10 minutes ago (an in-flight selection may
/// legitimately hold a fresh file). PHI-safe: logs carry counts only.
pub fn capture_tmp_sweep(data_dir: &Path) {
    let dir = data_dir.join("capture-tmp");
    let entries = match std::fs::read_dir(&dir) {
        Ok(d) => d,
        Err(_) => return, // dir doesn't exist yet — nothing ever captured
    };

    let now = std::time::SystemTime::now();
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let Some(name) = name else {
            continue;
        };
        // Only our own capture files — never anything else in the dir.
        if !name.starts_with("capture-") || path.extension().and_then(|e| e.to_str()) != Some("png")
        {
            continue;
        }
        // Age guard: an in-flight selection may hold a fresh file.
        let mtime = entry.metadata().and_then(|m| m.modified()).ok();
        if let Some(t) = mtime
            && now.duration_since(t).unwrap_or(Duration::ZERO) < Duration::from_secs(600)
        {
            continue;
        }
        // Plaintext PHI pixels — shred, never a plain unlink. NotFound (a
        // capture racing us) counts as gone.
        match medical_security::file_crypto::shred_and_unlink(&path) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => removed += 1,
            Err(e) => tracing::warn!(error = %e, "capture tmp sweep: delete failed"),
        }
    }
    if removed > 0 {
        info!(
            count = removed,
            "Shredded stale screenshot-OCR capture PNGs"
        );
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
            // The tick is SQLite + file shredding — blocking work. Run each
            // tick on the blocking pool, never on the async worker.
            let tick_db = Arc::clone(&db);
            if let Err(e) =
                tokio::task::spawn_blocking(move || retention_sweep_tick(&tick_db)).await
            {
                tracing::warn!(error = %e, "tombstone sweeper tick task failed");
            }
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

    /// Seed an AGED tombstone (deleted_at `days` ago) on a still-visible
    /// row with a single raw UPDATE — the same statement shape
    /// `soft_delete` uses. Updating `deleted_at` again AFTER soft_delete
    /// would fire the FTS trigger against an already de-indexed row and
    /// fail with SQLITE_CORRUPT.
    fn seed_aged_tombstone(conn: &rusqlite::Connection, rec: &Recording, days: i64) {
        let past = (chrono::Utc::now() - chrono::TimeDelta::days(days))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        conn.execute(
            "UPDATE recordings SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            rusqlite::params![past, rec.id.to_string()],
        )
        .expect("seed aged tombstone");
    }

    /// INVERTED (trash-restore D7): the purge now runs on EVERY machine.
    /// A client tick purges aged tombstones — row gone, audio file
    /// removed, ledger entry written — while the retention phase still
    /// only TRASHES old visible recordings (a just-trashed row is not yet
    /// purge-eligible, so it survives as a row).
    #[test]
    fn retention_sweep_client_purges_aged_tombstones_and_trashes_old_visible() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tombstone_audio = tmp.path().join("aged-tombstone.wav");
        std::fs::write(&tombstone_audio, b"RIFF fake wav bytes").expect("write audio");

        let db = Database::open_in_memory().expect("db");
        let (aged, old, fresh) = {
            let conn = db.conn().expect("conn");
            set_retention_days(&conn, Some(90));
            let aged = seed_days_old(&conn, 100, "aged.wav", tombstone_audio.clone());
            let old = seed_days_old(&conn, 100, "old-visit.wav", "/audio/old.wav".into());
            let fresh = seed_days_old(&conn, 10, "fresh-visit.wav", "/audio/fresh.wav".into());
            seed_aged_tombstone(&conn, &aged, 40);
            (aged, old, fresh)
        };

        // Client tick — no server anywhere; the purge runs regardless.
        retention_sweep_tick(&db);

        let conn = db.conn().expect("conn");
        assert!(
            !row_exists(&conn, aged.id),
            "aged tombstone purged on a client"
        );
        assert!(
            !tombstone_audio.exists(),
            "audio file removed with the purged row"
        );
        let ledger_at: Option<String> = conn
            .query_row(
                "SELECT purged_at FROM purged_recordings WHERE id = ?1",
                [aged.id.to_string()],
                |row| row.get(0),
            )
            .expect("query ledger");
        assert!(ledger_at.is_some(), "client purge writes the ledger");
        assert!(
            deleted_at_raw(&conn, old.id).is_some(),
            "old visible recording trashed by the retention phase"
        );
        assert!(
            row_exists(&conn, old.id),
            "a just-trashed row is not purge-eligible — it survives"
        );
        assert!(
            deleted_at_raw(&conn, fresh.id).is_none(),
            "fresh recording untouched"
        );
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
            seed_aged_tombstone(&conn, &rec, 40);
            (rec, visible)
        };

        retention_sweep_tick(&db);

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

    /// PURGE-FIRST race (trash-restore D7, Codie gate 1): a restore
    /// landing between the sweeper's listing and its purge must leave the
    /// row ACTIVE with its audio file intact on disk. Drives the exact
    /// production sequence: the stale candidate listing from
    /// `list_soft_deleted_older_than`, a restore, then
    /// `purge_tombstone_batch` on the stale listing.
    #[test]
    fn restore_between_listing_and_purge_leaves_row_active_with_audio() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let audio_path = tmp.path().join("raced-visit.wav");
        std::fs::write(&audio_path, b"RIFF fake wav bytes").expect("write audio");

        let db = Database::open_in_memory().expect("db");
        let rec = {
            let conn = db.conn().expect("conn");
            set_retention_days(&conn, None);
            let rec = seed_days_old(&conn, 100, "raced-visit.wav", audio_path.clone());
            seed_aged_tombstone(&conn, &rec, 40);
            rec
        };

        // The sweeper's listing runs FIRST…
        let stale_listing = {
            let conn = db.conn().expect("conn");
            RecordingsRepo::list_soft_deleted_older_than(&conn, 30, chrono::Utc::now())
                .expect("list candidates")
        };
        assert_eq!(stale_listing.len(), 1, "fixture: one purge candidate");

        // …then the user's restore lands inside the race window…
        {
            let conn = db.conn().expect("conn");
            RecordingsRepo::restore(&conn, &rec.id).expect("restore lands mid-sweep");
        }

        // …and the purge proceeds with the STALE listing.
        let confirmed = {
            let conn = db.conn().expect("conn");
            purge_tombstone_batch(&conn, &stale_listing)
        };

        assert!(confirmed.is_empty(), "the restored row is not purged");
        let conn = db.conn().expect("conn");
        assert!(row_exists(&conn, rec.id), "row survived");
        assert!(
            deleted_at_raw(&conn, rec.id).is_none(),
            "row is ACTIVE, not trashed"
        );
        assert!(
            audio_path.exists(),
            "restored row's audio must still be on disk"
        );
        let ledgered: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM purged_recordings WHERE id = ?1",
                [rec.id.to_string()],
                |row| row.get(0),
            )
            .expect("count ledger");
        assert_eq!(
            ledgered, 0,
            "no ledger entry for a row that was never purged"
        );
    }

    #[test]
    fn retention_sweep_without_window_trashes_nothing() {
        let db = Database::open_in_memory().expect("db");
        let old = {
            let conn = db.conn().expect("conn");
            set_retention_days(&conn, None);
            seed_days_old(&conn, 400, "ancient-visit.wav", "/audio/ancient.wav".into())
        };

        retention_sweep_tick(&db);

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
        // Keychain isolation: RAII guard with a synthetic key —
        // `encrypt_file_in_place` inside the sweep resolves through the
        // global mock on any thread, never the OS keychain. Pairing: the
        // DB is in-memory, the orphan WAV is a tempdir fixture.
        let _mock = crate::testutil::KeychainMockGuard::fixed_db_key([0xBBu8; 32]);

        let tmp = tempfile::tempdir().expect("tempdir");
        let orphan = write_aged_wav(tmp.path(), "crash-mid-recording.wav");

        let db = Database::open_in_memory().expect("db");

        // Crypto is always available via the synthetic provider — no need
        // for the probe-and-bail fallback.
        orphaned_wav_sweep(&db, tmp.path());

        assert!(
            medical_security::file_crypto::is_encrypted(&orphan),
            "row-less WAV must be encrypted at rest"
        );
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

    #[test]
    fn translation_wav_sweep_deletes_stale_utterances_only() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // Stale utterance (mtime an hour old) — crash leftover, delete it.
        let stale = write_aged_wav(tmp.path(), "utterance-deadbeef.wav");
        // Fresh utterance — could belong to an in-progress capture after a
        // very fast restart; must survive.
        let fresh = tmp.path().join("utterance-live.wav");
        std::fs::write(&fresh, b"RIFF in flight").expect("write fresh");
        // Aged file that is NOT one of ours — never touch it.
        let foreign = write_aged_wav(tmp.path(), "user-notes.wav");

        translation_wav_sweep(tmp.path());

        assert!(!stale.exists(), "stale utterance WAV must be deleted");
        assert!(fresh.exists(), "fresh utterance WAV must be kept");
        assert!(
            foreign.exists(),
            "non-utterance files must never be touched"
        );

        // Missing directory is a no-op, not an error (nothing ever captured).
        translation_wav_sweep(&tmp.path().join("does-not-exist"));
    }

    /// Screenshot-OCR capture leftovers (macOS is the one platform where
    /// pixels transit disk): aged `capture-*.png` files are shredded, fresh
    /// ones (possible in-flight selection) and foreign files are kept.
    #[test]
    fn capture_tmp_sweep_shreds_stale_capture_pngs_only() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // Stale capture (mtime an hour old) — crash/timeout leftover.
        let stale = tmp.path().join("capture-tmp").join("capture-deadbeef.png");
        std::fs::create_dir_all(stale.parent().unwrap()).expect("capture dir");
        std::fs::write(&stale, b"\x89PNG pixels").expect("write stale capture");
        backdate(&stale);
        // Fresh capture — an in-flight selection may hold it.
        let fresh = stale.parent().unwrap().join("capture-live.png");
        std::fs::write(&fresh, b"\x89PNG in flight").expect("write fresh");
        // Aged but NOT one of ours — never touch it.
        let foreign = stale.parent().unwrap().join("user-screenshot.png");
        std::fs::write(&foreign, b"\x89PNG user file").expect("write foreign");
        backdate(&foreign);

        capture_tmp_sweep(tmp.path());

        assert!(!stale.exists(), "stale capture PNG must be shredded");
        assert!(fresh.exists(), "fresh capture may be in flight — kept");
        assert!(
            foreign.exists(),
            "non-capture files are never ours to delete"
        );

        // Missing directory is a no-op, not an error (macOS-only feature;
        // other platforms never create it).
        capture_tmp_sweep(&tmp.path().join("nowhere"));
    }

    /// Backdate a file's mtime by an hour so the age guards let the sweeps
    /// see it (shared fixture helper for the newer sweeps).
    fn backdate(path: &Path) {
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let ft = filetime::FileTime::from_system_time(past);
        filetime::set_file_mtime(path, ft).expect("backdate mtime");
    }

    /// A purge whose audio removal FAILS (here: the path is a directory, so
    /// unlink errors with something other than NotFound) must enqueue the
    /// path for retry — the row is already gone, so the queue is the only
    /// thing that can ever finish the deletion.
    #[test]
    fn purge_removal_failure_queues_path_and_retry_sweep_completes_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // A directory where the audio file should be: remove_file fails
        // (EISDIR-class), never NotFound.
        let stubborn = tmp.path().join("locked-visit.wav");
        std::fs::create_dir(&stubborn).expect("create dir-as-audio-path");

        let db = Database::open_in_memory().expect("db");
        let rec = {
            let conn = db.conn().expect("conn");
            set_retention_days(&conn, None);
            let rec = seed_days_old(&conn, 100, "locked.wav", stubborn.clone());
            seed_aged_tombstone(&conn, &rec, 40);
            rec
        };

        // First tick: row purged, removal fails, path enqueued.
        retention_sweep_tick(&db);
        {
            let conn = db.conn().expect("conn");
            assert!(
                !row_exists(&conn, rec.id),
                "row purged despite file failure"
            );
        }
        let queue = {
            let conn = db.conn().expect("conn");
            ContentSyncRepo::get_pending_file_removals(&conn).expect("queue read")
        };
        assert_eq!(
            queue,
            vec![stubborn.to_string_lossy().into_owned()],
            "failed removal path must be queued for retry"
        );
        assert!(stubborn.exists(), "the stubborn path itself is untouched");

        // Retry while still failing: the path stays queued.
        {
            let conn = db.conn().expect("conn");
            pending_file_removals_sweep(&conn);
            let queue = ContentSyncRepo::get_pending_file_removals(&conn).expect("queue read");
            assert_eq!(queue.len(), 1, "still-failing removal stays queued");
        }

        // Obstacle removed (e.g. the lock released / user cleared it): the
        // next retry sweep finishes the job and empties the queue.
        std::fs::remove_dir(&stubborn).expect("clear the obstacle");
        {
            let conn = db.conn().expect("conn");
            pending_file_removals_sweep(&conn);
            let queue = ContentSyncRepo::get_pending_file_removals(&conn).expect("queue read");
            assert!(queue.is_empty(), "completed removal leaves the queue");
        }
    }

    /// The retry sweep drops a path on success AND on NotFound (cleaned up
    /// by other means) — only real errors keep it.
    #[test]
    fn pending_file_removals_sweep_removes_files_and_drops_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("queued-visit.wav");
        std::fs::write(&file, b"RIFF queued").expect("write file");
        let missing = tmp.path().join("already-gone.wav");

        let db = Database::open_in_memory().expect("db");
        {
            let conn = db.conn().expect("conn");
            ContentSyncRepo::set_pending_file_removals(
                &conn,
                &[
                    file.to_string_lossy().into_owned(),
                    missing.to_string_lossy().into_owned(),
                ],
            )
            .expect("seed queue");
            pending_file_removals_sweep(&conn);

            assert!(!file.exists(), "queued file must be removed");
            let queue = ContentSyncRepo::get_pending_file_removals(&conn).expect("queue read");
            assert!(
                queue.is_empty(),
                "success and NotFound both drop their paths"
            );
        }
    }

    /// Rowless `.enc` sync artifacts are deleted; row-backed, fresh,
    /// non-UUID-named, and non-`.enc` files are never touched.
    #[test]
    fn orphaned_enc_sweep_deletes_only_aged_rowless_uuid_enc() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // Aged + rowless + UUID stem — the crash-leftover this sweep exists
        // for.
        let orphan = tmp.path().join(format!("{}.enc", Uuid::new_v4()));
        std::fs::write(&orphan, b"FE1\x00\x01ciphertext").expect("write orphan");
        backdate(&orphan);

        // Row-backed: a row's audio_path references it — kept (its row's
        // 30-day purge owns it, live or trashed).
        let owned = tmp.path().join(format!("{}.enc", Uuid::new_v4()));
        std::fs::write(&owned, b"FE1\x00\x01ciphertext").expect("write owned");
        backdate(&owned);

        // Fresh rowless — may belong to an in-flight fetch/upload.
        let fresh = tmp.path().join(format!("{}.enc", Uuid::new_v4()));
        std::fs::write(&fresh, b"FE1\x00\x01ciphertext").expect("write fresh");

        // Aged rowless but NOT UUID-named — could be a user file.
        let foreign = tmp.path().join("notes.enc");
        std::fs::write(&foreign, b"user data").expect("write foreign");
        backdate(&foreign);

        // Aged rowless WAV — the wav sweep's business, never ours.
        let wav = tmp.path().join("crash.wav");
        std::fs::write(&wav, b"RIFF plaintext").expect("write wav");
        backdate(&wav);

        let db = Database::open_in_memory().expect("db");
        {
            let conn = db.conn().expect("conn");
            seed_days_old(&conn, 0, "owned.wav", owned.clone());
        }

        orphaned_enc_sweep(&db, tmp.path());

        assert!(
            !orphan.exists(),
            "aged rowless uuid-stem .enc must be deleted"
        );
        assert!(owned.exists(), "row-backed .enc must be kept");
        assert!(fresh.exists(), "fresh .enc may be in flight — kept");
        assert!(foreign.exists(), "non-UUID .enc is never ours to delete");
        assert!(wav.exists(), "wav files are the other sweep's business");
    }
}
