//! Decision read/write, and the "resume where I left off" lookup --
//! together, the thing that makes a review session survive quitting
//! mid-way through. See `Store::find_latest_scan_for_root`'s doc comment
//! for why opening the app on a known folder should resume rather than
//! re-scan.

use crate::state::AppState;
use axum::extract::{Path as AxPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use super::require_scan;

pub async fn get_decisions(
    State(state): State<AppState>,
    AxPath(scan_id): AxPath<String>,
) -> Result<Json<HashMap<String, bool>>, StatusCode> {
    let store = state.store.lock().unwrap();
    require_scan(&store, &scan_id)?;
    store.load_decisions(&scan_id).map(Json).map_err(internal)
}

#[derive(Deserialize)]
pub struct DecisionBody {
    pub relative_path: String,
    pub keep: bool,
}

pub async fn put_decision(
    State(state): State<AppState>,
    AxPath(scan_id): AxPath<String>,
    Json(body): Json<DecisionBody>,
) -> Result<StatusCode, StatusCode> {
    let store = state.store.lock().unwrap();
    require_scan(&store, &scan_id)?;
    store
        .record_decision(&scan_id, &body.relative_path, body.keep)
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(internal)
}

/// `DELETE /api/scans/{id}/decisions?relative_path=...` -- back to
/// undecided (what undoing a first decision needs).
pub async fn clear_decision(
    State(state): State<AppState>,
    AxPath(scan_id): AxPath<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<StatusCode, StatusCode> {
    let Some(path) = params.get("relative_path") else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let store = state.store.lock().unwrap();
    require_scan(&store, &scan_id)?;
    store.clear_decision(&scan_id, path).map(|()| StatusCode::NO_CONTENT).map_err(internal)
}

/// `POST /api/scans/{id}/decisions/bulk` -- many decisions in one
/// transaction (a cluster's whole keep/quarantine split, or "accept all
/// suggestions"), so a failure can't leave one half-recorded.
pub async fn put_decisions_bulk(
    State(state): State<AppState>,
    AxPath(scan_id): AxPath<String>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<Vec<DecisionBody>>,
) -> Result<StatusCode, StatusCode> {
    let rows: Vec<(String, bool)> = body.into_iter().map(|d| (d.relative_path, d.keep)).collect();
    // `?manual=1`: chosen on single files in the library overview (see `decisions.manual`).
    let manual = params.get("manual").is_some_and(|v| v == "1");
    let store = state.store.lock().unwrap();
    require_scan(&store, &scan_id)?;
    store.record_decisions_as(&scan_id, &rows, manual).map(|()| StatusCode::NO_CONTENT).map_err(internal)
}

/// `POST /api/scans/{id}/decisions/clear` with a JSON array of relative
/// paths -- back to undecided for all of them in one transaction ("reset
/// this group / category / everything").
pub async fn clear_decisions_bulk(
    State(state): State<AppState>,
    AxPath(scan_id): AxPath<String>,
    Json(paths): Json<Vec<String>>,
) -> Result<StatusCode, StatusCode> {
    let mut store = state.store.lock().unwrap();
    require_scan(&store, &scan_id)?;
    store.clear_decisions(&scan_id, &paths).map(|()| StatusCode::NO_CONTENT).map_err(internal)
}

#[derive(Serialize)]
pub struct ResumeDto {
    pub scan_id: Option<String>,
    pub started_at: Option<String>,
}

/// `GET /api/resume?root=<path>` -- the entry point a UI calls on startup
/// (or when the user points it at a folder) to decide whether to resume
/// an existing scan or offer to start a fresh one. Deliberately read-only
/// and side-effect-free: it only looks, it never triggers a scan itself
/// (that's the separate scan-trigger endpoint, not built yet -- see
/// ROADMAP.md).
pub async fn resume(State(state): State<AppState>, Query(params): Query<HashMap<String, String>>) -> Result<Json<ResumeDto>, StatusCode> {
    let Some(root) = params.get("root") else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let store = state.store.lock().unwrap();
    let found = store.find_latest_scan_for_root(&PathBuf::from(root)).map_err(internal)?;
    Ok(Json(match found {
        Some(s) => ResumeDto { scan_id: Some(s.id), started_at: Some(s.started_at) },
        None => ResumeDto { scan_id: None, started_at: None },
    }))
}

fn internal<E: std::fmt::Display>(e: E) -> StatusCode {
    eprintln!("server error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}
