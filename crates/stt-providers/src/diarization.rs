//! Speaker diarization using pyannote ONNX models via ort.
//!
//! Implements the pyannote pipeline directly in Rust (no Python dependency):
//!
//! 1. **Speech activity + speaker-slot segmentation** — pyannote `segmentation-3.0`
//!    ONNX model processes 10-second windows of 16 kHz i16 audio. Its output
//!    contract (decoded empirically, see [`SpeakerDiarizer::detect_speech_segments`]):
//!    a `(1, 589, 7)` log-softmax tensor per 10 s window, where each of the 589
//!    frames covers 270 samples starting at sample 721, channel 0 is non-speech,
//!    and channels 1..=6 are *local speaker slots for that window*. Slots are
//!    permuted per window (pyannote trains with random speaker permutation per
//!    chunk), so slot indices must NOT be treated as global speaker identities.
//!    A speaker change with no intervening silence appears as a change in the
//!    dominant non-zero slot (see the eval test `diar_eval_two_speaker_no_silence`),
//!    so turn boundaries are detected by tracking dominant-slot switches rather
//!    than speech/non-speech alone.
//!
//! 2. **Speaker embedding extraction** — WeSpeaker `CAM++` ONNX model converts each
//!    speech segment's fbank features (80-dim Mel filterbank via knf-rs) into a
//!    fixed-size speaker embedding vector. Embeddings are L2-normalized.
//!
//! 3. **Cosine-similarity speaker clustering** — greedy clustering: each embedding
//!    is compared against known speaker centroids. If the best cosine similarity
//!    exceeds 0.5, the segment is assigned to that speaker; otherwise a new
//!    speaker cluster is created. Cluster merges combine centroids by member
//!    weight (a merge is a weighted mean of the two centroids, never a
//!    delete-and-forget), and pair enumeration is deterministic (sorted IDs,
//!    lexicographic tie-break on equal similarity).
//!
//! All inference runs on CPU via the `ort` crate (ONNX Runtime bindings). The diarization
//! pipeline runs inside `spawn_blocking` to avoid blocking the async runtime.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use ndarray::{Array1, ArrayViewD, IxDyn};
use ort::session::Session;
use ort::value::{Tensor, TensorRef};
use tracing::{debug, info};

use medical_core::error::{AppError, AppResult};

/// Number of output channels of pyannote segmentation-3.0: channel 0 is
/// non-speech, channels 1..=NUM_SPEAKER_SLOTS are local speaker slots.
const SEGMENTATION_CHANNELS: usize = 7;

/// Frames in the segmentation output for one 10 s window at 16 kHz.
/// (589 = (160000 - receptive_field) / 270 + 1; matches pyannote's pyannote.audio
/// framing for a 10 s chunk.)
const SEGMENTATION_FRAMES: usize = 589;

/// Sample offset of the first output frame within a 10 s window.
const FRAME_START: usize = 721;

/// Samples covered by each output frame.
const FRAME_SIZE: usize = 270;

/// Consecutive frames of a different dominant slot required to declare a
/// speaker change. Guards against single-frame slot flicker at low-energy
/// frames; 3 frames ≈ 51 ms.
const SLOT_CHANGE_FRAMES: usize = 3;

/// Turns shorter than this are dropped (VAD flicker, not a real turn).
const MIN_TURN_S: f64 = 0.5;

/// Post-clustering merge threshold for centroid cosine similarity.
const CENTROID_MERGE_THRESHOLD: f32 = 0.75;

/// Greedy-assignment cosine-similarity threshold.
///
/// Calibrated on the eval fixture (tests/diar_eval): WeSpeaker CAM++
/// same-speaker turn similarity is 0.92–0.98 and cross-speaker (different
/// synthetic voices) is 0.43–0.50, so the historical 0.5 sat inside the
/// cross-speaker band and snapped a new speaker's first turn onto an existing
/// cluster. 0.7 is the midpoint of the measured separation with ~0.2 margin
/// on both sides.
const ASSIGN_THRESHOLD: f32 = 0.7;

/// A speaker turn: a contiguous time range attributed to one speaker.
///
/// Speaker IDs are zero-based and assigned by the greedy clustering algorithm
/// in [`SpeakerDiarizer::diarize()`]. The merge layer formats them as
/// `"Speaker N"` (1-based) for display.
#[derive(Debug, Clone)]
pub struct SpeakerTurn {
    /// Zero-based speaker cluster ID.
    pub speaker_id: usize,
    /// Turn start time in seconds.
    pub start: f64,
    /// Turn end time in seconds.
    pub end: f64,
}

/// A raw speech segment (one speaker-homogeneous turn candidate) with its audio samples.
struct SpeechSegment {
    start: f64,
    end: f64,
    samples: Vec<i16>,
}

/// Safe: push a speech segment if it spans a non-empty range of real audio.
///
/// `start_offset` and `end_samples` are both in **sample units** (not seconds).
/// Guards against out-of-bounds slicing when the model's frame offset lands
/// past the real audio (e.g., in zero-padded regions), and drops segments
/// whose clamped range is empty — the downstream fbank extractor panics or
/// produces NaN on zero-length input.
fn push_segment_if_valid(
    segments: &mut Vec<SpeechSegment>,
    start_offset: f64,
    end_samples: usize,
    samples_i16: &[i16],
    sample_rate: f64,
) {
    let start_idx = start_offset as usize;
    let end_idx = end_samples.min(samples_i16.len());

    if start_idx >= end_idx {
        return;
    }

    segments.push(SpeechSegment {
        start: start_offset / sample_rate,
        end: end_idx as f64 / sample_rate,
        samples: samples_i16[start_idx..end_idx].to_vec(),
    });
}

/// Speaker diarization using pyannote ONNX models.
///
/// Runs the three-stage pipeline: segmentation → embedding extraction → clustering.
/// Models are loaded fresh on each `diarize()` call. Both ONNX sessions
/// are configured with `intra_threads: 1` to avoid thread contention when
/// running inside `spawn_blocking` alongside other blocking tasks.
pub struct SpeakerDiarizer {
    segmentation_path: PathBuf,
    embedding_path: PathBuf,
}

impl SpeakerDiarizer {
    /// Create a diarizer with paths to the pyannote segmentation and WeSpeaker
    /// embedding ONNX models.
    ///
    /// No models are loaded at construction time — loading happens inside `diarize()`.
    pub fn new(segmentation_path: PathBuf, embedding_path: PathBuf) -> Self {
        Self {
            segmentation_path,
            embedding_path,
        }
    }

