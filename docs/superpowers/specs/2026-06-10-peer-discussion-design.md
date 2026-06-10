# Peer Discussion Feature Design

**Date:** 2026-06-10
**Status:** Approved
**Approach:** Full Parallel Path

## Overview

A new document type for generating structured physician-to-physician discussion notes from a recorded transcript. This is a peer-to-peer discussion note — physician documenting a conversation with another physician about a patient case (e.g., insurance peer-to-peer review).

**Input:** Transcript (from recording) + physician context (name, specialty, reason for discussion)

**Output:** Structured note with 6 sections: Header, Clinical Summary, Discussion Points, Assessment, Recommendations, Action Items

## Data Model

### Rust (`crates/core/src/types/recording.rs`)

Add new field to `Recording` struct:

```rust
pub peer_discussion: Option<String>,
```

### Rust (`crates/core/src/types/settings.rs`)

Add new field to `AppConfig`:

```rust
pub custom_peer_discussion_prompt: Option<String>,
```

### TypeScript (`src/lib/types/index.ts`)

Add to `Recording` interface:

```typescript
peer_discussion?: string;
```

Add to `AppConfig` interface:

```typescript
custom_peer_discussion_prompt?: string;
```

### New TypeScript interface

```typescript
interface PeerDiscussionContext {
  physician_name: string;
  specialty: string;
  reason: string;
}
```

### Type unions

- `DocType` (`src/lib/api/prompts.ts`): add `'peer_discussion'`
- `GeneratingType` (`src/lib/stores/generation.svelte.ts`): add `'peer_discussion'`

### Database

Migration to add `peer_discussion TEXT` column to recordings table. New migration file: `crates/db/src/migrations/m008_peer_discussion.rs` (following existing pattern in `crates/db/src/migrations/`).

## Backend (Rust)

### New module: `crates/processing/src/peer_discussion/`

Three files:

**`mod.rs`** — public API:
- `build_peer_discussion_prompt(config: &PeerDiscussionPromptConfig) -> String`
- `build_user_prompt(transcript: &str, physician_name: &str, specialty: &str, reason: &str) -> String`

**`prompt_template.rs`** — system prompt:
- `default_peer_discussion_prompt()` — returns the full system prompt
- `build_peer_discussion_prompt(config)` — resolves custom or default template with placeholders
- Anti-fabrication rules matching SOAP prompt
- Sections: Header, Clinical Summary, Discussion Points, Assessment, Recommendations, Action Items
- Placeholders: `{physician_name}`, `{specialty}`, `{reason}`
- Specialty-specific guidance via `template_guidance_text()`
- 6-point self-check checklist

**`user_prompt.rs`** — user prompt builder:
- Assembles transcript + physician context
- Sanitizes input against prompt injection (same pattern as SOAP `user_prompt.rs`)

### New Tauri command: `src-tauri/src/commands/generation/peer_discussion.rs`

- `generate_peer_discussion(recording_id, physician_name, specialty, reason)` command
- Validates inputs (non-empty, size limits)
- Resolves AI provider from registry
- Builds system prompt via `peer_discussion::build_peer_discussion_prompt()`
- Builds user prompt via `peer_discussion::build_user_prompt()`
- Calls `provider.complete(request)`
- Post-processes output (clean text, format sections)
- Saves context to recording metadata
- Persists `recording.peer_discussion = Some(text)` to DB
- Emits `generation-progress` events (started, completed, failed)

### Update existing files

**`src-tauri/src/commands/generation/mod.rs`**:
- Add `pub mod peer_discussion;`
- Add `generate_peer_discussion` to invoke handler

**`src-tauri/src/commands/settings.rs`**:
- Add `"peer_discussion"` match arm in `get_default_prompt` returning default prompt

**`src-tauri/src/lib.rs`** (or equivalent):
- Register `generate_peer_discussion` command

### Prompt config struct

```rust
pub struct PeerDiscussionPromptConfig {
    pub physician_name: String,
    pub specialty: String,
    pub reason: String,
    pub custom_prompt: Option<String>,
}
```

## Frontend (Svelte)

### Sidebar (`src/lib/components/Sidebar.svelte`)

Add to `documentNav`:

```typescript
{ id: 'peer_discussion', label: 'Peer Discussion', icon: '👥' }
```

### App.svelte routing

Add tab routing:

```svelte
{:else if activeTab === 'peer_discussion'}
  <EditorTab tabId="peer_discussion" />
```

### EditorTab (`src/lib/pages/EditorTab.svelte`)

- Extend `tabId` prop type: `'transcript' | 'soap' | 'referral' | 'letter' | 'peer_discussion'`
- Add config entry: `peer_discussion` → `recording.peer_discussion`

### GenerateTab (`src/lib/pages/GenerateTab.svelte`)

- Add state: `physicianName`, `specialty`, `reason`
- Add handler `handleGeneratePeerDiscussion()` that calls `generatePeerDiscussion(recordingId, physicianName, specialty, reason)`
- Pass handler to `GenerateControls`

