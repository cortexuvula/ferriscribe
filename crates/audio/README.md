# medical-audio

Microphone capture, audio device management, format conversion, playback, and
waveform data generation for the FerriScribe medical transcription desktop app.

## How It Fits

```
medical-core        ← shared domain types, error helpers
    ↑
medical-audio       ← THIS CRATE
    ↑
src-tauri           ← Tauri commands, app lifecycle
```

`medical-audio` depends on `medical-core` for shared domain types. It is
consumed exclusively by `src-tauri`, which wires audio capture into Tauri
commands and the Svelte 5 frontend.

## Key Modules

| Module      | Purpose                                                |
|-------------|--------------------------------------------------------|
| `capture`   | Microphone capture via cpal; writes WAV files           |
| `device`    | Enumerate and select audio input/output devices         |
| `convert`   | Decode any supported audio format to 32-bit float WAV   |
| `playback`  | Play audio files through the system output device       |
| `state`     | Recording session state machine (Idle→Recording→Paused→Stopped) |
| `waveform`  | Audio signal analysis — RMS, peak, dB, normalization    |

## Key Types

- **`CaptureConfig`** — Sample rate, channel count, and ring-buffer size for a
  capture session. Defaults to 16 kHz mono, 4096-frame buffer.
- **`CaptureHandle`** — RAII handle to an active capture session. Supports
  `pause()`, `resume()`, and `stop()`. Dropping the handle stops capture and
  joins the drain thread.
- **`AudioDevice`** — Serializable descriptor for a system audio device (name,
  direction, sample rates, channels). Returned by device enumeration.
- **`Player`** — Rodio-backed audio player with play/pause/stop/volume controls.
- **`RecordingState`** / **`StateMachine`** — Enum + transition manager for the
  recording lifecycle. Enforces valid state transitions and tracks elapsed time
  across pause/resume cycles.
- **`AudioError`** / **`AudioResult<T>`** — Crate-wide error type and result
  alias.

## How It Works

### Capture Lifecycle

```
1. Select device        device::get_input_device(name)
                              │
2. Start capture        capture::start_capture(device, config, path)
                              │
   ┌──────────────────────────┴──────────────────────────┐
   │  cpal input stream callback                         │
   │    → pushes f32 samples into a ring buffer          │
   │                                                     │
   │  Drain thread (spawned)                             │
   │    → pops samples from ring buffer                  │
   │    → writes them to a WAV file via hound            │
   │    → every ~50 ms, downsamples to 128-point         │
   │      waveform snapshot and sends via mpsc channel   │
   └─────────────────────────────────────────────────────┘
                              │
3. Control              handle.pause() / handle.resume()
                              │
4. Stop                 handle.stop()  (or drop handle)
   → sets stop flag
   → drain thread flushes remaining samples + finalizes WAV
   → joins thread
```

The capture callback and drain thread are decoupled by a lock-free ring buffer
(`ringbuf::HeapRb`) sized for ~2 seconds of audio. This prevents the real-time
audio callback from blocking on I/O.

### State Machine

```
         start()              pause()
Idle ──────────→ Recording ←──────────→ Paused
                      │                     │
                      │   stop()            │ stop()
                      ↓                     ↓
                   Stopped ←──────────── Stopped
                      │
                      │ reset()
                      ↓
                     Idle
```

- **Idle → Recording**: `start(file_path, device_name)`
- **Recording → Paused**: `pause()` — freezes elapsed timer
- **Paused → Recording**: `resume()` — restarts elapsed timer, accumulates
- **Recording | Paused → Stopped**: `stop()` — records final duration
- **Stopped | Idle → Idle**: `reset()`

Invalid transitions return `AudioError::InvalidTransition`.

### Waveform Data

The drain thread accumulates ~50 ms of samples, then calls
`downsample_waveform()` which reduces the chunk to 128 points by taking the
peak absolute value in each window. These snapshots are sent to the frontend
via a bounded `mpsc::sync_channel(32)` — if the UI consumer stalls, the newest
frames are silently dropped (back-pressure is acceptable for visualization).

### Format Conversion

`convert::convert_to_wav()` uses rodio's `Decoder` to read any supported format
(WAV, MP3, FLAC, OGG Vorbis, AAC/M4A) and writes a 32-bit float WAV via hound.
This is used to normalize uploaded or imported audio before transcription.

## External Dependencies

| Crate     | Role                                           |
|-----------|------------------------------------------------|
| `cpal`    | Cross-platform audio I/O (microphone capture)  |
| `rodio`   | Audio decoding and playback                    |
| `hound`   | WAV file reading/writing                       |
| `ringbuf` | Lock-free ring buffer for capture pipeline     |
| `tokio`   | Async runtime (used by the Tauri layer)        |

## Gotchas

### Audio Device Enumeration

- cpal's `supported_input_configs()` can fail or return empty on some
  platforms (especially in headless CI). Device enumeration functions handle
  this gracefully but may return empty lists.
- On macOS, device names may change between system restarts. Match by
  substring or use the system default when the configured name is not found.

### Sample Rate Negotiation

- The capture pipeline requests a sample rate (default 16 kHz) but the device
  may not support it. `negotiate_stream_config()` falls back through 48 kHz,
  44.1 kHz, 16 kHz, 22 kHz, 96 kHz, then the device's max rate.
- The **actual** rate and channel count may differ from the requested config.
  Always use the values from the negotiated `StreamConfig`, not the original
  `CaptureConfig`, when computing downstream parameters.

### Channel Count

- The capture callback consumes `&[f32]`, so cpal stream configs prefer
  `SampleFormat::F32` ranges. An I16-only device falls through to
  `default_input_config()`.
- The channel count in the returned `StreamConfig` always matches the
  device's native channel count. Asking for mono on a stereo-only device
  would fail. Downstream code (`audio_prep::to_mono`) handles the mixdown.

### State Machine

- `Instant` fields (`started_at`, `paused_at`) are `#[serde(skip)]` — they
  are not preserved across serialization. After deserialization, `elapsed()`
  returns only the accumulated `elapsed_before_pause` / `duration`.
- Calling `pause()` from `Idle` or `resume()` from `Recording` returns
  `AudioError::InvalidTransition`. The state machine is intentionally strict.

### Playback

- `Player::new()` requires an active audio output device. In headless
  environments it returns `AudioError::Playback` — callers should degrade
  gracefully.
- The `OutputStream` stored inside `Player` must not be dropped while the
  `Sink` is alive. This is why both are kept in the struct.
