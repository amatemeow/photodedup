//! Read endpoints over the SQLite store: scan listing/summary, clusters,
//! review pairs, burst runs -- the data the three-pane review UI renders.
//! All joins (file details into cluster members, etc.) happen here, not
//! in `Store`, which stays a lean normalized-data layer.

use crate::state::AppState;
use axum::extract::{Path as AxPath, State};
use axum::http::StatusCode;
use axum::Json;
use photodedup_core::model::MediaClassSerde;
use photodedup_core::store::{StoredAnalysis, StoredFile};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Serialize)]
pub struct ScanSummaryDto {
    id: String,
    root_path: String,
    started_at: String,
    file_count: usize,
    cluster_count: usize,
    review_pair_count: usize,
    burst_run_count: usize,
}

pub async fn list_scans(State(state): State<AppState>) -> Result<Json<Vec<ScanSummaryDto>>, StatusCode> {
    let store = state.store.lock().unwrap();
    let scans = store.list_scans().map_err(internal)?;
    let mut out = Vec::with_capacity(scans.len());
    for s in scans {
        out.push(ScanSummaryDto {
            id: s.id.clone(),
            root_path: s.root_path,
            started_at: s.started_at,
            file_count: store.load_files(&s.id).map_err(internal)?.len(),
            cluster_count: store.load_clusters(&s.id).map_err(internal)?.len(),
            review_pair_count: store.load_review_pairs(&s.id).map_err(internal)?.len(),
            burst_run_count: store.load_burst_runs(&s.id).map_err(internal)?.len(),
        });
    }
    Ok(Json(out))
}

#[derive(Serialize)]
pub struct FileDto {
    pub id: String,
    /// What decisions are keyed by (stable across re-scans, unlike `id`).
    pub relative_path: String,
    pub file_name: String,
    pub media_class: MediaClassSerde,
    pub size_bytes: u64,
    pub is_placeholder: bool,
}

impl From<&StoredFile> for FileDto {
    fn from(f: &StoredFile) -> Self {
        FileDto {
            id: f.id.clone(),
            relative_path: f.relative_path.clone(),
            file_name: f.file_name.clone(),
            media_class: f.media_class,
            size_bytes: f.size_bytes,
            is_placeholder: f.is_placeholder,
        }
    }
}

#[derive(Serialize)]
pub struct AnalysisDto {
    pub origin: String,
    pub confidence: f64,
    pub evidence: Vec<photodedup_core::provenance::Evidence>,
    pub sharpness: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub camera_model: Option<String>,
    pub capture_time: Option<String>,
    pub duration_seconds: Option<f64>,
}

impl From<StoredAnalysis> for AnalysisDto {
    fn from(a: StoredAnalysis) -> Self {
        AnalysisDto {
            origin: a.origin,
            confidence: a.confidence,
            evidence: a.evidence,
            sharpness: a.sharpness,
            width: a.width,
            height: a.height,
            camera_model: a.camera_model,
            capture_time: a.capture_time,
            duration_seconds: a.duration_seconds,
        }
    }
}

#[derive(Serialize)]
pub struct ClusterDto {
    pub id: String,
    pub tier: String,
    pub keeper_file_id: String,
    pub reasons: Vec<String>,
    pub members: Vec<FileDto>,
}

pub async fn list_clusters(
    State(state): State<AppState>,
    AxPath(scan_id): AxPath<String>,
) -> Result<Json<Vec<ClusterDto>>, StatusCode> {
    let store = state.store.lock().unwrap();
    super::require_scan(&store, &scan_id)?;
    let clusters = store.load_clusters(&scan_id).map_err(internal)?;
    let files = file_lookup(&store, &scan_id).map_err(internal)?;

    let out = clusters
        .into_iter()
        .map(|c| ClusterDto {
            id: c.id,
            tier: c.tier,
            keeper_file_id: c.keeper_file_id,
            reasons: c.reasons,
            members: c.member_file_ids.iter().filter_map(|id| files.get(id)).map(FileDto::from).collect(),
        })
        .collect();
    Ok(Json(out))
}

#[derive(Serialize)]
pub struct ReviewPairDto {
    pub id: String,
    pub tier: String,
    pub reason: String,
    pub phash_dist: i64,
    pub file_a: Option<FileDto>,
    pub file_b: Option<FileDto>,
}

pub async fn list_review_pairs(
    State(state): State<AppState>,
    AxPath(scan_id): AxPath<String>,
) -> Result<Json<Vec<ReviewPairDto>>, StatusCode> {
    let store = state.store.lock().unwrap();
    super::require_scan(&store, &scan_id)?;
    let pairs = store.load_review_pairs(&scan_id).map_err(internal)?;
    let files = file_lookup(&store, &scan_id).map_err(internal)?;

    let out = pairs
        .into_iter()
        .map(|p| ReviewPairDto {
            id: p.id,
            tier: p.tier,
            reason: p.reason,
            phash_dist: p.phash_dist,
            file_a: files.get(&p.file_a_id).map(FileDto::from),
            file_b: files.get(&p.file_b_id).map(FileDto::from),
        })
        .collect();
    Ok(Json(out))
}

#[derive(Serialize)]
pub struct BurstRunDto {
    pub id: String,
    pub camera_model: String,
    pub frames: Vec<FileDto>,
}

pub async fn list_burst_runs(
    State(state): State<AppState>,
    AxPath(scan_id): AxPath<String>,
) -> Result<Json<Vec<BurstRunDto>>, StatusCode> {
    let store = state.store.lock().unwrap();
    super::require_scan(&store, &scan_id)?;
    let runs = store.load_burst_runs(&scan_id).map_err(internal)?;
    let files = file_lookup(&store, &scan_id).map_err(internal)?;

    let out = runs
        .into_iter()
        .map(|r| BurstRunDto {
            id: r.id,
            camera_model: r.camera_model,
            frames: r.member_file_ids.iter().filter_map(|id| files.get(id)).map(FileDto::from).collect(),
        })
        .collect();
    Ok(Json(out))
}

pub async fn get_analysis(
    State(state): State<AppState>,
    AxPath(file_id): AxPath<Uuid>,
) -> Result<Json<AnalysisDto>, StatusCode> {
    let store = state.store.lock().unwrap();
    store
        .load_analysis(&file_id.to_string())
        .map_err(internal)?
        .map(|a| Json(a.into()))
        .ok_or(StatusCode::NOT_FOUND)
}

fn file_lookup(store: &photodedup_core::store::Store, scan_id: &str) -> rusqlite::Result<HashMap<String, StoredFile>> {
    Ok(store.load_files(&scan_id)?.into_iter().map(|f| (f.id.clone(), f)).collect())
}

fn internal<E: std::fmt::Display>(e: E) -> StatusCode {
    eprintln!("server error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}
