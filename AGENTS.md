# AGENTS.md

## Hard constraints (PHI / HIPAA)

- **No hosted AI APIs.** Only Ollama and LM Studio. Never introduce OpenAI, Anthropic, or any remote-provider client.
- **No PHI in logs.** Transcripts, SOAP content, medications, allergies, and conditions must never appear in `tracing::*`, `println!`, `eprintln!`, or `console.log`. Log counts, lengths, IDs — never content.
- **No telemetry / phone-home.** The app must not contact any remote endpoint other than user-configured AI/STT provider URLs.

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