    /// Run speaker diarization on 16 kHz mono i16 audio.
    ///
    /// Returns a list of [`SpeakerTurn`]s with start/end timestamps and speaker IDs.
    /// The three stages run sequentially:
    ///
    /// 1. **Segmentation** — pyannote segmentation model detects speech regions and
    ///    speaker changes in 10 s windows (dominant-slot tracking, see module docs)
    /// 2. **Embeddings** — WeSpeaker CAM++ extracts per-segment speaker vectors
    /// 3. **Clustering** — greedy cosine-similarity clustering with weight-carrying
    ///    centroid merges. If `max_speakers` is set, the most-similar clusters are
    ///    merged until the count is at or below the limit.
    ///
    /// Returns `Ok(vec![])` if no speech is detected. Returns `Err` if models
    /// fail to load or inference panics.
    pub fn diarize(
        &self,
        samples_i16: &[i16],
        sample_rate: u32,
        max_speakers: Option<u32>,
    ) -> AppResult<Vec<SpeakerTurn>> {
        info!(
            samples = samples_i16.len(),
            sample_rate, "Starting speaker diarization"
        );

        // Stage 1: segmentation — speech regions AND speaker-homogeneous turn candidates
        let segments = self.detect_speech_segments(samples_i16, sample_rate)?;
        info!(segments = segments.len(), "Segmentation stage complete");
        for (i, seg) in segments.iter().enumerate() {
            debug!(
                segment = i,
                start = format!("{:.2}", seg.start),
                end = format!("{:.2}", seg.end),
                duration = format!("{:.2}", seg.end - seg.start),
                samples = seg.samples.len(),
                "Speech segment"
            );
        }

        if segments.is_empty() {
            return Ok(Vec::new());
        }

        // Drop sub-MIN_TURN_S slivers BEFORE embedding/clustering: a ~0.2 s
        // handoff sliver straddles two voices, and its (mixed-speaker)
        // embedding pollutes centroid estimates — it can snap a real
        // speaker's next turn onto the wrong cluster or spawn a phantom
        // one. Filtering here also skips the wasted fbank+ONNX compute.
        let segments: Vec<SpeechSegment> = segments
            .into_iter()
            .filter(|seg| seg.end - seg.start >= MIN_TURN_S)
            .collect();

        // Stage 2: Extract speaker embeddings for each segment
        let embeddings = self.extract_embeddings(&segments)?;
        info!(embeddings = embeddings.len(), "Embedding stage complete");

        // Stage 3: Cluster embeddings into speakers
        let speaker_ids = cluster_speakers(&embeddings, ASSIGN_THRESHOLD, max_speakers);

        // Build speaker turns
        let turns: Vec<SpeakerTurn> = segments
            .iter()
            .zip(speaker_ids.iter())
            .map(|(seg, &speaker_id)| SpeakerTurn {
                speaker_id,
                start: seg.start,
                end: seg.end,
            })
            .collect();

        let num_speakers = turns
            .iter()
            .map(|t| t.speaker_id)
            .max()
            .map_or(0, |m| m + 1);
        info!(
            segments = segments.len(),
            embeddings = embeddings.len(),
            turns = turns.len(),
            speakers = num_speakers,
            "Diarization complete (stage counts)"
        );

        Ok(turns)
    }

    /// Stage 1: Run pyannote segmentation model to detect speech segments.
    ///
    /// Output contract (decoded empirically on the real model, and verified by
    /// the eval test `diar_eval_two_speaker_no_silence`):
    ///
    /// - Input `(B, 1, 160000)` f32; output `(B, 589, 7)` f32 log-softmax.
    /// - Dim 1 is time: 589 frames, frame `i` covering samples
    ///   `[721 + 270*i, 721 + 270*i + 270)` of the window.
    /// - Softmax is over dim 2 (7 channels): channel 0 = non-speech,
    ///   channels 1..=6 = local speaker slots.
    /// - Slot indices are per-window: the model is trained with a random
    ///   speaker→slot permutation per chunk, so slot 2 in window 1 and slot 2
    ///   in window 2 are unrelated. Global speaker identity comes only from
    ///   the embedding+clustering stages.
    ///
    /// Because a single `SpeechSegment` must be speaker-homogeneous to yield a
    /// meaningful embedding, segmentation tracks the *dominant* non-zero slot
    /// per frame and cuts a new segment when the dominant slot changes for
    /// [`SLOT_CHANGE_FRAMES`] consecutive frames. A speaker handoff with no
    /// silence still produces a slot switch, keeping handoffs inside one
    /// segment (the D1 defect) impossible.
    fn detect_speech_segments(
        &self,
        samples_i16: &[i16],
        sample_rate: u32,
    ) -> AppResult<Vec<SpeechSegment>> {
        if samples_i16.is_empty() {
            return Ok(Vec::new());
        }

        let mut session = Session::builder()
            .map_err(|e| {
                AppError::stt_provider(format!(
                    "Failed to create segmentation session builder: {e}"
                ))
            })?
            .with_intra_threads(1)
            .map_err(|e| AppError::stt_provider(format!("Failed to set intra threads: {e}")))?
            .commit_from_file(&self.segmentation_path)
            .map_err(|e| {
                AppError::stt_provider(format!("Failed to load segmentation model: {e}"))
            })?;

        let window_size = (sample_rate * 10) as usize; // 10-second windows

        let sr_f64 = sample_rate as f64;

        // Raw turn candidates: (start_sample, end_sample_exclusive), in sample units.
        let mut raw_turns: Vec<(usize, usize)> = Vec::new();
        // Open-turn state, all in sample units.
        let mut cur_slot: Option<usize> = None;
        let mut cur_start: usize = 0;
        let mut cur_end: usize = 0;
        // Pending handoff: dominant slot != current, awaiting SLOT_CHANGE_FRAMES
        // consecutive frames to be confirmed.
        let mut pending_slot: Option<usize> = None;
        let mut pending_run: usize = 0;

        // Pad to align to full windows
        let mut padded = Vec::from(samples_i16);
        let remainder = padded.len() % window_size;
        if remainder != 0 {
            let pad_len = window_size - remainder;
            padded.resize(padded.len() + pad_len, 0i16);
        }

        for chunk_start in (0..padded.len()).step_by(window_size) {
            let chunk_end = (chunk_start + window_size).min(padded.len());
            let window = &padded[chunk_start..chunk_end];

            // Reset frame position to this window's starting sample.
            let mut offset = chunk_start + FRAME_START;

            // Convert i16 window to f32 for the model
            let window_f32: Vec<f32> = window.iter().map(|&s| s as f32).collect();

            // Shape: [1, 1, window_size]
            let input = TensorRef::from_array_view(([1_usize, 1, window_f32.len()], &*window_f32))
                .map_err(|e| {
                    AppError::stt_provider(format!("Failed to create input tensor: {e}"))
                })?;

            let outputs = session.run(ort::inputs![input]).map_err(|e| {
                AppError::stt_provider(format!("Segmentation inference failed: {e}"))
            })?;

            let output = &outputs[0];
            let (shape, data) = output.try_extract_tensor::<f32>().map_err(|e| {
                AppError::stt_provider(format!("Failed to extract segmentation output: {e}"))
            })?;

            let shape_slice: Vec<usize> = (0..shape.len()).map(|i| shape[i] as usize).collect();
            let view = ArrayViewD::<f32>::from_shape(IxDyn(&shape_slice), data)
                .map_err(|e| AppError::stt_provider(format!("Failed to reshape output: {e}")))?;

            // Decode (589, 7) per-window output. `view` is (1, 589, 7); iterate
            // frames along axis 1, channels along the last axis.
            let frames = view
                .outer_iter()
                .next()
                .expect("segmentation output has batch dim");
            debug_assert_eq!(
                frames.dim(),
                IxDyn(&[SEGMENTATION_FRAMES, SEGMENTATION_CHANNELS])
            );
            for frame in frames.axis_iter(ndarray::Axis(0)) {
                // Dominant channel = argmax over the 7 channels. Channel 0 is
                // non-speech; 1..=6 are local speaker slots.
                let max_index = frame
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(Ordering::Equal))
                    .map(|(idx, _)| idx)
                    .unwrap_or(0);

                let slot = if max_index == 0 {
                    None
                } else {
                    Some(max_index)
                };

                match (cur_slot, slot) {
                    (None, None) => {}
                    (None, Some(s)) => {
                        // Speech onset (or slot after non-speech dip)
                        cur_slot = Some(s);
                        cur_start = offset;
                        cur_end = offset;
                        pending_slot = None;
                        pending_run = 0;
                    }
                    (Some(_), None) => {
                        // Non-speech dip inside an open turn. Do NOT clear a
                        // pending handoff: the frames around a speaker change
                        // often flicker through non-speech (breath, plosive),
                        // and clearing would swallow the handoff. The dip only
                        // pauses cur_end; the turn is closed when a confirmed
                        // new slot follows (see below) or at flush.
                    }
                    (Some(_), Some(s)) if Some(s) == cur_slot => {
                        // Same slot: confirmed speech, clear any pending handoff.
                        pending_slot = None;
                        pending_run = 0;
                        cur_end = offset;
                    }
                    (Some(_), Some(s)) => {
                        // Different slot: candidate handoff. Non-speech frames
                        // in between do not break the run (they neither confirm
                        // nor refute the new speaker), matching the calibrated
                        // prototype.
                        if pending_slot == Some(s) {
                            pending_run += 1;
                        } else {
                            pending_slot = Some(s);
                            pending_run = 1;
                        }
                        if pending_run >= SLOT_CHANGE_FRAMES {
                            // Confirmed speaker change with NO intervening silence.
                            // Cut the turn at the first frame of the new slot.
                            let boundary =
                                offset.saturating_sub((SLOT_CHANGE_FRAMES - 1) * FRAME_SIZE);
                            raw_turns.push((cur_start, boundary.max(cur_start)));
                            cur_slot = Some(s);
                            cur_start = boundary.max(cur_start);
                            cur_end = offset;
                            pending_slot = None;
                            pending_run = 0;
                        } else {
                            cur_end = offset;
                        }
                    }
                }

                offset += FRAME_SIZE;
            }
        }

