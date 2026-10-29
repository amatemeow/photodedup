//! Video duplicate matching. Port of the
//! Python prototype's `pass5_video.py` (not part of this repo): block candidates by duration
//! (±1.5s) to cut the search space, then compare a 5-point frame-pHash
//! sequence for survivors. Same structural safety rule as images (see
//! `classify.rs`): only `Derived`-vs-`CameraOriginal` pairs are eligible
//! for an auto-selected match; two native videos are never auto-matched.
//!
//! Frame extraction shells out to the bundled ffmpeg (`-ss <seek> -frames:v
//! 1 -f image2pipe -vcodec mjpeg -`, piped to stdout rather than a temp
//! file) -- the same binary already required for HEIC decode, so this adds
//! no new dependency (`/SPIKES.md` §1).

use crate::hash::{phash256, Hash256};
use crate::model::FileId;
use crate::platform::{platform, SchedulingHint};
use crate::provenance::Origin;
use rayon::prelude::*;
use std::path::Path;
use std::process::Command;

const DURATION_TOLERANCE_SECONDS: f64 = 1.5;
const FRAME_POINTS: [f64; 5] = [0.1, 0.3, 0.5, 0.7, 0.9];
pub const HIGH_CONFIDENCE_MAX_MEAN_DIST: f64 = 15.0;

/// A duration-matched candidate whose mean frame-pHash distance is at or
/// above this is noise, not a weak match -- picking the "best available"
/// candidate unconditionally is wrong when every candidate is actually
/// unrelated. Calibrated against the reference dataset's own video
/// matches: real matches (including a reframed/re-edited one) topped out
/// at mean distance 26.4; two genuinely unrelated clips (from outside
/// either trip phone) landed at 118.0 and 120.4 -- squarely in the same
/// "chance" territory as the image side's ~100-134 unrelated band. 50
/// sits with wide margin on both sides of that gap.
pub const MAX_MATCH_MEAN_DIST: f64 = 50.0;

#[derive(Debug, Clone)]
pub struct VideoCandidate {
    pub id: FileId,
    pub path: std::path::PathBuf,
    pub origin: Origin,
    pub duration_seconds: f64,
}

#[derive(Debug, Clone)]
pub struct VideoMatch {
    pub derived: FileId,
    pub native: FileId,
    pub mean_phash_dist: f64,
    pub high_confidence: bool,
}

/// Extracts one frame at `fraction` of `duration` via ffmpeg and returns
/// its pHash, or `None` if the seek/decode failed (short or corrupt clips
/// near the tail are the common case -- not fatal to the overall match).
/// Measured: for video frames (unlike the HEIC image path in `decode.rs`),
/// the seek+decode itself dominates wall time, not the output codec -- `ppm`
/// vs `mjpeg` made no measurable difference here. Used anyway for
/// consistency/simplicity with the image path, not as a performance claim.
pub fn frame_phash(path: &Path, fraction: f64, duration: f64, ffmpeg_bin: &str, hint: Option<SchedulingHint>) -> Option<Hash256> {
    let ts = (duration * fraction).max(0.0);
    let mut command = Command::new(ffmpeg_bin);
    let cmd = command.args(["-ss", &format!("{ts:.2}"), "-i"]).arg(path);
    if let Some(hint) = hint {
        platform().configure_child_process(cmd, hint);
    }
    let output = cmd
        .args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "ppm", "-"])
        .output()
        .ok()?;
    if output.stdout.is_empty() {
        return None;
    }
    let img = image::load_from_memory(&output.stdout).ok()?;
    Some(phash256(&img))
}

fn frame_sequence(path: &Path, duration: f64, ffmpeg_bin: &str, hint: Option<SchedulingHint>) -> Vec<Option<Hash256>> {
    FRAME_POINTS
        .iter()
        .map(|&f| frame_phash(path, f, duration, ffmpeg_bin, hint))
        .collect()
}

fn mean_distance(a: &[Option<Hash256>], b: &[Option<Hash256>]) -> Option<f64> {
    let dists: Vec<u32> = a
        .iter()
        .zip(b.iter())
        .filter_map(|(x, y)| match (x, y) {
            (Some(x), Some(y)) => Some(x.distance(y)),
            _ => None,
        })
        .collect();
    if dists.is_empty() {
        return None;
    }
    Some(dists.iter().sum::<u32>() as f64 / dists.len() as f64)
}

