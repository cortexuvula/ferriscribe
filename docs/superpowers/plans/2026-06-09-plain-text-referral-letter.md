# Plain-Text Referral & Letter Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make referral and letter generation produce clean plain-text output (no markdown syntax) via prompt changes and a post-processing strip function.

**Architecture:** Update the two built-in default prompts to request plain text, then add a `strip_markdown()` function as a safety net that runs on AI output before storing. The strip is applied in the Tauri generation commands for both referral and letter.

**Tech Stack:** Rust, `regex` crate (already a workspace dependency)

**Spec:** `docs/superpowers/specs/2026-06-09-plain-text-referral-letter-design.md`

---

## File Map

| File | Action | Purpose |
|------|--------|---------|
| `crates/processing/src/document_generator.rs` | Modify | Update default prompts, add `strip_markdown()` |
| `src-tauri/src/commands/generation/referral.rs` | Modify | Apply `strip_markdown()` to AI output |
| `src-tauri/src/commands/generation/letter.rs` | Modify | Apply `strip_markdown()` to AI output |

---

### Task 1: Add `strip_markdown()` with tests

**Files:**
- Modify: `crates/processing/src/document_generator.rs`
- Test: inline `#[cfg(test)]` module in same file

- [ ] **Step 1: Write failing tests for `strip_markdown()`**

Add these tests at the end of the `tests` module in `document_generator.rs`:

```rust
#[test]
fn strip_markdown_removes_bold() {
    assert_eq!(strip_markdown("**important**"), "important");
}

#[test]
fn strip_markdown_removes_italic() {
    assert_eq!(strip_markdown("*emphasis*"), "emphasis");
    assert_eq!(strip_markdown("_emphasis_"), "emphasis");
}

#[test]
fn strip_markdown_converts_heading_to_uppercase() {
    assert_eq!(strip_markdown("## Reason for Referral"), "REASON FOR REFERRAL");
}

#[test]
fn strip_markdown_converts_bullets() {
    assert_eq!(strip_markdown("- First item"), "• First item");
    assert_eq!(strip_markdown("* First item"), "• First item");
}

#[test]
fn strip_markdown_removes_inline_code() {
    assert_eq!(strip_markdown("use `metric` units"), "use metric units");
}

#[test]
fn strip_markdown_removes_links() {
    assert_eq!(strip_markdown("[click here](http://example.com)"), "click here");
}

#[test]
fn strip_markdown_removes_horizontal_rules() {
    let input = "Above\n\n---\n\nBelow";
    assert_eq!(strip_markdown(input), "Above\n\nBelow");
}

#[test]
fn strip_markdown_preserves_plain_text() {
    let input = "Dear Dr Smith,\n\nI am writing to refer the patient.\n\nSincerely,\nDr Jones";
    assert_eq!(strip_markdown(input), input);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p medical-processing --lib document_generator::tests::strip_markdown`
