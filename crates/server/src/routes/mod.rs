pub mod apply;
pub mod decisions;
pub mod files;
pub mod jobs;
pub mod scans;
pub mod settings;
pub mod shell;

use axum::http::StatusCode;
use photodedup_core::store::Store;

/// 404 unless `scan_id` names the live scan. Scans are replaced wholesale
/// when a new one completes, so a client holding an old id (a stale tab)
/// must get a plain "not found" -- not an empty list, and not a raw
/// foreign-key error from a write.
pub(crate) fn require_scan(store: &Store, scan_id: &str) -> Result<(), StatusCode> {
    match store.get_scan(scan_id) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            eprintln!("server error: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
