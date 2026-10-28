//! Turns raw candidate pairs into the clean edges/runs that actually feed
//! `cluster::merge_edges` and the UI. Port of
//! `~/photo-dedup/scripts/pass4b_resolve.py`: argmin resolution per file
//! (so a weak, borderline candidate pair surviving `CANDIDATE_MAX_DIST`'s
//! generous net can't merge two genuinely unrelated files -- only each
//! file's single *closest* match becomes an edge) and independent
//! burst-run detection by time+camera sequence.

use crate::classify::{PairClassification, Tier, BURST_GAP_SECONDS};
use crate::model::FileId;
use crate::provenance::Origin;
use chrono::NaiveDateTime;
use std::collections::HashMap;

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
pub fn resolve_t1b_edges(pairs: &[PairClassification]) -> Vec<(FileId, FileId, u32)> {
    let mut best: HashMap<FileId, (FileId, u32)> = HashMap::new();
    for p in pairs.iter().filter(|p| p.tier == Tier::T1b) {
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

#[derive(Debug, Clone)]
pub struct BurstRun {
    pub camera_model: String,
    pub files: Vec<FileId>, // in time order
}

/// Independent of pairwise classification: sorts each camera's native
/// images by capture time and splits into runs wherever the gap exceeds
/// `BURST_GAP_SECONDS`. A run of 1 isn't a burst by definition.
pub fn detect_burst_runs(images: &[(FileId, String, NaiveDateTime)]) -> Vec<BurstRun> {
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
            if gap <= BURST_GAP_SECONDS {
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
    use crate::classify::classify_pair;
    use crate::classify::ClassifyInput;

    fn ts(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn pair(a_id: FileId, a_origin: Origin, b_id: FileId, b_origin: Origin, dist: u32) -> PairClassification {
        let a = ClassifyInput { id: a_id, origin: a_origin, camera_model: None, capture_time: None };
        let b = ClassifyInput { id: b_id, origin: b_origin, camera_model: None, capture_time: None };
        classify_pair(&a, &b, dist)
    }

    #[test]
    fn t1_resolution_keeps_only_the_closest_native_match() {
        // Derived file 10 weakly matches native file 1 (d=40, likely noise
        // that only survived the generous CANDIDATE_MAX_DIST net) and
        // strongly matches native file 2 (d=5, the real duplicate).
        let pairs = vec![
            pair(10, Origin::Derived, 1, Origin::CameraOriginal, 40),
            pair(10, Origin::Derived, 2, Origin::CameraOriginal, 5),
        ];
        let edges = resolve_t1_edges(&pairs);
        assert_eq!(edges, vec![(10, 2, 5)]);
    }

    #[test]
    fn burst_run_splits_on_gap_exceeding_threshold() {
        let images = vec![
            (0u32, "OnePlus 15".to_string(), ts("2026-10-19 10:00:00")),
            (1u32, "OnePlus 15".to_string(), ts("2026-10-19 10:00:01")),
            (2u32, "OnePlus 15".to_string(), ts("2026-10-19 10:00:02")),
            // gap > BURST_GAP_SECONDS here
            (3u32, "OnePlus 15".to_string(), ts("2026-10-19 10:05:00")),
        ];
        let runs = detect_burst_runs(&images);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].files, vec![0, 1, 2]);
    }

    #[test]
    fn single_photo_is_not_a_burst() {
        let images = vec![(0u32, "OnePlus 15".to_string(), ts("2026-10-19 10:00:00"))];
        assert!(detect_burst_runs(&images).is_empty());
    }

    #[test]
    fn different_cameras_never_merge_into_one_run() {
        let images = vec![
            (0u32, "OnePlus 15".to_string(), ts("2026-10-19 10:00:00")),
            (1u32, "iPhone Air".to_string(), ts("2026-10-19 10:00:01")),
        ];
        assert!(detect_burst_runs(&images).is_empty());
    }
}
