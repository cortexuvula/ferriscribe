//! Speaker diarization using pyannote ONNX models via ort.
//!
//! Implements the pyannote pipeline directly:
//! 1. Voice Activity Detection / segmentation (pyannote segmentation-3.0 ONNX)
//! 2. Speaker embedding extraction (wespeaker CAM++ ONNX via knf-rs fbank features)
//! 3. Cosine-similarity speaker clustering

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use ndarray::{Array1, ArrayViewD, Axis, IxDyn};
use ort::session::Session;
use ort::value::{Tensor, TensorRef};
use tracing::{debug, info};

use medical_core::error::{AppError, AppResult};

/// A speaker turn: a contiguous time range attributed to one speaker.
#[derive(Debug, Clone)]
pub struct SpeakerTurn {
    pub speaker_id: usize,
    pub start: f64,
    pub end: f64,
}

/// A raw speech segment detected by VAD, with its audio samples.
struct SpeechSegment {
    start: f64,
    end: f64,
    samples: Vec<i16>,
}

/// Safely push a speech segment if it spans a non-empty range of real audio.
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
pub struct SpeakerDiarizer {
    segmentation_path: PathBuf,
    embedding_path: PathBuf,
}

impl SpeakerDiarizer {
    pub fn new(segmentation_path: PathBuf, embedding_path: PathBuf) -> Self {
        Self {
            segmentation_path,
            embedding_path,
        }
    }

    /// Run speaker diarization on 16 kHz mono i16 audio.
    ///
    /// Returns a list of speaker turns with start/end timestamps and speaker IDs.
    /// If models are missing or inference fails, returns an error.
    pub fn diarize(
        &self,
        samples_i16: &[i16],
        sample_rate: u32,
    ) -> AppResult<Vec<SpeakerTurn>> {
        info!(
            samples = samples_i16.len(),
            sample_rate,
            "Starting speaker diarization"
        );

        // Stage 1: Voice activity detection — find speech segments
        let segments = self.detect_speech_segments(samples_i16, sample_rate)?;
        info!(segments = segments.len(), "VAD found speech segments");
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

        // Stage 2: Extract speaker embeddings for each segment
        let embeddings = self.extract_embeddings(&segments)?;
        debug!(embeddings = embeddings.len(), "Extracted speaker embeddings");

        // Stage 3: Cluster embeddings into speakers
        let speaker_ids = cluster_speakers(&embeddings, 0.5);

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

        let num_speakers = speaker_ids.iter().max().map_or(0, |&m| m + 1);
        info!(
            turns = turns.len(),
            speakers = num_speakers,
            "Diarization complete"
        );

        Ok(turns)
    }

