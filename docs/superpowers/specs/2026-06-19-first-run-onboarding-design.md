# First-Run Onboarding Wizard

**Date:** 2026-06-19
**Status:** Approved → implementation planning
**Scope:** `crates/core/src/types/settings.rs`, `src-tauri/src/commands/settings.rs`, `src/lib/types/index.ts`, `src/lib/stores/settings.svelte.ts`, `src/App.svelte`, new `OnboardingWizard.svelte` + step components

## Problem

FerriScribe has no first-run / onboarding flow. On first launch the app opens
straight to the Record tab in an empty state. A new clinician is expected to
know to go to Settings and configure Ollama/LM Studio, pick + download a
whisper model, and confirm the recordings directory before anything works.
The first time they hit Record or Generate without a provider configured,
they get an `EndpointOffline` error with no guided setup to resolve it.

## Design choices (approved)

- **Scope:** Core workflow only — Welcome → AI provider → Whisper model →
  Recordings folder → Done. Sharing, prompts, etc. left to Settings.
- **Strictness:** Every step is skippable. The wizard never blocks; a user
  without Ollama running can skip and fix it later in Settings.
- **Audience:** New installs only. Existing users (who already have a saved
  config) are auto-marked onboarded on their next launch and never see the
  wizard.

## Gating mechanism

### New config field

Add `onboarding_completed: bool` to `AppConfig`
(`crates/core/src/types/settings.rs`), `#[serde(default)]` → `false`. Because
`AppConfig::default()` deserializes `"{}"`, a brand-new install produces a
fully-populated default config with `onboarding_completed = false` — this is
the "fresh install" signal. Mirrored in:
- `src/lib/types/index.ts` (`AppConfig` interface)
- `src/lib/stores/settings.svelte.ts` (`defaults` object)

### Existing-user auto-mark

In the `get_settings` Tauri command (`src-tauri/src/commands/settings.rs`),
after loading the config: if a config blob already existed in the DB **and**
`onboarding_completed` is `false`, set it to `true` and persist. This means
anyone with an existing FerriScribe install is auto-marked onboarded on their
next launch — only truly fresh installs (no prior saved config) see the
wizard. The detection is "did a row already exist for `app_config` before this
load," not "is some field non-default" (which would be fragile).

`SettingsRepo::load_config` (`crates/db/src/settings.rs:49-63`) currently
returns the config without indicating whether it was freshly-created vs.
loaded. The implementation adds a `config_exists(conn) -> bool` helper to
`SettingsRepo` (a `SELECT 1 FROM settings WHERE key = 'app_config'` check)
and the `get_settings` command calls it **before** `load_config`. If
`config_exists` is true but the loaded `onboarding_completed` is false, the
command sets it true and persists. This avoids the ambiguity of
`load_config` creating a default row on first access (which would make a
naive post-load check always return true).

### Frontend gate

In `App.svelte` `onMount`, after `settings.load()` (line 121), check
`settings.state.onboarding_completed`. If `false`, render only
`<OnboardingWizard>` instead of the app shell — the same `{#if}` overlay
pattern as the existing `recoveryReason` gate at line 205. The wizard is a
full-screen blocking modal; the rest of `onMount` (pipeline init, audio
rehydrate, event listeners) still runs so the app is ready when the wizard
finishes.

## Wizard structure

A new `OnboardingWizard.svelte` with `step = $state(0)` (0–3) and a top
progress indicator (4 dots/bars). Each step has `Skip for now` and
`Next` (or `Done` on step 3). `Skip` advances without saving; `Next` saves
the step's inputs (if changed) then advances. The wizard writes settings via
`settings.updateField(key, value)` (which refuses to run before `loaded`, so
`settings.load()` must precede the gate — it does).

### Step 1 — Welcome