Expected: FAIL (functions don't exist yet)

- [ ] **Step 3: Implement `strip_markdown()`**

Add this function to `document_generator.rs` after the `resolve_audience_user_template` function (around line 221):

```rust
/// Remove common markdown syntax from AI-generated text.
///
/// Converts headings to uppercase, replaces bullets with `•`, strips bold/italic
/// markers, inline code backticks, link syntax, and horizontal rules. Intended
/// as a safety net when prompts request plain text but the model produces markdown.
pub fn strip_markdown(text: &str) -> String {
    use regex::Regex;
    use std::sync::LazyLock;

    static HEADING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^#{1,6}\s+(.+)$").unwrap());
    static BOLD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*").unwrap());
    static ITALIC_STAR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)").unwrap());
    static ITALIC_UNDER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?<!_)_(?!_)(.+?)(?<!_)_(?!_)").unwrap());
    static INLINE_CODE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
    static LINK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\([^)]+\)").unwrap());
    static BULLET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^(\s*)[*-]\s+").unwrap());
    static HR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^[-*_]{3,}\s*$").unwrap());

    let mut out = text.to_string();

    // Convert headings to uppercase lines
    out = HEADING
        .replace_all(&out, |caps: &regex::Captures| {
            caps[1].to_uppercase()
        })
        .into_owned();

    // Strip bold
    out = BOLD.replace_all(&out, "$1").into_owned();

    // Strip italic (star and underscore)
    out = ITALIC_STAR.replace_all(&out, "$1").into_owned();
    out = ITALIC_UNDER.replace_all(&out, "$1").into_owned();

    // Strip inline code
    out = INLINE_CODE.replace_all(&out, "$1").into_owned();

    // Strip links (keep text)
    out = LINK.replace_all(&out, "$1").into_owned();

    // Replace bullets with bullet character
    out = BULLET.replace_all(&out, "${1}• ").into_owned();

    // Remove horizontal rules (line entirely)
    out = HR.replace_all(&out, "").into_owned();

    // Collapse runs of 3+ blank lines to 2
    static MULTI_BLANK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());
    out = MULTI_BLANK.replace_all(&out, "\n\n").into_owned();

    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p medical-processing --lib document_generator::tests::strip_markdown`
Expected: all 8 tests PASS

- [ ] **Step 5: Run full document_generator test suite**

Run: `cargo test -p medical-processing --lib document_generator::tests`
Expected: all tests PASS (existing + new)

- [ ] **Step 6: Commit**

```bash
git add crates/processing/src/document_generator.rs
git commit -m "feat: add strip_markdown() for plain-text referral/letter output"
```

---

### Task 2: Update default prompts

**Files:**
- Modify: `crates/processing/src/document_generator.rs:63-82`

- [ ] **Step 1: Update `default_referral_prompt()`**

Change lines 63-70 to:

```rust
pub fn default_referral_prompt() -> &'static str {
    "You are a medical scribe assistant specialising in professional referral letters. \
     Write a formal referral letter addressed to a {recipient_type}. \
     The urgency of this referral is: {urgency}. \
     Use appropriate clinical language, include relevant history and findings from the SOAP \
     note, clearly state the reason for referral, and request the desired action. \
     Format the letter professionally with greeting, body, and closing. \
     Do not use markdown formatting. Write in plain text only. \
     You may use uppercase headings (e.g., REASON FOR REFERRAL:) for structure."
}
```

- [ ] **Step 2: Update `default_letter_prompt()`**

Change lines 76-82 to:

```rust
pub fn default_letter_prompt() -> &'static str {
    "You are a medical scribe assistant helping to write patient-friendly correspondence. \
     Generate a {letter_type} letter for the patient. \
     Use clear, plain language the patient can understand. \
     Avoid unexplained medical jargon. \
     Be empathetic and professional. \
     Do not use markdown formatting. Write in plain text only. \
     You may use uppercase headings for structure."
}
```

- [ ] **Step 3: Run all document_generator tests**

Run: `cargo test -p medical-processing --lib document_generator::tests`
Expected: all tests PASS (prompt content assertions still hold)

- [ ] **Step 4: Commit**

```bash
git add crates/processing/src/document_generator.rs
git commit -m "feat: update default referral/letter prompts to request plain text"
```

---

### Task 3: Apply strip in generation commands

**Files:**
- Modify: `src-tauri/src/commands/generation/referral.rs:146`
- Modify: `src-tauri/src/commands/generation/letter.rs:157`

- [ ] **Step 1: Apply strip in referral generation**

In `src-tauri/src/commands/generation/referral.rs`, change line 146 from:

```rust
let referral_text = response.content;
```

to:

```rust
let referral_text = medical_processing::document_generator::strip_markdown(&response.content);
```

- [ ] **Step 2: Apply strip in letter generation**

In `src-tauri/src/commands/generation/letter.rs`, change line 157 from:

```rust
let letter_text = response.content;
```

to:

```rust
let letter_text = medical_processing::document_generator::strip_markdown(&response.content);
```

- [ ] **Step 3: Build the Tauri app to verify compilation**

Run: `cargo build -p rust-medical-assistant`
Expected: successful build with no errors

- [ ] **Step 4: Run workspace lib tests**

Run: `cargo test --workspace --lib`
Expected: all tests PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/generation/referral.rs src-tauri/src/commands/generation/letter.rs
git commit -m "feat: apply strip_markdown() to referral and letter generation output"
```
