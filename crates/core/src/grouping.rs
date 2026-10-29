//! The second half of the pipeline: from what was measured about each file
//! (fingerprints, capture times, provenance) plus the edges that do not
//! depend on any setting (exact copies, matched videos), decide which files
//! belong together.
//!
//! It is a pure function of its input and the thresholds, with no disk or
//! decoding. That is what lets a scan use it once at the end, and lets the
//! review use it again later on the saved analysis -- to preview a slider
//! position, or to apply one -- without scanning the folder again.

use crate::classify::{classify_pair, ClassifyInput, PairClassification};
use crate::cluster::merge_edges;
use crate::hash::Hash256;
use crate::keeper::{pick_keeper, score as keeper_score};
use crate::model::{FileId, FileRecord, MediaClassSerde};
use crate::pipeline::{Analyzed, Cluster};
use crate::provenance::{Evidence, Origin};
use crate::resolve::{detect_burst_runs, resolve_t1_edges, resolve_t1b_edges, split_runs_by_similarity, BurstRun};
use crate::thresholds::Thresholds;
use chrono::NaiveDateTime;
use std::collections::{HashMap, HashSet};

pub struct GroupingInput<'a> {
    /// Every file of the scan (exact copies can involve companions too).
    pub files: &'a [FileRecord],
    pub analyzed: &'a HashMap<FileId, Analyzed>,
    /// Files that are compared; companions of a primary ride along.
    pub primary_ids: &'a [FileId],
    /// Byte-identical pairs, which no setting changes.
    pub exact_edges: &'a [(FileId, FileId)],
    /// (derived, native) pairs from video matching, likewise fixed.
    pub video_edges: &'a [(FileId, FileId)],
}

pub struct Grouping {
    pub clusters: Vec<Cluster>,
    /// Review-only pairs; never auto-selectable.
    pub review_pairs: Vec<PairClassification>,
    pub burst_runs: Vec<BurstRun>,
}

pub fn group(input: &GroupingInput, t: &Thresholds) -> Grouping {
    let file_by_id: HashMap<FileId, &FileRecord> = input.files.iter().map(|f| (f.id, f)).collect();
    let analyzed = input.analyzed;

    let exact_group_of: HashMap<FileId, usize> = merge_edges(input.exact_edges.iter().copied())
        .into_iter()
        .enumerate()
        .flat_map(|(gi, members)| members.into_iter().map(move |m| (m, gi)))
        .collect();

    // --- Candidate search + classification, images only. ---
    let image_hash_entries: Vec<(FileId, Hash256)> = input
        .primary_ids
        .iter()
        .filter(|&&id| file_by_id.get(&id).is_some_and(|f| f.media_class == MediaClassSerde::Image))
        .filter_map(|&id| analyzed.get(&id).and_then(|a| a.phash).map(|h| (id, h)))
        .collect();
    let index = crate::search::BruteForceIndex::build(image_hash_entries);
    let candidate_pairs = index.all_pairs_within(t.match_distance);

    let pair_classifications: Vec<PairClassification> = candidate_pairs
        .into_iter()
        .filter(|(a, b, _)| {
            // Already an exact byte duplicate -> T0 handles it; skip here,
            // mirroring pass4_classify.py's identical skip.
            !matches!((exact_group_of.get(a), exact_group_of.get(b)), (Some(ga), Some(gb)) if ga == gb)
        })
        .map(|(a_id, b_id, dist)| {
            let a = analyzed.get(&a_id).unwrap();
            let b = analyzed.get(&b_id).unwrap();
            let ai = ClassifyInput { id: a_id, origin: a.provenance.origin, camera_model: a.camera_model.clone(), capture_time: a.capture_time };
            let bi = ClassifyInput { id: b_id, origin: b.provenance.origin, camera_model: b.camera_model.clone(), capture_time: b.capture_time };
            classify_pair(&ai, &bi, dist, t.sequence_gap_seconds)
        })
        .collect();

    let t1_image_edges = resolve_t1_edges(&pair_classifications);
    // A derived file that already resolved to a native original via T1
    // isn't "orphaned" -- T1b must not also merge it into an unrelated
    // cluster through a weaker derived-derived link. See resolve.rs's
    // doc comment on resolve_t1b_edges for the real discrepancy this
    // guards against.
    let already_t1_matched: HashSet<FileId> = t1_image_edges.iter().map(|&(derived, _, _)| derived).collect();
    let t1b_image_edges = resolve_t1b_edges(&pair_classifications, &already_t1_matched);

    // --- Union-find merge across every edge source, then keeper scoring
    // per cluster. See cluster.rs for why this must happen across ALL
    // edges together, not tier-by-tier. ---
    let mut tier_of_edge: HashMap<(FileId, FileId), &'static str> = HashMap::new();
    for &(a, b) in input.exact_edges {
        tier_of_edge.insert(norm(a, b), "T0");
    }
    for &(a, b, _) in &t1_image_edges {
        tier_of_edge.entry(norm(a, b)).or_insert("T1");
    }
    for &(a, b, _) in &t1b_image_edges {
        tier_of_edge.entry(norm(a, b)).or_insert("T1b");
    }
    for &(a, b) in input.video_edges {
        tier_of_edge.entry(norm(a, b)).or_insert("T1");
    }

    let all_edges: Vec<(FileId, FileId)> = input
        .exact_edges
        .iter()
        .copied()
        .chain(t1_image_edges.iter().map(|&(a, b, _)| (a, b)))
        .chain(t1b_image_edges.iter().map(|&(a, b, _)| (a, b)))
        .chain(input.video_edges.iter().copied())
        .collect();

    let clusters: Vec<Cluster> = merge_edges(all_edges)
        .into_iter()
        .map(|members| {
            let scored: Vec<(FileId, String, _)> = members
                .iter()
                .map(|&id| {
                    let rec = file_by_id[&id];
                    let ext = rec.path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    let (origin, pixels, has_gps) = match analyzed.get(&id) {
                        Some(a) => (
                            a.provenance.origin,
                            a.width.zip(a.height),
                            a.provenance.evidence.iter().any(|e| matches!(e, Evidence::GpsPresent)),
                        ),
                        None => (Origin::Unknown, None, false),
                    };
                    (id, rec.file_name.clone(), keeper_score(origin, pixels, rec.size_bytes, has_gps, ext))
                })
                .collect();
            let keeper = pick_keeper(&scored);
            let tier = cluster_tier(&members, &tier_of_edge);
            let reasons: Vec<&'static str> = pair_classifications
                .iter()
                .filter(|p| members.contains(&p.a) && members.contains(&p.b))
                .map(|p| p.reason)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            Cluster { tier, members, keeper, reasons }
        })
        .collect();

    // --- Burst runs: native images only, independent of pairwise
    // classification (see resolve.rs). ---
    let burst_input: Vec<(FileId, String, NaiveDateTime)> = input
        .primary_ids
        .iter()
        .filter(|&&id| file_by_id.get(&id).is_some_and(|f| f.media_class == MediaClassSerde::Image))
        .filter_map(|&id| {
            let a = analyzed.get(&id)?;
            if a.provenance.origin != Origin::CameraOriginal {
                return None;
            }
            Some((id, a.camera_model.clone()?, a.capture_time?))
        })
        .collect();
    // Time alone over-groups (see `thresholds::SEQUENCE_SIMILARITY`): keep only the
    // frames that also look like the same shot.
    let frame_hashes: HashMap<FileId, Hash256> = analyzed.iter().filter_map(|(&id, a)| a.phash.map(|h| (id, h))).collect();
    let burst_runs = split_runs_by_similarity(
        detect_burst_runs(&burst_input, t.sequence_gap_seconds),
        &frame_hashes,
        t.sequence_similarity,
    );

    let review_pairs = pair_classifications.into_iter().filter(|p| !p.tier.is_auto_selectable()).collect();

    Grouping { clusters, review_pairs, burst_runs }
}

