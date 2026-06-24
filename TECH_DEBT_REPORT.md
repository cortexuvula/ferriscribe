# FerriScribe — Technical Debt Report

**Generated:** 2026-06-24 (post v0.20.6)
**Method:** Four parallel static audits across Rust crates, Tauri shell, Svelte frontend, and build/CI/deps. Deduplicated against the June 16 `CODE_REVIEW_REPORT.md` (items already shipped as FERRI-3 hardening + FERRI-11 bug-audit are excluded).

## Severity summary

| Severity | Count | Theme |
|---|---|---|
| 🔴 Critical | 1 | Store-lifecycle leak (frontend) |
| 🟠 High | 11 | Crashes, PHI gap, blocking I/O, CI blind spots |
| 🟡 Medium | 16 | Error typing, dead code, dup logic, dep skew |
| 🟢 Low | 15 | Comment staleness, dead components, polish |
| **Total** | **43** | |

**Overall health:** The codebase is in good shape. Zero TODO/FIXME/HACK markers anywhere, exemplary event-listener hygiene in components, disciplined `spawn_blocking` for most DB I/O, bounded channels throughout, no committed secrets. The debt is concentrated in three areas: (1) the frontend store lifecycle layer, (2) blocking sync work in a handful of Tauri commands, (3) CI/test/tooling gaps that let regressions slip.

---

## 🔴 CRITICAL

### C1. `settings.subscribe()` leaks a root effect scope per subscriber
**File:** `src/lib/stores/settings.svelte.ts:72-79`
**What:** `subscribe()` returns `$effect.root(() => { $effect(() => { cb(this.state); }) })`. This is only safe if the caller invokes the teardown inside a reactive context. `endpointHealth.svelte.ts:168` calls it from `startPolling()` (module code, not a component effect), so the root scope leaks if the teardown is ever missed. Any future caller that forgets to call the returned function leaks a root scope forever.
**Fix:** Replace the `$effect.root` shim with an explicit `Set<cb>` subscriber pattern (call them on each `this.state =` assignment). The sibling `endpointHealth.svelte.ts:204-222` already does this correctly — copy that pattern.

---

## 🟠 HIGH

### H1. `panic!` on any non-recovery init failure
**File:** `src-tauri/src/lib.rs:176`
**What:** `panic!("Failed to initialize application state: {e}")` on `InitError::Other`. A DB disk error, file-lock, or migration failure crashes the app with no UI and no recovery path. Under `panic = "abort"` (release profile) this is a hard exit.
**Fix:** Surface the error to a managed fatal-error state and render a dialog (mirror the existing `DatabaseRecoveryDialog` pattern).

### H2. PHI risk: `frontend_log` writes arbitrary frontend text to the on-disk log
**File:** `src-tauri/src/commands/logging.rs:94`
**What:** `frontend_log(level, message, context)` forwards frontend-supplied `message`/`context` verbatim into `tracing::*`. Length caps bound volume (1KB/2KB) but not content. A frontend bug that does `invoke('frontend_log', { message: transcript })` lands patient text in the rotating log file — violating AGENTS.md line 6. The Rust side is otherwise scrupulous about logging only lengths/IDs.
**Fix:** Add a `PhiRedactor` pass on the message before emitting, OR restrict to an allowlist of known-safe categories, OR route frontend logs to a separate non-PHI sink.

### H3. Synchronous export commands block the IPC channel
**Files:** `src-tauri/src/commands/export.rs:11,35,61` (`export_pdf`, `export_docx`, `export_fhir`)
**What:** Sync `#[tauri::command]` handlers that read SQLite + render PDF/DOCX on the IPC thread. Every other `invoke()` stalls until the export finishes (can be seconds for large notes). Every other DB-touching command in this crate already uses `spawn_blocking`.
**Fix:** Convert to `pub async fn` + `tokio::task::spawn_blocking` (the established pattern at `commands/generation/helpers.rs:41`).

