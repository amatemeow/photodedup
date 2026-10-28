//! What the surrounding shell can do, and whether the tools the engine needs
//! are present.
//!
//! The UI talks only HTTP, so a native capability (a real folder dialog) is
//! reached the same way as everything else: the shell that embeds this
//! server hands it a callback, and the UI asks. In a plain browser there is
//! no callback, `/api/shell` says so, and the UI simply doesn't offer the
//! button. Nothing in the UI knows which shell it is running in.

use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use photodedup_core::tools::{self, ToolStatus};
use serde::Serialize;

#[derive(Serialize)]
pub struct ShellDto {
    /// A native "choose a folder" dialog is available (`POST /api/pick-folder`).
    pub native_folder_picker: bool,
    /// ffmpeg and ffprobe: needed for HEIC photos and for videos.
    pub tools: Vec<ToolStatus>,
}

pub async fn status(State(state): State<AppState>) -> Json<ShellDto> {
    // Spawns two short subprocesses; keep it off the async workers.
    let tools = tokio::task::spawn_blocking(|| vec![tools::check("ffmpeg"), tools::check("ffprobe")]).await.unwrap_or_default();
    Json(ShellDto { native_folder_picker: state.folder_picker.is_some(), tools })
}

#[derive(Serialize)]
pub struct PickedDto {
    /// The chosen folder, or null if the dialog was cancelled.
    pub path: Option<String>,
}

/// `POST /api/pick-folder` -- shows the shell's native folder dialog and
/// returns the choice. Only the person at the keyboard can answer it, and
/// what comes back is just a path they chose, which the scan then validates
/// like any other.
pub async fn pick_folder(State(state): State<AppState>) -> Result<Json<PickedDto>, StatusCode> {
    let Some(picker) = state.folder_picker.clone() else {
        return Err(StatusCode::NOT_IMPLEMENTED);
    };
    // A dialog blocks until the user answers: never on an async worker.
    let chosen = tokio::task::spawn_blocking(move || picker()).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(PickedDto { path: chosen.map(|p| p.to_string_lossy().into_owned()) }))
}
