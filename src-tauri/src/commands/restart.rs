//! Crash-safe app restart for the update flow.
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

use std::sync::atomic::{AtomicBool, Ordering};

use medical_core::error::AppResult;

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

/// Relaunch the app after an update install, crash-safely. Never returns
/// on success: tauri's `restart()` spawns the replacement and exits the
/// current process (both its exit paths — the direct main-thread one and
/// the run-loop one — run atexit, where the guard converts the exit).
#[tauri::command]
pub fn restart_app(app: tauri::AppHandle) -> AppResult<()> {
    RESTARTING.store(true, Ordering::SeqCst);
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard must be inert on a normal exit — every destructor runs
    /// exactly as it did before this module existed.
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
