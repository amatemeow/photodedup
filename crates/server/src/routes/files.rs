//! Thumbnail/preview/original serving -- the one place in this crate that
//! touches the filesystem directly, and the one place path-safety
//! actually matters. Every handler here resolves a filesystem path **only**
//! via `Store::get_file_full_path(file_id)` -- never from a path in the
//! request. A request can name a file by its UUID, never by a path. The id
//! arrives as a parsed `Uuid`, not a string, so a malformed one is rejected
//! before it reaches the database or the thumbnail cache's file names.

use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Path as AxPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use photodedup_core::model::MediaClassSerde;
use photodedup_core::thumbnail::{image_thumbnail, video_frame_thumbnail};
use std::collections::HashMap;
use std::path::PathBuf;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

/// Resolves `file_id` -> (full path, media class), the one DB round-trip
/// every file-serving handler needs before it can do anything.
fn resolve(state: &AppState, file_id: &Uuid) -> Result<(PathBuf, MediaClassSerde), StatusCode> {
    let id = file_id.to_string();
    let store = state.store.lock().unwrap();
    let (root, rel) = store
        .get_file_location(&id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    // Not just "root joined with rel": the path is checked to contain no
    // `..` and no symlink, so a file replaced by a link to somewhere else
    // after the scan cannot be used to read outside the scanned folder.
    let path = photodedup_core::fsguard::Workspace::open(std::path::Path::new(&root))
        .and_then(|ws| ws.existing_file(&rel))
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let file = store
        .get_file(&id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok((path, file.media_class))
}

/// `GET /files/{id}/thumb?size=256` -- a small JPEG for the review grid.
/// Cached on disk (content-addressed by file id + size), since repeated
/// requests are the common case (re-rendering the grid on scroll/filter).
pub async fn thumbnail(
    State(state): State<AppState>,
    AxPath(file_id): AxPath<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let size: u32 = params.get("size").and_then(|s| s.parse().ok()).unwrap_or(256);
    serve_generated_jpeg(state, file_id, size).await
}

/// `GET /files/{id}/preview?size=1600` -- a larger JPEG for the lightbox.
/// Same mechanism as `thumbnail`, just a bigger `size` by convention; kept
/// as a separate route so the UI's intent is clear and cache keys for the
/// two use cases never collide on a shared default.
pub async fn preview(
    State(state): State<AppState>,
    AxPath(file_id): AxPath<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let size: u32 = params.get("size").and_then(|s| s.parse().ok()).unwrap_or(1600);
    serve_generated_jpeg(state, file_id, size).await
}

async fn serve_generated_jpeg(state: AppState, file_id: Uuid, size: u32) -> Response {
    let (path, media_class) = match resolve(&state, &file_id) {
        Ok(v) => v,
        Err(code) => return code.into_response(),
    };

    if let Some(cached) = cache_read(&state.cache_dir, &file_id, size) {
        return jpeg_response(cached);
    }

    // Decode is real CPU work (HEIC via ffmpeg, video frame extraction) --
    // run it off the async runtime's worker thread via spawn_blocking so
    // one slow thumbnail doesn't stall every other in-flight request.
    let bytes = tokio::task::spawn_blocking(move || generate(&path, media_class, size)).await;

    match bytes {
        Ok(Ok(bytes)) => {
            cache_write(&state.cache_dir, &file_id, size, &bytes);
            jpeg_response(bytes)
        }
        _ => StatusCode::UNPROCESSABLE_ENTITY.into_response(),
    }
}

fn generate(path: &std::path::Path, media_class: MediaClassSerde, size: u32) -> anyhow::Result<Vec<u8>> {
    // Background hint, not AllIn/Performance: a thumbnail request serves
    // the UI, which is by definition something the user is actively
    // looking at -- but generation still shouldn't be allowed to peg every
    // core just because a grid scrolled past a lot of unthumbnailed files
    // at once. See throttle.rs/platform/ for why this matters.
    let hint = Some(photodedup_core::platform::SchedulingHint::Utility);
    match media_class {
        MediaClassSerde::Video => {
            // No duration on hand here without another DB round-trip to
            // `analysis` -- ffprobe it directly; this is a single cheap
            // metadata read, not a full decode.
            let duration = photodedup_core::metadata::extract_video(path, "ffprobe", hint)
                .ok()
                .and_then(|m| m.duration_seconds)
                .unwrap_or(1.0);
            Ok(video_frame_thumbnail(path, duration, 0.1, "ffmpeg", size, hint)?)
        }
        _ => Ok(image_thumbnail(path, "ffmpeg", size, hint)?),
    }
}

fn jpeg_response(bytes: Vec<u8>) -> Response {
    (
        [("content-type", "image/jpeg"), ("cache-control", "private, max-age=86400")],
        bytes,
    )
        .into_response()
}

/// `GET /files/{id}/original` -- the real file, streamed with HTTP Range
/// support (needed for `<video>` seeking) via `tower_http::ServeFile`,
/// which implements correct Range/If-Range handling rather than this
/// crate hand-rolling byte-range parsing.
pub async fn original(
    State(state): State<AppState>,
    AxPath(file_id): AxPath<Uuid>,
    headers: HeaderMap,
) -> Response {
    let (path, _) = match resolve(&state, &file_id) {
        Ok(v) => v,
        Err(code) => return code.into_response(),
    };
    let mut req = axum::http::Request::new(Body::empty());
    *req.headers_mut() = headers;
    match ServeFile::new(&path).oneshot(req).await {
        Ok(res) => res.into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

// --- content-addressed disk cache for generated thumbnails/previews ---
//
// Deliberately NOT `std::env::temp_dir()`: on a multi-user system that's
// shared and typically world-readable/writable, which for a tool whose
// whole job is handling someone's private photos means (a) another local
// user can read cached thumbnails of those photos via a predictable path,
// and (b) a predictable, shared path invites a symlink planted ahead of
// time to redirect a write. `init_cache_dir` instead colocates the cache
// with the scan database the user already chose and controls, and creates
// it owner-only; every write below is atomic and owner-only too, and
// every read refuses to follow a symlink.

/// Call once at server startup. Creates (if needed) a cache directory
/// next to `db_path`, owner-only on Unix (`0o700`) so no other local
/// account can read or write into it.
pub fn init_cache_dir(db_path: &std::path::Path) -> std::io::Result<PathBuf> {
    let mut name = db_path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".thumbs");
    let dir = db_path.with_file_name(name);

    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Re-assert the mode even if the directory already existed
                // (e.g. created by an older version of this code, or
                // manually) -- don't trust pre-existing permissions.
                let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            }
            Err(e) => return Err(e),
        }
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&dir).or_else(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { Ok(()) } else { Err(e) })?;
    }

    Ok(dir)
}

