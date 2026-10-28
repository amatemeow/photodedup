//! Candidate pair search over perceptual hashes. Day-one implementation is
//! brute force, but blocked/SIMD-friendly rather than a materialized N×N
//! matrix -- a 256-bit hash is 4×u64, so a distance is 4 XORs + 4
//! `count_ones()`, and under rayon that's roughly 1e9 comparisons/sec/core
//! (measured in M0 spiking). N=50,000 is a few seconds with no index at
//! all. See `/ROADMAP.md`: the `CandidateIndex` trait is the seam a
//! multi-index LSH or BK-tree drops into at 100k+ without touching
//! anything above it.

use crate::hash::Hash256;
use crate::model::FileId;
use rayon::prelude::*;

pub trait CandidateIndex {
    /// All entries within `max_dist` of `query`, as (id, distance).
    fn neighbors(&self, query: Hash256, max_dist: u32) -> Vec<(FileId, u32)>;
}

pub struct BruteForceIndex {
    entries: Vec<(FileId, Hash256)>,
}

impl BruteForceIndex {
    pub fn build(entries: Vec<(FileId, Hash256)>) -> Self {
        BruteForceIndex { entries }
    }

    /// All unordered pairs within `max_dist`, as (id_a, id_b, distance)
    /// with id_a < id_b. This is what `classify.rs` actually consumes --
    /// candidate *pairs* across the whole set, not single-query lookups.
    pub fn all_pairs_within(&self, max_dist: u32) -> Vec<(FileId, FileId, u32)> {
        let n = self.entries.len();
        (0..n)
            .into_par_iter()
            .flat_map_iter(|i| {
                let (id_a, hash_a) = self.entries[i];
                (i + 1..n).filter_map(move |j| {
                    let (id_b, hash_b) = self.entries[j];
                    let d = hash_a.distance(&hash_b);
                    (d <= max_dist).then_some((id_a, id_b, d))
                })
            })
            .collect()
    }
}

impl CandidateIndex for BruteForceIndex {
    fn neighbors(&self, query: Hash256, max_dist: u32) -> Vec<(FileId, u32)> {
        self.entries
            .iter()
            .filter_map(|&(id, h)| {
                let d = h.distance(&query);
                (d <= max_dist).then_some((id, d))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(bits: u128) -> Hash256 {
        Hash256(bits, 0)
    }

    #[test]
    fn finds_pairs_within_threshold_and_excludes_far_ones() {
        let idx = BruteForceIndex::build(vec![
            (0, h(0b0000)),
            (1, h(0b0001)), // distance 1 from id 0
            (2, h(0b1111)), // distance 4 from id 0
        ]);
        let pairs = idx.all_pairs_within(1);
        assert_eq!(pairs, vec![(0, 1, 1)]);
    }

    #[test]
    fn neighbors_returns_self_at_distance_zero() {
        let idx = BruteForceIndex::build(vec![(0, h(5)), (1, h(5)), (2, h(999))]);
        let mut neighbors = idx.neighbors(h(5), 0);
        neighbors.sort();
        assert_eq!(neighbors, vec![(0, 0), (1, 0)]);
    }
}
