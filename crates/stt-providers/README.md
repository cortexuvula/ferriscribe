# medical-stt-providers

Speech-to-text via Whisper (local whisper.cpp or remote OpenAI-compatible server) and speaker diarization via pyannote + WeSpeaker ONNX models.

## How It Fits

```
medical-core          ← types, traits (SttProvider), error model
medical-stt-providers ← this crate: transcription + diarization
src-tauri             ← Tauri commands, provider construction, settings
```

Depends on `medical-core` for the `SttProvider` trait, audio types (`AudioData`, `Transcript`, `TranscriptSegment`), error model (`AppError`/`AppResult`), and endpoint resolution primitives (`RemoteEndpoint`). Used by `src-tauri` as the single entry point for all transcription.

## Module Map

| Module | Purpose |
|---|---|
| `lib.rs` | Crate root — re-exports `LocalSttProvider`, defines `SttError` and `SttResult` |
| `local_provider` | `LocalSttProvider` — whisper-rs inference on local GPU/CPU |
| `remote_provider` | `RemoteSttProvider` — HTTP POST to OpenAI-compatible Whisper server |
| `endpoint` | URL resolution + 30-second cache for LAN/Tailscale endpoint probing |
| `client` | HTTP client for `POST /v1/audio/transcriptions` with cancellation |
| `whisper` | `WhisperTranscriber` — whisper-rs wrapper, beam search, centisecond→second conversion |
| `diarization` | `SpeakerDiarizer` — pyannote segmentation + WeSpeaker embeddings + cosine clustering |
| `audio_prep` | Resampling (rubato polyphase sinc), f32↔i16 conversion, WAV encoding |
| `merge` | Merge whisper segments with speaker turns by timestamp overlap |
| `models` | Model metadata, download/delete, path helpers, model catalog |

## Key Types

### Provider Trait (`medical_core::traits::SttProvider`)

Both `LocalSttProvider` and `RemoteSttProvider` implement the `SttProvider` trait from `medical-core`:

```rust
#[async_trait]
pub trait SttProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supports_streaming(&self) -> bool;
    fn supports_diarization(&self) -> bool;
    async fn transcribe(&self, audio: AudioData, config: SttConfig, cancel: CancellationToken) -> AppResult<Transcript>;
    async fn transcribe_stream(&self, ...) -> AppResult<Box<dyn Stream<...>>>;
}
```

Neither provider supports streaming — both return `Err` from `transcribe_stream`.

### Diarization Types

- **`SpeakerTurn`** — `{ speaker_id: usize, start: f64, end: f64 }` — a contiguous time range for one speaker
- **`SpeakerDiarizer`** — orchestrates the three-stage pyannote pipeline (VAD → embeddings → clustering)

### Model Metadata

- **`WhisperModelId`** — enum: `Base`, `Small`, `Medium`, `LargeV3Turbo`
- **`ModelInfo`** — `{ id, filename, size_bytes, download_url, description, downloaded }` — used by Settings UI to list available/downloaded models

## How It Works

### Local Transcription Flow

```
AudioData
  │
  ▼
audio_prep::to_16k_mono_f32()          ← resample to 16 kHz mono
  │
  ▼
whisper::WhisperTranscriber             ← whisper-rs (Metal GPU on macOS)
  │  spawn_blocking (CPU-intensive)      BeamSearch beam_size=5
  ▼
Vec<WhisperSegment>                     ← timestamped text segments
  │
  ├─ (optional) diarization::SpeakerDiarizer  ← pyannote VAD + WeSpeaker embeddings
  │    spawn_blocking                         cosine-similarity clustering
  │    ▼
  │  Vec<SpeakerTurn>
  │
  ▼
merge::merge_segments_with_speakers()   ← assign speaker labels by overlap
  │
  ▼
Transcript { text, segments, language, duration_seconds, provider, metadata }
```

