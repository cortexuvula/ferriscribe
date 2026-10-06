//! Crash-safe process exit for the update flow AND normal quits, with
//! coordinated shutdown.
//!
//! # Why this exists (2026-09-08 + 2026-09-23 + 2026-10-06 crash
//! investigations)
//!
//! The statically linked ONNX Runtime / kaldi-native-fbank block (the
//! diarization pipeline's `ort` download-binaries + `knf-rs`) registers a
//! C++ static destructor that **aborts** when the process exits with the
//! app's worker threads still live. The destructor chain runs on every
//! `exit`-shaped teardown. Three field reports pinned it:
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
//! - 2026-10-06 (update relaunch, **v0.77.12 — WITH the guard**): the
//!   same SIGABRT 130 ms after `update relaunch committed`, in a session
//!   that had run heavy diarization (19k diarization log lines). The
//!   crashed binary's UUID matched the v0.77.12 release artifact exactly,
//!   and the boot log carried no `exit guard registration failed` warn —
//!   the guard WAS registered and still lost. Root cause: atexit runs
//!   LIFO over one shared table, and the ORT/knf island registers its
//!   destructors **lazily at first inference** — i.e. AFTER the guard's
//!   boot-time registration — so they run FIRST at exit and the aborting
//!   one fires before the guard's `_exit(0)`. (Corroborating asymmetry:
//!   the replacement v0.78.0 process quit cleanly 73 s after boot — a
//!   session with no diarization has no post-guard registrations, so the
//!   boot-time guard was still the newest entry and won.)
//!
//! The earlier belief that only the update relaunch aborted was wrong;
//! every deliberate exit runs the same destructor. The 2026-10-06 report
//! then proved that an atexit guard registered at boot CANNOT reliably
//! win the LIFO race against runtime-registered destructors at all.
//!
//! # The fix (Phase 1 — every deliberate exit bypasses the destructors;
//! Phase 3 — always own the newest atexit entry)
//!
//! Layer 1 (2026-09-23, kept): [`install_exit_guard`] registers an
//! `atexit` handler from `run()`, terminating via `libc::_exit(0)`
//! unconditionally for every [`ExitKind`]. Covers every exit in a
//! session that has not run diarization — there, the boot-time guard is
//! still the newest table entry and wins the LIFO walk.
//!
//! Layer 2 (2026-10-06): after EVERY diarization use, the guard is
//! RE-registered ([`rearm_exit_guard`], wired through
//! `medical_stt_providers::diarization::set_exit_rearm_hook` and fired
//! by a Drop guard on every `diarize` exit path). The ORT/knf island —
//! the ONLY aborting destructor in the process (12 days of v0.77.12
//! field history quit WebKit/CoreAudio/GCD-thread-carrying sessions
//! without a single SIGABRT; only diarization sessions crashed) —
//! registers its destructors lazily at first inference; re-arming after
//! every use puts our entry strictly AFTER theirs again.
//!
//! Layer 3 (2026-10-06): the exits this app initiates re-arm one final
//! time immediately before riding the library `exit` — [`restart_app`]
//! flushes the log and re-arms right before `app.restart()` (tauri's
//! restart: spawn the replacement, then `exit(0)`), and the coordinated
//! quit's terminal step does the same before `app.exit(0)`. At the
//! moment `exit()` walks the table, our entry is the newest BY
//! CONSTRUCTION: the only code that runs between the re-arm and the
//! walk is the single-threaded spawn/exit sequence itself, and the only
//! aborting registrant (ORT/knf) registered strictly before our
//! post-diarize re-arms. Pinned across crates: diarization's test
//! module pins that `diarize` holds the re-arm Drop guard from its
//! first statement.
//!
//! Residual window (accepted, documented): the UN-interceptable exits
//! (dock-icon Quit / logout → AppKit `terminate:` straight to `exit()`)
//! during the FIRST `diarize` call of a session, after the island's lazy
//! registration but before the call's trailing re-arm, would still walk
//! the table with ORT/knf ahead of the boot guard. Unavoidable by
//! registration order (nothing can be registered ahead of an entry that
//! has not happened yet), and narrowed by this fix from "any quit after
//! any diarization, forever" to "quit within one diarize call, once per
//! session". The exits the app initiates are not meaningfully exposed:
//! they re-arm after the settle sequence (which cancels in-flight
//! pipelines under an 8 s bound), and a diarize still running at exit
//! time contributes its own trailing re-arm on completion.
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

