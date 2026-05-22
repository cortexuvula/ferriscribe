# Letter Audience System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable clinicians to generate letters tailored to different audiences (patient, insurance, tax, specialist, employer, legal) with audience-specific prompts and templates.

**Architecture:** Two-layer selection: audience (the "who") controls tone and structure via custom system prompts and user templates; letter type (the "what for") provides context via free text. Data layer uses a dedicated SQLite table with 6 seeded built-ins. Frontend provides a two-layer picker on the letter card and a settings panel for custom audience management.

**Tech Stack:** Rust (crates/db, crates/processing, crates/core), Tauri v2, Svelte 5, TypeScript, SQLite

**Spec:** `docs/superpowers/specs/2026-05-22-letter-audience-design.md`

---

## Phase 1: Backend Foundation

### Task 1: Add LetterAudience type to core crate

**Files:**
- Create: `crates/core/src/types/letter_audience.rs`
- Modify: `crates/core/src/types/mod.rs`

- [ ] **Step 1: Define LetterAudience struct**

Create `crates/core/src/types/letter_audience.rs`:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LetterAudience {
    pub id: Uuid,
    pub name: String,
    pub system_prompt: String,
    pub user_template: Option<String>,
    pub is_builtin: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LetterAudience {
    pub fn new(name: String, system_prompt: String, user_template: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            system_prompt,
            user_template,
            is_builtin: false,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn builtin(
        id: Uuid,
        name: &str,
        system_prompt: &str,
        user_template: Option<&str>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            name: name.to_string(),
            system_prompt: system_prompt.to_string(),
            user_template: user_template.map(|s| s.to_string()),
            is_builtin: true,
            created_at: now,
            updated_at: now,
        }
    }
}
```

- [ ] **Step 2: Export from types module**

Modify `crates/core/src/types/mod.rs` to add:

```rust
pub mod letter_audience;
pub use letter_audience::LetterAudience;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p medical-core --lib`
Expected: PASS (no tests yet, but compiles)

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/types/letter_audience.rs crates/core/src/types/mod.rs
git commit -m "feat(core): add LetterAudience type

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 2: Create database migration for letter_audiences table

**Files:**
- Create: `crates/db/src/migrations/m006_letter_audiences.rs`
- Modify: `crates/db/src/migrations/mod.rs`

- [ ] **Step 1: Create migration file**

Create `crates/db/src/migrations/m006_letter_audiences.rs`:

```rust
//! Migration 006: `letter_audiences` table for audience-specific letter generation.
//!
//! Stores system prompts and user templates for different letter recipients
//! (patient, insurance, tax, etc.). Includes 6 seeded built-in rows.

use rusqlite::Connection;
use uuid::Uuid;

use crate::DbResult;

pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS letter_audiences (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            system_prompt TEXT NOT NULL,
            user_template TEXT,
            is_builtin INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        "#,
    )?;

    // Seed 6 built-in audiences
    let now = "2026-05-22T00:00:00Z";
    let builtins = vec![
        (
            "builtin-patient",
            "Patient",
            "You are a medical scribe assistant helping to write patient-friendly correspondence. Use clear, plain language the patient can understand. Avoid unexplained medical jargon. Be empathetic and professional.",
            None,
        ),
        (
            "builtin-insurance",
            "Insurance Company",
            "You are a medical scribe assistant writing formal correspondence for insurance companies. Use precise medical necessity language, reference ICD-10 and CPT codes where applicable, and structure the letter to justify medical necessity for the requested service or treatment.",
            Some("Please write a {letter_type} letter for the insurance company based on the following SOAP note. Include a medical necessity statement, relevant diagnosis codes (ICD-10), and procedure codes (CPT) if applicable:\n\n{time_date}\n\n{soap_note}"),
        ),
        (
            "builtin-tax",
            "Tax Authority",
            "You are a medical scribe assistant writing correspondence for tax authorities or disability benefit agencies. Focus on factual timeline, expense justification, and medical necessity. Use formal, objective language.",
            Some("Please write a {letter_type} letter for the tax authority based on the following SOAP note. Include service dates, cost justification, and medical necessity for the expenses or disability claim:\n\n{time_date}\n\n{soap_note}"),
        ),
        (
            "builtin-specialist",
            "Specialist/Consultant",
            "You are a medical scribe assistant writing professional referral correspondence to a specialist or consultant. Use clinical detail, professional peer tone, and include relevant history, findings, and specific questions for the consultant.",
            Some("Please write a {letter_type} referral letter to the specialist based on the following SOAP note. Include relevant medical history, objective findings, and specific questions or requests for the consultant:\n\n{time_date}\n\n{soap_note}"),
        ),
        (
            "builtin-employer",
            "Employer/School",
            "You are a medical scribe assistant writing correspondence for employers or educational institutions. Focus on functional limitations, recommended accommodations, and fitness-for-duty. Keep medical details minimal and HIPAA-compliant.",
            Some("Please write a {letter_type} letter for the employer or school based on the following SOAP note. Focus on functional limitations and recommended accommodations. Avoid unnecessary medical details:\n\n{time_date}\n\n{soap_note}"),
        ),
        (
            "builtin-legal",
            "Legal/Court",
            "You are a medical scribe assistant writing formal medical opinion letters for legal proceedings or court. Use objective, factual language. Include chronological timeline, clinical findings, and professional medical opinion.",
            Some("Please write a {letter_type} letter for legal or court purposes based on the following SOAP note. Include a chronological timeline, objective clinical findings, and your professional medical opinion:\n\n{time_date}\n\n{soap_note}"),
        ),
    ];

    for (id, name, system_prompt, user_template) in builtins {
        let uuid = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR IGNORE INTO letter_audiences (id, name, system_prompt, user_template, is_builtin, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
            rusqlite::params![id, name, system_prompt, user_template, now, now],
        )?;
    }

    Ok(())
}
```

- [ ] **Step 2: Register migration in mod.rs**

Modify `crates/db/src/migrations/mod.rs`:

Add to the `pub mod` declarations:

```rust
pub mod m006_letter_audiences;
```

Add to `all_migrations()` function:

```rust
Migration {
    version: 6,
    name: "letter_audiences",
    up: m006_letter_audiences::up,
},
```

- [ ] **Step 3: Run migration tests**

Run: `cargo test -p medical-db migrations::tests`
Expected: PASS (migration applies cleanly)

- [ ] **Step 4: Commit**

```bash
git add crates/db/src/migrations/
git commit -m "feat(db): add letter_audiences table migration

