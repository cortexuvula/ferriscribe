//! Key-value settings store backed by the `settings` table, plus helpers
//! to load and persist the full `AppConfig`.

use rusqlite::Connection;

use medical_core::types::settings::{
    AppConfig, TRASH_RETENTION_DAYS_CHOICES, effective_trash_retention_days,
};

use crate::{DbError, DbResult};

pub struct SettingsRepo;

impl SettingsRepo {
    /// Return the stored value for `key`, or `None` if it is not present.
    pub fn get(conn: &Connection, key: &str) -> DbResult<Option<String>> {
        let result = conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        });
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sqlite(e)),
        }
    }

    /// Upsert `key = value`.  The `updated_at` column is refreshed automatically.
    pub fn set(conn: &Connection, key: &str, value: &str) -> DbResult<()> {
        conn.execute(
            "INSERT INTO settings (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET
                 value = excluded.value,
                 updated_at = excluded.updated_at",
            [key, value],
        )?;
        Ok(())
    }

    /// Remove the entry for `key`.  Silently succeeds if the key is absent.
    pub fn delete(conn: &Connection, key: &str) -> DbResult<()> {
        conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    /// True iff `key` is present in the settings table. Used by `get_settings`
    /// to detect whether an `app_config` row already existed (i.e. this is an
    /// existing install) vs. a fresh default — so existing users can be
    /// auto-marked as onboarded without showing them the first-run wizard.
    pub fn exists(conn: &Connection, key: &str) -> DbResult<bool> {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM settings WHERE key = ?1",
            [key],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }

    /// Deserialise the stored JSON blob (under the key `"app_config"`) into an
    /// `AppConfig`.  Falls back to `AppConfig::default()` when the key is
    /// absent or unparseable.
    pub fn load_config(conn: &Connection) -> DbResult<AppConfig> {
        match Self::get(conn, "app_config")? {
            Some(json) => {
                let cfg = match serde_json::from_str(&json) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to parse app_config JSON; using defaults");
                        Default::default()
                    }
                };
                Ok(cfg)
            }
            None => Ok(AppConfig::default()),
        }
    }

    /// Persist a whole config without authorizing a shorter Trash window.
    /// Background writers with stale snapshots must pass the same policy gate
    /// as the settings command; they cannot silently reduce the undo period.
    pub fn save_config(conn: &Connection, config: &AppConfig) -> DbResult<()> {
        Self::save_config_with_trash_confirmation(conn, config, None)
    }

    /// Validate the policy, recheck its current purge count, and persist in one
    /// transaction. Also works within a caller-owned transaction (template sync).
    pub fn save_config_with_trash_confirmation(
        conn: &Connection,
        config: &AppConfig,
        confirmed_trash_purge_count: Option<u32>,
    ) -> DbResult<()> {
        if conn.is_autocommit() {
            let tx = crate::unchecked_transaction_immediate(conn)?;
            Self::save_config_checked(&tx, config, confirmed_trash_purge_count)?;
            tx.commit()?;
            Ok(())
        } else {
            Self::save_config_checked(conn, config, confirmed_trash_purge_count)
        }
    }

    fn save_config_checked(
        conn: &Connection,
        config: &AppConfig,
        confirmed_trash_purge_count: Option<u32>,
    ) -> DbResult<()> {
        if !TRASH_RETENTION_DAYS_CHOICES.contains(&config.trash_retention_days) {
            return Err(DbError::Constraint(
                "trash_retention_days must be 7, 14, 21, or 30.".into(),
            ));
        }
        let previous = Self::load_config(conn)?;
        if config.trash_retention_days
            < effective_trash_retention_days(previous.trash_retention_days)
        {
            let count = crate::recordings::RecordingsRepo::count_soft_deleted_older_than(
                conn,
                config.trash_retention_days,
                chrono::Utc::now(),
            )?;
            if confirmed_trash_purge_count != Some(count) {
                return Err(DbError::Constraint(format!(
                    "Shortening the Trash window requires confirmation of the current purge count ({count}); refresh and confirm again."
                )));
            }
        }
        let json = serde_json::to_string(config).map_err(|e| DbError::Migration(e.to_string()))?;
        Self::set(conn, "app_config", &json)
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
    fn get_missing_none() {
        let conn = migrated();
        let v = SettingsRepo::get(&conn, "does_not_exist").unwrap();
        assert!(v.is_none());
    }

    #[test]
    fn set_and_get() {
        let conn = migrated();
        SettingsRepo::set(&conn, "theme", "dark").unwrap();
        let v = SettingsRepo::get(&conn, "theme").unwrap();
        assert_eq!(v.as_deref(), Some("dark"));
    }

    #[test]
    fn overwrite() {
        let conn = migrated();
        SettingsRepo::set(&conn, "key", "first").unwrap();
        SettingsRepo::set(&conn, "key", "second").unwrap();
        let v = SettingsRepo::get(&conn, "key").unwrap().unwrap();
        assert_eq!(v, "second");
    }

    #[test]
    fn delete() {
        let conn = migrated();
        SettingsRepo::set(&conn, "temp", "value").unwrap();
        SettingsRepo::delete(&conn, "temp").unwrap();
        let v = SettingsRepo::get(&conn, "temp").unwrap();
        assert!(v.is_none());
    }

    #[test]
    fn exists_false_for_missing_key() {
        let conn = migrated();
        assert!(!SettingsRepo::exists(&conn, "app_config").unwrap());
    }

    #[test]
    fn exists_true_after_set() {
        let conn = migrated();
        SettingsRepo::save_config(&conn, &AppConfig::default()).unwrap();
        assert!(SettingsRepo::exists(&conn, "app_config").unwrap());
    }

    #[test]
    fn load_default_when_none() {
        let conn = migrated();
        let cfg = SettingsRepo::load_config(&conn).unwrap();
        let default = AppConfig::default();
        assert_eq!(cfg.language, default.language);
        assert_eq!(cfg.theme, default.theme);
    }

    #[test]
    fn interleaved_stale_whole_config_save_cannot_reduce_trash_window() {
        let conn = migrated();
        // A background writer reads a formerly shorter window.
        SettingsRepo::set(&conn, "app_config", r#"{"trash_retention_days":7}"#).unwrap();
        let mut stale = SettingsRepo::load_config(&conn).unwrap();
        // The user subsequently increases it to 30.
        SettingsRepo::save_config(&conn, &AppConfig::default()).unwrap();
        stale.language = "de-DE".into();
        let _ = SettingsRepo::save_config(&conn, &stale);
        assert_eq!(
            SettingsRepo::load_config(&conn)
                .unwrap()
                .trash_retention_days,
            30,
            "a stale whole-config writer must not bypass reduction confirmation"
        );
    }

    #[test]
    fn save_and_load() {
        let conn = migrated();
        let mut cfg = AppConfig::default();
        cfg.language = "de-DE".into();
        cfg.window_width = 1440;
        SettingsRepo::save_config(&conn, &cfg).unwrap();

        let loaded = SettingsRepo::load_config(&conn).unwrap();
        assert_eq!(loaded.language, "de-DE");
        assert_eq!(loaded.window_width, 1440);
    }
}
