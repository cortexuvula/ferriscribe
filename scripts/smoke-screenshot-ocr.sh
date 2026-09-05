#!/usr/bin/env bash
# Screenshot-region OCR — per-platform smoke test.
#
# Runs the non-interactive checks automatically, then walks the operator
# through the interactive ones (region selection, OCR, clipboard). Only
# cells whose checks actually PASSED may be marked "verified" in the matrix
# (docs/zcode/screenshot-ocr-smoke-matrix.md) — an in-progress or skipped
# step stays unverified.
#
# Usage:
#   scripts/smoke-screenshot-ocr.sh          # non-interactive checks only
#   scripts/smoke-screenshot-ocr.sh --full   # + interactive flow (needs a
#                                            # running FerriScribe with an
#                                            # OCR model configured, and a
#                                            # human to drag the selection)

set -u

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$REPO_ROOT/target/debug/rust-medical-assistant"
MATRIX="$REPO_ROOT/docs/zcode/screenshot-ocr-smoke-matrix.md"

pass() { echo "  ✅ $1"; }
fail() { echo "  ❌ $1"; }
info() { echo "  ℹ️  $1"; }

echo "== Screenshot-OCR smoke test =="
uname -s | grep -q Darwin && PLATFORM="macos" || PLATFORM="linux"

# ── Non-interactive checks ──────────────────────────────────────────────────

echo "[1] Platform toolchain"
case "$PLATFORM" in
  macos)
    command -v screencapture >/dev/null 2>&1 && pass "macOS: screencapture present" || fail "macOS: screencapture missing"
    ;;
  linux)
    command -v slurp >/dev/null 2>&1 && pass "linux: slurp present" || fail "linux: slurp missing (needed on Wayland)"
    command -v grim  >/dev/null 2>&1 && pass "linux: grim present"  || fail "linux: grim missing (needed on Wayland)"
    ;;
esac

echo "[2] Binary present (cargo build -p rust-medical-assistant first)"
if [ -x "$BIN" ]; then
  pass "debug binary at $BIN"
else
  fail "debug binary missing — run: cargo build -p rust-medical-assistant"
  exit 1
fi

echo "[3] Cold-start rule (no FerriScribe running)"
# Make sure the single-instance socket has no live owner by checking for a
# running instance first.
if pgrep -f "rust-medical-assistant( |$)" | grep -qv $$; then
  info "FerriScribe appears to be RUNNING — skip this check (or quit it first)."
else
  "$BIN" --capture-ocr
  code=$?
  if [ "$code" -eq 2 ]; then
    pass "cold start exits 2 and fires a desktop notification (check your screen)"
  else
    fail "cold start exited $code (expected 2)"
  fi
fi

if [ "${1:-}" != "--full" ]; then
  echo
  echo "Non-interactive checks done. Re-run with --full (and FerriScribe"
  echo "running, OCR model configured) for the interactive capture checks."
  exit 0
fi

# ── Interactive checks ───────────────────────────────────────────────────────

prompt() { # prompt <label> → sets ANSWER=y/n
  while true; do
    read -r -p "$1 [y/n] " ANSWER
    case "$ANSWER" in y|Y|n|N) break ;; esac
  done
}

echo "[4] In-app trigger (Settings → General → Screenshot Region OCR → 'OCR a screen region')"
echo "    Expected: region picker appears; drag over on-screen text; toast says copied."
prompt "    Did text land on the clipboard? "
case "$ANSWER" in y|Y) pass "in-app trigger + OCR + clipboard" ;; *) fail "in-app trigger" ;; esac

echo "[5] Global hotkey (Cmd/Ctrl+Alt+O) from OUTSIDE the app"
echo "    Focus another app, press the hotkey, drag a region."
prompt "    Did the flow complete (toast in FerriScribe, text on clipboard)? "
case "$ANSWER" in y|Y) pass "global hotkey" ;; *) fail "global hotkey" ;; esac

echo "[6] Cancel path"
echo "    Trigger a capture, press Esc without dragging."
prompt "    Was it treated as a quiet cancel (no error toast)? "
case "$ANSWER" in y|Y) pass "cancel path" ;; *) fail "cancel path" ;; esac

if [ "$PLATFORM" = "linux" ]; then
  echo "[7] Compositor binding (Wayland)"
  echo "    Add the o.bind/bind line from Settings → General to your Hyprland"
  echo "    config (check bindings.lua vs bindings.conf!), reload, press Ctrl+Alt+O."
  prompt "    Did the CLI delegation trigger a capture in the running app? "
  case "$ANSWER" in y|Y) pass "compositor delegation" ;; *) fail "compositor delegation" ;; esac
fi

echo
echo "Update $MATRIX — mark ONLY the cells that passed above as verified."
