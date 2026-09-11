# Settings declutter implementation plan

> For Hermes: use bounded subagent-driven implementation and independent review.

Goal: implement the approved Configure surface: grouped sidebar, focused pages, aligned controls, progressive disclosure, without changing settings persistence or clinical prompts.
Architecture: retain Svelte 5 component/store/API contracts and existing section IDs; add vocabulary and advanced destinations. Existing training-corpus links remain valid. Settings-only styles reuse app tokens, preferring text-secondary over low-contrast text-muted.
Tech stack: Svelte 5, TypeScript, Vite, Vitest/jsdom. No backend changes.
Base: 4a0d88d. Isolated branch feat/settings-declutter.

## 1. Synthetic design renders BEFORE production edits
Create docs/settings-declutter/proposal.html (labelled simulation), General/Models/Prompts in light/dark using app tokens. Render six screenshots and measure overflow at narrow/medium/wide sizes. No app data, backend, credentials, microphone or external services.

## 2. Shell and composition (exclusive shell implementer)
Files: SettingsContent.svelte, settingsNav.svelte.ts, SettingsDialog.svelte, Modal.svelte (only opt-in settings sizing if required), settings/General.svelte, sections/GeneralBasics.svelte, sections/DataManagement.svelte, sections/AdvancedSettings.svelte; new settings/GeneralPreferences.svelte, StorageSettings.svelte, VocabularySettings.svelte, Advanced.svelte, settings.css as needed. New extraction files allowed.
- Write regression tests then verify failures for grouped navigation, all legacy section IDs, external navigation with unsaved prompt guard, decline/accept discard, same-section no-op, reopen on last section.
- Group Everyday (General, Recording & Transcription, AI Models), Clinical content (Prompts & Specialties, Vocabulary & Templates, Letter Audiences), System (Storage & Backup, Sharing, Advanced, About & Updates).
- Retain training-corpus route accessible via Advanced; do not remove any controls.
- General = theme/autosave/interval/sound. Move language into Audio via standalone TranscriptionLanguage component (Audio implementer owns insertion). Move storage/security/retention into Storage & Backup. Move vocabulary/dictionary/context templates into Vocabulary & Templates. Screenshot OCR, public-endpoint exception, capture-for-training, setup wizard into Advanced. Preserve warnings visible; public-endpoint warning at shell level. Preserve validations and all action handlers.
- Use 200-220px nav, 24-32px content padding, 44px minimum nav/action targets; stacked controls below container width 600px. One content scroll region with independently scrollable nav. Settings-only shared styles, no whole-app palette changes.

## 3. Models, Audio, Prompts (exclusive pane implementer)
Files: settings/Models.svelte, Audio.svelte, Prompts.svelte, respective tests, new sections/TranscriptionLanguage.svelte.
- Add failing component tests for disclosure, retained error visibility, and prompt document selector discard cancellation/acceptance.
- Models: selected provider and main model first. Feature overrides/temperature in Model options with nondefault summary; provider configuration collapsible (active provider first; retain access to all saved provider configurations). Errors stay visible.
- Audio: microphone/language/transcription mode/active engine first; speaker models/max speakers/sample rate in clearly labelled advanced disclosure with summary. Preserve download status/errors outside collapsed disclosure or auto-expand when necessary.
- Prompts: labelled document-type select replaces nested sidebar. Reuse handlePromptSelect and restore selector on cancelled discard. Pack/source/custom-conflict/error states stay visible. Token and long reference material under Help. No clinical prompt text changes. Preserve all Save/Reset/discard/loading/race guards.
- Reuse app palette, shared settings styles. Preserve API requests and contracts.

## 4. Verification and review (UI consultant)
Run node_modules/.bin/vitest run, npm run check, npm run lint, npm run build. Inspect actual production SettingsContent via isolated synthetic browser harness with Tauri API calls mocked; no App.svelte launch and no real settings/records access. Check General/Models/Prompts light/dark, narrow/medium/wide, keyboard navigation and overlays; compare with proposal. Persist screenshot matrix and measurements. Review all moved-control inventory and route callsites. Handoff exact branch and commit to Codie; do not merge/release before review. Report limitations explicitly.