Key points:
- Whisper runs via `tokio::task::spawn_blocking` — it's CPU/GPU-bound and would block the async runtime
- Cancellation is checked before and after Whisper (not mid-inference — whisper-rs doesn't support interrupt callbacks without significant plumbing)
- Diarization failures are non-fatal: the provider logs a warning and returns segments without speaker labels

### Remote Transcription Flow

```
AudioData
  │
  ▼
audio_prep::to_16k_mono_f32()           ← resample
  │
  ▼
audio_prep::f32_to_i16()                ← convert for WAV
  │
  ▼
audio_prep::write_pcm16_wav_bytes()     ← encode WAV
  │
  ▼
endpoint::current_base_url()            ← resolve LAN/Tailscale (30s cache)
  │
  ▼
client::post_audio()                    ← POST multipart to /v1/audio/transcriptions
  │  tokio::select! biased cancel        verbose_json response format
  ▼
client::VerboseJson { segments, language, text }
  │
  ├─ (optional) diarization             ← same local pyannote pipeline as local flow
  │
  ▼
merge::merge_segments_with_speakers()   ← same merge as local flow
  │
  ▼
Transcript
```

Key points:
- The remote server does transcription only — diarization always runs locally
- The HTTP client uses `tokio::select!` with `biased` ordering so cancellation is checked before the HTTP response on each poll
- The `api_key` field is semantically a bearer token for the auth proxy (sharing crate), not a whisper.cpp `--api-key`

### Endpoint Resolution

When a `RemoteEndpoint` is configured (LAN/Tailscale mode), `endpoint::current_base_url()` probes addresses and caches the result for 30 seconds. This avoids re-probing on every transcription call. The cache is invalidated when `set_endpoint()` is called.

```rust
// Cache behavior:
current_base_url(&endpoint, &base_url, &mut cache)
// 1. If cache is fresh (< 30s old) → return cached URL
// 2. If endpoint configured → probe LAN, then Tailscale → cache first reachable
// 3. If no endpoint → return static base_url
```

## Examples

### Constructing a Local Provider

```rust
use medical_stt_providers::LocalSttProvider;
use medical_core::traits::SttProvider;

let provider = LocalSttProvider::new(
    app_data_dir.join("models/whisper/ggml-base.bin"),
    app_data_dir.join("models/pyannote/segmentation-3.0.onnx"),
    app_data_dir.join("models/pyannote/wespeaker_en_voxceleb_CAM++.onnx"),
);

let transcript = provider.transcribe(audio, config, cancel).await?;
```

### Constructing a Remote Provider

```rust
use medical_stt_providers::remote_provider::RemoteSttProvider;

let provider = RemoteSttProvider::new(
    "192.168.1.42",     // host
    8080,               // port
    "whisper-1",        // model name
    false,              // allow_public (rejects non-LAN hosts)
    Some(bearer_token), // auth for the sharing crate's auth proxy
    seg_model_path,
    emb_model_path,
)?;
```

### Downloading a Model

```rust
use medical_stt_providers::models;

let models = models::available_whisper_models(&app_data_dir);
for m in &models {
    if !m.downloaded {
        models::download_model(&m.download_url, &dest_path, |downloaded, total| {
            println!("{}/{} bytes", downloaded, total);
        }).await?;
    }
}
```

## Gotchas

### `x-auth-reason` Header Contract

The auth proxy in `crates/sharing/src/auth_proxy.rs` tags 401 responses with `x-auth-reason: unknown-token` when the bearer doesn't match any non-revoked row. `client::post_audio()` reads this header and surfaces a specific re-pair instruction. **Do not change these header values without coordinating both sides.**

### `tokio::select!` Biased Cancellation

`client::post_audio()` uses `tokio::select! { biased; ... }` so the cancel branch is checked first on each poll. This means a mid-flight cancellation tears down the reqwest connection at the TCP layer. Without `biased`, the HTTP response branch could win a race even when cancellation was requested.

### Diarization Is Always Local

Even when Whisper runs on a remote server, speaker diarization runs locally using pyannote ONNX models. This is by design — the remote server is a pure Whisper endpoint and has no diarization capability. The remote provider sends the same audio buffer to both the remote Whisper server and the local diarization pipeline.

### Endpoint Cache Invalidation

`RemoteSttProvider::set_endpoint()` clears the URL cache, replaces the endpoint, and propagates the endpoint's bearer into `api_key`. Without the bearer propagation, an in-session Unpair → Pair would leave a stale bearer token — causing 401s until app restart.

### Whisper Beam Search vs Greedy

The local Whisper provider uses `BeamSearch { beam_size: 5 }` rather than `Greedy`. Greedy decoding triggers whisper.cpp's hallucination-skip on difficult stretches, silently dropping content — especially problematic with medical terminology. See `crates/stt-providers/examples/transcribe_probe.rs` for the A/B/C comparison.

### Resampler Choice

`audio_prep::to_16k_mono_f32` uses rubato polyphase sinc interpolation (Blackman-Harris window), not linear interpolation. Linear interpolation has no anti-aliasing filter, so frequency content between 8 kHz and the source Nyquist aliases back into the speech band, degrading consonant features that Whisper relies on.
