//! Asking the server to get thumbnails and previews ready. Both are only
//! hints about what will be looked at: they change nothing the user sees
//! except how soon it appears, and what the cache keeps (see `warm.rs`).

use super::require_scan;
use crate::state::AppState;
use crate::warm::Status;
use axum::extract::{Path as AxPath, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

/// `POST /api/scans/{id}/prerender` -- start making the grid thumbnails for
/// every grouped photo, quietly in the background. Safe to repeat.
pub async fn start_prerender(State(state): State<AppState>, AxPath(scan_id): AxPath<String>) -> Result<Json<Status>, StatusCode> {
    require_scan(&state.store.lock().unwrap(), &scan_id)?;
    let warm = state.warm.clone();
    Ok(Json(warm.start_prerender(state, scan_id)))
}

/// `GET /api/scans/{id}/prerender` -- how far it has got.
pub async fn prerender_status(State(state): State<AppState>, AxPath(scan_id): AxPath<String>) -> Json<Status> {
    Json(state.warm.status(&scan_id))
}

#[derive(Deserialize)]
pub struct Window {
    /// The photos around where the user is looking, most likely first.
    /// Empty when they are not looking at any (it releases the last window).
    pub ids: Vec<Uuid>,
}

/// `POST /api/prefetch` -- see `Warmer::window`.
pub async fn window(State(state): State<AppState>, Json(body): Json<Window>) -> StatusCode {
    let warm = state.warm.clone();
    warm.window(state, body.ids);
    StatusCode::ACCEPTED
}

/// `DELETE /api/cache` -- empties the cache. It fills again as photos are looked at.
pub async fn clear(State(state): State<AppState>) -> StatusCode {
    state.warm.cancel_prerender();
    state.cache.clear();
    StatusCode::NO_CONTENT
}
