use medical_core::types::rag::RagResult;
use std::collections::HashMap;
use uuid::Uuid;

/// Combine multiple ranked result sets using Reciprocal Rank Fusion.
///
/// `k` is the RRF constant (typically 60). Each document's score is the sum
/// over all sets of `1 / (k + rank + 1)` where `rank` is 0-based.
pub fn reciprocal_rank_fusion(result_sets: &[Vec<RagResult>], k: f32) -> Vec<RagResult> {
    let mut scores: HashMap<Uuid, f32> = HashMap::new();
    let mut by_id: HashMap<Uuid, RagResult> = HashMap::new();

    for set in result_sets {
        for (rank, result) in set.iter().enumerate() {
            let id = result.chunk_id;
            let rrf_score = 1.0 / (k + rank as f32 + 1.0);
            *scores.entry(id).or_insert(0.0) += rrf_score;
            by_id.entry(id).or_insert_with(|| result.clone());
        }
    }

    let mut results: Vec<RagResult> = by_id
        .into_iter()
        .map(|(id, mut r)| {
            r.score = scores[&id];
            r
        })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

/// Combine vector, BM25, and graph result sets using explicit weights.
///
/// For each document the combined score is the sum of `score * weight` across
/// whichever sets contain it. Results are sorted by combined score descending.
pub fn weighted_fusion(
    vector_results: &[RagResult],
    bm25_results: &[RagResult],
    graph_results: &[RagResult],
    vector_weight: f32,
    bm25_weight: f32,
    graph_weight: f32,
) -> Vec<RagResult> {
    let mut scores: HashMap<Uuid, f32> = HashMap::new();
    let mut by_id: HashMap<Uuid, RagResult> = HashMap::new();

    let sets: [(&[RagResult], f32); 3] = [
        (vector_results, vector_weight),
        (bm25_results, bm25_weight),
        (graph_results, graph_weight),
    ];

    for (set, weight) in &sets {
        for result in set.iter() {
            let id = result.chunk_id;
            *scores.entry(id).or_insert(0.0) += result.score * weight;
            by_id.entry(id).or_insert_with(|| result.clone());
        }
    }

    let mut results: Vec<RagResult> = by_id
        .into_iter()
        .map(|(id, mut r)| {
            r.score = scores[&id];
            r
        })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::rag::{RagChunkMetadata, SearchSource};

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

    #[test]
    fn rrf_combines_two_sets() {
        // Result with id=1 appears at rank 0 in both sets — should have highest score
        let set_a = vec![make_result(1, 1.0, "doc 1"), make_result(2, 0.8, "doc 2")];
        let set_b = vec![make_result(1, 0.9, "doc 1"), make_result(3, 0.7, "doc 3")];

        let fused = reciprocal_rank_fusion(&[set_a, set_b], 60.0);
        assert!(!fused.is_empty());
        // id=1 appears at rank 0 in both sets, so it should be first
        assert_eq!(fused[0].chunk_id, Uuid::from_u128(1));
    }

    #[test]
    fn rrf_empty() {
        let result = reciprocal_rank_fusion(&[], 60.0);
        assert!(result.is_empty());

        let result2 = reciprocal_rank_fusion(&[vec![]], 60.0);
        assert!(result2.is_empty());
    }

    #[test]
    fn weighted_respects_weights() {
        // Only vector result with high weight — that result should win
        let vector = vec![make_result(1, 1.0, "vector doc")];
        let bm25 = vec![make_result(2, 1.0, "bm25 doc")];
        let graph: Vec<RagResult> = vec![];

        let fused = weighted_fusion(&vector, &bm25, &graph, 0.9, 0.1, 0.0);
        assert_eq!(fused.len(), 2);
        // id=1 score=0.9, id=2 score=0.1 — id=1 wins
        assert_eq!(fused[0].chunk_id, Uuid::from_u128(1));
    }

    #[test]
    fn weighted_combines_overlapping() {
        // id=1 appears in both vector and bm25
        let vector = vec![make_result(1, 0.8, "overlap doc")];
        let bm25 = vec![
            make_result(1, 0.6, "overlap doc"),
            make_result(2, 1.0, "other"),
        ];
        let graph: Vec<RagResult> = vec![];

        let fused = weighted_fusion(&vector, &bm25, &graph, 0.5, 0.5, 0.0);
        // id=1 combined: 0.8*0.5 + 0.6*0.5 = 0.7; id=2: 1.0*0.5 = 0.5
        assert_eq!(fused[0].chunk_id, Uuid::from_u128(1));
        let score_1 = fused[0].score;
        assert!((score_1 - 0.7).abs() < 1e-5, "score was {}", score_1);
    }
}