    /// Stage 1: Run pyannote segmentation model to detect speech segments.
    fn detect_speech_segments(
        &self,
        samples_i16: &[i16],
        sample_rate: u32,
    ) -> AppResult<Vec<SpeechSegment>> {
        if samples_i16.is_empty() {
            return Ok(Vec::new());
        }

        let mut session = Session::builder()
            .map_err(|e| AppError::SttProvider(format!("Failed to create segmentation session builder: {e}")))?
            .with_intra_threads(1)
            .map_err(|e| AppError::SttProvider(format!("Failed to set intra threads: {e}")))?
            .commit_from_file(&self.segmentation_path)
            .map_err(|e| AppError::SttProvider(format!("Failed to load segmentation model: {e}")))?;

        let window_size = (sample_rate * 10) as usize; // 10-second windows
        let frame_size: usize = 270;
        let frame_start: usize = 721;

        let sr_f64 = sample_rate as f64;

        let mut is_speeching = false;
        let mut offset: usize;
        let mut start_offset = 0.0_f64;
        let mut segments = Vec::new();

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

            // Reset offset to this window's starting sample position.
            offset = chunk_start + frame_start;

            // Convert i16 window to f32 for the model
            let window_f32: Vec<f32> = window.iter().map(|&s| s as f32).collect();

            // Shape: [1, 1, window_size]
            let input = TensorRef::from_array_view(
                ([1_usize, 1, window_f32.len()], &*window_f32),
            )
            .map_err(|e| AppError::SttProvider(format!("Failed to create input tensor: {e}")))?;

            let outputs = session
                .run(ort::inputs![input])
                .map_err(|e| AppError::SttProvider(format!("Segmentation inference failed: {e}")))?;

            let output = &outputs[0];
            let (shape, data) = output
                .try_extract_tensor::<f32>()
                .map_err(|e| AppError::SttProvider(format!("Failed to extract segmentation output: {e}")))?;

            let shape_slice: Vec<usize> = (0..shape.len()).map(|i| shape[i] as usize).collect();
            let view = ArrayViewD::<f32>::from_shape(IxDyn(&shape_slice), data)
                .map_err(|e| AppError::SttProvider(format!("Failed to reshape output: {e}")))?;

            // Iterate over frames in the output
            for row in view.outer_iter() {
                for sub_row in row.axis_iter(Axis(0)) {
                    // Find the class with highest activation
                    let max_index = sub_row
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(Ordering::Equal))
                        .map(|(idx, _)| idx)
                        .unwrap_or(0);

                    if max_index != 0 {
                        // Speech detected
                        if !is_speeching {
                            start_offset = offset as f64;
                            is_speeching = true;
                        }
                    } else if is_speeching {
                        // End of speech segment
                        push_segment_if_valid(
                            &mut segments,
                            start_offset,
                            offset,
                            samples_i16,
                            sr_f64,
                        );
                        is_speeching = false;
                    }
                    offset += frame_size;
                }
            }
        }

        // Flush final segment if still speaking at end.
        //
        // `start_offset` is set from the model's frame `offset`, which advances
        // across the zero-padded tail (we pad to a 10 s window boundary in the
        // padding block above). When a recording ends mid-speech, `start_offset`
        // can land past `samples_i16.len()` — past the real audio, inside the
        // padding — and `samples_i16[start_idx..]` panics. The helper drops
        // the segment when the open-speech region was wholly inside the pad.
        if is_speeching {
            push_segment_if_valid(
                &mut segments,
                start_offset,
                samples_i16.len(),
                samples_i16,
                sr_f64,
            );
        }

        Ok(segments)
    }

    /// Stage 2: Extract speaker embedding for each speech segment using the wespeaker model.
    fn extract_embeddings(
        &self,
        segments: &[SpeechSegment],
    ) -> AppResult<Vec<Vec<f32>>> {
        let mut session = Session::builder()
            .map_err(|e| AppError::SttProvider(format!("Failed to create embedding session builder: {e}")))?
            .with_intra_threads(1)
            .map_err(|e| AppError::SttProvider(format!("Failed to set intra threads: {e}")))?
            .commit_from_file(&self.embedding_path)
            .map_err(|e| AppError::SttProvider(format!("Failed to load embedding model: {e}")))?;

        let mut embeddings = Vec::with_capacity(segments.len());

        for seg in segments {
            // Convert i16 → f32 for knf-rs fbank computation
            let mut samples_f32 = vec![0.0f32; seg.samples.len()];
            knf_rs::convert_integer_to_float_audio(&seg.samples, &mut samples_f32);

            // Compute fbank features (80-dim Mel filterbank)
            // knf-rs returns ndarray 0.16 types; extract raw data to bridge to ort's ndarray 0.17
            let features = knf_rs::compute_fbank(&samples_f32)
                .map_err(|e| AppError::SttProvider(format!("fbank computation failed: {e}")))?;
            let feat_shape = features.shape().to_vec(); // [frames, 80]
            let feat_data = features.into_raw_vec_and_offset().0;

            // Reshape to [1, frames, 80] for batch dimension
            let input = Tensor::from_array(
                ([1_usize, feat_shape[0], feat_shape[1]], feat_data.into_boxed_slice()),
            )
            .map_err(|e| AppError::SttProvider(format!("Failed to create embedding input: {e}")))?;

            let outputs = session
                .run(ort::inputs!["feats" => input])
                .map_err(|e| AppError::SttProvider(format!("Embedding inference failed: {e}")))?;

            let emb_output = outputs
                .get("embs")
                .ok_or_else(|| AppError::SttProvider("Embedding model missing 'embs' output".to_string()))?;

            let (_, data) = emb_output
                .try_extract_tensor::<f32>()
                .map_err(|e| AppError::SttProvider(format!("Failed to extract embedding: {e}")))?;

            embeddings.push(data.to_vec());
        }

        Ok(embeddings)
    }
}

/// Stage 3: Cluster speaker embeddings using cosine similarity.
///
/// Greedy clustering: each embedding is compared against known speaker centroids.
/// If the best match exceeds `threshold`, the segment is assigned to that speaker;
/// otherwise a new speaker is created.
fn cluster_speakers(embeddings: &[Vec<f32>], threshold: f32) -> Vec<usize> {
    let mut centroids: HashMap<usize, Array1<f32>> = HashMap::new();
    let mut next_id: usize = 0;
    let mut assignments = Vec::with_capacity(embeddings.len());

    for (idx, emb) in embeddings.iter().enumerate() {
        let emb_arr = Array1::from_vec(emb.clone());

        let mut best_id = None;
        let mut best_sim = threshold;

        for (&id, centroid) in &centroids {
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
                debug!(segment = idx, speaker = id, similarity = format!("{:.4}", best_sim), "Assigned to existing speaker");
                id
            }
            None => {
                let id = next_id;
                centroids.insert(id, emb_arr);
                next_id += 1;
                debug!(segment = idx, speaker = id, "Created new speaker");
                id
            }
        };

        assignments.push(assigned);
    }

    assignments
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
        let ids = cluster_speakers(&embeddings, 0.5);
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
        let ids = cluster_speakers(&embeddings, 0.5);
        assert_eq!(ids[0], ids[2]); // same speaker
        assert_eq!(ids[1], ids[3]); // same speaker
        assert_ne!(ids[0], ids[1]); // different speakers
    }

    #[test]
    fn diarizer_missing_models_returns_error() {
        let diarizer = SpeakerDiarizer::new(
            PathBuf::from("/nonexistent/seg.onnx"),
            PathBuf::from("/nonexistent/emb.onnx"),
        );
        let result = diarizer.diarize(&[0i16; 16000], 16000);
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
}