/// Re-register the exit guard so it is again the NEWEST atexit entry
/// (2026-10-06 fix, layer 3). atexit is one shared LIFO table: the ORT/knf
/// island registers its destructors lazily at first inference, which
/// pushes them AHEAD of the boot-time guard — so after every diarization
/// use the guard must be re-registered to reclaim the first slot. Wired
/// via `medical_stt_providers::diarization::set_exit_rearm_hook` in
/// `run()`; fires after every `diarize` call on every exit path.
///
/// Each call appends one table entry that will never individually run
/// (the newest entry `_exit`s first); the per-recording cost is a few
/// dozen bytes and one registration — negligible next to the inference
/// that just ran. Failing (ENOMEM) is warned, never fatal: the previous
/// entries remain, degrading to the boot-time guard's coverage.
pub fn rearm_exit_guard() {
    let rc = unsafe { libc::atexit(exit_guard) };
    if rc != 0 {
        tracing::warn!(
            rc,
            "exit guard re-arm failed — quitting after diarization may show a crash report"
        );
    }
}

/// Flush the non-blocking tracing buffer. Must be the LAST log-related
/// action before ANY direct termination (`_exit` skips the worker-guard
/// drop): the final line is emitted first, then this flush makes it
/// durable; anything logged after reaches the console layer only. A
/// poisoned slot (only possible if a panic hit while holding it) means
/// no flush — the tail survives via the console layer. Shared by the
/// quit path (`commands/quit.rs`) and [`restart_app`].
pub(crate) fn flush_log_buffer() {
    if let Ok(mut slot) = crate::LOG_GUARD.lock() {
        drop(slot.take());
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
/// current process through the library `exit`, whose atexit walk the
/// re-armed guard converts to `_exit(0)` before any ORT/knf destructor
/// (layer 3 — the boot-time guard ALONE lost exactly that walk in the
/// 2026-10-06 v0.77.12 field crash). On refusal, returns an error and
/// the app keeps running with the user's work intact.
#[tauri::command]
pub async fn restart_app(app: tauri::AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    ensure_safe_to_restart(&state).await?;
    // All work settled — commit the restart. Once RESTARTING is set,
    // `start_recording` refuses new takes, closing the check→exit TOCTOU
    // window (Codie review, 2026-09-09). Work settles first, always. The
    // logged exit kind states which deliberate exit fired (the forensics
    // the 2026-09-23 and 2026-10-06 investigations needed); the flush
    // makes it durable BEFORE the process can die inside tauri's exit —
    // `_exit` (which the guard converts that exit into) skips the
    // tracing worker's drop.
    tracing::info!(exit_kind = ?ExitKind::Restart, "update relaunch committed");
    RESTARTING.store(true, Ordering::SeqCst);
    flush_log_buffer();
    // Last-line re-arm (layer 3): our atexit entry must be the NEWEST by
    // construction when tauri's restart calls exit(0) microseconds later.
    // `app.restart()` never returns (its signature is `!`), so there is
    // no failure path that leaves the process alive past this point.
    rearm_exit_guard();
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

    /// 2026-10-06 fix pins: a dedicated re-arm entry point must exist and
    /// register the SAME shim as the boot-time install (a differently
    /// behaving entry would break the newest-entry invariant), and
    /// `restart_app` must flush → re-arm → `app.restart()` in that order
    /// — the trailing library `exit` inside tauri's restart is where the
    /// boot-time-only guard lost the LIFO walk (v0.77.12 field crash).
    #[test]
    fn rearm_and_restart_exit_ordering_are_pinned() {
        let code: String = include_str!("restart.rs")
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !(t.starts_with("///") || t.starts_with("//!") || t.starts_with("//"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            code.contains("pub fn rearm_exit_guard()"),
            "the re-arm entry point must exist"
        );
        assert!(
            code.matches("libc::atexit(exit_guard)").count() >= 2,
            "install AND re-arm must register the same shim"
        );
        let body = code
            .split("pub async fn restart_app(")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("restart_app must exist");
        let flush = body
            .find("flush_log_buffer();")
            .expect("restart_app must flush the log before the process can die");
        let rearm = body
            .find("rearm_exit_guard();")
            .expect("restart_app must re-arm the guard before tauri's exit");
        let restart = body
            .find("app.restart()")
            .expect("restart_app must terminate through tauri's restart");
        assert!(
            flush < rearm && rearm < restart,
            "restart_app must flush → re-arm → restart, in that order"
        );
    }

    /// Cross-crate contract (2026-10-06 fix, layer 2): `run()` must wire
    /// the diarization re-arm hook to the guard. A missing registration —
    /// or a drift to a differently-named function — silently drops the
    /// post-diarization coverage; the diarization crate pins the other
    /// half (its `diarize` holds the Drop guard from its first
    /// statement).
    #[test]
    fn run_registers_the_diarization_rearm_hook() {
        let lib: String = include_str!("../lib.rs")
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !(t.starts_with("///") || t.starts_with("//!") || t.starts_with("//"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            lib.contains("set_exit_rearm_hook(commands::restart::rearm_exit_guard)"),
            "run() must wire the diarization re-arm hook to the exit guard"
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
