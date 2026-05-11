# Training Corpus — Phase 3: Export Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the clinician export their promoted corpus to a redacted JSONL training file. Builds on Phase 2: takes rows where `corpus_status='promoted' AND final_text IS NOT NULL`, applies an extended PhiRedactor (patient names + datetimes), emits OpenAI chat-completion JSONL + manifest + README to a user-chosen directory.

**Architecture:** Three new pieces of backend (extended `PhiRedactor`, a `corpus_export` module, a Tauri command) plus an export dialog UI in the existing Promoted view. The export runs synchronously inside `tokio::task::spawn_blocking` (file I/O + regex) and reports progress via Tauri events on long-running jobs. Output is a single timestamped directory containing `train.jsonl` + `manifest.json` + `README.md`.

**Tech Stack:** Rust workspace (extend existing `PhiRedactor`, new module under `src-tauri`), Svelte 5 (modal dialog). Uses existing `tauri-plugin-dialog` for the directory chooser (verify availability). No new crates.

**Spec reference:** `docs/superpowers/specs/2026-05-11-training-corpus-design.md` — Phase 3 (Export).

**Depends on:** Phase 1 (Capture) and Phase 2 (Curate). The `generations` table must exist with promoted rows.

---

## File Structure

**Created:**
- `crates/security/src/phi_redactor/names.rs` — patient-name redaction extension
- `crates/security/src/phi_redactor/datetime.rs` — datetime + visit-date redaction extension
- `src-tauri/src/corpus_export/mod.rs` — orchestration (filter rows, redact, write)
- `src-tauri/src/corpus_export/jsonl_writer.rs` — JSONL emitter (OpenAI chat-completion format)
- `src-tauri/src/corpus_export/manifest.rs` — manifest.json builder
- `src-tauri/src/corpus_export/readme.rs` — README.md template
- `src-tauri/src/commands/training_corpus_export.rs` — Tauri command
- `src/lib/components/settings/training_corpus/ExportDialog.svelte` — modal UI

**Modified:**
- `crates/security/src/phi_redactor.rs` — module restructure to host the extension submodules
- `crates/security/src/lib.rs` — re-export if needed
- `src-tauri/src/lib.rs` — register the new module and command
- `src/lib/components/settings/training_corpus/PromotedList.svelte` — add "Export training corpus" button

**No other files touched.**

---

## Task 1: Restructure `PhiRedactor` to be extensible

**Files:**
- Modify: `crates/security/src/phi_redactor.rs` (move pattern list to a registry, add a `with_extensions` constructor)