### H4. Blocking CPU + disk I/O inside tokio async (`whisper_supervisor`)
**File:** `crates/sharing/src/whisper_supervisor.rs:189-218` (`download_and_verify`, `ensure_binary`)
**What:** `extract_archive` (full zip/tar.gz decompression), `std::fs::set_permissions`, `std::fs::write` all run on a tokio worker thread without `spawn_blocking`. Stalls the runtime for the duration of the binary extract on every supervisor (re)start.
**Fix:** Wrap the extract + chmod + write in `spawn_blocking`. The STT crate already does this correctly for inference (`local_provider.rs:113`) — follow that model.

### H5. Read locks held across `.await` (network calls) in `start_sharing_inner`
**File:** `src-tauri/src/commands/sharing/lifecycle.rs:154-181,229-247`
**What:** `state.ollama_provider.read().await` guard is held while `p.set_endpoint(...).await` resolves LAN/Tailscale addresses. Blocks `reinit_providers`, `download_model`, and `sharing_status` for the duration. Three sequential locks compound the stall.
**Fix:** Clone the `Arc<Provider>` out of the guard, drop the guard, then `.set_endpoint(...).await` on the clone. The pattern is already used correctly for `ai_providers` in `chat.rs:163-167`.

### H6. Audio store has no teardown on app exit
**File:** `src/lib/stores/audio.svelte.ts` (no `destroy()`)
**What:** `App.svelte` onDestroy calls `pipeline.destroy()` and `updater.stopAutoCheck()` but never tears down the audio store's `timer` (setInterval) or `waveformUnlisten`. On webview reload these can orphan.
**Fix:** Add `destroy()` that calls `clearTimer()` + `waveformUnlisten?.()`, call it in App.svelte onDestroy.

### H7. `createEventDispatcher` (Svelte 4 API) used in two sharing components
**Files:** `src/lib/components/settings/sharing/ServerStatus.svelte:4,8`, `ServerWizard.svelte:3,5`
**What:** Svelte 4 API mixed into a runes-mode app. Deprecated guidance; produces dev warnings. Every other component uses `$props` callbacks.
**Fix:** Replace with callback props (`let { onstopped }: Props = $props()`).

### H8. CI runs no frontend tests, no typecheck, no clippy, no fmt check
**File:** `.github/workflows/ci.yml`
**What:** CI runs `cargo test --workspace --lib` + `cargo test -p medical-sharing` but **never** runs `npx vitest run`, `npm run check`, `cargo clippy`, or `cargo fmt --check`. The entire Svelte frontend test suite (257 tests) and svelte-check have zero CI coverage. Also no `timeout-minutes` on any job (a hung test can burn the 6h runner cap).
**Fix:** Add a `frontend` job (`npm ci`, `npm run check`, `npx vitest run`); add `cargo clippy --workspace -- -D warnings`; add `timeout-minutes: 30`.

### H9. Version sync is purely manual with no guard
**Files:** `src-tauri/Cargo.toml`, `package.json`, `src-tauri/tauri.conf.json`
**What:** AGENTS.md documents "bump all three together" as a manual ritual. No script, no pre-commit hook, no CI check that the three versions match. A missed file silently ships a wrong-versioned installer and can break the auto-updater (it consumes `latest.json` from GitHub Releases).
**Fix:** Add `scripts/check-version-sync.*` (asserts the three match, exits non-zero on mismatch); call it as the first CI step.

### H10. Dead Tauri commands wired into `invoke_handler` but never called
**Files:** `commands/rag.rs:32,81,136` (`ingest_document`, `search_rag`, `rag_stats`), `commands/settings.rs:181` (`list_api_keys`)
**What:** The entire RAG command surface is wired up (ingestion, vector store, BM25, fusion) but unreachable from the UI. Cross-checked against all `invoke(...)` calls in `src/` — zero callers.
**Fix:** Either wire the frontend to use them, or remove from `generate_handler!` and delete the fns.

### H11. Dead public modules across crates (zero callers)
**Files:** `crates/security/src/rate_limiter.rs`, `input_sanitizer.rs`, `audit_logger.rs`; `crates/rag/src/query_expander.rs`; `crates/translation/src/canned_responses.rs`
**What:** Each exports a `pub` API with full impls + tests, but nothing in the workspace or `src-tauri` references them. They bloat the build and mislead readers into thinking they're load-bearing.
**Fix:** Wire into `src-tauri` if intended for use, gate behind `#[cfg(test)]`, or delete.

---

## 🟡 MEDIUM