Seed 6 built-in audiences: patient, insurance, tax, specialist, employer, legal.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 3: Implement LetterAudiencesRepo

**Files:**
- Create: `crates/db/src/letter_audiences.rs`
- Modify: `crates/db/src/lib.rs`
- Test: `crates/db/src/letter_audiences.rs` (inline tests)

- [ ] **Step 1: Write failing tests**

Create `crates/db/src/letter_audiences.rs`:

```rust
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use uuid::Uuid;

use medical_core::types::LetterAudience;

use crate::{DbError, DbResult};

pub struct LetterAudiencesRepo;

impl LetterAudiencesRepo {
    pub fn list_all(conn: &Connection) -> DbResult<Vec<LetterAudience>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, system_prompt, user_template, is_builtin, created_at, updated_at
             FROM letter_audiences
             ORDER BY is_builtin DESC, name"
        )?;
        let rows = stmt.query_map([], Self::row_to_audience)?;
        let mut audiences = Vec::new();
        for row in rows {
            audiences.push(row?);
        }
        Ok(audiences)
    }

    pub fn get_by_id(conn: &Connection, id: &Uuid) -> DbResult<LetterAudience> {
        conn.query_row(
            "SELECT id, name, system_prompt, user_template, is_builtin, created_at, updated_at
             FROM letter_audiences WHERE id = ?1",
            [id.to_string()],
            Self::row_to_audience,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                DbError::NotFound(format!("letter audience {id}"))
            }
            other => DbError::Sqlite(other),
        })
    }

    pub fn upsert(conn: &Connection, audience: &LetterAudience) -> DbResult<()> {
        conn.execute(
            "INSERT INTO letter_audiences (id, name, system_prompt, user_template, is_builtin, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 system_prompt = excluded.system_prompt,
                 user_template = excluded.user_template,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                audience.id.to_string(),
                audience.name,
                audience.system_prompt,
                audience.user_template,
                audience.is_builtin as i32,
                audience.created_at.to_rfc3339(),
                audience.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &Uuid) -> DbResult<()> {
        // Check if builtin first
        let is_builtin: bool = conn
            .query_row(
                "SELECT is_builtin FROM letter_audiences WHERE id = ?1",
                [id.to_string()],
                |row| row.get::<_, i32>(0),
            )
            .map(|v| v != 0)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    DbError::NotFound(format!("letter audience {id}"))
                }
                other => DbError::Sqlite(other),
            })?;

        if is_builtin {
            return Err(DbError::Constraint(
                "Cannot delete built-in audience".to_string(),
            ));
        }

        let rows = conn.execute(
            "DELETE FROM letter_audiences WHERE id = ?1",
            [id.to_string()],
        )?;
        if rows == 0 {
            return Err(DbError::NotFound(format!("letter audience {id}")));
        }
        Ok(())
    }

    fn row_to_audience(row: &rusqlite::Row<'_>) -> rusqlite::Result<LetterAudience> {
        let id_str: String = row.get(0)?;
        let is_builtin_int: i32 = row.get(4)?;
        let created_str: String = row.get(5)?;
        let updated_str: String = row.get(6)?;

        Ok(LetterAudience {
            id: Uuid::parse_str(&id_str).unwrap_or_else(|_| Uuid::nil()),
            name: row.get(1)?,
            system_prompt: row.get(2)?,
            user_template: row.get(3)?,
            is_builtin: is_builtin_int != 0,
            created_at: DateTime::parse_from_rfc3339(&created_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: DateTime::parse_from_rfc3339(&updated_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    #[test]
    fn list_all_includes_builtins() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        let audiences = LetterAudiencesRepo::list_all(&conn).unwrap();
        assert_eq!(audiences.len(), 6);
        assert!(audiences.iter().all(|a| a.is_builtin));
    }

    #[test]
    fn upsert_and_get() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();

        let audience = LetterAudience::new(
            "Custom".to_string(),
            "Custom prompt".to_string(),
            Some("Custom template".to_string()),
        );

        LetterAudiencesRepo::upsert(&conn, &audience).unwrap();

        let fetched = LetterAudiencesRepo::get_by_id(&conn, &audience.id).unwrap();
        assert_eq!(fetched.name, "Custom");
        assert!(!fetched.is_builtin);
    }

    #[test]
    fn delete_builtin_fails() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        let audiences = LetterAudiencesRepo::list_all(&conn).unwrap();
        let builtin_id = audiences[0].id;

        let result = LetterAudiencesRepo::delete(&conn, &builtin_id);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DbError::Constraint(_)));
    }

    #[test]
    fn delete_custom_succeeds() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();

        let audience = LetterAudience::new(
            "Custom".to_string(),
            "Custom prompt".to_string(),
            None,
        );
        LetterAudiencesRepo::upsert(&conn, &audience).unwrap();

        LetterAudiencesRepo::delete(&conn, &audience.id).unwrap();

        let result = LetterAudiencesRepo::get_by_id(&conn, &audience.id);
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }
}
```

