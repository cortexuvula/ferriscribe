//! Crash-safe process exit for the update flow AND normal quits, with
//! coordinated shutdown.
//!
//! # Why this exists (2026-09-08 + 2026-09-23 crash investigations)
//!
//! The statically linked ONNX Runtime / kaldi-native-fbank block (the
//! diarization pipeline's `ort` download-binaries + `knf-rs`) registers a
//! C++ static destructor that **aborts** when the process exits with the
//! app's worker threads still live. The destructor chain runs on every
//! `exit`-shaped teardown. Two field reports pinned it:
//!
//! - 2026-09-07 / 2026-09-08 (update relaunch): tauri's `process::restart`
//!   ends in a library `exit`, stack `exit → __cxa_finalize_ranges →
//!   abort`, frames inside the ORT/knf C++ island.
//! - 2026-09-23 (NORMAL quit, v0.77.11): the same SIGABRT on a plain
//!   Cmd+Q — stack `-[NSApplication terminate:] → exit →
//!   __cxa_finalize_ranges → abort`. AppKit's `terminate:` calls
//!   `exit()` directly; control never returns to the Rust run loop, so
//!   no Tauri event can intercept it. All four unnamed binary frames
//!   were symbolicated against the exact shipped binary (bracketed by
//!   the only defined C++ symbols — `knf::OnlineGenericBaseFeature…` and
//!   `onnx::propagateShapeAndTypeFromFirstInput`, with the abort site
//!   passing ORT-style source-line immediates) — they are the ORT/knf
//!   island, not Rust code.
//!
//! The earlier belief that only the update relaunch aborted was wrong;
//! every deliberate exit runs the same destructor.
//!
//! # The fix (Phase 1 — every deliberate exit bypasses the destructors)
//!
//! [`install_exit_guard`] registers an `atexit` handler from `run()`,
//! i.e. AFTER every pre-main C++ static registration — and atexit runs
//! LIFO, so the guard executes FIRST at process exit. It terminates via
//! `libc::_exit(0)` unconditionally: every exit that reaches atexit in
//! this process is a deliberate app exit (normal quit, recovery-boot
//! quit, last-window close, `AppHandle::exit`, update relaunch), and
//! NONE of them may run the ORT/knf destructor that aborts. The decision
//! matrix is [`ExitKind`] / [`bypasses_destructor_chain`], pinned by
//! test so no future edit narrows a path back to the destructor chain.
//!
//! Skipping static destructors is safe for every one of those exits: on
//! macOS a normal quit is `terminate: → exit()`, which never returns
//! through `run()`'s epilogue — Rust locals (including the
//! `tracing_appender` `WorkerGuard`) never `Drop` today either, and the
//! run-loop epilogue on other platforms ends in a library `exit` before
//! main returns. The only teardown an unconditional `_exit` skips is the
//! remaining atexit chain (whose only handler of consequence is this
//! guard) and the C++ static destructors, which hold no user state —
//! the one C++ destructor that does something is ORT/knf, the thing that
//! aborts. Durable-write safety was verified, not assumed:
//!
//! - **DB**: SQLite/SQLCipher commits are synchronous (WAL + fsync).
//! - **Audio at rest**: `file_crypto::encrypt_file_in_place` is atomic
//!   (temp + fsync + rename); anything it left half-done is finished by
//!   the boot sweeps (`encryption_pending_sweep`, `orphaned_wav_sweep`).
//! - **Backups** (`backup_run_now` and the scheduled sidecar — a separate
//!   process an app exit cannot touch): snapshots build into a `.tmp-<id>`
//!   sibling and rename into place; every payload blob (agent PUT, folder
//!   push, drill re-pull) lands via `.tmp-` + hash-verify + rename, and
//!   all payload bytes are encrypted (FE1 recordings, re-encrypted DB
//!   copy, `manifest.json.enc`). Stale `.tmp-*` leftovers are swept.
//! - **PDF/DOCX/FHIR exports**: the Rust side only returns bytes over
//!   IPC; the file write happens in the frontend save dialog, whose
//!   behavior under quit is unchanged from before this fix.
//! - **`export_audio` / `export_support_bundle`**: NOT atomic —
//!   `export_audio` decrypts PHI audio to a plaintext WAV and writes it
//!   incrementally to a user-chosen path. These are tracked by an
//!   in-flight counter ([`crate::commands::quit::file_export_in_flight`])
//!   and the coordinated-quit path REFUSES to exit while one is running
//!   (see `commands/quit.rs`); they are the documented exceptions, not
//!   silent ones.
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

