//! Scan-trigger endpoint and progress streaming. `POST /api/scans/start`
//! kicks off the pipeline on a blocking thread and returns a job id at
//! once; `GET /api/jobs/{id}/events` streams its status as SSE and
//! `GET /api/jobs/{id}` returns the same status as a plain snapshot (what
//! a UI reads after a reconnect or page reload, since `EventSource` can't
//! replay what it missed).
//!
//! **One scan at a time.** The pipeline is deliberately throttled to be a
//! polite background citizen (see `core::throttle`); two concurrent scans
//! would defeat that, so a second start while one runs is a 409.
//!
//! **Path handling.** This is the one place a filesystem path comes from a
//! request (`root`) -- unavoidable, since picking a folder to scan is the
//! point. It's still behind the session token, is canonicalized, and must
//! be an existing directory; nothing here reads or serves file contents
//! from it, the pipeline only walks it. File *serving* still resolves
//! exclusively through the database (`routes/files.rs`).

use crate::state::{AppState, Job, JobStatus};
use axum::extract::{Path as AxPath, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use photodedup_core::pipeline::{run_cancellable, PipelineOptions, Progress};
use photodedup_core::throttle::ScanIntensity;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio_stream::wrappers::WatchStream;
use tokio_stream::{Stream, StreamExt};

#[derive(Deserialize)]
pub struct StartBody {
    pub root: PathBuf,
    /// `efficient` | `default` | `performance` | a worker count. Omitted
    /// means the default tier.
    pub mode: Option<String>,
    /// Scan videos too. Omitted means yes.
    pub include_videos: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct StartDto {
    pub job_id: u64,
}

/// An explicit mode in the request wins (scripts, tests); otherwise the
/// person's standing preference from Options; otherwise the default.
fn resolve_intensity(requested: Option<&str>, store: &photodedup_core::store::Store) -> Result<ScanIntensity, String> {
    match requested {
        Some(m) => m.parse(),
        None => Ok(super::settings::stored_intensity(store)),
    }
}

pub async fn start_scan(State(state): State<AppState>, Json(body): Json<StartBody>) -> Result<(StatusCode, Json<StartDto>), (StatusCode, String)> {
    let (intensity, thresholds) = {
        let store = state.store.lock().unwrap();
        (
            resolve_intensity(body.mode.as_deref(), &store).map_err(|e| (StatusCode::BAD_REQUEST, e))?,
            super::settings::stored_thresholds(&store),
        )
    };
    let include_videos = body.include_videos.unwrap_or(true);
    let root = std::fs::canonicalize(&body.root)
        .ok()
        .filter(|p| p.is_dir())
        .ok_or((StatusCode::BAD_REQUEST, format!("not a directory: {}", body.root.display())))?;

    // The database's thumbnail cache sits next to the database. If that is
    // inside the folder being scanned, a re-scan would read our own
    // thumbnails as photos; refuse rather than quietly pollute the results.
    if std::fs::canonicalize(&*state.cache_dir).is_ok_and(|c| c.starts_with(&root)) {
        return Err((
            StatusCode::BAD_REQUEST,
            "the database (and its thumbnail cache) is inside this folder; start the app with a database path outside it".into(),
        ));
    }

    // Claim the slot and publish the job before spawning, so a racing
    // second request sees it running.
    let cancel = Arc::new(AtomicBool::new(false));
    let (job_id, tx) = {
        let mut slot = state.job.lock().unwrap();
        if slot.as_ref().is_some_and(|j| !j.status().is_terminal()) {
            return Err((StatusCode::CONFLICT, "a scan is already running".into()));
        }
        let id = slot.as_ref().map_or(1, |j| j.id + 1);
        let (tx, rx) = tokio::sync::watch::channel(JobStatus::Running(Progress::Scanning));
        *slot = Some(Job { id, rx, cancel: cancel.clone() });
        (id, tx)
    };

    let store = state.store.clone();
    let cache_dir = state.cache_dir.clone();
    tokio::task::spawn_blocking(move || {
        let opts = PipelineOptions { intensity, include_videos, thresholds, ..Default::default() };
        // `send_replace`, not `send`: it never fails when no one is
        // listening, and watch keeps only the latest value -- exactly the
        // coalescing a per-file callback needs.
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_cancellable(
                &root,
                &opts,
                &|p| {
                    tx.send_replace(JobStatus::Running(p));
                },
                &cancel,
            )
        }));
        let status = match ran {
            // Nothing is saved for a cancelled scan: a half-analyzed
            // result isn't a scan anyone could meaningfully resume.
            Ok(None) => JobStatus::Cancelled,
            Ok(Some(result)) => match save_and_record(&store, &root, &result, &opts) {
                Ok(scan_id) => {
                    // The previous scan is gone; so are its thumbnails.
                    super::files::clear_cache(&cache_dir);
                    JobStatus::Done { scan_id }
                }
                Err(e) => JobStatus::Failed { message: format!("saving results failed: {e}") },
            },
            Err(_) => JobStatus::Failed { message: "scan crashed".into() },
        };
        tx.send_replace(status);
    });

    Ok((StatusCode::ACCEPTED, Json(StartDto { job_id })))
}

