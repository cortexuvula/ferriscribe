//! Coordinated shutdown on quit (2026-09-23 quit-time SIGABRT fix, phase 2).
//!
//! # What this module does
//!
//! The crash itself is fixed by the unconditional atexit guard
//! (`commands/restart.rs` — every deliberate exit terminates via
//! `libc::_exit`, so the ORT/knf C++ static destructor never aborts).
//! Independently of the crash, a plain quit ALSO (a) orphaned the
//! whisper-server child process — a live PHI-processing process with
//! ports open — and (b) lost the non-blocking log tail. This module adds
//! a best-effort coordinated shutdown on the quit paths that can be
//! intercepted.
//!
//! # The interception mechanism (and why it is menu-shaped)
//!
//! **There is no Tauri event that fires on a macOS Cmd+Q.** AppKit's
//! `-[NSApplication terminate:]` calls `exit()` directly — control never
//! returns to the Rust run loop (the 2026-09-23 crash report's own stack
//! is the proof) — so `RunEvent::Exit`/`ExitRequested` cannot fire there,
//! and Apple documents that quitting does not invoke
//! `windowShouldClose:` (`CloseRequested`) on any window. The ONE
//! reliably interceptable surface is the Quit **menu item**: under
//! Tauri/muda the default Quit item is the predefined `terminate:`
//! selector, so [`build_app_menu`] replaces it with a custom item (same
//! label, same Cmd+Q key equivalent) whose selection reaches
//! [`on_quit_requested`]. The menu is installed for every boot —
//! including the recovery/fatal dialogs (no managed `AppState`), where
//! the handler skips the settle sequence entirely.
//!
//! # The settle sequence
//!
//! 1. `try_state::<AppState>()` — never `state()`: recovery/fatal boots
//!    manage `RecoveryState`/`FatalErrorState` instead. `None` → skip
//!    the whole sequence and exit through the guard (nothing to flush).
//! 2. `mark_quitting()` BEFORE the settle (set in [`on_quit_requested`],
//!    before the task is even spawned) — `start_recording` refuses under
//!    the widened `exit_committed()` gate, closing the check→exit TOCTOU.
//! 3. Refuse-or-cancel, per settled policy:
//!    - **active recording / translation capture → REFUSE** (native
//!      dialog — the webview may be mid-teardown). Both share
//!      `recording_active`; a take is only registered/finalized in the
//!      stop path's task, so quitting over a live take loses it.
//!    - **in-flight file export → REFUSE**: `export_audio` decrypts PHI
//!      audio to a plaintext WAV written incrementally to a user-chosen
//!      path — not atomic and not cancellable mid-write; a quit under it
//!      would leave a truncated PHI file (see `file_export_in_flight`).
//!    - **in-flight pipeline / chat stream → CANCEL** the tokens and let
//!      the timeout bound the wait: cancelled work is marked `Failed` on
//!      next boot by `fail_stuck_processing_sweep` (sweeps.rs), a
//!      background WAV left mid-encryption is finished by
//!      `encryption_pending_sweep`, and a rowless WAV by
//!      `orphaned_wav_sweep`. Accept-and-recover, deliberately.
//! 4. Flush the pending debounced editor edit via
//!    `restart::ensure_safe_to_restart` (on failure the edit is put
//!    back and the quit is REFUSED — never exit with the only copy in a
//!    dying webview).
//! 5. `stop_sharing_inner` — AWAITED, never fire-and-forget (the
//!    window-close handler's spawn-and-forget shape is not acceptable
//!    here): it kills the whisper child via `.kill().await` and can
//!    contend with an in-flight `start_sharing_inner` on the lifecycle
//!    lock — which is exactly why step 7's timeout exists.
//! 6. Emit the final "exiting" log line, THEN flush the non-blocking
//!    log buffer (dropping the `WorkerGuard` from `crate::LOG_GUARD`),
//!    THEN terminate. Inverted order loses the final line — the exact
//!    bug this step exists to fix. Lines logged by the runtime teardown
//!    after the flush reach the console only; the flushed "exiting"
//!    line is the durable end-of-session marker.
//! 7. The whole settle is bounded by [`QUIT_SETTLE_TIMEOUT`]; on expiry
//!    the quit proceeds to exit regardless (logging kinds/counts only).
//!
//! # Documented edge-path gaps (not papered over)
//!
//! **Dock-icon Quit and logout-driven termination send `terminate:`
//! directly and bypass the menu-item handler.** Those paths get the
//! phase-1 crash fix (no SIGABRT) but NOT this coordinated shutdown:
//! an orphaned whisper child is still possible there (as it always was),
//! and the log tail is lost. That is a strict improvement over the
//! pre-fix behavior — today those paths crash AND orphan — but it is not
//! full coordinated shutdown, and no Tauri/tao hook exists to close the
//! gap (tao 0.35.3 implements no `applicationShouldTerminate:` delegate
//! hook; upstream Cmd+Q → `ExitRequested` is a known-open bug).
//!
//! # Privacy
//!
//! The quit path logs booleans, counts, durations, and error KINDS
//! (`AppError::kind_str`) only — never recording/save/transcript
//! content, never a formatted `AppError` display (a DB error string can
//! embed PHI-adjacent text). Pinned structurally by
//! `quit_module_never_logs_content`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tauri::Manager;

