//! Turns raw candidate pairs into the clean edges/runs that actually feed
//! `cluster::merge_edges` and the UI. Port of the
//! Python prototype's `pass4b_resolve.py` (not part of this repo): argmin resolution per file
//! (so a weak, borderline candidate pair surviving the match-distance setting's
//! generous net can't merge two genuinely unrelated files -- only each
//! file's single *closest* match becomes an edge) and independent
//! burst-run detection by time+camera sequence.

use crate::classify::{PairClassification, Tier};
use crate::hash::Hash256;
use crate::model::FileId;
use crate::provenance::Origin;
use chrono::NaiveDateTime;
use std::collections::{HashMap, HashSet};

/// `T1` pairs (native vs. derived): keep only each `Derived`-side file's
/// single closest native match. Mirrors `pass4b_resolve.py`'s
/// `by_messenger` argmin -- the direction is always derived-file-keyed
/// since that's the side with ambiguity (which original is this a copy
/// of?); the native side has no analogous ambiguity to resolve.
pub fn resolve_t1_edges(pairs: &[PairClassification]) -> Vec<(FileId, FileId, u32)> {
    let mut best: HashMap<FileId, (FileId, u32)> = HashMap::new();
    for p in pairs.iter().filter(|p| p.tier == Tier::T1) {
        let (derived, native) = if p.a_origin == Origin::Derived {
            (p.a, p.b)
        } else {
            (p.b, p.a)
        };
        best.entry(derived)
            .and_modify(|cur| {
                if p.phash_dist < cur.1 {
                    *cur = (native, p.phash_dist);
                }
            })
            .or_insert((native, p.phash_dist));
    }
    best.into_iter().map(|(derived, (native, d))| (derived, native, d)).collect()
}

/// `T1b` pairs (derived vs. derived, no surviving original): both sides
/// are symmetric, so argmin-resolve from each file's own perspective and
/// take the union of best-edges. A derived file that's genuinely the
/// closest match for two *different* other derived files (three re-saves
/// of the same original circulating) legitimately chains all three
/// together through `cluster::merge_edges` -- that's correct, not
/// over-merging, since it only happens when each pairwise link is itself
/// each side's closest match.
///
/// `already_t1_matched`: derived-side `FileId`s that already resolved to
/// a native original via `resolve_t1_edges`. A pair where *either* side is
/// in this set is skipped entirely -- found via a real discrepancy against
/// the reference dataset: two messenger copies of two different,
/// burst-adjacent native photos (1s apart, same camera) can themselves
/// hash close enough to pass as `T1b` candidates (the same burst-vs-
/// duplicate ambiguity the native-native structural rule exists to
/// prevent, just unguarded on the derived side, since derived files
/// usually lack the timestamp that rule relies on). `T1b`'s whole premise
/// is *orphaned* derived copies with no surviving original; a file that
/// already has one via `T1` isn't orphaned, so it must not also merge
/// into an unrelated cluster through a weaker `T1b` link.
pub fn resolve_t1b_edges(
    pairs: &[PairClassification],
    already_t1_matched: &HashSet<FileId>,
) -> Vec<(FileId, FileId, u32)> {
    let mut best: HashMap<FileId, (FileId, u32)> = HashMap::new();
    for p in pairs.iter().filter(|p| p.tier == Tier::T1b) {
        if already_t1_matched.contains(&p.a) || already_t1_matched.contains(&p.b) {
            continue;
        }
        for (me, other) in [(p.a, p.b), (p.b, p.a)] {
            best.entry(me)
                .and_modify(|cur| {
                    if p.phash_dist < cur.1 {
                        *cur = (other, p.phash_dist);
                    }
                })
                .or_insert((other, p.phash_dist));
        }
    }
    let mut edges: Vec<(FileId, FileId, u32)> = best
        .into_iter()
        .map(|(me, (other, d))| if me < other { (me, other, d) } else { (other, me, d) })
        .collect();
    edges.sort();
    edges.dedup();
    edges
}