        // Flush the open turn.
        if cur_slot.is_some() {
            raw_turns.push((cur_start, (cur_end + FRAME_SIZE).min(samples_i16.len())));
        }

        // Convert raw turn candidates into clamped SpeechSegments.
        let mut segments: Vec<SpeechSegment> = Vec::new();
        for &(start, end) in &raw_turns {
            push_segment_if_valid(&mut segments, start as f64, end, samples_i16, sr_f64);
        }

        // NOTE: no audio-contiguity gap-merging pass. The decode loop keeps a
        // turn open across short non-speech dips (a dip neither confirms nor
        // refutes a handoff), so segments are already speaker-homogeneous; a
        // contiguity merge would re-fuse slot-cut turns at handoffs (their
        // boundary gap is exactly 0) and reintroduce the D1 collapse.

        debug!(
            raw_turns = raw_turns.len(),
            segments = segments.len(),
            "Segmentation turn candidates"
        );

        Ok(segments)
    }

    /// Stage 2: Extract speaker embedding for each speech segment using the wespeaker model.
    fn extract_embeddings(&self, segments: &[SpeechSegment]) -> AppResult<Vec<Vec<f32>>> {
        let mut session = Session::builder()
            .map_err(|e| {
                AppError::stt_provider(format!("Failed to create embedding session builder: {e}"))
            })?
            // CORRECTNESS: graph optimizations MUST stay disabled for this
            // model. The ONNX Runtime ~1.22 binaries shipped by ort
            // 2.0.0-rc.13 mis-execute the WeSpeaker CAM++ graph at
            // optimization Level 1+ — embeddings come back with exploded
            // norms (22 -> 70/541) and near-zero cosine to the reference,
            // which collapses speaker clustering. Verified against Python
            // onnxruntime 1.30 (bit-identical at L0; the bug is fixed
            // upstream) on byte-identical fbank inputs; the pyannote
            // segmentation model is NOT affected (identical at every
            // level). See the eval test `diar_eval_two_speaker_no_silence`.
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Disable)
            .map_err(|e| AppError::stt_provider(format!("Failed to set optimization level: {e}")))?
            .with_intra_threads(1)
            .map_err(|e| AppError::stt_provider(format!("Failed to set intra threads: {e}")))?
            .commit_from_file(&self.embedding_path)
            .map_err(|e| AppError::stt_provider(format!("Failed to load embedding model: {e}")))?;

        let mut embeddings = Vec::with_capacity(segments.len());

        for seg in segments {
            // Convert i16 → f32 for knf-rs fbank computation
            let mut samples_f32 = vec![0.0f32; seg.samples.len()];
            knf_rs::convert_integer_to_float_audio(&seg.samples, &mut samples_f32);

            // Compute fbank features (80-dim Mel filterbank)
            // knf-rs returns ndarray 0.16 types; extract raw data to bridge to ort's ndarray 0.17
            let features = knf_rs::compute_fbank(&samples_f32)
                .map_err(|e| AppError::stt_provider(format!("fbank computation failed: {e}")))?;
            let feat_shape = features.shape().to_vec(); // [frames, 80]
            let feat_data = features.into_raw_vec_and_offset().0;

            // Reshape to [1, frames, 80] for batch dimension
            let input = Tensor::from_array((
                [1_usize, feat_shape[0], feat_shape[1]],
                feat_data.into_boxed_slice(),
            ))
            .map_err(|e| {
                AppError::stt_provider(format!("Failed to create embedding input: {e}"))
            })?;

            let outputs = session
                .run(ort::inputs!["feats" => input])
                .map_err(|e| AppError::stt_provider(format!("Embedding inference failed: {e}")))?;

            let emb_output = outputs.get("embs").ok_or_else(|| {
                AppError::stt_provider("Embedding model missing 'embs' output".to_string())
            })?;

            let (_, data) = emb_output
                .try_extract_tensor::<f32>()
                .map_err(|e| AppError::stt_provider(format!("Failed to extract embedding: {e}")))?;

            embeddings.push(data.to_vec());
        }

        Ok(embeddings)
    }
}

