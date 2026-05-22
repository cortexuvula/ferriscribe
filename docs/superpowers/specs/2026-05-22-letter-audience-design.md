# Letter Audience System Design

**Date:** 2026-05-22
**Status:** Draft
**Version:** 0.10.87+

## Overview

Extend the letter generation feature to support audience-specific prompts and templates. Clinicians can select from 6 predefined audiences (Patient, Insurance, Tax, Specialist, Employer, Legal) or define custom audiences. Each audience has a tailored system prompt and optional user template that guide the AI to produce letters appropriate for the intended recipient.

## Problem Statement

The current letter generation system accepts a `letter_type` parameter (e.g., "follow-up," "results") but uses a hardcoded patient-focused prompt: "Generate a {letter_type} letter for the patient. Use clear, plain language..." This doesn't work when the clinician needs a letter for an insurance company (which requires medical necessity language and billing codes), a tax authority (which requires expense justification), or other non-patient recipients.

## Solution: Two-Layer Selection

- **Layer 1: Audience** (the "who") — Patient, Insurance, Tax, Specialist, Employer, Legal, or custom
- **Layer 2: Letter type** (the "what for") — Free text like "pre-authorization," "disability claim," "return-to-work," "results," "follow-up"

Both layers feed the prompt builder. The audience controls tone, language, and structure. The letter type provides context for what the letter is about.

## Architecture

### Data Layer (`crates/db`)

New `letter_audiences` table with CRUD operations in `LetterAudiencesRepo`. Six built-in rows seeded on migration. Custom audiences stored alongside with `is_builtin=0`.

### Backend Commands (`src-tauri/src/commands/letter_audiences.rs`)

Three CRUD commands: `list_letter_audiences`, `upsert_letter_audience`, `delete_letter_audience`. Modified `generate_letter` command gains `audience_id` parameter.

### Prompt Builder (`crates/processing/src/document_generator.rs`)

`build_letter_prompt` gains `audience` parameter. When provided, uses audience-specific system prompt and optional user template. Falls back to default behavior when `None` (backward compatible).

### Sync Layer (`src-tauri/src/commands/sharing/` + `crates/sharing`)

Audience CRUD endpoints on office server, mirroring vocabulary sync pattern. Client-side `LetterAudienceRemote` routes commands through paired server when connected.

### Frontend (`src/lib`)

Two-layer picker on letter card in `GenerateTab.svelte`. Settings panel for managing custom audiences. Runic store for audience state.

## Data Model

### Table: `letter_audiences`

```sql
CREATE TABLE letter_audiences (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  system_prompt TEXT NOT NULL,
  user_template TEXT,
  is_builtin INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
```

| Column | Type | Notes |
|--------|------|-------|
| `id` | `TEXT` PRIMARY KEY | UUID v4, stable across sync |
| `name` | `TEXT` NOT NULL | Display name (e.g. "Insurance Company") |
| `system_prompt` | `TEXT` NOT NULL | Role/tone instructions for the AI |
| `user_template` | `TEXT` | Optional. Template with `{letter_type}` and `{soap_note}` placeholders. `NULL` = use default user template |
| `is_builtin` | `INTEGER` NOT NULL DEFAULT 0 | `1` for seeded rows, `0` for custom |
| `created_at` | `TEXT` NOT NULL | ISO 8601 |
| `updated_at` | `TEXT` NOT NULL | ISO 8601, used for sync conflict resolution |

### Six Seeded Built-Ins

| Name | System Prompt (Summary) | User Template |
|------|------------------------|---------------|
| **Patient** | Plain language, empathetic, avoid jargon | Default (existing) |
| **Insurance Company** | Formal medical necessity language, ICD/CPT references | Includes medical necessity statement, diagnosis codes section |
| **Tax Authority** | Expense/disability justification, factual, timeline-focused | Includes service dates, cost justification |
| **Specialist/Consultant** | Clinical detail, professional peer tone | Includes relevant history, findings, specific questions |
| **Employer/School** | Accommodations, fitness-for-duty, HIPAA-minimal | Includes functional limitations, recommended accommodations |
| **Legal/Court** | Formal medical opinion, timeline, objective findings | Includes chronological timeline, clinical findings |

**Key decisions:**
- Built-in rows are non-deletable (UI disables delete button, backend rejects with `AppError::Other`)
- `user_template = NULL` means "use the default user prompt builder with `{letter_type}`"
- The `letter_type` field (free text) is always injected into `{letter_type}` in the user template

## Prompt Builder Changes

### New Struct

```rust
pub struct LetterAudience {
    pub name: String,
    pub system_prompt: String,
    pub user_template: Option<String>,
}
```

### Modified Signature

