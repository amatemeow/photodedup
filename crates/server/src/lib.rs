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

use axum::routing::get;
use axum::Router;
use state::AppState;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

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
    let store = photodedup_core::store::Store::open(db_path)?;
    let token = auth::new_token();
    let cache_dir = routes::files::init_cache_dir(db_path)?;
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        token: Arc::from(token.as_str()),
        cache_dir: Arc::new(cache_dir),
    };

    let protected = Router::new()
        .route("/api/scans", get(routes::scans::list_scans))
        .route("/api/scans/{id}/clusters", get(routes::scans::list_clusters))
        .route("/api/scans/{id}/review-pairs", get(routes::scans::list_review_pairs))
        .route("/api/scans/{id}/burst-runs", get(routes::scans::list_burst_runs))
        .route("/api/files/{id}/analysis", get(routes::scans::get_analysis))
        .route(
            "/api/scans/{id}/decisions",
            get(routes::decisions::get_decisions).put(routes::decisions::put_decision),
        )
        .route("/api/resume", get(routes::decisions::resume))
        .route("/files/{id}/thumb", get(routes::files::thumbnail))
        .route("/files/{id}/preview", get(routes::files::preview))
        .route("/files/{id}/original", get(routes::files::original))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth::require_token))
        .with_state(state);

    let public = Router::new()
        .route("/health", get(|| async { "ok" }))
        // Placeholder until the real UI lands -- `ServerInfo::url()` points
        // here, and auto-opening the browser at a bare 404 is a bad first
        // impression. Unauthenticated (static text, nothing from the
        // database) so it's never itself the thing that breaks if the
        // token is wrong -- a wrong/missing token shows this page with a
        // visible explanation rather than axum's bare 404.
        .route("/", get(placeholder_page));

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
<p>The review UI isn't built yet (M2 in progress) -- this is a
placeholder so the URL this process opens doesn't just 404.</p>
<p>The API is live, e.g. <code>GET /api/scans?token=...</code>.</p>
</body>"#,
    )
}

/// Binds and serves until the process is killed. Returns `ServerInfo`
/// immediately (before the serve loop, which runs in the background) so
/// the caller can print/open the URL right away.
pub async fn run(db_path: PathBuf) -> anyhow::Result<ServerInfo> {
    let (listener, router, info) = build(&db_path).await?;
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
    async fn test_server_with_scan() -> (Router, ServerInfo, i64, std::path::PathBuf) {
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
        let uri = format!("/files/999/original?token={}", info.token);
        let res = router
            .oneshot(axum::http::Request::get(uri).body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 404);
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
