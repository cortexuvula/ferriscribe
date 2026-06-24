use medical_db::encryption::{apply_pragma_key, is_plaintext_db, migrate_plaintext_to_encrypted};
use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn migration_happy_path_preserves_row_counts() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("medical.db");

    // Seed a plaintext DB with two tables.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE recordings (id TEXT, transcript TEXT);
             INSERT INTO recordings VALUES ('r1', 'hello'), ('r2', 'world');
             CREATE TABLE notes (id INTEGER, body TEXT);
             INSERT INTO notes VALUES (1, 'note one'), (2, 'note two'), (3, 'note three');",
        )
        .unwrap();
    }

    let key = [0xABu8; 32];
    let outcome = migrate_plaintext_to_encrypted(&db_path, &key).expect("migration");
    assert!(
        outcome.backup_deleted,
        "backup should be deleted on success"
    );
    assert_eq!(outcome.encrypted_path, db_path);

    // The DB at db_path is now encrypted — its header is no longer "SQLite format 3\0".
    assert!(
        !is_plaintext_db(&db_path).unwrap(),
        "DB should no longer be plaintext"
    );

    // Backup should be gone.
    let mut backup = db_path.as_os_str().to_owned();
    backup.push(".pre-encryption.bak");
    let backup_path = std::path::PathBuf::from(backup);
    assert!(!backup_path.exists(), "backup file should be deleted");

    // Open encrypted, verify row counts.
    let conn = Connection::open(&db_path).unwrap();
    apply_pragma_key(&conn, &key).unwrap();
    let recordings_count: i64 = conn
        .query_row("SELECT count(*) FROM recordings", [], |row| row.get(0))
        .unwrap();
    assert_eq!(recordings_count, 2);
    let notes_count: i64 = conn
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(notes_count, 3);

    // Verify a row's content too — make sure we're not just counting empty rows.
    let transcript: String = conn
        .query_row(
            "SELECT transcript FROM recordings WHERE id = 'r1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(transcript, "hello");
}

#[test]
fn migration_is_idempotent_check_via_is_plaintext_db() {
    // Migration is gated by the boot flow on is_plaintext_db. After encryption,
    // is_plaintext_db returns false, so the boot flow won't re-migrate.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("medical.db");

    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1);")
            .unwrap();
    }

    let key = [0xABu8; 32];
    migrate_plaintext_to_encrypted(&db_path, &key).expect("first migration");
    assert!(!is_plaintext_db(&db_path).unwrap(), "encrypted now");
    // The boot flow uses is_plaintext_db() to decide whether to migrate, so
    // the second-launch behavior is "skip migration entirely". We verify the
    // signal here rather than calling migrate again (which would fail because
    // the DB is no longer plaintext).
}

#[test]
fn migration_preserves_indices_and_views() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("medical.db");

    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE recordings (id TEXT PRIMARY KEY, transcript TEXT);
             CREATE INDEX idx_recordings_transcript ON recordings(transcript);
             CREATE VIEW recordings_view AS SELECT id FROM recordings;
             INSERT INTO recordings VALUES ('r1', 'hello world');",
        )
        .unwrap();
    }

    let key = [0xABu8; 32];
    migrate_plaintext_to_encrypted(&db_path, &key).expect("migration");

    let conn = Connection::open(&db_path).unwrap();
    apply_pragma_key(&conn, &key).unwrap();

    // Index survived — query plan check.
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='recordings'")
        .unwrap();
    let indices: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        indices.iter().any(|n| n == "idx_recordings_transcript"),
        "index should survive migration; got: {indices:?}"
    );

    // View survived.
    let view_count: i64 = conn
        .query_row("SELECT count(*) FROM recordings_view", [], |row| row.get(0))
        .unwrap();
    assert_eq!(view_count, 1);
}
