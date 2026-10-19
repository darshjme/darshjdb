//! Pure rank fusion for a future hybrid memory-recall adapter.
//! No retrieval, database access, embeddings, or score normalization happens here.
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHit {
    pub id: String,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FusedHit {
    pub id: String,
    pub rrf_score: f64,
    pub text_rank: Option<usize>,
    pub vector_rank: Option<usize>,
    pub text_score: Option<f64>,
    pub vector_score: Option<f64>,
}

/// Fuse independently ranked lists using `sum(1 / (k + one_based_rank))`.
///
/// Each ID contributes once per branch, at its earliest input position. Duplicate
/// slots still consume positions; the earliest raw score is preserved for inspection.
/// Raw scores (including NaN) never affect ranking. Exact fused-score ties use ID
/// ascending. Non-finite or negative `k` becomes zero; finite positive `k` is kept.
/// Very large finite `k` can make different ranks indistinguishable at f64 precision.
/// Results contain only supplied IDs, and are truncated after fusion.
pub fn fuse_ranks(text: &[MemoryHit], vector: &[MemoryHit], k: f64, limit: usize) -> Vec<FusedHit> {
    if limit == 0 {
        return Vec::new();
    }
    let k = if k.is_finite() && k >= 0.0 { k } else { 0.0 };
    let mut hits: HashMap<&str, FusedHit> = HashMap::new();
    for (rows, is_text) in [(text, true), (vector, false)] {
        for (index, hit) in rows.iter().enumerate() {
            let entry = hits.entry(&hit.id).or_insert_with(|| FusedHit {
                id: hit.id.clone(),
                rrf_score: 0.0,
                text_rank: None,
                vector_rank: None,
                text_score: None,
                vector_score: None,
            });
            let (rank, score) = if is_text {
                (&mut entry.text_rank, &mut entry.text_score)
            } else {
                (&mut entry.vector_rank, &mut entry.vector_score)
            };
            if rank.is_none() {
                let position = index + 1;
                *rank = Some(position);
                *score = Some(hit.score);
                entry.rrf_score += 1.0 / (k + position as f64);
            }
        }
    }
    let mut result: Vec<_> = hits.into_values().collect();
    result.sort_by(|a, b| {
        b.rrf_score
            .total_cmp(&a.rrf_score)
            .then_with(|| a.id.cmp(&b.id))
    });
    result.truncate(limit);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hit(id: &str, score: f64) -> MemoryHit {
        MemoryHit {
            id: id.into(),
            score,
        }
    }
    #[test]
    fn symmetric_ranks_tie_by_id_not_raw_score() {
        let result = fuse_ranks(
            &[hit("b", 1000.0), hit("a", -1.0)],
            &[hit("a", 0.0), hit("b", 9000.0)],
            60.0,
            10,
        );
        assert_eq!(
            result.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(result[0].rrf_score, 1.0 / 61.0 + 1.0 / 62.0);
    }
    #[test]
    fn duplicates_keep_first_position_and_score_without_extra_votes() {
        let result = fuse_ranks(
            &[hit("a", 2.0), hit("a", 99.0), hit("b", 3.0)],
            &[hit("a", 4.0), hit("a", 88.0), hit("c", 5.0)],
            0.0,
            10,
        );
        let a = &result[0];
        assert_eq!(
            (a.text_rank, a.vector_rank, a.text_score, a.vector_score),
            (Some(1), Some(1), Some(2.0), Some(4.0))
        );
        assert_eq!(a.rrf_score, 2.0);
        assert_eq!(result[1].text_rank, Some(3));
        assert_eq!(result[2].vector_rank, Some(3));
    }
    #[test]
    fn empty_branches_and_limit_boundaries() {
        assert!(fuse_ranks(&[], &[], 60.0, 10).is_empty());
        let rows = [hit("a", 1.0), hit("b", 0.5)];
        assert!(fuse_ranks(&rows, &rows, 60.0, 0).is_empty());
        assert_eq!(fuse_ranks(&rows, &[], 60.0, 1).len(), 1);
        let vector = fuse_ranks(&[], &rows, 60.0, 99);
        assert_eq!(vector.len(), 2);
        assert_eq!(
            (vector[0].text_rank, vector[0].vector_rank),
            (None, Some(1))
        );
        assert_eq!(fuse_ranks(&rows, &[], 60.0, 99)[0].vector_rank, None);
    }
    #[test]
    fn invalid_k_is_zero_and_nonfinite_raw_scores_are_only_metadata() {
        let rows = [hit("b", f64::NAN), hit("a", f64::INFINITY)];
        for k in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let result = fuse_ranks(&rows, &[], k, 2);
            assert_eq!(result[0].id, "b");
            assert_eq!(result[0].rrf_score, 1.0);
            assert!(result[0].text_score.unwrap().is_nan());
        }
        let result = fuse_ranks(&rows, &[], f64::MAX, 2);
        assert!(
            result
                .iter()
                .all(|h| h.rrf_score.is_finite() && h.rrf_score > 0.0)
        );
    }
}
