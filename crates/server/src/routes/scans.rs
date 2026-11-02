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

/// What a scan was made with, so the review can show it and notice when the
/// saved settings have moved on since.
#[derive(Serialize)]
pub struct UsedSettings {
    pub match_distance: u32,
    pub sequence_similarity: u32,
    pub sequence_gap_seconds: i64,
    pub include_videos: bool,
}

#[derive(Serialize)]
pub struct ScanInfoDto {
    pub id: String,
    pub root_path: String,
    pub started_at: String,
    /// `None` for a scan saved before this was recorded.
    pub used: Option<UsedSettings>,
}

pub async fn scan_info(State(state): State<AppState>, AxPath(scan_id): AxPath<String>) -> Result<Json<ScanInfoDto>, StatusCode> {
    let store = state.store.lock().unwrap();
    let scan = store.get_scan(&scan_id).map_err(internal)?.ok_or(StatusCode::NOT_FOUND)?;
    let used = store.get_scan_settings(&scan_id).map_err(internal)?.map(|(t, include_videos)| UsedSettings {
        match_distance: t.match_distance,
        sequence_similarity: t.sequence_similarity,
        sequence_gap_seconds: t.sequence_gap_seconds,
        include_videos,
    });
    Ok(Json(ScanInfoDto { id: scan.id, root_path: scan.root_path, started_at: scan.started_at, used }))
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
    /// Pixel size when known; the overview shapes its cards by it.
    pub width: Option<u32>,
    pub height: Option<u32>,
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
            width: f.width,
            height: f.height,
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
    /// When the photo was taken, from the original: the suggested keeper's
    /// capture time, else the earliest any member has. A copy that was
    /// re-saved or forwarded carries the date of that save, not of the shot.
    pub captured_at: Option<String>,
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

    let mut out = Vec::with_capacity(clusters.len());
    for c in clusters {
        let keeper_time = store.load_analysis(&c.keeper_file_id).map_err(internal)?.and_then(|a| a.capture_time);
        let captured_at = match keeper_time {
            Some(t) => Some(t),
            None => earliest_capture(&store, &c.member_file_ids).map_err(internal)?,
        };
        out.push(ClusterDto {
            id: c.id,
            tier: c.tier,
            keeper_file_id: c.keeper_file_id,
            captured_at,
            reasons: c.reasons,
            members: c.member_file_ids.iter().filter_map(|id| files.get(id)).map(FileDto::from).collect(),
        });
    }
    Ok(Json(out))
}

#[derive(Serialize)]
pub struct ReviewPairDto {
    pub id: String,
    pub tier: String,
    pub reason: String,
    pub phash_dist: i64,
    /// Earliest capture time of the two photos.
    pub captured_at: Option<String>,
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

    let mut out = Vec::with_capacity(pairs.len());
    for p in pairs {
        let captured_at = earliest_capture(&store, &[p.file_a_id.clone(), p.file_b_id.clone()]).map_err(internal)?;
        out.push(ReviewPairDto {
            id: p.id,
            tier: p.tier,
            reason: p.reason,
            phash_dist: p.phash_dist,
            captured_at,
            file_a: files.get(&p.file_a_id).map(FileDto::from),
            file_b: files.get(&p.file_b_id).map(FileDto::from),
        });
    }
    Ok(Json(out))
}

#[derive(Serialize)]
pub struct BurstRunDto {
    pub id: String,
    pub camera_model: String,
    /// In capture-time order.
    pub frames: Vec<FileDto>,
    /// The first frame's capture time.
    pub captured_at: Option<String>,
    /// Laplacian-variance sharpness per frame id (where analysis exists).
    pub sharpness: HashMap<String, f64>,
    /// The frame suggested to keep when thinning the run: the sharpest.
    pub suggested_file_id: Option<String>,
}

