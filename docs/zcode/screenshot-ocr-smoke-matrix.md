# Screenshot-Region OCR — per-platform smoke-test matrix

Companion to `scripts/smoke-screenshot-ocr.sh`. A single dev run exercises
exactly one platform, so **only cells whose checks were actually executed and
passed in this run are marked ✅ verified** — everything else is ⬜ unverified,
no exceptions. Update this file only after running the script (and its
`--full` interactive leg where applicable) on the platform in question.

| # | Check | macOS (arm64) | Linux X11 | Linux Wayland (Hyprland/Omarchy) | Windows |
|---|---|---|---|---|---|
| 1 | Region-capture mechanism present | ✅ `screencapture` (2026-09-05) | ⬜ overlay + xcap | ⬜ `slurp`+`grim` | ⬜ overlay + xcap |
| 2 | Capture → local vision model OCR → clipboard text | ⬜ | ⬜ | ⬜ | ⬜ |
| 3 | Pixel data never on clipboard; macOS PNG shredded before OCR | ⬜ (code-reviewed + unit tests only) | ⬜ (n/a — no disk) | ⬜ (n/a — no disk) | ⬜ (n/a — no disk) |
| 4 | Global hotkey fires from outside the app | ⬜ | ⬜ | ⬜ (n/a — compositor owns hotkeys) | ⬜ |
| 5 | In-app button + in-app shortcut (hotkey disabled) | ⬜ | ⬜ | ⬜ | ⬜ |
| 6 | Cancel (Esc) is a quiet notice, not an error | ⬜ | ⬜ | ⬜ | ⬜ |
| 7 | `--capture-ocr` delegation to running instance | ⬜ | ⬜ | ⬜ | ⬜ |
| 8 | Cold start (no instance): exit 2 + desktop notification, no stale socket | ✅ (2026-09-05) | ⬜ | ⬜ | ⬜ |
| 9 | Compositor binding (`o.bind` / `bind =`) triggers capture | n/a | n/a | ⬜ | n/a |
| 10 | Rebinding + disable hotkey in Settings applies immediately | ⬜ | ⬜ | ⬜ (disable leg only) | ⬜ |
| 11 | Progress pill visible with readable label during OCR | ✅ (2026-09-07, v0.76.4 — `--pill-selftest` probe + screenshot; see run log) | ⬜ | ⬜ | ⬜ |

## Notes

- **Progress pill background (row 11) is page-painted, not window-painted:**
  wry applies `background_color` to the WEBVIEW only with its `transparent`
  feature, which Tauri enables via `macOSPrivateApi` — deliberately off in
  this project. On macOS the WKWebView therefore keeps its default opaque
  white page background, and because the pill's label/spinner are white,
  v0.75.5–v0.76.0 showed a blank light box (v0.76.1 moved the dark
  background into `OcrProgressIndicator.svelte`'s page CSS so every
  platform is identical). v0.76.1–v0.76.3 were STILL blank on macOS: the
  pill page co-imported `ScreenRegionOverlay.svelte` (one shared hash-route
  branch in `src/main.ts`), whose page-global
  `background: transparent !important` beat the pill's dark rule and left
  the white webview showing. v0.76.4 splits the routes so each page imports
  only its own component (pinned by `src/main.test.ts`). Row 11 can be
  checked WITHOUT a running OCR backend: `rust-medical-assistant
  --pill-selftest` boots a minimal shell, shows the pill (plus a control
  window), logs the committed URL + an in-page probe
  (`mounted:true,htmlBg:rgb(20, 20, 24)` = pass), and exits after ~12 s —
  screenshot mid-run to eyeball the label. Row 11 must still be eyeballed
  per platform: dark pill, white "Recognizing text…" label + spinner
  visible for the OCR duration.
- **Mixed-DPI multi-monitor (known limitation, X11/Windows overlay path):** the
  overlay spans the whole virtual desktop with ONE `scale_factor` (the
  window's), so a drag rectangle on a monitor whose DPI differs from the
  window's reported scale maps to the wrong physical pixels. Single-DPI setups
  (the common case) are correct. Fixing per-monitor DPI needs monitor-aware
  coordinate mapping in `screen_region_submit` — deferred until it bites.
- **macOS Screen Recording permission (TCC):** the first interactive capture
  triggers macOS's Screen Recording permission prompt for FerriScribe; without
  it, `screencapture -i` yields wallpaper-only/empty frames which surface as
  the "cancelled / no text found" outcome. This is a runtime condition the
  interactive legs (rows 2–6) must confirm.
- **XDG portal note (deliberate deviation):** the spec's Linux-Wayland
  priority list names the XDG portal Screenshot as the fallback when
  `slurp`+`grim` are absent. This build does **not** wire the portal: the
  portal writes its screenshot to a compositor-chosen shared temp path outside
  app control, which conflicts with the absolute private-dir/no-shared-`/tmp`
  PHI constraint in the same spec. Wayland users without `slurp`+`grim` get a
  clear actionable error instead ("Install slurp and grim — Omarchy ships
  both"). Revisit only with a portal path that keeps pixels inside
  app-controlled storage.

## Run log

- 2026-09-05, macOS arm64, branch `feat/screenshot-region-ocr`: script
  non-interactive checks executed — `screencapture` present; cold start
  exited 2, notification fired (osascript path confirmed), single-instance
  socket cleaned up. Interactive legs (rows 2–7, 10) require a running app
  with a configured OCR model plus a human dragging the selection — not
  executed in this run, intentionally left unverified. Rows 2–10 on Linux and
  Windows entirely unexecuted (different platforms).
- 2026-09-07, macOS arm64, branch `fix/ocr-pill-blank` (v0.76.4): row 11
  executed via `--pill-selftest` — pill built from a worker thread (the
  production creation context), committed URL
  `tauri://localhost/index.html#ocr-progress`, in-page probe reported
  `mounted:true, htmlBg:rgb(20, 20, 24)` at +1.5/+4/+8 s, and a mid-run
  screenshot shows the dark pill with spinner + "Recognizing text…" label.
  The pre-fix binary reproduced the user's blank white pill under the same
  harness (probe: `htmlBg:rgba(0, 0, 0, 0)` + the overlay's
  `background: transparent !important` sheet present). Full-OCR interactive
  legs (rows 2–7, 10) still pending a human pass.