```rust
pub fn build_letter_prompt(
    soap_note: &str,
    letter_type: &str,
    audience: Option<&LetterAudience>,
    custom_template: Option<&str>,  // legacy custom_letter_prompt from settings
) -> (String, String)
```

### Resolution Order

1. If `audience` is provided AND `audience.user_template` is `Some(...)`, use the audience's system prompt and user template. Inject `{letter_type}` and `{soap_note}` into the user template.
2. If `audience` is provided BUT `audience.user_template` is `None`, use the audience's system prompt but fall back to the default user prompt builder.
3. If `audience` is `None`, behave exactly as today (backward compatible).
4. The legacy `custom_template` parameter (from `settings.custom_letter_prompt`) is **deprecated** and ignored when an audience is provided. When no audience is given, it still works as before.

**Why this order?** The audience is the clinician's deliberate choice for the setting. A global `custom_letter_prompt` in settings was a blunt instrument — audience-specific prompts replace it cleanly. The deprecation is soft: old settings remain functional for callers that haven't migrated.

## Backend Commands

### New Module: `src-tauri/src/commands/letter_audiences.rs`

```rust
#[tauri::command]
pub async fn list_letter_audiences(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<LetterAudience>>

#[tauri::command]
pub async fn upsert_letter_audience(
    state: tauri::State<'_, AppState>,
    audience: LetterAudience,
) -> AppResult<LetterAudience>

#[tauri::command]
pub async fn delete_letter_audience(
    state: tauri::State<'_, AppState>,
    id: String,
) -> AppResult<()>
```

**Constraints:**
- `delete_letter_audience` rejects deletion when `is_builtin=1`
- `upsert_letter_audience` generates a UUID if `id` is empty (new custom audience), otherwise updates existing
- All three commands check for paired-server mode and route through `LetterAudienceRemote` when connected

### Modified Command: `generate_letter`

```rust
#[tauri::command]
pub async fn generate_letter(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    letter_type: Option<String>,
    audience_id: Option<String>,  // NEW
) -> AppResult<String>
```

When `audience_id` is provided, the command fetches the audience row and passes it to `build_letter_prompt`. When `None`, behavior is unchanged.

### Registered in `lib.rs`

```rust
commands::letter_audiences::list_letter_audiences,
commands::letter_audiences::upsert_letter_audience,
commands::letter_audiences::delete_letter_audience,
```

## Sync Layer (Paired-Server Mode)

The sync follows the exact pattern established by vocabulary sync in `src-tauri/src/sharing_vocab_api.rs` and `src-tauri/src/vocab_remote.rs`.

### Office-Server Side: `src-tauri/src/sharing_audience_api.rs`

When the sharing service starts, it registers three HTTP routes:

```
GET    /api/v1/letter-audiences     — list all audiences
PUT    /api/v1/letter-audiences     — upsert one (by UUID)
DELETE /api/v1/letter-audiences/:id — delete one
```

Handlers call into the office server's `LetterAudiencesRepo` directly. Authentication uses the same bearer-token middleware as vocabulary routes.

### Client Side: `src-tauri/src/audience_remote.rs`

```rust
pub struct LetterAudienceRemote {
    endpoint: RemoteEndpoint,
    http: Arc<reqwest::Client>,
}
```

### Routing Logic in Commands

The three `letter_audiences` commands check `load_paired_connection()` first. If this machine is a paired client, they delegate to `LetterAudienceRemote`. If not paired, they operate on the local DB directly.

### Sync Semantics

- **Last-write-wins** by `updated_at` timestamp
- **Seeding guard:** Built-in rows (`is_builtin=1`) are protected from overwrite on the office server
- **Conflict on delete:** Deletes propagate on next sync. Later `updated_at` wins on conflicts.

## Frontend UI

### Two-Layer Picker on Letter Card (`GenerateTab.svelte`)

The existing "Patient Letter" card gains:

1. **Audience selector**: Dropdown listing the 6 built-in audiences + custom ones. Labeled "Audience."
2. **Letter type field**: Free-text input, placeholder "e.g. follow-up, pre-authorization, disability claim." Labeled "Letter purpose."

The Generate button passes both `audience_id` and `letter_type` to `generateLetter()`. The card description updates dynamically based on the selected audience.

### Settings Panel: `LetterAudiences.svelte`

New component under `src/lib/components/settings/`, listed alongside Vocabulary and Context Templates in the Settings dialog. Shows:

- Built-in audiences (read-only, non-deletable) with a "View prompt" button
- Custom audiences with edit/delete buttons
- "Add custom audience" button opening a form with: name, system prompt (textarea), user template (textarea, optional, with helper text explaining `{letter_type}` and `{soap_note}` placeholders)

### State Management: `src/lib/stores/letterAudiences.svelte.ts`