/// Stage 3: Cluster speaker embeddings using cosine similarity.
///
/// Greedy clustering with centroid updates: each embedding is compared against
/// known speaker centroids (running mean of all assigned segments). If the best
/// match exceeds `threshold`, the segment is assigned and the centroid is
/// updated; otherwise a new speaker is created.
///
/// After greedy clustering, similar centroids (cosine similarity >
/// [`CENTROID_MERGE_THRESHOLD`]) are merged, then — if `max_speakers` is set —
/// the most-similar centroids are iteratively merged until the count is at or
/// below the limit. Merges combine centroids by member weight:
/// `merged = (a * weight_a + b * weight_b) / (weight_a + weight_b)`, so the
/// surviving centroid reflects all its members, and the merged cluster's
/// weight is carried forward for subsequent merges (fixes the D3
/// weighted-centroid defect).
fn cluster_speakers(
    embeddings: &[Vec<f32>],
    threshold: f32,
    max_speakers: Option<u32>,
) -> Vec<usize> {
    let mut clusters: HashMap<usize, (Array1<f32>, usize)> = HashMap::new(); // id -> (centroid, weight)
    let mut next_id: usize = 0;
    let mut assignments = Vec::with_capacity(embeddings.len());

    // Greedy clustering with centroid updates
    for (idx, emb) in embeddings.iter().enumerate() {
        let emb_arr = Array1::from_vec(emb.clone());

        let mut best_id = None;
        let mut best_sim = threshold;

        // Deterministic candidate order: iterate cluster IDs ascending so a
        // similarity TIE resolves to the lowest cluster ID. HashMap iteration
        // order must never influence assignment (D3 determinism: probes
        // equidistant from two centroids flipped labels run-to-run).
        let mut candidate_ids: Vec<usize> = clusters.keys().copied().collect();
        candidate_ids.sort_unstable();
        for id in candidate_ids {
            let (centroid, _) = &clusters[&id];
            let sim = cosine_similarity(&emb_arr, centroid);
            debug!(
                segment = idx,
                speaker = id,
                similarity = format!("{:.4}", sim),
                threshold = format!("{:.4}", threshold),
                "Comparing segment to speaker centroid"
            );
            if sim > best_sim {
                best_id = Some(id);
                best_sim = sim;
            }
        }

        let assigned = match best_id {
            Some(id) => {
                // Update centroid: running mean of all assigned embeddings
                let (centroid, count) = clusters.get_mut(&id).unwrap();
                // new_centroid = (old_centroid * count + new_emb) / (count + 1)
                let n = *count as f32;
                for (c, &e) in centroid.iter_mut().zip(emb_arr.iter()) {
                    *c = (*c * n + e) / (n + 1.0);
                }
                *count += 1;
                debug!(
                    segment = idx,
                    speaker = id,
                    similarity = format!("{:.4}", best_sim),
                    "Assigned to existing speaker"
                );
                id
            }
            None => {
                let id = next_id;
                clusters.insert(id, (emb_arr, 1));
                next_id += 1;
                debug!(segment = idx, speaker = id, "Created new speaker");
                id
            }
        };

        assignments.push(assigned);
    }

    debug!(
        embeddings = embeddings.len(),
        greedy_clusters = clusters.len(),
        "Greedy clustering complete"
    );

    // Post-clustering merge: merge centroids with cosine similarity above threshold
    merge_similar_centroids(&mut clusters, &mut assignments, CENTROID_MERGE_THRESHOLD);

    // If max_speakers is set, merge the most-similar centroids until we're at or below the limit.
    // Guard: a zero cap would collapse everything into one cluster — treat 0 as
    // no-limit (the save-time validator rejects it, but this is defense-in-depth
    // for configs arriving via sync or migration).
    if let Some(max) = max_speakers.filter(|&m| m > 0) {
        let max = max as usize;
        if clusters.len() > max {
            merge_to_limit(&mut clusters, &mut assignments, max);
        }
    }

    // Renumber surviving cluster IDs to 0, 1, 2, ... sequentially. Without
    // this, merging can leave gaps (e.g. clusters 0 and 4 survive → speakers
    // labeled "Speaker 1" and "Speaker 5", skipping 2/3/4). Sort the IDs so
    // the lowest original ID becomes 0 (stable across runs).
    let sorted_ids: Vec<usize> = {
        let mut ids: Vec<usize> = clusters.keys().copied().collect();
        ids.sort_unstable();
        ids
    };
    let remap: HashMap<usize, usize> = sorted_ids
        .iter()
        .enumerate()
        .map(|(new_id, &old_id)| (old_id, new_id))
        .collect();
    for a in assignments.iter_mut() {
        if let Some(&new_id) = remap.get(a) {
            *a = new_id;
        }
    }

    debug!(
        final_clusters = sorted_ids.len(),
        "Final cluster count after merges"
    );

    assignments
}

/// Deterministic enumeration of cluster pairs, ordered (min_id, max_id).
fn sorted_cluster_pairs(clusters: &HashMap<usize, (Array1<f32>, usize)>) -> Vec<(usize, usize)> {
    let mut ids: Vec<usize> = clusters.keys().copied().collect();
    ids.sort_unstable();
    let mut pairs = Vec::with_capacity(ids.len() * (ids.len() - 1) / 2);
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            pairs.push((ids[i], ids[j]));
        }
    }
    pairs
}

/// Merge cluster `from` into cluster `to`, combining centroids by member weight.
///
/// The merged centroid is `(to.centroid * to.weight + from.centroid * from.weight)
/// / (to.weight + from.weight)` and the merged weight is `to.weight + from.weight`,
/// so subsequent merges see the true combined centroid (fixes the D3 defect
/// where the absorbed cluster was deleted without updating the survivor).
fn merge_weighted(
    clusters: &mut HashMap<usize, (Array1<f32>, usize)>,
    assignments: &mut [usize],
    to: usize,
    from: usize,
) {
    let from_cluster = clusters.remove(&from).expect("absorbed cluster exists");
    let (to_centroid, to_weight) = clusters.get_mut(&to).expect("surviving cluster exists");
    let (from_centroid, from_weight) = from_cluster;

    let total = (*to_weight + from_weight) as f32;
    for (c, &f) in to_centroid.iter_mut().zip(from_centroid.iter()) {
        *c = (*c * *to_weight as f32 + f * from_weight as f32) / total;
    }
    *to_weight += from_weight;

    for a in assignments.iter_mut() {
        if *a == from {
            *a = to;
        }
    }

    debug!(
        surviving = to,
        absorbed = from,
        weight = *to_weight,
        "Weighted cluster merge"
    );
}

