//! Append-only audit log backed by the `audit_log` table.

use chrono::{DateTime, Utc};
use rusqlite::Connection;

use crate::DbResult;

/// A single entry read back from the audit log.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub id: i64,
    pub timestamp: DateTime<Utc>,
    pub action: String,
    pub actor: String,
    pub resource: String,
    pub details: Option<String>,
}

/// Repository for the append-only `audit_log` table.
///
/// The audit log records immutable entries for tracking data mutations.
/// SQLite triggers prevent UPDATE and DELETE operations on the table.
pub struct AuditRepo;

impl AuditRepo {
    /// Append a new audit entry and return the auto-generated row ID.
    ///
    /// # Parameters
    ///
    /// - `action` -- what happened (e.g. "create", "update", "delete").
    /// - `actor` -- who did it (e.g. "user1", "system").
    /// - `resource` -- what was affected (e.g. "recording/abc-123").
    /// - `details` -- optional free-form detail (must not contain PHI).
    pub fn append(
        conn: &Connection,
        action: &str,
        actor: &str,
        resource: &str,
        details: Option<&str>,
    ) -> DbResult<i64> {
        conn.execute(
            "INSERT INTO audit_log (action, actor, resource, details)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![action, actor, resource, details],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Return the `limit` most-recent audit entries, newest first.
    pub fn list_recent(conn: &Connection, limit: u32) -> DbResult<Vec<AuditEntry>> {
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, action, actor, resource, details
             FROM audit_log
             ORDER BY id DESC
             LIMIT ?1",
        )?;

        let entries = stmt
            .query_map([limit], |row| {
                let id: i64 = row.get(0)?;
                let ts_str: String = row.get(1)?;
                let action: String = row.get(2)?;
                let actor: String = row.get(3)?;
                let resource: String = row.get(4)?;
                let details: Option<String> = row.get(5)?;

                let timestamp = DateTime::parse_from_rfc3339(&ts_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| {
                        tracing::error!(
                            ts_str = %ts_str,
                            error = %e,
                            "corrupt audit_log timestamp in DB"
                        );
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;

                Ok(AuditEntry {
                    id,
                    timestamp,
                    action,
                    actor,
                    resource,
                    details,
                })
            })?
            .filter_map(|r| {
                r.map_err(|e| tracing::warn!(error = %e, "dropping unreadable row"))
                    .ok()
            })
            .collect();

        Ok(entries)
    }

    /// Total number of entries in the audit log.
    pub fn count(conn: &Connection) -> DbResult<u32> {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))?;
        Ok(n as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::MigrationEngine;
    use rusqlite::Connection;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        MigrationEngine::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn append_and_list() {
        let conn = migrated();
        AuditRepo::append(&conn, "create", "user1", "recording/1", None).unwrap();
        AuditRepo::append(
            &conn,
            "update",
            "user1",
            "recording/1",
            Some("changed transcript"),
        )
        .unwrap();
        AuditRepo::append(&conn, "delete", "admin", "recording/1", None).unwrap();

        let entries = AuditRepo::list_recent(&conn, 10).unwrap();
        assert_eq!(entries.len(), 3);
        // Most recent first
        assert_eq!(entries[0].action, "delete");
        assert_eq!(entries[1].action, "update");
        assert_eq!(entries[1].details.as_deref(), Some("changed transcript"));
        assert_eq!(entries[2].action, "create");
    }

    #[test]
    fn append_only_enforcement() {
        let conn = migrated();
        let id = AuditRepo::append(&conn, "action", "actor", "res", None).unwrap();
        // UPDATE should be rejected by trigger
        let upd = conn.execute("UPDATE audit_log SET action='hacked' WHERE id=?1", [id]);
        assert!(upd.is_err(), "UPDATE must be rejected");
        // DELETE should be rejected by trigger
        let del = conn.execute("DELETE FROM audit_log WHERE id=?1", [id]);
        assert!(del.is_err(), "DELETE must be rejected");
    }

    #[test]
    fn count() {
        let conn = migrated();
        assert_eq!(AuditRepo::count(&conn).unwrap(), 0);
        AuditRepo::append(&conn, "a", "b", "c", None).unwrap();
        AuditRepo::append(&conn, "x", "y", "z", Some("detail")).unwrap();
        assert_eq!(AuditRepo::count(&conn).unwrap(), 2);
    }
}