/// Empties the cache. Cache entries are keyed by file id, and a new scan
/// replaces all file rows (SQLite may hand the old ids out again), so
/// leaving the old thumbnails would show the wrong photo for a new file.
///
/// Deletes only files that look like cache entries (`<id>-<size>.jpg`, or
/// a leftover `.<id>-<size>.<pid>.tmp`), never anything else in the
/// directory, never follows a link, never recurses: a mis-set `cache_dir`
/// must not be able to wipe unrelated files.
pub fn clear_cache(cache_dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if e.file_type().is_ok_and(|t| !t.is_dir()) && is_cache_entry_name(&name) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn is_cache_entry_name(name: &str) -> bool {
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    // `<uuid>_<size>`, in the canonical hyphenated lowercase form.
    let id_size = |s: &str| {
        s.split_once('_')
            .is_some_and(|(id, size)| id.len() == 36 && Uuid::parse_str(id).is_ok() && id == id.to_ascii_lowercase() && numeric(size))
    };
    if let Some(stem) = name.strip_suffix(".jpg") {
        return id_size(stem);
    }
    if let Some(rest) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tmp")) {
        return rest.rsplit_once('.').is_some_and(|(a, pid)| numeric(pid) && id_size(a));
    }
    false
}

fn cache_path(cache_dir: &std::path::Path, file_id: &Uuid, size: u32) -> PathBuf {
    cache_dir.join(format!("{file_id}_{size}.jpg"))
}

fn cache_read(cache_dir: &std::path::Path, file_id: &Uuid, size: u32) -> Option<Vec<u8>> {
    let path = cache_path(cache_dir, file_id, size);
    // Refuse to follow a symlink -- `symlink_metadata` (not `metadata`)
    // inspects the link itself rather than its target.
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    std::fs::read(path).ok()
}

fn cache_write(cache_dir: &std::path::Path, file_id: &Uuid, size: u32, bytes: &[u8]) {
    let final_path = cache_path(cache_dir, file_id, size);
    // Write-to-temp-then-rename: atomic (same directory = same
    // filesystem), and `create_new` refuses to follow a pre-existing
    // symlink or overwrite a pre-existing file at the temp name.
    let tmp_path = cache_dir.join(format!(".{file_id}_{size}.{}.tmp", std::process::id()));

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let Ok(mut f) = opts.open(&tmp_path) else { return };
    if std::io::Write::write_all(&mut f, bytes).is_err() {
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }
    drop(f);
    let _ = std::fs::rename(&tmp_path, &final_path);
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn clear_cache_removes_only_cache_entries() {
        let dir = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let dir_like = format!("{}_9.jpg", Uuid::new_v4());
        for name in [format!("{id}_256.jpg"), format!("{id}_1024.jpg"), format!(".{id}_256.4242.tmp")] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        for name in ["notes.txt", "photo.jpg", "7-256.jpg", "42_256.jpg"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::write(dir.path().join(format!("{id}_256.jpg.bak")), b"x").unwrap();
        std::fs::create_dir(dir.path().join(&dir_like)).unwrap(); // a directory that merely looks like one

        clear_cache(dir.path());

        let mut left: Vec<String> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into()).collect();
        left.sort();
        let mut want = vec![dir_like, format!("{id}_256.jpg.bak"), "notes.txt".into(), "photo.jpg".into(), "7-256.jpg".into(), "42_256.jpg".into()];
        want.sort();
        assert_eq!(left, want);
    }

    #[test]
    fn init_cache_dir_is_owner_only_on_unix() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("scan.sqlite3");
        let dir = init_cache_dir(&db_path).unwrap();
        assert!(dir.ends_with("scan.sqlite3.thumbs"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn write_then_read_round_trips() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = init_cache_dir(&tmp.path().join("scan.sqlite3")).unwrap();
        let id = Uuid::new_v4();
        cache_write(&dir, &id, 256, b"hello jpeg bytes");
        assert_eq!(cache_read(&dir, &id, 256), Some(b"hello jpeg bytes".to_vec()));
    }

    #[test]
    fn read_refuses_to_follow_a_symlink_at_the_cache_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = init_cache_dir(&tmp.path().join("scan.sqlite3")).unwrap();
        let id = Uuid::new_v4();
        let secret = tmp.path().join("not-a-thumbnail.txt");
        std::fs::write(&secret, b"should never be served as a thumbnail").unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&secret, cache_path(&dir, &id, 256)).unwrap();
            assert_eq!(cache_read(&dir, &id, 256), None);
        }
    }

    #[test]
    fn cache_miss_returns_none_without_creating_anything() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = init_cache_dir(&tmp.path().join("scan.sqlite3")).unwrap();
        assert_eq!(cache_read(&dir, &Uuid::new_v4(), 256), None);
    }
}
