//! Crash-safe app restart for the update flow, with coordinated shutdown.
//!
//! # Why this exists (2026-09-08 crash investigation)
//!
//! The auto-updater's relaunch ends in tauri's `process::restart`, whose
//! final step is `std::process::exit(0)` — which runs the C++ static
//! destructors (`__cxa_finalize_ranges`). The statically linked ONNX
//! Runtime / kaldi-native-fbank block (the diarization pipeline's `ort`
//! download-binaries) registers a destructor that **aborts** when the
//! process exits with the app's worker threads still live. Every
//! auto-update relaunch since the diarization pipeline shipped produced a
//! SIGABRT crash report on quit (2026-09-07 and 2026-09-08, both stacks:
//! `exit → __cxa_finalize_ranges → abort`, frames inside the ORT/knf C++
//! island between the only named C++ symbols in the binary). The crash is
//! cosmetic in effect — the replacement process starts and runs — but it
//! surfaces a crash dialog per update and buries the real story.
//!
//! # The fix
//!
//! `restart_app` sets a flag and calls tauri's own restart (which spawns
//! the replacement correctly and then exits — no spawn logic of ours).
//! The [`install_exit_guard`] call at boot registers an `atexit` handler
//! from `run()`, i.e. AFTER every pre-main C++ static registration — and
//! atexit runs LIFO, so the guard executes FIRST at exit. When the
//! restart flag is set it terminates via `libc::_exit(0)`: the remaining
//! atexit chain — including the ORT/knf destructor that aborts — never
//! runs. Normal quits leave the flag unset: the guard no-ops and every
//! destructor runs exactly as before (no abort was ever observed on a
//! normal quit, only on the update relaunch).
//!
//! Skipping static destructors is safe for the restart path: every
//! durable write in this app is committed synchronously before this point
//! (SQLite WAL commits, atomic fsync+rename file writes), and Rust
//! destructors do not run at process exit anyway — the only teardown
//! being skipped is the C++ runtime's, which holds no user state.
//!
//! # Coordinated shutdown (U1: restart can destroy active work)
//!
//! "Restart now" used to relaunch immediately. A recording in flight is
//! only registered/finalized in `stop_recording` — a restart mid-take
//! left the WAV outside the recordings list (the orphan sweep does not
//! reconstruct recording rows). A pending debounced editor save was
//! discarded entirely. Both are unacceptable for clinical data, so
//! `restart_app` now REFUSES to restart while work is in flight:
//!
//! - **Active recording (or translation capture)** → `InvalidInput`
//!   refusal. The user must stop the recording first (that path persists
//!   and registers everything); auto-restarting a capture in progress
//!   cannot be done safely from here because finalization runs in the
//!   stop path's task, not under this command's control.
//! - **Pending debounced editor save** → flushed via the
//!   `save_recording_field` command on the exact pending edit, then
//!   awaited. Save failure → `Database` error refusal (the edit was NOT
//!   lost — the frontend still holds `pendingValue` until its own save
//!   completes — but we must not exit into a state where the only copy
//!   lives in a dying webview).
//!
//! Refusals never exit: the app, the unsaved edit, and the "restart
//! required" banner all survive, and the frontend surfaces the reason
//! (this replaces the previous console-only catch that could strand the
//! user with no feedback). Only when every check passes does the flag
//! get set and `_exit` teardown begin — the guard's contract (settle
//! work BEFORE setting `RESTARTING`, never switch to a "graceful" exit
//! path that would re-run the aborting destructor chain) is unchanged.

use std::sync::atomic::{AtomicBool, Ordering};

use medical_core::error::{AppError, AppResult};
use tauri::State;

use crate::state::{AppState, PendingEdit};

/// Set when the process is exiting ONLY to relaunch a fresh copy (the
/// update flow). Read by [`exit_guard`] — see the module docs.
static RESTARTING: AtomicBool = AtomicBool::new(false);

/// Terminate immediately when `restarting`, skipping the remaining atexit
/// chain. Split from the `extern "C"` shim so the no-op path is testable
/// (the `_exit` path terminates the process by construction).
fn exit_guard_if(restarting: bool) {
    if restarting {
        // _exit (not exit): atexit handlers — including the ORT/knf C++
        // static destructor that aborts — must not run on a restart exit.
        unsafe { libc::_exit(0) }
    }
}

/// atexit shim. Registered from `run()` (after all pre-main static
/// registrations) so LIFO ordering makes it the FIRST handler to run at
/// process exit.
extern "C" fn exit_guard() {
    exit_guard_if(RESTARTING.load(Ordering::SeqCst));
}

/// Register the exit guard. Call exactly once, early in `run()`.
pub fn install_exit_guard() {
    // Best-effort: atexit failing (ENOMEM under fd/thread pressure) means
    // restarts exit through the full destructor chain — the pre-fix
    // behavior (crash report, relaunch still succeeds) — never a hard
    // error worth failing boot over.
    let rc = unsafe { libc::atexit(exit_guard) };
    if rc != 0 {
        tracing::warn!(
            rc,
            "restart exit guard registration failed — update relaunches may show a quit-time crash report"
        );
    }
}

