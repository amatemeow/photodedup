//! Trying matching settings on a scan that already exists. The fingerprints
//! and the links no setting changes are saved with the scan, so grouping can
//! be redone from them in a moment, without reading the folder again:
//! `preview` says what a setting would change and touches nothing, `apply`
//! does it. Both answer 409 for a scan saved before this was possible.

use crate::state::AppState;
use axum::extract::{Path as AxPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use photodedup_core::grouping::{group, Counts};
use photodedup_core::store::StoredGroupingData;
use photodedup_core::thresholds::Thresholds;
use serde::{Deserialize, Serialize};

type Fail = (StatusCode, String);

const OLD_SCAN: &str = "this scan was saved before settings could be tried on it; scan the folder again";

fn io(e: rusqlite::Error) -> Fail {
    eprintln!("server error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// What a scan was made with, and everything needed to group it again. The
/// store lock is held only for the loads.
fn load(state: &AppState, scan_id: &str) -> Result<(StoredGroupingData, Thresholds), Fail> {
    let store = state.store.lock().unwrap();
    let (used, _) = store.get_scan_settings(scan_id).map_err(io)?.ok_or((StatusCode::CONFLICT, OLD_SCAN.into()))?;
    let data = store.load_grouping_data(scan_id).map_err(io)?.ok_or((StatusCode::CONFLICT, OLD_SCAN.into()))?;
    Ok((data, used))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, Fail> + Send + 'static) -> Result<T, Fail> {
    tokio::task::spawn_blocking(f).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}

#[derive(Deserialize)]
pub struct Candidate {
    pub match_distance: u32,
    pub sequence_similarity: u32,
    pub sequence_gap_seconds: i64,
}

#[derive(Serialize)]
pub struct Preview {
    /// What the review shows now.
    pub current: Counts,
    /// What it would show with the requested values.
    pub proposed: Counts,
}

/// `GET /api/scans/{id}/regroup/preview?match_distance=..&sequence_similarity=..&sequence_gap_seconds=..`
pub async fn preview(State(state): State<AppState>, AxPath(scan_id): AxPath<String>, Query(c): Query<Candidate>) -> Result<Json<Preview>, Fail> {
    let proposed = Thresholds { match_distance: c.match_distance, sequence_similarity: c.sequence_similarity, sequence_gap_seconds: c.sequence_gap_seconds };
    proposed.validate().map_err(|m| (StatusCode::BAD_REQUEST, m))?;
    blocking(move || {
        let (data, used) = load(&state, &scan_id)?;
        let current = group(&data.input(), &used).counts();
        let proposed = if proposed == used { current } else { group(&data.input(), &proposed).counts() };
        Ok(Json(Preview { current, proposed }))
    })
    .await
}

/// `POST /api/scans/{id}/regroup` -- groups the scan again with the settings
/// saved now and replaces what the review shows. Choices stay. Returns the
/// new counts.
pub async fn apply(State(state): State<AppState>, AxPath(scan_id): AxPath<String>) -> Result<Json<Counts>, Fail> {
    blocking(move || {
        let (data, _) = load(&state, &scan_id)?;
        let wanted = super::settings::stored_thresholds(&state.store.lock().unwrap());
        let grouping = group(&data.input(), &wanted);
        state.store.lock().unwrap().replace_grouping(&scan_id, &data, &grouping, &wanted).map_err(io)?;
        Ok(Json(grouping.counts()))
    })
    .await
}
