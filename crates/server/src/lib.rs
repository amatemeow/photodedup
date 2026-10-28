//! Loopback HTTP server for the review UI. Security properties (per the
//! architecture plan): bound to `127.0.0.1` only, a random OS-assigned
//! port (never a fixed/well-known one), a session token required on every
//! request (`auth.rs`), and -- most importantly -- every file-serving
//! endpoint resolves a filesystem path *only* through the database
//! (`Store::get_file_full_path`), never from anything in the request
//! (`routes/files.rs`). No path parameter ever reaches the filesystem
//! directly.

pub mod auth;
#[cfg(feature = "embed-ui")]
mod embedded;
mod routes;
mod state;

pub use state::FolderPicker;

use axum::routing::{get, post};
use axum::Router;
use state::AppState;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};

pub struct ServerInfo {
    pub addr: std::net::SocketAddr,
    pub token: String,
}

impl ServerInfo {
    /// The URL to open -- token included, so a plain click/paste into a
    /// browser just works without the user having to assemble it.
    pub fn url(&self) -> String {
        format!("http://{}/?token={}", self.addr, self.token)
    }
}

/// Builds the router and binds a listener, without starting to serve yet
/// -- split out from `run` so tests can bind an ephemeral port and issue
/// requests against the router directly (via `tower::ServiceExt::oneshot`)
/// without a real network round-trip.
pub async fn build(db_path: &std::path::Path) -> anyhow::Result<(TcpListener, Router, ServerInfo)> {
    build_with_ui(db_path, None).await
}

/// How the server is embedded.
#[derive(Default, Clone)]
pub struct Config {
    /// Serve the web UI from this folder (a Vite `dist/`), which wins over
    /// the compiled-in copy -- handy for development.
    pub ui_dir: Option<PathBuf>,
    /// The shell's native folder dialog, if it has one.
    pub folder_picker: Option<FolderPicker>,
}

/// `build_with`, serving the built web UI from `ui_dir` when given.
pub async fn build_with_ui(
    db_path: &std::path::Path,
    ui_dir: Option<PathBuf>,
) -> anyhow::Result<(TcpListener, Router, ServerInfo)> {
    build_with(db_path, Config { ui_dir, ..Config::default() }).await
}

/// The full builder. The UI's files are public static assets -- no token
/// needed to fetch them, and nothing secret in them; the token only
/// matters for the API, and the UI picks it up from the URL the server
/// opened. Without a UI folder or a compiled-in copy, `/` is a plain
/// placeholder page.
pub async fn build_with(db_path: &std::path::Path, config: Config) -> anyhow::Result<(TcpListener, Router, ServerInfo)> {
    let Config { ui_dir, folder_picker } = config;
    let store = photodedup_core::store::Store::open(db_path)?;
    // A crash mid-apply leaves a journalled batch half-done; reconcile it
    // with what is actually on disk before serving anything.
    match photodedup_core::apply::recover(&store) {
        Ok(0) => {}
        Ok(n) => eprintln!("photodedup: finished {n} interrupted apply batch(es)"),
        Err(e) => eprintln!("photodedup: could not check for interrupted batches: {e}"),
    }
    let token = auth::new_token();
    let cache_dir = routes::files::init_cache_dir(db_path)?;
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        token: Arc::from(token.as_str()),
        cache_dir: Arc::new(cache_dir),
        job: Arc::new(Mutex::new(None)),
        folder_picker,
    };

    let protected = Router::new()
        .route("/api/scans", get(routes::scans::list_scans))
        .route("/api/scans/{id}/info", get(routes::scans::scan_info))
        .route("/api/scans/{id}/clusters", get(routes::scans::list_clusters))
        .route("/api/scans/{id}/review-pairs", get(routes::scans::list_review_pairs))
        .route("/api/scans/{id}/burst-runs", get(routes::scans::list_burst_runs))
        .route("/api/scans/start", post(routes::jobs::start_scan))
        .route("/api/jobs/{id}", get(routes::jobs::job_status))
        .route("/api/jobs/{id}/cancel", post(routes::jobs::cancel_job))
        .route("/api/jobs/{id}/events", get(routes::jobs::job_events))
        .route("/api/scans/{id}/orphan-videos", get(routes::scans::list_orphan_videos))
        .route("/api/scans/{id}/apply/plan", get(routes::apply::preview))
        .route("/api/scans/{id}/apply", post(routes::apply::run))
        .route("/api/batches", get(routes::apply::batches))
        .route("/api/batches/{id}/undo", post(routes::apply::undo))
        .route("/api/settings", get(routes::settings::get).put(routes::settings::put))
        .route("/api/files/{id}/analysis", get(routes::scans::get_analysis))
        .route(
            "/api/scans/{id}/decisions",
            get(routes::decisions::get_decisions)
                .put(routes::decisions::put_decision)
                .delete(routes::decisions::clear_decision),
        )
        .route(
            "/api/scans/{id}/decisions/bulk",
            post(routes::decisions::put_decisions_bulk),
        )
        .route("/api/resume", get(routes::decisions::resume))
        .route("/api/shell", get(routes::shell::status))
        .route("/api/pick-folder", post(routes::shell::pick_folder))
        .route("/files/{id}/thumb", get(routes::files::thumbnail))
        .route("/files/{id}/preview", get(routes::files::preview))
        .route("/files/{id}/original", get(routes::files::original))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth::require_token))
        .with_state(state);

    let public = Router::new().route("/health", get(|| async { "ok" }));
    let public = match ui_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            public.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
        }
        None => embedded_or_placeholder(public),
    };

    let router = public.merge(protected);

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    Ok((listener, router, ServerInfo { addr, token }))
}