use crate::state::AppState;

use super::restart;

/// Hard bound on the coordinated-shutdown settle (steps 3–5). Sharing
/// stop can block on the lifecycle lock against an in-flight start and
/// on reaping the whisper child; both are unbounded without this cap.
/// 8s: generous for a child kill (~2–3s) plus an edit flush, short
/// enough that a wedged quit still looks like a quit.
pub(crate) const QUIT_SETTLE_TIMEOUT: Duration = Duration::from_secs(8);

/// Menu id of the custom Quit item. The default (predefined) Quit item
/// never surfaces an event to Rust — it drives NSApp `terminate:`
/// directly — so `build_app_menu` swaps in a `MenuItem` with this id.
pub(crate) const QUIT_ITEM_ID: &str = "ferriscribe-quit";

/// File exports currently writing to a user-chosen path (RAII-tracked
/// via [`track_file_export`]). `export_audio` decrypts PHI audio to a
/// PLAINTEXT WAV and writes it incrementally — not atomic, not
/// cancellable mid-write — so the quit path refuses while this is >0
/// instead of truncating a PHI file under `_exit`. The redacted
/// support-bundle export rides the same counter (not PHI-bearing, but
/// the same non-atomic shape).
static FILE_EXPORTS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Current in-flight file-export count (kinds/counts are PHI-safe to log).
pub(crate) fn file_export_in_flight() -> usize {
    FILE_EXPORTS_IN_FLIGHT.load(Ordering::SeqCst)
}

/// RAII marker: hold across an export's write window; Drop decrements
/// the counter on every exit path (including panics in the task).
pub(crate) struct FileExportTrack;