/// Merge centroids with cosine similarity above `merge_threshold`.
/// Reassigns affected segments to the surviving centroid.
fn merge_similar_centroids(
    clusters: &mut HashMap<usize, (Array1<f32>, usize)>,
    assignments: &mut [usize],
    merge_threshold: f32,
) {
    loop {
        let mut best_merge: Option<(usize, usize, f32)> = None; // (id_to, id_from, similarity)

        for (id_a, id_b) in sorted_cluster_pairs(clusters) {
            let sim = cosine_similarity(&clusters[&id_a].0, &clusters[&id_b].0);
            if sim > merge_threshold {
                // Deterministic tie-break: pairs are enumerated in sorted
                // (min_id, max_id) order and only strictly greater similarity
                // replaces the incumbent.
                if best_merge.is_none() || sim > best_merge.unwrap().2 {
                    best_merge = Some((id_a, id_b, sim));
                }
            }
        }

        match best_merge {
            Some((id_to, id_from, sim)) => {
                debug!(
                    speaker_a = id_to,
                    speaker_b = id_from,
                    similarity = format!("{:.4}", sim),
                    "Merging similar speaker centroids"
                );
                merge_weighted(clusters, assignments, id_to, id_from);
            }
            None => break,
        }
    }
}

/// Iteratively merge the two most-similar centroids until count <= target.
fn merge_to_limit(
    clusters: &mut HashMap<usize, (Array1<f32>, usize)>,
    assignments: &mut [usize],
    target: usize,
) {
    while clusters.len() > target {
        let mut best_merge: Option<(usize, usize, f32)> = None;

        for (id_a, id_b) in sorted_cluster_pairs(clusters) {
            let sim = cosine_similarity(&clusters[&id_a].0, &clusters[&id_b].0);
            if best_merge.is_none() || sim > best_merge.unwrap().2 {
                best_merge = Some((id_a, id_b, sim));
            }
        }

        match best_merge {
            Some((id_to, id_from, _sim)) => {
                merge_weighted(clusters, assignments, id_to, id_from);
            }
            None => break,
        }
    }
}

