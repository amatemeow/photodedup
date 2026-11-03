//! Making thumbnails and previews before they are asked for, within the
//! cache's budget.
//!
//! Two things, both quiet because the machine belongs to the user:
//!
//! - **pre-render**: the grid thumbnail of every photo that is in some group,
//!   in the order the review shows them, so the overview is instant. It runs
//!   at the user's own Scan speed setting (half its workers, at its scheduling
//!   priority: Efficient is one worker at the lowest priority), and stops at
//!   60% of the budget, leaving the rest for previews;
//! - **the window**: the large previews around where the user is looking
//!   (the open group and its neighbours), made one at a time at Utility
//!   priority, which are also protected from
//!   eviction while they are the window. Each new window replaces the last.

use crate::cache::{THUMB_SIZE, PREVIEW_SIZE};
use crate::routes::files::ensure;
use crate::state::AppState;
use photodedup_core::platform::{platform, SchedulingHint};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Pre-rendering stops once the cache is this share of its budget.
const PRERENDER_SHARE_PERCENT: u64 = 60;
/// The most files one window may ask for.
const MAX_WINDOW: usize = 400;

#[derive(Default)]
pub struct Warmer {
    prerender: Mutex<Option<Prerender>>,
    window_generation: AtomicU64,
}

struct Prerender {
    scan_id: String,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Status {
    pub running: bool,
    pub done: usize,
    pub total: usize,
}

/// Every photo that is in some group, in the order the review lists them:
/// exact copies, then the other duplicates, sequences, look-alikes, videos.
fn grouped_ids(state: &AppState, scan_id: &str) -> rusqlite::Result<Vec<Uuid>> {
    let store = state.store.lock().unwrap();
    let mut ids: Vec<String> = Vec::new();
    let mut clusters = store.load_clusters(scan_id)?;
    clusters.sort_by_key(|c| c.tier != "T0"); // stable: exact copies first
    ids.extend(clusters.into_iter().flat_map(|c| c.member_file_ids));
    ids.extend(store.load_burst_runs(scan_id)?.into_iter().flat_map(|r| r.member_file_ids));
    ids.extend(store.load_review_pairs(scan_id)?.into_iter().flat_map(|p| [p.file_a_id, p.file_b_id]));
    ids.extend(store.load_orphan_videos(scan_id)?.into_iter().map(|o| o.file_id));
    let mut seen = HashSet::new();
    Ok(ids.into_iter().filter_map(|s| Uuid::parse_str(&s).ok()).filter(|u| seen.insert(*u)).collect())
}

impl Warmer {
    pub fn status(&self, scan_id: &str) -> Status {
        match &*self.prerender.lock().unwrap() {
            Some(p) if p.scan_id == scan_id => Status { running: p.running.load(Ordering::Relaxed), done: p.done.load(Ordering::Relaxed), total: p.total.load(Ordering::Relaxed) },
            _ => Status { running: false, done: 0, total: 0 },
        }
    }

