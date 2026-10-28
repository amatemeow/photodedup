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
    use tower::ServiceExt;

    async fn test_server() -> (Router, ServerInfo) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let (_listener, router, info) = build(tmp.path()).await.unwrap();
        std::mem::forget(tmp); // keep the file alive for the test's duration
        (router, info)
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
}