#[cfg(feature = "embed-ui")]
fn embedded_or_placeholder(public: Router) -> Router {
    if embedded::available() {
        public.fallback(embedded::serve)
    } else {
        public.route("/", get(placeholder_page))
    }
}

#[cfg(not(feature = "embed-ui"))]
fn embedded_or_placeholder(public: Router) -> Router {
    // Unauthenticated static text, so a wrong or missing token shows an
    // explanation rather than axum's bare 404.
    public.route("/", get(placeholder_page))
}

async fn placeholder_page() -> axum::response::Html<&'static str> {
    axum::response::Html(
        r#"<!doctype html><meta charset="utf-8">
<title>photodedup</title>
<body style="font:14px system-ui;max-width:40em;margin:4em auto;line-height:1.5">
<h1>photodedup server is running</h1>
<p>The web UI wasn't found (build it with <code>npm run build</code> in
<code>ui/</code>, or set <code>PHOTODEDUP_UI_DIR</code>) -- this is a
placeholder so the URL this process opens doesn't just 404.</p>
<p>The API is live, e.g. <code>GET /api/scans?token=...</code>.</p>
</body>"#,
    )
}

/// Binds and serves until the process is killed. Returns `ServerInfo`
/// immediately (before the serve loop, which runs in the background) so
/// the caller can print/open the URL right away.
pub async fn run(db_path: PathBuf, ui_dir: Option<PathBuf>) -> anyhow::Result<ServerInfo> {
    run_with(db_path, Config { ui_dir, ..Config::default() }).await
}

