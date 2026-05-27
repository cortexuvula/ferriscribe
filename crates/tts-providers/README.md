# medical-tts-providers

Text-to-speech provider implementations for the medical assistant.

This crate provides concrete [`TtsProvider`] implementations that turn text
into spoken audio. Two providers ship today:

| Provider | Module | Backend | Returns audio bytes? |
|---|---|---|---|
| **ElevenLabs** | [`elevenlabs_tts`] | ElevenLabs REST API | Yes (MP3 bytes) |
| **Local** | [`local_tts`] | OS-native TTS (feature-gated) | No — plays directly through speakers |

[`TtsProvider`]: medical_core::traits::TtsProvider

## How It Fits

```
medical-core  (defines the TtsProvider trait, TtsConfig, VoiceInfo)
    |
    v
medical-tts-providers  <-- this crate (implements the trait)
    |
    v
src-tauri  (selects a provider at runtime, drives synthesis via Tauri commands)
```

This crate depends only on **`medical-core`** for trait and type definitions.
The **`src-tauri`** application crate consumes the providers and wires them
behind Tauri command handlers.

## Crate Features

| Feature | Default | Effect |
|---|---|---|
| `local-tts` | **off** | Enables the `local_tts` module with a real OS-backed TTS engine via the [`tts`](https://crates.io/crates/tts) crate. |

When `local-tts` is **disabled** (the default), [`LocalTtsProvider`] is still
exported as a zero-sized stub so downstream code can reference the type
unconditionally.

## Key Types

### From `medical-core`

- **`TtsProvider`** — async trait: `name()`, `available_voices()`, `synthesize(text, config)`.
- **`TtsConfig`** — voice ID, language, speed, volume, model override.
- **`VoiceInfo`** — voice metadata (id, name, language, gender, preview URL).

### Defined here

- **[`ElevenLabsTtsProvider`]`::new(api_key)`** — constructs the HTTP client
  with the API key baked into default headers.
- **[`LocalTtsProvider`]`::new()`** — initialises the platform TTS engine
  (or falls back to a degraded no-op state).
- **[`TtsError`]** — crate-local error enum for construction failures
  (invalid headers, HTTP client build errors). Runtime synthesis errors use
  `AppError::TtsProvider` from core.

## How It Works

### Provider Selection

`src-tauri` constructs one provider at startup based on user settings and
holds it as a `Box<dyn TtsProvider>`. Switching providers means constructing
a new boxed trait object; there is no registry in this crate.

### Synthesis Flow

```
text + TtsConfig
       |
       v
  TtsProvider::synthesize()
       |
       +--- ElevenLabs: POST to api.elevenlabs.io, return MP3 bytes
       |
       +--- Local: speak via OS audio device, return empty Vec<u8>
```

**Important:** The local provider plays audio directly through the system
speakers and returns an *empty* byte vector. The ElevenLabs provider returns
encoded audio bytes (typically MP3) that the caller must play or persist.

### Voice Listing

- **ElevenLabs** returns a hard-coded list of five popular English voices
  (Rachel, Domi, Bella, Antoni, Arnold). This avoids an extra API round-trip
  and is sufficient for the app's current use case.
- **Local** queries the OS speech engine for installed voices and maps them
  to `VoiceInfo`.

## Configuration Defaults

### ElevenLabs

| Setting | Default |
|---|---|
| Model | `eleven_flash_v2_5` |
| Voice | `21m00Tcm4TlvDq8ikWAM` (Rachel) |
| Stability | 0.5 |
| Similarity boost | 0.75 |
| Style | 0.0 |
| Speaker boost | enabled |
| HTTP timeout | 60 seconds |

These are set in the request body and are not configurable through
`TtsConfig` fields today. The `model` field on `TtsConfig` is honoured.

### Local

Speed, volume, and voice from `TtsConfig` are forwarded to the OS engine.
Not all platforms support every setting — unsupported adjustments log a
warning and are silently skipped.

## Gotchas

1. **Local TTS does not return audio bytes.** The `tts` crate speaks
   directly through the system audio output. `synthesize()` returns
   `Ok(vec![])`. If downstream code expects to persist or stream audio,
   use ElevenLabs instead.

2. **`LocalTtsProvider` may be degraded.** If the platform TTS engine fails
   to initialise (e.g. missing `speech-dispatcher` on Linux), `new()`
   succeeds but every `synthesize()` / `available_voices()` call returns
   `AppError::TtsProvider`. Check logs for the warning at construction time.

3. **`LocalTtsProvider` is `Send + Sync` but thread-affine internally.**
   The OS speech APIs on some platforms require single-threaded access.
   A `Mutex` serialises calls, and all OS interaction happens through
   `spawn_blocking`-friendly paths.

4. **ElevenLabs voice list is static.** Adding voices requires updating the
   hard-coded list in `elevenlabs_tts.rs`. This is intentional — it avoids
   an extra API call at startup.

5. **API key validation.** `ElevenLabsTtsProvider::new()` parses the API key
   into an HTTP header value. Keys containing non-ASCII characters, newlines,
   or control characters will fail with `TtsError::InvalidHeader`.

6. **No retry logic.** Transient ElevenLabs failures (429, 503) are returned
   as errors immediately. Retry/policy lives in the caller.

## Testing

```sh
cargo test -p medical-tts-providers
```

Tests cover provider construction, header validation, voice list sanity,
and `TtsConfig` serialisation. No tests make live network calls.