```ts
function createLetterAudiencesStore() {
  let audiences = $state<LetterAudience[]>([]);
  // list(), upsert(), delete() — invoke Tauri commands
  return { get audiences() { return audiences; }, list, upsert, delete };
}
```

Loaded on mount in `App.svelte` or lazy-loaded when Settings opens.

### API Wrapper: `src/lib/api/letterAudiences.ts`

```ts
export function listLetterAudiences(): Promise<LetterAudience[]>
export function upsertLetterAudience(a: LetterAudience): Promise<LetterAudience>
export function deleteLetterAudience(id: string): Promise<void>
```

### Modified: `src/lib/api/generation.ts`

```ts
export async function generateLetter(
  recordingId: string,
  letterType?: string,
  audienceId?: string,  // NEW
): Promise<string>
```

## Migration Strategy

### Database Migration

New migration file `crates/db/migrations/20260522_letter_audiences.sql`:

```sql
CREATE TABLE IF NOT EXISTS letter_audiences (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  system_prompt TEXT NOT NULL,
  user_template TEXT,
  is_builtin INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- Seed built-in audiences (only if table is empty)
INSERT OR IGNORE INTO letter_audiences (id, name, system_prompt, user_template, is_builtin, created_at, updated_at)
VALUES
  ('builtin-patient', 'Patient', 'You are a medical scribe assistant helping to write patient-friendly correspondence. Use clear, plain language the patient can understand. Avoid unexplained medical jargon. Be empathetic and professional.', NULL, 1, '2026-05-22T00:00:00Z', '2026-05-22T00:00:00Z'),
  ('builtin-insurance', 'Insurance Company', 'You are a medical scribe assistant writing formal correspondence for insurance companies. Use precise medical necessity language, reference ICD-10 and CPT codes where applicable, and structure the letter to justify medical necessity for the requested service or treatment.', 'Please write a {letter_type} letter for the insurance company based on the following SOAP note. Include a medical necessity statement, relevant diagnosis codes (ICD-10), and procedure codes (CPT) if applicable:\n\n{time_date}\n\n{soap_note}', 1, '2026-05-22T00:00:00Z', '2026-05-22T00:00:00Z'),
  -- ... (4 more built-ins)
```

### Backward Compatibility

- Existing `generate_letter` calls with no `audience_id` continue to work (defaults to `None`, uses legacy behavior)
- The `settings.custom_letter_prompt` field remains functional when no audience is selected
- Frontend can default the audience picker to "Patient" to match the old behavior

## Testing Strategy

### Backend Tests

- `crates/processing/src/document_generator.rs`: Unit tests for `build_letter_prompt` with various audience combinations
- `crates/db/src/letter_audiences.rs`: Integration tests for CRUD operations, seeding, deletion protection
- `src-tauri/src/commands/letter_audiences.rs`: Command-level tests with mock state
- `src-tauri/src/commands/generation/letter.rs`: Test that `audience_id` is correctly passed through

### Frontend Tests

- `src/lib/api/letterAudiences.test.ts`: API wrapper tests
- `src/lib/stores/letterAudiences.test.ts`: Store behavior tests
- `src/lib/components/settings/LetterAudiences.test.ts`: Component tests for add/edit/delete flows

### Manual Testing

- Generate letters with each of the 6 built-in audiences, verify tone and structure match expectations
- Create a custom audience, generate a letter with it, verify custom prompt is used
- Test paired-server sync: create custom audience on client, verify it appears on office server and other clients
- Test backward compatibility: generate a letter with no audience selected, verify it matches old behavior

## Rollout Plan

1. **Phase 1: Backend foundation** — DB migration, CRUD commands, prompt builder changes
2. **Phase 2: Frontend UI** — Two-layer picker, settings panel, state management
3. **Phase 3: Sync layer** — Office-server endpoints, client-side remote, routing logic
4. **Phase 4: Testing and polish** — Comprehensive test coverage, manual QA, documentation

## Success Criteria

- Clinicians can generate letters tailored to 6 different audiences with appropriate tone and structure
- Custom audiences can be created, edited, deleted, and synced across paired machines
- Backward compatibility is maintained (existing workflows continue to work)
- Sync conflicts are resolved predictably (last-write-wins by timestamp)
- Built-in audiences are protected from accidental deletion

## Out of Scope

- **Audience-specific validation**: The system trusts the AI to follow the prompt. No post-generation checks for "did this letter actually include ICD codes?"
- **Template variables beyond `{letter_type}` and `{soap_note}`**: Future versions might add `{patient_name}`, `{clinician_name}`, etc., but not in this iteration
- **Audience categories or grouping**: Flat list is sufficient for now
- **Bulk audience operations**: No "generate letters for all recordings with this audience" — out of scope