/// Finds each `Derived` video's best-matching `CameraOriginal` video among
/// duration-compatible candidates. Videos with no match at all -- no
/// duration-matching candidate, frame extraction failing outright, or
/// every candidate's mean distance landing at or above
/// `MAX_MATCH_MEAN_DIST` (noise, not a weak match; see its doc comment) --
/// are simply absent from the result. The caller surfaces those as
/// "orphan" for display, exactly as the MVP's report did (never silently
/// dropped).
///
/// Parallelized across derived videos with rayon: this is the pipeline's
/// one unavoidably sequential-feeling phase (each comparison round-trips
/// through an ffmpeg subprocess), so letting independent derived videos
/// run concurrently matters in practice, not just in principle -- the
/// reference dataset's 8 derived videos otherwise serialize through
/// hundreds of subprocess spawns on a single core.
pub fn find_video_matches(
    candidates: &[VideoCandidate],
    ffmpeg_bin: &str,
    hint: Option<SchedulingHint>,
    on_progress: &(dyn Fn(usize, usize) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Vec<VideoMatch> {
    let derived: Vec<&VideoCandidate> = candidates.iter().filter(|c| c.origin == Origin::Derived).collect();
    let native: Vec<&VideoCandidate> = candidates
        .iter()
        .filter(|c| c.origin == Origin::CameraOriginal)
        .collect();

    // Progress is counted in derived videos finished: each one is a long
    // run of ffmpeg frame extractions, so there are few, slow units -- a
    // coarse count is still the honest thing to show. `cancelled` is
    // checked before each one starts, so a cancel waits for at most the
    // videos already in flight.
    let total = derived.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    on_progress(0, total);

    derived
        .into_par_iter()
        .filter_map(|dv| {
            if cancelled() {
                return None;
            }
            let result = match_one(dv, &native, ffmpeg_bin, hint);
            on_progress(done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, total);
            result
        })
        .collect()
}

fn match_one(dv: &VideoCandidate, native: &[&VideoCandidate], ffmpeg_bin: &str, hint: Option<SchedulingHint>) -> Option<VideoMatch> {
    let duration_candidates: Vec<&&VideoCandidate> = native
        .iter()
        .filter(|nv| (nv.duration_seconds - dv.duration_seconds).abs() <= DURATION_TOLERANCE_SECONDS)
        .collect();
    if duration_candidates.is_empty() {
        return None;
    }

    let dv_hashes = frame_sequence(&dv.path, dv.duration_seconds, ffmpeg_bin, hint);
    if dv_hashes.iter().all(|h| h.is_none()) {
        return None;
    }

    duration_candidates
        .into_iter()
        .filter_map(|nv| {
            let nv_hashes = frame_sequence(&nv.path, nv.duration_seconds, ffmpeg_bin, hint);
            mean_distance(&dv_hashes, &nv_hashes).map(|d| (nv.id, d))
        })
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .filter(|&(_, mean_d)| mean_d < MAX_MATCH_MEAN_DIST)
        .map(|(native_id, mean_d)| VideoMatch {
            derived: dv.id,
            native: native_id,
            mean_phash_dist: mean_d,
            high_confidence: mean_d <= HIGH_CONFIDENCE_MAX_MEAN_DIST,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_run_skips_every_video_but_still_reports_the_total() {
        let derived = VideoCandidate {
            id: 1,
            path: std::path::PathBuf::from("/nonexistent/derived.mp4"),
            origin: Origin::Derived,
            duration_seconds: 10.0,
        };
        let seen = std::sync::Mutex::new(Vec::new());
        let out = find_video_matches(&[derived], "ffmpeg-not-needed", None, &|d, t| seen.lock().unwrap().push((d, t)), &|| true);
        assert!(out.is_empty());
        assert_eq!(*seen.lock().unwrap(), vec![(0, 1)]);
    }

    #[test]
    fn progress_counts_each_derived_video_once_it_is_finished() {
        let mk = |id| VideoCandidate {
            id,
            path: std::path::PathBuf::from("/nonexistent/derived.mp4"),
            origin: Origin::Derived,
            duration_seconds: 10.0,
        };
        let seen = std::sync::Mutex::new(Vec::new());
        // No native candidates, so each derived video finishes at once
        // without needing ffmpeg.
        find_video_matches(&[mk(1), mk(2)], "ffmpeg-not-needed", None, &|d, t| seen.lock().unwrap().push((d, t)), &|| false);
        let mut got = seen.lock().unwrap().clone();
        got.sort();
        assert_eq!(got, vec![(0, 2), (1, 2), (2, 2)]);
    }

    #[test]
    fn mean_distance_ignores_failed_frame_extractions() {
        let a = vec![Some(Hash256(0, 0)), None, Some(Hash256(0, 0))];
        let b = vec![Some(Hash256(1, 0)), None, Some(Hash256(1, 0))];
        // Both present pairs have distance 1; the None/None pair is skipped.
        assert_eq!(mean_distance(&a, &b), Some(1.0));
    }

    #[test]
    fn mean_distance_is_none_when_nothing_overlaps() {
        let a = vec![None, Some(Hash256(0, 0))];
        let b = vec![Some(Hash256(0, 0)), None];
        assert_eq!(mean_distance(&a, &b), None);
    }

    /// Pins `MAX_MATCH_MEAN_DIST` and `HIGH_CONFIDENCE_MAX_MEAN_DIST`
    /// against the actual mean distances the MVP's pass5_video.py measured
    /// on the reference dataset's 8 derived videos (the
    /// prototype's `video_t1_groups.json` output): 4 high-confidence real
    /// matches, 2 medium-confidence (a reframed/re-edited clip), and 2
    /// genuinely unrelated clips that must be rejected as noise rather
    /// than reported as the "best available" match. This is a pure-data
    /// regression test (no ffmpeg needed) for the threshold logic that
    /// `find_video_matches` applies via `.filter(|&(_, d)| d < MAX_MATCH_MEAN_DIST)`.
    #[test]
    fn threshold_separates_real_matches_from_noise_on_reference_distances() {
        let high_confidence = [4.8, 4.8, 4.4, 4.4];
        let medium_confidence = [25.2, 26.4];
        let unrelated_noise = [120.4, 118.0];

        for d in high_confidence {
            assert!(d < MAX_MATCH_MEAN_DIST && d <= HIGH_CONFIDENCE_MAX_MEAN_DIST);
        }
        for d in medium_confidence {
            assert!(d < MAX_MATCH_MEAN_DIST && d > HIGH_CONFIDENCE_MAX_MEAN_DIST);
        }
        for d in unrelated_noise {
            assert!(
                d >= MAX_MATCH_MEAN_DIST,
                "{d} should be rejected as noise, not reported as a match"
            );
        }
    }
}
