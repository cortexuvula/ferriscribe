# Share for Support — Design

**Date:** 2026-06-27
**Status:** Approved

## Purpose

Allow a clinician to export their application logs as a PHI-redacted plain-text file they can send to a support person for troubleshooting.

## Bundle contents

Application logs only (no system info, no settings). All available `.log` files from the log directory, concatenated oldest-to-newest, each with a header showing the filename. PHI-redacted via the existing `PhiRedactor`.

## File format

Plain text (`.txt`). A single file containing:

1. A bundle header with app version, generated-at timestamp, and log file count.
2. For each log file (oldest first): a `=== <filename> ===` separator, followed by the file's contents.

## Architecture

### Backend (Rust)

New module `src-tauri/src/commands/support.rs` with:

- `export_support_bundle()` — `#[tauri::command] pub async fn` that runs on `spawn_blocking`.
- `export_support_bundle_inner(log_dir: &Path) -> AppResult<String>` — testable inner function that:
  1. Reads the log directory, finds all `.log` files.
  2. Sorts by modified time (oldest first).
  3. Concatenates with `=== <filename> ===` separators.
  4. Prepends a bundle header: `FerriScribe Support Bundle\nVersion: X.Y.Z\nGenerated: <ISO timestamp>\nLog files: N\n\n`.
  5. Runs the entire string through `PhiRedactor::redact()`.
  6. Returns the redacted text.

### Frontend (Svelte)

- New button "Share logs for support" in `About.svelte` (Settings → About section, below the update controls).
- On click: calls `invoke('export_support_bundle')`, then `save()` from `@tauri-apps/plugin-dialog` with default filename `ferriscribe-support-logs.txt`, writes via `writeTextFile` from `@tauri-apps/plugin-fs` (or `@tauri-apps/plugin-dialog`'s returned path + a Rust write command — whichever is simpler). Shows a success toast on completion, error toast on failure.
- Button shows a "Preparing…" state while the command runs.

### Data flow

```
About.svelte button → invoke('export_support_bundle')
  → spawn_blocking: read all .log files → sort → concatenate → PhiRedactor::redact()
  → returns String (PHI-free)
  → frontend: save() dialog → writeTextFile → success toast
```

## PHI safety

The redaction happens on the Rust side before the string crosses IPC. The `PhiRedactor` strips: SSNs, phone numbers, email addresses, DOBs, MRNs, addresses, ZIP codes. If redaction fails, the command returns an error rather than raw logs.

## Error handling

- No log files found → `AppError::Other("No log files found")` → error toast.
- File read failure → propagate as `AppError` → error toast.
- User cancels save dialog → silently abort (no toast).

## Testing

- **Rust:** unit test that `export_support_bundle_inner` produces a bundle with the correct header, includes file separators, and that a known PHI pattern (e.g., a phone number) in the log text is redacted in the output.
- **Frontend:** no new test needed — the button is a thin wrapper around `invoke` + dialog + toast, all already-tested patterns.

## Registration

- Register `export_support_bundle` in the `generate_handler!` macro in `src-tauri/src/lib.rs`.
- Add `pub mod support;` to `src-tauri/src/commands/mod.rs`.

## Out of scope

- No system info report (OS, provider names).
- No settings export.
- No zip/archive format.
- No auto-upload or email integration.