impl Drop for FileExportTrack {
    fn drop(&mut self) {
        FILE_EXPORTS_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Mark a file export as in flight. Call immediately before the work;
/// keep the returned guard alive until the write has finished.
pub(crate) fn track_file_export() -> FileExportTrack {
    FILE_EXPORTS_IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    FileExportTrack
}

/// Build the app menu: a mirror of tauri's `Menu::default` with exactly
/// ONE deviation — the predefined Quit item (muda's `terminate:`
/// selector) is replaced by a custom [`MenuItem`] with
/// [`QUIT_ITEM_ID`], so Cmd+Q and the Quit menu click reach
/// [`on_quit_requested`] instead of AppKit directly. Registered at
/// builder time in `lib.rs` (`.menu(...)` + `.on_menu_event(...)`).
///
/// The item lists below intentionally mirror `tauri::menu::Menu::default`
/// (tauri 2.11.5) entry for entry: losing a standard item (Edit's
/// copy/paste, Window menu) would break the webview text editing, so
/// this must not drift from the upstream default apart from the Quit
/// swap.
pub fn build_app_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{AboutMetadata, Menu, PredefinedMenuItem, Submenu};

    let pkg_info = app.package_info();
    let config = app.config();
    let about_metadata = || AboutMetadata {
        name: Some(pkg_info.name.clone()),
        version: Some(pkg_info.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };

    // The one deviation from `Menu::default`: our Quit item instead of
    // `PredefinedMenuItem::quit`. Same Cmd+Q key equivalent the platform
    // expects; macOS label convention is "Quit <AppName>". Scoped use:
    // Linux/BSD builds have no Quit item at all (matching the default
    // menu) and must not warn about the unused import.
    #[cfg(not(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    )))]
    let quit = {
        use tauri::menu::MenuItem;
        MenuItem::with_id(
            app,
            QUIT_ITEM_ID,
            if cfg!(target_os = "macos") {
                format!("Quit {}", pkg_info.name)
            } else {
                "Exit".to_string()
            },
            true,
            Some("CmdOrControl+Q"),
        )?
    };

    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let window_menu = Submenu::with_id_and_items(
        app,
        tauri::menu::WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    let help_menu = Submenu::with_id_and_items(
        app,
        tauri::menu::HELP_SUBMENU_ID,
        "Help",
        true,
        &[
            #[cfg(not(target_os = "macos"))]
            &PredefinedMenuItem::about(app, None, Some(about_metadata()))?,
        ],
    )?;

    Menu::with_items(
        app,
        &[
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                pkg_info.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, Some(about_metadata()))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?,
            #[cfg(not(any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd"
            )))]
            &Submenu::with_items(
                app,
                "File",
                true,
                &[
                    &PredefinedMenuItem::close_window(app, None)?,
                    #[cfg(not(target_os = "macos"))]
                    &quit,
                ],
            )?,
            &edit_menu,
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &window_menu,
            &help_menu,
        ],
    )
}

/// Why a quit was refused (step 3/4 of the settle). The dialog text is
/// static, user-facing, and PHI-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitRefusal {
    /// A recording or translation capture is in flight.
    RecordingActive,
    /// A file export is writing to a user-chosen path.
    ExportInFlight,
    /// The pending editor edit could not be flushed.
    SaveFailed,
}

impl QuitRefusal {
    fn dialog_title(&self) -> &'static str {
        match self {
            QuitRefusal::RecordingActive => "Recording in progress",
            QuitRefusal::ExportInFlight => "Export in progress",
            QuitRefusal::SaveFailed => "Could not save your last edit",
        }
    }

    fn dialog_body(&self) -> &'static str {
        match self {
            QuitRefusal::RecordingActive => {
                "A recording is in progress. Stop it first, then quit FerriScribe."
            }
            QuitRefusal::ExportInFlight => {
                "A file export is still writing. Try quitting again in a moment."
            }
            QuitRefusal::SaveFailed => {
                "Your latest edit could not be saved and is still open in the editor. \
                 Check the editor and try quitting again."
            }
        }
    }
}

/// Entry point from the menu event. Idempotent: a second Cmd+Q during
/// the settle window is ignored (the first request owns the sequence),
/// and a quit request during a committed update relaunch is swallowed.
pub fn on_quit_requested(app: &tauri::AppHandle) {
    if restart::exit_committed() {
        tracing::info!("quit requested while an exit is already committed — ignoring");
        return;
    }
    // Flag BEFORE spawning (step 2): from this instant `start_recording`
    // refuses, so no new take can open inside the settle window.
    restart::mark_quitting();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        quit_sequence(handle).await;
    });
}

/// The full quit sequence (module docs, steps 1–7). Never blocks the
/// main thread: menu events fire on the run loop, so the body is
/// spawned by [`on_quit_requested`] and only fast, send-safe calls
/// happen there.
async fn quit_sequence(app: tauri::AppHandle) {
    // Step 1 — try_state, never state(): recovery/fatal boots manage
    // RecoveryState/FatalErrorState instead and have nothing to settle.
    let Some(state) = app.try_state::<AppState>() else {
        tracing::info!("quit requested with no app state (recovery/fatal boot) — exiting");
        exit_through_guard(&app).await;
        return;
    };

    // Steps 3–5 under the hard bound (step 7). The timeout COVERS the
    // settle, not the exit: on expiry we log and fall through.
    let refusal = match tokio::time::timeout(QUIT_SETTLE_TIMEOUT, settle_for_quit(&state)).await {
        Ok(Ok(())) => None,
        Ok(Err(reason)) => Some(reason),
        Err(_) => {
            // Kinds/counts only (PHI): what was still settling is
            // recoverable by the boot sweeps named in the module docs.
            tracing::warn!(
                timeout_secs = QUIT_SETTLE_TIMEOUT.as_secs(),
                pipelines_cancelled = cancelled_pipeline_count(&state),
                "quit settle timed out — proceeding to exit"
            );
            None
        }
    };

    if let Some(reason) = refusal {
        tracing::info!(
            reason = ?reason,
            "quit refused — app keeps running (flag withdrawn)"
        );
        show_refusal_dialog(&app, reason);
        restart::clear_quitting();
        return;
    }

    exit_through_guard(&app).await;
}

