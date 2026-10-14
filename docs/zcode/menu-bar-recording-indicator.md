# FerriScribe: Menu-bar / System-tray Recording Indicator

Implement a cross-platform tray indicator for FerriScribe (Tauri 2 + Svelte 5 + Rust, repo root `~/Development/rustMedicalAssistant`, GitHub `cortexuvula/ferriscribe`): an icon in the macOS menu bar / Windows system tray / Linux tray that is RED while a recording session is active and GREEN when idle, with a minimal menu (status text, Show FerriScribe, Quit FerriScribe). Scoped and reviewed by repo-auditor and argus; this prompt encodes their verdicts. Feature name: `tray-recording-indicator`.

## Hard facts (verified file:line — reuse, do not reinvent or guess)

- Tauri 2.11.5 pinned. `src-tauri/Cargo.toml:18`: `tauri = { version = "2", features = [] }` — the `tray-icon` feature is NOT enabled. Add `features = ["tray-icon"]`. Do NOT add `image-png`/`image-ico`: decode icons at compile time with `tauri::include_image!("icons/<file>.png")` (tauri-macros supports this; removes the runtime decode failure path entirely).
- Source of truth: `AppState.recording_active: Arc<tokio::sync::Mutex<bool>>` at `src-tauri/src/state.rs:310`. tokio Mutex, `.lock().await`.
- Recording flag mutation sites (DO NOT modify these files or their ordering rules — see Non-negotiables): `src-tauri/src/commands/audio.rs:121,131,244,270,364,447,712` and `src-tauri/src/commands/translation.rs:423,432,543,629`. A structural test `translation.rs:993-1011` greps translation.rs source for `state.recording_active.lock().await` — refactoring those lines breaks it.
- `get_recording_state` command exists at `audio.rs:805` (returns {active, recording_id, elapsed_secs, paused}); AppState boots with `recording_active=false` (`state.rs:1024-1036`) — fresh-process boots are always idle; only a webview reload in the same process can orphan a live capture.
- Builder: `tauri::Builder::default()` at `lib.rs:212`; `.manage(state)` at :216; the ONLY setup closure at `lib.rs:262` runs only when `app_state_managed` is true (normal boot). Recovery/fatal boots (DB init failed) have no AppState and never run setup — recording cannot happen there, so no tray is correct there. Do NOT add a second setup.
- Tray events route through the SAME global menu-event listener as the app menu (`lib.rs:379-383` currently matches `QUIT_ITEM_ID` → `commands::quit::on_quit_requested`). Add tray item IDs to that existing match. Do NOT use `TrayIconBuilder::on_menu_event` — it registers a second global listener and would double-fire.
- Coordinated quit: `commands::quit::on_quit_requested(app: &tauri::AppHandle)` (`quit.rs:332-344`) is public, idempotent, refuses while recording (`QuitRefusal::RecordingActive`, quit.rs:397-398), flushes pending edits, cancels pipelines, then exits. A tray Quit MUST call this — never `app.exit(0)` directly.
- Window close: `lib.rs:319-333` prevent_close → stop sharing → `window.destroy()`. Tauri runtime requests exit when the LAST window is destroyed (no ExitRequested override in this app; `.run(app_context())` at lib.rs:571). So closing the window exits the app on ALL platforms today. This close handler is a SEPARATE bug (close-while-recording skips coordinated quit) — OUT OF SCOPE, do not modify it.
- Release profile: `panic = "abort"` (Cargo.toml:44-45). Any tray-init panic kills the process; `catch_unwind` cannot help. Tray build must be best-effort: log the error kind and continue, never propagate from setup.

## Design decisions (all resolved)

