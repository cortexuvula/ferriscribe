# In-App Auto-Updater with User Consent

**Date:** 2026-06-22
**Status:** Approved → implementation planning
**Scope:** `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `src-tauri/tauri.conf.json`, `package.json`, `crates/core/src/types/settings.rs`, `.github/workflows/release.yml`, `AGENTS.md`, new updater store + banner + About pane + onboarding step, frontend types

## Problem

FerriScribe has no in-app updater. Users discover new versions by chance
(visiting GitHub) and must manually download + replace the app. Non-technical
clinicians — the target audience — are unlikely to do this. The app needs to
detect available updates and walk the user through installing them, across
macOS, Windows, and Linux.

## Design choices (approved)

- **Host:** GitHub Releases (anonymous GET for `latest.json` + the update
  binary). No separate CDN.
- **UX:** Auto-check on launch + every 12h while running; non-intrusive banner
  when an update is available; "Download & Install" + restart. Manual "Check
  for updates" button in Settings → About.
- **Signing:** Tauri updater signing key pair (one-time `tauri signer generate`);
  public key in `tauri.conf.json`, private key + password in GitHub Secrets.
  CI signs every release artifact; app verifies signature before installing.
- **Consent:** New onboarding step (between Welcome and Setup Mode) asks the user
  to opt in/out of automatic update checks. A toggle in Settings → About
  controls it at any time. Default: auto-check ON.

## PHI / AGENTS.md policy update

AGENTS.md line 7 currently forbids all remote contact except user-configured
AI/STT providers. Update checks require contacting GitHub. The rule is updated
to add an explicit exception:

> Update checks may contact `github.com/cortexuvula/rustMedicalAssistant`
> (GitHub Releases) to fetch the update manifest and binary. No patient data is
> transmitted; only an anonymous GET for `latest.json` and the update artifact.

No PHI is transmitted. The request is an anonymous HTTP GET; no headers,
cookies, tokens, or body carry identifying or clinical information.

## Gating mechanism — consent

### New config field

Add `auto_update_check: bool` to `AppConfig`
(`crates/core/src/types/settings.rs`), `#[serde(default = "default_true")]`
so existing users (who upgrade into this feature) get auto-check ON by default,
matching the new-user default. Mirrored in:
- `src/lib/types/index.ts` (`AppConfig` interface)
- `src/lib/stores/settings.svelte.ts` (`defaults` object)

### Onboarding consent step

New `StepUpdates.svelte` inserted between Welcome and Setup Mode:

```
Step 0: Welcome
Step 1: Automatic updates  ← NEW
Step 2: Setup mode (branch)
Step 3: Provider (local) / Pair (server)
Step 4: Model (local) / Folder+Done (server)
Step 5: Folder+Done (local)
```

The step explains:
- What is checked (version manifest from GitHub Releases).
- What is sent (nothing — anonymous GET, no patient data).
- Two options: "Check for updates automatically (recommended)" (default) /
  "I'll check manually".
- The choice is saved as `auto_update_check` via
  `settings.updateField('auto_update_check', true/false)`.
- Skippable; skipping defaults to auto-check ON (the recommended path).

The wizard's `stepLabels` arrays are updated to reflect the new step count
(6 for local path, 5 for server path).

### Settings → About toggle

The new About pane contains:
- Current version display (`settings.state.version` or the tauri.conf value).
- ☑ "Check for updates automatically" checkbox — flips `auto_update_check` via
  `settings.updateField` + immediately starts/stops the 12h interval.
- "Check for updates now" button — works regardless of the toggle (manual check
  is always available).
- Last-checked timestamp.

### Runtime gate

On launch: `if (settings.state.auto_update_check) { checkForUpdate();
startInterval(); }`. The updater store subscribes to the setting reactively —
toggling it in Settings starts/stops the interval without a restart.

## Architecture — Tauri v2 updater

### Dependencies

| File | Addition |
|---|---|
| `src-tauri/Cargo.toml` | `tauri-plugin-updater = "2"` in `[dependencies]` |
| `package.json` | `@tauri-apps/plugin-updater` in `dependencies` |

### Plugin registration

`src-tauri/src/lib.rs`: add `.plugin(tauri_plugin_updater::Builder::new().build())`
to the Tauri builder chain (alongside the other plugins).

### tauri.conf.json changes

```jsonc
{
  "bundle": {
    "targets": ["deb", "rpm", "msi", "nsis", "dmg", "app", "updater"],
    //                                                              ^^^^^^^^^^ new
    "createUpdaterArtifacts": true  // new — tells tauri build to produce .sig files
  },
  "plugins": {
    "updater": {
      "pubkey": "<BASE64_PUBLIC_KEY>",  // from `tauri signer generate`
      "endpoints": [
        "https://github.com/cortexuvula/rustMedicalAssistant/releases/latest/download/latest.json"
      ]
    },
    "deep-link": { /* existing */ }
  }
}
```

