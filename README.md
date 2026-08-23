# FerriScribe

A privacy-first medical transcription desktop application built with Rust and Svelte. Record doctor-patient encounters, transcribe them locally with speaker diarization, generate SOAP notes and clinical documents, draft letters from scanned paper documents, OCR supporting documents, sync across machines, and export to PDF, DOCX, or FHIR.

## Features

### Transcription
- **Local Speech-to-Text** — Whisper (via whisper-rs / whisper.cpp) with Metal GPU acceleration on macOS. Runs on-device with beam-search decoding and temperature fallback for accuracy on long recordings; no audio leaves your machine in this mode.
- **Remote Whisper (optional)** — Switch STT Mode to Remote to offload transcription to any OpenAI-compatible Whisper server (e.g. `whisper.cpp server`, `faster-whisper-server`, LocalAI) running on another machine over LAN or Tailscale. See [Running Across Machines](#running-across-machines-lan--tailscale).
- **Speaker Diarization** — Pyannote + WeSpeaker (ONNX) pipeline labels who is speaking (e.g. Doctor vs. Patient). Runs locally in both STT modes.
- **Custom Vocabulary** — User-defined find/replace rules applied after STT, with word-boundary matching, priority ordering, and import/export compatible with the Python Medical-Assistant `vocabulary.json` format.
- **Condition Chips** — One-tap quick-add of common conditions (e.g. hypertension, T2DM) into the patient context. Chips are frequency-ranked by usage, fully editable, and sync across machines with the same last-write-wins semantics as other content.
- **User Dictionary** — Add words (drug names, local surnames, abbreviations) to the editor's spellchecker; the dictionary syncs across machines so corrections learned on one laptop follow the clinician everywhere.

### Documents & Review
- **SOAP Note Generation** — AI-powered Subjective / Objective / Assessment / Plan notes from transcripts.
- **ICD-9 Billing Codes** — BC MSP-accepted ICD-9 codes (7,122 codes) with intelligent candidate selection. The selector scores codes against the transcript using a keyword-overlap inverted index enriched with medical synonyms, plus a specificity adjustment favoring precise codes over generic ones (e.g. cervicalgia over backache). Off-list codes are flagged with amber chips in the UI.
- **Referral, Clinical Letter, and Synopsis Generation** — Templated AI generation with per-document custom prompts.
- **Letter Audiences** — Generate letters tailored to different recipients:
  - **Patient** — Plain language, empathetic
  - **Insurance Company** — Medical necessity language, ICD/CPT codes
  - **Tax Authority** — Expense justification, timeline
  - **Specialist/Consultant** — Clinical detail, peer tone
  - **Employer/School** — Accommodations, HIPAA-minimal
  - **Legal/Court** — Formal opinion, timeline

  Create custom audiences in **Settings → Letter Audiences** with your own system prompts and user templates.
- **Letter Writer** — A standalone Workflow tab for drafting letters from paper documents: drop in any document (scanned PDF, photo, export), OCR it, fill in recipient / type / tone / RE line plus freeform instructions, and generate a polished letter. Anti-fabrication rules keep the letter grounded in the source document — anything missing surfaces as a `[NOT IN SOURCE: …]` placeholder instead of invented content. Not tied to a recording; the drafted letter stays on screen for copying (state persists across tab switches).
- **Context Templates** — Pre-built visit types (e.g. Follow-up, New Patient) with custom instructions layered on top of the base prompt; import/export as JSON.
- **RSVP Speed Reader** — Rapid-serial-visual-presentation review mode for SOAP notes and transcripts — chunk-size, WPM, and per-section filters configurable in Settings.
- **Inline Preview** — Generated documents display in a collapsible preview directly in the Generate tab, no need to switch tabs.

### Document OCR
- **Multi-Format OCR** — Drop documents into the context panel to extract text via a local vision model (e.g. glm-ocr). Supported formats:
  - **Text** — `.txt`, `.md`, `.csv` (read directly, no model needed)
  - **Images** — `.png`, `.jpg`, `.jpeg`, `.bmp`, `.webp`, `.tiff` (sent to vision model)
  - **PDF** — `.pdf` (embedded text extracted via pdf-extract; scanned/image-only PDFs are rendered page-by-page with pdfium and OCR'd through the vision model — up to 50 pages at 150 DPI)
  - **Office** — `.docx` (text from Word XML), `.xlsx` (cell data from all sheets)
- **Scanned-PDF Renderer** — Scanned-PDF OCR is powered by [pdfium](https://github.com/bblanchon/pdfium-binaries) (Chrome's PDF engine, BSD-licensed). The library is downloaded automatically into the app data directory the first time you OCR a scanned PDF — nothing to install, and it runs entirely locally (no network after the one-time fetch). The download is pinned to a known release and verified against a SHA-256 digest before the library is loaded.
- **OCR Model Setting** — Configure a dedicated vision model for OCR separately from the text generation model in **Settings → Models**.
- **Integration** — OCR'd text is combined with notes and structured patient context (medications, allergies, conditions) and threaded into all generation types (SOAP, referral, letter, peer discussion). Available in the Record, Generate, and Letter Writer tabs.

### Content Sync
- **Bidirectional Sync** — Sync transcripts, SOAP notes, letters, referrals, peer discussions, and audio between machines over Tailscale. Per-field last-write-wins merge with separate push/pull cursors; each field carries its own timestamp and origin machine.
- **Deletions & Restores Propagate** — Trashing a recording on one machine tombstones it everywhere; restoring it (newer write wins) revives it everywhere. The office server permanently purges trash after 30 days and keeps an id-only purge ledger, so a machine that was offline during the deletion can never resurrect purged content. Condition chips and the user dictionary sync with the same tombstone-aware merge, and a deleted chip stays deleted until explicitly re-added.
- **Background Sync** — Automatic sync every 5 minutes when enabled, with a manual "Sync Now" button and last-synced timestamp.
- **Cloud Badge** — Remote-synced recordings display a cloud badge for easy identification.
- **Real-time Updates** — SSE-based change notifications refresh the recordings list instantly when new content arrives.

### AI providers
- **Local and LAN-accessible only** — Ollama and LM Studio, each configurable with a remote host/port so you can run the heavy model on a separate machine over LAN or Tailscale.
- **Thinking Control** — Reasoning models (Qwen3 & co.) can spend minutes in a "thinking" phase before writing a note. **Settings → Models** has a per-provider **Disable thinking** toggle. Ollama skips reasoning via `reasoning_effort: "none"`; LM Studio ignores API thinking parameters, so FerriScribe injects a pre-closed think-block prefill instead — for a fix that covers every app at once, edit the model's prompt template in LM Studio (Model Settings → Prompt Template, add `{%- set enable_thinking = false %}`).
- **Retrieval-Augmented Generation (RAG)** — Ingest clinical documents; embeddings served by the same Ollama instance, with BM25 + vector + graph retrieval at query time.
- **Agentic Workflows** — Multi-step orchestrator with tool use (RAG search, note generation) for chat sessions.

### Data
- **Recording Management** — Record, import, search, tag, and organize audio. SQLite-backed with soft-delete and undo (8-second window), a 30-day trash (with a configurable retention policy under **Settings → Data Management** for auto-trashing old recordings), and permanent purge on the office server with a resurrection-proof ledger.
- **Export** — PDF, DOCX, and FHIR R4 (healthcare interoperability standard).
- **Encrypted Storage** — Audio recordings encrypted at rest with AES-256-GCM; fetched/decrypted audio is re-encrypted in memory before touching disk. Database uses SQLCipher (AES-256) via the OS keychain.
- **Secure Key Storage** — API keys encrypted at rest with AES-256-GCM; the master cipher key is derived via PBKDF2-HMAC-SHA256 (600 000 iterations) from an optional `MEDICAL_ASSISTANT_MASTER_KEY` env var or a per-machine identifier.

### Platform
- **Cross-Platform** — macOS (Apple Silicon; Metal-accelerated STT), Windows, and Linux. Note: Windows builds are produced by CI but excluded from the automated test matrix (cpal audio-device enumeration crashes on headless runners); macOS installers are Apple-Silicon-only.

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Frontend | Svelte 5 (runes mode), TypeScript, Vite |
| Backend | Rust (edition 2024), Tauri v2 |
| STT | whisper-rs (whisper.cpp), ort (ONNX Runtime), knf-rs, rubato |
| Database | SQLite with SQLCipher (AES-256 encryption) |
| AI | Ollama, LM Studio (OpenAI-compatible wire protocol) |
| OCR | Vision models via Ollama/LM Studio, pdfium (scanned-PDF rendering), pdf-extract, calamine, quick-xml |
| Export | PDF (printpdf), DOCX (docx-rs), FHIR R4 |
| Security | AES-256-GCM + PBKDF2 (aes-gcm + pbkdf2 crates), SQLCipher |

## Architecture

FerriScribe is organized as a Cargo workspace with 13 crates:

```
crates/
  core/           — shared types, traits, error handling, ICD-9 codes
  db/             — SQLite database, settings, recordings, content sync
  security/       — AES-256-GCM file/key encryption
  audio/          — microphone capture (cpal)
  ai-providers/   — Ollama + LM Studio (OpenAI-compat wire, vision support)
  stt-providers/  — whisper transcription + pyannote diarization
  tts-providers/  — text-to-speech
  agents/         — agentic orchestrator with tool registry
  rag/            — vector store, BM25, graph search, ingestion
  processing/     — transcription pipeline, SOAP generation, OCR, ICD-9 selector
  export/         — PDF, DOCX, FHIR export
  translation/    — text translation
  sharing/        — office-server sharing, mDNS, Tailscale, auth proxy, whisper supervisor
src-tauri/        — Tauri app shell, commands, state management
src/              — Svelte 5 frontend
```

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) 1.85+ (required by `edition = "2024"`)
- [Node.js](https://nodejs.org/) 20+
- [CMake](https://cmake.org/) and Clang (for whisper.cpp, ONNX Runtime, and libheif)
- macOS: Xcode Command Line Tools

### Build & Run

```bash
npm install
npm run tauri dev
```

Release builds are produced by the GitHub Actions workflow on tag pushes matching `v*`. Artifacts are attached to the release page.

### Development

```bash
# Backend tests (lib only — integration tests are crate-scoped)
cargo test --workspace --lib

# Sharing integration tests (needs FERRISCRIBE_MDNS_TEST=1 only on Linux)
cargo test -p medical-sharing

# DB integration tests (condition_chips_sync, content_sync, deletion_model,
# encryption, recording_sync_merge, retention — NOT covered by --lib above)
cargo test -p medical-db

# Frontend tests
npx vitest run

# Type-check (svelte-check)
npm run check

# Rust formatting + lints — both gates enforced by CI
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Audio device tests are gated behind `FERRISCRIBE_AUDIO_TEST=1` (cpal enumeration can block on machines with virtual audio hardware); without the env var they skip.

### Model Setup

On first launch, go to **Settings > Audio / STT** and download:

1. **Whisper model** — Choose a size (base ~148 MB to large-v3-turbo ~1.6 GB). Larger models are more accurate. Skip this step if you'll only use Remote STT.
2. **Diarization models** — required in BOTH STT modes, since diarization always runs locally:
   - Pyannote segmentation 3.0 (~6 MB)
   - WeSpeaker CAM++ embedding (~28 MB)

For **OCR** (optional), go to **Settings > Models** and set an OCR / Vision Model (e.g. `glm-ocr`). If not set, the text generation model is used for OCR.

Models are downloaded from HuggingFace / GitHub and stored under the app's data directory (see [Where Your Data Lives](#where-your-data-lives)).

## Usage

1. **Record** — Start a new recording or import an existing audio file.
2. **Add Context** — Enter medications, allergies, conditions, and notes. Drop supporting documents (PDFs, images, Word/Excel files) for OCR extraction.
3. **Transcribe** — Local Whisper runs on-device by default; Custom Vocabulary corrections are applied automatically after STT.
4. **Generate** — Produce a SOAP note, referral, clinical letter, or synopsis from the transcript, optionally guided by a Context Template. Supporting documents and patient context are automatically included.
5. **Review** — Preview inline in the Generate tab, edit in the Editor tab, or use the RSVP speed reader.
6. **Export** — Save as PDF, DOCX, or FHIR R4.
7. **Chat** — Ask follow-up questions grounded in the recording and any ingested RAG documents.

**No recording needed?** Use the **Letter Writer** tab (Workflow section) to OCR a paper document and draft a letter directly from it.

## Running Across Machines (LAN / Tailscale)

FerriScribe can run AI on a powerful office computer and connect from
laptops over the LAN or Tailscale. No terminals, no environment
variables.

### On the office server

1. Install FerriScribe.
2. Open **Settings → Sharing** → **This machine is the office server** →
   **Start sharing**. The wizard installs a persistent Ollama service,
   downloads whisper.cpp (Windows only — see note below), and shows a pairing
   screen with a QR code and a 6-digit code.
3. If LM Studio is installed, open it and click **Start Server** in its
   Local Server tab. (FerriScribe doesn't manage LM Studio's toggle.)

> **macOS / Linux whisper-server:** whisper.cpp does not currently ship prebuilt
> `whisper-server` binaries for macOS or Linux. Office-server admins on those
> platforms must build it from source
> (`cmake -B build && cmake --build build --target whisper-server -j`)
> and place the resulting binary in the FerriScribe app-data `bin/` directory
> before starting sharing. See https://github.com/ggml-org/whisper.cpp#server
> for full build instructions. Windows office servers download the binary
> automatically.

### On each clinician's laptop

1. Install FerriScribe.
2. Open **Settings → Sharing** → **This machine connects to an office
   server**. Servers found on the local network appear in the list —
   click **Connect** and enter the 6-digit code from the office server.
3. Off-network or remote? Scan the QR or paste the
   `ferriscribe://pair?...` URL the office server displayed.

The model pickers under **Settings → Models** then list whatever models
the office server has installed. No models are downloaded on the
laptop.

### Content Sync

Enable **Sync patient content via Tailscale** in **Settings → Sharing** to
bidirectionally sync transcripts, SOAP notes, letters, referrals, peer
discussions, and audio between machines, along with condition chips and
the user dictionary (deletions propagate; restoring a trashed recording
on any machine revives it everywhere). Background sync runs every 5
minutes. Use the **Sync Now** button for manual sync.

### Security

Per-client tokens are issued during pairing and stored in the laptop's
OS keychain. Revoke a lost / stolen laptop's access from the office
server's **Connected clients** panel.

Pairing traffic is plain HTTP. On a clinic LAN with guest Wi-Fi or BYOD
risk, prefer Tailscale (which transparently encrypts with WireGuard).

### What stays local on each laptop

- Audio capture and waveform display
- Speaker diarization (pyannote + WeSpeaker)
- SQLite database (SQLCipher encrypted), vocabulary rules, RAG vector store
- The SOAP / referral / letter / synopsis editors

Only Whisper inference and Ollama chat / embedding calls cross the wire.

## Where Your Data Lives

Recordings, transcripts, settings, downloaded models, and the encrypted keystore all live under the OS-specific app data directory:

| OS | Path |
|----|------|
| macOS | `~/Library/Application Support/rust-medical-assistant/` |
| Linux | `~/.local/share/rust-medical-assistant/` |
| Windows | `%APPDATA%\rust-medical-assistant\` |

Inside you'll find `medical.db` (SQLCipher-encrypted SQLite), `config/keys.json` (encrypted API keys), `models/whisper/*.bin`, `models/pyannote/*.onnx`, `pdfium/` (the scanned-PDF renderer, fetched on first use), and the recordings themselves (AES-256-GCM encrypted `.enc` files) in whatever path you configured under **Settings → General**. Delete the directory to fully remove all user data.

### Optional: stronger master key

By default the keystore's master cipher key is derived from the machine identifier. To bind it to a secret you control — for example if multiple users share the same machine — set `MEDICAL_ASSISTANT_MASTER_KEY` in the environment FerriScribe is launched from; PBKDF2-HMAC-SHA256 will derive the cipher key from that value instead. Losing the env var value makes the keystore unrecoverable.

## Off-machine backup (encrypted, append-only)

Disk #1 dying is the one failure that actually happens, and until FerriScribe's
backup existed it meant total clinical loss — DB, encrypted recordings, and keys
all lived on one machine. The `ferriscribe-backup` tool fixes that with a
pull-based, append-only design:

- **Snapshots** are consistent (`VACUUM INTO` while the app may be open), fully
  encrypted (DB and recordings are already ciphertext at rest; the DB key itself
  travels inside every snapshot, wrapped under a separate *backup wrapping key*),
  authenticated with an HMAC over every payload byte, and carry **no PHI in any
  filename or receipt** — only counts, sizes, and opaque ids.
- **Key escrow**: the wrapping key's off-machine copies are two *independently
  sufficient* artifacts — a printable recovery sheet (for the safe) and an offline
  USB file — each self-verifying via an embedded check code (one mistyped hex
  digit fails loudly instead of restoring garbage). Losing the machine's keychain
  loses nothing; the sheet alone restores everything.
- **Append-only target**: the target machine (e.g. `cortex-home` over Tailscale)
  runs `ferriscribe-backup serve`. The token the *source* holds can only append
  new snapshots — there is no route it can reach that deletes or overwrites
  anything, so ransomware on the clinical machine cannot erase its own history.
  Pruning (keep newest N) requires the target-side admin token that never leaves
  the target, and growth is capped (disk-measured) so a compromised source can't
  fill the target's disk either.
- **Tested restore**: every backup ends with a drill — the job re-pulls the
  snapshot from the target, restores it to a throwaway directory, opens the
  restored SQLCipher DB with the escrow-recovered key, decrypts a sample
  recording, and diffs record counts. Any mismatch fails the job loudly.
  Settings → Backup shows the verdict and turns red on failure or >48h staleness.
- **Incremental (content-addressed)**: recordings are immutable ciphertext, so
  each one is stored on the target exactly once, keyed by the SHA-256 of its
  encrypted bytes. After the initial full push, a nightly backup transfers only
  what is new — typically ~30 MB (fresh DB snapshot + wrapped key) plus any new
  recordings. Local-only snapshots are hardlink-staged: self-contained against
  file loss, but not a substitute for the off-machine copy (they share fate
  with the source disk).

**Storage model (why the target stays bounded):**

- Initial full backup ≈ the size of your recording library, once
  (e.g. ~33 GB for 294 recordings).
- Each daily backup adds one fresh DB snapshot + wrapped key (~30 MB for a
  29 MB database — `VACUUM INTO` re-encrypts with a new salt every run, so
  the DB never dedups) **plus only recordings that are new since the last
  backup**.
- Retention prunes old snapshots (keep newest N); blobs that no retained
  snapshot references are garbage-collected automatically, with the target's
  admin authority only.
- Steady state ≈ recording library + N × ~30 MB. With retention applied the
  total stays well under the cap (default 1 TiB) as long as the recording
  library itself is bounded.

**Where's the tool?** Since v0.53.0 the binary ships *inside* the app bundle —
on an installed Mac it is `/Applications/FerriScribe.app/Contents/MacOS/
ferriscribe-backup`. On the target machine (or for local dev) build it with
`cargo build --release -p medical-backup`. The easiest way to run the bundled
copy: `alias ferriscribe-backup="/Applications/FerriScribe.app/Contents/MacOS/ferriscribe-backup"`.

### Setup (one time)

The guided path is **Settings → Backup** in the app (or the final onboarding
step): a wizard walks through picking a destination, printing the recovery
sheet, choosing a backup time, and running a first backup + restore test.
Destination options, easiest first:

- **USB / external drive, or any folder** (including one that syncs to
  iCloud Drive or Dropbox): pick it with the folder picker; the wizard
  checks it's writable and shows free space. The store layout is
  identical to what `serve` writes, content-addressed so recordings
  transfer once; retention keeps the newest 30 snapshots. A `/Volumes`
  destination also gets a launchd `StartOnMount` trigger — plugging the
  drive in fires a catch-up backup. **Trade-off:** this machine can
  rewrite the folder, so it lacks the append-only ransomware protection
  of the server target — unplug USB drives between backups.
- **Backup server** (advanced, append-only): run `ferriscribe-backup
  serve` on a second machine as below — the strongest model, and the
  wizard's "Test connection" probe validates reachability + token at
  setup time.

The equivalent CLI flow, including the target machine which has no app:

```bash
# 1. On the clinical machine: create the wrapping key + escrow artifacts.
#    (Or use Settings → Backup → Recovery key escrow.)
ferriscribe-backup escrow init --out-dir ~/Desktop
#    PRINT the recovery sheet → safe. Copy the .escrow file → offline USB.
#    Verify each: ferriscribe-backup escrow verify --file <path>

# 2. On the target machine (e.g. cortex-home, over Tailscale):
export FERRISCRIBE_BACKUP_APPEND_TOKEN=<random>
export FERRISCRIBE_BACKUP_ADMIN_TOKEN=<different random>
ferriscribe-backup serve --root /srv/ferriscribe-backups --bind 100.64.0.2:8741
#    (run under launchd/systemd; the admin token never leaves this machine)

# 3. On the clinical machine: schedule the daily 03:30 backup + push + drill.
#    (Or use Settings → Backup → Daily schedule.)
ferriscribe-backup install-schedule --hour 3 --minute 30 \
  --url http://100.64.0.2:8741 --token <the append token>

#    Folder destination instead of a server (USB drive, NAS share, or a
#    cloud-synced folder — no second machine needed):
ferriscribe-backup install-schedule --hour 3 --minute 30 --dest /Volumes/BackupDrive
```

The schedule runs **outside the app** (a launchd LaunchAgent pointing at a stable
copy of the tool in the app-data `bin/` directory), so backups continue even if
FerriScribe is closed, crashed, or trashed. On Linux, point a systemd timer
(`OnCalendar=daily`) at the same `backup-and-push` command. If you change the
recordings storage folder later, re-run install-schedule — the resolved path is
baked into the schedule.

### Recovering a backup (disaster recovery)

First: **deleted one recording by mistake?** Check FerriScribe's in-app trash
first (30-day soft-delete with undo) — that's the fast path, not the backup.

For a dead or replaced machine, you need three things: the **recovery sheet**
(printed, in the safe) or the **USB escrow file** — either alone is enough —
plus the **backup target** running on your network, plus its **append token**
(it lives in the target's `FERRISCRIBE_BACKUP_APPEND_TOKEN` service config).

```bash
# 0. Install FerriScribe from the DMG (this also gives you the tool),
#    then make it convenient:
alias ferriscribe-backup="/Applications/FerriScribe.app/Contents/MacOS/ferriscribe-backup"

# 1. Pull the newest snapshot from the target. This VERIFIES it first —
#    HMAC + per-file hashes against the escrow key — before writing
#    anything; a corrupted or tampered snapshot fails closed.
ferriscribe-backup pull --url http://100.64.0.2:8741 --token <append> \
  --out ~/restored --escrow-file ~/Desktop/ferriscribe-backup-recovery-sheet.txt
#    (For the USB escrow instead: --escrow-file /Volumes/STICK/ferriscribe-backup-key.escrow)
#
#    Folder destination? Skip the pull — restore straight from the drive's
#    store (same verification, assembling + HMAC-checking every blob):
#    ferriscribe-backup restore --store-dir /Volumes/BackupDrive --dest ...

# 2. Restore into the app's data dir. This ALSO installs the snapshot's
#    database key into this machine's keychain, so the app can open
#    everything. Note: ~ must be OUTSIDE the quotes to expand.
ferriscribe-backup restore \
  --snapshot-dir ~/restored/<snap-id> \
  --dest ~/"Library/Application Support/rust-medical-assistant" \
  --escrow-file ~/Desktop/ferriscribe-backup-recovery-sheet.txt

# 3. Prove it before trusting it: re-pull from the target, restore to a
#    scratch dir, open the DB with the recovered key, decrypt a sample
#    recording, diff record counts. Prints DRILL PASSED or fails loudly.
ferriscribe-backup drill --url http://100.64.0.2:8741 --token <append> \
  --escrow-file ~/Desktop/ferriscribe-backup-recovery-sheet.txt
```

Launch FerriScribe — transcripts, notes, and encrypted recordings are all back.
Data loss is bounded by the snapshot interval (worst case ~24h on the default
schedule).

**Safety details worth knowing:**

- `restore` is guarded: it refuses to overwrite a *different* existing database
  key in the keychain unless you pass `--force` — restoring an old snapshot onto
  a working machine can never silently lock you out of your current data.
- Escrow artifacts self-verify (embedded check code), so a transcription error
  in the sheet is rejected at step 1, not discovered mid-restore.
- `drill` never touches live data or the keychain — it only ever restores into
  a scratch directory. Run it any time you're nervous.
- Provider API keys (`config/keys.json`) restore as bytes but their keystore is
  machine-bound — re-enter API keys in Settings if you use them. All clinical
  data fully recovers.
- If the target machine is gone but you kept local staging copies (app-data
  `backups/`, newest 14), `restore` works directly on those snapshot dirs with
  no `pull` needed.

**Operational notes.** `serve` refuses to start without an explicit `--bind` —
never expose it to `0.0.0.0`. The target bounds growth with disk-measured caps
(default 1 TiB / 1,000 snapshots; override with `FERRISCRIBE_BACKUP_MAX_BYTES` /
`FERRISCRIBE_BACKUP_MAX_SNAPSHOTS`) and sweeps crashed in-flight uploads after 7
days. Retention: set `FERRISCRIBE_BACKUP_KEEP_N` on the target (e.g. `90` for
~3 months of dailies) and it automatically prunes to the newest N snapshots
after every successful commit, using the target's own authority — the append
token still cannot delete anything. Without it, history is kept up to the caps
and pruning is a manual admin action (`POST /v1/admin/prune?keep=N`). Locally,
the daily job verifies the *target's* copy by re-pulling and drilling it, then
trims local staging copies to the newest 14 (`--keep-local N`). Scheduled
backups are macOS-only (launchd); "Back up now" works everywhere. One known
limitation: files are hashed and transferred whole in memory — for recording
libraries in the tens of GB, run the backup on a machine with ample RAM until a
streaming refactor lands.

## Disclaimer

FerriScribe is a transcription and note-drafting tool. It is **not** a medical device and has not been reviewed or approved by the FDA, CE, TGA, or any other regulatory body. Clinicians are responsible for verifying transcript accuracy and any AI-generated content before relying on it for patient care.

## License

MIT
