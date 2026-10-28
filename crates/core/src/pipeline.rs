//! Orchestrates scan -> metadata/provenance -> hash -> candidate search ->
//! classify -> resolve -> cluster -> keeper into one pass over a folder.
//! This is the thing `crates/cli` calls, and the thing M1's end-to-end
//! parity test runs against the reference Japan-trip dataset.

use crate::classify::{classify_pair, ClassifyInput, PairClassification};
use crate::thresholds::Thresholds;
use crate::cluster::merge_edges;
use crate::decode::decode_image;
use crate::exacthash::find_exact_duplicates;
use crate::hash::{phash256, sharpness, Hash256};
use crate::keeper::{pick_keeper, score as keeper_score};
use crate::metadata::{extract_image, extract_video};
use crate::model::{FileId, FileRecord, MediaClassSerde, SiblingSet};
use crate::provenance::{evaluate, Origin, Provenance};
use crate::resolve::{detect_burst_runs, resolve_t1_edges, resolve_t1b_edges, split_runs_by_similarity, BurstRun};
use crate::scan::scan;
use crate::throttle::{build_pool, lower_process_priority, ScanIntensity};
use crate::video_match::{find_video_matches, VideoCandidate, VideoMatch};
use chrono::NaiveDateTime;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub struct PipelineOptions {
    pub ffmpeg_bin: String,
    pub ffprobe_bin: String,
    /// How hard the scan is allowed to push the machine -- see
    /// `throttle.rs`. Defaults to `ScanIntensity::Background`: this is a
    /// deliberate product decision, not a tuning afterthought (an
    /// unthrottled run was observed pegging every core on a 12-core
    /// machine during development).
    pub intensity: ScanIntensity,
    /// Whether videos are part of the scan at all. Off means they are left
    /// out entirely -- not listed, not hashed (video files are the big
    /// ones), not probed, not matched -- as if the folder had none. On by
    /// default.
    pub include_videos: bool,
    /// The tunable matching distances and time gap (see `thresholds`).
    pub thresholds: Thresholds,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        PipelineOptions {
            ffmpeg_bin: crate::tools::ffmpeg(),
            ffprobe_bin: crate::tools::ffprobe(),
            intensity: ScanIntensity::default(),
            include_videos: true,
            thresholds: Thresholds::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Analyzed {
    pub provenance: Provenance,
    pub phash: Option<Hash256>,
    pub sharpness: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: u64,
    pub camera_model: Option<String>,
    pub capture_time: Option<NaiveDateTime>,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Cluster {
    pub tier: &'static str,
    pub members: Vec<FileId>,
    pub keeper: FileId,
    pub reasons: Vec<&'static str>,
}

/// Coarse progress of a running pipeline, reported through the callback
/// given to `run_with_progress`. Only the analyzing and video phases carry
/// a count -- analyzing photos is what dominates wall-clock time on a real
/// library (decode + hash per file); every other phase is short enough, or
/// has no natural unit, that "which phase" is all a progress display can
/// usefully say.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Progress {
    Scanning,
    /// Photos read and fingerprinted so far (everything that isn't a
    /// video). Photos go first; videos are a separate phase below.
    Analyzing { done: usize, total: usize },
    /// Video metadata read so far (an ffprobe each: cheap next to a photo).
    AnalyzingVideos { done: usize, total: usize },
    ExactDuplicates,
    Matching,
    /// Derived videos matched so far -- few units, each slow (a run of
    /// ffmpeg frame extractions), so the count moves in big steps.
    Videos { done: usize, total: usize },
    Clustering,
}

pub struct PipelineResult {
    pub files: Vec<FileRecord>,
    pub analyzed: HashMap<FileId, Analyzed>,
    pub clusters: Vec<Cluster>,
    /// Review-only pair classifications (`T2Burst`/`T2NativeReview`/
    /// `T3CrossDevice`/`T2DerivedReview`/`T2UnknownReview`) -- never
    /// auto-selectable, but still real engine output the MVP's report
    /// surfaced as its "same-device review" browse-only tab. Auto-
    /// selectable pairs (`T1`/`T1b`) are not duplicated here; they're
    /// already in `clusters`.
    pub review_pairs: Vec<PairClassification>,
    pub burst_runs: Vec<BurstRun>,
    pub video_matches: Vec<VideoMatch>,
    pub orphan_videos: Vec<FileId>,
    pub unknown_provenance_videos: Vec<FileId>,
    pub placeholder_count: u32,
    pub placeholder_bytes: u64,
    pub refused_managed_libraries: Vec<PathBuf>,
    /// RAW/sidecar/Live-Photo companions of each primary file. Saved so
    /// that applying a decision to a photo moves its companions with it.
    pub sibling_sets: Vec<SiblingSet>,
}

/// Entry point. Builds a worker pool sized per `opts.intensity`, lowers OS
/// scheduling priority, and runs the whole pipeline inside that pool --
/// never on `rayon`'s ambient global pool, which has no throttle of its
/// own (see `throttle.rs`).
pub fn run(root: &Path, opts: &PipelineOptions) -> PipelineResult {
    run_with_progress(root, opts, &|_| {})
}

/// Like `run`, reporting coarse progress through `on_progress`. The
/// callback is invoked from worker threads (once per analyzed file during
/// analyzing phases), so it must be cheap and non-blocking -- a consumer that
/// wants to rate-limit should keep only the latest value, not queue.
pub fn run_with_progress(root: &Path, opts: &PipelineOptions, on_progress: &(dyn Fn(Progress) + Sync)) -> PipelineResult {
    run_cancellable(root, opts, on_progress, &AtomicBool::new(false)).expect("never cancelled")
}

/// Like `run_with_progress`, but stops early and returns `None` once
/// `cancel` is set. Cancellation is cooperative: it's checked per file
/// while analyzing, per derived video during video matching, and
/// between the later phases. The exact-hash pass has no internal
/// checkpoints, so a cancel that lands inside it takes effect when that
/// pass ends.
pub fn run_cancellable(
    root: &Path,
    opts: &PipelineOptions,
    on_progress: &(dyn Fn(Progress) + Sync),
    cancel: &AtomicBool,
) -> Option<PipelineResult> {
    lower_process_priority();
    let pool = build_pool(opts.intensity);
    pool.install(|| run_inner(root, opts, on_progress, cancel))
}

fn run_inner(root: &Path, opts: &PipelineOptions, on_progress: &(dyn Fn(Progress) + Sync), cancel: &AtomicBool) -> Option<PipelineResult> {
    let cancelled = || cancel.load(Ordering::Relaxed);
    on_progress(Progress::Scanning);
    let mut scan_report = scan(root);
    if !opts.include_videos {
        drop_videos(&mut scan_report);
    }
    let file_by_id: HashMap<FileId, &FileRecord> = scan_report.files.iter().map(|f| (f.id, f)).collect();

    // Only sibling-set primaries get independently analyzed/compared;
    // siblings (RAW/XMP/etc.) ride along with their primary's decision.
    let primary_ids: Vec<FileId> = scan_report.sibling_sets.iter().map(|s| s.primary).collect();

    // Photos first, then videos, each with its own count: a single combined
    // count ("N of M photos and videos") would be wrong both ways, since
    // the two kinds are not processed together and cost very differently.
    let (video_ids, photo_ids): (Vec<FileId>, Vec<FileId>) = primary_ids
        .iter()
        .copied()
        .partition(|id| file_by_id[id].media_class == MediaClassSerde::Video);

    let mut analyzed: HashMap<FileId, Analyzed> = HashMap::new();
    for (ids, are_videos) in [(&photo_ids, false), (&video_ids, true)] {
        if ids.is_empty() {
            continue;
        }
        let total = ids.len();
        let done = AtomicUsize::new(0);
        let report = |done: usize| {
            on_progress(if are_videos { Progress::AnalyzingVideos { done, total } } else { Progress::Analyzing { done, total } })
        };
        report(0);
        let batch: Vec<(FileId, Analyzed)> = ids
            .par_iter()
            .filter_map(|&id| {
                if cancelled() {
                    return None;
                }
                let rec = file_by_id[&id];
                let a = analyze_one(rec, opts).map(|a| (id, a));
                report(done.fetch_add(1, Ordering::Relaxed) + 1);
                a
            })
            .collect();
        analyzed.extend(batch);
        if cancelled() {
            return None;
        }
    }

    // --- T0: exact byte duplicates, across ALL files (not just primaries;
    // a sync-conflict duplicate can exist between any two files on disk,
    // and this is cheap -- see exacthash.rs's size-bucketing). ---
    on_progress(Progress::ExactDuplicates);
    let file_tuples: Vec<(FileId, &Path, u64)> = scan_report
        .files
        .iter()
        .map(|f| (f.id, f.path.as_path(), f.size_bytes))
        .collect();
    let exact_groups = find_exact_duplicates(&file_tuples);
    if cancelled() {
        return None;
    }
    let mut t0_edges: Vec<(FileId, FileId)> = Vec::new();
    let mut exact_group_of: HashMap<FileId, usize> = HashMap::new();
    for (gi, g) in exact_groups.iter().enumerate() {
        for &m in &g.members {
            exact_group_of.insert(m, gi);
        }
        for w in g.members.windows(2) {
            t0_edges.push((w[0], w[1]));
        }
    }

    // --- Candidate search + classification, images only. ---
    on_progress(Progress::Matching);
    let image_hash_entries: Vec<(FileId, Hash256)> = primary_ids
        .iter()
        .filter(|&&id| file_by_id[&id].media_class == MediaClassSerde::Image)
        .filter_map(|&id| analyzed.get(&id).and_then(|a| a.phash).map(|h| (id, h)))
        .collect();
    let index = crate::search::BruteForceIndex::build(image_hash_entries);
    let candidate_pairs = index.all_pairs_within(opts.thresholds.match_distance);

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
            let ai = ClassifyInput {
                id: a_id,
                origin: a.provenance.origin,
                camera_model: a.camera_model.clone(),
                capture_time: a.capture_time,
            };
            let bi = ClassifyInput {
                id: b_id,
                origin: b.provenance.origin,
                camera_model: b.camera_model.clone(),
                capture_time: b.capture_time,
            };
            classify_pair(&ai, &bi, dist, opts.thresholds.sequence_gap_seconds)
        })
        .collect();

    let t1_image_edges = resolve_t1_edges(&pair_classifications);
    // A derived file that already resolved to a native original via T1
    // isn't "orphaned" -- T1b must not also merge it into an unrelated
    // cluster through a weaker derived-derived link. See resolve.rs's
    // doc comment on resolve_t1b_edges for the real discrepancy this
    // guards against.
    let already_t1_matched: std::collections::HashSet<FileId> =
        t1_image_edges.iter().map(|&(derived, _, _)| derived).collect();
    let t1b_image_edges = resolve_t1b_edges(&pair_classifications, &already_t1_matched);

    // --- Video matching (separate mechanism: duration-blocked + frame
    // sequence, not phash-matrix candidate search). ---
    let video_candidates: Vec<VideoCandidate> = primary_ids
        .iter()
        .filter(|&&id| file_by_id[&id].media_class == MediaClassSerde::Video)
        .filter_map(|&id| {
            let a = analyzed.get(&id)?;
            Some(VideoCandidate {
                id,
                path: file_by_id[&id].path.clone(),
                origin: a.provenance.origin,
                duration_seconds: a.duration_seconds?,
            })
        })
        .collect();
    let video_matches = find_video_matches(
        &video_candidates,
        &opts.ffmpeg_bin,
        opts.intensity.scheduling_hint(),
        // With videos excluded there is nothing to match, so don't announce
        // a phase that would flash by with no count.
        &|done, total| {
            if opts.include_videos {
                on_progress(Progress::Videos { done, total })
            }
        },
        &cancelled,
    );
    if cancelled() {
        return None;
    }
    let matched_derived_videos: std::collections::HashSet<FileId> =
        video_matches.iter().map(|m| m.derived).collect();
    let orphan_videos: Vec<FileId> = video_candidates
        .iter()
        .filter(|c| c.origin == Origin::Derived && !matched_derived_videos.contains(&c.id))
        .map(|c| c.id)
        .collect();
    // `find_video_matches` only ever considers Derived-vs-CameraOriginal
    // pairs (never auto-matching is the point, same as the image side) --
    // an Unknown-origin video is never a match candidate on either side,
    // so it would otherwise just vanish from every video list (not
    // matched, not "orphan" either, since orphan only covers Derived).
    // Surface it explicitly instead of silently dropping it.
    let unknown_provenance_videos: Vec<FileId> = video_candidates
        .iter()
        .filter(|c| c.origin == Origin::Unknown)
        .map(|c| c.id)
        .collect();
    let video_edges: Vec<(FileId, FileId)> = video_matches.iter().map(|m| (m.derived, m.native)).collect();

    // --- Union-find merge across every edge source, then keeper scoring
    // per cluster. See cluster.rs for why this must happen across ALL
    // edges together, not tier-by-tier. ---
    on_progress(Progress::Clustering);
    let mut tier_of_edge: HashMap<(FileId, FileId), &'static str> = HashMap::new();
    for &(a, b) in &t0_edges {
        tier_of_edge.insert(norm(a, b), "T0");
    }
    for &(a, b, _) in &t1_image_edges {
        tier_of_edge.entry(norm(a, b)).or_insert("T1");
    }
    for &(a, b, _) in &t1b_image_edges {
        tier_of_edge.entry(norm(a, b)).or_insert("T1b");
    }
    for &(a, b) in &video_edges {
        tier_of_edge.entry(norm(a, b)).or_insert("T1");
    }

    let all_edges: Vec<(FileId, FileId)> = t0_edges
        .iter()
        .copied()
        .chain(t1_image_edges.iter().map(|&(a, b, _)| (a, b)))
        .chain(t1b_image_edges.iter().map(|&(a, b, _)| (a, b)))
        .chain(video_edges.iter().copied())
        .collect();

    let raw_clusters = merge_edges(all_edges);
    let clusters: Vec<Cluster> = raw_clusters
        .into_iter()
        .map(|members| {
            let scored: Vec<(FileId, String, _)> = members
                .iter()
                .map(|&id| {
                    let rec = file_by_id[&id];
                    let a = analyzed.get(&id);
                    let ext = rec.path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    let (origin, pixels, has_gps) = match a {
                        Some(a) => (
                            a.provenance.origin,
                            a.width.zip(a.height),
                            a.provenance.evidence.iter().any(|e| matches!(e, crate::provenance::Evidence::GpsPresent)),
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

            Cluster {
                tier,
                members,
                keeper,
                reasons,
            }
        })
        .collect();

    // --- Burst runs: native images only, independent of pairwise
    // classification (see resolve.rs). ---
    let burst_input: Vec<(FileId, String, NaiveDateTime)> = primary_ids
        .iter()
        .filter(|&&id| file_by_id[&id].media_class == MediaClassSerde::Image)
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
        detect_burst_runs(&burst_input, opts.thresholds.sequence_gap_seconds),
        &frame_hashes,
        opts.thresholds.sequence_similarity,
    );

    let review_pairs: Vec<PairClassification> = pair_classifications
        .into_iter()
        .filter(|p| !p.tier.is_auto_selectable())
        .collect();

    Some(PipelineResult {
        files: scan_report.files,
        analyzed,
        clusters,
        review_pairs,
        burst_runs,
        video_matches,
        orphan_videos,
        unknown_provenance_videos,
        placeholder_count: scan_report.placeholder_count,
        placeholder_bytes: scan_report.placeholder_bytes,
        refused_managed_libraries: scan_report.refused_managed_libraries,
        sibling_sets: scan_report.sibling_sets,
    })
}

/// Removes every video, and any sidecar travelling with one, from a scan
/// report -- before anything downstream (the exact-hash pass reads whole
/// files) can touch them.
fn drop_videos(report: &mut crate::scan::ScanReport) {
    let video: HashSet<FileId> = report
        .files
        .iter()
        .filter(|f| f.media_class == MediaClassSerde::Video)
        .map(|f| f.id)
        .chain(
            report
                .sibling_sets
                .iter()
                .filter(|s| report.files.iter().any(|f| f.id == s.primary && f.media_class == MediaClassSerde::Video))
                .flat_map(|s| s.all_members()),
        )
        .collect();
    report.files.retain(|f| !video.contains(&f.id));
    report.sibling_sets.retain(|s| !video.contains(&s.primary));
}

/// Picks a cluster's display tier (T0 > T1 > T1b) from whichever of its
/// members' edges are present in `tier_of_edge`. Checks **all pairs**
/// among `members`, not just sorted-adjacent ones: after union-find, a
/// cluster's real edges don't necessarily fall between consecutive
/// elements of the sorted member list -- e.g. two derived files that each
/// separately matched the same native keeper produce edges
/// (native,d1)/(native,d2); if d1 and d2 sort between other members,
/// neither sorted-adjacent pair is a real edge at all. An earlier version
/// of this used `.windows(2)` and could silently mislabel such a cluster's
/// tier as the `unwrap_or` fallback. Clusters are small (almost always 2-3
/// members), so the O(k^2) pair scan here is free.
fn cluster_tier(members: &[FileId], tier_of_edge: &HashMap<(FileId, FileId), &'static str>) -> &'static str {
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

fn norm(a: FileId, b: FileId) -> (FileId, FileId) {
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
    fn videos_are_left_out_of_the_scan_when_the_flag_is_off() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("clip.mp4"), b"not a real video").unwrap();
        std::fs::write(tmp.path().join("clip2.MOV"), b"not a real video").unwrap();
        std::fs::write(tmp.path().join("pic.jpg"), b"not a real jpeg").unwrap();

        let names = |opts: &PipelineOptions| {
            let mut n: Vec<String> = run(tmp.path(), opts).files.into_iter().map(|f| f.file_name).collect();
            n.sort();
            n
        };
        let on = names(&PipelineOptions::default());
        assert_eq!(on, ["clip.mp4", "clip2.MOV", "pic.jpg"]);

        let off = names(&PipelineOptions { include_videos: false, ..Default::default() });
        assert_eq!(off, ["pic.jpg"], "photos are untouched, videos are gone");
    }

    #[test]
    fn photos_are_reported_first_then_videos_each_with_its_own_count() {
        let tmp = tempfile::tempdir().unwrap();
        for n in ["a.jpg", "b.jpg", "c.jpg"] {
            std::fs::write(tmp.path().join(n), b"not a real jpeg").unwrap();
        }
        for n in ["x.mp4", "y.mov"] {
            std::fs::write(tmp.path().join(n), b"not a real video").unwrap();
        }
        let seen = std::sync::Mutex::new(Vec::new());
        run_with_progress(tmp.path(), &PipelineOptions::default(), &|p| seen.lock().unwrap().push(p));
        let seen = seen.into_inner().unwrap();

        let photo_totals: Vec<usize> = seen.iter().filter_map(|p| if let Progress::Analyzing { total, .. } = p { Some(*total) } else { None }).collect();
        let video_totals: Vec<usize> = seen.iter().filter_map(|p| if let Progress::AnalyzingVideos { total, .. } = p { Some(*total) } else { None }).collect();
        assert!(photo_totals.iter().all(|&t| t == 3), "the photo count covers photos only: {photo_totals:?}");
        assert!(video_totals.iter().all(|&t| t == 2), "the video count covers videos only: {video_totals:?}");

        let last_photo = seen.iter().rposition(|p| matches!(p, Progress::Analyzing { .. })).unwrap();
        let first_video = seen.iter().position(|p| matches!(p, Progress::AnalyzingVideos { .. })).unwrap();
        assert!(last_photo < first_video, "photos finish before videos start");
        assert!(seen.contains(&Progress::Analyzing { done: 3, total: 3 }));
        assert!(seen.contains(&Progress::AnalyzingVideos { done: 2, total: 2 }));
    }

    #[test]
    fn video_phases_are_not_reported_when_videos_are_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pic.jpg"), b"not a real jpeg").unwrap();
        std::fs::write(tmp.path().join("clip.mp4"), b"not a real video").unwrap();
        let phases = |include_videos: bool| {
            let seen = std::sync::Mutex::new(Vec::new());
            let opts = PipelineOptions { include_videos, ..Default::default() };
            run_with_progress(tmp.path(), &opts, &|p| seen.lock().unwrap().push(p));
            seen.into_inner().unwrap()
        };
        let is_video_phase = |p: &Progress| matches!(p, Progress::AnalyzingVideos { .. } | Progress::Videos { .. });
        assert!(phases(true).iter().any(is_video_phase));
        assert!(!phases(false).iter().any(is_video_phase));
    }

    #[test]
    fn a_pre_set_cancel_flag_stops_the_run_before_any_result_exists() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.jpg"), b"x").unwrap();
        let cancel = AtomicBool::new(true);
        let result = run_cancellable(tmp.path(), &PipelineOptions::default(), &|_| {}, &cancel);
        assert!(result.is_none());
    }

    #[test]
    fn an_unset_cancel_flag_runs_to_completion() {
        let tmp = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        assert!(run_cancellable(tmp.path(), &PipelineOptions::default(), &|_| {}, &cancel).is_some());
    }

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

fn analyze_one(rec: &FileRecord, opts: &PipelineOptions) -> Option<Analyzed> {
    let hint = opts.intensity.scheduling_hint();
    match rec.media_class {
        MediaClassSerde::Image => {
            let meta = extract_image(&rec.path).ok()?;
            let provenance = evaluate(&meta, &rec.file_name);
            let img = decode_image(&rec.path, &opts.ffmpeg_bin, hint);
            let (phash, sharp, w, h) = match &img {
                Some(img) => (
                    Some(phash256(img)),
                    Some(sharpness(img)),
                    Some(img.width()),
                    Some(img.height()),
                ),
                None => (None, None, meta.width, meta.height),
            };
            Some(Analyzed {
                provenance,
                phash,
                sharpness: sharp,
                width: w,
                height: h,
                size_bytes: rec.size_bytes,
                camera_model: meta.camera_model(),
                capture_time: meta.datetime_original,
                duration_seconds: None,
            })
        }
        MediaClassSerde::Video => {
            let meta = extract_video(&rec.path, &opts.ffprobe_bin, hint).ok()?;
            let provenance = evaluate(&meta, &rec.file_name);
            Some(Analyzed {
                provenance,
                phash: None,
                sharpness: None,
                width: meta.width,
                height: meta.height,
                size_bytes: rec.size_bytes,
                camera_model: meta.camera_model(),
                capture_time: meta.datetime_original,
                duration_seconds: meta.duration_seconds,
            })
        }
        MediaClassSerde::Raw | MediaClassSerde::Sidecar => None,
    }
}