fn cosine_similarity(a: &Array1<f32>, b: &Array1<f32>) -> f32 {
    let dot = a.dot(b);
    let norm_a = a.dot(a).sqrt();
    let norm_b = b.dot(b).sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// ORT exit-abort investigation harness (2026-09-08 crash review).
    ///
    /// Two app crashes (2026-09-07, 2026-09-08) aborted inside a C++
    /// static destructor during process exit after sessions where
    /// diarization (ONNX Runtime) had run. FINDING: this standalone repro
    /// — full diarize() inference against the real pyannote models, then
    /// normal process exit — does NOT abort; the abort requires the
    /// updater-relaunch exit shape (tauri's restart calling exit(0) while
    /// the app's worker threads are still live), which a test binary
    /// cannot faithfully produce. Symbolication pinned the aborting frames
    /// to the ORT/kaldi-native-fbank C++ island (the only named C++
    /// symbols in the release binary bracket the crash region); the fix
    /// lives in src-tauri/src/commands/restart.rs (atexit guard +
    /// `_exit` on restart exits). This harness stays as the fastest way
    /// to re-check the ORT teardown story after any `ort` re-pin:
    ///
    ///     FERRISCRIBE_ORT_REPRO=<models dir> cargo test -p medical-stt-providers --lib ort_exit_repro -- --nocapture
    #[test]
    fn ort_exit_repro_builds_session_then_process_exits() {
        let Some(dir) = std::env::var_os("FERRISCRIBE_ORT_REPRO") else {
            eprintln!("skipping: set FERRISCRIBE_ORT_REPRO=<models dir> to run");
            return;
        };
        let root = PathBuf::from(&dir);
        let segmentation = root.join("pyannote").join("segmentation-3.0.onnx");
        let embedding = root
            .join("pyannote")
            .join("wespeaker_en_voxceleb_CAM++.onnx");
        if !segmentation.exists() || !embedding.exists() {
            eprintln!("skipping: no diarization models under {}", root.display());
            return;
        }
        // Full pipeline — inference, not just session construction: the
        // crashed app sessions ran diarize() (thread pools + EP state)
        // before exiting.
        let diarizer = SpeakerDiarizer::new(segmentation, embedding);
        // A quiet, constant tone with envelope changes — zeros alone get
        // filtered by the VAD and skip embedding inference entirely.
        let mut samples = vec![0i16; 16000 * 12];
        for (i, s) in samples.iter_mut().enumerate() {
            *s = ((i as f32 * 0.05).sin() * 8000.0) as i16;
        }
        match diarizer.diarize(&samples, 16000, None) {
            Ok(turns) => eprintln!("diarize returned {} turns", turns.len()),
            Err(e) => eprintln!("diarize error (repro continues): {e}"),
        }
        eprintln!("pipeline done; exiting normally next — watch for signal 6");
    }

    /// Multi-speaker eval: alternating speakers with NO intervening silence,
    /// including handoffs across the 10 s inference-window boundary.
    ///
    /// Fixture: TTS-rendered (macOS `say`, Daniel + Karen voices), hard-concatenated
    /// 22 s conversation:
    ///
    ///   spk_A  0.0–5.0   spk_B  5.0–9.7   spk_A  9.7–14.0 (spans window boundary)
    ///   spk_B  14.0–18.0 spk_A  18.0–22.0
    ///
    /// Proves the D1 fix: baseline boolean VAD yields 4 mixed-speaker segments
    /// whose embeddings cross-similarity is ~0.96 → ONE cluster (collapse).
    /// With dominant-slot change detection the pipeline recovers ≥2 speakers
    /// with turn boundaries within 0.7 s of ground truth.
    ///
    /// To regenerate the fixture:
    ///
    ///     say -v Daniel --file-format=AIFF -o spk_Daniel.aiff "<line>"
    ///     afconvert -f WAVE -d LEI16@16000 -c 1 spk_Daniel.aiff spk_Daniel.wav
    ///     # then hard-concatenate trimmed turns per tests/diar_eval/README.md
    ///
    ///     FERRISCRIBE_DIAR_EVAL=<dir with mix_two_speaker_nosilence.wav + models parent> \
    ///       cargo test -p medical-stt-providers --lib diar_eval -- --nocapture
    #[test]
    fn diar_eval_two_speaker_no_silence() {
        let Some(dir) = std::env::var_os("FERRISCRIBE_DIAR_EVAL") else {
            eprintln!("skipping: set FERRISCRIBE_DIAR_EVAL=<models dir> to run");
            return;
        };
        let root = PathBuf::from(&dir);
        let mix = root.join("mix_two_speaker_nosilence.wav");
        let models = root.join("pyannote");
        if !mix.exists()
            || !models.join("segmentation-3.0.onnx").exists()
            || !models.join("wespeaker_en_voxceleb_CAM++.onnx").exists()
        {
            eprintln!(
                "skipping: eval fixture or models missing under {}",
                root.display()
            );
            return;
        }

        // Decode WAV (16 kHz mono i16) without extra deps.
        let bytes = std::fs::read(&mix).expect("read mix wav");
        assert_eq!(&bytes[0..4], b"RIFF", "WAV header");
        assert_eq!(&bytes[8..12], b"WAVE", "WAVE tag");
        let mut pos = 12;
        let mut data: Option<(usize, usize)> = None;
        while pos + 8 <= bytes.len() {
            let id = &bytes[pos..pos + 4];
            let sz = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
            match id {
                b"fmt " => {
                    let fmt = u16::from_le_bytes(bytes[pos + 8..pos + 10].try_into().unwrap());
                    let ch = u16::from_le_bytes(bytes[pos + 10..pos + 12].try_into().unwrap());
                    let rate = u32::from_le_bytes(bytes[pos + 12..pos + 16].try_into().unwrap());
                    let bits = u16::from_le_bytes(bytes[pos + 22..pos + 24].try_into().unwrap());
                    assert_eq!(fmt, 1, "PCM");
                    assert_eq!(ch, 1, "mono");
                    assert_eq!(rate, 16000, "16 kHz");
                    assert_eq!(bits, 16, "16-bit");
                }
                b"data" => data = Some((pos + 8, sz)),
                _ => {}
            }
            pos += 8 + sz + (sz % 2); // chunks are word-aligned
        }
        let (doff, dsz) = data.expect("data chunk");
        let samples: Vec<i16> = bytes[doff..doff + dsz]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();
        eprintln!(
            "eval audio: {} samples ({:.1}s)",
            samples.len(),
            samples.len() as f64 / 16000.0
        );

        let diarizer = SpeakerDiarizer::new(
            models.join("segmentation-3.0.onnx"),
            models.join("wespeaker_en_voxceleb_CAM++.onnx"),
        );
        let turns = diarize_and_report(&diarizer, &samples);

        // Ground truth handoffs (seconds).
        let gt_boundaries = [5.0_f64, 9.7, 14.0, 18.0];
        let gt_speakers = 2;

        let unique: std::collections::HashSet<usize> = turns.iter().map(|t| t.speaker_id).collect();
        eprintln!(
            "eval result: {} turns, {} speakers: {:?}",
            turns.len(),
            unique.len(),
            turns
                .iter()
                .map(|t| (t.speaker_id, (t.start, t.end)))
                .collect::<Vec<_>>()
        );

        // D1 acceptance: the two alternating speakers must be separated.
        assert!(
            unique.len() >= gt_speakers,
            "D1 regression: multi-speaker audio collapsed to {} speaker(s); turns: {:?}",
            unique.len(),
            turns
        );

        // Boundary quality: each GT handoff must have a detected boundary
        // within 0.7 s, and turn count must be within 1 of GT (5 turns).
        let detected: Vec<f64> = turns
            .windows(2)
            .filter_map(|w| (w[0].speaker_id != w[1].speaker_id).then_some(w[1].start))
            .collect();
        for &gt in &gt_boundaries {
            let best = detected
                .iter()
                .map(|&d| (d - gt).abs())
                .fold(f64::INFINITY, f64::min);
            assert!(
                best <= 0.7,
                "GT handoff at {:.2}s has no detected boundary within 0.7s (detected: {:?})",
                gt,
                detected
            );
        }
        assert!(
            (turns.len() as i64 - 5).abs() <= 1,
            "expected ~5 turns, got {}: {:?}",
            turns.len(),
            turns
        );

        // Objective attribution accuracy: attribute each detected turn to the
        // GT interval it overlaps most (resolving the cluster-ID ↔ GT-speaker
        // permutation implicitly), then measure the fraction of GT speaking
        // time covered by correctly-attributed detected time.
        let duration_s = samples.len() as f64 / 16000.0;
        let gt_turns: Vec<(usize, f64, f64)> = {
            let mut v = Vec::with_capacity(gt_boundaries.len() + 1);
            let mut speaker = 0; // GT alternates A=0, B=1 starting at 0.0
            let mut prev = 0.0;
            for &b in gt_boundaries.iter().chain(std::iter::once(&duration_s)) {
                v.push((speaker, prev, b));
                prev = b;
                speaker = 1 - speaker;
            }
            v
        };
        let total_time: f64 = gt_turns.iter().map(|&(_, s, e)| e - s).sum();
        let mut correct_time = 0.0f64;
        for t in &turns {
            let attributed_speaker = gt_turns
                .iter()
                .map(|&(sp, s, e)| (sp, (t.end.min(e) - t.start.max(s)).max(0.0)))
                .max_by(|a, b| {
                    a.1.partial_cmp(&b.1)
                        .unwrap_or(Ordering::Equal)
                        .then(a.0.cmp(&b.0))
                })
                .map(|(sp, _)| sp)
                .unwrap_or(0);
            for &(sp, s, e) in &gt_turns {
                if sp == attributed_speaker {
                    correct_time += (t.end.min(e) - t.start.max(s)).max(0.0);
                }
            }
        }
        let accuracy = correct_time / total_time;
        eprintln!(
            "attribution accuracy: {:.1}% ({:.2}s correct of {:.2}s GT speech)",
            accuracy * 100.0,
            correct_time,
            total_time
        );
        assert!(
            accuracy >= 0.85,
            "attribution accuracy {:.1}% below 85% floor",
            accuracy * 100.0
        );

        // DER (250 ms collar, permutation-invariant matching, no-overlap
        // policy — see der_with_collar docs) reported alongside attribution;
        // ceiling chosen with headroom above the observed post-D1 value.
        let der = der_with_collar(&turns, &gt_turns, duration_s, 0.25);
        eprintln!("DER (250ms collar): {:.1}%", der * 100.0);
        assert!(der <= 0.10, "DER {:.1}% above 10% ceiling", der * 100.0);
    }

    /// Optimal-map DER (diarization error rate) on a 10 ms time grid with a
    /// collar around every GT turn boundary and permutation-invariant
    /// speaker matching (exhaustive hyp→GT mapping search; the fixture's
    /// speaker counts keep this trivially small).
    ///
    /// Overlap policy (declared): the fixture contains NO overlapping speech
    /// — turns are hard-concatenated — and the pipeline never emits
    /// overlapping turns; a grid point covered by more than one hypothesis
    /// turn would be attributed to the earliest-starting turn. Points within
    /// `collar_s` of a GT boundary are excluded from scoring entirely
    /// (neither credit nor error), forgiving boundary jitter.
    ///
    /// DER = (speaker confusion + missed detection + false alarm) / GT speech,
    /// each measured on the collared grid.
    fn der_with_collar(
        turns: &[SpeakerTurn],
        gt_turns: &[(usize, f64, f64)],
        duration_s: f64,
        collar_s: f64,
    ) -> f64 {
        const STEP: f64 = 0.01;
        let hyp_ids: Vec<usize> = {
            let mut v: Vec<usize> = turns.iter().map(|t| t.speaker_id).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let gt_speakers: Vec<usize> = {
            let mut v: Vec<usize> = gt_turns.iter().map(|&(s, _, _)| s).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        if hyp_ids.is_empty() {
            // No detected speech at all: everything missed.
            return 1.0;
        }
        let hyp_index = |id: usize| hyp_ids.iter().position(|&h| h == id).unwrap_or(0);

        // Internal GT boundaries only (0.0 / duration edges get no collar).
        let boundaries: Vec<f64> = gt_turns
            .iter()
            .flat_map(|&(_, s, e)| [s, e])
            .filter(|&b| b > 0.0 && b < duration_s)
            .collect();

        // Odometer over all functions hyp_ids -> gt_speakers.
        let mut choice = vec![0usize; hyp_ids.len()];
        let mut best_der = f64::INFINITY;
        loop {
            let mut confusion = 0.0_f64;
            let mut missed = 0.0_f64;
            let mut false_alarm = 0.0_f64;
            let mut total = 0.0_f64;
            let mut t = 0.0_f64;
            while t < duration_s {
                let collared = boundaries.iter().any(|&b| (t - b).abs() <= collar_s);
                if !collared {
                    let gt = gt_turns
                        .iter()
                        .find(|&&(_, s, e)| t >= s && t < e)
                        .map(|&(s, _, _)| s);
                    let hyp = turns
                        .iter()
                        .find(|tr| t >= tr.start && t < tr.end)
                        .map(|tr| tr.speaker_id);
                    match (gt, hyp) {
                        (Some(g), Some(h)) => {
                            total += STEP;
                            if gt_speakers[choice[hyp_index(h)]] != g {
                                confusion += STEP;
                            }
                        }
                        (Some(_), None) => {
                            total += STEP;
                            missed += STEP;
                        }
                        (None, Some(_)) => false_alarm += STEP,
                        (None, None) => {}
                    }
                }
                t += STEP;
            }
            if total > 0.0 {
                let der = (confusion + missed + false_alarm) / total;
                if der < best_der {
                    best_der = der;
                }
            }
            // Increment the odometer; terminate after the last mapping.
            let mut i = choice.len();
            loop {
                if i == 0 {
                    return best_der;
                }
                i -= 1;
                choice[i] += 1;
                if choice[i] < gt_speakers.len() {
                    break;
                }
                choice[i] = 0;
            }
        }
    }

    /// Shared helper for eval tests: run diarize and print the instrumented
    /// stage counts for before/after comparison.
    fn diarize_and_report(diarizer: &SpeakerDiarizer, samples: &[i16]) -> Vec<SpeakerTurn> {
        diarizer.diarize(samples, 16000, None).expect("diarize")
    }

    #[test]
    fn cosine_similarity_identical() {
        let a = Array1::from_vec(vec![1.0, 2.0, 3.0]);
        let sim = cosine_similarity(&a, &a);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = Array1::from_vec(vec![1.0, 0.0]);
        let b = Array1::from_vec(vec![0.0, 1.0]);
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-5);
    }

    #[test]
    fn cluster_single_speaker() {
        let emb = vec![1.0, 0.0, 0.0];
        let embeddings = vec![emb.clone(), emb.clone(), emb.clone()];
        let ids = cluster_speakers(&embeddings, 0.5, None);
        assert!(ids.iter().all(|&id| id == 0));
    }

    #[test]
    fn cluster_two_speakers() {
        let speaker_a = vec![1.0, 0.0, 0.0];
        let speaker_b = vec![0.0, 1.0, 0.0]; // orthogonal → different speaker
        let embeddings = vec![
            speaker_a.clone(),
            speaker_b.clone(),
            speaker_a.clone(),
            speaker_b.clone(),
        ];
        let ids = cluster_speakers(&embeddings, 0.5, None);
        assert_eq!(ids[0], ids[2]); // same speaker
        assert_eq!(ids[1], ids[3]); // same speaker
        assert_ne!(ids[0], ids[1]); // different speakers
    }

    #[test]
    fn cluster_centroid_updates_reduce_over_clustering() {
        // Speaker A's voice shifts across segments (simulating emotion/distance changes).
        // Without centroid updates, later segments would create new clusters.
        // With centroid updates, the running mean keeps them grouped.
        let a1 = vec![1.0, 0.1, 0.0];
        let a2 = vec![0.9, 0.2, 0.1]; // similar to a1
        let a3 = vec![0.8, 0.3, 0.15]; // drifting but still same speaker
        let a4 = vec![0.75, 0.35, 0.2]; // further drift
        let speaker_b = vec![0.0, 1.0, 0.0]; // clearly different speaker

        let embeddings = vec![a1, speaker_b.clone(), a2, a3, a4, speaker_b];
        let ids = cluster_speakers(&embeddings, 0.5, None);

        // All A segments should be the same speaker
        assert_eq!(ids[0], ids[2], "a1 and a2 should be same speaker");
        assert_eq!(ids[0], ids[3], "a1 and a3 should be same speaker");
        assert_eq!(ids[0], ids[4], "a1 and a4 should be same speaker");
        // B segments should be the same speaker
        assert_eq!(ids[1], ids[5], "b1 and b2 should be same speaker");
        // A and B should be different
        assert_ne!(ids[0], ids[1], "A and B should be different speakers");
    }

    #[test]
    fn cluster_max_speakers_caps_output() {
        // Create 4 distinct clusters, then cap at 2
        let embeddings = vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
            vec![0.0, 0.0, 0.0, 1.0],
        ];
        let ids = cluster_speakers(&embeddings, 0.5, Some(2));
        let unique: std::collections::HashSet<usize> = ids.iter().copied().collect();
        assert!(
            unique.len() <= 2,
            "Expected at most 2 speakers, got {}",
            unique.len()
        );
    }

    #[test]
    fn cluster_merge_similar_centroids() {
        // Two very similar embeddings that would normally be separate clusters
        // (similarity just above threshold but below merge threshold)
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.95, 0.05, 0.0]; // very similar to a (cosine ~0.999)
        let c = vec![0.0, 1.0, 0.0]; // orthogonal

        let embeddings = vec![a, b, c];
        let ids = cluster_speakers(&embeddings, 0.5, None);

        // a and b should be merged (similarity > 0.75 merge threshold)
        assert_eq!(ids[0], ids[1], "Similar embeddings should be merged");
        // c should be separate
        assert_ne!(ids[0], ids[2], "Dissimilar embedding should be separate");
    }

    /// D3 regression: `merge_weighted` must combine centroids by member weight,
    /// not delete the absorbed cluster without updating the survivor.
    #[test]
    fn merge_weighted_combines_centroids_by_weight() {
        let mut clusters: HashMap<usize, (Array1<f32>, usize)> = HashMap::new();
        clusters.insert(0, (Array1::from_vec(vec![0.9, 0.3]), 2)); // 2 members
        clusters.insert(1, (Array1::from_vec(vec![0.0, 1.0]), 1)); // 1 member
        let mut assignments = vec![0, 0, 1];

        merge_weighted(&mut clusters, &mut assignments, 0, 1);

        assert_eq!(clusters.len(), 1, "absorbed cluster must be removed");
        let (centroid, weight) = &clusters[&0];
        assert_eq!(*weight, 3, "weights must carry through the merge");
        // Weighted mean: (2*[0.9,0.3] + 1*[0,1]) / 3 = [0.6, 0.5333]
        assert!((centroid[0] - 0.6).abs() < 1e-6, "got {}", centroid[0]);
        assert!(
            (centroid[1] - 0.5333334).abs() < 1e-5,
            "got {}",
            centroid[1]
        );
        assert!(
            assignments.iter().all(|&a| a == 0),
            "assignments remapped to survivor"
        );
    }

    /// D3 regression: chained merges must use the weighted centroid from the
    /// previous merge, and pair selection must be deterministic.
    #[test]
    fn merge_to_limit_uses_weighted_centroids_in_chain() {
        let mut clusters: HashMap<usize, (Array1<f32>, usize)> = HashMap::new();
        clusters.insert(0, (Array1::from_vec(vec![1.0, 0.0]), 1));
        clusters.insert(1, (Array1::from_vec(vec![0.98, 0.05]), 1));
        clusters.insert(2, (Array1::from_vec(vec![0.0, 1.0]), 1));
        clusters.insert(3, (Array1::from_vec(vec![0.05, 0.98]), 1));
        let mut assignments = vec![0, 1, 2, 3];

        merge_to_limit(&mut clusters, &mut assignments, 2);

        assert_eq!(clusters.len(), 2);
        // Merge 1: (0,1) sim ~0.9987 — tie with (2,3); sorted-order tie-break
        // picks (0,1). Merge 2: (2,3). Weighted centroids:
        let (c01, w01) = &clusters[&0];
        assert_eq!(*w01, 2);
        assert!((c01[0] - 0.99).abs() < 1e-6, "got {}", c01[0]);
        assert!((c01[1] - 0.025).abs() < 1e-6, "got {}", c01[1]);
        let (c23, w23) = &clusters[&2];
        assert_eq!(*w23, 2);
        assert!((c23[0] - 0.025).abs() < 1e-6, "got {}", c23[0]);
        assert!((c23[1] - 0.99).abs() < 1e-6, "got {}", c23[1]);
        // A naive delete-without-update would leave centroid 0 at exactly
        // [1.0, 0.0]; the weighted value is [0.99, 0.025].
        assert!(
            (c01[1] - 0.025).abs() > 1e-4 || (c01[0] - 1.0).abs() > 1e-4,
            "sanity"
        );
    }

    /// D3 regression: deterministic output — same embeddings, same assignments.
    /// (HashMap iteration order must not influence merge order/tie-breaks.)
    #[test]
    fn cluster_merge_deterministic() {
        // Three clusters arranged so different merge orders give different
        // final assignments for a borderline probe.
        let embeddings = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![0.6, 0.6, 0.0],
            vec![0.0, 0.6, 0.6],
        ];
        let first = cluster_speakers(&embeddings, 0.5, Some(2));
        let second = cluster_speakers(&embeddings, 0.5, Some(2));
        assert_eq!(first, second, "clustering must be deterministic");
        let unique: std::collections::HashSet<usize> = first.iter().copied().collect();
        assert_eq!(unique.len(), 2, "max_speakers=2 must cap at 2");
    }

    #[test]
    fn diarizer_missing_models_returns_error() {
        let diarizer = SpeakerDiarizer::new(
            PathBuf::from("/nonexistent/seg.onnx"),
            PathBuf::from("/nonexistent/emb.onnx"),
        );
        let result = diarizer.diarize(&[0i16; 16000], 16000, None);
        assert!(result.is_err());
    }

    #[test]
    fn push_segment_if_valid_normal_case() {
        let mut segments = Vec::new();
        let samples = vec![0i16; 16000];
        push_segment_if_valid(&mut segments, 0.0, 8000, &samples, 16000.0);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start, 0.0);
        assert_eq!(segments[0].end, 0.5);
        assert_eq!(segments[0].samples.len(), 8000);
    }

    #[test]
    fn push_segment_if_valid_start_past_end() {
        let mut segments = Vec::new();
        let samples = vec![0i16; 16000];
        push_segment_if_valid(&mut segments, 10000.0, 5000, &samples, 16000.0);
        assert_eq!(segments.len(), 0);
    }

    #[test]
    fn push_segment_if_valid_start_past_buffer() {
        let mut segments = Vec::new();
        let samples = vec![0i16; 16000];
        push_segment_if_valid(&mut segments, 20000.0, 25000, &samples, 16000.0);
        assert_eq!(segments.len(), 0);
    }

    #[test]
    fn push_segment_if_valid_end_clamped() {
        let mut segments = Vec::new();
        let samples = vec![0i16; 16000];
        push_segment_if_valid(&mut segments, 8000.0, 20000, &samples, 16000.0);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].samples.len(), 8000);
    }

    #[test]
    fn push_segment_if_valid_empty_buffer() {
        let mut segments = Vec::new();
        let samples: Vec<i16> = Vec::new();
        push_segment_if_valid(&mut segments, 0.0, 100, &samples, 16000.0);
        assert_eq!(segments.len(), 0);
    }

    #[test]
    fn push_segment_if_valid_zero_length_segment() {
        let mut segments = Vec::new();
        let samples = vec![0i16; 16000];
        push_segment_if_valid(&mut segments, 5000.0, 5000, &samples, 16000.0);
        assert_eq!(segments.len(), 0);
    }

    /// **D5 regression**: zero max_speakers should be treated as no-limit,
    /// not collapse all clusters to one. Cluster two distinct embeddings with
    /// max_speakers=Some(0) — should preserve both speakers, not merge to one.
    #[test]
    fn cluster_speakers_zero_max_speakers_treated_as_no_limit() {
        // Two distinct embeddings (orthogonal vectors)
        let embeddings = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let threshold = 0.5;

        // With max_speakers=Some(0), should NOT collapse — treat as no-limit
        let ids_zero = cluster_speakers(&embeddings, threshold, Some(0));
        assert_eq!(
            ids_zero.len(),
            2,
            "zero max_speakers should not collapse clusters"
        );
        assert_ne!(
            ids_zero[0], ids_zero[1],
            "distinct embeddings should be distinct speakers"
        );

        // With max_speakers=None (explicit no-limit), same behavior
        let ids_none = cluster_speakers(&embeddings, threshold, None);
        assert_eq!(ids_none.len(), 2);
        assert_ne!(ids_none[0], ids_none[1]);

        // With max_speakers=Some(1), SHOULD collapse to one speaker
        let ids_one = cluster_speakers(&embeddings, threshold, Some(1));
        assert_eq!(ids_one.len(), 2); // still 2 assignments
        assert_eq!(
            ids_one[0], ids_one[1],
            "max_speakers=1 should collapse to one speaker"
        );
    }
}