pub async fn list_burst_runs(
    State(state): State<AppState>,
    AxPath(scan_id): AxPath<String>,
) -> Result<Json<Vec<BurstRunDto>>, StatusCode> {
    let store = state.store.lock().unwrap();
    super::require_scan(&store, &scan_id)?;
    let runs = store.load_burst_runs(&scan_id).map_err(internal)?;
    let files = file_lookup(&store, &scan_id).map_err(internal)?;

    let mut out = Vec::with_capacity(runs.len());
    for r in runs {
        let frames: Vec<FileDto> = r.member_file_ids.iter().filter_map(|id| files.get(id)).map(FileDto::from).collect();
        let mut sharpness = HashMap::new();
        let mut times = Vec::new();
        for f in &frames {
            if let Some(a) = store.load_analysis(&f.id).map_err(internal)? {
                if let Some(sh) = a.sharpness {
                    sharpness.insert(f.id.clone(), sh);
                }
                times.extend(a.capture_time);
            }
        }
        let captured_at = times.into_iter().min();
        let suggested_file_id = frames
            .iter()
            .filter_map(|f| sharpness.get(&f.id).map(|&sh| (f, sh)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(f, _)| f.id.clone())
            .or_else(|| frames.first().map(|f| f.id.clone()));
        out.push(BurstRunDto { id: r.id, camera_model: r.camera_model, frames, captured_at, sharpness, suggested_file_id });
    }
    Ok(Json(out))
}

#[derive(Serialize)]
pub struct OrphanVideoDto {
    pub file: FileDto,
    pub captured_at: Option<String>,
    /// `no_match` or `unknown_origin`.
    pub reason: String,
}

/// Videos with no counterpart to compare against. Decided like everything
/// else, with everything kept by default.
pub async fn list_orphan_videos(
    State(state): State<AppState>,
    AxPath(scan_id): AxPath<String>,
) -> Result<Json<Vec<OrphanVideoDto>>, StatusCode> {
    let store = state.store.lock().unwrap();
    super::require_scan(&store, &scan_id)?;
    let orphans = store.load_orphan_videos(&scan_id).map_err(internal)?;
    let files = file_lookup(&store, &scan_id).map_err(internal)?;
    let mut out = Vec::with_capacity(orphans.len());
    for o in orphans {
        let Some(f) = files.get(&o.file_id) else { continue };
        let captured_at = earliest_capture(&store, std::slice::from_ref(&o.file_id)).map_err(internal)?;
        out.push(OrphanVideoDto { file: FileDto::from(f), captured_at, reason: o.reason });
    }
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

/// Earliest capture time among these files. Capture times are stored as
/// `YYYY-MM-DD HH:MM:SS`, which sorts correctly as text.
fn earliest_capture(store: &photodedup_core::store::Store, file_ids: &[String]) -> rusqlite::Result<Option<String>> {
    let mut best: Option<String> = None;
    for id in file_ids {
        if let Some(t) = store.load_analysis(id)?.and_then(|a| a.capture_time) {
            if best.as_ref().is_none_or(|b| t < *b) {
                best = Some(t);
            }
        }
    }
    Ok(best)
}

fn file_lookup(store: &photodedup_core::store::Store, scan_id: &str) -> rusqlite::Result<HashMap<String, StoredFile>> {
    Ok(store.load_files(&scan_id)?.into_iter().map(|f| (f.id.clone(), f)).collect())
}

fn internal<E: std::fmt::Display>(e: E) -> StatusCode {
    eprintln!("server error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

#[derive(Serialize)]
pub struct LibraryFileDto {
    pub id: String,
    pub relative_path: String,
    pub file_name: String,
    pub media_class: MediaClassSerde,
    pub size_bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// From the file's own metadata, `YYYY-MM-DD HH:MM:SS`, when it has any.
    pub captured_at: Option<String>,
    pub mtime_unix: i64,
}

/// `GET /api/scans/{id}/library` -- every photo and video of the scan that is in the library now
/// (grouped or not), for the library overview. Companions and files in quarantine are left out.
pub async fn list_library(State(state): State<AppState>, AxPath(scan_id): AxPath<String>) -> Result<Json<Vec<LibraryFileDto>>, StatusCode> {
    let store = state.store.lock().unwrap();
    super::require_scan(&store, &scan_id)?;
    let rows = store.load_library(&scan_id).map_err(|e| {
        eprintln!("server error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(
        rows.into_iter()
            .map(|r| LibraryFileDto {
                id: r.id,
                relative_path: r.relative_path,
                file_name: r.file_name,
                media_class: r.media_class,
                size_bytes: r.size_bytes,
                width: r.width,
                height: r.height,
                captured_at: r.captured_at,
                mtime_unix: r.mtime_unix,
            })
            .collect(),
    ))
}