The current `PhiRedactor::redact` uses module-static patterns. For the corpus export we need per-export patterns (patient names from the specific recording's `patient_name` column). Refactor to support both module-static defaults and per-call extensions, without breaking existing callers.

### Steps

- [ ] **Step 1: Add an extension struct**

  Modify `crates/security/src/phi_redactor.rs` to expose:

  ```rust
  /// A compiled extension pattern that can be added to a redaction
  /// pass. Built per-export from the recording's patient_name and any
  /// other per-corpus identifiers.
  pub struct Extension {
      pub regex: Regex,
      pub placeholder: &'static str,
  }

  impl PhiRedactor {
      /// Same as `redact`, but applies the given extensions first.
      /// Extensions run before the static patterns so a patient name
      /// like "John Smith" gets replaced with [PT_NAME] before the
      /// static EMAIL pattern could try to match an email containing
      /// "smith". (Defense-in-depth ordering.)
      pub fn redact_with(text: &str, extensions: &[Extension]) -> String {
          let mut result = text.to_string();
          for ext in extensions {
              result = ext.regex.replace_all(&result, ext.placeholder).into_owned();
          }
          for pattern in PATTERNS.iter() {
              result = pattern.regex.replace_all(&result, pattern.placeholder).into_owned();
          }
          result
      }

      /// Same predicate as `contains_phi`, but checks both static
      /// patterns and the supplied extensions.
      pub fn contains_phi_with(text: &str, extensions: &[Extension]) -> bool {
          extensions.iter().any(|e| e.regex.is_match(text))
              || PATTERNS.iter().any(|p| p.regex.is_match(text))
      }
  }
  ```

- [ ] **Step 2: Add a test for the new methods**

  Append to the existing test block:

  ```rust
  #[test]
  fn redact_with_extensions_runs_extensions_first() {
      let ext = Extension {
          regex: Regex::new(r"(?i)\bJohn Smith\b").unwrap(),
          placeholder: "[PT_NAME]",
      };
      let input = "Mr. John Smith was seen for follow-up; reach him at john.smith@example.com.";
      let out = PhiRedactor::redact_with(input, &[ext]);
      assert!(out.contains("[PT_NAME]"), "name should be redacted: {out}");
      assert!(out.contains("[EMAIL]"), "email should be redacted: {out}");
      assert!(!out.contains("John Smith"), "raw name leaked: {out}");
  }

  #[test]
  fn redact_with_empty_extensions_matches_redact() {
      let input = "Call (555) 867-5309.";
      let a = PhiRedactor::redact(input);
      let b = PhiRedactor::redact_with(input, &[]);
      assert_eq!(a, b);
  }
  ```

- [ ] **Step 3: Run tests**

  Run: `cargo test -p medical-security --lib phi_redactor`
  Expected: existing tests still pass + 2 new pass.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/security/src/phi_redactor.rs
  git commit -m "feat(security): PhiRedactor::redact_with for per-call extensions

  Adds Extension struct + redact_with(text, extensions) so the corpus
  export can layer per-recording patient-name patterns on top of the
  static defaults. Extensions run first so they win over generic
  matches. Existing redact() / contains_phi() unchanged."
  ```

---

## Task 2: Patient-name extension builder

**Files:**
- Create: `crates/security/src/phi_redactor/names.rs`
- Modify: `crates/security/src/phi_redactor.rs` (declare and re-export the submodule)

Note: if `phi_redactor.rs` is currently a single file, you'll need to convert it to a directory (`phi_redactor/mod.rs` + `phi_redactor/names.rs`) OR keep both as siblings (`phi_redactor.rs` and `phi_redactor_names.rs`). Project convention varies — read existing examples in the security crate. The simplest path: put the new code in `phi_redactor.rs` directly as a `pub mod names` inline.

For this plan, assume INLINE submodule (`pub mod names { ... }` inside `phi_redactor.rs`).

### Steps

- [ ] **Step 1: Write failing tests**

  Append to the existing test block in `crates/security/src/phi_redactor.rs`:

  ```rust
  #[cfg(test)]
  mod names_tests {
      use super::*;

      #[test]
      fn build_patient_name_extension_handles_full_name() {
          let ext = names::build_patient_name_extension("Jane Smith")
              .expect("should build extension");
          let out = PhiRedactor::redact_with("Jane Smith presents with cough.", &[ext]);
          assert!(out.contains("[PT_NAME]"));
          assert!(!out.contains("Jane Smith"));
      }

      #[test]
      fn build_patient_name_extension_handles_possessive() {
          let ext = names::build_patient_name_extension("Jane Smith").unwrap();
          let out = PhiRedactor::redact_with("Reviewed Jane Smith's results today.", &[ext]);
          assert!(out.contains("[PT_NAME]"), "{out}");
      }

      #[test]
      fn build_patient_name_extension_handles_first_only() {
          let ext = names::build_patient_name_extension("Jane Smith").unwrap();
          let out = PhiRedactor::redact_with("Jane is doing well.", &[ext]);
          // First-name-only should still match.
          assert!(out.contains("[PT_NAME]"), "{out}");
      }

      #[test]
      fn build_patient_name_extension_handles_last_only_with_title() {
          let ext = names::build_patient_name_extension("Jane Smith").unwrap();
          let out = PhiRedactor::redact_with("Mrs. Smith returns for follow-up.", &[ext]);
          assert!(out.contains("[PT_NAME]"), "{out}");
      }

      #[test]
      fn build_patient_name_extension_returns_none_for_empty() {
          assert!(names::build_patient_name_extension("").is_none());
          assert!(names::build_patient_name_extension("   ").is_none());
      }

      #[test]
      fn build_patient_name_extension_does_not_match_unrelated_text() {
          let ext = names::build_patient_name_extension("Jane Smith").unwrap();
          let out = PhiRedactor::redact_with("Patient denied chest pain.", &[ext]);
          assert_eq!(out, "Patient denied chest pain.");
      }
  }
  ```

- [ ] **Step 2: Implement the extension builder**

  Add to `crates/security/src/phi_redactor.rs` (next to the existing types):

  ```rust
  /// Per-recording patient-name pattern construction. Builds a single
  /// Extension that matches the full name, possessive form, first
  /// name alone, and last name preceded by a salutation. Returns
  /// None if the input is empty/whitespace-only.
  pub mod names {
      use super::{Extension, Regex};

      pub fn build_patient_name_extension(patient_name: &str) -> Option<Extension> {
          let trimmed = patient_name.trim();
          if trimmed.is_empty() {
              return None;
          }
          let parts: Vec<&str> = trimmed.split_whitespace().collect();
          if parts.is_empty() {
              return None;
          }

          // Escape each token, then assemble three alternatives:
          //   1) Full name (with possessive): "First Last('s)?"
          //   2) First alone (word-boundary)
          //   3) Salutation Last: "(Mr|Mrs|Ms|Dr|Miss) Last"
          let escape = |s: &str| regex::escape(s);
          let mut alts: Vec<String> = Vec::new();

          let full = parts
              .iter()
              .map(|p| escape(p))
              .collect::<Vec<_>>()
              .join(r"\s+");
          alts.push(format!(r"\b{full}(?:'s)?\b"));

          if parts.len() >= 1 {
              alts.push(format!(r"\b{}(?:'s)?\b", escape(parts[0])));
          }
          if parts.len() >= 2 {
              let last = parts.last().unwrap();
              alts.push(format!(
                  r"\b(?:Mr|Mrs|Ms|Miss|Dr)\.?\s+{}(?:'s)?\b",
                  escape(last)
              ));
          }

          let combined = format!(r"(?i)(?:{})", alts.join("|"));
          let regex = Regex::new(&combined).ok()?;
          Some(Extension { regex, placeholder: "[PT_NAME]" })
      }
  }
  ```

- [ ] **Step 3: Run tests**

  Run: `cargo test -p medical-security --lib phi_redactor::names_tests`
  Expected: 6/6 pass. Also run the broader `phi_redactor` test suite to confirm no regressions.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/security/src/phi_redactor.rs
  git commit -m "feat(security): patient-name redaction extension builder

  names::build_patient_name_extension(name) produces a single
  Extension that matches the full name with optional possessive,
  the first name alone, and salutation+last forms (Mr/Mrs/Ms/Miss/Dr).
  Used by the corpus export pipeline. Returns None for empty input."
  ```

---

## Task 3: Datetime extension

**Files:**
- Modify: `crates/security/src/phi_redactor.rs` (add a `datetime` submodule next to `names`)

### Steps

- [ ] **Step 1: Write tests**

  Append:

  ```rust
  #[cfg(test)]
  mod datetime_tests {
      use super::*;

      #[test]
      fn datetime_extension_redacts_iso_format() {
          let ext = datetime::build_datetime_extension();
          let out = PhiRedactor::redact_with("Visit on 2026-05-11 14:30:00.", &[ext]);
          assert!(out.contains("[DATETIME]"), "{out}");
      }

      #[test]
      fn datetime_extension_redacts_us_short_date() {
          let ext = datetime::build_datetime_extension();
          let out = PhiRedactor::redact_with("Surgery scheduled 05/15/2026.", &[ext]);
          assert!(out.contains("[DATE]"), "{out}");
      }

      #[test]
      fn datetime_extension_does_not_redact_clinical_numbers() {
          let ext = datetime::build_datetime_extension();
          let cases = ["BP 120/80", "98.6 F", "Lab 5/15 reactive"];
          for c in cases {
              let out = PhiRedactor::redact_with(c, &[ext.clone()]);
              assert_eq!(out, c, "clinical number wrongly redacted: {c} -> {out}");
          }
      }
  }
  ```

  Note: the test `datetime_extension_does_not_redact_clinical_numbers` is a sharp constraint — clinical values like "5/15 reactive" or "BP 120/80" use slash-separated numbers but are NOT dates. The implementation needs to be specific enough to avoid these. If you can't satisfy this without false negatives on real dates, document the trade-off and skip the over-strict assertion (replace with a soft check).

- [ ] **Step 2: Implement**

  Add to `phi_redactor.rs`:

  ```rust
  pub mod datetime {
      use super::{Extension, Regex};

      pub fn build_datetime_extension() -> Extension {
          // Conservative: match ISO datetime first, then specific
          // unambiguous date formats. Avoid bare MM/DD which collides
          // with clinical fractions.
          // - ISO datetime: 2026-05-11T14:30:00 or 2026-05-11 14:30:00
          // - ISO date alone: 2026-05-11 (requires 4-digit year)
          // - US short date: MM/DD/YYYY (requires 4-digit year)
          // - Long English: "May 11, 2026"
          let pat = r"(?ix)
              \b
              (?:
                  \d{4}-\d{2}-\d{2}(?:[T\s]\d{2}:\d{2}(?::\d{2})?)?     # ISO date(+time)
                  |
                  \d{1,2}/\d{1,2}/\d{4}                                 # US short date with 4-digit year
                  |
                  (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{1,2},?\s+\d{4}  # Long form
              )
              \b
          ";
          let regex = Regex::new(pat).expect("hardcoded datetime regex");
          Extension { regex, placeholder: "[DATE]" }
      }
  }
  ```

  This implementation uses `[DATE]` as the placeholder. Update the test that expected `[DATETIME]` to expect `[DATE]` instead (consistent single placeholder), OR change the placeholder to `[DATETIME]`. Pick one and apply uniformly.

  **Important:** `Extension` is currently NOT `Clone`. The test above uses `ext.clone()`. Add `#[derive(Clone)]` to `Extension` (and `Clone` on the underlying `Regex` is already `Clone`-friendly), OR remove the loop's `ext.clone()` and just call `build_datetime_extension()` once per case.

- [ ] **Step 3: Run tests**

  Run: `cargo test -p medical-security --lib phi_redactor::datetime_tests`
  Expected: pass. Adjust the "doesn't redact clinical numbers" test if needed — see test note above.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/security/src/phi_redactor.rs
  git commit -m "feat(security): datetime redaction extension

  Matches ISO datetimes/dates, US short dates with 4-digit years
  (avoids clinical fraction collision), and long English dates.
  Single [DATE] placeholder. Used by the corpus export pipeline."
  ```

---

## Task 4: Corpus export orchestration module

**Files:**
- Create: `src-tauri/src/corpus_export/mod.rs`
- Create: `src-tauri/src/corpus_export/jsonl_writer.rs`
- Create: `src-tauri/src/corpus_export/manifest.rs`
- Create: `src-tauri/src/corpus_export/readme.rs`
- Modify: `src-tauri/src/lib.rs` (declare the module)

### Steps

- [ ] **Step 1: JSONL writer**

  Create `src-tauri/src/corpus_export/jsonl_writer.rs`:

  ```rust
  //! Writes the OpenAI chat-completion JSONL format.
  //!
  //! Each line is one JSON object:
  //! {"messages":[
  //!   {"role":"system","content":"<prompt template name>"},
  //!   {"role":"user","content":"<redacted transcript>"},
  //!   {"role":"assistant","content":"<redacted final SOAP>"}
  //! ]}

  use serde::Serialize;
  use std::io::Write;

  #[derive(Serialize)]
  struct Message<'a> {
      role: &'a str,
      content: &'a str,
  }

  #[derive(Serialize)]
  struct Record<'a> {
      messages: Vec<Message<'a>>,
  }

  pub fn write_jsonl<W: Write>(
      writer: &mut W,
      records: impl IntoIterator<Item = TrainingRecord>,
  ) -> std::io::Result<usize> {
      let mut count = 0usize;
      for r in records {
          let record = Record {
              messages: vec![
                  Message { role: "system", content: &r.system },
                  Message { role: "user", content: &r.user },
                  Message { role: "assistant", content: &r.assistant },
              ],
          };
          let line = serde_json::to_string(&record)
              .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
          writer.write_all(line.as_bytes())?;
          writer.write_all(b"\n")?;
          count += 1;
      }
      Ok(count)
  }

  pub struct TrainingRecord {
      pub system: String,
      pub user: String,
      pub assistant: String,
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn writes_one_record_per_line() {
          let mut buf: Vec<u8> = Vec::new();
          let records = vec![
              TrainingRecord {
                  system: "soap".to_string(),
                  user: "transcript A".to_string(),
                  assistant: "note A".to_string(),
              },
              TrainingRecord {
                  system: "soap".to_string(),
                  user: "transcript B".to_string(),
                  assistant: "note B".to_string(),
              },
          ];
          let n = write_jsonl(&mut buf, records).unwrap();
          assert_eq!(n, 2);
          let s = String::from_utf8(buf).unwrap();
          assert_eq!(s.lines().count(), 2);
          for line in s.lines() {
              let _: serde_json::Value = serde_json::from_str(line).expect("each line must be valid JSON");
          }
      }
  }
  ```

- [ ] **Step 2: Manifest builder**

  Create `src-tauri/src/corpus_export/manifest.rs`:

  ```rust
  //! Builds the manifest.json that accompanies a training-corpus export.

  use serde::Serialize;

  #[derive(Serialize)]
  pub struct Manifest {
      pub schema_version: u32,
      pub exported_at: String,                // RFC3339
      pub ferri_scribe_version: String,
      pub corpus_size: CorpusSize,
      pub base_model_filter: Vec<String>,
      pub prompt_template_filter: Vec<String>,
      pub redaction_strictness: String,       // 'standard' | 'aggressive'
      pub redaction_rules_applied: Vec<String>,
      pub warnings: Vec<Warning>,
  }

  #[derive(Serialize)]
  pub struct CorpusSize {
      pub pairs: u32,
      pub input_tokens_est: u64,
      pub output_tokens_est: u64,
  }

  #[derive(Serialize)]
  pub struct Warning {
      pub row_index: u32,
      pub reason: String,
  }

  /// Cheap token estimate: ~1 token per 4 characters of UTF-8 text.
  /// Accurate enough for the manifest's "estimated tokens" field.
  pub fn estimate_tokens(s: &str) -> u64 {
      (s.chars().count() as f64 / 4.0).ceil() as u64
  }

  pub fn write_manifest(manifest: &Manifest, path: &std::path::Path) -> std::io::Result<()> {
      let json = serde_json::to_string_pretty(manifest)
          .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
      std::fs::write(path, json)
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn estimate_tokens_basic() {
          assert_eq!(estimate_tokens(""), 0);
          assert_eq!(estimate_tokens("abcd"), 1);
          assert_eq!(estimate_tokens("abcde"), 2);
      }

      #[test]
      fn manifest_serializes_with_pretty_format() {
          let m = Manifest {
              schema_version: 1,
              exported_at: "2026-05-11T22:30:00Z".to_string(),
              ferri_scribe_version: "0.10.56".to_string(),
              corpus_size: CorpusSize { pairs: 10, input_tokens_est: 1000, output_tokens_est: 500 },
              base_model_filter: vec!["llama3:70b".to_string()],
              prompt_template_filter: vec!["soap-default".to_string()],
              redaction_strictness: "standard".to_string(),
              redaction_rules_applied: vec!["SSN".to_string(), "PT_NAME".to_string()],
              warnings: vec![],
          };
          let json = serde_json::to_string_pretty(&m).unwrap();
          assert!(json.contains("\"schema_version\""));
          assert!(json.contains("\"pairs\": 10"));
      }
  }
  ```

- [ ] **Step 3: README generator**

  Create `src-tauri/src/corpus_export/readme.rs`:

  ```rust
  //! Generates the README.md that accompanies a training-corpus export.
  //!
  //! Documents the JSONL format, redaction caveats, and points the
  //! clinician at typical fine-tuning recipes.

  pub fn render_readme(
      pair_count: u32,
      base_models: &[String],
      ferri_scribe_version: &str,
  ) -> String {
      let models = if base_models.is_empty() {
          "(unspecified)".to_string()
      } else {
          base_models.join(", ")
      };
      format!(
          r#"# Training corpus export

Exported from FerriScribe v{ferri_scribe_version}.

## Contents

- `train.jsonl` — {pair_count} training pairs in OpenAI chat-completion format.
- `manifest.json` — corpus statistics, base-model filter, redaction settings.
- `README.md` — this file.

## Format

Each line of `train.jsonl` is a JSON object with this shape:

```json
{{"messages": [
  {{"role": "system", "content": "<prompt template identifier>"}},
  {{"role": "user", "content": "<redacted transcript + context>"}},
  {{"role": "assistant", "content": "<redacted final SOAP note>"}}
]}}
```

## Base model

Drafts in this corpus were generated by: **{models}**.

Fine-tune a compatible base model for best results. Mixing drafts from
different base models into a single fine-tune typically produces a
confused model — the export already filters to your chosen base.

## Redaction caveats

Redaction is rule-based, not ML-based. Known limitations:

- Patient names not stored in the recording's `patient_name` column
  are NOT redacted (e.g., second/third parties mentioned in the note).
- Provider names are only partially covered (depends on the
  `saved_recipients` list; aggressive mode covers more).
- Free-text re-identification through context (rare diagnosis + small
  town + age) is not addressed by this redaction.
- Review at least a random sample of the output before using it for
  training, especially before sharing the corpus with anyone else.

## Suggested fine-tuning recipes

This is a small personal corpus. **LoRA / QLoRA** is the right approach,
not full fine-tuning.

Typical hyperparameters for a personal-style fine-tune:

- LoRA rank: 16
- LoRA alpha: 32
- Learning rate: 2e-4
- Epochs: 2–3
- Batch size: 1–4 (depending on context length)

Common toolchains that accept this format directly:

- [unsloth](https://github.com/unslothai/unsloth) — fastest on a single GPU
- [mlx-lm](https://github.com/ml-explore/mlx-examples/tree/main/llms) — Apple Silicon native
- [torchtune](https://github.com/pytorch/torchtune) — official PyTorch
- [axolotl](https://github.com/OpenAccess-AI-Collective/axolotl) — config-driven

## After training

Pull the resulting model into Ollama:

```
ollama create soap-personal-v1 -f ./Modelfile
```

It will then appear in FerriScribe's Ollama provider list.

## Privacy reminder

This corpus may contain residual identifiers despite redaction. Keep
it on the same device as the source data, or transfer only over
encrypted channels to a trusted training environment.
"#,
          pair_count = pair_count,
          models = models,
          ferri_scribe_version = ferri_scribe_version,
      )
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn readme_includes_pair_count_and_models() {
          let out = render_readme(123, &["llama3:70b".to_string()], "0.10.56");
          assert!(out.contains("123 training pairs"));
          assert!(out.contains("llama3:70b"));
          assert!(out.contains("0.10.56"));
      }

      #[test]
      fn readme_handles_empty_model_list() {
          let out = render_readme(5, &[], "0.10.56");
          assert!(out.contains("(unspecified)"));
      }
  }
  ```

- [ ] **Step 4: Orchestration module**

  Create `src-tauri/src/corpus_export/mod.rs`:

  ```rust
  //! Orchestrates the training-corpus export pipeline.
  //!
  //! Filters promoted rows → applies redaction (static + per-recording
  //! extensions) → writes JSONL + manifest + README to the
  //! caller-supplied directory.

  pub mod jsonl_writer;
  pub mod manifest;
  pub mod readme;

  use medical_db::generations::{Generation, GenerationsRepo};
  use medical_security::phi_redactor::{Extension, PhiRedactor};
  use medical_security::phi_redactor::names::build_patient_name_extension;
  use medical_security::phi_redactor::datetime::build_datetime_extension;
  use rusqlite::Connection;
  use serde::Serialize;
  use std::path::{Path, PathBuf};

  pub struct ExportOptions {
      pub output_dir: PathBuf,         // user-chosen
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

      // 4. Generate records and write JSONL.
      let jsonl_path = corpus_dir.join("train.jsonl");
      let file = std::fs::File::create(&jsonl_path)
          .map_err(|e| format!("create train.jsonl: {e}"))?;
      let mut writer = std::io::BufWriter::new(file);

      let mut input_tokens_est: u64 = 0;
      let mut output_tokens_est: u64 = 0;

      let records = promoted.iter().enumerate().map(|(idx, gen)| {
          let pt_name_ext = lookup_patient_name(conn, &gen.recording_id)
              .and_then(|n| build_patient_name_extension(&n));
          let mut extensions: Vec<Extension> = Vec::new();
          if let Some(e) = pt_name_ext { extensions.push(e); }
          extensions.push(datetime_ext.clone()); // requires #[derive(Clone)] on Extension

          let user_input = format_user_input(gen);
          let redacted_user = PhiRedactor::redact_with(&user_input, &extensions);
          let redacted_final = PhiRedactor::redact_with(
              gen.final_text.as_deref().unwrap_or(""),
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

          jsonl_writer::TrainingRecord {
              system: gen.prompt_template_name.clone().unwrap_or_else(|| "soap".to_string()),
              user: redacted_user,
              assistant: redacted_final,
          }
      });

      let pairs = jsonl_writer::write_jsonl(&mut writer, records)
          .map_err(|e| format!("write_jsonl: {e}"))?;
      use std::io::Write;
      writer.flush().map_err(|e| format!("flush: {e}"))?;

      // 5. Manifest.
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
              "SSN".into(), "PHONE".into(), "EMAIL".into(), "DOB".into(),
              "MRN".into(), "ADDRESS".into(), "ZIP".into(),
              "PT_NAME".into(), "DATE".into(),
          ],
          warnings: warnings.clone(),
      };
      manifest::write_manifest(&m, &corpus_dir.join("manifest.json"))
          .map_err(|e| format!("write manifest: {e}"))?;

      // 6. README.
      let readme_text = readme::render_readme(
          pairs as u32,
          &opts.base_model_filter,
          &opts.ferri_scribe_version,
      );
      std::fs::write(corpus_dir.join("README.md"), readme_text)
          .map_err(|e| format!("write readme: {e}"))?;

      Ok(ExportResult {
          corpus_dir,
          pairs_written: pairs as u32,
          warnings,
      })
  }

  fn fetch_promoted(conn: &Connection, model_filter: &[String]) -> Result<Vec<Generation>, String> {
      // Use GenerationsRepo::list_by_status to page all promoted; for
      // v1 we expect at most a few thousand promoted rows so a single
      // call with limit 200 paginated until done is fine.
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
          .filter(|g| model_filter.is_empty() || model_filter.iter().any(|m| m == &g.ai_model))
          .collect();
      Ok(filtered)
  }

  fn lookup_patient_name(conn: &Connection, recording_id: &uuid::Uuid) -> Option<String> {
      conn.query_row(
          "SELECT patient_name FROM recordings WHERE id = ?",
          rusqlite::params![recording_id.to_string()],
          |row| row.get::<_, Option<String>>(0),
      )
      .ok()
      .flatten()
      .filter(|s| !s.trim().is_empty())
  }

  fn format_user_input(gen: &Generation) -> String {
      // For v1: concatenate transcript + context_json. The fine-tune
      // sees the same input shape as the SOAP generation pipeline.
      let mut s = gen.input_transcript.clone();
      if let Some(ctx) = &gen.input_context_json {
          if !ctx.trim().is_empty() && ctx != "null" {
              s.push_str("\n\n[Context]\n");
              s.push_str(ctx);
          }
      }
      s
  }
  ```

- [ ] **Step 5: Register the module**

  In `src-tauri/src/lib.rs`, near the existing `mod commands;`, add:

  ```rust
  pub mod corpus_export;
  ```

- [ ] **Step 6: Build**

  Run: `cargo build -p rust-medical-assistant`
  Expected: clean. If `chrono::Utc::now()` isn't already a workspace dep, check `crates/core/Cargo.toml` and add `chrono = { workspace = true }` to `src-tauri/Cargo.toml` if needed.

- [ ] **Step 7: Commit**

  ```bash
  git add src-tauri/src/corpus_export/ src-tauri/src/lib.rs src-tauri/Cargo.toml
  git commit -m "feat(export): corpus export pipeline orchestration

  Pulls promoted rows with non-NULL final_text, applies per-recording
  patient-name + shared datetime redaction extensions over the
  existing PhiRedactor defaults, writes JSONL + manifest + README to
  a timestamped subdirectory of the caller's chosen output dir.

  Flags rows where residual PHI is detected after redaction in the
  manifest.warnings array."
  ```

---

## Task 5: Tauri command + dialog

**Files:**
- Create: `src-tauri/src/commands/training_corpus_export.rs`
- Modify: `src-tauri/src/commands/mod.rs` (register)
- Modify: `src-tauri/src/lib.rs` (register invoke handler)

### Steps

- [ ] **Step 1: Create the command**

  Create `src-tauri/src/commands/training_corpus_export.rs`:

  ```rust
  //! Tauri command for the corpus-export pipeline (Phase 3).

  use medical_core::error::{AppError, AppResult};
  use serde::{Deserialize, Serialize};
  use std::path::PathBuf;

  use crate::corpus_export::{self, ExportOptions, RedactionStrictness};
  use crate::state::AppState;

  #[derive(Debug, Deserialize)]
  pub struct ExportRequest {
      pub output_dir: String,
      pub base_model_filter: Vec<String>,
      pub redaction_strictness: String, // 'standard' | 'aggressive'
  }

  #[derive(Debug, Serialize)]
  pub struct ExportResponse {
      pub corpus_dir: String,
      pub pairs_written: u32,
      pub warning_count: u32,
  }

  #[tauri::command]
  pub async fn training_corpus_export(
      state: tauri::State<'_, AppState>,
      req: ExportRequest,
  ) -> AppResult<ExportResponse> {
      let strictness = match req.redaction_strictness.as_str() {
          "aggressive" => RedactionStrictness::Aggressive,
          _ => RedactionStrictness::Standard,
      };
      let opts = ExportOptions {
          output_dir: PathBuf::from(req.output_dir),
          base_model_filter: req.base_model_filter,
          redaction_strictness: strictness,
          ferri_scribe_version: env!("CARGO_PKG_VERSION").to_string(),
      };

      // The pipeline is sync (file I/O + regex); spawn_blocking
      // keeps the runtime responsive.
      let db = std::sync::Arc::clone(&state.db);
      let result = tokio::task::spawn_blocking(move || -> Result<_, String> {
          let conn = db.conn().map_err(|e| e.to_string())?;
          corpus_export::export(&conn, opts)
      })
      .await
      .map_err(|e| AppError::Other(format!("export task join: {e}")))?
      .map_err(|e| AppError::Other(format!("export failed: {e}")))?;

      Ok(ExportResponse {
          corpus_dir: result.corpus_dir.to_string_lossy().to_string(),
          pairs_written: result.pairs_written,
          warning_count: result.warnings.len() as u32,
      })
  }
  ```

- [ ] **Step 2: Register**

  Add `pub mod training_corpus_export;` to `src-tauri/src/commands/mod.rs`. Add `commands::training_corpus_export::training_corpus_export,` to the `invoke_handler` list in `src-tauri/src/lib.rs`.

- [ ] **Step 3: Build**

  Run: `cargo build -p rust-medical-assistant`
  Expected: clean.

- [ ] **Step 4: Commit**

  ```bash
  git add src-tauri/src/commands/training_corpus_export.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs
  git commit -m "feat(commands): training_corpus_export Tauri command

  Wraps the export pipeline in a Tauri command, runs the sync
  pipeline inside spawn_blocking to keep the runtime responsive."
  ```

---

## Task 6: Frontend export dialog

**Files:**
- Create: `src/lib/components/settings/training_corpus/ExportDialog.svelte`
- Modify: `src/lib/components/settings/training_corpus/PromotedList.svelte` (add the Export button)

### Steps

- [ ] **Step 1: Build the dialog**

  Create `src/lib/components/settings/training_corpus/ExportDialog.svelte`:

  ```svelte
  <script lang="ts">
    import { invoke } from '@tauri-apps/api/core';
    import { open as openDialog } from '@tauri-apps/plugin-dialog';

    type Props = {
      onclose: () => void;
      onsuccess: (corpusDir: string, pairs: number, warnings: number) => void;
      promotedCount: number;
      availableModels: string[]; // distinct ai_model values seen in promoted rows
    };
    let { onclose, onsuccess, promotedCount, availableModels }: Props = $props();

    let outputDir = $state<string | null>(null);
    let selectedModels = $state<string[]>(availableModels.length > 0 ? [availableModels[0]] : []);
    let strictness: 'standard' | 'aggressive' = $state('standard');
    let exporting = $state(false);
    let error: string | null = $state(null);

    async function pickDirectory() {
      try {
        const selected = await openDialog({ directory: true, multiple: false });
        if (typeof selected === 'string') outputDir = selected;
      } catch (e) {
        error = String(e);
      }
    }

    async function runExport() {
      if (!outputDir) {
        error = "Choose an output directory first.";
        return;
      }
      exporting = true;
      error = null;
      try {
        const resp = await invoke<{ corpus_dir: string; pairs_written: number; warning_count: number }>(
          'training_corpus_export',
          {
            req: {
              output_dir: outputDir,
              base_model_filter: selectedModels,
              redaction_strictness: strictness,
            },
          }
        );
        onsuccess(resp.corpus_dir, resp.pairs_written, resp.warning_count);
      } catch (e) {
        error = String(e);
      } finally {
        exporting = false;
      }
    }
  </script>

  <div class="modal-backdrop" onclick={onclose}>
    <div class="modal" onclick={(e) => e.stopPropagation()}>
      <header><h3>Export training corpus</h3></header>

      <p>
        Will export <strong>{promotedCount}</strong> promoted SOAP pair{promotedCount === 1 ? '' : 's'}
        to a timestamped subdirectory.
      </p>

      <fieldset>
        <legend>Base model filter</legend>
        {#each availableModels as model}
          <label>
            <input type="checkbox"
              checked={selectedModels.includes(model)}
              onchange={(e) => {
                if ((e.target as HTMLInputElement).checked) {
                  selectedModels = [...selectedModels, model];
                } else {
                  selectedModels = selectedModels.filter(m => m !== model);
                }
              }}
            />
            <code>{model}</code>
          </label>
        {/each}
        {#if availableModels.length === 0}
          <p class="hint">(no model info available — exporting all rows)</p>
        {/if}
      </fieldset>

      <fieldset>
        <legend>Redaction strictness</legend>
        <label><input type="radio" name="strict" value="standard" bind:group={strictness} /> Standard (default)</label>
        <label><input type="radio" name="strict" value="aggressive" bind:group={strictness} disabled /> Aggressive (v2, coming later)</label>
      </fieldset>

      <fieldset>
        <legend>Output directory</legend>
        <div class="dir-row">
          <button onclick={pickDirectory}>Choose folder…</button>
          {#if outputDir}<code>{outputDir}</code>{/if}
        </div>
      </fieldset>

      {#if error}<div class="error">{error}</div>{/if}

      <footer class="modal-actions">
        <button onclick={onclose} disabled={exporting}>Cancel</button>
        <button class="primary" onclick={runExport} disabled={exporting || !outputDir}>
          {exporting ? 'Exporting…' : 'Export'}
        </button>
      </footer>
    </div>
  </div>

  <style>
    .modal-backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.4); display: flex; align-items: center; justify-content: center; z-index: 100; }
    .modal { background: var(--bg, white); border-radius: 8px; padding: 1.5rem; min-width: 520px; max-width: 700px; display: flex; flex-direction: column; gap: 1rem; }
    fieldset { border: 1px solid var(--border, #ddd); border-radius: 6px; padding: 0.75rem; }
    legend { font-weight: 600; padding: 0 0.5rem; }
    fieldset label { display: block; padding: 0.2rem 0; }
    .dir-row { display: flex; gap: 0.5rem; align-items: center; }
    .dir-row code { font-size: 0.85rem; color: var(--muted-foreground, #888); }
    .hint { color: var(--muted-foreground, #888); font-size: 0.85rem; }
    .error { background: #fee; color: #991b1b; padding: 0.5rem; border-radius: 4px; }
    .modal-actions { display: flex; justify-content: flex-end; gap: 0.5rem; padding-top: 0.5rem; border-top: 1px solid var(--border, #ddd); }
    button.primary { background: #0066cc; color: white; padding: 0.4rem 1rem; border-radius: 4px; border: none; cursor: pointer; }
    button.primary:disabled { background: #ccc; cursor: not-allowed; }
  </style>
  ```

- [ ] **Step 2: Wire into `PromotedList`**

  Modify `src/lib/components/settings/training_corpus/PromotedList.svelte` to add an "Export training corpus" button at the top of the list, and a state variable to open/close the dialog. Sketch:

  ```svelte
  <script lang="ts">
    // ... existing imports + state ...
    import ExportDialog from './ExportDialog.svelte';

    let showExport = $state(false);
    let successMessage: string | null = $state(null);

    function distinctModels(): string[] {
      const set = new Set(items.map((g) => g.ai_model));
      return Array.from(set).sort();
    }
  </script>

  <div class="promoted-toolbar">
    <button onclick={() => (showExport = true)} disabled={total === 0}>
      Export training corpus…
    </button>
    {#if successMessage}<span class="success">{successMessage}</span>{/if}
  </div>

  <!-- existing list rendering ... -->

  {#if showExport}
    <ExportDialog
      promotedCount={total}
      availableModels={distinctModels()}
      onclose={() => (showExport = false)}
      onsuccess={(dir, pairs, warnings) => {
        showExport = false;
        successMessage = `Exported ${pairs} pair${pairs === 1 ? '' : 's'} to ${dir}` +
                         (warnings > 0 ? ` (${warnings} redaction warnings — see manifest.json)` : '');
      }}
    />
  {/if}
  ```

  Add the necessary CSS for `.promoted-toolbar` and `.success`. Adjust the `items` access if the list structure differs from Phase 2's CandidatesList.

- [ ] **Step 3: Verify the dialog plugin is registered**

  The export dialog uses `@tauri-apps/plugin-dialog`. Verify with:
  ```
  grep -n "plugin-dialog\|tauri-plugin-dialog" src-tauri/Cargo.toml package.json
  ```
  If not present, add:
  - `src-tauri/Cargo.toml`: under `[dependencies]`, `tauri-plugin-dialog = "2"`
  - `src-tauri/src/lib.rs`: in the Tauri builder chain, `.plugin(tauri_plugin_dialog::init())`
  - `package.json`: under dependencies, `"@tauri-apps/plugin-dialog": "^2.0.0"` (or whichever version matches Tauri 2)

  Run `npm install` if you added the JS dep.

- [ ] **Step 4: Type-check + manual smoke test**

  Run `npm run check`. Then `npm run tauri dev`. Flow:
  1. Enable capture, generate + save a few SOAPs.
  2. Promote them in the Candidates view.
  3. Switch to Promoted, click "Export training corpus…".
  4. Pick a directory, click Export.
  5. Confirm the success message; check the chosen directory for `training-corpus-YYYY-MM-DD-HHMMSS/` containing `train.jsonl`, `manifest.json`, `README.md`.
  6. Open `train.jsonl` and verify each line is valid JSON in the expected chat-completion shape, with PHI replaced by placeholders.
  7. Open `manifest.json` and verify counts match.
  8. Open `README.md` and verify it includes the pair count and base-model identifier.

- [ ] **Step 5: Commit**

  ```bash
  git add src/lib/components/settings/training_corpus/ExportDialog.svelte src/lib/components/settings/training_corpus/PromotedList.svelte src-tauri/Cargo.toml package.json src-tauri/src/lib.rs
  git commit -m "feat(ui): training corpus export dialog

  Modal launched from the Promoted view. Output directory picker via
  tauri-plugin-dialog, multi-model filter, redaction strictness (v1
  has only Standard active; Aggressive disabled for v2). Success
  toast shows where the export landed and any redaction warnings."
  ```

---

## Final verification

- [ ] **Workspace test sweep**

  ```bash
  cargo test --workspace --lib
  ```

  Expected: pass. New tests from Tasks 2, 3, 4 add to the count.

- [ ] **Frontend type-check**

  ```bash
  npm run check
  ```

  Expected: 0 errors.

- [ ] **End-to-end smoke**

  See Task 6 Step 4. Particularly important:
  - Verify a transcript containing real-looking PHI ("Mrs. Johnson, 67, called yesterday about 555-1212") comes out redacted in `train.jsonl`.
  - Verify the manifest's `warnings` array correctly flags any row where redaction left residual PHI.
  - Verify the README is readable and helpful.

- [ ] **PHI policy spot-check**

  ```
  git diff master..HEAD -- '*.rs' | grep -E "^\+.*tracing::"
  ```

  Expected: no new log lines that emit transcript or SOAP content. Capture failures and export errors should log only structural fields.

- [ ] **Performance check**

  Export 100 rows from a populated corpus. Expected: completes in <1 second (it's all in-memory regex + a single file write).

---

## Out-of-scope follow-ups (for future plans)

- **Aggressive redaction (provider names, locations).** The radio button exists but is disabled. v2 fills this in with patterns sourced from `saved_recipients` + a project-shipped list of common Canadian hospitals/clinics.
- **NER-based name detection** for second/third-party names not in `recordings.patient_name`. Adds an ML dep; out of scope.
- **Continuous evaluation** — run the same transcript through the base model and the fine-tuned model, show quality delta.
- **Template versioning** — capture the rendered prompt template per row so a corpus that spans template revisions can be filtered or re-rendered.
- **Multi-format export** — currently only OpenAI chat-completion JSONL. If clinicians ask for Alpaca / ShareGPT / raw-pair JSONL, add a format selector.

---

## Implementation handoff

After this plan completes, the full training-corpus feature is shippable: clinician can opt in via Settings, generate SOAPs (auto-captured), curate them in the Training Corpus tab, and export a redacted JSONL ready for fine-tuning with their preferred toolchain. No data has left the device.

Combined with Phases 1 and 2, this is roughly 18 commits across the three plans. The full feature can ship in a single release once all three plans are merged.
