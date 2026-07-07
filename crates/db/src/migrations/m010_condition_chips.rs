use rusqlite::Connection;

use crate::DbResult;

/// Create the `condition_chips` table and seed it from existing
/// `AppConfig.custom_conditions` values (if any).
///
/// Each existing condition becomes an active row with `updated_at = now()`.
/// The old `custom_conditions` field in the settings blob is left intact
/// (inert) for rollback safety.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS condition_chips (
            id          TEXT PRIMARY KEY,
            text        TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            deleted_at  TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_condition_chips_active
            ON condition_chips(text) WHERE deleted_at IS NULL;",
    )?;

    // Seed from existing custom_conditions in the settings blob.
    seed_from_custom_conditions(conn)?;

    Ok(())
}

/// Read `custom_conditions` from the `settings` table (key "app_config")
/// and insert each as an active condition chip.
fn seed_from_custom_conditions(conn: &Connection) -> DbResult<()> {
    use medical_core::types::condition_chip::{ConditionChip, deterministic_id};
    use medical_core::types::settings::AppConfig;

    let json: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'app_config'",
            [],
            |row| row.get(0),
        )
        .ok();

    let Some(json) = json else {
        return Ok(());
    };
    let config: AppConfig = match serde_json::from_str(&json) {
        Ok(c) => c,
        Err(_) => return Ok(()), // unparseable config — skip seeding
    };

    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();
    for text in &config.custom_conditions {
        let chip = ConditionChip {
            id: deterministic_id(text),
            text: text.clone(),
            updated_at: now.clone(),
            deleted_at: None,
            sort_order: 0,
        };
        let _ = conn.execute(
            "INSERT OR IGNORE INTO condition_chips (id, text, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, NULL)",
            rusqlite::params![chip.id, chip.text, chip.updated_at],
        );
    }

    tracing::info!(
        count = config.custom_conditions.len(),
        "Seeded condition chips from custom_conditions"
    );
    Ok(())
}
