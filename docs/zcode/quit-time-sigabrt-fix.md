# zcode prompt — Fix quit-time SIGABRT (`exit → __cxa_finalize_ranges → abort`)

You are fixing a **crash-on-quit** in FerriScribe (Tauri 2 + Svelte 5 + Rust workspace, local-first AI medical scribe). The field report: closing the app normally (Cmd+Q / Quit menu) produces a macOS crash dialog. This is a Rust/C++ process-teardown bug in the desktop shell, **not** an OCR/STT/AI logic bug. Do not touch the privacy model, the AI providers, or the diarization quality code.

## The crash signature (verified field report)

```
Exception Type:    EXC_CRASH (SIGABRT)
Termination Reason: Namespace SIGNAL, Code 6, Abort trap: 6
Application Specific Information: abort() called
Triggered by Thread: 0 (main, com.apple.main-thread)

Thread 0 Crashed::
0  libsystem_kernel   __pthread_kill
1  libsystem_pthread  pthread_kill
2  libsystem_c        abort
3  rust-medical-assistant  (image offset 30737668)      ← unnamed frame #1
4  rust-medical-assistant  (image offset 12145460)      ← unnamed frame #2
5  rust-medical-assistant  (image offset 12147776)      ← unnamed frame #3
6  rust-medical-assistant  (image offset 12152792)      ← unnamed frame #4
7  libsystem_c        __cxa_finalize_ranges
8  libsystem_c        exit
9  AppKit             -[NSApplication terminate:]
```

App version **0.77.11**, launched ~5 days before the crash, with ~30 `tokio-rt-worker` threads, an `r2d2` pool, `tracing-appender`, and an `mDNS_daemon` thread all still alive at exit. The four unnamed frames sit *inside* the `rust-medical-assistant` binary.

## Root cause (already diagnosed in-repo — read it)

