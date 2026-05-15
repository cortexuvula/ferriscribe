# Security Policy

FerriScribe handles Protected Health Information (PHI) when used in clinical settings. This document describes the security posture, how to report vulnerabilities, and which versions are supported.

## Threat model summary

FerriScribe is a **local-first, on-device** application. The default deployment is intended to keep PHI on the clinician's machine. Two trust boundaries matter most:

1. **The app's local data store.** Recordings, transcripts, SOAP notes, medications, allergies, and conditions live in an on-device SQLite database (encrypted at rest with SQLCipher) and audio files in the OS-provided app data directory.
2. **User-configured network endpoints.** STT (Whisper) and AI providers (Ollama / LM Studio) may be configured to run on the same machine or on a LAN / Tailscale peer. Audio and prompts flow to those endpoints when the user invokes them.

The application is **not** intended to be exposed to the public internet, and there is no telemetry, analytics, crash reporting, or remote logging.

## Hard constraints enforced by the project

These rules are project invariants. Maintainers and contributors are expected to uphold them; violations are treated as security defects.

- **Local-only AI providers.** Only Ollama and LM Studio are supported. No hosted-API clients (OpenAI, Anthropic, etc.) are wired up. The Settings UI is the only place a base URL is set, and clinicians are expected to point it at a loopback / LAN / Tailscale endpoint.
- **No PHI in logs.** Patient transcripts, SOAP content, medications, allergies, and conditions must never appear in `tracing::*`, `println!`, `eprintln!`, or `console.log` output. Logs may record lengths, counts, IDs, and structural metadata — never content.
- **No telemetry / phone-home.** The application contacts only the AI/STT endpoints the user configures. It does not POST to any analytics, error-reporting, or update-check service.
- **PHI never leaves the device by default.** Export of training data, FHIR bundles, PDF/DOCX documents, and audio is always an explicit user action targeting a local filesystem path the user chooses.

## Data flows

| Flow | Direction | Triggered by | Crosses network? |
|---|---|---|---|
| Audio capture → on-device file | local | Recording | No |
| Audio file → STT provider | local or LAN | Transcription | Only if STT mode = Remote |
| Transcript + context → AI provider | local or LAN | SOAP / referral / letter / chat | Only if AI host is non-loopback |
| Document export | local | User clicks Export | No |
| Training corpus export | local | User clicks Export | No |
| Pairing with office server | LAN / Tailscale | User scans QR / enters code | Yes, between paired peers only |

There are no analytics, crash reports, version check pings, or any other unsolicited outbound connections.

## Cryptographic posture

- **Database encryption.** SQLite via [SQLCipher](https://www.zetetic.net/sqlcipher/) with a per-installation key derived from a per-machine identifier or, if set, the `MEDICAL_ASSISTANT_MASTER_KEY` environment variable. See `crates/db/src/encryption.rs`.
- **API key storage.** STT/AI provider bearer tokens and pairing secrets live in the OS-native keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service via the `keyring` crate). They are not stored in SQLite or in plain config files.
- **At-rest API-key envelope.** Where API keys are persisted to disk (legacy paths) they are wrapped with AES-256-GCM; the cipher key is derived via PBKDF2-HMAC-SHA256 (600 000 iterations).
- **Pairing transport.** Office-server pairing uses bearer-token auth over HTTPS / mTLS within the LAN or Tailscale tailnet that the paired peers share; tokens are scoped per service.

## Reporting a vulnerability

If you believe you have found a security issue, please report it privately. Do **not** open a public GitHub issue for vulnerabilities involving PHI exposure, key/credential leaks, or remote code execution.

- **Preferred:** open a [private security advisory](https://github.com/cortexuvula/rustMedicalAssistant/security/advisories/new) on GitHub.
- **Alternative:** email the maintainer at the address listed in the repository owner's GitHub profile.

When reporting, please include:

1. A clear description of the issue and its potential impact (PHI exposure, RCE, privilege escalation, etc.).
2. The affected version (`Settings → About` shows the version; `git describe` works on a clone).
3. Steps to reproduce, ideally a minimal proof of concept.
4. Your suggested fix, if you have one.

We aim to acknowledge reports within 5 working days and to release a patch within 30 days for high-severity issues. Coordinated disclosure is appreciated.

## Supported versions

Only the most recent minor version receives security fixes. Patch releases (`vX.Y.Z`) supersede prior `vX.Y.*` and are the supported version for that minor line.

| Version | Supported |
|---|---|
| latest `v0.10.x` | ✅ |
| anything older | ❌ — please upgrade |

Until the project reaches `v1.0.0`, breaking changes may be required as part of a security fix.

## Reporting non-security bugs

For non-security issues, please open a regular [issue](https://github.com/cortexuvula/rustMedicalAssistant/issues) on the repository.
