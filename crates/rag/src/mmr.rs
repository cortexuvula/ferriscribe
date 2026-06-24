use medical_core::types::rag::{RagResult, SearchSource};
use std::collections::HashSet;

/// Compute the cosine similarity between two embedding vectors.
///
/// Returns 0.0 for empty slices, mismatched lengths, or zero-norm vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

/// Compute word-level Jaccard similarity between two text strings.
///
/// Returns 1.0 when both strings are identical (or produce the same word set),
/// 0.0 when the word sets are disjoint.
pub fn jaccard_similarity(a: &str, b: &str) -> f32 {
    let words_a: HashSet<&str> = a.split_whitespace().collect();
    let words_b: HashSet<&str> = b.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        return 0.0;
    }

    intersection as f32 / union as f32
}

/// Re-rank a slice of [`RagResult`]s using Maximal Marginal Relevance (MMR).
///
/// `lambda` balances relevance (`1.0`) vs. diversity (`0.0`).
/// `top_k` limits the number of returned results.
///
/// The original relevance score from each result is used as the "query
/// similarity" proxy. Text-level Jaccard similarity is used to measure
/// pairwise similarity between already-selected documents.
///
/// All returned results have their `source` set to [`SearchSource::Fused`].
pub fn mmr_rerank(results: &[RagResult], lambda: f32, top_k: usize) -> Vec<RagResult> {
    if results.is_empty() || top_k == 0 {
        return Vec::new();
    }

    let n = results.len();
    let mut selected: Vec<usize> = Vec::with_capacity(top_k.min(n));
    let mut remaining: Vec<usize> = (0..n).collect();

    while selected.len() < top_k && !remaining.is_empty() {
        let best = remaining.iter().cloned().max_by(|&i, &j| {
            let mmr_i = mmr_score(results, i, &selected, lambda);
            let mmr_j = mmr_score(results, j, &selected, lambda);
            mmr_i
                .partial_cmp(&mmr_j)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if let Some(idx) = best {
            selected.push(idx);
            remaining.retain(|&x| x != idx);
        } else {
            break;
        }
    }

    selected
        .into_iter()
        .map(|idx| {
            let mut r = results[idx].clone();
            r.source = SearchSource::Fused;
            r
        })
        .collect()
}

/// Compute the MMR score for candidate `idx` given already-selected indices.
fn mmr_score(results: &[RagResult], idx: usize, selected: &[usize], lambda: f32) -> f32 {
    let relevance = results[idx].score;

    if selected.is_empty() {
        return lambda * relevance;
    }

    let max_sim = selected
        .iter()
        .map(|&s| jaccard_similarity(&results[idx].content, &results[s].content))
        .fold(f32::NEG_INFINITY, f32::max);

    lambda * relevance - (1.0 - lambda) * max_sim
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::rag::RagChunkMetadata;
    use uuid::Uuid;

    fn make_result(id: u128, score: f32, content: &str) -> RagResult {
        RagResult {
            chunk_id: Uuid::from_u128(id),
            document_id: Uuid::from_u128(0),
            content: content.to_string(),
            score,
            source: SearchSource::Vector,
            metadata: RagChunkMetadata {
                document_title: None,
                chunk_index: 0,
                total_chunks: 1,
                page_number: None,
            },
        }
    }

    // ---- cosine_similarity ----

    #[test]
    fn cosine_identical() {
        let v = vec![1.0_f32, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-5, "expected ~1.0, got {sim}");
    }

    #[test]
    fn cosine_orthogonal() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 1e-5, "expected ~0.0, got {sim}");
    }

    #[test]
    fn cosine_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
    }

    // ---- jaccard_similarity ----

    #[test]
    fn jaccard_identical() {
        let sim = jaccard_similarity("the quick brown fox", "the quick brown fox");
        assert!((sim - 1.0).abs() < 1e-5, "expected ~1.0, got {sim}");
    }

    #[test]
    fn jaccard_partial() {
        // "the fox" ∩ "the dog" = {"the"}; union = {"the", "fox", "dog"} → 1/3
        let sim = jaccard_similarity("the fox", "the dog");
        assert!((sim - 1.0 / 3.0).abs() < 1e-5, "expected ~0.333, got {sim}");
    }

    #[test]
    fn jaccard_disjoint() {
        let sim = jaccard_similarity("apple banana", "cherry grape");
        assert!((sim - 0.0).abs() < 1e-5, "expected ~0.0, got {sim}");
    }

    // ---- mmr_rerank ----

    #[test]
    fn mmr_selects_top_k() {
        let results = vec![
            make_result(1, 0.9, "alpha beta gamma"),
            make_result(2, 0.8, "delta epsilon zeta"),
            make_result(3, 0.7, "eta theta iota"),
            make_result(4, 0.6, "kappa lambda mu"),
        ];
        let reranked = mmr_rerank(&results, 0.7, 3);
        assert_eq!(reranked.len(), 3);
    }

    #[test]
    fn mmr_favors_diversity() {
        // Two near-duplicate high-score results and one diverse lower-score result.
        // With lambda < 1.0 the diverse result should be preferred over the duplicate.
        let results = vec![
            make_result(1, 0.9, "hypertension blood pressure heart"),
            make_result(2, 0.85, "hypertension blood pressure heart"), // near duplicate of 1
            make_result(3, 0.7, "diabetes insulin glucose"),           // diverse
        ];
        let reranked = mmr_rerank(&results, 0.5, 3);
        // id=1 selected first (highest score). Next: diversity should prefer id=3 over id=2.
        assert_eq!(reranked.len(), 3);
        assert_eq!(reranked[0].chunk_id, Uuid::from_u128(1));
        // id=3 should rank above id=2 because it's diverse
        let pos_2 = reranked
            .iter()
            .position(|r| r.chunk_id == Uuid::from_u128(2))
            .unwrap();
        let pos_3 = reranked
            .iter()
            .position(|r| r.chunk_id == Uuid::from_u128(3))
            .unwrap();
        assert!(
            pos_3 < pos_2,
            "diverse result (pos {pos_3}) should beat duplicate (pos {pos_2})"
        );
    }

    #[test]
    fn mmr_empty() {
        assert!(mmr_rerank(&[], 0.7, 5).is_empty());
    }

    #[test]
    fn mmr_top_k_larger_than_results() {
        let results = vec![make_result(1, 0.9, "alpha"), make_result(2, 0.8, "beta")];
        let reranked = mmr_rerank(&results, 0.7, 10);
        assert_eq!(reranked.len(), 2);
    }

    #[test]
    fn mmr_sets_source_to_fused() {
        let results = vec![make_result(1, 0.9, "some content")];
        let reranked = mmr_rerank(&results, 0.7, 1);
        assert_eq!(reranked[0].source, SearchSource::Fused);
    }
}
