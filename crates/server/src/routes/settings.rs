//! Permanent preferences: how hard a scan may push the machine, and the
//! three matching thresholds (an "advanced" group, each with a fixed safe
//! range). Per-scan choices (include videos) and view choices
//! (sort order) deliberately live where they are used, not here.
//!
//! Stored in the database, not the browser, so the choice follows the data
//! and applies however the app is opened.

use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use photodedup_core::store::Store;
use photodedup_core::thresholds::{Range, Thresholds, MATCH_DISTANCE, SEQUENCE_GAP_SECONDS, SEQUENCE_SIMILARITY};
use photodedup_core::throttle::ScanIntensity;
use serde::{Deserialize, Serialize};

const KEY: &str = "scan_intensity";
const K_SIMILARITY: &str = "sequence_similarity";
const K_GAP: &str = "sequence_gap_seconds";
const K_MATCH: &str = "match_distance";
const K_CACHE_MB: &str = "cache_budget_mb";

/// Disk space the thumbnail and preview cache may use, in MB. The floor leaves
/// room for a few hundred previews; the ceiling is a deliberate "this is
/// enough for anyone" so a typo cannot hand the cache a whole disk.
pub const CACHE_MB: Range = Range { min: 256, max: 10_240, default: 1_024 };

/// The saved cache budget in bytes: the default if missing, unparseable or out of range.
pub fn stored_cache_bytes(store: &Store) -> u64 {
    let mb = store.get_setting(K_CACHE_MB).ok().flatten().and_then(|v| v.parse::<i64>().ok()).filter(|v| CACHE_MB.contains(*v)).unwrap_or(CACHE_MB.default);
    mb as u64 * 1024 * 1024
}

/// The saved thresholds. Each one that is missing, unparseable or outside
/// its range falls back to its default: a stored value is never trusted
/// past what the engine was calibrated to accept.
pub fn stored_thresholds(store: &Store) -> Thresholds {
    let read = |key: &str, range: Range| -> i64 {
        store
            .get_setting(key)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|v| range.contains(*v))
            .unwrap_or(range.default)
    };
    Thresholds {
        match_distance: read(K_MATCH, MATCH_DISTANCE) as u32,
        sequence_similarity: read(K_SIMILARITY, SEQUENCE_SIMILARITY) as u32,
        sequence_gap_seconds: read(K_GAP, SEQUENCE_GAP_SECONDS),
    }
}

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

/// One slider: its current value and the range it may move in.
#[derive(Serialize)]
pub struct Knob {
    pub value: i64,
    pub default: i64,
    pub min: i64,
    pub max: i64,
}

#[derive(Serialize)]
pub struct Advanced {
    pub sequence_similarity: Knob,
    pub sequence_gap_seconds: Knob,
    pub match_distance: Knob,
}

/// The preview cache: its budget (a slider) and how much it holds now.
#[derive(Serialize)]
pub struct CacheDto {
    pub budget_mb: Knob,
    pub used_bytes: u64,
}

#[derive(Serialize)]
pub struct SettingsDto {
    pub scan_intensity: &'static str,
    pub tiers: Vec<Tier>,
    pub logical_cores: usize,
    pub advanced: Advanced,
    pub cache: CacheDto,
}

fn knob(value: i64, r: Range) -> Knob {
    Knob { value, default: r.default, min: r.min, max: r.max }
}

fn dto(store: &Store, cache: &crate::cache::ThumbCache) -> SettingsDto {
    let t = stored_thresholds(store);
    SettingsDto {
        scan_intensity: name_of(stored_intensity(store)),
        tiers: ScanIntensity::NAMED.iter().map(|(id, t)| Tier { id, workers: t.worker_count() }).collect(),
        logical_cores: ScanIntensity::logical_cores(),
        advanced: Advanced {
            sequence_similarity: knob(t.sequence_similarity as i64, SEQUENCE_SIMILARITY),
            sequence_gap_seconds: knob(t.sequence_gap_seconds, SEQUENCE_GAP_SECONDS),
            match_distance: knob(t.match_distance as i64, MATCH_DISTANCE),
        },
        cache: CacheDto { budget_mb: knob((stored_cache_bytes(store) / (1024 * 1024)) as i64, CACHE_MB), used_bytes: cache.used() },
    }
}