### Error typing
- **M1.** `DbError` has no `Io` variant → 17 I/O failures in `crates/db/src/encryption.rs` get stringified via `DbError::Other(format!(...))`. Add `Io(#[from] std::io::Error)`.
- **M2.** `Result<_, String>` boundaries in `corpus_export/mod.rs:41,167` and `sharing_vocab_api.rs` (13 sites) lose typed error info. Convert to `AppResult`.
- **M3.** `SharingError` variants (`TokenStore(String)`, `Pairing(String)`, `AuthProxy(String)`) are opaque strings; frontend can't distinguish corruption vs cancel.

### Dead code / unused deps
- **M4.** Unused Cargo deps: `hyper`+`hyper-util` (sharing), `eyre` (stt-providers), `hmac` (security — only needed transitively).
- **M5.** Dead frontend components: `TabBar.svelte`, `TextEditor.svelte`, `SettingsPage.svelte` — 0 imports.

### Duplicated logic
- **M6.** Markdown-stripping: `processing/document_generator.rs:235` (`strip_markdown`) and `soap_generator/postprocess.rs:59` (`clean_text`) implement overlapping regex sets. Comment in source acknowledges this.
- **M7.** `cosine_similarity` duplicated: `rag/mmr.rs:7` (`&[f32]`) and `stt-providers/diarization.rs:527` (`&Array1`). Forward one to the other.
- **M8.** Patient-context state (4 fields + hydration) copy-pasted across `RecordTab.svelte` and `GenerateTab.svelte`. Extract a `usePatientContext` helper.
- **M9.** Escape-key dialog handler duplicated in 3 dialogs (`ContextTemplateDialog`, `DictionaryDialog`, `VocabularyDialog`). Extract an action.
- **M10.** Remote-vs-local + spawn_blocking scaffolding duplicated in `commands/vocabulary.rs` and `context_templates.rs` (~150 lines). Extract a helper.

### Dependencies / config
- **M11.** `workspace.package.version = "0.1.0"` while app is `0.20.6` — all 13 library crates record as `0.1.0` in `Cargo.lock`. Set workspace version to match or stop documenting it as app version.
- **M12.** Significant Cargo version skew: `rand` (0.8/0.9/0.10), `thiserror` (1/2), `base64` (0.21/0.22), `reqwest` (0.12/0.13), `zip` (0.6/2.4/4.6), `ndarray` (0.15/0.16/0.17). Bloats build + binary.
- **M13.** `rusqlite = "0.32"` is 5 minor versions behind (latest 0.37); blocks SQLCipher/libsqlite3 CVE patches. `r2d2_sqlite 0.25` locks to the 0.32 track.
- **M14.** `ort = "2.0.0-rc.12"` — production medical app shipping on an ONNX Runtime release candidate. Move to stable `2.x`.
- **M15.** `vite.config.ts` / `vitest.config.ts` duplicate the `dictionary-en` plugin + path constants (AGENTS.md acknowledges this). Extract to shared file.
- **M16.** README staleness: says "12 crates" (reality 13), lists "SvelteKit" (reality Svelte 5 + Vite), omits that Windows is CI-excluded and macOS is Apple-Silicon-only.

### Frontend patterns
- **M17.** `SettingsContent.svelte:17` — `$effect` calls `settingsNav.clear()` (writes state it reads). Wrap in `untrack`.
- **M18.** `GenerateTab.svelte:22` — `$effect` does network fetch (`letterAudiences.list()`) with no reactive guard. Move to `onMount`.

---

## 🟢 LOW

