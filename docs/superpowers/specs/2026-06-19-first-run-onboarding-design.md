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

A new `OnboardingWizard.svelte` with a `step` state machine and a top
progress indicator. Each step has `Skip for now` and `Next` (or `Done` on the
last step). `Skip` advances without saving; `Next` saves the step's inputs
(if changed) then advances. The wizard writes settings via
`settings.updateField(key, value)` (which refuses to run before `loaded`, so
`settings.load()` must precede the gate — it does).

The wizard **branches by deployment mode** chosen right after Welcome. Two
paths share the Welcome and Folder/Done steps; the middle steps differ:

- **Local path** (this machine runs its own AI): Welcome → Mode → AI provider →
  Whisper model → Folder + Done.
- **Server path** (this machine connects to an office server): Welcome → Mode →
  Pair to server → Folder + Done. (No local provider/whisper steps — the
  server supplies them.)

### Step 1 — Welcome

What FerriScribe is, the privacy-first / local-only pitch ("your audio never
leaves your machine"), and a "Let's get started" button. No config. Pure
orientation. This step is also skippable (advances to Mode on click).

### Step 2 — Deployment mode (branch point)

"How will this machine use FerriScribe?" with two radio cards:

- **"This machine runs its own AI"** (Local) → branches to Step 3L (AI
  provider). Default selection (matches `AppConfig` defaults, which assume
  local providers).
- **"This machine connects to an office server"** (Server) → branches to
  Step 3S (Pair). Each card has a one-line description of when to pick it
  (e.g. Local: "for a single computer or the clinic's main server"; Server:
  "for a laptop that offloads to another machine on the network").

The choice is **not persisted as a mode flag** — it only controls which
subsequent steps the wizard shows. A user who picks Server pairs, and the
paired-endpoint config (`sharing-paired.json` + settings) is what makes the
app a client; there's no separate "client mode" boolean to store. A user can
always reconfigure later via Settings → Sharing.

### Step 3L — AI provider (local path)

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

### Step 3S — Pair to office server (server path)

Reuses the **mechanism** of `ClientPair.svelte` (the existing Settings →
Sharing client UI), not the 460-line component itself. The wizard step
implements a trimmed pairing flow:

- A "Found on your network" list from `discover_servers` + `discover_via_tailscale`
  (run on step entry), each with a "Connect" button that prompts for the
  6-digit code and calls `pair_with_server`. A "Rescan" button.
- A fallback "paste a pairing URL" input (`ferriscribe://pair?...`) using the
  same URL parsing as `ClientPair.pairFromUrl`.
- A label field prefilled via `suggestedClientLabel()`.
- Shows the paired status (`paired_endpoint`) once paired, with an "Unpair"
  option to retry.
- This step is skippable too — a user who can't see their server on the
  network (or whose server isn't up yet) can skip and pair later from
  Settings → Sharing.

The pairing commands (`discover_servers`, `discover_via_tailscale`,
`pair_with_server`, `paired_endpoint`, `unpair`, `suggestedClientLabel`) are
all already-registered Tauri commands; no new backend work. The dedup +
address-ranking logic from `ClientPair` (lines 40-98) is copied into the
wizard step — it's ~60 lines and tightly coupled to the discovery result
shape, so duplicating it keeps the wizard self-contained (same rationale as
the whisper step, see "Component reuse").

### Step 4 — Recordings folder + Done (both paths)

- Shows the current folder (`settings.state.storage_path || 'Default
  (application data)'`).
- "Choose folder" button using `@tauri-apps/plugin-dialog`
  `open({directory: true})` → `settings.updateField('storage_path', ...)`.
- "Reset to default" button sets it back to `null`.
- "Done" button sets `onboarding_completed = true` via
  `settings.updateField('onboarding_completed', true)` and dismisses the
  wizard → app shell renders.

## Component reuse

Both reusable settings components are designed for their parent panes with
specific prop/callback contracts. Rather than wedge the wizard into those
contracts, the wizard duplicates the **data-flow patterns** inline:

- **Whisper step (local path):** `WhisperLocalSection.svelte`'s pattern
  (fetch `listWhisperModels`, `downloadModel`, listen for
  `model-download-progress`) — ~30 lines.
- **Pair step (server path):** `ClientPair.svelte`'s discovery + dedup +
  address-ranking + `pair_with_server` flow — ~60 lines.

This keeps the wizard self-contained and avoids coupling step lifecycles to
the settings panes. The duplication is the simpler boundary for both.

Given the wizard now has 5 logical step bodies (Welcome, Mode, Provider,
Whisper, Pair, Folder), extract them into `src/lib/components/onboarding/`
step components (`StepWelcome.svelte`, `StepMode.svelte`, `StepProvider.svelte`,
`StepModel.svelte`, `StepPair.svelte`, `StepFolder.svelte`) from the start,
with `OnboardingWizard.svelte` as the state machine + progress indicator that
renders the active step. This avoids one giant file and matches the "smaller,
focused files" principle.

## Files touched

| File | Change |
|---|---|
| `crates/core/src/types/settings.rs` | Add `onboarding_completed: bool` field (`#[serde(default)]`) |
| `crates/db/src/settings.rs` | Add `config_exists(conn) -> bool` helper (checks for an existing `app_config` row) |
| `src-tauri/src/commands/settings.rs` | In `get_settings`: auto-mark `onboarding_completed = true` when a config already existed but the flag is false; persist |
| `src/lib/types/index.ts` | Add `onboarding_completed: boolean` to `AppConfig` |
| `src/lib/stores/settings.svelte.ts` | Add `onboarding_completed: false` to `defaults` |
| `src/App.svelte` | After `settings.load()`, gate the app shell on `onboarding_completed`; render `<OnboardingWizard>` when false |
| `src/lib/components/OnboardingWizard.svelte` **(new)** | Step state machine + progress indicator; renders the active step component; handles branch (local vs server); writes `onboarding_completed` on Done |
| `src/lib/components/onboarding/StepWelcome.svelte` **(new)** | Welcome / orientation |
| `src/lib/components/onboarding/StepMode.svelte` **(new)** | Deployment-mode radio (local vs server) — the branch point |
| `src/lib/components/onboarding/StepProvider.svelte` **(new)** | Local AI provider select + host/port + test connection (local path) |
| `src/lib/components/onboarding/StepModel.svelte` **(new)** | Whisper model select + download (local path) |
| `src/lib/components/onboarding/StepPair.svelte` **(new)** | Office-server discovery + 6-digit code / URL pairing (server path) |
| `src/lib/components/onboarding/StepFolder.svelte` **(new)** | Recordings folder picker + Done (both paths) |

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
- **Manual (server path):** with an office server running on another machine,
  fresh-install flow → choose "connect to office server" → server appears in
  the discovery list → pair with 6-digit code → paired status shows → Done.
  Also verify the URL-paste fallback and the skip-and-pair-later path.

## Constraints honored

- **No PHI in logs.** The wizard logs nothing beyond step transitions
  ("onboarding step 2", "onboarding completed") — never host/port/model values
  (host/port are config, not PHI, but keeping logs clean matches the codebase
  style).
- **No new remote endpoints.** Test-connection probes go to the user's own
  localhost Ollama/LM Studio.
- **Local-only.** The wizard does not introduce any hosted-AI involvement.

## Open questions resolved

- Scope: **core workflow** with deployment-mode branching.
- Branching: **at the top** — Step 2 asks "runs its own AI" vs "connects to an
  office server" and routes to the local path (provider + whisper) or the
  server path (pair) respectively.
- Strictness: **all steps skippable**.
- Audience: **new installs only**; existing users auto-marked.
- Mode choice: **not persisted as a flag** — it only controls which wizard
  steps show. The paired-endpoint config is what makes the app a client.
- Whisper step: **duplicate the data-flow pattern** inline rather than embed
  `WhisperLocalSection.svelte` (simpler boundary, ~30 lines).
- Pair step: **duplicate the discovery/dedup/pair flow** inline rather than
  embed `ClientPair.svelte` (simpler boundary, ~60 lines).
- Step components: **extract from the start** into `onboarding/` per-step
  files (5 bodies is too much for one file).
- Existing-user detection: **"did a config row already exist"** via
  `config_exists`, not "is some field non-default."
