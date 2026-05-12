//! Orchestrates the training-corpus export pipeline.
//!
//! Filters promoted rows → applies redaction (static + per-recording
//! extensions) → writes JSONL + manifest + README to the
//! caller-supplied directory.

pub mod jsonl_writer;
pub mod manifest;
pub mod readme;

use medical_db::generations::{Generation, GenerationsRepo};
use medical_db::Connection;
use medical_security::phi_redactor::datetime::build_datetime_extension;
use medical_security::phi_redactor::names::build_patient_name_extension;
use medical_security::phi_redactor::{Extension, PhiRedactor};
use serde::Serialize;
use std::io::Write as _;
use std::path::PathBuf;

pub struct ExportOptions {
    pub output_dir: PathBuf,            // user-chosen
    pub base_model_filter: Vec<String>, // empty = all
    pub redaction_strictness: RedactionStrictness,
    pub ferri_scribe_version: String,
}

#[derive(Serialize, Debug, Clone, Copy)]
pub enum RedactionStrictness {
    Standard,
    Aggressive, // v2 — provider names + locations; v1 acts like Standard
}

pub struct ExportResult {
    pub corpus_dir: PathBuf,
    pub pairs_written: u32,
    pub warnings: Vec<manifest::Warning>,
}

/// Run the full export pipeline. Synchronous (caller should
/// spawn_blocking on this).
pub fn export(conn: &Connection, opts: ExportOptions) -> Result<ExportResult, String> {
    // 1. Build the output directory with a timestamp suffix.
    let timestamp = chrono::Utc::now().format("%Y-%m-%d-%H%M%S").to_string();
    let corpus_dir = opts.output_dir.join(format!("training-corpus-{timestamp}"));
    std::fs::create_dir_all(&corpus_dir).map_err(|e| format!("mkdir: {e}"))?;

    // 2. Pull all promoted rows with final_text NOT NULL, applying
    //    the base-model filter if any.
    let promoted: Vec<Generation> = fetch_promoted(conn, &opts.base_model_filter)?;

    // 3. Build per-recording redaction extensions:
    //    one Extension per recording's patient_name + the static
    //    datetime extension shared across all rows.
    let datetime_ext = build_datetime_extension();
    let mut warnings: Vec<manifest::Warning> = Vec::new();

    // 4. Generate records by collecting first to avoid borrow-check
    //    issues with the mutable warnings vec inside an iterator.
    let mut training_records: Vec<jsonl_writer::TrainingRecord> = Vec::new();
    let mut input_tokens_est: u64 = 0;
    let mut output_tokens_est: u64 = 0;

    for (idx, row) in promoted.iter().enumerate() {
        let pt_name_ext = lookup_patient_name(conn, &row.recording_id)
            .and_then(|n| build_patient_name_extension(&n));
        let mut extensions: Vec<Extension> = Vec::new();
        if let Some(e) = pt_name_ext {
            extensions.push(e);
        }
        extensions.push(datetime_ext.clone()); // Extension derives Clone

        let user_input = format_user_input(row);
        let redacted_user = PhiRedactor::redact_with(&user_input, &extensions);
        let redacted_final = PhiRedactor::redact_with(
            row.final_text.as_deref().unwrap_or(""),
            &extensions,
        );

        if PhiRedactor::contains_phi_with(&redacted_user, &extensions)
            || PhiRedactor::contains_phi_with(&redacted_final, &extensions)
        {
            warnings.push(manifest::Warning {
                row_index: idx as u32,
                reason: "residual PHI detected after redaction".to_string(),
            });
        }

        input_tokens_est += manifest::estimate_tokens(&redacted_user);
        output_tokens_est += manifest::estimate_tokens(&redacted_final);

        training_records.push(jsonl_writer::TrainingRecord {
            system: row
                .prompt_template_name
                .clone()
                .unwrap_or_else(|| "soap".to_string()),
            user: redacted_user,
            assistant: redacted_final,
        });
    }

    // 5. Write JSONL.
    let jsonl_path = corpus_dir.join("train.jsonl");
    let file =
        std::fs::File::create(&jsonl_path).map_err(|e| format!("create train.jsonl: {e}"))?;
    let mut writer = std::io::BufWriter::new(file);
    let pairs = jsonl_writer::write_jsonl(&mut writer, training_records)
        .map_err(|e| format!("write_jsonl: {e}"))?;
    writer.flush().map_err(|e| format!("flush: {e}"))?;

    // 6. Manifest.
    let m = manifest::Manifest {
        schema_version: 1,
        exported_at: chrono::Utc::now().to_rfc3339(),
        ferri_scribe_version: opts.ferri_scribe_version.clone(),
        corpus_size: manifest::CorpusSize {
            pairs: pairs as u32,
            input_tokens_est,
            output_tokens_est,
        },
        base_model_filter: opts.base_model_filter.clone(),
        prompt_template_filter: vec![], // v1: not filtered separately
        redaction_strictness: match opts.redaction_strictness {
            RedactionStrictness::Standard => "standard".to_string(),
            RedactionStrictness::Aggressive => "aggressive".to_string(),
        },
        redaction_rules_applied: vec![
            "SSN".into(),
            "PHONE".into(),
            "EMAIL".into(),
            "DOB".into(),
            "MRN".into(),
            "ADDRESS".into(),
            "ZIP".into(),
            "PT_NAME".into(),
            "DATE".into(),
        ],
        warnings: warnings.clone(),
    };
    manifest::write_manifest(&m, &corpus_dir.join("manifest.json"))
        .map_err(|e| format!("write manifest: {e}"))?;

    // 7. README.
    let readme_text = readme::render_readme(
        pairs as u32,
        &opts.base_model_filter,
        &opts.ferri_scribe_version,
    );
    std::fs::write(corpus_dir.join("README.md"), readme_text)
        .map_err(|e| format!("write readme: {e}"))?;

    tracing::info!(
        pairs = pairs,
        warnings = warnings.len(),
        "corpus export complete"
    );

    Ok(ExportResult {
        corpus_dir,
        pairs_written: pairs as u32,
        warnings,
    })
}

