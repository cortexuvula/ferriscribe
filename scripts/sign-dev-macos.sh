#!/usr/bin/env bash
# Re-sign the dev app binary with the local Developer ID identity.
#
# Why this exists: the keychain password prompt for the
# `rustMedicalAssistant` items (db-key, backup-wrapping-key) is keyed to
# the reading binary's CODE IDENTITY. Cargo links the dev binary ad-hoc
# (linker-signed), and an ad-hoc identity changes on EVERY rebuild — so
# each rebuild re-triggers macOS's keychain password prompt at app boot,
# even after "Always Allow". Signing with the stable Developer ID
# certificate makes the identity stable: one "Always Allow" then covers
# all future rebuilds. (Installed releases are already Developer ID
# signed by CI, so they only ever need it once.)
#
# Usage: after building the dev binary, run `npm run sign:dev`.
set -euo pipefail

BINARY="target/debug/rust-medical-assistant"
# Override with FERRISCRIBE_SIGN_IDENTITY if more than one identity exists.
IDENTITY="${FERRISCRIBE_SIGN_IDENTITY:-}"

if [[ "$(uname)" != "Darwin" ]]; then
  echo "sign-dev-macos: macOS only" >&2
  exit 1
fi
if [[ -z "$IDENTITY" ]]; then
  IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null |
    grep -o '"Developer ID Application: .*"' | head -1 | tr -d '"')
fi
if [[ -z "$IDENTITY" ]]; then
  echo "sign-dev-macos: no Developer ID Application identity found in the login keychain." >&2
  echo "Set FERRISCRIBE_SIGN_IDENTITY to the identity name and retry." >&2
  exit 1
fi
if [[ ! -f "$BINARY" ]]; then
  echo "sign-dev-macos: dev binary not found at $BINARY — build first:" >&2
  echo "  cargo build -p rust-medical-assistant" >&2
  exit 1
fi

codesign --force --options runtime --sign "$IDENTITY" "$BINARY"
echo "sign-dev-macos: signed $BINARY"
echo "  identity: $IDENTITY"
echo "First run after signing may prompt once for keychain access — choose"
echo "\"Always Allow\" and this identity stays trusted across rebuilds."