/// Two frames of a time-run belong to the same sequence only if their
/// perceptual hashes differ by at most this many of 256 bits.
///
/// Why this exists: a run found by timestamps alone is just "photos taken
/// within the sequence time gap of each other", and on the reference
/// dataset a quarter of the consecutive frames inside such runs were at
/// chance-level difference (110+ bits: unrelated shots). Thinning "the
/// sharpest of the run" across a portrait, a full-body shot and a
/// different pose would be misleading, so each run is split into groups
/// of frames that look like the same shot.
///
/// Calibrated by eye on contact sheets of real runs, not derived: there is
/// no natural gap in the distances (they form a continuum from 0 to 150).
/// At 80 a nine-frame run of one pose stayed whole, a full-body shot and a
/// waist-up portrait on the same stairs split apart, and a wide shot plus
/// five different portrait poses fragmented (as intended: different poses
/// are different groups). At 60 same-pose runs over-split; past 90 the
/// threshold nears the 100-134 range of unrelated photos. Erring toward
/// over-splitting is the cheap direction: a frame left alone is simply
/// kept.
pub const BURST_VISUAL_MAX_DIST: u32 = crate::thresholds::SEQUENCE_SIMILARITY.default as u32;

/// Refines time-based runs into groups that also look alike. Single-linkage
/// within each run (a frame joins a group if it is within `max_dist` of any
/// member), so a slowly changing pose stays together while a different
/// subject or framing does not. Groups keep time order and need two or more
/// frames; a frame with no hash cannot be compared, so it is left out (and
/// therefore never offered for removal).
pub fn split_runs_by_similarity(runs: Vec<BurstRun>, hashes: &HashMap<FileId, Hash256>, max_dist: u32) -> Vec<BurstRun> {
    let mut out = Vec::new();
    for run in runs {
        let frames: Vec<(FileId, Hash256)> = run.files.iter().filter_map(|&f| hashes.get(&f).map(|&h| (f, h))).collect();

        let mut parent: Vec<usize> = (0..frames.len()).collect();
        fn find(parent: &mut [usize], mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        for i in 0..frames.len() {
            for j in i + 1..frames.len() {
                if frames[i].1.distance(&frames[j].1) <= max_dist {
                    let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                    parent[ri] = rj;
                }
            }
        }

        // Groups in order of their first frame; members already in time order.
        let mut groups: Vec<Vec<FileId>> = Vec::new();
        let mut group_of_root: HashMap<usize, usize> = HashMap::new();
        for i in 0..frames.len() {
            let root = find(&mut parent, i);
            let g = *group_of_root.entry(root).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[g].push(frames[i].0);
        }
        out.extend(groups.into_iter().filter(|g| g.len() >= 2).map(|files| BurstRun {
            camera_model: run.camera_model.clone(),
            files,
        }));
    }
    out
}

#[derive(Debug, Clone)]
pub struct BurstRun {
    pub camera_model: String,
    pub files: Vec<FileId>, // in time order
}

/// Independent of pairwise classification: sorts each camera's native
/// images by capture time and splits into runs wherever the gap exceeds
/// `gap_seconds`. A run of 1 isn't a burst by definition.
pub fn detect_burst_runs(images: &[(FileId, String, NaiveDateTime)], gap_seconds: i64) -> Vec<BurstRun> {
    let mut by_camera: HashMap<&str, Vec<(NaiveDateTime, FileId)>> = HashMap::new();
    for (id, camera, t) in images {
        by_camera.entry(camera.as_str()).or_default().push((*t, *id));
    }

    let mut runs = Vec::new();
    for (camera, mut items) in by_camera {
        items.sort_by_key(|(t, _)| *t);
        let mut run: Vec<(NaiveDateTime, FileId)> = vec![items[0]];
        for &(t, id) in &items[1..] {
            let gap = (t - run.last().unwrap().0).num_seconds();
            if gap <= gap_seconds {
                run.push((t, id));
            } else {
                if run.len() >= 2 {
                    runs.push(BurstRun {
                        camera_model: camera.to_string(),
                        files: run.iter().map(|(_, id)| *id).collect(),
                    });
                }
                run = vec![(t, id)];
            }
        }
        if run.len() >= 2 {
            runs.push(BurstRun {
                camera_model: camera.to_string(),
                files: run.iter().map(|(_, id)| *id).collect(),
            });
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hash that is exactly `bits` bits away from `h(0)`.
    fn h(bits: u32) -> Hash256 {
        let ones = |n: u32| if n == 0 { 0 } else { u128::MAX >> (128 - n) };
        Hash256(ones(bits.min(128)), ones(bits.saturating_sub(128)))
    }
    fn run(files: &[FileId]) -> BurstRun {
        BurstRun { camera_model: "Cam".into(), files: files.to_vec() }
    }
    fn split(files: &[(FileId, Option<u32>)]) -> Vec<Vec<FileId>> {
        let hashes: HashMap<FileId, Hash256> = files.iter().filter_map(|&(id, b)| b.map(|b| (id, h(b)))).collect();
        let ids: Vec<FileId> = files.iter().map(|f| f.0).collect();
        split_runs_by_similarity(vec![run(&ids)], &hashes, BURST_VISUAL_MAX_DIST).into_iter().map(|r| r.files).collect()
    }

    #[test]
    fn frames_that_look_alike_stay_one_group() {
        assert_eq!(split(&[(1, Some(0)), (2, Some(10)), (3, Some(20))]), vec![vec![1, 2, 3]]);
    }

    #[test]
    fn a_different_shot_inside_the_run_splits_it_in_two() {
        // 1,2 are one shot; 3,4 are another, far from both (the "full-body
        // then portrait" case). Distances are all from the 0 hash, so
        // 1-3 differ by 120 bits.
        let got = split(&[(1, Some(0)), (2, Some(5)), (3, Some(120)), (4, Some(125))]);
        assert_eq!(got, vec![vec![1, 2], vec![3, 4]]);
    }

    #[test]
    fn alternating_shots_regroup_by_look_not_by_adjacency() {
        // portrait, full-body, portrait, full-body: two groups of two,
        // even though no two neighbours match.
        let got = split(&[(1, Some(0)), (2, Some(120)), (3, Some(4)), (4, Some(124))]);
        assert_eq!(got, vec![vec![1, 3], vec![2, 4]]);
    }

    #[test]
    fn a_lone_frame_is_not_a_group_and_is_dropped() {
        assert_eq!(split(&[(1, Some(0)), (2, Some(5)), (3, Some(200))]), vec![vec![1, 2]]);
        assert!(split(&[(1, Some(0)), (2, Some(200))]).is_empty());
    }

    #[test]
    fn a_frame_with_no_hash_is_never_grouped() {
        assert_eq!(split(&[(1, Some(0)), (2, None), (3, Some(5))]), vec![vec![1, 3]]);
    }

    #[test]
    fn a_slowly_changing_sequence_chains_into_one_group() {
        // Each step is within the threshold, though the ends are not.
        let got = split(&[(1, Some(0)), (2, Some(60)), (3, Some(120))]);
        assert_eq!(got, vec![vec![1, 2, 3]]);
        let end_to_end = h(0).distance(&h(120));
        assert!(end_to_end > BURST_VISUAL_MAX_DIST);
    }

    #[test]
    fn the_threshold_is_inclusive() {
        let at = BURST_VISUAL_MAX_DIST;
        assert_eq!(split(&[(1, Some(0)), (2, Some(at))]), vec![vec![1, 2]]);
        assert!(split(&[(1, Some(0)), (2, Some(at + 1))]).is_empty());
    }
    use crate::classify::classify_pair;
    use crate::classify::ClassifyInput;

    fn ts(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn pair(a_id: FileId, a_origin: Origin, b_id: FileId, b_origin: Origin, dist: u32) -> PairClassification {
        let a = ClassifyInput { id: a_id, origin: a_origin, camera_model: None, capture_time: None };
        let b = ClassifyInput { id: b_id, origin: b_origin, camera_model: None, capture_time: None };
        classify_pair(&a, &b, dist, 3)
    }

    #[test]
    fn t1_resolution_keeps_only_the_closest_native_match() {
        // Derived file 10 weakly matches native file 1 (d=40, likely noise
        // that only survived the generous match-distance net) and
        // strongly matches native file 2 (d=5, the real duplicate).
        let pairs = vec![
            pair(10, Origin::Derived, 1, Origin::CameraOriginal, 40),
            pair(10, Origin::Derived, 2, Origin::CameraOriginal, 5),
        ];
        let edges = resolve_t1_edges(&pairs);
        assert_eq!(edges, vec![(10, 2, 5)]);
    }

    #[test]
    fn t1b_does_not_bridge_two_files_that_already_have_their_own_native_original() {
        // Regression test for a real discrepancy found against the
        // reference dataset: IMG_3537.JPG (derived) is a true duplicate of
        // native IMG20261016074102.jpg (d=0-2), and IMG_3552.JPG (derived)
        // is a true duplicate of native IMG20261016074103.jpg (d=0-2) --
        // two separate burst-adjacent photos (1s apart, same camera), each
        // independently re-saved through a messenger. The two derived
        // copies themselves hash close enough (d=44, in the same ambiguous
        // band as a genuine burst pair) to classify as a T1b candidate.
        // Without the already_t1_matched guard, this merges two unrelated
        // T1 clusters into one four-file cluster -- exactly the burst-vs-
        // duplicate ambiguity the native-native structural rule exists to
        // prevent, just unguarded on the derived side.
        let native_a = 1; // IMG20261016074102.jpg
        let native_b = 2; // IMG20261016074103.jpg
        let derived_a = 10; // IMG_3537.JPG
        let derived_b = 11; // IMG_3552.JPG

        let pairs = vec![
            pair(derived_a, Origin::Derived, native_a, Origin::CameraOriginal, 2),
            pair(derived_b, Origin::Derived, native_b, Origin::CameraOriginal, 0),
            pair(derived_a, Origin::Derived, derived_b, Origin::Derived, 44),
        ];

        let t1_edges = resolve_t1_edges(&pairs);
        let matched: std::collections::HashSet<FileId> =
            t1_edges.iter().map(|&(derived, _, _)| derived).collect();
        assert_eq!(matched, [derived_a, derived_b].into_iter().collect());

        let t1b_edges = resolve_t1b_edges(&pairs, &matched);
        assert!(
            t1b_edges.is_empty(),
            "T1b must not bridge two files that already have their own native original, got {t1b_edges:?}"
        );
    }

    #[test]
    fn t1b_still_merges_genuinely_orphaned_derived_copies() {
        // The case T1b exists for: neither side has a surviving native
        // original, so merging them (keep the larger) is the correct,
        // safe behavior -- the guard must not break this.
        let pairs = vec![pair(10, Origin::Derived, 11, Origin::Derived, 5)];
        let edges = resolve_t1b_edges(&pairs, &std::collections::HashSet::new());
        assert_eq!(edges, vec![(10, 11, 5)]);
    }

    #[test]
    fn burst_run_splits_on_gap_exceeding_threshold() {
        let images = vec![
            (0u32, "OnePlus 15".to_string(), ts("2026-10-19 10:00:00")),
            (1u32, "OnePlus 15".to_string(), ts("2026-10-19 10:00:01")),
            (2u32, "OnePlus 15".to_string(), ts("2026-10-19 10:00:02")),
            // gap > the burst gap here
            (3u32, "OnePlus 15".to_string(), ts("2026-10-19 10:05:00")),
        ];
        let runs = detect_burst_runs(&images, 3);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].files, vec![0, 1, 2]);
    }

    #[test]
    fn the_time_gap_setting_decides_what_counts_as_one_sequence() {
        let images = vec![
            (0u32, "Cam".to_string(), ts("2026-10-19 10:00:00")),
            (1u32, "Cam".to_string(), ts("2026-10-19 10:00:05")),
            (2u32, "Cam".to_string(), ts("2026-10-19 10:00:10")),
        ];
        assert!(detect_burst_runs(&images, 3).is_empty(), "five seconds apart is not a sequence at a 3 s gap");
        let loose = detect_burst_runs(&images, 5);
        assert_eq!(loose.len(), 1);
        assert_eq!(loose[0].files, vec![0, 1, 2], "the gap is inclusive");
        assert_eq!(detect_burst_runs(&images, 30).len(), 1);
    }

    #[test]
    fn single_photo_is_not_a_burst() {
        let images = vec![(0u32, "OnePlus 15".to_string(), ts("2026-10-19 10:00:00"))];
        assert!(detect_burst_runs(&images, 3).is_empty());
    }

    #[test]
    fn different_cameras_never_merge_into_one_run() {
        let images = vec![
            (0u32, "OnePlus 15".to_string(), ts("2026-10-19 10:00:00")),
            (1u32, "iPhone Air".to_string(), ts("2026-10-19 10:00:01")),
        ];
        assert!(detect_burst_runs(&images, 3).is_empty());
    }
}
