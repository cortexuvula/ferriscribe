# Two-speaker word-attribution fixture (AMI EN2009c excerpt)

**Purpose.** Score *word-level speaker attribution* — not just speaker turns. A
diarization benchmark that supplies only turns cannot tell you whether words landed
on the right speaker, which is the user-visible symptom this fixture exists to measure.

## Source and licence

- Corpus: **AMI Meeting Corpus** (public research corpus, **CC BY 4.0** — attribution required).
- Meeting: **EN2009c**; excerpt **290.0–380.0 s** (90.0 s contiguous).
- Audio render: **`EN2009c.Array1-01.wav`** — the **single distant microphone (SDM)**
  condition: one channel of the tabletop array. Chosen deliberately over
  `Mix-Headset.wav`, which is a mix of close-talk headset mics and is *not* a fair
  proxy for the single room/phone microphone the app actually records from.
- Annotations: AMI manual annotations v1.6.2, per-speaker word XML (`words/`), which
  carry per-word `starttime`/`endtime` and a speaker id per file.

If you redistribute or publish results from this fixture, credit the AMI Meeting Corpus.

## How the excerpt was selected

Selected **from annotations alone, before any system output was inspected** — the
excerpt was not tuned to produce a favourable score. Criteria applied:

1. Contiguous 90 s window containing **exactly two active speakers**.
2. Both speakers substantially present (>= 40 words each) — avoids a window where one
   party says three words.
3. **No third speaker's words inside the window**, and none within a **10 s margin**
   outside either boundary (so a cut edge cannot leak an unannotated voice in).
4. Substantial speech and **moderate** overlap (<= 25% of the window), so the excerpt
   is representative of conversational speech rather than an extreme overlap case.

## Declared properties (measure with, and report alongside, any score)

| Property | Value |
|---|---|
| Duration | 90.0 s, 16 kHz, mono, 16-bit |
| Speakers retained | A and B (all of their words — the reference is NOT subset) |
| Reference words | 361 (A: 180, B: 181) |
| Reference turns | 37 (A: 18, B: 19) |
| Speech span | 88.5 s of 90 s (98.3%) |
| Overlapping speech | 6.4 s (7.1%) — overlap policy must be declared when scoring |
| Third-speaker contamination | none (verified inside the window and 10 s outside) |

The reference retains **every** word of both speakers. Speakers C/D are absent from
this window entirely, so contamination cannot hide: any word the system attributes to
speaker 1/2 that is not in this reference is a measurable error.

## Scoring configuration (identical for baseline and candidate)

- `max_speakers = 2` (the production setting for a two-party encounter).
- Same audio file, same model, same scoring policy for both the baseline
  (segment-window) and the candidate (word-window) merge.
- Report **transcription error separately from speaker attribution**, declaring how
  substitutions/insertions/deletions are handled — an ASR substitution must not be
  silently counted as an attribution error.

## Files

| File | sha256 |
|---|---|
| `ami_en2009c_290-380_sdm.wav` | `b32fb339e2a26e54fdcec17cfa17a58995e28e2f16f8765e278d0cad44ce0480` |
| `ami_en2009c_290-380_reference.json` | `c2ec5f20f51741e0cea3ee554d80e062c847351905f57bf5b806af60c5c0d048` |

## Regenerating

Annotations (22.9 MB, no registration):

```bash
curl -sL -o ami_ann.zip http://groups.inf.ed.ac.uk/ami/AMICorpusAnnotations/ami_public_manual_1.6.2.zip
unzip -q ami_ann.zip          # -> words/EN2009c.<A|B|C|D>.words.xml
```

Excerpt audio is byte-range fetched (no need for the full ~99 MB file):
WAV header is 44 bytes, 16 kHz mono 16-bit => 32,000 bytes/s, so
`t=290 s` starts at byte `44 + 290*32000 = 9,280,044`.

```bash
curl -sL -r 9280044-12160043 -o excerpt_raw.pcm \
  https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/EN2009c/audio/EN2009c.Array1-01.wav
# prepend a 44-byte RIFF/WAVE header (16 kHz, mono, 16-bit) to make it playable
```
