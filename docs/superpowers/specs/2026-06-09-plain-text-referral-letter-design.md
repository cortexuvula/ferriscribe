# Plain-Text Output for Referral & Letter Generation

**Date:** 2026-06-09
**Status:** Approved

## Problem

AI models default to markdown syntax (`**bold**`, `## headers`, `- bullets`) in generated referral letters and patient letters. The TipTap editor renders this as rich text, but the PDF/DOCX exporters do not parse markdown — exported documents contain raw markdown characters like `**` and `#`. The goal is to produce clean plain-text output that exports correctly without any markdown artifacts.

## Approach

**Belt-and-suspenders:** instruct the AI to produce plain text via prompt changes, and strip residual markdown from the output as a safety net before storing.

### Scope

- Generation pipeline only (`document_generator.rs` + `referral.rs` + `letter.rs` generation commands).
- ReferralAgent chat agent — **not changed**.
- Letter audience prompts — **not changed** (user-configurable, out of scope).

## Changes

### 1. Update default prompts (`crates/processing/src/document_generator.rs`)

Append to `default_referral_prompt()`:
```
Do not use markdown formatting. Write in plain text only. You may use uppercase headings (e.g., REASON FOR REFERRAL:) for structure.
```

Append to `default_letter_prompt()`:
```
Do not use markdown formatting. Write in plain text only. You may use uppercase headings for structure.
```

These instructions are only in the built-in default prompts. Custom templates (user-supplied or letter-audience-supplied) are left unchanged — the strip function handles those.

### 2. Add `strip_markdown()` function (`crates/processing/src/document_generator.rs`)

A public function that removes common markdown syntax from a string:

| Pattern | Replacement |
|---------|-------------|
| `**bold**` | `bold` |
| `*italic*` / `_italic_` | `italic` |
| `# Heading` | `HEADING` (uppercase) |
| `- item` / `* item` | `• item` |
| `` `code` `` | `code` |
| `[text](url)` | `text` |
| `---` (horizontal rule) | *(removed)* |

Implementation: regex-based replacements using the `regex` crate (already a workspace dependency).

### 3. Apply strip in generation commands

**`src-tauri/src/commands/generation/referral.rs:146`:**
```rust
let referral_text = medical_processing::document_generator::strip_markdown(&response.content);
```

**`src-tauri/src/commands/generation/letter.rs:157`:**
```rust
let letter_text = medical_processing::document_generator::strip_markdown(&response.content);
```

### 4. Export `strip_markdown` from processing crate

Ensure `document_generator` module and `strip_markdown` are publicly accessible from `medical_processing`.

## Files to Modify

1. `crates/processing/src/document_generator.rs` — update default prompts, add `strip_markdown()`
2. `src-tauri/src/commands/generation/referral.rs` — apply strip on line 146
3. `src-tauri/src/commands/generation/letter.rs` — apply strip on line 157
4. `crates/processing/src/lib.rs` — ensure `pub use` or module visibility

## No Changes Needed

- **RichEditor** — displays plain text as-is (no markdown to render)
- **PDF/DOCX exporters** — already render line-by-line as plain text
- **RSVP speed reader** — already treats content as plain text
- **Database schema** — still `Option<String>`

## Testing

- Unit tests for `strip_markdown()` covering each pattern type
- Verify existing `document_generator` tests still pass
- Manual test: generate a referral/letter and confirm no markdown in output
- Manual test: export to PDF/DOCX and confirm clean plain text
