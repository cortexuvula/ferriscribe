# AGENTS.md

## Hard constraints (PHI / HIPAA)

- **No hosted AI APIs.** Only Ollama and LM Studio. Never introduce OpenAI, Anthropic, ElevenLabs, or any remote-provider client — this includes TTS (only local OS speech synthesis is supported).
- **No PHI in logs.** Transcripts, SOAP content, medications, allergies, and conditions must never appear in `tracing::*`, `println!`, `eprintln!`, or `console.log`. Log counts, lengths, IDs — never content.
- **No telemetry / phone-home.** The app must not contact any remote endpoint other than user-configured AI/STT provider URLs. **Exception:** update checks may contact `github.com/cortexuvula/ferriscribe` (GitHub Releases) to fetch the update manifest (`latest.json`) and binary. This is an anonymous GET — no patient data, tokens, cookies, or identifying headers are transmitted. The user can disable automatic update checks in Settings → About or during onboarding.

## Commands

```bash
# Backend tests (lib only — integration tests are crate-scoped)
cargo test --workspace --lib

# Sharing integration tests (needs FERRISCRIBE_MDNS_TEST=1 only on Linux)
cargo test -p medical-sharing

# DB integration tests (condition_chips_sync, content_sync, encryption,
# recording_sync_merge — NOT covered by --lib above)
cargo test -p medical-db

# Audio device tests are gated behind FERRISCRIBE_AUDIO_TEST=1 (cpal
# enumeration can block indefinitely on machines with busy/virtual audio
# hardware — same reason Windows CI is excluded). Without the env var they
# skip, so `cargo test --workspace --lib` stays runnable on dev machines.
FERRISCRIBE_AUDIO_TEST=1 cargo test -p medical-audio --lib

# Frontend tests
npx vitest run

# Coverage — thresholds (floors calibrated 2026-08-25, ratchet up as gaps
# close) live in vitest.config.ts and gate CI's frontend job.
npm run test:coverage

# Rust coverage — one-time setup:
#   cargo install cargo-llvm-cov --locked && rustup component add llvm-tools-preview
# Same scope as the lib-test gate above; CI enforces --fail-under-lines.
cargo llvm-cov --workspace --lib --summary-only
cargo llvm-cov --workspace --lib --html   # writes coverage/index.html

# Type-check (runs svelte-check, NOT SvelteKit)
npm run check

# Rust formatting + lints — both gates enforced by CI on push/PR.
# Run before pushing; fmt drift on master only gets caught by the next PR.
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Dev
npm run tauri dev
```

`npm run tauri dev` chains `npm run build:sidecar` (stages the `ferriscribe-backup` Tauri sidecar under `src-tauri/binaries/`, gitignored). The first run pays one `medical-backup` release build; afterwards cargo's cache makes it seconds. Local release builds (`npm run tauri build`) must run `npm run build:sidecar` first — CI's `release.yml` does this explicitly per target. Never commit anything under `src-tauri/binaries/`.

CI (`ci.yml`, lint job) enforces `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` on every push to master and every PR. Run both locally before pushing — there is no separate "lint" npm script; invoke the cargo commands directly as shown. Frontend linting uses `npm run lint` (eslint), also gated in CI.

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
- **ICD-9 billing codes:** The SOAP generator constrains code selection to the bundled BC MSP ICD-9 list (7,122 codes in `crates/core/icd9_codes.json`). The selector uses a pre-computed inverted index for performance. Do not change the default from ICD-9 to ICD-10 — BC MSP bills ICD-9. Since v0.61 the stored SOAP note carries **no code lines**: `generate_soap` strips the model's `ICD-9 Code: …` lines (`soap_generator::postprocess::extract_icd_codes`) and persists them as structured `metadata.icd_codes` (`{code, description, kind}`); the frontend's billing-code list renders from that (`resolveIcdCodes` in `src/lib/icd.ts`), falling back to mining the note text only for legacy recordings (no `icd_codes` key). The model-written descriptions (and the official MSP descriptions via `get_icd9_descriptions`) are the list's explaining titles. Post-generation validation still flags off-list codes as amber.

## Known deferred debt

These items remain from various reviews and are tracked for a future effort:

- **Sync/merge remaining items (2026-08-17 bug review; deletion-model redesign shipped on `feat/deletion-model`)** — fixed there: restore-vs-tombstone LWW, chips/dictionary tombstone propagation, purge-ledger resurrection safety, Equal-tie field divergence (max(revision,row) stamps on both builders). Still open: (5) one future timestamp (clock skew) pins pull cursors fleet-wide; (6) audio uploads capped at `take(10)` per push batch with no retry queue (`pending_audio_uploads` in m015 is dead code); (7) the dictionary list/sync feedback storm (every list = full push + unconditional server broadcast — suppress the broadcast on no-op merges); (8) full revision-coverage for every `RecordingsRepo::update` writer (the max(revision,row) rider mitigates but writers should upsert revisions); (9) `set_encryption_done` leaves tombstoned rows FTS-indexed by design (trigger-corruption workaround), so a later `sync_restore`/`restore` on such a row double-indexes — duplicate search results and index drift only (no disclosure: search resolves ids through `get_many` which filters tombstones); fix by probing index membership before re-indexing, pinned with a MATCH-count test.
- **Remaining bug-review items (2026-08-17)** — duplicate `recording-updated` handling (App.svelte listener has no dirty check, clobbers in-flight edits); `selectRecording` staleness race; mid-recording crash leaves row-less plaintext WAVs invisible to sweeps; sync tombstones don't de-index FTS (can fail restore/purge); `settings.updateField` whole-config snapshot vs pairing write race; vocab API binds `0.0.0.0` plain HTTP (Tailscale rule is client-side only); `ConditionChipsRepo::add` resets `use_count` on resurrection; deferred transactions don't serialize MAX-then-INSERT (`SQLITE_BUSY_SNAPSHOT` spurious failures — consider `BEGIN IMMEDIATE`); `stop_recording` releases `recording_active` before consuming `current_recording`.

- **Dependency version pins** — `printpdf` 0.7 (its lopdf 0.31 copy is write-only; user-PDF parsing moved to pdf-extract 0.12 / lopdf 0.42 in 2026-08; 0.9 is a major API rewrite, deferred. cargo-audit 2026-08-28: lopdf 0.31 carries RUSTSEC-2026-0187, a parse-path stack overflow — not reachable through the write-only export copy; revisit with the printpdf 0.9 migration), `rusqlite` 0.32 (blocked by SQLCipher/libsqlite3-sys conflict — newer rusqlite requires a different libsqlite3-sys that conflicts with bundled-sqlcipher), `ort` `=2.0.0-rc.12` (exact-pinned: rc.13 exists but was never validated against this crate; no stable 2.x yet — re-pin deliberately, not via lock drift). All are ecosystem-blocked, not neglect. (Resolved 2026-08-28: calamine 0.26 → 0.36 dropped quick-xml 0.31 — RUSTSEC-2026-0194/0195, DoS-class parsing bugs on user-supplied spreadsheets — from the graph entirely.)
- **Pdfium download verification helper exists; migrate remaining paths** (2026-08-17) — `medical-core/src/net.rs` provides `download_bytes`/`download_file` with timeouts + optional SHA-256; pdfium now downloads hash-verified (pinned digests in `PDFIUM_SHA256`) and whisper/STT models route through the same helper.
- **`state.rs::initialize()` remains large; some command files untested** (2026-08-17 review) — the boot sweeps were extracted to `src-tauri/src/sweeps.rs` (unit-tested) and `content_sync.rs` now has cursor/wire-building tests, but `recovery.rs`, `recordings.rs`, and `sharing/lifecycle.rs` commands still have no shell-side tests. Split `initialize()` further before extending it.
- **Remaining copy-paste clusters** (2026-08-17 review) — Ollama/LM Studio endpoint-resolution (`ollama.rs`/`lmstudio.rs`), RecordTab/GenerateTab patient-context state, ContextPanel/PatientContextSidebar parallel surfaces.

### Resolved (kept for historical context)
- ~~**Commands-layer duplication**~~ — resolved 2026-08-17: shared `load_app_config` helper (was ~15 inline spawn_blocking copies), parameterized `test_models_endpoint` for the test-connection trio, `paired_endpoints` single source of truth, `usePairing()` composable for ClientPair/StepPair, `unchecked_transaction()` standardization in `crates/db` (manual BEGIN/COMMIT with swallowed ROLLBACKs removed).
- ~~**Dead modules**~~ — removed 2026-08-17: `rag::query_expander`, `translation::canned_responses`, `audio::playback`, `db::recipients`, `ai_providers::CircuitBreaker`, plus the unused `proptest` and `machine-uid` deps.
- ~~**Runes compile parity**~~ — `svelte.config.js` now sets `compilerOptions.runes = true` (single source for both `vite.config.ts` and `vitest.config.ts`).
- ~~**General.svelte split**~~ — done in v0.29; now 32 lines, delegates to `settings/sections/` components.
- ~~**Recording soft-delete/undo**~~ — done in v0.29; m009 migration, `soft_delete`, `restore_recording`, Undo toast, and 30-day tombstone sweeper (with RAG vector cleanup) are all wired.
- ~~**Template handlers typed AppError**~~ — done in v0.24.3.
- ~~**`delete_rag_vectors_best_effort` orphan sweeper**~~ — done in v0.30.26; tombstone sweeper in `state.rs` now cleans RAG vectors before purging.