- D1. **Watcher task, not push.** Do NOT add tray calls at the mutation sites. Spawn ONE tokio task from setup that polls `recording_active` every ~300ms, copies the value out of the lock, and updates the icon/status ONLY when the value changes. Rationale (argus F3): no drift on future recording paths, no races (always shows latest value), no AppHandle injection into `stop_recording`/`cancel_recording` (audio.rs:405, 663 — they currently take no AppHandle), and mutation files stay untouched. Never call tray APIs while holding the mutex guard (argus F7: `set_icon` from a non-main thread waits on the main thread; holding a lock the main thread also takes deadlocks).
- D2. **Tray built in code, in the existing setup closure.** `TrayIconBuilder::new().id("main")` in `lib.rs:262` setup (after the state is managed). Do NOT use a `trayIcon` entry in `tauri.conf.json` — config-defined trays appear in the helper processes too (`--capture-ocr` delegate `lib.rs:723-752`, pill self-test `lib.rs:602-721`), which both reuse `app_context()`.
- D3. **Linux availability guard.** On Linux only, before building the tray, dlopen-check for the runtime library (names, in order: `libayatana-appindicator3.so.1`, `libappindicator3.so.1`, then the unversioned names) using `libc::dlopen`+`dlclose` (libc already a dependency). If none loads: log ONE warning ("tray unavailable"), skip the tray, app continues normally. `libappindicator-sys` panics in its loader when the library is absent (libappindicator-sys-0.9.0/src/lib.rs:13-55) — with panic=abort that kills the app; this guard is mandatory. GNOME without the AppIndicator extension: tray simply won't show — acceptable, no crash.
- D4. **Menu (also the Linux status surface — Linux has NO tooltips, tauri tray/mod.rs:262-268):**
  1. Disabled status item: text "Not recording" / "Recording" (updated by the watcher; `set_text` on the MenuItem). This is the tooltip fallback on Linux and an accessibility signal everywhere.
  2. Separator.
  3. "Show FerriScribe" (id `tray-show`): show + unminimize + focus the main window (same pattern as single-instance re-surface, lib.rs:394-398).
  4. "Quit FerriScribe" (id `tray-quit`): calls `commands::quit::on_quit_requested(app)` via the existing `on_menu_event` match (lib.rs:379-383). Add both IDs to that match.
  Tooltips (macOS/Windows only — set anyway, ignored on Linux): "FerriScribe - Recording" / "FerriScribe - Not recording".
- D5. **Icons: shape + colour, non-template.** Two new PNG assets in `src-tauri/icons/`: `tray-idle.png` and `tray-recording.png` (square, >=36px; 64x64 recommended; macOS renders at 18pt, Windows scales a single-size RGBA PNG). Green = idle, red = recording, AND the two states must differ by shape, not just colour (red/green is the classic colour-blind confusion pair — e.g. filled circle vs hollow ring, or dot vs mic glyph). `icon_as_template(false)` — template icons drop colour and dark-mode/light-mode both must show the colour. Embed with `include_image!`.
- D6. **Red while paused.** `recording_active` stays true during pause (pause only sets `CurrentRecording.paused_at`, state.rs:205; the stream stays open). Red means "capture session in progress" — quit is ALSO refused during pause, so red is consistent with "can't quit". No amber in this feature.
- D7. **PHI-safe by construction.** Only the static strings above appear anywhere (icon, tooltip, menu). NO recording IDs, filenames, timestamps, durations, clinical text, or error strings in tray/tooltip/menu/log additions. Tray code follows the PHI-free logging rule (quit.rs:90-96). Do not add `dbg!()` anywhere.

## Non-negotiable constraints (do not violate)

1. Do not modify `audio.rs` / `translation.rs` recording code, their slot-before-flag ordering, or the source-grepping structural test at translation.rs:993-1011. The watcher reads state; it does not hook mutations.
2. Do not touch the close handler (lib.rs:319-333), `quit.rs` machinery, `restart.rs` exit guard, or window lifecycle. Close behavior is UNCHANGED: closing the window exits the app as today (the indicator lives while the app runs).
3. No cloud, no telemetry, no PHI in logs, no new permissions/CSP/capabilities changes (tray is backend-only; `capabilities/default.json` untouched).
4. No `trayIcon` key in `tauri.conf.json`. Tray is code-built in setup only.
5. Tray errors are logged (kind only) and swallowed — a tray problem must never stop app boot (argus F6; `lib.rs:572` `.expect` on run would turn a propagated setup error into a boot fail).
6. `#[cfg(desktop)]`-guard the new tray module if needed for mobile compile (the crate has a mobile entry point, lib.rs:91). The app's release targets are deb/rpm/appimage/msi/nsis/dmg/app.