`src-tauri/src/commands/restart.rs` module doc, lines 1–35, already names this crash: the statically-linked **ONNX Runtime + kaldi-native-fbank** C++ island (diarization pipeline's `ort` `download-binaries` + `knf-rs`) registers a static destructor that **aborts when the process exits with its worker threads still alive**. The `exit → __cxa_finalize_ranges → abort` stack shape matches the report. The existing fix — an `atexit` **exit guard** — only suppresses it on the **restart** path:

- `src-tauri/src/lib.rs:88` — `commands::restart::install_exit_guard();` (first statement of `run()`, so atexit LIFO runs it first)
- `src-tauri/src/commands/restart.rs:79` — `static RESTARTING: AtomicBool`
- `restart.rs:91-104` — `exit_guard_if` / `extern "C" fn exit_guard()` call `libc::_exit(0)` **only when `RESTARTING` is set**
- `restart.rs:222-234` — `restart_app` sets `RESTARTING.store(true)` then `app.restart()`

The claim at `restart.rs:28-29` ("no abort was ever observed on a normal quit") is **disproven by this report** — a normal quit runs the same aborting destructor, and the guard no-ops. Correct that comment.

## Critical constraint you must design around (verified against tao/muda source and Apple docs — do not re-derive)

**There is no Tauri event that fires on a macOS Cmd+Q / Dock-Quit / Apple-menu-Quit.** Three independent facts:

1. **AppKit `-[NSApplication terminate:]` calls `libsystem_c exit()` directly; control never returns to the Rust run loop.** The crash report's own stack is the proof (frames 8–9). So `RunEvent::Exit` cannot fire on Cmd+Q.
2. **tao 0.35.3 (pinned in Cargo.lock) implements no `applicationShouldTerminate:` hook** — there is no way to observe or veto `terminate:` from the run loop. `RunEvent::ExitRequested` is emitted only from a last-window `Destroyed` and from a `RequestExit` message; its `prevent_exit` contract is synchronous and cannot await async shutdown. Upstream, Cmd+Q → `ExitRequested` is a known-broken open bug (tauri#9198, duplicates #12978/#13778).
3. **Apple documentation: quitting does *not* invoke `windowShouldClose:`** (`WindowEvent::CloseRequested`) on any window — that fires only from the close button / Close command. So extending the existing `CloseRequested` handler (`lib.rs:291-306`) will **not** catch Cmd+Q either.

**Consequence:** the crash fix must go through the one thing that provably fires on every quit path — the existing `atexit` exit guard — and the coordinated-shutdown layer must be scoped honestly (best-effort, via menu/quit-item interception, with the remaining edge paths explicitly acknowledged).

## Resolved design decision

### Phase 1 — the crash fix (mandatory, no hook dependency): make the exit guard cover every deliberate exit

The `exit_guard_if` predicate is the only reliable interception point. Extend it so the ORT/knf destructor is skipped on **all** deliberate exits, not just restart: replace the flag-dependent skip with an **unconditional `libc::_exit(0)`** in the atexit guard (or, if you prefer a flag, a `QUITTING` flag that is set on every exit-committing path — but you MUST not leave a quit path where the flag is unset and the aborting destructor still runs).

Why this is safe (restart.rs:31-35 already argues it, and it holds for normal quit too): on macOS, a normal quit is `terminate: → exit()`, which never returns through main's epilogue — so Rust essentials (`run()`'s locals, including the `tracing_appender` `WorkerGuard` `_guard` at `lib.rs:115`) never `Drop` *today* either. The only teardown an unconditional `_exit` skips is the remaining atexit chain (whose only handler of consequence is our own guard) and the C++ static destructors (which hold **no user state** — every durable write is committed synchronously before any exit-committing operation; SQLite WAL commits and atomic fsync+rename writes are synchronous). The *one* C++ destructor that does something — ORT/knf — is the thing that aborts, so skipping it is the entire point.

**Scope the "every durable write is committed synchronously" claim, do not repeat it unverified.** Verify the backup/export/support-bundle commands (`backup_run_now`, `export_pdf/docx`, `export_support_bundle`) actually write atomically and do not leave PHI-bearing temp files that `_exit` would truncate. Where an in-flight backup/export writes a temp file you cannot make atomic, name it explicitly and add it to the shutdown-refusal/timeout set below — do not leave a PHI-bearing temp file silently mid-write under `_exit`.

### Phase 2 — coordinated shutdown on quit (best-effort, must not regress the crash fix)

The ORT destructor abort is the crash. Independent of it, Cmd+Q today already (a) orphans the whisper-server child process (a live PHI-processing process with ports open) and (b) loses the non-blocking log tail. Fix these where a hook genuinely exists, and accept the edge-path gaps below:

- **Intercept the Quit menu item and its Cmd+Q key-equivalent** to run coordinated shutdown before exit. Under Tauri/muda, the default Quit item is the predefined `terminate:` selector; replace it with a custom handler that runs the shutdown sequence, then terminates through the `_exit` guard.
- **The shutdown sequence, in order, with a hard bound:**
  1. **`try_state::<AppState>()`** — never `state::<AppState>()` (AppState is only managed on successful init, `lib.rs:187-193`; a recovery/fatal boot manages `RecoveryState`/`FatalErrorState` instead). In `None`, skip the whole sequence and exit through the guard (nothing to flush exists in that boot).
  2. **Guard against new work:** set the quit/exit flag (the same one `exit_guard` reads) *before* running shutdown, and make `start_recording` refuse under it via the existing `restart_committed()` gate (`restart.rs:84`, checked at `audio.rs:96`) — widen that gate to "exit committed", not just "restart committed". This closes the check→exit TOCTOU for the quit path.
  3. **Refuse (or cancel) in-flight work, per a settled policy:** active recording/translation capture → **refuse** (surface a native dialog — `tauri-plugin-dialog` is registered at `lib.rs:357`, and `app.dialog().message(...)` is the correct surface since the webview is dying) — do not `_exit` over a live take. In-flight **pipeline/chat**: **cancel** the tokens and await briefly rather than refusing — `AppState.pipeline_cancels` is a `HashMap<String, CancellationToken>` at `state.rs:351` populated by `process_recording` (`commands/pipeline.rs:74-89`) and fired by `cancel_pipeline` (`pipeline.rs:276`) — cancelled work is then marked `Failed` on next boot by `fail_stuck_processing_sweep` (`sweeps.rs:20`); a background WAV left mid-encryption is recovered by `encryption_pending_sweep` (`sweeps.rs:38`) and a rowless WAV by `orphaned_wav_sweep` (`sweeps.rs:151`). Name these recovery paths in the code comment so the accept-and-recover decision is explicit, not accidental.
  4. **Flush pending editor save** — reuse the pure `ensure_safe_to_restart(&AppState)` (`restart.rs:153-181`; note it takes `&AppState`, not `State`, and is callable from any hook). On save failure, put the edit back (the existing contract) — never exit with the only copy in a dying webview.
  5. **Stop office-server sharing and AWAIT it** — `stop_sharing_inner(&AppState)` is `pub` (`commands/sharing/lifecycle.rs:223`), re-exported at `sharing/mod.rs:24`. **Await it to completion; never fire-and-forget** (the existing `CloseRequested` handler `spawn`s and forgets — that template is not acceptable here). It takes `sharing_lifecycle` (`lifecycle.rs:228`) which can be held by an in-flight `start_sharing_inner` (`lifecycle.rs:50`), and it kills the whisper child via `.kill().await` — both can block. That is why step 7's timeout is mandatory.
  6. **Drain the non-blocking log buffer** before the guard runs: `_guard` (`lib.rs:115`) must be reachable (move it into a `static OnceLock<WorkerGuard>` or into `AppState`) so the shutdown path can flush it. Emit the final "exiting" log line *before* flushing, and make the flush the last action before termination — if the ordering is inverted the final line is lost, which is the exact bug this step exists to fix.
  7. **Hard timeout on the whole settle** (sharing lock contention + child reap are unbounded otherwise): cap the shutdown at a named constant (5–10 s), then proceed to exit regardless, logging **kinds/counts/lengths only** (never content).

- **Edge-path gaps you MUST document, not paper over:** Dock-icon Quit and Apple-menu-Quit send `terminate:` directly in a way that bypasses a menu-item handler; those paths get the Phase-1 crash fix (no SIGABRT) but may not get the Phase-2 coordinated shutdown. That is a strict improvement over today (today they both crash AND orphan) but is not full coordinated shutdown on those paths — say so in the module doc rather than implying they are covered.

## Hard facts (reuse; do not reinvent or guess)

- **Binary name** is `rust-medical-assistant` (`src-tauri/Cargo.toml` `[package] name`, line 2), not "ferriscribe".
- **`panic = "abort"`** in the release profile: workspace `Cargo.toml:45`. This is *not* the abort in question (the SIGABRT is the C++ static destructor, not a Rust panic) — but it is *why* a hypothetical panic in a quit hook also aborts. Do not casually change this line.
- **`ort`** dependency: `crates/stt-providers/Cargo.toml:23` — `ort = { version = "=2.0.0-rc.13", features = [..., "download-binaries", ...] }`; **`knf-rs = "0.3.2"`** at line 25. Both statically link the aborting C++ island; the destructor is registered at image load (pre-main) regardless of whether diarization ever ran. **Do not** reinterpret this fix as "remove download-binaries / knf" — that is out of scope, and its narrowing ("skip only if ORT was loaded") is *not implementable* (static registration is pre-main).
- **ORT sessions** are created per-`diarize()` call (`crates/stt-providers/src/diarization.rs:278` and `:463`). The abort depends on live worker threads at dtor time, not on live session handles — dropping a session is insufficient; the guard must bypass the static-destructor chain.
- **Guard to extend:** `src-tauri/src/commands/restart.rs` (read the whole module first) and its registration at `src-tauri/src/lib.rs:88`.
- **Keep `.run(app_context())` at `lib.rs:517`** — do NOT switch to `build(context)` + run-loop event handling. That form compiles but `RunEvent::ExitRequested`/`Exit` provably cannot fire on Cmd+Q (see the Critical constraint), so it would silently no-op on the exact quit that crashes. There is no event-loop hook to reach; drop that idea entirely.

## Privacy constraints (absolute — load-bearing, do not weaken)

- The quit/shutdown path must log **booleans, counts, lengths, and error *kinds* only** — never recording/save/transcript content, and never a formatted `AppError` Display that could embed PHI-adjacent text (`SAVE_FAILED` at `restart.rs:174` already embeds a DB error string; do not log the value). Pin this structurally: a comment-stripped `include_str!` assertion that no tracing macro in the quit module references `edit.value` or `value =` (mirror `guard_defaults_and_exit_choice_are_pinned` at `restart.rs:253`).
- `flush_pending_edit` (`restart.rs:138-147`) contains **no** logging; the `value_len` log is in `ensure_safe_to_restart` (`restart.rs:162-165`). Preserve this exactly.
- The whisper-server stderr forwarding is allowlist-prefixed and logs only `len` for everything else; **do not "simplify" it** while touching the adjacent quit path — treat it as an untouchable invariant (file:line `whisper_supervisor.rs`).
- This change introduces **no** network calls, telemetry, or new PHI handling.

## Deliverable

1. Phase 1: the exit guard extended/rewritten so every deliberate exit terminates via `libc::_exit(0)`, with the corrected module doc (normal quits *do* abort without it) and a scoped (verified, not asserted) statement of which writes are atomic and which are refused under the shutdown.
2. Phase 2: the best-effort coordinated-shutdown path (menu/quit-item interception) implementing the 7-step sequence with `try_state`, the widened `restart_committed()` gate, the settled pipeline/chat cancel-vs-refuse policy, awaited stop-sharing, log-buffer flush in the correct order, and the hard timeout.
3. Tests, in the same commit:
   - the guard still calls `libc::_exit(0)` and **never** `std::process::exit` (extend `guard_defaults_and_exit_choice_are_pinned`);
   - the exit predicate triggers `_exit` on the normal-quit path **and** the restart path, and a clean quit with **no managed AppState** (recovery boot) exits without panicking;
   - `start_recording` refuses when the quit/exit flag is set (the widened gate);
   - the shutdown timeout path (a `tokio::time::timeout` around the settle) is exercised;
   - the PHI structural pin (no content-referencing tracing macro in the quit module).
4. The documented edge-path gaps (Dock/Apple-menu quit get the crash fix but not full coordinated shutdown).

## Verification gates (run each and report individually — do not chain into one opaque failure)

- `cargo test --workspace --lib`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `npx vitest run`
- `npm run check`

## Symbolication gate (before you trust the teardown bypass)

The root-cause attribution is a **stack-shape match, not a symbolicated proof** — frames 3–6 are unnamed. Before shipping the `_exit` bypass, symbolicate the four offsets (`30737668`, `12145460`, `12147776`, `12152792`, image base `0x100ee8000`) with `atos` against the exact 0.77.11 binary or its dSYM, and confirm they land in the ORT/knf C++ island. If they do **not**, a blanket `_exit` would be masking a different bug — stop and report instead of shipping the bypass.

## Manual repro gate (the actual acceptance criterion)

Build the release app, launch it, and quit **normally** (Cmd+Q and the Quit menu item) at least three times each. After every quit, confirm **no** new crash report and no crash dialog. "Tests pass" is not the bar; a clean normal quit with no SIGABRT is. Report the clean-quit count, and separately note what (if anything) still diverges on Dock-icon quit.

## Report

Report back: the final guard predicate (and whether you chose unconditional `_exit` or a widened flag), the exact menu/quit-interception mechanism used, the finalized 7-step shutdown sequence with the timeout constant, which backup/export paths are confirmed-atomic vs added to the refusal/timeout set, the `atos` symbolication result for the four frames, the tests added, all gate results, and the manual quit-repro result (N quits, N clean), plus the acknowledged Dock/Apple-menu edge-path gaps.