### GenerateControls (`src/lib/components/GenerateControls.svelte`)

Add new card after Letter card:

- **Peer Discussion card** with three input fields:
  - Physician Name (text input)
  - Specialty (text input)
  - Reason for Discussion (textarea)
- Generate button triggers `onGeneratePeerDiscussion` callback
- Shows generating state, done state, copy/regenerate buttons (same as other cards)

### Generation API (`src/lib/api/generation.ts`)

```typescript
export async function generatePeerDiscussion(
  recordingId: string,
  physicianName: string,
  specialty: string,
  reason: string
): Promise<string> {
  return invoke('generate_peer_discussion', {
    recordingId,
    physicianName,
    specialty,
    reason,
  });
}
```

### Generation store (`src/lib/stores/generation.svelte.ts`)

- Add `'peer_discussion'` to `GeneratingType` union

### Prompts API (`src/lib/api/prompts.ts`)

- Add `'peer_discussion'` to `DocType` union

### Prompts settings (`src/lib/components/settings/Prompts.svelte`)

- Add tab for customizing the peer discussion prompt
- Loads/saves `custom_peer_discussion_prompt` in AppConfig

## System Prompt

```
You are a physician creating a structured peer-to-peer discussion note from a patient consultation transcript.

RULES:
1. The transcript is the sole source of truth for clinical content.
2. Never fabricate clinical information, diagnoses, medications, or findings not present in the transcript.
3. Background context (patient records, prior notes) may populate historical fields only.
4. Use professional physician voice throughout.
5. Focus on the clinical discussion between physicians.

FORBIDDEN INFERENCES:
- Do not infer patient demographics not stated in the transcript
- Do not infer medications or dosages not mentioned
- Do not infer family history not discussed
- Do not infer physical exam findings not described
- Do not infer follow-up intervals not specified
- Do not infer provider names not mentioned

OUTPUT FORMAT:

HEADER
Physician: {physician_name}
Reason for Discussion: {reason}

CLINICAL SUMMARY
- Brief patient history relevant to the discussion
- Current clinical status
- Pertinent findings and results

DISCUSSION POINTS
- Key clinical questions addressed
- Specific issues discussed
- Areas of clinical uncertainty or disagreement

ASSESSMENT
- Clinical opinion based on discussion
- Differential diagnosis considerations
- Risk stratification if applicable

RECOMMENDATIONS
- Suggested diagnostic workup
- Treatment plan modifications
- Follow-up recommendations

ACTION ITEMS
- Specific tasks assigned
- Responsible parties
- Timeline for completion

FORMATTING RULES:
- Use plain text only
- Use dash-prefixed lines for list items
- Include all sections even if brief
- Keep language professional and clinical

SELF-CHECK:
1. Header contains physician name and reason for discussion
2. Clinical summary is grounded in transcript content only
3. Discussion points reflect actual conversation topics
4. Assessment is supported by discussed findings
5. Recommendations are actionable and specific
6. Action items have clear ownership and timeline
```

## File Changes Summary

| File | Change |
|---|---|
| `crates/core/src/types/recording.rs` | Add `peer_discussion: Option<String>` |
| `crates/core/src/types/settings.rs` | Add `custom_peer_discussion_prompt: Option<String>` |
| `crates/db/src/migrations/m008_peer_discussion.rs` | New migration for `peer_discussion` column |
| `crates/processing/src/peer_discussion/mod.rs` | New module |
| `crates/processing/src/peer_discussion/prompt_template.rs` | New prompt template |
| `crates/processing/src/peer_discussion/user_prompt.rs` | New user prompt builder |
| `crates/processing/src/lib.rs` | Add `pub mod peer_discussion;` |
| `src-tauri/src/commands/generation/peer_discussion.rs` | New Tauri command |
| `src-tauri/src/commands/generation/mod.rs` | Add module + command registration |
| `src-tauri/src/commands/settings.rs` | Add `get_default_prompt` arm |
| `src/lib/types/index.ts` | Add `peer_discussion` to Recording + AppConfig |
| `src/lib/api/generation.ts` | Add `generatePeerDiscussion()` |
| `src/lib/api/prompts.ts` | Add `'peer_discussion'` to DocType |
| `src/lib/stores/generation.svelte.ts` | Add `'peer_discussion'` to GeneratingType |
| `src/lib/components/Sidebar.svelte` | Add sidebar entry |
| `src/App.svelte` | Add tab routing |
| `src/lib/pages/EditorTab.svelte` | Add tabId support |
| `src/lib/pages/GenerateTab.svelte` | Add state + handler |
| `src/lib/components/GenerateControls.svelte` | Add Peer Discussion card |
| `src/lib/components/settings/Prompts.svelte` | Add prompt customization tab |

## Testing

- **Rust unit tests:** Prompt generation, user prompt assembly, post-processing
- **Frontend tests:** Component rendering, API call mocking, store updates
- **Integration:** Generate peer discussion from test recording, verify output structure