/// Steps 3–5 of the settle: refuse-or-cancel in-flight work, flush the
/// pending edit, stop sharing (awaited). Factored out so the timeout
/// wiring in [`quit_sequence`] is exercised by tests against the same
/// function production runs.
async fn settle_for_quit(state: &AppState) -> Result<(), QuitRefusal> {
    // Step 3a — REFUSE over live capture. Medical recordings and
    // translation captures share `recording_active`; finalization lives
    // in the stop path's task, not under our control.
    if *state.recording_active.lock().await {
        return Err(QuitRefusal::RecordingActive);
    }

    // Step 3a' — REFUSE over an in-flight file export (plaintext PHI
    // WAV mid-write; see `file_export_in_flight`).
    let exports = file_export_in_flight();
    if exports > 0 {
        return Err(QuitRefusal::ExportInFlight);
    }

    // Step 3b — CANCEL pipelines and the chat stream (accept-and-
    // recover: fail_stuck_processing_sweep / encryption_pending_sweep /
    // orphaned_wav_sweep pick the pieces up on next boot).
    let pipelines = cancel_inflight_pipelines(state);
    let chat = cancel_active_chat_stream(state).await;
    if pipelines > 0 || chat {
        tracing::info!(
            pipelines_cancelled = pipelines,
            chat_cancelled = chat,
            "cancelled in-flight work before quit"
        );
    }

    // Step 4 — flush the pending debounced editor edit. On failure the
    // edit was put back (restart.rs contract) and we refuse: never exit
    // with the only copy in a dying webview. Error kind only — the
    // formatted error can embed a DB message.
    if let Err(e) = restart::ensure_safe_to_restart(state).await {
        tracing::warn!(
            error_kind = e.kind_str(),
            "pending edit flush failed — quit refused"
        );
        return Err(QuitRefusal::SaveFailed);
    }

    // Step 5 — stop office sharing, AWAITED (kills the whisper child).
    // A failure here must not block the quit: the alternative (keeping
    // the app alive on a stop failure) orphans the child on the happy
    // path's error twin. Kind only.
    if let Err(e) = super::sharing::stop_sharing_inner(state).await {
        tracing::warn!(
            error_kind = e.kind_str(),
            "sharing stop failed during quit — continuing"
        );
    }

    Ok(())
}

/// Fire every registered pipeline cancel token; returns how many. Uses
/// `cancel_pipeline`'s exact mechanism (token `.cancel()`), so in-flight
/// stages bail at their poll points.
fn cancel_inflight_pipelines(state: &AppState) -> usize {
    match state.pipeline_cancels.lock() {
        Ok(tokens) => {
            let count = tokens.len();
            for token in tokens.values() {
                token.cancel();
            }
            count
        }
        Err(_) => {
            tracing::warn!(poisoned = true, "pipeline cancel registry poisoned at quit");
            0
        }
    }
}

fn cancelled_pipeline_count(state: &AppState) -> usize {
    state
        .pipeline_cancels
        .lock()
        .map(|tokens| tokens.len())
        .unwrap_or(0)
}

/// Cancel the active chat stream, if any (same mechanism as
/// `chat_cancel_stream`). Returns whether one was active.
async fn cancel_active_chat_stream(state: &AppState) -> bool {
    let slot = state.chat_stream_cancel.lock().await;
    match slot.as_ref() {
        Some((_id, token)) => {
            token.cancel();
            true
        }
        None => false,
    }
}

