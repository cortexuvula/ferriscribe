# rustMedicalAssistant — Project Context

A Tauri + Svelte 5 + Rust desktop app for clinicians: records consultations, transcribes locally (whisper.cpp) or via remote STT, generates SOAP notes via local AI, plus referral / patient-letter / synopsis generation.

## Hard constraints

- **Local-only AI providers.** Only Ollama and LM Studio. No hosted APIs (OpenAI, Anthropic, etc.) — PHI/HIPAA constraint. Do not introduce hosted-provider clients.
- **No PHI in logs.** Patient transcripts, SOAP content, medications, allergies, and conditions must never appear in `tracing::*` macros, `println!`, `eprintln!`, or `console.log` output. Log counts, lengths, IDs — never content.
- **No telemetry / phone-home.** No remote endpoints contacted from this app other than user-configured AI/STT provider URLs.

## Conventions

- **Plan execution:** prefer subagent-driven development (`superpowers:subagent-driven-development`) with TDD per task, fresh subagent per task, two-stage review (spec compliance, then code quality).
- **Branch hygiene:** isolated git worktrees under `.worktrees/` (already gitignored). Never start implementation directly on `master`.
- **Versioning:** synchronized across `src-tauri/Cargo.toml`, `package.json`, and `src-tauri/tauri.conf.json`. Tag releases as `vX.Y.Z` for stable or `vX.Y.Z-beta.N` (or any semver pre-release suffix containing `-`) for prereleases. `release.yml` builds installers on tag push and marks tags with a `-` as GitHub prereleases. Promote a beta to stable by tagging the same commit `vX.Y.Z`.

## Toolchain notes

- Workspace package name for the Tauri app is `rust-medical-assistant` (not `medical-tauri`). Use `cargo build -p rust-medical-assistant` and `cargo test -p rust-medical-assistant`.
- `npm run check` runs `svelte-check`. (Earlier versions invoked `svelte-kit sync` — this is not a SvelteKit project; the prefix has been removed.)
- Frontend tests: `npx vitest run`. Backend tests: `cargo test --workspace --lib`.

## Known constraints worth preserving

- The `recordings.metadata` JSON column holds both freeform `context` (string) and structured `patient_context` (`PatientContext` shape). New metadata keys are non-breaking; index signature on the TS side accepts unknown keys.
- The SOAP system prompt has hardened anti-fabrication rules; treat the prompt as a precision instrument. Background-supplied facts populate historical Subjective fields only — never alter today's Assessment or Plan.
