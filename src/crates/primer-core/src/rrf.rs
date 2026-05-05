//! Reciprocal Rank Fusion (RRF).
//!
//! RRF is the de-facto industry standard for combining ranked lists
//! from heterogeneous retrievers (e.g. BM25 + dense-vector cosine).
//! For each list and each item, the contribution is `1 / (k + rank)`
//! (1-indexed rank). The combined score for an item is the sum of its
//! contributions across all lists in which it appears.
//!
//! Properties:
//! - **Scale-invariant.** Each list's scores are not used; only the
//!   ranks are. So BM25 (negative-rank scale) and cosine (-1..1 scale)
//!   fuse cleanly without normalisation.
//! - **List-length tolerant.** A list shorter than another simply
//!   contributes fewer items; absent items get zero contribution.
//! - **Tuning-free.** The constant `k = 60` works well across many
//!   IR domains and is the published default in the original RRF
//!   paper (Cormack et al., 2009). Smaller `k` weights the very top
//!   ranks more heavily; larger `k` flattens the curve.
//!
//! This module is generic over any hashable `Id` so both
//! `primer-knowledge` (passage ids) and `primer-storage` (turn ids)
//! can use it.

use std::collections::HashMap;
use std::hash::Hash;

/// Reciprocal Rank Fusion over an arbitrary number of ranked id lists.
///
/// Inputs:
/// - `ranked_lists`: an outer slice of inner slices. Each inner slice
///   is one retriever's ranked output, most-relevant-first.
/// - `k`: the RRF constant (60.0 is the published default).
///
/// Returns: a vec of `(id, fused_score)` pairs sorted descending by
/// fused score. Ties break by id-equality only — callers that need
/// stable tie-breaking should sort the inputs first.
///
/// Implementation note: ids are cloned into the output. This keeps
/// the API simple and is fine for retrieval-time use (top-K of two
/// 20-item lists is at most 40 ids of a few dozen bytes each).
pub fn fuse<Id>(ranked_lists: &[&[Id]], k: f64) -> Vec<(Id, f64)>
where
    Id: Eq + Hash + Clone,
{
    let mut scores: HashMap<Id, f64> = HashMap::new();
    for list in ranked_lists {
        for (rank, id) in list.iter().enumerate() {
            // 1-indexed rank: top item = rank 0 → contribution 1/(k+1).
            let contrib = 1.0 / (k + (rank as f64) + 1.0);
            *scores.entry(id.clone()).or_default() += contrib;
        }
    }
    let mut fused: Vec<(Id, f64)> = scores.into_iter().collect();
    fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    fused
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty_output() {
        let empty: Vec<&[String]> = vec![];
        let out = fuse::<String>(&empty, 60.0);
        assert!(out.is_empty());
    }

    #[test]
    fn single_list_preserves_order() {
        let list = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let out = fuse(&[&list], 60.0);
        let ids: Vec<&str> = out.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn item_in_both_lists_outranks_item_in_one() {
        // A appears top of list 1 and bottom of list 2; B appears only
        // top of list 2; C only top of list 1. A's combined score
        // should beat both B-alone and C-alone.
        let list1 = vec!["A".to_string(), "C".to_string()];
        let list2 = vec!["B".to_string(), "A".to_string()];
        let out = fuse(&[&list1, &list2], 60.0);
        let ids: Vec<&str> = out.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids[0], "A", "item appearing in both lists must rank first");
    }

    #[test]
    fn rank_one_item_scores_one_over_k_plus_one() {
        let list = vec!["x".to_string()];
        let out = fuse(&[&list], 60.0);
        let (_, score) = &out[0];
        let expected = 1.0 / 61.0;
        assert!(
            (score - expected).abs() < 1e-9,
            "rank-1 single-list score should be 1/(k+1); got {score}, expected {expected}"
        );
    }

    #[test]
    fn smaller_k_concentrates_score_at_the_top() {
        // With k = 0, the top item contributes 1/1 = 1.0 and the next
        // 1/2 = 0.5 — a 2x gap. With k = 60, top is 1/61 and next is
        // 1/62 — a much smaller gap.
        let list = vec!["a".to_string(), "b".to_string()];
        let small_k = fuse(&[&list], 0.0);
        let big_k = fuse(&[&list], 60.0);
        let small_ratio = small_k[0].1 / small_k[1].1;
        let big_ratio = big_k[0].1 / big_k[1].1;
        assert!(
            small_ratio > big_ratio,
            "smaller k must concentrate score at the top (small ratio {small_ratio} > big ratio {big_ratio})"
        );
    }

    #[test]
    fn lists_of_different_lengths_fuse_cleanly() {
        let short = vec!["x".to_string()];
        let long = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let out = fuse(&[&short, &long], 60.0);
        // No panics, no missing ids; all 5 distinct ids present.
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn integer_ids_work() {
        let l1 = vec![1, 2, 3];
        let l2 = vec![3, 4, 5];
        let out = fuse(&[&l1, &l2], 60.0);
        // 3 appears in both, should be first.
        assert_eq!(out[0].0, 3);
        assert_eq!(out.len(), 5);
    }
}
