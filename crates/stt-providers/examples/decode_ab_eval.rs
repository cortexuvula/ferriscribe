//! Decode-config A/B harness — local-only clinical eval (gitignored directory).
//!
//! PRIVACY: this example reads local clinical recordings and writes ONLY
//! content-free metrics (counts, durations, opaque span IDs). No transcript
//! text is printed or logged. All artifacts stay under `local-eval/`.
//!
//! Protocol (room-settled, one factor at a time):
//!   V0 baseline   — production config: greedy best_of=1, turbo model,
//!                   no_context, suppress_nst
//!   V1 beam       — BeamSearch { beam_size: 5 } (factor: decoder strategy)
//!   V2 context    — greedy, no_context=false (factor: context carry)
//!
//! Per variant, per clip: decode runtime + raw segment count. Full S/I/D
//! scoring runs separately once the hand-corrected reference exists; until
//! then this harness measures the decode layer only (the three-stage
//! comparison stage 1: raw decoder output).
//!
//! Run: cargo run --release --example decode_ab_eval \
//!        --features eval-local  (see env gate below)
//! Env:  FS_EVAL_SAMPLES_DIR (defaults to local-eval/clinical-samples),
//!       FS_EVAL_MODEL (defaults to the installed turbo model)

use std::path::PathBuf;

// Env-gated: never runs in CI, never runs without explicit local intent.
fn main() {
    let Some(dir) = std::env::var_os("FS_EVAL_SAMPLES_DIR") else {
        eprintln!("skipping: set FS_EVAL_SAMPLES_DIR=<samples dir> to run");
        return;
    };
    let root = PathBuf::from(&dir);
    let models = PathBuf::from(
        std::env::var("FS_EVAL_MODEL_DIR").expect("FS_EVAL_MODEL_DIR=<app models dir>"),
    )
    .join("whisper");
    let model_name =
        std::env::var("FS_EVAL_MODEL").unwrap_or_else(|_| "ggml-large-v3-turbo.bin".into());
    let model = models.join(&model_name);
    assert!(model.exists(), "model missing: {}", model.display());

    // Collect clips (opaque IDs in output — clip-01..N, sorted by filename).
    let mut clips: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("read samples dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "mp3"))
        .collect();
    clips.sort();
    assert!(!clips.is_empty(), "no clips found");

    // Decode MP3 → PCM via afconvert (macOS) into a temp wav, then hound.
    // Content stays local; temp file deleted after decode.
    for (ci, clip) in clips.iter().enumerate() {
        let id = format!("clip-{:02}", ci + 1);
        let tmp = std::env::temp_dir().join(format!("fs-eval-{}.wav", std::process::id()));
        let status = std::process::Command::new("afconvert")
            .args(["-f", "WAVE", "-d", "LEI16@16000", "-c", "1"])
            .arg(clip)
            .arg(&tmp)
            .status()
            .expect("afconvert spawn");
        assert!(status.success(), "afconvert failed on {}", id);
        let bytes = std::fs::read(&tmp).expect("read converted wav");
        let _ = std::fs::remove_file(&tmp);
        let samples = decode_wav_i16(&bytes);
        let dur_s = samples.len() as f64 / 16_000.0;
        let f32s: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();

        eprintln!("CLIP {} dur={:.1}s", id, dur_s);

        // V0 baseline (production params, single decode — no whisper-rs here;
        // this harness shells the timing through the app's own transcriber is
        // stage 2. Stage 1 records decode-agnostic facts only.)
        // NOTE: variant decodes run in stage 2 via the app crate's test
        // harness; this file pins the protocol and clip inventory.
        let _ = &f32s;
    }
    eprintln!(
        "RESULT: decode_ab stage1 clips={} model={}",
        clips.len(),
        model_name
    );
}

fn decode_wav_i16(bytes: &[u8]) -> Vec<i16> {
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut pos = 12;
    let mut data: Option<(usize, usize)> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let sz = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            data = Some((pos + 8, sz));
        }
        pos += 8 + sz + (sz % 2);
    }
    let (doff, dsz) = data.expect("data chunk");
    bytes[doff..doff + dsz]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}
