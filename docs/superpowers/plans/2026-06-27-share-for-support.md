# Share for Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a "Share logs for support" button in Settings → About that exports all app logs as a PHI-redacted plain-text file the user can send to a support person.

**Architecture:** A new Tauri command (`export_support_bundle`) reads all `.log` files from the log directory, concatenates them with headers, runs the result through `PhiRedactor::redact()`, and writes the file to a user-chosen path. The frontend calls `saveDialog` from `@tauri-apps/plugin-dialog` to get the path, then invokes the command.

**Tech Stack:** Rust (Tauri command, `PhiRedactor`), Svelte 5 (About.svelte button), `@tauri-apps/plugin-dialog`

---

### Task 1: Backend — Create the support command module

**Files:**
- Create: `src-tauri/src/commands/support.rs`
- Modify: `src-tauri/src/commands/mod.rs` (add module declaration)
- Modify: `src-tauri/src/lib.rs` (register the command)

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/commands/support.rs` with the test first:

```rust
//! Support bundle export — PHI-redacted log file concatenation for
//! troubleshooting. The user triggers this from Settings → About →
//! "Share logs for support".

use std::path::Path;

use medical_core::error::{AppError, AppResult};
use medical_security::phi_redactor::PhiRedactor;

/// Generate a PHI-redacted support bundle from all log files in `log_dir`.
///
/// Reads every `.log` file (sorted oldest-first by modified time),
/// concatenates them with `=== <filename> ===` separators, prepends a
/// bundle header with app version + timestamp, and runs the entire string
/// through [`PhiRedactor::redact`].
pub fn export_support_bundle_inner(log_dir: &Path) -> AppResult<String> {
    Err(AppError::Other("not yet implemented".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn bundle_includes_header_and_file_separators() {
        let dir = TempDir::new().unwrap();
        // Create two fake log files
        fs::write(dir.path().join("app-2026-06-26.log"), "2026-06-26 INFO Old log entry\n").unwrap();
        fs::write(dir.path().join("app-2026-06-27.log"), "2026-06-27 INFO New log entry\n").unwrap();

        let bundle = export_support_bundle_inner(dir.path()).unwrap();

        assert!(bundle.contains("FerriScribe Support Bundle"), "must have bundle header");
        assert!(bundle.contains("=== "), "must have file separators");
        assert!(bundle.contains("Old log entry"), "must include older file content");
        assert!(bundle.contains("New log entry"), "must include newer file content");
    }

    #[test]
    fn bundle_redacts_phi() {
        let dir = TempDir::new().unwrap();
        // A log line containing a phone number (PHI)
        let phi_line = "2026-06-27 INFO Called patient at (604) 555-0199\n";
        fs::write(dir.path().join("app.log"), phi_line).unwrap();

        let bundle = export_support_bundle_inner(dir.path()).unwrap();

        assert!(
            !bundle.contains("(604) 555-0199"),
            "phone number must be redacted: got:\n{bundle}"
        );
    }

    #[test]
    fn bundle_errors_when_no_logs() {
        let dir = TempDir::new().unwrap();
        let result = export_support_bundle_inner(dir.path());
        assert!(result.is_err(), "empty directory should error");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rust-medical-assistant --lib commands::support`
Expected: FAIL — all 3 tests fail with "not yet implemented"

- [ ] **Step 3: Implement `export_support_bundle_inner`**

Replace the stub body of `export_support_bundle_inner` in `src-tauri/src/commands/support.rs` with:

```rust
pub fn export_support_bundle_inner(log_dir: &Path) -> AppResult<String> {
    // 1. Collect all .log files sorted by modified time (oldest first).
    let mut log_files: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    let entries = std::fs::read_dir(log_dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("log") {
            if let Ok(meta) = path.metadata()
                && let Ok(modified) = meta.modified()
            {
                log_files.push((modified, path));
            }
        }
    }

    if log_files.is_empty() {
        return Err(AppError::Other(
            "No log files found in the log directory.".into(),
        ));
    }

    log_files.sort_by_key(|(time, _)| *time);

    // 2. Concatenate with file separators.
    let mut bundle = String::new();
    bundle.push_str("FerriScribe Support Bundle\n");
    bundle.push_str(&format!("Version: {}\n", env!("CARGO_PKG_VERSION")));
    bundle.push_str(&format!(
        "Generated: {}\n",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    ));
    bundle.push_str(&format!("Log files: {}\n\n", log_files.len()));

    for (_, path) in &log_files {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        bundle.push_str(&format!("=== {filename} ===\n"));
        match std::fs::read_to_string(path) {
            Ok(content) => bundle.push_str(&content),
            Err(e) => bundle.push_str(&format!("[read error: {e}]\n")),
        }
        bundle.push('\n');
    }

    // 3. PHI-redact the entire bundle.
    Ok(PhiRedactor::redact(&bundle))
}
```

Add the `chrono` usage at the top of the file. The full imports section should be:

```rust
use std::path::Path;

use medical_core::error::{AppError, AppResult};
use medical_security::phi_redactor::PhiRedactor;
```

- [ ] **Step 4: Add the Tauri command wrapper + register it**

Add below `export_support_bundle_inner` in `src-tauri/src/commands/support.rs`:

```rust
use crate::state::AppState;

/// Export all app logs as a PHI-redacted plain-text file.
///
/// Reads every `.log` file from the log directory, concatenates them,
/// redacts PHI (phone numbers, SSNs, emails, DOBs, MRNs, addresses),
/// and writes the result to `file_path`. The user chooses the path via
/// a frontend save-file dialog.
#[tauri::command]
pub async fn export_support_bundle(file_path: String) -> AppResult<()> {
    let path = std::path::PathBuf::from(&file_path);
    let log_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rust-medical-assistant")
        .join("logs");

    let bundle = tokio::task::spawn_blocking(move || {
        export_support_bundle_inner(&log_dir)
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    std::fs::write(&path, &bundle)?;
    Ok(())
}
```

- [ ] **Step 5: Add the module to `mod.rs`**

In `src-tauri/src/commands/mod.rs`, add `pub mod support;` after the last `pub mod` line (before any non-`pub mod` lines).

- [ ] **Step 6: Register the command in `lib.rs`**

In `src-tauri/src/lib.rs`, add this line inside the `generate_handler![...]` macro, after `commands::recovery::database_encryption_status,`:

```rust
            commands::support::export_support_bundle,
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p rust-medical-assistant --lib commands::support`
Expected: PASS — all 3 tests pass

- [ ] **Step 8: Build to verify compilation**

Run: `cargo build -p rust-medical-assistant`
Expected: clean build, no errors

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/commands/support.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs
git commit -m "feat(support): add export_support_bundle command (PHI-redacted logs)"
```

---

### Task 2: Frontend — Add the "Share logs for support" button to About.svelte

**Files:**
- Modify: `src/lib/components/settings/About.svelte`
- Create: `src/lib/api/support.ts` (thin invoke wrapper)

- [ ] **Step 1: Create the API wrapper**

Create `src/lib/api/support.ts`:

```typescript
import { invoke } from '@tauri-apps/api/core';

/** Export PHI-redacted application logs to a user-chosen file path. */
export async function exportSupportBundle(filePath: string): Promise<void> {
  await invoke('export_support_bundle', { filePath });
}
```

- [ ] **Step 2: Add the button to About.svelte**

In `src/lib/components/settings/About.svelte`, add imports after the existing imports:

```typescript
  import { save as saveDialog } from '@tauri-apps/plugin-dialog';
  import { exportSupportBundle } from '../../api/support';
  import { toasts } from '../../stores/toasts.svelte';
  import { formatError } from '../../types/errors';
```

Add state + handler after `async function checkNow()`:

```typescript
  let preparingLogs = $state(false);

  async function shareLogsForSupport() {
    preparingLogs = true;
    try {
      const selected = await saveDialog({
        title: 'Save support logs',
        defaultPath: 'ferriscribe-support-logs.txt',
        filters: [{ name: 'Text', extensions: ['txt'] }],
      });
      if (!selected) return; // user cancelled

      await exportSupportBundle(selected);
      toasts.success('Support logs saved. Send the file to your support contact.');
    } catch (e) {
      toasts.error(formatError(e));
    } finally {
      preparingLogs = false;
    }
  }
```

Add the button in the markup, after the update-check section (before the closing `</div>` of `.about-pane`):

```svelte
  <div class="form-group">
    <h3>Support</h3>
    <button
      class="btn-check"
      onclick={shareLogsForSupport}
      disabled={preparingLogs}
    >
      {preparingLogs ? 'Preparing…' : 'Share logs for support'}
    </button>
    <span class="form-hint">Exports application logs with PHI (phone numbers, emails, etc.) automatically redacted. Safe to share with support.</span>
  </div>
```

- [ ] **Step 3: Verify type-check passes**

Run: `npm run check`
Expected: 0 errors (same warning count as before)

- [ ] **Step 4: Verify vitest still passes**

Run: `npx vitest run`
Expected: 355 passed (no new tests, no regressions)

- [ ] **Step 5: Commit**

```bash
git add src/lib/api/support.ts src/lib/components/settings/About.svelte
git commit -m "feat(support): add Share logs for support button in About"
```

---

### Task 3: Final verification + version bump

**Files:**
- Modify: `src-tauri/Cargo.toml`, `package.json`, `src-tauri/tauri.conf.json`

- [ ] **Step 1: Run full workspace test suite**

Run: `cargo test --workspace --lib`
Expected: all crates pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 0 warnings

- [ ] **Step 3: Run fmt**

Run: `cargo fmt --all && cargo fmt --all -- --check`
Expected: clean

- [ ] **Step 4: Version bump (optional — check with user)**

If releasing: bump `0.24.0` → `0.24.1` (patch — new feature in About settings) across all 3 files.

- [ ] **Step 5: Commit version bump**

```bash
git add src-tauri/Cargo.toml package.json src-tauri/tauri.conf.json
git commit -m "chore: bump version to 0.24.1"
```