/// Set when the process is preparing to exit ONLY to relaunch a fresh copy
/// (the update flow). Read by [`exit_committed`]. Never cleared: it is
/// only set after all refusal checks pass, and the process exits via
/// `_exit` immediately after, so no reset path is needed.
static RESTARTING: AtomicBool = AtomicBool::new(false);

/// Set while a coordinated QUIT (`commands/quit.rs`) is settling work,
/// CLEARED again if that quit is refused (active recording / in-flight
/// file export / failed edit flush) so the app keeps working. Read by
/// [`exit_committed`] — the same TOCTOU gate as restarts.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// True when a deliberate exit (restart OR coordinated quit) has been
/// committed and new destructive work must not start. `start_recording`
/// consults this to refuse new takes during the window between the
/// coordinated-shutdown checks and process exit.
pub fn exit_committed() -> bool {
    RESTARTING.load(Ordering::SeqCst) || QUITTING.load(Ordering::SeqCst)
}

/// Mark a coordinated quit as in progress (closes the start-new-work
/// TOCTOU window while the settle sequence runs).
pub(crate) fn mark_quitting() {
    QUITTING.store(true, Ordering::SeqCst);
}

/// Withdraw a coordinated quit that was refused — the app keeps running
/// and must accept new work again. Only `commands/quit.rs` calls this,
/// on its refusal paths; once exit is truly committed the process never
/// returns here.
pub(crate) fn clear_quitting() {
    QUITTING.store(false, Ordering::SeqCst);
}

/// Every deliberate exit this process can reach through atexit. The
/// 2026-09-23 normal-quit SIGABRT proved this set cannot be narrowed to
/// "just restarts": AppKit's Cmd+Q → `terminate:` → `exit()` runs the
/// same aborting ORT/knf destructor, from a normal boot AND from a
/// recovery/fatal boot alike (no managed `AppState` — the destructor
/// doesn't care).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitKind {
    /// Normal quit: Cmd+Q, the Quit menu item, last-window close, or a
    /// quit from the recovery/fatal-error boot (which manages no
    /// AppState but links the same ORT/knf island).
    NormalQuit,
    /// An exit requested programmatically: `AppHandle::exit` or tao's
    /// run-loop epilogue after a `RequestExit` (this is what the
    /// intercepted Quit menu item terminates through, so coordinated
    /// work is settled BEFORE the request is made).
    RequestedExit,
    /// The update relaunch (`restart_app` → tauri's restart).
    Restart,
}

/// The atexit guard's bypass predicate: TRUE for every [`ExitKind`]. A
/// function (not a bare unconditional in the shim) so the decision
/// matrix is a pinned, unit-tested contract — if a future edit narrows
/// any path back to the destructor chain, the test fails and points
/// here.
pub(crate) fn bypasses_destructor_chain(kind: ExitKind) -> bool {
    matches!(
        kind,
        ExitKind::NormalQuit | ExitKind::RequestedExit | ExitKind::Restart
    )
}

/// Terminate immediately, skipping the remaining atexit chain. Split
/// from the `extern "C"` shim so the no-op path is testable (the `_exit`
/// path terminates the process by construction).
fn exit_guard_if(bypass: bool) {
    if bypass {
        // `_exit` (never the atexit-running library exit): the remaining
        // atexit chain — including the ORT/knf C++ static destructor
        // that aborts — must not run on ANY deliberate exit.
        unsafe { libc::_exit(0) }
    }
}

/// atexit shim. Registered from `run()` (after all pre-main static
/// registrations) so LIFO ordering makes it the FIRST handler to run at
/// process exit. Unconditional by design: atexit cannot tell us WHICH
/// deliberate exit fired, and the 2026-09-23 report proved that treating
/// the least-flagged path (normal quit) as safe is exactly the mistake
/// that crashed. `NormalQuit` is that least-specific kind — if the
/// predicate holds for it, the pinned matrix guarantees it holds for
/// every kind.
extern "C" fn exit_guard() {
    exit_guard_if(bypasses_destructor_chain(ExitKind::NormalQuit));
}