### Signing key (one-time setup)

Generated by the user locally; I provide the exact commands in the implementation
plan. The **public key** goes in `tauri.conf.json` (`plugins.updater.pubkey`).
The **private key + password** go in GitHub Secrets
(`TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). The CI signs
every release artifact; the app verifies the signature before installing.

Until the key is generated and added to secrets, the updater infrastructure is
wired but no signed artifacts are produced — the banner would show "update
available" but installation would fail signature verification. This is clearly
documented in the plan as a prerequisite step.

## CI changes

`.github/workflows/release.yml` — pass the signing env vars to tauri-action on
all three platform jobs:

```yaml
env:
  GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
  TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
```

With `createUpdaterArtifacts: true` in tauri.conf.json and the signing key
present, tauri-action automatically produces and uploads:
- `.app.tar.gz` + `.app.tar.gz.sig` (macOS)
- `FerriScribe-setup.exe` + `.sig` (Windows NSIS)
- `FerriScribe_version_amd64.AppImage` + `.sig` (Linux)
- `latest.json` (the Tauri updater manifest — per-platform download URLs +
  signatures + version)

## Components

### Updater store — `src/lib/stores/updater.svelte.ts` (new)

```typescript
class UpdaterStore {
  state = $state<'idle' | 'checking' | 'available' | 'downloading' | 'installed' | 'error'>('idle');
  availableVersion = $state<string | null>(null);
  downloadProgress = $state<number>(0);
  errorMessage = $state<string | null>(null);
  lastCheckedAt = $state<Date | null>(null);

  private intervalId: ReturnType<typeof setInterval> | null = null;

  async checkForUpdate(): Promise<void>;       // calls @tauri-apps/plugin-updater
  async downloadAndInstall(): Promise<void>;   // download + verify sig + install
  startAutoCheck(): void;                       // 12h interval, gated on auto_update_check
  stopAutoCheck(): void;
}
```

The store imports `check` from `@tauri-apps/plugin-updater` and
`getCurrentWindow` from `@tauri-apps/api/window` (for the restart prompt).

### Update banner — `src/lib/components/UpdateBanner.svelte` (new)

A slim banner fixed at the top of the app shell (below the title bar, above the
content). Only renders when `updater.state === 'available'` or `'downloading'`.

States:
- **available:** "🟢 FerriScribe X.Y.Z is available — [Download & Install] [Later]"
- **downloading:** "Downloading… [progress bar]"
- **installed:** "Update installed — [Restart now] [Later]"
- **error:** "Update failed: {message} — [Retry] [Dismiss]"

"Later" dismisses the banner (sets state back to idle) but doesn't disable
future checks. The banner reappears on the next check if the version is still
newer.

### Settings → About — `src/lib/components/settings/About.svelte` (new)

- Current version (large, prominent).
- "Check for updates automatically" checkbox (bound to
  `settings.state.auto_update_check`).
- "Check for updates now" button (calls `updater.checkForUpdate()`).
- Last-checked timestamp.
- Update status / progress (if a check or download is active).
- Link to the GitHub Releases page (for users who prefer manual download or
  are on a deb/rpm Linux install where in-place update isn't supported).

### Onboarding step — `src/lib/components/onboarding/StepUpdates.svelte` (new)

Two radio cards:
- **"Check for updates automatically (recommended)"** — explains the GitHub
  check, no PHI sent, anonymous GET. Default selected.
- **"I'll check manually"** — explains the Settings → About button.

On Next: saves via `settings.updateField('auto_update_check', ...)`.
On Skip: defaults to `true` (recommended path) and advances.

## Platform behavior

| Platform | Update artifact | Install mechanism | Notes |
|---|---|---|---|
| **macOS** | `.app.tar.gz` + `.sig` | Plugin extracts + replaces `.app` bundle, restarts | Apple Silicon only (Intel dropped per release.yml) |
| **Windows** | NSIS `.exe` + `.sig` | Plugin runs NSIS installer silently, restarts | Unsigned installer (no Windows code-sign cert today); updater signature still verifies |
| **Linux** | AppImage + `.sig` | Plugin replaces AppImage file, restarts | deb/rpm users see the banner but must update via their package manager — documented in the banner |

For Linux deb/rpm: the banner shows "Update available" but the "Download &
Install" button is replaced with a "Download from GitHub" link, since the
Tauri updater can't replace an apt/rpm-installed package in place. AppImage
users get the full in-place update.

**Detecting the install format:** the updater store checks
`import.meta.env` / the running binary path. If the app is running from an
AppImage (the executable path ends in `.AppImage` or the `APPIMAGE` env var
is set), in-place update is offered. Otherwise (deb/rpm install in
`/usr/bin/`), the banner links to GitHub Releases. The detection is
best-effort — if undetermined, default to showing the GitHub link (safe
fallback).

## Data flow

```
App launch
  → settings.load()
  → updater.startAutoCheck()  (if auto_update_check is true)
  → checkForUpdate()
     → check() from @tauri-apps/plugin-updater
     → GET latest.json from GitHub Releases
     → compare version (semver)
     → newer? state = 'available', banner appears
     → schedule next check in 12h

User clicks "Download & Install"
  → downloadAndInstall()
  → plugin downloads the artifact, verifies signature
  → installs (platform-specific)
  → state = 'installed'
  → "Restart now?" → relaunch

User toggles auto_update_check OFF in Settings
  → updater.stopAutoCheck()
  → interval cleared, no more automatic checks
  → manual "Check now" button still works
```

## Files touched

| File | Change |
|---|---|
| `crates/core/src/types/settings.rs` | Add `auto_update_check: bool` field (`#[serde(default = "default_true")]`) |
| `src/lib/types/index.ts` | Add `auto_update_check: boolean` to `AppConfig` |
| `src/lib/stores/settings.svelte.ts` | Add `auto_update_check: true` to `defaults` |
| `src-tauri/Cargo.toml` | Add `tauri-plugin-updater = "2"` |
| `package.json` | Add `@tauri-apps/plugin-updater` |
| `src-tauri/src/lib.rs` | Register the updater plugin |
| `src-tauri/tauri.conf.json` | Add `"updater"` target, `createUpdaterArtifacts`, `plugins.updater` config (pubkey + endpoint) |
| `.github/workflows/release.yml` | Pass `TAURI_SIGNING_PRIVATE_KEY` + password env vars |
| `AGENTS.md` | Add GitHub-Releases update-check exception to the phone-home rule |
| `src/lib/stores/updater.svelte.ts` **(new)** | Reactive store: check, download+install, auto-check interval, consent gate |
| `src/lib/components/UpdateBanner.svelte` **(new)** | The non-intrusive banner with progress + restart prompt |
| `src/lib/components/settings/About.svelte` **(new)** | Version display, auto-update toggle, manual check button, last-checked |
| `src/lib/components/onboarding/StepUpdates.svelte` **(new)** | Update-consent step (auto vs manual) |
| `src/lib/components/OnboardingWizard.svelte` | Insert Updates step between Welcome and Mode; update stepLabels |
| `src/lib/components/SettingsContent.svelte` | Add "About" nav item |
| `src/App.svelte` | Render `<UpdateBanner>` at the top of the app shell; call `updater.startAutoCheck()` after settings.load() |

## Testing

- **Type-check** (`npm run check`): new fields + components compile.
- **vitest** (`npx vitest run`): existing tests unaffected.
- **Rust** (`cargo test --workspace --lib` + `cargo build -p rust-medical-assistant`):
  the plugin dependency compiles + registers cleanly.
- **Manual (update flow):** push a tag → CI produces signed artifacts +
  `latest.json` → on next app launch, the banner appears → click Download &
  Install → verify signature → install → restart → new version running.
- **Manual (consent):** toggle auto_update_check off in Settings → no banner on
  next launch → toggle on → banner appears on next check.
- **Manual (onboarding):** fresh install → walk through wizard → Updates step
  appears between Welcome and Mode → choose manual → no auto-check after
  finishing.

## Constraints honored

- **No PHI in logs.** The updater store logs only version numbers and state
  transitions ("update check: v0.18.5 → v0.19.0 available"). Never patient data.
- **No new remote endpoints beyond GitHub Releases.** The only outbound call is
  the anonymous GET for `latest.json` + the update binary from GitHub.
- **Local-only AI.** The updater has no interaction with AI/STT providers.

## Open questions resolved

- Host: **GitHub Releases** (anonymous GET, no CDN).
- UX: **auto-check on launch + 12h + banner + Settings button**.
- Signing: **Tauri updater key pair** (one-time `tauri signer generate`).
- Consent: **onboarding step + Settings toggle**, default auto-check ON.
- New field: **`auto_update_check: bool`** with `default_true`.
- Linux deb/rpm: **banner shows, but "Download from GitHub" link instead of
  in-place install** (AppImage gets full in-place).
- Windows signing: **updater signature verifies the download; the installer
  itself remains unsigned** (no Windows code-sign cert today). The updater
  still works — it verifies the `.sig` before running the installer.
