use photodedup_core::store::Store;
use std::path::PathBuf;
use photodedup_core::pipeline::Progress;
use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// Shared server state. `Store` wraps a single `rusqlite::Connection`,
/// which isn't `Sync` -- a `Mutex` serializes access across concurrent
/// axum handlers. Fine for this scale: a single-user local loopback
/// server doing occasional reads/writes, not a high-concurrency service.
/// Asks the surrounding shell for a folder, with its native dialog. `None`
/// means the dialog was cancelled. Blocks until the user answers.
pub type FolderPicker = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;

/// How many photos are rendered at the same time. Each render is an ffmpeg process or a decode, so a
/// handful keeps the machine usable; the rest wait in line.
pub const RENDER_PERMITS: usize = 4;
/// Video frames take far longer, so they have lanes of their own and never hold up a photo.
pub const VIDEO_PERMITS: usize = 2;
/// Files not yet downloaded from a cloud drive: the wait is the download, so these queue apart from
/// everything that is already on the disk.
pub const REMOTE_PERMITS: usize = 4;

/// The lanes a picture waits in before it is rendered. A slow kind of picture fills only its own lane.
#[derive(Clone)]
pub struct RenderGates {
    pub photos: Arc<tokio::sync::Semaphore>,
    pub videos: Arc<tokio::sync::Semaphore>,
    pub remote: Arc<tokio::sync::Semaphore>,
}

impl Default for RenderGates {
    fn default() -> Self {
        let lane = |n| Arc::new(tokio::sync::Semaphore::new(n));
        RenderGates { photos: lane(RENDER_PERMITS), videos: lane(VIDEO_PERMITS), remote: lane(REMOTE_PERMITS) }
    }
}

impl RenderGates {
    /// The lane for a file: waiting on a download beats being a video beats being a photo.
    pub fn lane(&self, is_video: bool, on_cloud: bool) -> &Arc<tokio::sync::Semaphore> {
        if on_cloud {
            &self.remote
        } else if is_video {
            &self.videos
        } else {
            &self.photos
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    /// Minted once at boot (`server::new_token`), required on every
    /// request -- see `auth.rs`. Not a secret in the cryptographic sense,
    /// just enough to stop any other local process or webpage from
    /// talking to this server by guessing a well-known port.
    pub token: Arc<str>,
    /// Where generated thumbnails/previews are cached. **Deliberately not**
    /// `std::env::temp_dir()`: that's shared, world-readable/writable on
    /// most multi-user systems -- a predictable path there leaks cached
    /// thumbnails of someone's private photos to other local users, and
    /// (since the directory is shared) invites a symlink pre-placed at a
    /// guessed cache path to redirect a write. Colocated with the scan DB
    /// instead (a location the user already chose and controls), created
    /// owner-only -- see `routes/files.rs`'s cache functions for the
    /// owner-only creation and atomic-write half of this.
    pub cache: Arc<crate::cache::ThumbCache>,
    /// Background thumbnail/preview preparation (see `warm.rs`).
    pub warm: Arc<crate::warm::Warmer>,
    /// The most recent scan job (running or finished), if any -- see
    /// `routes/jobs.rs`. Only one runs at a time, and only the latest is
    /// kept: a finished scan's real record is its row in the store.
    pub job: Arc<Mutex<Option<Job>>>,
    /// Where pictures are served from when they have a port of their own (see `lib.rs::run_with`).
    pub media_addr: Arc<std::sync::OnceLock<std::net::SocketAddr>>,
    /// How many pictures may be rendered at once. The rest wait here, in the media server, so a flood
    /// of thumbnails is a queue of its own and never delays the app's other requests.
    pub render_gate: RenderGates,
    /// Present only when running inside a shell that can show a native dialog.
    pub folder_picker: Option<FolderPicker>,
}

pub struct Job {
    pub id: u64,
    pub rx: watch::Receiver<JobStatus>,
    /// Set by `POST /api/jobs/{id}/cancel`; polled by the pipeline.
    pub cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn status(&self) -> JobStatus {
        self.rx.borrow().clone()
    }
}

/// What a scan job reports to clients, serialized as the SSE payload and
/// the snapshot body: `{"state":"running","phase":...}`,
/// `{"state":"done","scan_id":N}`, `{"state":"cancelled"}` or
/// `{"state":"failed","message":...}`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobStatus {
    Running(Progress),
    Done { scan_id: String },
    Cancelled,
    Failed { message: String },
}

impl JobStatus {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, JobStatus::Running(_))
    }
}
