//! Applying decisions: preview, apply, list batches, undo. Thin wrappers
//! over `photodedup_core::apply`, which does the real work and holds the
//! safety rules (nothing outside the scanned folder, nothing deleted,
//! nothing overwritten). All of them need the session token like every
//! other route, and moving files additionally needs an explicit
//! `{"confirm": true}` body, so a stray request can never move anything.

use crate::state::AppState;
use axum::extract::{Path as AxPath, State};
use axum::http::StatusCode;
use axum::Json;
use photodedup_core::apply::{self, ApplyError, BatchSummary, ExecuteResult, Plan, UndoResult};
use serde::Deserialize;
use uuid::Uuid;

fn status_for(e: &ApplyError) -> StatusCode {
    match e {
        ApplyError::NoSuchScan | ApplyError::NoSuchBatch => StatusCode::NOT_FOUND,
        ApplyError::NothingToApply | ApplyError::AlreadyUndone => StatusCode::CONFLICT,
        ApplyError::Guard(_) => StatusCode::UNPROCESSABLE_ENTITY,
        ApplyError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn fail(e: ApplyError) -> (StatusCode, String) {
    if status_for(&e) == StatusCode::INTERNAL_SERVER_ERROR {
        eprintln!("server error: {e}");
    }
    (status_for(&e), e.to_string())
}

/// `GET /api/scans/{id}/apply/plan` -- the dry run: what would move and
/// what was refused. Touches nothing.
pub async fn preview(State(state): State<AppState>, AxPath(scan_id): AxPath<String>) -> Result<Json<Plan>, (StatusCode, String)> {
    let store = state.store.lock().unwrap();
    apply::plan(&store, &scan_id).map(Json).map_err(fail)
}

#[derive(Deserialize)]
pub struct ApplyBody {
    #[serde(default)]
    pub confirm: bool,
}

/// `POST /api/scans/{id}/apply` with `{"confirm": true}` -- moves the marked
/// files into a new quarantine batch.
pub async fn run(
    State(state): State<AppState>,
    AxPath(scan_id): AxPath<String>,
    Json(body): Json<ApplyBody>,
) -> Result<Json<ExecuteResult>, (StatusCode, String)> {
    if !body.confirm {
        return Err((StatusCode::BAD_REQUEST, "send {\"confirm\": true} to move files".into()));
    }
    let store = state.store.clone();
    // Moving files is blocking work; keep it off the async workers.
    tokio::task::spawn_blocking(move || {
        let store = store.lock().unwrap();
        apply::execute(&store, &scan_id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map(Json)
    .map_err(fail)
}

/// `GET /api/batches` -- every batch, newest first, with how much of each
/// is still in quarantine.
pub async fn batches(State(state): State<AppState>) -> Result<Json<Vec<BatchSummary>>, (StatusCode, String)> {
    let store = state.store.lock().unwrap();
    apply::list_batches(&store).map(Json).map_err(fail)
}

/// `POST /api/batches/{id}/undo` -- puts a batch back. Safe to repeat.
pub async fn undo(State(state): State<AppState>, AxPath(batch_id): AxPath<Uuid>) -> Result<Json<UndoResult>, (StatusCode, String)> {
    let store = state.store.clone();
    let id = batch_id.to_string();
    tokio::task::spawn_blocking(move || {
        let store = store.lock().unwrap();
        apply::undo(&store, &id)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map(Json)
    .map_err(fail)
}