fn fetch_promoted(
    conn: &Connection,
    model_filter: &[String],
) -> Result<Vec<Generation>, String> {
    // Page all promoted rows; v1 expects at most a few thousand so
    // a single paginated loop with limit 200 is sufficient.
    let mut all: Vec<Generation> = Vec::new();
    let mut offset: u32 = 0;
    loop {
        let (page, _total) = GenerationsRepo::list_by_status(conn, "promoted", 200, offset)
            .map_err(|e| format!("list_by_status: {e}"))?;
        if page.is_empty() {
            break;
        }
        let n = page.len() as u32;
        all.extend(page);
        offset += n;
    }
    // Filter to final_text IS NOT NULL and (optionally) by model.
    let filtered: Vec<Generation> = all
        .into_iter()
        .filter(|g| g.final_text.is_some())
        .filter(|g| {
            model_filter.is_empty() || model_filter.iter().any(|m| m == &g.ai_model)
        })
        .collect();
    Ok(filtered)
}

fn lookup_patient_name(conn: &Connection, recording_id: &uuid::Uuid) -> Option<String> {
    conn.query_row(
        "SELECT patient_name FROM recordings WHERE id = ?",
        [recording_id.to_string()],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
    .filter(|s| !s.trim().is_empty())
}

fn format_user_input(row: &Generation) -> String {
    // For v1: concatenate transcript + context_json. The fine-tune
    // sees the same input shape as the SOAP generation pipeline.
    let mut s = row.input_transcript.clone();
    if let Some(ctx) = &row.input_context_json {
        if !ctx.trim().is_empty() && ctx != "null" {
            s.push_str("\n\n[Context]\n");
            s.push_str(ctx);
        }
    }
    s
}