- [ ] **Step 2: Export from db lib.rs**

Modify `crates/db/src/lib.rs` to add:

```rust
pub mod letter_audiences;
pub use letter_audiences::LetterAudiencesRepo;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p medical-db letter_audiences --lib`
Expected: PASS (4 tests)

- [ ] **Step 4: Commit**

```bash
git add crates/db/src/letter_audiences.rs crates/db/src/lib.rs
git commit -m "feat(db): add LetterAudiencesRepo with CRUD operations

Includes protection against deleting built-in audiences.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 4: Update prompt builder to support audiences

**Files:**
- Modify: `crates/processing/src/document_generator.rs`

- [ ] **Step 1: Add audience parameter to build_letter_prompt**

Modify `crates/processing/src/document_generator.rs`:

Add this struct at the top of the file (after imports):

```rust
#[derive(Debug, Clone)]
pub struct LetterAudienceContext {
    pub name: String,
    pub system_prompt: String,
    pub user_template: Option<String>,
}
```

Modify the `build_letter_prompt` function signature and implementation:

```rust
/// Build `(system_prompt, user_prompt)` for generating patient correspondence.
pub fn build_letter_prompt(
    soap_note: &str,
    letter_type: &str,
    audience: Option<&LetterAudienceContext>,
    custom_template: Option<&str>,
) -> (String, String) {
    let time_date = format_now_for_prompt();

    // Resolution order:
    // 1. Audience with user_template -> use both
    // 2. Audience without user_template -> use audience system prompt, default user template
    // 3. No audience -> use legacy custom_template or defaults
    if let Some(audience) = audience {
        let system = audience.system_prompt.clone();

        let user = if let Some(user_template) = &audience.user_template {
            let mut placeholders = HashMap::new();
            placeholders.insert("letter_type", letter_type.to_string());
            placeholders.insert("soap_note", soap_note.to_string());
            placeholders.insert("time_date", time_date.clone());
            resolve_prompt(user_template, &placeholders)
        } else {
            // Fall back to default user template
            format!(
                "Please write a {letter_type} letter for the {audience_name} based on the following SOAP \
                 note:\n\n{time_date}\n\n{soap_note}",
                letter_type = letter_type,
                audience_name = audience.name,
                time_date = time_date,
                soap_note = soap_note,
            )
        };

        (system, user)
    } else {
        // Legacy behavior (backward compatible)
        let template = custom_template
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default_letter_prompt());

        let mut placeholders = HashMap::new();
        placeholders.insert("letter_type", letter_type.to_string());

        let system = resolve_prompt(template, &placeholders);

        let user = format!(
            "Please write a {letter_type} letter for the patient based on the following SOAP \
             note:\n\n{time_date}\n\n{soap_note}",
            letter_type = letter_type,
            time_date = time_date,
            soap_note = soap_note,
        );

        (system, user)
    }
}
```

- [ ] **Step 2: Add tests for audience parameter**

Add these tests to the `tests` module in `crates/processing/src/document_generator.rs`:

```rust
#[test]
fn letter_with_audience_uses_audience_prompts() {
    let soap = "S: Anxiety\nO: HR 90\nA: GAD\nP: CBT referral";
    let audience = LetterAudienceContext {
        name: "Insurance Company".to_string(),
        system_prompt: "Insurance-specific prompt".to_string(),
        user_template: Some("Write {letter_type} for insurance:\n\n{soap_note}".to_string()),
    };

    let (system, user) = build_letter_prompt(soap, "pre-auth", Some(&audience), None);

    assert_eq!(system, "Insurance-specific prompt");
    assert!(user.contains("pre-auth"));
    assert!(user.contains("Anxiety"));
    assert!(!user.contains("{letter_type}"));
    assert!(!user.contains("{soap_note}"));
}

#[test]
fn letter_with_audience_no_user_template_uses_default() {
    let soap = "S: Test";
    let audience = LetterAudienceContext {
        name: "Specialist".to_string(),
        system_prompt: "Specialist prompt".to_string(),
        user_template: None,
    };

    let (system, user) = build_letter_prompt(soap, "referral", Some(&audience), None);

    assert_eq!(system, "Specialist prompt");
    assert!(user.contains("Specialist"));
    assert!(user.contains("referral"));
    assert!(user.contains("Test"));
}

#[test]
fn letter_without_audience_uses_legacy_behavior() {
    let soap = "S: Test";
    let (system, user) = build_letter_prompt(soap, "follow-up", None, None);

    assert!(system.contains("patient-friendly"));
    assert!(user.contains("follow-up"));
    assert!(user.contains("Test"));
}