/// `run`, with a full `Config` (what a shell that embeds the server uses).
pub async fn run_with(db_path: PathBuf, config: Config) -> anyhow::Result<ServerInfo> {
    let (listener, router, info) = build_with(&db_path, config).await?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("server error: {e}");
        }
    });
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tower::ServiceExt;

    async fn test_server() -> (Router, ServerInfo) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let (_listener, router, info) = build(tmp.path()).await.unwrap();
        std::mem::forget(tmp); // keep the file alive for the test's duration
        (router, info)
    }

    /// Like `test_server`, but the DB already has one minimal scan saved
    /// -- what the decisions/resume tests need to exercise real queries
    /// instead of hitting an empty database.
    async fn test_server_with_scan() -> (Router, ServerInfo, String, std::path::PathBuf) {
        use photodedup_core::model::{FileRecord, MediaClassSerde};

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let root = std::env::temp_dir().join("photodedup-server-test-root");
        let files = vec![FileRecord {
            id: 0,
            path: root.join("a.jpg"),
            file_name: "a.jpg".into(),
            media_class: MediaClassSerde::Image,
            size_bytes: 100,
            mtime_unix: 0,
            is_placeholder: false,
        }];
        let result = photodedup_core::pipeline::PipelineResult {
            files,
            analyzed: Default::default(),
            clusters: vec![],
            review_pairs: vec![],
            burst_runs: vec![],
            video_matches: vec![],
            orphan_videos: vec![],
            unknown_provenance_videos: vec![],
            placeholder_count: 0,
            placeholder_bytes: 0,
            refused_managed_libraries: vec![],
            sibling_sets: vec![],
        };
        let scan_id = {
            let mut store = photodedup_core::store::Store::open(tmp.path()).unwrap();
            store.save_scan(&root, &result).unwrap()
        };

        let (_listener, router, info) = build(tmp.path()).await.unwrap();
        std::mem::forget(tmp);
        (router, info, scan_id, root)
    }

    async fn send(router: &Router, req: axum::http::Request<axum::body::Body>) -> axum::http::Response<axum::body::Body> {
        router.clone().oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn health_check_needs_no_token() {
        let (router, _info) = test_server().await;
        let res = router
            .oneshot(axum::http::Request::get("/health").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }

    #[tokio::test]
    async fn ui_dir_is_served_publicly_while_the_api_stays_behind_the_token() {
        let ui = tempfile::tempdir().unwrap();
        std::fs::write(ui.path().join("index.html"), "<title>ui</title>").unwrap();
        std::fs::write(ui.path().join("app.js"), "console.log(1)").unwrap();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let (_listener, router, _info) = build_with_ui(tmp.path(), Some(ui.path().to_path_buf())).await.unwrap();

        for (uri, want) in [("/", "<title>ui</title>"), ("/app.js", "console.log(1)"), ("/some/client/route", "<title>ui</title>")] {
            let res = send(&router, axum::http::Request::get(uri).body(axum::body::Body::empty()).unwrap()).await;
            assert_eq!(res.status(), 200, "{uri}");
            let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
            assert_eq!(body, want, "{uri}");
        }
        let res = send(&router, axum::http::Request::get("/api/scans").body(axum::body::Body::empty()).unwrap()).await;
        assert_eq!(res.status(), 401);
    }

    #[tokio::test]
    async fn protected_route_without_token_is_rejected() {
        let (router, _info) = test_server().await;
        let res = router
            .oneshot(axum::http::Request::get("/api/scans").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 401);
    }

    #[tokio::test]
    async fn protected_route_with_correct_token_succeeds() {
        let (router, info) = test_server().await;
        let uri = format!("/api/scans?token={}", info.token);
        let res = router
            .oneshot(axum::http::Request::get(uri).body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }

    #[tokio::test]
    async fn protected_route_with_wrong_token_is_rejected() {
        let (router, _info) = test_server().await;
        let res = router
            .oneshot(
                axum::http::Request::get("/api/scans?token=not-the-real-token")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 401);
    }

    #[tokio::test]
    async fn unknown_file_id_returns_not_found_not_a_crash() {
        let (router, info) = test_server().await;
        let uri = format!("/files/{}/original?token={}", uuid::Uuid::new_v4(), info.token);
        let res = send(&router, axum::http::Request::get(uri).body(axum::body::Body::empty()).unwrap()).await;
        assert_eq!(res.status(), 404);
    }

    #[tokio::test]
    async fn malformed_or_path_like_file_ids_are_rejected_before_touching_anything() {
        let (router, info) = test_server().await;
        for bad in ["999", "not-a-uuid", "..%2F..%2Fetc%2Fpasswd", "%2e%2e", "0000"] {
            for kind in ["thumb", "preview", "original"] {
                let uri = format!("/files/{bad}/{kind}?token={}", info.token);
                let res = send(&router, axum::http::Request::get(uri).body(axum::body::Body::empty()).unwrap()).await;
                assert!(res.status().is_client_error() && res.status() != 200, "{bad}/{kind} -> {}", res.status());
            }
        }
    }

    #[tokio::test]
    async fn decision_written_then_read_back_round_trips() {
        let (router, info, scan_id, _root) = test_server_with_scan().await;

        let put_uri = format!("/api/scans/{scan_id}/decisions?token={}", info.token);
        let put_res = send(
            &router,
            axum::http::Request::put(put_uri)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(r#"{"relative_path":"a.jpg","keep":true}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(put_res.status(), 204);

        let get_uri = format!("/api/scans/{scan_id}/decisions?token={}", info.token);
        let get_res = send(&router, axum::http::Request::get(get_uri).body(axum::body::Body::empty()).unwrap()).await;
        assert_eq!(get_res.status(), 200);
        let body = axum::body::to_bytes(get_res.into_body(), usize::MAX).await.unwrap();
        let decisions: HashMap<String, bool> = serde_json::from_slice(&body).unwrap();
        assert_eq!(decisions.get("a.jpg"), Some(&true));
    }

    #[tokio::test]
    async fn burst_runs_suggest_the_sharpest_frame_and_orphan_videos_list_with_their_reason() {
        use photodedup_core::model::{FileRecord, MediaClassSerde};
        use photodedup_core::pipeline::Analyzed;
        use photodedup_core::provenance::{Origin, Provenance};

        let root = std::env::temp_dir().join("photodedup-server-test-root-burst");
        let rec = |id, name: &str, class| FileRecord {
            id,
            path: root.join(name),
            file_name: name.into(),
            media_class: class,
            size_bytes: 10,
            mtime_unix: 0,
            is_placeholder: false,
        };
        let analysis = |sharpness, hour: u32| Analyzed {
            provenance: Provenance { origin: Origin::CameraOriginal, confidence: 1.0, evidence: vec![] },
            phash: None,
            sharpness: Some(sharpness),
            width: None,
            height: None,
            size_bytes: 10,
            camera_model: None,
            capture_time: chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap().and_hms_opt(hour, 0, 0),
            duration_seconds: None,
        };
        let result = photodedup_core::pipeline::PipelineResult {
            files: vec![
                rec(0, "f0.jpg", MediaClassSerde::Image),
                rec(1, "f1.jpg", MediaClassSerde::Image),
                rec(2, "f2.jpg", MediaClassSerde::Image),
                rec(3, "v.mp4", MediaClassSerde::Video),
            ],
            analyzed: [(0, analysis(10.0, 9)), (1, analysis(99.0, 8)), (2, analysis(50.0, 10))].into_iter().collect(),
            clusters: vec![],
            review_pairs: vec![],
            burst_runs: vec![photodedup_core::resolve::BurstRun { camera_model: "Cam".into(), files: vec![0, 1, 2] }],
            video_matches: vec![],
            orphan_videos: vec![3],
            unknown_provenance_videos: vec![],
            placeholder_count: 0,
            placeholder_bytes: 0,
            refused_managed_libraries: vec![],
            sibling_sets: vec![],
        };
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let scan_id = photodedup_core::store::Store::open(tmp.path()).unwrap().save_scan(&root, &result).unwrap();
        let (_l, router, info) = build(tmp.path()).await.unwrap();
        std::mem::forget(tmp);

        let get = |path: String| {
            let router = router.clone();
            async move {
                let res = send(&router, axum::http::Request::get(path).body(axum::body::Body::empty()).unwrap()).await;
                assert_eq!(res.status(), 200);
                let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()
            }
        };
        let runs = get(format!("/api/scans/{scan_id}/burst-runs?token={}", info.token)).await;
        let run = &runs[0];
        let frames: Vec<&str> = run["frames"].as_array().unwrap().iter().map(|f| f["file_name"].as_str().unwrap()).collect();
        assert_eq!(frames, ["f0.jpg", "f1.jpg", "f2.jpg"], "capture order is preserved");
        let sharpest = run["suggested_file_id"].as_str().unwrap();
        let sharpest_name = run["frames"].as_array().unwrap().iter().find(|f| f["id"] == sharpest).unwrap()["file_name"].clone();
        assert_eq!(sharpest_name, "f1.jpg");
        assert_eq!(run["sharpness"].as_object().unwrap().len(), 3);
        assert_eq!(run["captured_at"], "2026-10-15 08:00:00", "a sequence is dated by its earliest frame");

        let orphans = get(format!("/api/scans/{scan_id}/orphan-videos?token={}", info.token)).await;
        assert_eq!(orphans[0]["file"]["file_name"], "v.mp4");
        assert_eq!(orphans[0]["reason"], "no_match");
    }

    /// A server over real files on disk: `files` are created under a temp
    /// root and scanned into one cluster whose keeper is the first.
    async fn server_over_real_files(files: &[(&str, &[u8])]) -> (Router, ServerInfo, String, std::path::PathBuf, tempfile::TempDir) {
        use photodedup_core::model::{FileRecord, MediaClassSerde};
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let mut records = Vec::new();
        for (i, (rel, body)) in files.iter().enumerate() {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            let meta = std::fs::metadata(&p).unwrap();
            records.push(FileRecord {
                id: i as u32,
                path: p.clone(),
                file_name: p.file_name().unwrap().to_string_lossy().into(),
                media_class: MediaClassSerde::Image,
                size_bytes: meta.len(),
                mtime_unix: meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
                is_placeholder: false,
            });
        }
        let result = photodedup_core::pipeline::PipelineResult {
            clusters: vec![photodedup_core::pipeline::Cluster {
                tier: "T1",
                members: (0..files.len() as u32).collect(),
                keeper: 0,
                reasons: vec![],
            }],
            files: records,
            analyzed: Default::default(),
            review_pairs: vec![],
            burst_runs: vec![],
            video_matches: vec![],
            orphan_videos: vec![],
            unknown_provenance_videos: vec![],
            placeholder_count: 0,
            placeholder_bytes: 0,
            refused_managed_libraries: vec![],
            sibling_sets: vec![],
        };
        // The database lives OUTSIDE the scanned folder, as it must.
        let db = tempfile::NamedTempFile::new().unwrap();
        let scan_id = photodedup_core::store::Store::open(db.path()).unwrap().save_scan(&root, &result).unwrap();
        let (_l, router, info) = build(db.path()).await.unwrap();
        std::mem::forget(db);
        (router, info, scan_id, root, dir)
    }

    async fn call(router: &Router, method: &str, uri: String, body: &str) -> (u16, serde_json::Value) {
        let res = send(
            router,
            axum::http::Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn preview_apply_list_and_undo_work_end_to_end_and_need_explicit_confirmation() {
        let (router, info, scan, root, _dir) = server_over_real_files(&[("keep.jpg", b"the original"), ("dup.jpg", b"copy")]).await;
        let t = &info.token;

        // mark the duplicate for quarantine
        let (s, _) = call(&router, "PUT", format!("/api/scans/{scan}/decisions?token={t}"), r#"{"relative_path":"dup.jpg","keep":false}"#).await;
        assert_eq!(s, 204);

        // the dry run touches nothing
        let (s, plan) = call(&router, "GET", format!("/api/scans/{scan}/apply/plan?token={t}"), "").await;
        assert_eq!(s, 200);
        assert_eq!(plan["items"][0]["relative_path"], "dup.jpg");
        assert!(root.join("dup.jpg").exists());

        // no confirmation, no move
        let (s, _) = call(&router, "POST", format!("/api/scans/{scan}/apply?token={t}"), "{}").await;
        assert_eq!(s, 400);
        assert!(root.join("dup.jpg").exists());

        let (s, done) = call(&router, "POST", format!("/api/scans/{scan}/apply?token={t}"), r#"{"confirm":true}"#).await;
        assert_eq!(s, 200);
        assert_eq!(done["moved"], 1);
        assert!(!root.join("dup.jpg").exists() && root.join("keep.jpg").exists());

        let (_, batches) = call(&router, "GET", format!("/api/batches?token={t}"), "").await;
        assert_eq!(batches[0]["state"], "done");
        assert_eq!(batches[0]["in_quarantine"], 1);
        let batch_id = batches[0]["id"].as_str().unwrap().to_string();

        let (s, undone) = call(&router, "POST", format!("/api/batches/{batch_id}/undo?token={t}"), "").await;
        assert_eq!(s, 200);
        assert_eq!(undone["restored"], 1);
        assert_eq!(std::fs::read(root.join("dup.jpg")).unwrap(), b"copy");

        let (s, _) = call(&router, "POST", format!("/api/batches/{batch_id}/undo?token={t}"), "").await;
        assert_eq!(s, 409, "already undone");
        let (s, _) = call(&router, "POST", format!("/api/batches/not-a-uuid/undo?token={t}"), "").await;
        assert!(s == 400 || s == 404);
    }

    #[tokio::test]
    async fn applying_with_nothing_marked_is_a_conflict_and_moves_nothing() {
        let (router, info, scan, root, _dir) = server_over_real_files(&[("a.jpg", b"one"), ("b.jpg", b"two")]).await;
        let (s, _) = call(&router, "POST", format!("/api/scans/{scan}/apply?token={}", info.token), r#"{"confirm":true}"#).await;
        assert_eq!(s, 409);
        assert!(root.join("a.jpg").exists() && root.join("b.jpg").exists());
    }

    #[tokio::test]
    async fn a_file_swapped_for_a_symlink_after_the_scan_cannot_be_read_through_the_server() {
        let (router, info, scan, root, _dir) = server_over_real_files(&[("a.jpg", b"one"), ("b.jpg", b"two")]).await;
        let secret = tempfile::tempdir().unwrap();
        std::fs::write(secret.path().join("secret.txt"), b"top secret").unwrap();

        let (_, clusters) = call(&router, "GET", format!("/api/scans/{scan}/clusters?token={}", info.token), "").await;
        let id = clusters[0]["members"][0]["id"].as_str().unwrap().to_string(); // a.jpg
        // a.jpg is replaced with a link to a file outside the folder
        std::fs::remove_file(root.join("a.jpg")).unwrap();
        std::os::unix::fs::symlink(secret.path().join("secret.txt"), root.join("a.jpg")).unwrap();

        for kind in ["original", "thumb", "preview"] {
            let res = send(&router, axum::http::Request::get(format!("/files/{id}/{kind}?token={}", info.token)).body(axum::body::Body::empty()).unwrap()).await;
            assert_eq!(res.status(), 404, "{kind}");
        }
    }

    #[tokio::test]
    async fn the_scan_intensity_setting_defaults_validates_and_persists_across_restarts() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let (_l, router, info) = build(tmp.path()).await.unwrap();
        let t = &info.token;

        let (s, got) = call(&router, "GET", format!("/api/settings?token={t}"), "").await;
        assert_eq!(s, 200);
        assert_eq!(got["scan_intensity"], "default");
        let ids: Vec<&str> = got["tiers"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["efficient", "default", "performance"]);
        let workers = |g: &serde_json::Value, id: &str| g["tiers"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap()["workers"].as_u64().unwrap();
        assert!(workers(&got, "efficient") <= workers(&got, "default") && workers(&got, "default") <= workers(&got, "performance"));
        assert_eq!(workers(&got, "performance"), got["logical_cores"].as_u64().unwrap());

        for bad in [r#"{"scan_intensity":"turbo"}"#, r#"{"scan_intensity":"4"}"#, r#"{"scan_intensity":""}"#] {
            let (s, _) = call(&router, "PUT", format!("/api/settings?token={t}"), bad).await;
            assert_eq!(s, 400, "{bad}");
        }
        let (_, still) = call(&router, "GET", format!("/api/settings?token={t}"), "").await;
        assert_eq!(still["scan_intensity"], "default", "a rejected value changes nothing");

        let (s, saved) = call(&router, "PUT", format!("/api/settings?token={t}"), r#"{"scan_intensity":"efficient"}"#).await;
        assert_eq!(s, 200);
        assert_eq!(saved["scan_intensity"], "efficient");

        // a fresh server on the same database (a restart) still has it
        let (_l2, router2, info2) = build(tmp.path()).await.unwrap();
        let (_, after) = call(&router2, "GET", format!("/api/settings?token={}", info2.token), "").await;
        assert_eq!(after["scan_intensity"], "efficient");
    }

    #[tokio::test]
    async fn the_advanced_thresholds_have_ranges_validate_as_a_whole_and_persist() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let (_l, router, info) = build(tmp.path()).await.unwrap();
        let t = &info.token;
        let url = format!("/api/settings?token={t}");

        let (_, got) = call(&router, "GET", url.clone(), "").await;
        let a = &got["advanced"];
        assert_eq!((a["match_distance"]["value"].as_i64(), a["match_distance"]["default"].as_i64()), (Some(45), Some(45)));
        assert_eq!((a["sequence_similarity"]["min"].as_i64(), a["sequence_similarity"]["max"].as_i64()), (Some(40), Some(100)));
        assert_eq!(a["sequence_gap_seconds"]["value"], 3);

        // a partial update changes only what was sent
        let (s, saved) = call(&router, "PUT", url.clone(), r#"{"sequence_gap_seconds":10}"#).await;
        assert_eq!(s, 200);
        assert_eq!(saved["advanced"]["sequence_gap_seconds"]["value"], 10);
        assert_eq!(saved["advanced"]["match_distance"]["value"], 45);
        assert_eq!(saved["scan_intensity"], "default");

        // out-of-range, negative and mixed requests are refused whole
        for bad in [
            r#"{"match_distance":24}"#, r#"{"match_distance":61}"#, r#"{"match_distance":-5}"#, r#"{"sequence_similarity":101}"#,
            r#"{"sequence_gap_seconds":0}"#, r#"{"sequence_gap_seconds":31}"#,
            r#"{"scan_intensity":"performance","match_distance":999}"#,
        ] {
            let (s, _) = call(&router, "PUT", url.clone(), bad).await;
            assert_eq!(s, 400, "{bad}");
        }
        let (_, after) = call(&router, "GET", url.clone(), "").await;
        assert_eq!(after["scan_intensity"], "default", "the valid half of a refused request was not saved");
        assert_eq!(after["advanced"]["sequence_gap_seconds"]["value"], 10);

        // range limits are inclusive, and everything survives a restart
        let (s, _) = call(&router, "PUT", url, r#"{"match_distance":25,"sequence_similarity":100}"#).await;
        assert_eq!(s, 200);
        let (_l2, router2, info2) = build(tmp.path()).await.unwrap();
        let (_, again) = call(&router2, "GET", format!("/api/settings?token={}", info2.token), "").await;
        assert_eq!(again["advanced"]["match_distance"]["value"], 25);
        assert_eq!(again["advanced"]["sequence_similarity"]["value"], 100);
        assert_eq!(again["advanced"]["sequence_gap_seconds"]["value"], 10);
    }

    #[tokio::test]
    async fn shell_status_reports_the_tools_and_whether_a_native_picker_exists() {
        let (router, info, _scan, _root) = test_server_with_scan().await;
        let (s, got) = call(&router, "GET", format!("/api/shell?token={}", info.token), "").await;
        assert_eq!(s, 200);
        assert_eq!(got["native_folder_picker"], false, "a plain browser has no native dialog");
        let names: Vec<&str> = got["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["ffmpeg", "ffprobe"]);
        assert!(got["tools"][0]["found"].is_boolean());

        let (s, _) = call(&router, "GET", "/api/shell".to_string(), "").await;
        assert_eq!(s, 401, "needs the token like everything else");
    }

    #[tokio::test]
    async fn the_native_folder_picker_is_offered_only_when_a_shell_provides_one() {
        let tmp = tempfile::NamedTempFile::new().unwrap();

        // no shell: the route exists but says it is not implemented
        let (_l, plain, info) = build(tmp.path()).await.unwrap();
        let (s, _) = call(&plain, "POST", format!("/api/pick-folder?token={}", info.token), "").await;
        assert_eq!(s, 501);

        // a shell that returns a folder
        let chosen: FolderPicker = Arc::new(|| Some(PathBuf::from("/Users/me/Pictures")));
        let (_l, with, info) = build_with(tmp.path(), Config { folder_picker: Some(chosen), ..Config::default() }).await.unwrap();
        let (_, shell) = call(&with, "GET", format!("/api/shell?token={}", info.token), "").await;
        assert_eq!(shell["native_folder_picker"], true);
        let (s, picked) = call(&with, "POST", format!("/api/pick-folder?token={}", info.token), "").await;
        assert_eq!(s, 200);
        assert_eq!(picked["path"], "/Users/me/Pictures");

        // the user cancels the dialog
        let cancelled: FolderPicker = Arc::new(|| None);
        let (_l, cancel, info) = build_with(tmp.path(), Config { folder_picker: Some(cancelled), ..Config::default() }).await.unwrap();
        let (s, picked) = call(&cancel, "POST", format!("/api/pick-folder?token={}", info.token), "").await;
        assert_eq!(s, 200);
        assert!(picked["path"].is_null());

        // and it is behind the token
        let (s, _) = call(&with, "POST", "/api/pick-folder".to_string(), "").await;
        assert_eq!(s, 401);
    }

    /// Only with `--features embed-ui` (needs `npm run build` in `ui/` first).
    #[cfg(feature = "embed-ui")]
    #[tokio::test]
    async fn the_compiled_in_ui_is_served_with_a_fallback_and_the_api_stays_protected() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let (_l, router, _info) = build(tmp.path()).await.unwrap();
        for path in ["/", "/some/client/route"] {
            let res = send(&router, axum::http::Request::get(path).body(axum::body::Body::empty()).unwrap()).await;
            assert_eq!(res.status(), 200, "{path}");
            assert!(res.headers()["content-type"].to_str().unwrap().starts_with("text/html"), "{path}");
        }
        // a real built asset is served with its own type
        let index = axum::body::to_bytes(send(&router, axum::http::Request::get("/").body(axum::body::Body::empty()).unwrap()).await.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8_lossy(&index);
        let js = html.split("src=\"").nth(1).and_then(|r| r.split('"').next()).expect("index.html references a script");
        let res = send(&router, axum::http::Request::get(js.trim_start_matches('.')).body(axum::body::Body::empty()).unwrap()).await;
        assert_eq!(res.status(), 200);
        assert!(res.headers()["content-type"].to_str().unwrap().contains("javascript"));

        let res = send(&router, axum::http::Request::get("/api/scans").body(axum::body::Body::empty()).unwrap()).await;
        assert_eq!(res.status(), 401);
    }

    #[tokio::test]
    async fn a_stale_scan_id_is_not_found_everywhere_not_empty_and_not_a_server_error() {
        let (router, info, _scan_id, _root) = test_server_with_scan().await;
        let stale = uuid::Uuid::new_v4();
        let t = &info.token;
        let json = |m: &str, uri: String, body: &str| {
            axum::http::Request::builder()
                .method(m)
                .uri(uri)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap()
        };
        let cases = [
            json("GET", format!("/api/scans/{stale}/info?token={t}"), ""),
            json("GET", format!("/api/scans/{stale}/clusters?token={t}"), ""),
            json("GET", format!("/api/scans/{stale}/review-pairs?token={t}"), ""),
            json("GET", format!("/api/scans/{stale}/burst-runs?token={t}"), ""),
            json("GET", format!("/api/scans/{stale}/orphan-videos?token={t}"), ""),
            json("GET", format!("/api/scans/{stale}/apply/plan?token={t}"), ""),
            json("POST", format!("/api/scans/{stale}/apply?token={t}"), r#"{"confirm":true}"#),
            json("GET", format!("/api/scans/{stale}/decisions?token={t}"), ""),
            json("PUT", format!("/api/scans/{stale}/decisions?token={t}"), r#"{"relative_path":"a.jpg","keep":true}"#),
            json("POST", format!("/api/scans/{stale}/decisions/bulk?token={t}"), r#"[{"relative_path":"a.jpg","keep":true}]"#),
            json("DELETE", format!("/api/scans/{stale}/decisions?relative_path=a.jpg&token={t}"), ""),
        ];
        for req in cases {
            let desc = format!("{} {}", req.method(), req.uri().path());
            assert_eq!(send(&router, req).await.status(), 404, "{desc}");
        }
    }

    #[tokio::test]
    async fn bulk_decisions_and_clear_work_over_http() {
        let (router, info, scan_id, _root) = test_server_with_scan().await;
        let base = format!("/api/scans/{scan_id}/decisions");
        let t = &info.token;

        let res = send(
            &router,
            axum::http::Request::post(format!("{base}/bulk?token={t}"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"[{"relative_path":"a.jpg","keep":true},{"relative_path":"b.jpg","keep":false}]"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), 204);

        let res = send(
            &router,
            axum::http::Request::delete(format!("{base}?relative_path=a.jpg&token={t}")).body(axum::body::Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(res.status(), 204);

        let res = send(&router, axum::http::Request::get(format!("{base}?token={t}")).body(axum::body::Body::empty()).unwrap()).await;
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let got: HashMap<String, bool> = serde_json::from_slice(&body).unwrap();
        assert_eq!(got, HashMap::from([("b.jpg".to_string(), false)]));
    }

    #[tokio::test]
    async fn resume_finds_the_scan_for_a_known_root_and_nothing_for_an_unknown_one() {
        let (router, info, scan_id, root) = test_server_with_scan().await;

        let known_uri = format!("/api/resume?root={}&token={}", root.display(), info.token);
        let res = send(&router, axum::http::Request::get(known_uri).body(axum::body::Body::empty()).unwrap()).await;
        assert_eq!(res.status(), 200);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let dto: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(dto["scan_id"], scan_id);

        let unknown_uri = format!("/api/resume?root=/nothing/scanned/here&token={}", info.token);
        let res = send(&router, axum::http::Request::get(unknown_uri).body(axum::body::Body::empty()).unwrap()).await;
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let dto: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(dto["scan_id"].is_null());
    }

    #[tokio::test]
    async fn decisions_route_also_requires_the_token() {
        let (router, _info, scan_id, _root) = test_server_with_scan().await;
        let uri = format!("/api/scans/{scan_id}/decisions");
        let res = send(&router, axum::http::Request::get(uri).body(axum::body::Body::empty()).unwrap()).await;
        assert_eq!(res.status(), 401);
    }
}