/// How many entries each review tab would hold, so a setting can be
/// compared before and after without opening anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Counts {
    /// Exact-duplicate groups (T0 clusters).
    pub exact: usize,
    /// Other duplicate groups.
    pub duplicates: usize,
    pub sequences: usize,
    /// Photos inside those sequences.
    pub sequence_photos: usize,
    /// Look-alike pairs outside sequences.
    pub look_alikes: usize,
}

impl Grouping {
    pub fn counts(&self) -> Counts {
        let exact = self.clusters.iter().filter(|c| c.tier == "T0").count();
        Counts {
            exact,
            duplicates: self.clusters.len() - exact,
            sequences: self.burst_runs.len(),
            sequence_photos: self.burst_runs.iter().map(|r| r.files.len()).sum(),
            look_alikes: self.review_pairs.iter().filter(|p| p.tier != crate::classify::Tier::T2Burst).count(),
        }
    }
}

pub(crate) fn cluster_tier(members: &[FileId], tier_of_edge: &HashMap<(FileId, FileId), &'static str>) -> &'static str {
    members
        .iter()
        .enumerate()
        .flat_map(|(i, &a)| members[i + 1..].iter().map(move |&b| (a, b)))
        .filter_map(|(a, b)| tier_of_edge.get(&norm(a, b)))
        .copied()
        .max_by_key(|t| match *t {
            "T0" => 2,
            "T1" => 1,
            _ => 0,
        })
        .unwrap_or("T1b")
}

pub(crate) fn norm(a: FileId, b: FileId) -> (FileId, FileId) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_tier_finds_t0_edge_regardless_of_sort_order() {
        let mut edges = HashMap::new();
        edges.insert(norm(1, 2), "T0");
        assert_eq!(cluster_tier(&[1, 2], &edges), "T0");
    }

    #[test]
    fn cluster_tier_survives_edges_that_skip_every_sorted_adjacent_pair() {
        // Regression test for a real bug: a 6-member cluster whose real
        // edges are {1,4},{2,4},{2,6},{3,6},{3,5} -- none of which is a
        // sorted-adjacent pair ((1,2),(2,3),(3,4),(4,5),(5,6)). The old
        // `.windows(2)` implementation found zero matches here and
        // silently fell back to "T1b" even when the real edges were T1.
        let mut edges = HashMap::new();
        edges.insert(norm(1, 4), "T1");
        edges.insert(norm(2, 4), "T1");
        edges.insert(norm(2, 6), "T1");
        edges.insert(norm(3, 6), "T1");
        edges.insert(norm(3, 5), "T1");

        let members = vec![1, 2, 3, 4, 5, 6];
        assert_eq!(cluster_tier(&members, &edges), "T1");
    }

    #[test]
    fn cluster_tier_prefers_t0_over_t1_when_both_present() {
        let mut edges = HashMap::new();
        edges.insert(norm(1, 2), "T1");
        edges.insert(norm(2, 3), "T0");
        assert_eq!(cluster_tier(&[1, 2, 3], &edges), "T0");
    }

    #[test]
    fn cluster_tier_falls_back_to_t1b_only_when_truly_no_edge_matches() {
        let edges: HashMap<(FileId, FileId), &'static str> = HashMap::new();
        assert_eq!(cluster_tier(&[1, 2], &edges), "T1b");
    }
}
