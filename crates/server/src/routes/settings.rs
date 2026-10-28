//! Permanent preferences. Today that is one setting: how hard a scan may
//! push the machine. Per-scan choices (include videos) and view choices
//! (sort order) deliberately live where they are used, not here.
//!
//! Stored in the database, not the browser, so the choice follows the data
//! and applies however the app is opened.

use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use photodedup_core::store::Store;
use photodedup_core::throttle::ScanIntensity;
use serde::{Deserialize, Serialize};

const KEY: &str = "scan_intensity";

/// The standing intensity: what was saved if it is still a valid tier,
/// otherwise the default.
pub fn stored_intensity(store: &Store) -> ScanIntensity {
    store
        .get_setting(KEY)
        .ok()
        .flatten()
        .and_then(|n| ScanIntensity::from_name(&n))
        .unwrap_or_default()
}

fn name_of(i: ScanIntensity) -> &'static str {
    ScanIntensity::NAMED.iter().find(|(_, t)| std::mem::discriminant(t) == std::mem::discriminant(&i)).map_or("default", |(n, _)| n)
}

#[derive(Serialize)]
pub struct Tier {
    pub id: &'static str,
    /// Worker threads this tier runs on this machine.
    pub workers: usize,
}

#[derive(Serialize)]
pub struct SettingsDto {
    pub scan_intensity: &'static str,
    pub tiers: Vec<Tier>,
    pub logical_cores: usize,
}

fn dto(store: &Store) -> SettingsDto {
    SettingsDto {
        scan_intensity: name_of(stored_intensity(store)),
        tiers: ScanIntensity::NAMED.iter().map(|(id, t)| Tier { id, workers: t.worker_count() }).collect(),
        logical_cores: ScanIntensity::logical_cores(),
    }
}

pub async fn get(State(state): State<AppState>) -> Json<SettingsDto> {
    Json(dto(&state.store.lock().unwrap()))
}

#[derive(Deserialize)]
pub struct SettingsBody {
    pub scan_intensity: String,
}

pub async fn put(State(state): State<AppState>, Json(body): Json<SettingsBody>) -> Result<Json<SettingsDto>, (StatusCode, String)> {
    let Some(tier) = ScanIntensity::from_name(&body.scan_intensity) else {
        return Err((StatusCode::BAD_REQUEST, format!("unknown scan intensity '{}': expected efficient, default or performance", body.scan_intensity)));
    };
    let store = state.store.lock().unwrap();
    store.set_setting(KEY, name_of(tier)).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(dto(&store)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standing_preference_is_used_and_a_corrupt_value_falls_back_to_default() {
        let store = Store::open_in_memory().unwrap();
        assert!(matches!(stored_intensity(&store), ScanIntensity::Default));
        store.set_setting(KEY, "performance").unwrap();
        assert!(matches!(stored_intensity(&store), ScanIntensity::Performance));
        store.set_setting(KEY, "efficient").unwrap();
        assert!(matches!(stored_intensity(&store), ScanIntensity::Efficient));
        store.set_setting(KEY, "nonsense").unwrap();
        assert!(matches!(stored_intensity(&store), ScanIntensity::Default), "a bad stored value is ignored, never trusted");
    }

    #[test]
    fn every_named_tier_round_trips_through_its_name() {
        for (name, tier) in ScanIntensity::NAMED {
            assert_eq!(name_of(tier), name);
            assert!(ScanIntensity::from_name(name).is_some());
        }
        assert!(ScanIntensity::from_name("Default").is_none(), "names are exact");
    }
}