/// Saves the scan, then notes what it was made with so the review can say
/// so and notice when the saved settings move on. The lock is taken and
/// released per step (never held across both). A failure to record the
/// settings costs only that note, not the scan.
fn save_and_record(
    store: &std::sync::Mutex<photodedup_core::store::Store>,
    root: &std::path::Path,
    result: &photodedup_core::pipeline::PipelineResult,
    opts: &PipelineOptions,
) -> rusqlite::Result<String> {
    let scan_id = store.lock().unwrap().save_scan(root, result)?;
    if let Err(e) = store.lock().unwrap().record_scan_settings(&scan_id, &opts.thresholds, opts.include_videos) {
        eprintln!("server error: could not record scan settings: {e}");
    }
    Ok(scan_id)
}

fn find_job(state: &AppState, id: u64) -> Result<tokio::sync::watch::Receiver<JobStatus>, StatusCode> {
    match &*state.job.lock().unwrap() {
        Some(j) if j.id == id => Ok(j.rx.clone()),
        _ => Err(StatusCode::NOT_FOUND),
    }
}

/// `POST /api/jobs/{id}/cancel`. Asks a running scan to stop and returns
/// at once (202); the job's status flips to `cancelled` when the pipeline
/// reaches its next checkpoint, which clients see on the event stream.
/// Repeating it while the job is still winding down is harmless. 409 if
/// the job already finished, 404 if it isn't the current job.
pub async fn cancel_job(State(state): State<AppState>, AxPath(id): AxPath<u64>) -> StatusCode {
    match &*state.job.lock().unwrap() {
        Some(j) if j.id == id => {
            if j.status().is_terminal() {
                StatusCode::CONFLICT
            } else {
                j.cancel.store(true, Ordering::Relaxed);
                StatusCode::ACCEPTED
            }
        }
        _ => StatusCode::NOT_FOUND,
    }
}

pub async fn job_status(State(state): State<AppState>, AxPath(id): AxPath<u64>) -> Result<Json<JobStatus>, StatusCode> {
    Ok(Json(find_job(&state, id)?.borrow().clone()))
}