pub async fn get(State(state): State<AppState>) -> Json<SettingsDto> {
    Json(dto(&state.store.lock().unwrap(), &state.cache))
}

/// Every field is optional: send only what changed. The whole request is
/// checked before anything is saved, so a bad value changes nothing.
#[derive(Deserialize)]
pub struct SettingsBody {
    pub scan_intensity: Option<String>,
    pub sequence_similarity: Option<i64>,
    pub sequence_gap_seconds: Option<i64>,
    pub match_distance: Option<i64>,
    pub cache_budget_mb: Option<i64>,
}

pub async fn put(State(state): State<AppState>, Json(body): Json<SettingsBody>) -> Result<Json<SettingsDto>, (StatusCode, String)> {
    let bad = |m: String| (StatusCode::BAD_REQUEST, m);
    let store = state.store.lock().unwrap();

    let tier = match body.scan_intensity.as_deref() {
        Some(name) => Some(ScanIntensity::from_name(name).ok_or_else(|| bad(format!("unknown scan intensity '{name}': expected efficient, default or performance")))?),
        None => None,
    };

    // Build the thresholds as they would be, then validate them together.
    let mut t = stored_thresholds(&store);
    if let Some(v) = body.match_distance {
        t.match_distance = u32::try_from(v).map_err(|_| bad("match distance is out of range".into()))?;
    }
    if let Some(v) = body.sequence_similarity {
        t.sequence_similarity = u32::try_from(v).map_err(|_| bad("sequence similarity is out of range".into()))?;
    }
    if let Some(v) = body.sequence_gap_seconds {
        t.sequence_gap_seconds = v;
    }
    t.validate().map_err(bad)?;
    if let Some(mb) = body.cache_budget_mb {
        if !CACHE_MB.contains(mb) {
            return Err(bad(format!("cache size must be between {} and {} MB (got {mb})", CACHE_MB.min, CACHE_MB.max)));
        }
    }

    let io = |e: rusqlite::Error| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    if let Some(tier) = tier {
        store.set_setting(KEY, name_of(tier)).map_err(io)?;
    }
    if body.match_distance.is_some() {
        store.set_setting(K_MATCH, &t.match_distance.to_string()).map_err(io)?;
    }
    if body.sequence_similarity.is_some() {
        store.set_setting(K_SIMILARITY, &t.sequence_similarity.to_string()).map_err(io)?;
    }
    if body.sequence_gap_seconds.is_some() {
        store.set_setting(K_GAP, &t.sequence_gap_seconds.to_string()).map_err(io)?;
    }
    if let Some(mb) = body.cache_budget_mb {
        store.set_setting(K_CACHE_MB, &mb.to_string()).map_err(io)?;
        state.cache.set_budget(mb as u64 * 1024 * 1024);
    }
    Ok(Json(dto(&store, &state.cache)))
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
    fn thresholds_default_clamp_to_defaults_when_missing_or_corrupt_or_out_of_range() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(stored_thresholds(&store), Thresholds::default());

        store.set_setting(K_MATCH, "30").unwrap();
        store.set_setting(K_SIMILARITY, "not a number").unwrap();
        store.set_setting(K_GAP, "9999").unwrap();
        let t = stored_thresholds(&store);
        assert_eq!(t.match_distance, 30, "a valid value is used");
        assert_eq!(t.sequence_similarity, 80, "an unparseable one falls back");
        assert_eq!(t.sequence_gap_seconds, 3, "an out-of-range one falls back");
        assert!(t.validate().is_ok());
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