- **L1.** Stale comment `transcription/inner.rs:432` claims transcript is "logged via tracing::error!" — actually written to a recovery file. Rewrite comment.
- **L2.** `delete_recording` (`recordings.rs:71`) uses `.ok()` — DB error silently treated as "not found", orphans WAV file. Distinguish `Ok(None)` from `Err`.
- **L3.** `mark_recording_failed_db_only` (`helpers.rs:207`) swallows the failure-marker error → recording stuck in Processing forever, no log. Add `tracing::warn!`.
- **L4.** Sync keychain access in `get/set/list_api_keys` blocks IPC thread. Make async + `spawn_blocking`.
- **L5.** `import_audio_file` (`recordings.rs:188`) uses `.ok()` on WavReader → malformed import silently gets `duration = None`. Surface the error.
- **L6.** Production `.unwrap()` on regex literals (`processing/postprocess.rs`, `document_generator.rs`, `agents/vitals_extractor.rs`, `security/phi_redactor.rs:242`) — safe today (compile-time constants) but under `panic = "abort"` any malformed literal kills the app. Use `lazy_regex` or centralize construction.
- **L7.** `console.log` debug leftovers: `StepProvider.svelte:25,28`, `GenerateTab.svelte:111`. Replace with `log.debug` or remove.
- **L8.** `spellchecker.ts:115` `as unknown as` cast hides a real type relationship. Make impl implement the extended interface.
- **L9.** `EditorTab.svelte:42` casts `metadata.transcript_segments` without validation. Add a type guard.
- **L10.** `letterAudiences` store mutates `$state` array in place (`audiences[i] = ...`) — inconsistent with sibling stores that reassign. Inconsistent deep-mutation style.
- **L11.** `Prompts.svelte:104` setTimeout handle dropped — fires on unmounted component. Store + clearTimeout.
- **L12.** `toasts` store setTimeout handle dropped (idempotent dismiss, low impact). Store handles in a Map.
- **L13.** No ESLint/Prettier/frontend lint config (AGENTS.md admits this). Add `eslint` + `eslint-plugin-svelte`.
- **L14.** Large components (>400 lines): `General.svelte` (565), `ClientPair.svelte` (460), `App.svelte` (443), `Models.svelte` (435), `LetterAudiences.svelte` (425). Split candidates.
- **L15.** Test coverage gaps: stores with stateful/timer logic but no tests — `audio`, `chat`, `settings`, `recordings`, `updater`, `rsvp`. Untested pure utils: `format.ts`, `vocabularyFilter.ts`, `endpointPolicy.ts`.

---

## Recommended fix order

**Quick wins (1-2h each, high signal):**
1. **H9** — version-sync script + CI check (prevents silent updater breakage)
2. **C1** — replace `settings.subscribe` shim (eliminates leak class)
3. **H6** — audio store `destroy()` (2-line change)
4. **H10/H11** — delete dead commands + modules (pure removal, smaller binary)
5. **L7** — remove console.log leftovers (trivial)

**Medium effort (half-day each):**
6. **H8** — CI: add frontend tests + typecheck + clippy + fmt + timeouts (catches the regression class that hit v0.20.5→v0.20.6)
7. **H3** — convert export commands to async + spawn_blocking
8. **H5** — fix lock-held-across-await in sharing lifecycle
9. **H2** — add PHI scrubber to frontend_log
10. **M13/M14** — bump rusqlite + ort to stable (security + supply chain)

**Larger investment (multi-day batches):**
11. **H1** — fatal-error dialog instead of panic on init failure
12. **H4** — wrap whisper_supervisor I/O in spawn_blocking
13. **M1/M2/M3** — typed error migration (Io variant, AppResult, SharingError structs)
14. **M6-M10** — consolidate duplicated logic (markdown, cosine, patient-context, scaffolding)
15. **M11/M12** — workspace version alignment + dependency dedup

---

## Verified clean (no action)

- No committed secrets, keystores, or `.env` files.
- No PHI in any Rust `tracing::*` call (verified across all crates).
- Zero TODO/FIXME/HACK/XXX markers anywhere in the codebase.
- Zero `@ts-ignore`/`@ts-nocheck` in the frontend.
- Event-listener hygiene in components is exemplary (every `addEventListener` has matching `removeEventListener` in `onDestroy`).
- No `Mutex<RwLock<...>>` nesting; no `std::sync::Mutex` held across `.await`.
- All Tauri `listen()` calls have matching unlisten (including race-guarded ones).
- `SharingService::stop()` correctly drains/aborts all JoinHandles; `WhisperSupervisor::stop()` kills the child.
- All channels bounded (`mpsc::sync_channel(32)` / `mpsc::channel(32/64)` throughout).
- Cargo.lock + package-lock.json both committed (correct for an app).
- Tauri updater pubkey is intentionally public.
- `tts-providers` has only 5 tests but is not zero.