## Deliverable (numbered, each with an acceptance criterion)

1. **Cargo feature**: `src-tauri/Cargo.toml:18` → `tauri = { version = "2", features = ["tray-icon"] }`. ACCEPT: `cargo check` compiles; no other Cargo.toml feature changes.
2. **Icons**: `src-tauri/icons/tray-idle.png`, `src-tauri/icons/tray-recording.png` (>=36px, non-template, distinct shape+colour). ACCEPT: files exist, commit, differ in shape AND colour.
3. **New module `src-tauri/src/tray.rs`**: Linux dlopen guard (D3); best-effort `init_tray(app) -> ()` building the tray with menu (D4) and idle icon; `set_recording_state(app, active)` updating icon + tooltip + status-item text; embedded icons via `include_image!`. Registered as `mod tray;` (with `#[cfg(desktop)]` guard) in `src-tauri/src/lib.rs`. ACCEPT: builds on all three desktop targets; no tray created in recovery boot or helper processes; tray absent (not crashed) when Linux runtime lib missing.
4. **Watcher task**: spawned from the setup closure (lib.rs:262) after tray init; polls `recording_active` ~300ms; updates only on change; never calls tray APIs under the mutex guard. ACCEPT: start/stop/pause/cancel and translation-tap cycles always end at the correct colour within ~1s; rapid flip-flop leaves the icon matching the latest flag value.
5. **Menu wiring**: `tray-show` (show/unminimize/focus "main") and `tray-quit` (→ `on_quit_requested`) added to the existing `on_menu_event` match at lib.rs:379-383. ACCEPT: tray Quit while recording shows the refusal (same dialog as app-menu quit); tray Quit when idle exits cleanly through the coordinated sequence (log shows flush + shutdown).
6. **Tests** (in the same commit):
   - Pure function (active: bool) → (icon, tooltip, status text) — unit-tested for both states (argus A6).
   - Watcher change-detection: emits update on 0→1 and 1→0, not on no-change.
   - Structural test: `tauri.conf.json` contains no `trayIcon` key (prevents F4 regression).
   - Regression: existing suite still green, including translation.rs:1006 structural test (files untouched).
7. **CI/packaging check**: Linux CI (ci.yml) installs no appindicator -dev package — correct, since loading is runtime; DO NOT add one. Verify the built `.deb`/`.rpm` declare the runtime dependency (e.g. `libayatana-appindicator3-1`) — if the bundler (tauri 2.11.5) does NOT add it automatically, add `bundle.linux.deb.depends` / `rpm.depends` in `tauri.conf.json`. ACCEPT: report the dpkg/rpm dependency state after a Linux bundle build, with the exact `depends` lines.

## Verification gates (run individually, report each separately)

- `cargo fmt --check` (and `cargo fmt` if it fails — CI runs it)
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --lib` — full suite, plus the new tests
- `cargo build` (debug, macOS at minimum)
- If a Linux box is available: boot with and without the appindicator runtime present; confirm "tray unavailable" log + normal operation without it.

## Report (report back, do not just commit)

- Files changed (absolute paths), with the diff stat.
- Each gate's exact result line(s), including the new test names.
- The `.deb`/`.rpm` runtime-dependency finding (dpkg -I / rpm -qpR), and whether tauri.conf.json needed `depends` additions.
- Any deviation from this spec and why. If the watcher-interval choice (300ms) was changed, say so and why.
- Note: leave changes UNCOMMITTED in the worktree — review happens on the diff before any commit. Do not push, tag, or open PRs.
