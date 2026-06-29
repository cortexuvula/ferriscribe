# AGENTS.md

## Hard constraints (PHI / HIPAA)

- **No hosted AI APIs.** Only Ollama and LM Studio. Never introduce OpenAI, Anthropic, ElevenLabs, or any remote-provider client — this includes TTS (only local OS speech synthesis is supported).
- **No PHI in logs.** Transcripts, SOAP content, medications, allergies, and conditions must never appear in `tracing::*`, `println!`, `eprintln!`, or `console.log`. Log counts, lengths, IDs — never content.
- **No telemetry / phone-home.** The app must not contact any remote endpoint other than user-configured AI/STT provider URLs. **Exception:** update checks may contact `github.com/cortexuvula/rustMedicalAssistant` (GitHub Releases) to fetch the update manifest (`latest.json`) and binary. This is an anonymous GET — no patient data, tokens, cookies, or identifying headers are transmitted. The user can disable automatic update checks in Settings → About or during onboarding.

## Commands

```bash
# Backend tests (lib only — integration tests are crate-scoped)
cargo test --workspace --lib

# Sharing integration tests (needs FERRISCRIBE_MDNS_TEST=1 only on Linux)
cargo test -p medical-sharing

# Frontend tests
npx vitest run

# Type-check (runs svelte-check, NOT SvelteKit)
npm run check

# Dev
npm run tauri dev
```

There is no top-level lint command wired up. Run crate-specific checks with `cargo clippy --workspace` if needed.

`npm run check` runs `svelte-check` — an earlier version invoked `svelte-kit sync`; that prefix was removed because **this is not a SvelteKit project**. The README's mention of "SvelteKit" is stale; treat Svelte 5 + Vite as the truth.

## Workspace layout

- `crates/` — 13 library crates, all named `medical-*` (`medical-core`, `medical-db`, `medical-security`, `medical-audio`, `medical-ai-providers`, `medical-stt-providers`, `medical-tts-providers`, `medical-agents`, `medical-rag`, `medical-processing`, `medical-export`, `medical-translation`, `medical-sharing`)
- `src-tauri/` — Tauri app shell; Cargo package name is `rust-medical-assistant` (not `medical-tauri`). Use `cargo build -p rust-medical-assistant` / `cargo test -p rust-medical-assistant`.
- `src/` — Svelte 5 frontend (runes mode). Frontend tests live alongside source as `*.test.ts` and run under vitest with jsdom.
- Front↔back boundary: `src-tauri/src/commands/` (~80 `#[tauri::command]` functions called by `invoke()` from Svelte).

## Versioning

Version is kept in sync across three files — bump all three together:
- `src-tauri/Cargo.toml`
- `package.json`
- `src-tauri/tauri.conf.json`

Release tags: `vX.Y.Z` (stable) or `vX.Y.Z-beta.N` (any tag with `-` is a prerelease). `release.yml` builds installers on tag push.

## Rust toolchain

- Edition 2024, `rust-version = "1.85"`. Use Rust 1.85+.
- `whisper-rs` / `whisper.cpp` requires CMake + Clang at build time.
- Windows CI is excluded from the test matrix because `cpal` device-enumeration crashes on headless runners (no audio hardware). Linux CI needs `libwebkit2gtk-4.1-dev libgtk-3-dev libasound2-dev libssl-dev pkg-config`.

## Domain notes

- `recordings.metadata` JSON column holds both freeform `context` (string) and structured `patient_context` (`PatientContext` shape). New metadata keys are non-breaking.
- The SOAP system prompt has hardened anti-fabrication rules. Background-supplied facts populate only historical Subjective fields — never alter today's Assessment or Plan.
- vitest config mirrors the `dictionary-en` asset resolver from `vite.config.ts`; the plugin is duplicated in both files intentionally (Vitest strips Vite's strict-exports check differently). Keep them in sync if you change one.
- Branch hygiene: isolated git worktrees live under `.worktrees/` (gitignored). Never start implementation directly on `master`.

## Privacy architecture (v0.24+)

These features are load-bearing for HIPAA compliance. Do not regress them:

- **Database:** SQLCipher (AES-256) via `rusqlite`'s `bundled-sqlcipher-vendored-openssl`. Keys live in the OS keychain (`medical-security/src/keychain.rs`). The `AppState::initialize` function refuses to silently fall back to a plaintext DB when patient data exists — it surfaces `InitError::EncryptionUnavailable` instead.
- **Audio recordings at rest:** WAV files are encrypted with AES-256-GCM via `medical-security/src/file_crypto.rs`. The on-disk format is `[magic "FE1" (3 bytes)] [nonce (12 bytes)] [ciphertext]`. The key is derived from the DB key via SHA-256 with a domain separator. `encrypt_file_in_place` is atomic (temp + fsync + rename). Legacy plaintext WAVs are auto-detected (no magic) and read as-is.
- **Orphaned transcripts:** Written as encrypted `.enc` files (same `file_crypto` helper), with a `.txt` fallback only if the keychain is unavailable.
- **Webview CSP:** `tauri.conf.json` has a strict CSP (`default-src 'self'`). Do not set it to `null`. The `connect-src` includes `ipc:`, `asset:`, and `http://asset.localhost` for Tauri IPC + asset protocol.
- **TTS:** Only local TTS (`"local"`). Cloud TTS providers (ElevenLabs etc.) were removed — AGENTS.md's "no hosted AI" rule applies to TTS too. `SUPPORTED_TTS_PROVIDERS` in `settings.rs` is the allowlist.
- **ICD-9 billing codes:** The SOAP generator constrains code selection to the bundled BC MSP ICD-9 list (7,122 codes in `crates/core/icd9_codes.json`). The selector uses a pre-computed inverted index for performance. Post-generation validation flags off-list codes as amber chips in the frontend. Do not change the default from ICD-9 to ICD-10 — BC MSP bills ICD-9.

## Known deferred debt

These items were explicitly deferred from the v0.24 review and are tracked for a future effort:

- **General.svelte** (`src/lib/components/settings/General.svelte`) is 567 lines mixing 6 concerns. Should be split into section components.
- **Recording deletion** is a hard irreversible delete (no soft-delete / undo). The `delete_rag_vectors_best_effort` doc promises an orphan-vector sweeper that doesn't exist yet.
- **`sharing_vocab_api.rs` template handlers** (`templates_rename_handler`, `templates_delete_handler`) still use `(StatusCode, String)` tuples while the other handlers use typed `AppError` — consistency cleanup.
