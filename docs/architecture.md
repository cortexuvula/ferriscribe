# FerriScribe Architecture

This document provides a visual overview of how FerriScribe's Rust workspace is organized:
which crates exist, how they depend on each other, and how data flows through the major
pipelines.

## Workspace Dependency Graph

The workspace is organized into four layers. Foundation crates provide shared types and
persistence. Provider crates interface with external services (AI, STT, TTS). Feature crates
implement domain logic (RAG, agents, pipeline orchestration, export, sharing, audio capture).
The Tauri app shell ties everything together.

```mermaid
graph TB
    subgraph Foundation
        core[core<br/>shared types, errors, traits]
        security[security<br/>AES-256-GCM keystore]
        db[db<br/>SQLite, recordings, settings]
    end

    subgraph Providers
        ai[ai-providers<br/>Ollama, LM Studio]
        stt[stt-providers<br/>Whisper, diarization]
        tts[tts-providers<br/>text-to-speech]
        translation[translation<br/>text translation]
    end

    subgraph Features
        rag[rag<br/>embeddings, BM25, vector store]
        agents[agents<br/>agentic orchestrator]
        processing[processing<br/>transcription pipeline]
        export[export<br/>PDF, DOCX, FHIR]
        sharing[sharing<br/>LAN pairing, auth proxy]
        audio[audio<br/>microphone capture]
    end

    subgraph Shell
        tauri[src-tauri<br/>Tauri commands, state]
    end

    security --> core
    db --> core
    ai --> core
    stt --> core
    tts --> core
    translation --> core
    export --> core
    audio --> core
    rag --> core
    rag --> db
    agents --> core
    agents --> rag
    processing --> core
    processing --> db
    sharing --> core
    sharing --> security
    tauri --> core
    tauri --> db
    tauri --> security
    tauri --> audio
    tauri --> ai
    tauri --> stt
    tauri --> tts
    tauri --> agents
    tauri --> rag
    tauri --> processing
    tauri --> export
    tauri --> translation
    tauri --> sharing
```

## Transcription Pipeline

When a recording finishes, the transcription pipeline converts raw audio into a
diarized, vocabulary-corrected transcript. Audio capture feeds WAV chunks into
the processing pipeline, which dispatches to either local Whisper (on-device) or
remote Whisper (over LAN) for speech-to-text. Speaker diarization (pyannote +
WeSpeaker) always runs locally regardless of STT mode. Finally, custom vocabulary
rules are applied before the transcript is persisted to the database.

```mermaid
flowchart LR
    mic[microphone<br/>audio capture] --> wav[WAV chunks]
    wav --> proc[processing<br/>pipeline orchestration]
    proc --> whisper{STT mode?}
    whisper -->|local| local[stt-providers<br/>whisper.cpp on-device]
    whisper -->|remote| remote[stt-providers<br/>whisper server over LAN]
    local --> raw[raw transcript]
    remote --> raw
    raw --> diarize[speaker diarization<br/>pyannote + WeSpeaker]
    diarize --> vocab[vocabulary correction<br/>find/replace rules]
    vocab --> db[(db<br/>persist transcript)]
```

## Generation & Export Flow

After transcription, the clinician can generate SOAP notes, referrals, or letters.
The processing crate orchestrates generation by resolving the appropriate prompt
(base + context template + custom instructions), calling the AI provider (Ollama or
LM Studio), and optionally enriching with RAG-retrieved clinical documents. The
generated document is stored in the database and can be reviewed via RSVP speed
reader before export to PDF, DOCX, or FHIR R4.

```mermaid
flowchart LR
    transcript[transcript + context] --> proc[processing<br/>prompt resolution]
    proc --> prompt[resolved prompt]
    prompt --> ai[ai-providers<br/>Ollama / LM Studio]
    rag_docs[RAG documents] --> proc
    rag[rag<br/>BM25 + vector + graph] --> rag_docs
    ai --> doc[generated document<br/>SOAP / referral / letter]
    doc --> db[(db<br/>store document)]
    db --> rsvp[RSVP speed reader]
    db --> export[export<br/>PDF / DOCX / FHIR]
```

## LAN Sharing Architecture

FerriScribe supports running AI inference on a powerful office server while
clinicians connect from laptops over LAN or Tailscale. The sharing crate handles
mDNS discovery, QR-code pairing, token-based authentication, and an auth proxy
that forwards Whisper and Ollama requests. Only STT and AI calls cross the wire;
audio capture, diarization, the SQLite database, and all editors stay local on
each laptop.

```mermaid
flowchart TB
    subgraph Laptop
        app[FerriScribe app]
        local_audio[audio capture<br/>+ diarization]
        local_db[(SQLite)]
        local_editors[editors + RSVP]
    end

    subgraph "Office Server (LAN / Tailscale)"
        proxy[sharing<br/>auth proxy]
        whisper[whisper.cpp server]
        ollama[Ollama / LM Studio]
        tokens[token store<br/>per-client tokens]
    end

    app -->|pairing QR + 6-digit code| proxy
    app -->|STT requests + token| proxy
    app -->|AI requests + token| proxy
    proxy --> whisper
    proxy --> ollama
    proxy --> tokens

    local_audio -.->|stays local| app
    local_db -.->|stays local| app
    local_editors -.->|stays local| app
```