/// Why a restart was refused. Returned to the frontend as a typed
/// `AppError::InvalidInput` whose message starts with a stable machine
/// prefix so both update surfaces (UpdateBanner, Settings → About) can
/// render the right guidance without string-matching free prose.
pub mod refusal {
    /// A recording (or translation capture) is in flight.
    pub const RECORDING_ACTIVE: &str = "RESTART_REFUSED_RECORDING_ACTIVE: stop the recording first";
    /// A pending editor save failed to flush.
    pub const SAVE_FAILED: &str = "RESTART_REFUSED_SAVE_FAILED";
}

/// Flush one pending debounced editor edit by invoking the same save
/// command the debounced timer would have run. Shared by the sync and
/// async paths of [`restart_app`].
///
/// Content never enters logs (PHI): the edit's value is passed through
/// to the save command untouched; only lengths are logged.
async fn flush_pending_edit(state: &AppState, edit: &PendingEdit) -> Result<(), AppError> {
    let db = state.db.clone();
    super::recordings_edit::save_recording_field_core(
        &db,
        &edit.recording_id,
        &edit.field,
        &edit.value,
    )
    .await
}

/// Coordinated-shutdown checks shared by [`restart_app`]. Returns
/// `Ok(())` when it is safe to relaunch, or the refusal error. Pure
/// logic over the AppState — no exit, no flag mutation — so it is
/// unit-testable without terminating the test process.
async fn ensure_safe_to_restart(state: &AppState) -> Result<(), AppError> {
    if *state.recording_active.lock().await {
        return Err(AppError::InvalidInput(
            refusal::RECORDING_ACTIVE.to_string(),
        ));
    }
    if let Some(edit) = state.pending_edit.lock().await.take() {
        match flush_pending_edit(state, &edit).await {
            Ok(()) => {
                tracing::info!(
                    value_len = edit.value.len(),
                    "pending editor edit flushed before restart"
                );
            }
            Err(e) => {
                // Put the edit BACK: the frontend's debounce timer may
                // already have cleared its pendingValue when it delegated
                // to us, so the backend copy is the only guaranteed one.
                // The user keeps their work and the error; nothing exits.
                *state.pending_edit.lock().await = Some(edit);
                return Err(AppError::Database {
                    message: format!("{} ({e})", refusal::SAVE_FAILED),
                    source: None,
                });
            }
        }
    }
    Ok(())
}

/// Register the freshest pending editor edit (called by the frontend at
/// edit time, before the 1 s debounce fires). Overwrites any previous
/// registration — only the latest edit needs flushing. Content is never
/// logged (PHI).
#[tauri::command]
pub async fn register_pending_edit(
    state: State<'_, AppState>,
    recording_id: String,
    field: String,
    value: String,
) -> AppResult<()> {
    *state.pending_edit.lock().await = Some(PendingEdit {
        recording_id,
        field,
        value,
    });
    Ok(())
}

/// Clear the pending-edit registration once the frontend's save has
/// completed (the edit is now durable in the DB).
#[tauri::command]
pub async fn clear_pending_edit(state: State<'_, AppState>, field: String) -> AppResult<()> {
    let mut guard = state.pending_edit.lock().await;
    if let Some(edit) = guard.as_ref()
        && edit.field == field
    {
        *guard = None;
    }
    Ok(())
}

/// Relaunch the app after an update install, crash-safely — but only
/// after coordinated shutdown (see module docs). On success never
/// returns: tauri's `restart()` spawns the replacement and exits the
/// current process (both its exit paths — the direct main-thread one and
/// the run-loop one — run atexit, where the guard converts the exit).
/// On refusal, returns an error and the app keeps running with the
/// user's work intact.
#[tauri::command]
pub async fn restart_app(app: tauri::AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    ensure_safe_to_restart(&state).await?;
    // All work settled — only now set the flag. Ordering matters: the
    // guard must never fire with unsaved work still in flight.
    RESTARTING.store(true, Ordering::SeqCst);
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard must be inert on a normal exit — every destructor runs
    /// exactly as they did before this module existed.
    #[test]
    fn exit_guard_noops_when_not_restarting() {
        exit_guard_if(false); // returns — the test process survives
        assert!(!RESTARTING.load(Ordering::SeqCst));
    }

    /// Structural pins: the flag defaults to unset (normal exits run every
    /// destructor exactly as before), and the guard terminates via
    /// `_exit` (a `std::process::exit` here would run the destructor chain
    /// that aborts — the comment-stripped source must not contain one).
    #[test]
    fn guard_defaults_and_exit_choice_are_pinned() {
        assert!(!RESTARTING.load(Ordering::SeqCst));
        let code: String = include_str!("restart.rs")
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !(t.starts_with("///")
                    || t.starts_with("//!")
                    || t.starts_with("//")
                    || t.starts_with('*'))
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            code.contains("libc::_exit(0)"),
            "the guard must terminate via _exit"
        );
        // The forbidden call, spelled so this assertion's own text does
        // not match the stripped source.
        let forbidden = ["std::process::", "exit("].concat();
        assert!(
            !code.contains(&forbidden),
            "the guard must never call the atexit-running exit"
        );
    }
}