/// Step 6 + termination: final log line, then flush the non-blocking
/// buffer (dropping the worker guard flushes everything buffered before
/// it), then exit through tauri — whose exit chain ends in the library
/// `exit` the atexit guard converts to `_exit(0)`.
async fn exit_through_guard(app: &tauri::AppHandle) {
    // The exit kind on the final line mirrors restart_app's — log
    // forensics for "which deliberate exit fired" (what the 2026-09-23
    // investigation had to reconstruct from a crash report).
    tracing::info!(exit_kind = ?restart::ExitKind::RequestedExit, "FerriScribe exiting");
    flush_log_buffer();
    app.exit(0);
}

/// Flush the non-blocking tracing buffer. Must be the LAST log-related
/// action before termination: the final line is emitted first, then the
/// guard drop flushes it; anything logged after reaches the console
/// layer only. A poisoned slot (only possible if a panic hit while
/// holding it) means no flush — the crash-log tail survives via the
/// console layer.
fn flush_log_buffer() {
    if let Ok(mut slot) = crate::LOG_GUARD.lock() {
        drop(slot.take());
    }
}

/// Surface a refusal natively — the webview is dying, so the dialog
/// plugin (registered for every boot) is the correct surface.
fn show_refusal_dialog(app: &tauri::AppHandle, reason: QuitRefusal) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    app.dialog()
        .message(reason.dialog_body())
        .title(reason.dialog_title())
        .kind(MessageDialogKind::Warning)
        .show(|_| {});
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    /// PHI structural pin (mirror of `guard_defaults_and_exit_choice_are_pinned`):
    /// no tracing macro in this module may reference edit content or
    /// format an error Display into a log line. Comment-stripped source;
    /// the forbidden literals are assembled at runtime so this test's own
    /// text cannot match.
    #[test]
    fn quit_module_never_logs_content() {
        let code: String = include_str!("quit.rs")
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
        let edit_pat = ["edit", ".", "value"].concat();
        assert!(
            !code.contains(&edit_pat),
            "the quit path must never log edit content"
        );
        let value_field_pat = ["val", "ue ="].concat();
        assert!(
            !code.contains(&value_field_pat),
            "the quit path must never log values into tracing fields"
        );
        // The Display-formatting idiom is banned here: an AppError
        // Display can embed PHI-adjacent DB text. kind_str() is the
        // sanctioned spelling.
        let display_fmt = [" = ", "%"].concat();
        assert!(
            !code.contains(&display_fmt),
            "quit logs carry error kinds (kind_str), never formatted Display"
        );
    }

    /// The refusal dialog texts are static and never interpolate state.
    #[test]
    fn refusal_dialogs_are_static() {
        for reason in [
            QuitRefusal::RecordingActive,
            QuitRefusal::ExportInFlight,
            QuitRefusal::SaveFailed,
        ] {
            assert!(!reason.dialog_title().is_empty());
            assert!(!reason.dialog_body().is_empty());
        }
    }

    /// The in-flight export counter round-trips and the RAII guard
    /// decrements on early drop — the quit gate depends on it reaching
    /// zero exactly when the export's write window ends.
    #[test]
    fn file_export_counter_tracks_raii_guard() {
        let before = file_export_in_flight();
        {
            let _t = track_file_export();
            assert_eq!(file_export_in_flight(), before + 1);
            let _t2 = track_file_export();
            assert_eq!(file_export_in_flight(), before + 2);
        }
        assert_eq!(file_export_in_flight(), before);
    }

    // ── settle_for_quit (steps 3–5) against a real AppState ────────────

    async fn quit_test_state() -> AppState {
        // Same construction shape as generation::test_helpers (in-memory
        // DB, no keychain), trimmed to what the settle touches. The
        // provider registry stays empty: the settle path never resolves
        // a provider (sharing stop with an empty slot is a no-op).
        let db = std::sync::Arc::new(medical_db::Database::open_in_memory().expect("db"));
        let tool_registry = medical_agents::tools::ToolRegistry::with_defaults();
        let orchestrator = std::sync::Arc::new(
            medical_agents::orchestrator::AgentOrchestrator::new(tool_registry),
        );
        let tmp = tempfile::tempdir().expect("tempdir");
        let keys = medical_security::key_storage::KeyStorage::open(&tmp.path().join("config"))
            .expect("keys");
        std::mem::forget(tmp);
        AppState {
            db,
            keys: std::sync::Arc::new(keys),
            data_dir: std::path::PathBuf::from("/tmp/quit-test-data"),
            recording_active: std::sync::Arc::new(tokio::sync::Mutex::new(false)),
            pending_edit: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            ai_providers: std::sync::Arc::new(tokio::sync::Mutex::new(
                medical_ai_providers::ProviderRegistry::new(),
            )),
            stt_providers: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            orchestrator,
            chat_doc_index: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            capture_handle: std::sync::Arc::new(std::sync::Mutex::new(
                crate::state::SendCaptureHandle(None, None),
            )),
            current_recording: std::sync::Arc::new(std::sync::Mutex::new(None)),
            translation: std::sync::Arc::new(tokio::sync::Mutex::new(Default::default())),
            tts: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pipeline_cancels: std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
            generation_locks: std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
            sharing: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
            sharing_lifecycle: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            vocab_api: tokio::sync::RwLock::new(None),
            ollama_provider: tokio::sync::RwLock::new(None),
            lmstudio_provider: tokio::sync::RwLock::new(None),
            omlx_provider: tokio::sync::RwLock::new(None),
            remote_stt_provider: tokio::sync::RwLock::new(None),
            local_stt_provider: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
            http_client: std::sync::Arc::new(reqwest::Client::new()),
            content_sync_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            chat_stream_cancel: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            content_sse_cancel: std::sync::Arc::new(std::sync::Mutex::new(None)),
            condition_sse_cancel: std::sync::Arc::new(std::sync::Mutex::new(None)),
            dict_sse_cancel: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    #[tokio::test]
    async fn settle_refuses_over_live_recording() {
        let state = quit_test_state().await;
        *state.recording_active.lock().await = true;
        assert_eq!(
            settle_for_quit(&state).await,
            Err(QuitRefusal::RecordingActive)
        );
    }

    #[tokio::test]
    async fn settle_refuses_over_in_flight_export() {
        let state = quit_test_state().await;
        let _export = track_file_export();
        assert_eq!(
            settle_for_quit(&state).await,
            Err(QuitRefusal::ExportInFlight)
        );
    }

    #[tokio::test]
    async fn settle_cancels_pipelines_and_chat_then_settles() {
        let state = quit_test_state().await;
        let pipeline_token = CancellationToken::new();
        let chat_token = CancellationToken::new();
        state
            .pipeline_cancels
            .lock()
            .unwrap()
            .insert("r1".to_string(), pipeline_token.clone());
        *state.chat_stream_cancel.lock().await = Some(("s1".to_string(), chat_token.clone()));

        // Empty sharing slot + no pending edit: the settle runs to Ok and
        // `stop_sharing_inner` completes (writes to the cfg(test) app-data
        // tempdir, never the developer's real one).
        settle_for_quit(&state)
            .await
            .expect("idle state must settle cleanly");
        assert!(pipeline_token.is_cancelled());
        assert!(chat_token.is_cancelled());
    }

    /// The hard bound (step 7): `stop_sharing_inner` waits out the
    /// sharing lifecycle lock against an in-flight start, which can be
    /// unbounded — the production timeout in `quit_sequence` exists for
    /// exactly this shape. Drives the SAME settle function under the
    /// SAME `tokio::time::timeout` wrapper, with the lock wedged.
    #[tokio::test]
    async fn settle_is_bounded_when_sharing_stop_blocks() {
        let state = quit_test_state().await;
        // Wedge the lifecycle lock the way a slow `start_sharing_inner`
        // would: a task holds it (almost) forever.
        let lock = std::sync::Arc::clone(&state.sharing_lifecycle);
        let wedger = tokio::spawn(async move {
            let _guard = lock.lock().await;
            tokio::time::sleep(Duration::from_secs(3600)).await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await; // let it acquire

        let result =
            tokio::time::timeout(Duration::from_millis(400), settle_for_quit(&state)).await;
        assert!(
            result.is_err(),
            "settle must be time-bounded when the sharing stop blocks"
        );
        wedger.abort();
    }
}
