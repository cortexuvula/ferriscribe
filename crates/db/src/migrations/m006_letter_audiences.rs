//! Migration 006: `letter_audiences` table for audience-specific letter generation.
//!
//! Stores system prompts and user templates for different letter recipients
//! (patient, insurance, tax, etc.). Includes 6 seeded built-in rows.

use rusqlite::Connection;

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

    // Seed 6 built-in audiences (using valid UUIDs so the repo can reference them)
    let now = "2026-05-22T00:00:00Z";
    let builtins: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "00000000-0000-0000-0000-000000000001",
            "Patient",
            "You are a medical scribe assistant helping to write patient-friendly correspondence. Use clear, plain language the patient can understand. Avoid unexplained medical jargon. Be empathetic and professional.",
            None,
        ),
        (
            "00000000-0000-0000-0000-000000000002",
            "Insurance Company",
            "You are a medical scribe assistant writing formal correspondence for insurance companies. Use precise medical necessity language, reference ICD-10 and CPT codes where applicable, and structure the letter to justify medical necessity for the requested service or treatment.",
            Some("Please write a {letter_type} letter for the insurance company based on the following SOAP note. Include a medical necessity statement, relevant diagnosis codes (ICD-10), and procedure codes (CPT) if applicable:\n\n{time_date}\n\n{soap_note}"),
        ),
        (
            "00000000-0000-0000-0000-000000000003",
            "Tax Authority",
            "You are a medical scribe assistant writing correspondence for tax authorities or disability benefit agencies. Focus on factual timeline, expense justification, and medical necessity. Use formal, objective language.",
            Some("Please write a {letter_type} letter for the tax authority based on the following SOAP note. Include service dates, cost justification, and medical necessity for the expenses or disability claim:\n\n{time_date}\n\n{soap_note}"),
        ),
        (
            "00000000-0000-0000-0000-000000000004",
            "Specialist/Consultant",
            "You are a medical scribe assistant writing professional referral correspondence to a specialist or consultant. Use clinical detail, professional peer tone, and include relevant history, findings, and specific questions for the consultant.",
            Some("Please write a {letter_type} referral letter to the specialist based on the following SOAP note. Include relevant medical history, objective findings, and specific questions or requests for the consultant:\n\n{time_date}\n\n{soap_note}"),
        ),
        (
            "00000000-0000-0000-0000-000000000005",
            "Employer/School",
            "You are a medical scribe assistant writing correspondence for employers or educational institutions. Focus on functional limitations, recommended accommodations, and fitness-for-duty. Keep medical details minimal and HIPAA-compliant.",
            Some("Please write a {letter_type} letter for the employer or school based on the following SOAP note. Focus on functional limitations and recommended accommodations. Avoid unnecessary medical details:\n\n{time_date}\n\n{soap_note}"),
        ),
        (
            "00000000-0000-0000-0000-000000000006",
            "Legal/Court",
            "You are a medical scribe assistant writing formal medical opinion letters for legal proceedings or court. Use objective, factual language. Include chronological timeline, clinical findings, and professional medical opinion.",
            Some("Please write a {letter_type} letter for legal or court purposes based on the following SOAP note. Include a chronological timeline, objective clinical findings, and your professional medical opinion:\n\n{time_date}\n\n{soap_note}"),
        ),
    ];

    for (id, name, system_prompt, user_template) in builtins {
        conn.execute(
            "INSERT OR IGNORE INTO letter_audiences (id, name, system_prompt, user_template, is_builtin, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
            rusqlite::params![id, name, system_prompt, user_template, now, now],
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn in_memory() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn creates_table() {
        let conn = in_memory();
        super::up(&conn).expect("migration should succeed");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='letter_audiences'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        assert!(count > 0, "letter_audiences table should exist");
    }

    #[test]
    fn seeds_six_builtins() {
        let conn = in_memory();
        super::up(&conn).expect("migration");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM letter_audiences", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 6, "should have exactly 6 built-in audiences");
    }

    #[test]
    fn builtin_names_correct() {
        let conn = in_memory();
        super::up(&conn).expect("migration");
        let mut stmt = conn
            .prepare("SELECT name FROM letter_audiences ORDER BY name")
            .expect("prepare");
        let names: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .expect("query")
            .map(|r| r.expect("row"))
            .collect();
        assert_eq!(
            names,
            vec![
                "Employer/School",
                "Insurance Company",
                "Legal/Court",
                "Patient",
                "Specialist/Consultant",
                "Tax Authority",
            ]
        );
    }

    #[test]
    fn idempotent_seeds() {
        let conn = in_memory();
        super::up(&conn).expect("first migration");
        super::up(&conn).expect("second migration should not fail");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM letter_audiences", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 6, "re-running should not duplicate seeds");
    }

    #[test]
    fn patient_has_no_user_template() {
        let conn = in_memory();
        super::up(&conn).expect("migration");
        let template: Option<String> = conn
            .query_row(
                "SELECT user_template FROM letter_audiences WHERE id='00000000-0000-0000-0000-000000000001'",
                [],
                |r| r.get(0),
            )
            .expect("query");
        assert!(template.is_none(), "Patient audience should have NULL user_template");
    }

    #[test]
    fn insurance_has_user_template() {
        let conn = in_memory();
        super::up(&conn).expect("migration");
        let template: Option<String> = conn
            .query_row(
                "SELECT user_template FROM letter_audiences WHERE id='00000000-0000-0000-0000-000000000002'",
                [],
                |r| r.get(0),
            )
            .expect("query");
        assert!(template.is_some(), "Insurance audience should have a user_template");
        let t = template.unwrap();
        assert!(t.contains("{letter_type}"), "template should contain letter_type placeholder");
        assert!(t.contains("{soap_note}"), "template should contain soap_note placeholder");
    }
}
