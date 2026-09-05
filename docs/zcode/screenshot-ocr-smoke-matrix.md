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

## Notes

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