/// Emits the current status immediately, then every change, and ends the
/// stream right after the terminal (`done`/`failed`) event so the client's
/// `EventSource` gets a clean close instead of auto-reconnecting forever.
pub async fn job_events(State(state): State<AppState>, AxPath(id): AxPath<u64>) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let rx = find_job(&state, id)?;
    let mut finished = false;
    let stream = WatchStream::new(rx)
        .take_while(move |s| {
            let keep = !finished;
            finished = s.is_terminal();
            keep
        })
        .map(|s| Ok(Event::default().json_data(&s).expect("status serializes")));
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use photodedup_core::store::Store;
    use std::sync::{Arc, Mutex};

    /// The cache dir is a real, private temp dir per state -- never the
    /// shared system temp dir, which a cache clear must not be pointed at.
    fn test_state() -> AppState {
        let cache = tempfile::tempdir().unwrap().keep();
        AppState {
            store: Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
            token: Arc::from("t"),
            cache_dir: Arc::new(cache),
            job: Arc::new(Mutex::new(None)),
        }
    }

    fn body(root: &std::path::Path, mode: Option<&str>) -> Json<StartBody> {
        Json(StartBody { root: root.to_path_buf(), mode: mode.map(String::from), include_videos: None })
    }

    #[test]
    fn a_scan_uses_the_saved_preference_unless_the_request_names_a_mode() {
        let store = Store::open_in_memory().unwrap();
        assert!(matches!(resolve_intensity(None, &store), Ok(ScanIntensity::Default)));

        store.set_setting("scan_intensity", "efficient").unwrap();
        assert!(matches!(resolve_intensity(None, &store), Ok(ScanIntensity::Efficient)), "the saved preference is used");
        assert!(matches!(resolve_intensity(Some("performance"), &store), Ok(ScanIntensity::Performance)), "an explicit mode wins");
        assert!(matches!(resolve_intensity(Some("3"), &store), Ok(ScanIntensity::Custom(_))), "scripts can still pass a worker count");
        assert!(resolve_intensity(Some("turbo"), &store).is_err());
    }

    #[tokio::test]
    async fn bad_root_and_bad_mode_are_rejected_before_any_job_starts() {
        let state = test_state();
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, b"x").unwrap();

        for root in [&missing, &file] {
            let err = start_scan(State(state.clone()), body(root, None)).await.unwrap_err();
            assert_eq!(err.0, StatusCode::BAD_REQUEST);
        }
        let err = start_scan(State(state.clone()), body(tmp.path(), Some("turbo"))).await.unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(state.job.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_scan_is_refused_when_the_database_cache_is_inside_the_folder_being_scanned() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state();
        let inside = root.path().join("scan.sqlite3.thumbs");
        std::fs::create_dir(&inside).unwrap();
        state.cache_dir = Arc::new(inside);
        let err = start_scan(State(state), body(root.path(), None)).await.unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("inside this folder"));
    }

    #[tokio::test]
    async fn second_start_while_running_conflicts_but_a_finished_job_does_not_block() {
        let state = test_state();
        let tmp = tempfile::tempdir().unwrap();

        let (tx, rx) = tokio::sync::watch::channel(JobStatus::Running(Progress::Scanning));
        *state.job.lock().unwrap() = Some(Job { id: 7, rx, cancel: Default::default() });
        let err = start_scan(State(state.clone()), body(tmp.path(), None)).await.unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);

        tx.send_replace(JobStatus::Done { scan_id: "s".into() });
        let (status, Json(dto)) = start_scan(State(state.clone()), body(tmp.path(), None)).await.unwrap();
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(dto.job_id, 8, "ids increase so a stale client can't read a newer job as its own");
    }

    #[tokio::test]
    async fn scan_runs_to_done_saves_a_scan_and_the_event_stream_ends_on_it() {
        let state = test_state();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("notes.txt"), b"not media").unwrap();

        let (_, Json(dto)) = start_scan(State(state.clone()), body(tmp.path(), Some("1"))).await.unwrap();
        let rx = find_job(&state, dto.job_id).unwrap();
        let events: Vec<JobStatus> = tokio::time::timeout(std::time::Duration::from_secs(60), WatchStream::new(rx).take_while({
            let mut finished = false;
            move |s| {
                let keep = !finished;
                finished = s.is_terminal();
                keep
            }
        }).collect())
        .await
        .expect("stream must end once the job is terminal");

        let JobStatus::Done { scan_id } = events.last().unwrap().clone() else {
            panic!("expected done, got {:?}", events.last());
        };
        // the scan remembers what it was made with
        let (used, with_videos) = state.store.lock().unwrap().get_scan_settings(&scan_id).unwrap().expect("settings recorded");
        assert_eq!(used, photodedup_core::thresholds::Thresholds::default());
        assert!(with_videos);
        assert!(state.store.lock().unwrap().get_scan(&scan_id).unwrap().is_some());
        assert!(matches!(&*job_status(State(state), AxPath(dto.job_id)).await.unwrap(), JobStatus::Done { .. }));
    }

    #[tokio::test]
    async fn cancel_stops_a_running_scan_without_saving_and_rejects_finished_or_unknown_jobs() {
        let state = test_state();
        let tmp = tempfile::tempdir().unwrap();

        assert_eq!(cancel_job(State(state.clone()), AxPath(1)).await, StatusCode::NOT_FOUND);

        // Enough files that the scan can't finish before the cancel lands
        // -- the flag is checked per file, so with it already set the
        // pipeline bails before analyzing anything.
        for i in 0..50 {
            std::fs::write(tmp.path().join(format!("IMG_{i:04}.JPG")), b"not a real jpeg").unwrap();
        }
        let (_, Json(dto)) = start_scan(State(state.clone()), body(tmp.path(), Some("1"))).await.unwrap();
        assert_eq!(cancel_job(State(state.clone()), AxPath(dto.job_id)).await, StatusCode::ACCEPTED);

        let mut rx = find_job(&state, dto.job_id).unwrap();
        let last = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                let s = rx.borrow_and_update().clone();
                if s.is_terminal() {
                    return s;
                }
                rx.changed().await.unwrap();
            }
        })
        .await
        .expect("cancelled job must reach a terminal state");

        // The cancel can race a very fast scan to completion; either way
        // it must end terminal, and if it was cancelled nothing was saved.
        match last {
            JobStatus::Cancelled => assert!(state.store.lock().unwrap().list_scans().unwrap().is_empty()),
            JobStatus::Done { .. } => {}
            other => panic!("unexpected terminal state {other:?}"),
        }
        assert_eq!(cancel_job(State(state), AxPath(dto.job_id)).await, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn unknown_job_is_not_found() {
        let state = test_state();
        assert_eq!(job_status(State(state.clone()), AxPath(1)).await.unwrap_err(), StatusCode::NOT_FOUND);
        assert!(job_events(State(state), AxPath(1)).await.is_err());
    }
}
