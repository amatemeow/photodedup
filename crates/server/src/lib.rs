//! Loopback HTTP server for the review UI. Security properties (per the
//! architecture plan): bound to `127.0.0.1` only, a random OS-assigned
//! port (never a fixed/well-known one), a session token required on every
//! request (`auth.rs`), and -- most importantly -- every file-serving
//! endpoint resolves a filesystem path *only* through the database
//! (`Store::get_file_full_path`), never from anything in the request
//! (`routes/files.rs`). No path parameter ever reaches the filesystem
//! directly.

pub mod auth;
mod routes;
mod state;

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

/// `build`, serving the built web UI from `ui_dir` (a Vite `dist/`) at `/`
/// when given. Without one, `/` is a plain placeholder page. The UI's
/// files are public static assets -- no token needed to fetch them, and
/// nothing secret in them; the token only matters for the API, and the UI
/// picks it up from the URL the server opened.
pub async fn build_with_ui(
    db_path: &std::path::Path,
    ui_dir: Option<PathBuf>,
) -> anyhow::Result<(TcpListener, Router, ServerInfo)> {
    let store = photodedup_core::store::Store::open(db_path)?;
    let token = auth::new_token();
    let cache_dir = routes::files::init_cache_dir(db_path)?;
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        token: Arc::from(token.as_str()),
        cache_dir: Arc::new(cache_dir),
        job: Arc::new(Mutex::new(None)),
    };

    let protected = Router::new()
        .route("/api/scans", get(routes::scans::list_scans))
        .route("/api/scans/{id}/clusters", get(routes::scans::list_clusters))
        .route("/api/scans/{id}/review-pairs", get(routes::scans::list_review_pairs))
        .route("/api/scans/{id}/burst-runs", get(routes::scans::list_burst_runs))
        .route("/api/scans/start", post(routes::jobs::start_scan))
        .route("/api/jobs/{id}", get(routes::jobs::job_status))
        .route("/api/jobs/{id}/cancel", post(routes::jobs::cancel_job))
        .route("/api/jobs/{id}/events", get(routes::jobs::job_events))
        .route("/api/scans/{id}/orphan-videos", get(routes::scans::list_orphan_videos))
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
        // Unauthenticated static text, so a wrong or missing token shows
        // an explanation rather than axum's bare 404.
        None => public.route("/", get(placeholder_page)),
    };

    let router = public.merge(protected);

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    Ok((listener, router, ServerInfo { addr, token }))
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
    let (listener, router, info) = build_with_ui(&db_path, ui_dir).await?;
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
        let analysis = |sharpness| Analyzed {
            provenance: Provenance { origin: Origin::CameraOriginal, confidence: 1.0, evidence: vec![] },
            phash: None,
            sharpness: Some(sharpness),
            width: None,
            height: None,
            size_bytes: 10,
            camera_model: None,
            capture_time: None,
            duration_seconds: None,
        };
        let result = photodedup_core::pipeline::PipelineResult {
            files: vec![
                rec(0, "f0.jpg", MediaClassSerde::Image),
                rec(1, "f1.jpg", MediaClassSerde::Image),
                rec(2, "f2.jpg", MediaClassSerde::Image),
                rec(3, "v.mp4", MediaClassSerde::Video),
            ],
            analyzed: [(0, analysis(10.0)), (1, analysis(99.0)), (2, analysis(50.0))].into_iter().collect(),
            clusters: vec![],
            review_pairs: vec![],
            burst_runs: vec![photodedup_core::resolve::BurstRun { camera_model: "Cam".into(), files: vec![0, 1, 2] }],
            video_matches: vec![],
            orphan_videos: vec![3],
            unknown_provenance_videos: vec![],
            placeholder_count: 0,
            placeholder_bytes: 0,
            refused_managed_libraries: vec![],
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

        let orphans = get(format!("/api/scans/{scan_id}/orphan-videos?token={}", info.token)).await;
        assert_eq!(orphans[0]["file"]["file_name"], "v.mp4");
        assert_eq!(orphans[0]["reason"], "no_match");
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
            json("GET", format!("/api/scans/{stale}/clusters?token={t}"), ""),
            json("GET", format!("/api/scans/{stale}/review-pairs?token={t}"), ""),
            json("GET", format!("/api/scans/{stale}/burst-runs?token={t}"), ""),
            json("GET", format!("/api/scans/{stale}/orphan-videos?token={t}"), ""),
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