#[test]
fn letter_audience_ignores_custom_template() {
    let soap = "S: Test";
    let audience = LetterAudienceContext {
        name: "Legal".to_string(),
        system_prompt: "Legal prompt".to_string(),
        user_template: None,
    };
    let custom = "This should be ignored";

    let (system, _user) = build_letter_prompt(soap, "court", Some(&audience), Some(custom));

    assert_eq!(system, "Legal prompt");
    assert!(!system.contains("ignored"));
}
```

- [ ] **Step 3: Update existing tests**

Modify the existing `letter_default_contains_type` and `letter_custom_template_overrides` tests to pass `None` for the audience parameter:

```rust
#[test]
fn letter_default_contains_type() {
    let soap = "S: Anxiety\nO: HR 90\nA: GAD\nP: CBT referral";
    let (system, user) = build_letter_prompt(soap, "results", None, None);

    assert!(system.contains("results"));
    assert!(!system.contains("{letter_type}"));
    assert!(user.contains("Anxiety"));
    assert!(user.contains("Time") && user.contains("Date"));
}

#[test]
fn letter_custom_template_overrides() {
    let soap = "S: foo";
    let custom = "CUSTOM: {letter_type} letter";
    let (system, _user) = build_letter_prompt(soap, "follow-up", None, Some(custom));
    assert!(system.starts_with("CUSTOM: follow-up letter"));
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p medical-processing letter --lib`
Expected: PASS (all letter tests, including new audience tests)

- [ ] **Step 5: Commit**

```bash
git add crates/processing/src/document_generator.rs
git commit -m "feat(processing): add audience parameter to build_letter_prompt

Resolution order:
1. Audience with user_template -> use both
2. Audience without user_template -> use audience system prompt, default user template
3. No audience -> use legacy custom_template or defaults

Backward compatible: existing calls with audience=None work as before.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 2: Tauri Commands

### Task 5: Implement letter_audiences CRUD commands

**Files:**
- Create: `src-tauri/src/commands/letter_audiences.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Create letter_audiences command module**

Create `src-tauri/src/commands/letter_audiences.rs`:

```rust
use medical_core::error::{AppError, AppResult};
use medical_core::types::LetterAudience;
use medical_db::LetterAudiencesRepo;
use tracing::debug;
use uuid::Uuid;

use crate::state::AppState;

/// List all letter audiences (built-in + custom).
#[tauri::command]
pub async fn list_letter_audiences(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<LetterAudience>> {
    let conn = state.db.conn().map_err(|e| {
        AppError::Database(format!("Failed to get DB connection: {e}"))
    })?;

    LetterAudiencesRepo::list_all(&conn).map_err(|e| {
        AppError::Database(format!("Failed to list letter audiences: {e}"))
    })
}

/// Insert or update a letter audience.
/// If `audience.id` is nil (all zeros), generates a new UUID.
#[tauri::command]
pub async fn upsert_letter_audience(
    state: tauri::State<'_, AppState>,
    mut audience: LetterAudience,
) -> AppResult<LetterAudience> {
    // Generate UUID if nil
    if audience.id.is_nil() {
        audience.id = Uuid::new_v4();
        audience.created_at = chrono::Utc::now();
    }
    audience.updated_at = chrono::Utc::now();

    // Prevent creating new built-ins
    if audience.is_builtin {
        return Err(AppError::Other(
            "Cannot create custom audience with is_builtin=true".to_string(),
        ));
    }

    let conn = state.db.conn().map_err(|e| {
        AppError::Database(format!("Failed to get DB connection: {e}"))
    })?;

    LetterAudiencesRepo::upsert(&conn, &audience).map_err(|e| {
        AppError::Database(format!("Failed to upsert letter audience: {e}"))
    })?;

    debug!(audience_id = %audience.id, "upserted letter audience");

    Ok(audience)
}

/// Delete a letter audience. Built-in audiences cannot be deleted.
#[tauri::command]
pub async fn delete_letter_audience(
    state: tauri::State<'_, AppState>,
    id: Uuid,
) -> AppResult<()> {
    let conn = state.db.conn().map_err(|e| {
        AppError::Database(format!("Failed to get DB connection: {e}"))
    })?;

    LetterAudiencesRepo::delete(&conn, &id).map_err(|e| match e {
        medical_db::DbError::Constraint(msg) => AppError::Other(msg),
        medical_db::DbError::NotFound(msg) => AppError::Other(msg),
        other => AppError::Database(format!("Failed to delete letter audience: {other}")),
    })?;

    debug!(audience_id = %id, "deleted letter audience");

    Ok(())
}
```

- [ ] **Step 2: Export from commands module**

Modify `src-tauri/src/commands/mod.rs` to add:

```rust
pub mod letter_audiences;
```

- [ ] **Step 3: Register commands in lib.rs**

Modify `src-tauri/src/lib.rs` to add these three commands to the `invoke_handler!` macro:

```rust
commands::letter_audiences::list_letter_audiences,
commands::letter_audiences::upsert_letter_audience,
commands::letter_audiences::delete_letter_audience,
```

- [ ] **Step 4: Build and test**

Run: `cargo build -p rust-medical-assistant`
Expected: Compiles successfully

Run: `cargo test -p rust-medical-assistant --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/letter_audiences.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs
git commit -m "feat(tauri): add letter_audiences CRUD commands

Three commands: list, upsert, delete. Built-in audiences are protected
from deletion. UUID is generated server-side if nil.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 6: Update generate_letter command to accept audience_id

**Files:**
- Modify: `src-tauri/src/commands/generation/letter.rs`

- [ ] **Step 1: Add audience_id parameter**

Modify the `generate_letter` function signature in `src-tauri/src/commands/generation/letter.rs`:

```rust
#[tauri::command]
pub async fn generate_letter(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    letter_type: Option<String>,
    audience_id: Option<Uuid>,
) -> AppResult<String> {
    let _ = app.emit(
        "generation-progress",
        GenerationProgress {
            doc_type: "letter".into(),
            status: "started".into(),
            recording_id: recording_id.clone(),
        },
    );

    let result = generate_letter_inner(&state, &recording_id, letter_type.as_deref(), audience_id.as_ref()).await;

    match &result {
        Ok(_) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "letter".into(),
                    status: "completed".into(),
                    recording_id: recording_id.clone(),
                },
            );
        }
        Err(err) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "letter".into(),
                    status: format_progress_error(err),
                    recording_id: recording_id.clone(),
                },
            );
        }
    }

    result
}
```

- [ ] **Step 2: Update generate_letter_inner to fetch and use audience**

Modify `generate_letter_inner`:

```rust
async fn generate_letter_inner(
    state: &AppState,
    recording_id: &str,
    letter_type: Option<&str>,
    audience_id: Option<&Uuid>,
) -> AppResult<String> {
    let (mut recording, settings, config) =
        load_recording_and_settings(&state.db, recording_id).await?;

    // Pre-flight: probe the remote AI endpoint before doing any work.
    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::GenerateLetter,
        &config,
    )
    .await?;

    let provider = resolve_provider(state, &settings.ai_provider).await?;

    let soap_note = recording
        .soap_note
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::Processing(
                "Recording has no SOAP note. Generate a SOAP note first.".to_string(),
            )
        })?;

    if soap_note.len() > MAX_SOAP_NOTE_CHARS {
        return Err(AppError::Other(format!(
            "SOAP note too large: {} chars, limit is {}",
            soap_note.len(),
            MAX_SOAP_NOTE_CHARS
        )));
    }

    let ltype = letter_type.unwrap_or("follow-up");

    // Fetch audience if provided
    let audience_context = if let Some(id) = audience_id {
        let conn = state.db.conn().map_err(|e| {
            AppError::Database(format!("Failed to get DB connection: {e}"))
        })?;
        let audience = medical_db::LetterAudiencesRepo::get_by_id(&conn, id).map_err(|e| {
            AppError::Database(format!("Failed to fetch letter audience: {e}"))
        })?;

        Some(medical_processing::document_generator::LetterAudienceContext {
            name: audience.name,
            system_prompt: audience.system_prompt,
            user_template: audience.user_template,
        })
    } else {
        None
    };

    let (system_prompt, user_prompt) = document_generator::build_letter_prompt(
        soap_note,
        ltype,
        audience_context.as_ref(),
        settings.custom_letter_prompt.as_deref(),
    );

    debug!(
        "generate_letter: provider='{}', recording='{}', letter_type='{}', audience_id='{:?}'",
        provider.name(),
        recording_id,
        ltype,
        audience_id,
    );

    let request = build_completion_request(
        system_prompt,
        user_prompt,
        settings.model,
        settings.temperature,
        None,
    );

    let response = provider
        .complete(request)
        .await
        .map_err(|e| match e {
            AppError::EndpointOffline { .. } => e,
            _ => AppError::AiProvider(format!(
                "AI completion failed: {}",
                crate::commands::unwrap_app_error_message(e)
            )),
        })?;

    let letter_text = response.content;
    if letter_text.is_empty() {
        return Err(AppError::AiProvider(
            "AI returned an empty letter.".to_string(),
        ));
    }

    // Persist to DB (on blocking thread)
    recording.letter = Some(letter_text.clone());
    persist_recording(&state.db, recording).await?;

    Ok(letter_text)
}
```

- [ ] **Step 3: Update test to pass None for audience_id**

Modify the existing test in `src-tauri/src/commands/generation/letter.rs`:

```rust
#[tokio::test]
async fn generate_letter_returns_endpoint_offline_when_ai_unreachable() {
    // ... (existing setup code)

    let result = generate_letter_inner(
        &state,
        &recording_id,
        None, // letter_type
        None, // audience_id
    )
    .await;

    // ... (rest of test unchanged)
}
```

- [ ] **Step 4: Build and test**

Run: `cargo build -p rust-medical-assistant`
Expected: Compiles successfully

Run: `cargo test -p rust-medical-assistant generate_letter --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/generation/letter.rs
git commit -m "feat(tauri): add audience_id parameter to generate_letter command

When provided, fetches the audience from DB and passes it to the prompt
builder. Backward compatible: audience_id=None uses legacy behavior.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 3: Frontend Implementation

### Task 7: Create frontend API wrapper and types

**Files:**
- Create: `src/lib/types/letterAudience.ts`
- Create: `src/lib/api/letterAudiences.ts`
- Modify: `src/lib/api/generation.ts`

- [ ] **Step 1: Define LetterAudience type**

Create `src/lib/types/letterAudience.ts`:

```typescript
export interface LetterAudience {
  id: string;
  name: string;
  system_prompt: string;
  user_template: string | null;
  is_builtin: boolean;
  created_at: string;
  updated_at: string;
}
```

- [ ] **Step 2: Create API wrapper**

Create `src/lib/api/letterAudiences.ts`:

```typescript
import { invoke } from '@tauri-apps/api/core';
import type { LetterAudience } from '../types/letterAudience';

export async function listLetterAudiences(): Promise<LetterAudience[]> {
  return invoke('list_letter_audiences');
}

export async function upsertLetterAudience(
  audience: LetterAudience
): Promise<LetterAudience> {
  return invoke('upsert_letter_audience', { audience });
}

export async function deleteLetterAudience(id: string): Promise<void> {
  return invoke('delete_letter_audience', { id });
}
```

- [ ] **Step 3: Update generateLetter to accept audienceId**

Modify `src/lib/api/generation.ts`:

```typescript
export async function generateLetter(
  recordingId: string,
  letterType?: string,
  audienceId?: string
): Promise<string> {
  return invokeWithOfflineHandling('generate_letter', {
    recordingId,
    letterType: letterType ?? null,
    audienceId: audienceId ?? null,
  });
}
```

- [ ] **Step 4: Run frontend tests**

Run: `npx vitest run src/lib/api/generation.test.ts`
Expected: Update existing tests to pass `undefined` for `audienceId`

- [ ] **Step 5: Commit**

```bash
git add src/lib/types/letterAudience.ts src/lib/api/letterAudiences.ts src/lib/api/generation.ts
git commit -m "feat(frontend): add letter audience API wrapper and types

Three functions: listLetterAudiences, upsertLetterAudience, deleteLetterAudience.
Updated generateLetter to accept optional audienceId parameter.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 8: Create letter audiences state management

**Files:**
- Create: `src/lib/stores/letterAudiences.svelte.ts`

- [ ] **Step 1: Create runic store**

Create `src/lib/stores/letterAudiences.svelte.ts`:

```typescript
import type { LetterAudience } from '../types/letterAudience';
import {
  listLetterAudiences,
  upsertLetterAudience as apiUpsert,
  deleteLetterAudience as apiDelete,
} from '../api/letterAudiences';

function createLetterAudiencesStore() {
  let audiences = $state<LetterAudience[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function list() {
    loading = true;
    error = null;
    try {
      audiences = await listLetterAudiences();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load audiences';
      console.error('Failed to load letter audiences:', e);
    } finally {
      loading = false;
    }
  }

  async function upsert(audience: LetterAudience) {
    try {
      const updated = await apiUpsert(audience);
      const index = audiences.findIndex((a) => a.id === updated.id);
      if (index >= 0) {
        audiences[index] = updated;
      } else {
        audiences.push(updated);
      }
      return updated;
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Failed to save audience';
      error = msg;
      throw e;
    }
  }

  async function remove(id: string) {
    try {
      await apiDelete(id);
      audiences = audiences.filter((a) => a.id !== id);
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Failed to delete audience';
      error = msg;
      throw e;
    }
  }

  return {
    get audiences() {
      return audiences;
    },
    get loading() {
      return loading;
    },
    get error() {
      return error;
    },
    list,
    upsert,
    delete: remove,
  };
}

export const letterAudiences = createLetterAudiencesStore();
```

- [ ] **Step 2: Run type check**

Run: `npm run check`
Expected: PASS (no type errors)

- [ ] **Step 3: Commit**

```bash
git add src/lib/stores/letterAudiences.svelte.ts
git commit -m "feat(frontend): add letterAudiences runic store

Provides list, upsert, delete operations with loading and error state.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 9: Create LetterAudiences settings panel

**Files:**
- Create: `src/lib/components/settings/LetterAudiences.svelte`
- Modify: `src/lib/components/SettingsContent.svelte`

- [ ] **Step 1: Create LetterAudiences component**

Create `src/lib/components/settings/LetterAudiences.svelte`:

```svelte
<script lang="ts">
  import { letterAudiences } from '../../stores/letterAudiences.svelte';
  import { onMount } from 'svelte';
  import type { LetterAudience } from '../../types/letterAudience';

  let editingId = $state<string | null>(null);
  let editingName = $state('');
  let editingSystemPrompt = $state('');
  let editingUserTemplate = $state('');

  onMount(() => {
    letterAudiences.list();
  });

  function startEdit(audience?: LetterAudience) {
    if (audience) {
      editingId = audience.id;
      editingName = audience.name;
      editingSystemPrompt = audience.system_prompt;
      editingUserTemplate = audience.user_template ?? '';
    } else {
      editingId = 'new';
      editingName = '';
      editingSystemPrompt = '';
      editingUserTemplate = '';
    }
  }

  function cancelEdit() {
    editingId = null;
    editingName = '';
    editingSystemPrompt = '';
    editingUserTemplate = '';
  }

  async function saveEdit() {
    if (!editingName.trim()) {
      alert('Name is required');
      return;
    }

    const audience: LetterAudience = {
      id: editingId === 'new' ? '' : editingId!,
      name: editingName.trim(),
      system_prompt: editingSystemPrompt.trim(),
      user_template: editingUserTemplate.trim() || null,
      is_builtin: false,
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
    };

    try {
      await letterAudiences.upsert(audience);
      cancelEdit();
    } catch (e) {
      console.error('Failed to save audience:', e);
    }
  }

  async function handleDelete(id: string) {
    if (!confirm('Delete this audience?')) return;

    try {
      await letterAudiences.delete(id);
    } catch (e) {
      console.error('Failed to delete audience:', e);
    }
  }
</script>

<div class="letter-audiences-settings">
  <h3>Letter Audiences</h3>
  <p class="description">
    Configure system prompts and templates for different letter recipients.
  </p>

  {#if letterAudiences.loading}
    <p>Loading...</p>
  {:else}
    <div class="audiences-list">
      {#each letterAudiences.audiences as audience (audience.id)}
        <div class="audience-item" class:builtin={audience.is_builtin}>
          <div class="audience-info">
            <strong>{audience.name}</strong>
            {#if audience.is_builtin}
              <span class="badge">Built-in</span>
            {/if}
          </div>
          <div class="audience-actions">
            {#if !audience.is_builtin}
              <button onclick={() => startEdit(audience)}>Edit</button>
              <button onclick={() => handleDelete(audience.id)}>Delete</button>
            {:else}
              <button onclick={() => alert(audience.system_prompt)}>View Prompt</button>
            {/if}
          </div>
        </div>
      {/each}
    </div>

    {#if editingId}
      <div class="edit-form">
        <h4>{editingId === 'new' ? 'Add Custom Audience' : 'Edit Audience'}</h4>
        <label>
          Name
          <input type="text" bind:value={editingName} placeholder="e.g. Law Firm" />
        </label>
        <label>
          System Prompt
          <textarea
            bind:value={editingSystemPrompt}
            placeholder="You are a medical scribe assistant..."
            rows="4"
          ></textarea>
        </label>
        <label>
          User Template (optional)
          <textarea
            bind:value={editingUserTemplate}
            placeholder="Use {letter_type} and {soap_note} placeholders"
            rows="4"
          ></textarea>
          <small>
            Placeholders: <code>{`{letter_type}`}</code>, <code>{`{soap_note}`}</code>,
            <code>{`{time_date}`}</code>
          </small>
        </label>
        <div class="form-actions">
          <button onclick={cancelEdit}>Cancel</button>
          <button onclick={saveEdit}>Save</button>
        </div>
      </div>
    {:else}
      <button class="add-button" onclick={() => startEdit()}>
        Add Custom Audience
      </button>
    {/if}
  {/if}
</div>

<style>
  .letter-audiences-settings {
    padding: 1rem;
  }

  .description {
    color: var(--text-secondary);
    margin-bottom: 1rem;
  }

  .audiences-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-bottom: 1rem;
  }

  .audience-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.75rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-secondary);
  }

  .audience-item.builtin {
    opacity: 0.7;
  }

  .audience-info {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .badge {
    font-size: 0.75rem;
    padding: 0.125rem 0.5rem;
    background: var(--accent);
    color: white;
    border-radius: var(--radius-sm);
  }

  .audience-actions {
    display: flex;
    gap: 0.5rem;
  }

  .edit-form {
    padding: 1rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-secondary);
  }

  .edit-form label {
    display: block;
    margin-bottom: 0.75rem;
  }

  .edit-form input,
  .edit-form textarea {
    width: 100%;
    margin-top: 0.25rem;
    padding: 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-primary);
    color: var(--text-primary);
    font-family: inherit;
  }

  .edit-form small {
    display: block;
    margin-top: 0.25rem;
    color: var(--text-secondary);
  }

  .form-actions {
    display: flex;
    gap: 0.5rem;
    justify-content: flex-end;
  }

  .add-button {
    width: 100%;
    padding: 0.75rem;
    border: 2px dashed var(--border);
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
  }

  .add-button:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
```

- [ ] **Step 2: Add to SettingsContent**

Modify `src/lib/components/SettingsContent.svelte` to add a new tab for letter audiences. Find the section with vocabulary and context templates, and add:

```svelte
<button
  class="tab"
  class:active={activeTab === 'letter-audiences'}
  onclick={() => (activeTab = 'letter-audiences')}
>
  Letter Audiences
</button>
```

And in the content section:

```svelte
{#if activeTab === 'letter-audiences'}
  <LetterAudiences />
{/if}
```

Import the component at the top:

```svelte
import LetterAudiences from './settings/LetterAudiences.svelte';
```

- [ ] **Step 3: Run type check**

Run: `npm run check`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/lib/components/settings/LetterAudiences.svelte src/lib/components/SettingsContent.svelte
git commit -m "feat(frontend): add LetterAudiences settings panel

Allows viewing built-in audiences, creating/editing/deleting custom audiences.
Integrated into Settings dialog as a new tab.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 10: Add two-layer picker to GenerateTab

**Files:**
- Modify: `src/lib/pages/GenerateTab.svelte`

- [ ] **Step 1: Add audience selector and letter type input**

Modify `src/lib/pages/GenerateTab.svelte` to add state and UI for the two-layer picker. Add to the script section:

```typescript
import { letterAudiences } from '../stores/letterAudiences.svelte';
import { onMount } from 'svelte';

let selectedAudienceId = $state<string | null>(null);
let letterType = $state('follow-up');

onMount(() => {
  letterAudiences.list();
  // Default to "Patient" audience
  const patientAudience = letterAudiences.audiences.find((a) => a.id === 'builtin-patient');
  if (patientAudience) {
    selectedAudienceId = patientAudience.id;
  }
});
```

- [ ] **Step 2: Update handleGenerate to pass audienceId**

Modify the `handleGenerate` function:

```typescript
async function handleGenerate(type: 'soap' | 'referral' | 'letter') {
  if (!recordings.selectedRecording) return;
  const recordingId = recordings.selectedRecording.id;
  generation.startGenerating(type);
  try {
    if (type === 'soap') {
      // ... existing code
    } else if (type === 'referral') {
      // ... existing code
    } else if (type === 'letter') {
      await generateLetter(recordingId, letterType, selectedAudienceId ?? undefined);
    }
  } catch (e) {
    // ... existing error handling
  }
}
```

- [ ] **Step 3: Add UI controls to the letter card**

Find the "Patient Letter" GenerateItem in the template and add controls before it:

```svelte
<div class="letter-controls">
  <label class="audience-selector">
    <span>Audience</span>
    <select bind:value={selectedAudienceId}>
      {#each letterAudiences.audiences as audience (audience.id)}
        <option value={audience.id}>{audience.name}</option>
      {/each}
    </select>
  </label>
  <label class="letter-type-input">
    <span>Letter purpose</span>
    <input
      type="text"
      bind:value={letterType}
      placeholder="e.g. follow-up, pre-authorization, disability claim"
    />
  </label>
</div>

<GenerateItem
  title="Letter"
  description={letterAudiences.audiences.find((a) => a.id === selectedAudienceId)?.name ?? 'Patient'}
  generating={generation.state.generating === 'letter'}
  anyGenerating={generation.state.generating !== null}
  done={!!recordings.selectedRecording.letter}
  copyStatus={copyStatus['letter']}
  onGenerate={() => handleGenerate('letter')}
  onCopy={() => handleCopy('letter')}
  onSpeedRead={() => handleSpeedRead('letter')}
/>
```

- [ ] **Step 4: Add styles**

Add these styles to the `<style>` section:

```css
.letter-controls {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
  margin-bottom: 1rem;
}

.audience-selector,
.letter-type-input {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.audience-selector span,
.letter-type-input span {
  font-size: 0.875rem;
  font-weight: 600;
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.audience-selector select,
.letter-type-input input {
  padding: 0.5rem;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-family: inherit;
}
```

- [ ] **Step 5: Run type check and test**

Run: `npm run check`
Expected: PASS

Run: `npx vitest run`
Expected: PASS (update any failing tests to mock letterAudiences)

- [ ] **Step 6: Commit**

```bash
git add src/lib/pages/GenerateTab.svelte
git commit -m "feat(frontend): add two-layer picker to letter generation

Audience selector (defaults to Patient) and letter purpose input.
Both parameters passed to generateLetter API.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 4: Testing and Polish

### Task 11: Integration testing and documentation

- [ ] **Step 1: Manual testing checklist**

Test each built-in audience:
- Generate a letter with "Patient" audience, verify plain language
- Generate a letter with "Insurance Company" audience, verify medical necessity language and ICD/CPT references
- Generate a letter with "Tax Authority" audience, verify timeline and expense focus
- Generate a letter with "Specialist/Consultant" audience, verify clinical detail
- Generate a letter with "Employer/School" audience, verify accommodations focus and HIPAA-minimal
- Generate a letter with "Legal/Court" audience, verify formal tone and timeline

Test custom audience:
- Create a custom audience in Settings
- Generate a letter with the custom audience
- Verify custom prompt is used

Test backward compatibility:
- Generate a letter with no audience selected (or Patient as default)
- Verify it matches the old behavior

- [ ] **Step 2: Update README**

Add a section to README.md about letter audiences:

```markdown
### Letter Audiences

Generate letters tailored to different recipients:

- **Patient** — Plain language, empathetic
- **Insurance Company** — Medical necessity language, ICD/CPT codes
- **Tax Authority** — Expense justification, timeline
- **Specialist/Consultant** — Clinical detail, peer tone
- **Employer/School** — Accommodations, HIPAA-minimal
- **Legal/Court** — Formal opinion, timeline

Create custom audiences in **Settings → Letter Audiences** with your own system prompts and user templates.
```

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: add letter audiences section to README

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Success Criteria

- [ ] All 6 built-in audiences generate letters with appropriate tone and structure
- [ ] Custom audiences can be created, edited, deleted via Settings panel
- [ ] Two-layer picker (audience + letter purpose) works on GenerateTab
- [ ] Backward compatibility maintained (no audience selected = old behavior)
- [ ] All tests pass (backend and frontend)
- [ ] Type checking passes (`npm run check`)

## Notes

**Sync layer:** This plan implements the local-only feature. Paired-server sync (office server endpoints, client remote, routing logic) is a follow-up task that can be added after the core feature is stable. The sync pattern is well-established in the codebase (vocabulary, context templates) and can be added using the same approach.

**Prompt tuning:** The built-in audience prompts are starting points. Clinicians may want to customize them further based on their specific practice needs. The Settings panel allows this for custom audiences, but built-ins are read-only to maintain consistency.

**Testing:** Manual testing with real SOAP notes is essential to verify that the AI produces appropriate output for each audience. The prompt builder tests verify the plumbing, but the actual letter quality depends on the AI model and prompt wording.
