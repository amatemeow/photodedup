//! Union-find merge of overlapping groups into single clusters. Port of the
//! Python prototype's fix (`pass6_keeper.py`, not part of this repo) for a real
//! bug class: a file can be an exact byte-dup of one file *and* a perceptual
//! dup of another (e.g. two independent messenger re-saves of the same
//! original). Without merging connected components across all edges first,
//! that produces overlapping groups where a file is "keeper" in one group
//! and "to quarantine" in another at the same time. The MVP verified zero
//! file appeared in >1 final group after this fix; `/ROADMAP.md`'s
//! verification plan keeps that as a standing invariant.

use crate::model::FileId;
use std::collections::HashMap;

pub struct UnionFind {
    parent: HashMap<FileId, FileId>,
}

impl UnionFind {
    pub fn new() -> Self {
        UnionFind {
            parent: HashMap::new(),
        }
    }

    fn find(&mut self, x: FileId) -> FileId {
        let p = *self.parent.entry(x).or_insert(x);
        if p == x {
            x
        } else {
            let root = self.find(p);
            self.parent.insert(x, root);
            root
        }
    }

    pub fn union(&mut self, a: FileId, b: FileId) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent.insert(ra, rb);
        }
    }

    /// Connected components among every `FileId` ever passed to `union`,
    /// as root -> sorted member list.
    pub fn components(&mut self) -> HashMap<FileId, Vec<FileId>> {
        let ids: Vec<FileId> = self.parent.keys().copied().collect();
        let mut out: HashMap<FileId, Vec<FileId>> = HashMap::new();
        for id in ids {
            let root = self.find(id);
            out.entry(root).or_default().push(id);
        }
        for members in out.values_mut() {
            members.sort();
        }
        out
    }
}

impl Default for UnionFind {
    fn default() -> Self {
        Self::new()
    }
}

/// Merge a set of pairwise edges (e.g. every `T0`/`T1`/`T1b` pair) into
/// clusters. Each returned cluster's members are sorted `FileId`s.
pub fn merge_edges(edges: impl IntoIterator<Item = (FileId, FileId)>) -> Vec<Vec<FileId>> {
    let mut uf = UnionFind::new();
    for (a, b) in edges {
        uf.union(a, b);
    }
    let mut clusters: Vec<Vec<FileId>> = uf.components().into_values().collect();
    clusters.sort_by(|a, b| a.first().cmp(&b.first()));
    clusters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitively_connected_files_merge_into_one_cluster() {
        // a--b (exact dup), b--c (perceptual dup) -> one cluster {a,b,c},
        // even though a and c were never directly compared.
        let clusters = merge_edges([(0, 1), (1, 2)]);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0], vec![0, 1, 2]);
    }

    #[test]
    fn disjoint_edges_stay_separate_clusters() {
        let mut clusters = merge_edges([(0, 1), (5, 6)]);
        clusters.sort();
        assert_eq!(clusters, vec![vec![0, 1], vec![5, 6]]);
    }

    #[test]
    fn no_file_appears_in_more_than_one_cluster() {
        // Invariant from the plan's Verification section: union-find over
        // any edge set must never let one id land in two components.
        let clusters = merge_edges([(0, 1), (2, 3), (1, 4), (5, 2)]);
        let mut seen = std::collections::HashSet::new();
        for cluster in &clusters {
            for &id in cluster {
                assert!(seen.insert(id), "file {id} appeared in more than one cluster");
            }
        }
    }
}