/// Register the exit guard. Call exactly once, early in `run()`.
pub fn install_exit_guard() {
    // Best-effort: atexit failing (ENOMEM under fd/thread pressure) means
    // deliberate exits run the full destructor chain — the pre-fix
    // behavior (SIGABRT crash report on every quit) — never a hard
    // error worth failing boot over.
    let rc = unsafe { libc::atexit(exit_guard) };
    if rc != 0 {
        tracing::warn!(
            rc,
            "exit guard registration failed — quitting may show a crash report"
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

/// Coordinated-shutdown checks shared by [`restart_app`] and the quit
/// path (`commands/quit.rs` — flushes the same pending edit through the
/// same code). Returns `Ok(())` when it is safe to exit, or the refusal
/// error. Pure logic over the AppState — no exit, no flag mutation — so
/// it is unit-testable without terminating the test process.
pub(crate) async fn ensure_safe_to_restart(state: &AppState) -> Result<(), AppError> {
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
    // All work settled — commit the restart. Once RESTARTING is set,
    // `start_recording` refuses new takes, closing the check→exit TOCTOU
    // window (Codie review, 2026-09-09). Ordering vs. the guard is
    // unchanged: work settles first, always. `app.restart()` never
    // returns (its signature is `!`): it spawns the replacement and exits
    // the process via the guard, so there is no failure path that leaves
    // the process alive with the flag set. The logged exit kind makes
    // the final log line state which deliberate exit fired (the same
    // forensics the 2026-09-23 crash investigation needed).
    tracing::info!(exit_kind = ?ExitKind::Restart, "update relaunch committed");
    RESTARTING.store(true, Ordering::SeqCst);
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bypass predicate must hold for EVERY deliberate exit kind —
    /// the 2026-09-23 normal-quit SIGABRT is what happens when any one
    /// of them is narrowed back to the destructor chain.
    #[test]
    fn every_deliberate_exit_bypasses_the_destructor_chain() {
        for kind in [
            ExitKind::NormalQuit,
            ExitKind::RequestedExit,
            ExitKind::Restart,
        ] {
            assert!(
                bypasses_destructor_chain(kind),
                "exit kind {kind:?} must bypass the ORT/knf destructor chain"
            );
        }
    }

    /// The no-op branch of the split guard helper returns — the test
    /// process survives. (The bypass branch terminates by construction;
    /// its behavior is pinned structurally below and by the manual
    /// quit-repro gate.)
    #[test]
    fn exit_guard_helper_returns_when_not_bypassing() {
        exit_guard_if(false);
    }

    /// Structural pins: the guard terminates via `_exit` (the atexit-
    /// running library exit would run the destructor chain that aborts —
    /// the comment-stripped source must not contain one), the shim's
    /// body consults NO flag (an unconditional guard is the entire
    /// normal-quit fix — a flag read would reintroduce the missed-path
    /// bug), and it touches no state (a recovery-boot quit with no
    /// managed AppState takes the identical, panic-free path).
    #[test]
    fn guard_defaults_and_exit_choice_are_pinned() {
        // Only RESTARTING's default is pinned here: the widened-gate test
        // below exercises QUITTING and tests run concurrently in one
        // process, so no other test may read QUITTING.
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
        // Pin the shim to the unconditional shape: extract its body and
        // assert it reads no exit flag and no app state.
        let shim = code
            .split("extern \"C\" fn exit_guard()")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("exit_guard shim must exist");
        assert!(
            !shim.contains("RESTARTING") && !shim.contains("QUITTING"),
            "the guard must be unconditional — a flag read reintroduces the 2026-09-23 crash"
        );
        assert!(
            !shim.contains("state"),
            "the guard must not touch app state (recovery-boot quits ride it too)"
        );
    }

    /// The widened exit gate: `exit_committed` must cover the
    /// coordinated-quit commit (and the formula ORs in the restart flag,
    /// pinned structurally — this test must not touch RESTARTING because
    /// the defaults test above reads it concurrently). `start_recording`
    /// refuses under the combined gate (audio.rs).
    #[test]
    fn exit_committed_covers_restart_and_quit_flags() {
        assert!(!exit_committed());
        mark_quitting();
        assert!(exit_committed(), "quit commit must refuse new work");
        clear_quitting();
        assert!(!exit_committed(), "a refused quit must re-open the gate");
        // The restart half of the OR, pinned structurally so this test
        // never mutates the RESTARTING static.
        let code: String = include_str!("restart.rs")
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !(t.starts_with("///") || t.starts_with("//!") || t.starts_with("//"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            code.contains("RESTARTING.load(Ordering::SeqCst) || QUITTING.load(Ordering::SeqCst)"),
            "exit_committed must OR the restart and quit commits"
        );
    }
}