What FerriScribe is, the privacy-first / local-only pitch ("your audio never
leaves your machine"), and a "Let's get started" button. No config. Pure
orientation. This step is also skippable (advances to step 1 on click).

### Step 2 — AI provider

- A `<select>` for Ollama vs LM Studio (defaults to `settings.state.ai_provider`).
- Host/port inputs (prefilled from settings: LM Studio localhost:1234, Ollama
  localhost:11434; the non-active provider's fields are editable but only the
  active one is tested).
- A "Test connection" button using the existing `testLmStudioConnection` /
  `testOllamaConnection` API, with the `idle|testing|success|error` status
  pattern (reuse the visual style from `Models.svelte:294-315`).
- On Next: save via `settings.updateField('ai_provider', ...)`,
  `settings.updateField('lmstudio_host'/'ollama_host', ...)`, etc., then call
  `reinitProviders()` so the active provider is live.
- On Skip: advance without saving. Defaults already point at localhost, so the
  app isn't broken — it just isn't verified.

### Step 3 — Whisper model

- Reuses the **data-flow pattern** of `WhisperLocalSection.svelte` (not the
  component itself — see "Component reuse" below): on mount, fetch
  `listWhisperModels()`; render a `<select>` bound to
  `settings.state.whisper_model` plus a per-model download row with progress
  from `model-download-progress` events.
- Defaults to `large-v3-turbo`. User can pick a smaller/faster model or defer
  download.
- On Next: `settings.updateField('whisper_model', ...)`. Download is skippable
  (the model can be downloaded later from Settings → Audio).
- On Skip: advance without saving.

### Step 4 — Recordings folder + Done

- Shows the current folder (`settings.state.storage_path || 'Default
  (application data)'`).
- "Choose folder" button using `@tauri-apps/plugin-dialog`
  `open({directory: true})` → `settings.updateField('storage_path', ...)`.
- "Reset to default" button sets it back to `null`.
- "Done" button sets `onboarding_completed = true` via
  `settings.updateField('onboarding_completed', true)` and dismisses the
  wizard → app shell renders.

## Component reuse

`WhisperLocalSection.svelte` is designed to be embedded inside the Audio
settings pane with callbacks (`onModelChange`, `onDownload`, `onDelete`) and
props (`whisperModels`, `downloadingModel`, `downloadProgress`). Rather than
wedging the wizard into that prop contract, the wizard step will **duplicate
the data-flow pattern** (fetch `listWhisperModels`, `downloadModel`, listen
for `model-download-progress`) inline. This keeps the wizard self-contained
and avoids coupling step 3's lifecycle to the Audio pane's. The duplication is
~30 lines and is the simpler boundary.

If the wizard file grows large (>~300 lines), extract the step bodies into
`onboarding/StepWelcome.svelte`, `StepProvider.svelte`, `StepModel.svelte`,
`StepFolder.svelte`. Decide during implementation based on size.

## Files touched

| File | Change |
|---|---|
| `crates/core/src/types/settings.rs` | Add `onboarding_completed: bool` field (`#[serde(default)]`) |
| `crates/db/src/settings.rs` | Add a way to tell whether `load_config` created a fresh default vs loaded an existing row (e.g. `load_config_with_existed_flag`) |
| `src-tauri/src/commands/settings.rs` | In `get_settings`: auto-mark `onboarding_completed = true` when a config already existed but the flag is false; persist |
| `src/lib/types/index.ts` | Add `onboarding_completed: boolean` to `AppConfig` |
| `src/lib/stores/settings.svelte.ts` | Add `onboarding_completed: false` to `defaults` |
| `src/App.svelte` | After `settings.load()`, gate the app shell on `onboarding_completed`; render `<OnboardingWizard>` when false |
| `src/lib/components/OnboardingWizard.svelte` **(new)** | The 4-step wizard: step state, progress indicator, per-step UI, settings writes, completion |

## Backend change (minimal)

Only the `onboarding_completed` field is added to `AppConfig`. No new Tauri
commands — the wizard reuses `save_settings`, `test_*_connection`,
`reinit_providers`, `list_whisper_models`, `download_model`. The
existing-user auto-mark is ~5 lines in `get_settings`.

## Testing

- **Type-check** (`npm run check`): the new field flows through the TS types.
- **vitest** (`npx vitest run`): existing tests unaffected; no unit tests for
  the wizard UI itself (matches the codebase pattern of not unit-testing
  Svelte UI components).
- **Rust** (`cargo test --workspace --lib`): the `settings.rs` change may
  affect existing settings tests — update any that assert the exact field
  count or serialize/deserialize round-trips.
- **Manual (fresh-install simulation):** delete the `app_config` row from the
  settings DB (or point at a fresh data dir) → wizard appears; walk through
  + Done → wizard doesn't reappear on relaunch. Then simulate an existing
  user (restore the row) → wizard does NOT appear.

## Constraints honored

- **No PHI in logs.** The wizard logs nothing beyond step transitions
  ("onboarding step 2", "onboarding completed") — never host/port/model values
  (host/port are config, not PHI, but keeping logs clean matches the codebase
  style).
- **No new remote endpoints.** Test-connection probes go to the user's own
  localhost Ollama/LM Studio.
- **Local-only.** The wizard does not introduce any hosted-AI involvement.

## Open questions resolved

- Scope: **core workflow** (4 steps).
- Strictness: **all skippable**.
- Audience: **new installs only**; existing users auto-marked.
- Whisper step: **duplicate the data-flow pattern** inline rather than embed
  `WhisperLocalSection.svelte` (simpler boundary, ~30 lines).
- Existing-user detection: **"did a config row already exist"**, not
  "is some field non-default."
