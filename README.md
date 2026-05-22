# FerriScribe

A privacy-first medical transcription desktop application built with Rust and Svelte. Record doctor-patient encounters, transcribe them locally with speaker diarization, generate SOAP notes and clinical documents, and export to PDF, DOCX, or FHIR.

## Features

### Transcription
- **Local Speech-to-Text** — Whisper (via whisper-rs / whisper.cpp) with Metal GPU acceleration on macOS. Runs on-device with beam-search decoding and temperature fallback for accuracy on long recordings; no audio leaves your machine in this mode.
- **Remote Whisper (optional)** — Switch STT Mode to Remote to offload transcription to any OpenAI-compatible Whisper server (e.g. `whisper.cpp server`, `faster-whisper-server`, LocalAI) running on another machine over LAN or Tailscale. See [Running Across Machines](#running-across-machines-lan--tailscale).
- **Speaker Diarization** — Pyannote + WeSpeaker (ONNX) pipeline labels who is speaking (e.g. Doctor vs. Patient). Runs locally in both STT modes.
- **Custom Vocabulary** — User-defined find/replace rules applied after STT, with word-boundary matching, priority ordering, and import/export compatible with the Python Medical-Assistant `vocabulary.json` format.

### Documents & Review
- **SOAP Note Generation** — AI-powered Subjective / Objective / Assessment / Plan notes from transcripts.
- **Referral, Clinical Letter, and Synopsis Generation** — Templated AI generation with per-document custom prompts.
- **Letter Audiences** — Generate letters tailored to different recipients:
  - **Patient** — Plain language, empathetic
  - **Insurance Company** — Medical necessity language, ICD/CPT codes
  - **Tax Authority** — Expense justification, timeline
  - **Specialist/Consultant** — Clinical detail, peer tone
  - **Employer/School** — Accommodations, HIPAA-minimal
  - **Legal/Court** — Formal opinion, timeline

  Create custom audiences in **Settings → Letter Audiences** with your own system prompts and user templates.
- **Context Templates** — Pre-built visit types (e.g. Follow-up, New Patient) with custom instructions layered on top of the base prompt; import/export as JSON.
- **RSVP Speed Reader** — Rapid-serial-visual-presentation review mode for SOAP notes and transcripts — chunk-size, WPM, and per-section filters configurable in Settings.

### AI providers
- **Local and LAN-accessible only** — Ollama and LM Studio, each configurable with a remote host/port so you can run the heavy model on a separate machine over LAN or Tailscale.
- **Retrieval-Augmented Generation (RAG)** — Ingest clinical documents; embeddings served by the same Ollama instance, with BM25 + vector + graph retrieval at query time.
- **Agentic Workflows** — Multi-step orchestrator with tool use (RAG search, note generation) for chat sessions.

### Data
- **Recording Management** — Record, import, search, tag, and organize audio. SQLite-backed.
- **Export** — PDF, DOCX, and FHIR R4 (healthcare interoperability standard).
- **Secure Key Storage** — API keys encrypted at rest with AES-256-GCM; the master cipher key is derived via PBKDF2-HMAC-SHA256 (600 000 iterations) from an optional `MEDICAL_ASSISTANT_MASTER_KEY` env var or a per-machine identifier.

### Platform
- **Cross-Platform** — macOS (Metal-accelerated STT), Windows, and Linux.

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Frontend | Svelte 5, SvelteKit, TypeScript, Vite |
| Backend | Rust (edition 2024), Tauri v2 |
| STT | whisper-rs (whisper.cpp), ort (ONNX Runtime), knf-rs, rubato |
| Database | SQLite (via rusqlite) |
| AI | Ollama, LM Studio (OpenAI-compatible wire protocol) |
| Export | PDF (printpdf), DOCX (docx-rs), FHIR R4 |
| Security | AES-256-GCM + PBKDF2 (aes-gcm + pbkdf2 crates) |

## Architecture

FerriScribe is organized as a Cargo workspace with 12 crates:

```
crates/
  core/           — shared types, traits, error handling
  db/             — SQLite database, settings, recordings
  security/       — AES-256-GCM API-key storage
  audio/          — microphone capture (cpal)
  ai-providers/   — Ollama + LM Studio (OpenAI-compat wire)
  stt-providers/  — whisper transcription + pyannote diarization
  tts-providers/  — text-to-speech
  agents/         — agentic orchestrator with tool registry
  rag/            — vector store, BM25, graph search, ingestion
  processing/     — transcription pipeline orchestration
  export/         — PDF, DOCX, FHIR export
  translation/    — text translation
src-tauri/        — Tauri app shell, commands, state management
src/              — Svelte 5 frontend
```

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) 1.85+ (required by `edition = "2024"`)
- [Node.js](https://nodejs.org/) 20+
- [CMake](https://cmake.org/) and Clang (for whisper.cpp and ONNX Runtime)
- macOS: Xcode Command Line Tools

### Build & Run

```bash
npm install
npm run tauri dev
```

Release builds are produced by the GitHub Actions workflow on tag pushes matching `v*`. Artifacts are attached to the release page.

### Model Setup

On first launch, go to **Settings > Audio / STT** and download:

1. **Whisper model** — Choose a size (base ~148 MB to large-v3-turbo ~1.6 GB). Larger models are more accurate. Skip this step if you'll only use Remote STT.
2. **Diarization models** — required in BOTH STT modes, since diarization always runs locally:
   - Pyannote segmentation 3.0 (~6 MB)
   - WeSpeaker CAM++ embedding (~28 MB)

Models are downloaded from HuggingFace / GitHub and stored under the app's data directory (see [Where Your Data Lives](#where-your-data-lives)).

## Usage

1. **Record** — Start a new recording or import an existing audio file.
2. **Transcribe** — Local Whisper runs on-device by default; Custom Vocabulary corrections are applied automatically after STT.
3. **Generate** — Produce a SOAP note, referral, clinical letter, or synopsis from the transcript, optionally guided by a Context Template.
4. **Review** — Edit inline, or use the RSVP speed reader to review at high WPM.
5. **Export** — Save as PDF, DOCX, or FHIR R4.
6. **Chat** — Ask follow-up questions grounded in the recording and any ingested RAG documents.

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

### Security

Per-client tokens are issued during pairing and stored in the laptop's
OS keychain. Revoke a lost / stolen laptop's access from the office
server's **Connected clients** panel.

Pairing traffic is plain HTTP. On a clinic LAN with guest Wi-Fi or BYOD
risk, prefer Tailscale (which transparently encrypts with WireGuard).

### What stays local on each laptop

- Audio capture and waveform display
- Speaker diarization (pyannote + WeSpeaker)
- SQLite database, vocabulary rules, RAG vector store
- The SOAP / referral / letter / synopsis editors

Only Whisper inference and Ollama chat / embedding calls cross the wire.

## Where Your Data Lives

Recordings, transcripts, settings, downloaded models, and the encrypted keystore all live under the OS-specific app data directory:

| OS | Path |
|----|------|
| macOS | `~/Library/Application Support/rust-medical-assistant/` |
| Linux | `~/.local/share/rust-medical-assistant/` |
| Windows | `%APPDATA%\rust-medical-assistant\` |

Inside you'll find `medical.db` (SQLite), `config/keys.json` (encrypted API keys), `models/whisper/*.bin`, `models/pyannote/*.onnx`, and the recordings themselves in whatever path you configured under **Settings → General**. Delete the directory to fully remove all user data.

### Optional: stronger master key

By default the keystore's master cipher key is derived from the machine identifier. To bind it to a secret you control — for example if multiple users share the same machine — set `MEDICAL_ASSISTANT_MASTER_KEY` in the environment FerriScribe is launched from; PBKDF2-HMAC-SHA256 will derive the cipher key from that value instead. Losing the env var value makes the keystore unrecoverable.

## Disclaimer

FerriScribe is a transcription and note-drafting tool. It is **not** a medical device and has not been reviewed or approved by the FDA, CE, TGA, or any other regulatory body. Clinicians are responsible for verifying transcript accuracy and any AI-generated content before relying on it for patient care.

## License

MIT
