# Diarization eval fixture: two speakers, no intervening silence

Fixture for the gated eval test `diar_eval_two_speaker_no_silence` in
`crates/stt-providers/src/diarization.rs` (run with `FERRISCRIBE_DIAR_EVAL`,
see below). It is the D1 regression case: a speaker handoff with NO
intervening silence must not collapse the conversation to one speaker.

## Contents

- `mix_two_speaker_nosilence.wav` — 22.0 s, 16 kHz mono 16-bit PCM.
  sha256 `e05ca494d91f904515a156671bbb510e9056daa1813ae1dce876a4b9699397c4`.
- `ground_truth.json` — reference speaker turns:

  | turn | speaker | start | end   |
  |------|---------|-------|-------|
  | T1   | spk_A (Daniel) | 0.0 | 5.0   |
  | T2   | spk_B (Karen)  | 5.0 | 9.7   |
  | T3   | spk_A (Daniel) | 9.7 | 14.0  | spans the 10 s inference-window boundary
  | T4   | spk_B (Karen)  | 14.0 | 18.0 |
  | T5   | spk_A (Daniel) | 18.0 | 22.0 |

  Turn boundaries are hard concatenations of trimmed TTS speech: there is NO
  silence at any handoff. GT boundaries used by the test: 5.0, 9.7, 14.0, 18.0.

## Running the eval

The models are NOT committed (repo-size + license hygiene); point the env var
at a directory containing a `pyannote/` subdirectory with
`segmentation-3.0.onnx` and `wespeaker_en_voxceleb_CAM++.onnx`:

```sh
FERRISCRIBE_DIAR_EVAL=<dir with mix_two_speaker_nosilence.wav + pyannote/> \
  cargo test -p medical-stt-providers --lib diar_eval -- --nocapture
```

Convenient staging dir (fixture + symlinked models):

```sh
mkdir -p /tmp/diar_eval_root
cp crates/stt-providers/tests/diar_eval/mix_two_speaker_nosilence.wav /tmp/diar_eval_root/
ln -s "<models dir>/pyannote" /tmp/diar_eval_root/pyannote
```

On this machine the models live at
`~/Library/Application Support/rust-medical-assistant/models/pyannote`.

## Regenerating

macOS only (uses `say` + `afconvert`). `say --data-format` is broken on this
OS build, so render AIFF first, then convert. Requires python3 + numpy.

```sh
cd crates/stt-providers/tests/diar_eval

# 1. Render one line per voice, convert to 16 kHz mono LEI16 WAV.
for pair in \
  "Daniel|Good morning, this is Daniel. I wanted to review the blood pressure results from your last visit and talk about the medication dosage." \
  "Karen|Thank you Daniel. I have been taking the pills every morning but I still get dizzy when I stand up quickly." \
; do
  v="${pair%%|*}"; line="${pair#*|}"
  say -v "$v" --file-format=AIFF -o "spk_$v.aiff" "$line"
  afconvert -f WAVE -d LEI16@16000 -c 1 "spk_$v.aiff" "spk_$v.wav"
done

# 2. Trim, slice, and hard-concatenate per the layout table above.
#    Per-turn source slices (seconds into each voice's trimmed track):
#      T1 Daniel[0.0:5.0]  T2 Karen[0.0:4.7]  T3 Daniel[2.0:6.3]
#      T4 Karen[0.8:4.8]   T5 Daniel[3.6:7.6]
#    Trim = 20 ms pad beyond first/last sample with |x| > 300.
#    Then write mix_two_speaker_nosilence.wav + ground_truth.json.
#    (See the equivalent python inline in the D1 commit history:
#     trim_silence(x, thresh=300.0, pad_ms=20), np.concatenate, wave out.)
```

Note on determinism: `say` output is not bit-stable across macOS versions;
the committed WAV is authoritative for the asserted thresholds (96.2%
attribution, DER, boundary tolerance). A regenerated mix may need its
thresholds re-validated. TTS lines deliberately avoid silence-heavy phrasing
so hard-concatenated handoffs stay no-silence.

## Scoring (DER + attribution)

The eval test computes, alongside the ≥2-speaker and boundary assertions:

- Attribution accuracy (per-turn majority-overlap vs GT,
  permutation-invariant): fraction of GT speaking time covered by
  correctly-attributed detected time. Floor: 85%.
- DER (Diarization Error Rate) with a 0.25 s collar around every GT
  boundary and a declared overlap policy: the fixture contains no
  overlapping speech (hard-concatenated turns), so the confusion-matrix DER
  is computed on collared GT/hypothesis regions with no special overlap
  scoring; detected speech outside all GT turns counts as false alarm,
  GT speech with no matching detected speech as missed detection, and
  same-region-wrong-speaker time as speaker confusion. Optimal (0%) is
  expected only in the absence of boundary jitter; the test asserts a
  ceiling rather than perfection.
