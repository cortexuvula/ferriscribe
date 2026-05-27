# medical-processing

Transcription pipeline orchestration and document generation for FerriScribe.

This crate (~2,700 lines) turns recordings into transcripts, and transcripts
into clinical documents: SOAP notes, referral letters, patient correspondence,
and synopses. It is the central "work" crate — everything upstream captures
audio, everything downstream displays results; this is where the AI-assisted
transformation happens.

> **Audience:** future-you returning to this crate after months away.

---

## How It Fits in the Workspace

```
medical-core ──(types, traits)──▶  medical-processing  ◀──(persistence)──  medical-db
                                        │
                                        ▼
                                   src-tauri
                            (Tauri commands that drive
                             recording → document flow)
```

| Relationship | Crate | What it provides / consumes |
|---|---|---|
| **Depends on** | `medical-core` | `ProcessingEvent`, `TaskType`, `BatchState`, `PatientContext`, `VocabularyEntry`, `SoapTemplate`, `LetterAudience` and related types |
| **Depends on** | `medical-db` | Persistence layer for transcripts and generated documents |
| **Used by** | `src-tauri` | Tauri commands that trigger the pipeline on a recording, batch-process multiple recordings, or generate a document from an existing SOAP note |

---

## Module Map

| Module | Purpose |
|---|---|
| `pipeline` | `PipelineConfig`, `PipelineStep`, `run_pipeline()` — orchestrates the per-recording processing pipeline with step-level progress events over an `mpsc` channel. |
| `batch` | `BatchJob` — state tracker for multi-recording batch processing jobs (progress counts, completion status). |
| `soap_generator` | SOAP system prompt (`prompt_template`), user-turn prompt assembly with sanitization (`user_prompt`), and AI-output post-processing (`postprocess`). The anti-fabrication prompt lives here. |
| `document_generator` | Prompt builders for referral letters, patient correspondence, and synopses. Each returns a `(system_prompt, user_prompt)` tuple ready for an AI provider. |
| `prompt_resolver` | `resolve_prompt()` — simple `{key}` → value substitution in user-editable templates. Unknown tokens pass through unchanged. |
| `vocabulary_corrector` | `apply_corrections()` — word-boundary-aware find-and-replace with priority ordering, case sensitivity, and regex caching. |
| `edit_distance` | `word_edit_distance()` — word-level Levenshtein distance and ratio for the training-corpus quality signal. |

---

## Key Types

### Pipeline

- **`PipelineConfig`** — controls which optional steps execute: SOAP generation,
  referral letter, patient letter, RAG indexing. Defaults: SOAP on, referral
  and letter off, RAG on.
- **`PipelineStep`** — enum of every possible step (`Transcribing`,
  `GeneratingSoap`, `GeneratingReferral`, `GeneratingLetter`, `ExtractingData`,
  `IndexingRag`, `Complete`). Each has a `label()` for UI display.
- **`ProgressSender`** — alias for `mpsc::Sender<ProcessingEvent>`. The pipeline
  emits `TaskQueued → TaskStarted → TaskCompleted` events per step, plus a
  terminal `BatchCompleted`.

### Document Generation

- **`SoapPromptConfig`** — inputs to `build_soap_prompt()`: template variant,
  ICD version, optional custom prompt override.
- **`LetterAudienceContext`** — lightweight prompt-relevant subset of a letter
  audience (name, system prompt, optional user template).

### Batch Processing

- **`BatchJob`** — tracks a multi-recording batch: recording IDs, config,
  progress counts, and derived `BatchState` (`Queued → Running → Completed |
  Failed`).

### Error Handling

- **`ProcessingError`** — unified error enum: `Pipeline`, `Generation`, `Stt`,
  `Database`, `Cancelled`.
- **`ProcessingResult<T>`** — convenience alias for `Result<T, ProcessingError>`.

---

## How It Works

### The Processing Pipeline

When a recording finishes capturing audio, `src-tauri` calls
`pipeline::run_pipeline()` with a `PipelineConfig` and a progress channel:

```
Audio captured
  → run_pipeline(recording_id, config, progress_tx)
    → Step 1: Transcribing (always) — emit TaskQueued/Started/Completed
    → Step 2: GeneratingSoap (if config.generate_soap)
    → Step 3: GeneratingReferral (if config.generate_referral)
    → Step 4: GeneratingLetter (if config.generate_letter)
    → Step 5: ExtractingData (always) — medication/condition/allergy extraction
    → Step 6: IndexingRag (if config.auto_index_rag)
    → Step 7: Complete — emit BatchCompleted
```

Each step emits three progress events (`TaskQueued`, `TaskStarted`,
`TaskCompleted`) so the frontend can show granular progress. The pipeline is
sequential — each step completes before the next begins.

### SOAP Note Generation

SOAP generation is the most intricate document flow. It has three layers:

1. **System prompt** (`soap_generator::prompt_template::build_soap_prompt`) —
   the built-in default prompt or a user-supplied custom template, with
   `{icd_label}`, `{icd_instruction}`, and `{template_guidance}` placeholders
   resolved.

2. **User prompt** (`soap_generator::user_prompt::build_user_prompt`) —
   assembles the user-turn message from:
   - The transcript (primary source of truth, **never truncated here**)
   - Patient record block (structured medications/allergies/conditions —
     authoritative for historical Subjective fields only)
   - Supplementary background (freeform context, truncated to 8,000 chars)
   - All inputs are sanitized to strip prompt-injection patterns