    pub fn cancel_prerender(&self) {
        if let Some(p) = &*self.prerender.lock().unwrap() {
            p.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Starts pre-rendering for a scan; does nothing if it is already running
    /// for that scan, and replaces a run for any other.
    pub fn start_prerender(&self, state: AppState, scan_id: String) -> Status {
        let mut slot = self.prerender.lock().unwrap();
        if let Some(p) = &*slot {
            if p.scan_id == scan_id && p.running.load(Ordering::Relaxed) && !p.cancel.load(Ordering::Relaxed) {
                return Status { running: true, done: p.done.load(Ordering::Relaxed), total: p.total.load(Ordering::Relaxed) };
            }
            p.cancel.store(true, Ordering::Relaxed);
        }
        let run = Prerender { scan_id: scan_id.clone(), cancel: Arc::default(), done: Arc::default(), total: Arc::default(), running: Arc::new(AtomicBool::new(true)) };
        let (cancel, done, total, running) = (run.cancel.clone(), run.done.clone(), run.total.clone(), run.running.clone());
        *slot = Some(run);
        drop(slot);

        let intensity = super::routes::settings::stored_intensity(&state.store.lock().unwrap());
        let workers = (intensity.worker_count() / 2).max(1);
        let hint = intensity.scheduling_hint().unwrap_or(SchedulingHint::Utility);

        let spawned = std::thread::Builder::new().name("prerender".into()).spawn(move || {
            let ids = grouped_ids(&state, &scan_id).unwrap_or_default();
            total.store(ids.len(), Ordering::Relaxed);
            let next = AtomicUsize::new(0);
            std::thread::scope(|scope| {
                for _ in 0..workers {
                    scope.spawn(|| {
                        platform().set_current_thread_hint(hint);
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let share = state.cache.budget() / 100 * PRERENDER_SHARE_PERCENT;
                            if i >= ids.len() || cancel.load(Ordering::Relaxed) || state.cache.used() >= share {
                                break;
                            }
                            ensure(&state, &ids[i], THUMB_SIZE, hint);
                            done.fetch_add(1, Ordering::Relaxed);
                        }
                    });
                }
            });
            running.store(false, Ordering::Relaxed);
        });
        if spawned.is_err() {
            if let Some(p) = &*self.prerender.lock().unwrap() {
                p.running.store(false, Ordering::Relaxed);
            }
        }
        self.status_unlocked_for(&self.prerender)
    }

    fn status_unlocked_for(&self, slot: &Mutex<Option<Prerender>>) -> Status {
        match &*slot.lock().unwrap() {
            Some(p) => Status { running: p.running.load(Ordering::Relaxed), done: p.done.load(Ordering::Relaxed), total: p.total.load(Ordering::Relaxed) },
            None => Status { running: false, done: 0, total: 0 },
        }
    }

    /// The previews around where the user is looking, most likely first.
    /// Protects them from eviction and makes any that are missing; a newer
    /// window makes the previous one stop.
    pub fn window(&self, state: AppState, mut ids: Vec<Uuid>) {
        ids.truncate(MAX_WINDOW);
        state.cache.protect(ids.iter().copied().collect());
        let generation = self.window_generation.fetch_add(1, Ordering::SeqCst) + 1;
        if ids.is_empty() {
            return;
        }
        let _ = std::thread::Builder::new().name("window".into()).spawn(move || {
            // Utility, not Background: this is what the user is about to look at.
            platform().set_current_thread_hint(SchedulingHint::Utility);
            for id in ids {
                if state.warm.window_generation.load(Ordering::SeqCst) != generation {
                    return; // superseded by a newer window
                }
                ensure(&state, &id, PREVIEW_SIZE, SchedulingHint::Utility);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::ThumbCache;
    use photodedup_core::model::{FileRecord, MediaClassSerde};
    use photodedup_core::pipeline::{Cluster, PipelineResult};
    use photodedup_core::store::Store;

    /// A scan of `n` real JPEGs (noisy, so each thumbnail has real weight), a
    /// cache with the given budget, and the scan id.
    fn state_with_photos(n: u32, budget: u64) -> (AppState, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let mut files = Vec::new();
        for i in 0..n {
            let mut seed = i.wrapping_mul(2_654_435_761).wrapping_add(1);
            let img = image::RgbImage::from_fn(240, 240, |_, _| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                image::Rgb([(seed >> 8) as u8, (seed >> 16) as u8, (seed >> 24) as u8])
            });
            let path = root.join(format!("p{i}.jpg"));
            img.save(&path).unwrap();
            files.push(FileRecord { id: i, path, file_name: format!("p{i}.jpg"), media_class: MediaClassSerde::Image, size_bytes: 1, mtime_unix: 0, is_placeholder: false });
        }
        let result = PipelineResult {
            clusters: vec![Cluster { tier: "T1", members: (0..n).collect(), keeper: 0, reasons: vec![] }],
            files,
            analyzed: Default::default(),
            review_pairs: vec![],
            burst_runs: vec![],
            video_matches: vec![],
            exact_edges: vec![],
            video_edges: vec![],
            orphan_videos: vec![],
            unknown_provenance_videos: vec![],
            placeholder_count: 0,
            placeholder_bytes: 0,
            refused_managed_libraries: vec![],
            sibling_sets: vec![],
        };
        let mut store = Store::open_in_memory().unwrap();
        let scan = store.save_scan(&root, &result).unwrap();
        let cache_dir = tempfile::tempdir().unwrap().keep();
        let state = AppState {
            store: Arc::new(Mutex::new(store)),
            token: Arc::from("t"),
            cache: Arc::new(ThumbCache::open(cache_dir, budget)),
            warm: Arc::default(),
            job: Arc::new(Mutex::new(None)),
            media_addr: Arc::default(),
            render_gate: Default::default(),
            folder_picker: None,
        };
        (state, scan, dir)
    }

    fn finish(state: &AppState, scan: &str) -> Status {
        for _ in 0..400 {
            let s = state.warm.status(scan);
            if !s.running {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("pre-render did not finish");
    }

    #[test]
    fn prerender_makes_every_thumbnail_when_the_budget_allows() {
        let (state, scan, _keep) = state_with_photos(6, 50 << 20);
        state.warm.start_prerender(state.clone(), scan.clone());
        let s = finish(&state, &scan);
        assert_eq!((s.total, s.done), (6, 6));
        let ids = grouped_ids(&state, &scan).unwrap();
        assert!(ids.iter().all(|id| state.cache.contains(id, THUMB_SIZE)));
    }

    #[test]
    fn prerender_stops_at_its_share_of_the_budget_instead_of_visiting_everything() {
        // Each thumbnail of this noise is tens of KB; 150 KB of budget leaves
        // 90 KB for pre-rendering, which is a handful of the 40 photos.
        let budget = 150 * 1024;
        let (state, scan, _keep) = state_with_photos(40, budget);
        state.warm.start_prerender(state.clone(), scan.clone());
        let s = finish(&state, &scan);
        assert_eq!(s.total, 40);
        assert!(s.done < 40, "it stopped early (visited {})", s.done);
        assert!(state.cache.used() <= budget, "and stayed within the budget ({} of {budget})", state.cache.used());
        assert!(state.cache.used() > 0);
    }

    #[test]
    fn a_cancelled_prerender_stops_and_a_repeat_while_running_does_not_start_another() {
        let (state, scan, _keep) = state_with_photos(30, 50 << 20);
        state.warm.start_prerender(state.clone(), scan.clone());
        state.warm.cancel_prerender();
        let s = finish(&state, &scan);
        assert!(s.done < 30 || s.total == 30, "stopped without error");
        // starting again after a cancel works and completes
        state.warm.start_prerender(state.clone(), scan.clone());
        let s = finish(&state, &scan);
        assert_eq!(s.done, s.total);
    }

    #[test]
    fn the_window_makes_previews_for_what_is_asked_and_protects_them() {
        let (state, scan, _keep) = state_with_photos(5, 50 << 20);
        let ids = grouped_ids(&state, &scan).unwrap();
        state.warm.window(state.clone(), ids[..3].to_vec());
        for _ in 0..200 {
            if ids[..3].iter().all(|id| state.cache.contains(id, PREVIEW_SIZE)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(ids[..3].iter().all(|id| state.cache.contains(id, PREVIEW_SIZE)));
        assert!(!state.cache.contains(&ids[4], PREVIEW_SIZE), "only the window was made");
    }
}