3. **Post-processing** (`soap_generator::postprocess::postprocess_soap`) —
   strips markdown formatting, citation markers, and ensures proper paragraph
   separation between SOAP sections.

### Referral / Letter / Synopsis Generation

`document_generator` provides simpler prompt builders that take an existing
SOAP note and produce `(system_prompt, user_prompt)` tuples:

- `build_referral_prompt(soap, recipient_type, urgency, custom_template)` —
  resolves `{recipient_type}` and `{urgency}` placeholders.
- `build_letter_prompt(soap, letter_type, audience, custom_template)` — three
  resolution paths depending on whether a `LetterAudienceContext` is provided
  (see the function doc for the precedence order).
- `build_synopsis_prompt(soap, custom_template)` — concise ≤200-word summary.

### Prompt Resolution

`prompt_resolver::resolve_prompt()` is a simple `{key}` → value substitution
used by all document generators. Unknown tokens pass through unchanged so
user-visible typos in custom templates remain debuggable.

### Vocabulary Correction

`vocabulary_corrector::apply_corrections()` runs after STT to fix common
medical abbreviations and misrecognised terms. Entries are sorted by priority
(then by match length), matched at word boundaries with optional case
sensitivity, and compiled regex patterns are cached per `(find_text,
case_sensitive)` pair.

### Batch Processing

`BatchJob` tracks progress across multiple recordings processed with the same
`PipelineConfig`. Call `record_success()` / `record_failure()` after each
recording; `is_done()` and `progress_percent()` report completion. Mixed
outcomes (some succeeded, some failed) are reported as `BatchState::Failed` —
there is no `PartiallyCompleted` variant in the processing type set.

---

## SOAP Prompt Anti-Fabrication Rules

> **⚠ The SOAP system prompt is a precision instrument.**

The default SOAP system prompt in `prompt_template::default_soap_prompt()` is
~280 lines of carefully tuned instructions. Key constraints:

1. **Transcript is the sole source of truth.** Every clinical finding must be
   directly traceable to the recording. The prompt names ten categories of
   "forbidden inferences" (demographics, past conditions, medication doses,
   family history, social history specifics, visit modality, general
   appearance, referral provider names, follow-up intervals, red-flag
   warnings).

2. **Background context populates historical Subjective fields only.**
   Patient record entries (medications, allergies, conditions) and
   supplementary background text may populate Past Medical History, Current
   Medications, Allergies, Surgical History, Family History, and Social
   History. They must **never** alter today's Objective findings, Assessment,
   Differential Diagnosis, or Plan.

3. **ICD codes and Differential Diagnosis are the only inference-permitted
   sections.** Both must render items as plain text with no `(suggested)` or
   similar qualifier markers.

4. **A 10-point self-check checklist** at the end of the prompt (recency
   matters for LLM compliance) forces the model to verify each line against
   the transcript before outputting.

5. **Two few-shot examples** (a sparse injury visit and a lab-review visit)
   demonstrate disciplined extraction, including what would constitute
   fabrication.

If you change this prompt, run the full `soap_generator` test suite — the
tests encode dozens of invariants about prompt structure, section ordering,
and fabrication guards.

---

## Gotchas

- **`sanitize_prompt` does NOT truncate.** A previous version silently
  truncated the transcript to 10K chars inside `sanitize_prompt`, causing the
  model to hallucinate the missing Assessment and Plan. Truncation
  responsibility now lives at the command layer (`MAX_TRANSCRIPT_CHARS` in
  `src-tauri`) for transcripts, and at `MAX_CONTEXT_LENGTH` (8,000 chars) for
  supplementary background.

- **Prompt substitution order is non-deterministic.** `resolve_prompt` iterates
  a `HashMap`, so if a value contains `{key}` matching another key in the map,
  the output is unstable. Values must not contain cross-referencing
  placeholder tokens.

- **Batch state has no `PartiallyCompleted`.** When some recordings succeed
  and others fail, `BatchJob` reports `BatchState::Failed`. Callers must check
  `completed_count` vs `failed_count` to distinguish total from partial
  failure.

- **Pipeline steps are sequential, not parallel.** The pipeline emits progress
  events for each step in order. If you need concurrent processing across
  recordings, that concurrency lives in the caller (src-tauri spawns multiple
  pipeline tasks), not in this crate.

- **Letter audience takes precedence over custom template.** When
  `build_letter_prompt` receives both an audience and a custom template, the
  audience wins. This is intentional — audience-specific prompts are curated
  and should not be silently overridden.

- **Vocabulary corrections use word boundaries.** The regex wraps each
  `find_text` in `\b...\b`, so "washington" won't match an entry for
  "washing". Multi-word entries (e.g., "dm type 2") are matched as a single
  unit. Disabled entries are silently skipped.

- **Edit distance is word-level, not character-level.** This matches clinician
  intuition ("you changed 20% of the words") better than character delta. For
  typical 200–800-word SOAP notes, computation is sub-millisecond.

---

## Testing

```bash
cargo test -p medical-processing --lib
```

The crate has extensive test coverage, particularly for:
- SOAP prompt structure invariants (section ordering, anti-fabrication guards,
  few-shot example placement, self-check completeness)
- Prompt substitution edge cases (empty values, unknown tokens, repeated
  placeholders)
- Vocabulary corrector ordering and boundary behaviour
- Batch job state machine transitions
- Pipeline step execution under different configs
