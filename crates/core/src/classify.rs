//! Pairwise classification: turns a candidate pair's distance + provenance
//! into a tier. This is the safety core of the whole engine -- port of
//! `~/photo-dedup/scripts/pass4_classify.py`'s structural rule, generalized
//! from the MVP's `is_native` boolean to the `Origin` enum (see
//! `provenance.rs`), plus one deliberate improvement (`T1b`, see below).
//!
//! The empirical calibration this is built on (from `PROJECT_CONTEXT.md`,
//! re-validated in M0's hash parity spike, `/SPIKES.md` §3): a true
//! re-encoded duplicate sits at pHash distance ~20/256, but a genuine
//! burst-mode sibling (same camera, ~1s apart) can *also* land at
//! ~18-20/256 -- indistinguishable from a duplicate by distance alone.
//! That overlap is why this module never collapses two `CameraOriginal`
//! files regardless of how close their hash distance is: the structural
//! rule, not the threshold, is what protects burst photos.

use crate::model::FileId;
use crate::provenance::Origin;
use chrono::NaiveDateTime;

pub const CANDIDATE_MAX_DIST: u32 = 45; // generous net; true dupes ~20, noise ~100+
pub const BURST_GAP_SECONDS: i64 = 3;
pub const SAME_DERIVED_MAX_SECONDS: i64 = 30; // mirrors MVP's CROSS_DEVICE_MAX_SECONDS name/value

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Native-vs-derived: safe to auto-select a keeper (native wins).
    T1,
    /// Both `Derived`, no surviving candidate original in the pair.
    /// **Deliberate improvement over the MVP**, which sent this case to
    /// review-only (`pass4_classify.py:128-135`): two WhatsApp re-saves of
    /// a photo you no longer have the original of is a legitimate
    /// auto-dedup (keep the larger) -- see `/ROADMAP.md`.
    T1b,
    /// Both `CameraOriginal`, same camera, within the burst time window.
    /// Review-only by construction -- see module docs.
    T2Burst,
    /// Both `CameraOriginal`, same camera, not within the burst window.
    T2NativeReview,
    /// Both `CameraOriginal`, different camera models.
    T3CrossDevice,
    /// Both `Derived`, too far apart in time to treat as the same shot.
    T2DerivedReview,
    /// Either side is `Origin::Unknown` -- never auto-classified.
    T2UnknownReview,
}

impl Tier {
    pub fn is_auto_selectable(&self) -> bool {
        matches!(self, Tier::T1 | Tier::T1b)
    }
}

#[derive(Debug, Clone)]
pub struct ClassifyInput {
    pub id: FileId,
    pub origin: Origin,
    pub camera_model: Option<String>,
    pub capture_time: Option<NaiveDateTime>,
}

#[derive(Debug, Clone)]
pub struct PairClassification {
    pub a: FileId,
    pub b: FileId,
    /// Carried through from the inputs so `resolve.rs` can tell, without a
    /// second lookup, which side of a `T1` pair is the one needing
    /// argmin-resolution (the `Derived` side).
    pub a_origin: Origin,
    pub b_origin: Origin,
    pub phash_dist: u32,
    pub dt_seconds: Option<i64>,
    pub tier: Tier,
    pub reason: &'static str,
}

fn dt_seconds(a: &ClassifyInput, b: &ClassifyInput) -> Option<i64> {
    match (a.capture_time, b.capture_time) {
        (Some(ta), Some(tb)) => Some((ta - tb).num_seconds().abs()),
        _ => None,
    }
}

pub fn classify_pair(a: &ClassifyInput, b: &ClassifyInput, phash_dist: u32) -> PairClassification {
    let dt = dt_seconds(a, b);
    let same_camera = matches!((&a.camera_model, &b.camera_model), (Some(x), Some(y)) if x == y);

    let (tier, reason) = if a.origin == Origin::Unknown || b.origin == Origin::Unknown {
        (Tier::T2UnknownReview, "unknown_provenance_review")
    } else if a.origin == Origin::CameraOriginal && b.origin == Origin::CameraOriginal {
        // Never auto-collapse two camera-originals, regardless of distance
        // -- see module docs for why this must be structural, not a
        // tighter threshold.
        if same_camera && dt.is_some_and(|d| d <= BURST_GAP_SECONDS) {
            (Tier::T2Burst, "burst_same_camera")
        } else if same_camera {
            (Tier::T2NativeReview, "native_native_review")
        } else {
            (Tier::T3CrossDevice, "native_native_cross_device")
        }
    } else if a.origin == Origin::Derived && b.origin == Origin::Derived {
        if dt.is_none_or(|d| d <= SAME_DERIVED_MAX_SECONDS) {
            (Tier::T1b, "derived_vs_derived_no_surviving_original")
        } else {
            (Tier::T2DerivedReview, "derived_vs_derived_too_far_apart")
        }
    } else {
        // exactly one CameraOriginal, one Derived
        (Tier::T1, "native_vs_derived")
    };

    PairClassification {
        a: a.id,
        b: b.id,
        a_origin: a.origin,
        b_origin: b.origin,
        phash_dist,
        dt_seconds: dt,
        tier,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: FileId, origin: Origin, camera: &str, t: Option<NaiveDateTime>) -> ClassifyInput {
        ClassifyInput {
            id,
            origin,
            camera_model: Some(camera.to_string()),
            capture_time: t,
        }
    }

    fn ts(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn two_native_files_never_auto_collapse_even_at_zero_distance() {
        // The core safety property: distance 0 between two CameraOriginal
        // files must still land in a review-only tier.
        let a = input(0, Origin::CameraOriginal, "OnePlus 15", Some(ts("2026-10-19 10:14:41")));
        let b = input(1, Origin::CameraOriginal, "OnePlus 15", Some(ts("2026-10-19 10:14:42")));
        let c = classify_pair(&a, &b, 0);
        assert!(!c.tier.is_auto_selectable());
        assert_eq!(c.tier, Tier::T2Burst);
    }

    #[test]
    fn burst_sibling_one_second_apart_same_camera_is_burst_tier() {
        let a = input(0, Origin::CameraOriginal, "OnePlus 15", Some(ts("2026-10-19 10:14:41")));
        let b = input(1, Origin::CameraOriginal, "OnePlus 15", Some(ts("2026-10-19 10:14:42")));
        assert_eq!(classify_pair(&a, &b, 30).tier, Tier::T2Burst);
    }

    #[test]
    fn native_vs_derived_is_t1_auto_selectable() {
        let a = input(0, Origin::CameraOriginal, "OnePlus 15", None);
        let b = input(1, Origin::Derived, "OnePlus 15", None);
        let c = classify_pair(&a, &b, 5);
        assert_eq!(c.tier, Tier::T1);
        assert!(c.tier.is_auto_selectable());
    }

    #[test]
    fn two_derived_files_close_in_time_are_t1b() {
        let a = input(0, Origin::Derived, "unknown", Some(ts("2026-10-19 10:14:41")));
        let b = input(1, Origin::Derived, "unknown", Some(ts("2026-10-19 10:14:45")));
        let c = classify_pair(&a, &b, 5);
        assert_eq!(c.tier, Tier::T1b);
        assert!(c.tier.is_auto_selectable());
    }

    #[test]
    fn two_derived_files_with_no_timestamp_default_to_auto_selectable() {
        // No timestamp evidence either way -- MVP's analogous case
        // ("both_messenger_no_time_review") was conservative; our
        // T1b improvement still requires lack of evidence AGAINST it,
        // which `dt.is_none_or(...)` deliberately treats as permissive
        // here since there's no original left to protect either way.
        let a = input(0, Origin::Derived, "unknown", None);
        let b = input(1, Origin::Derived, "unknown", None);
        assert_eq!(classify_pair(&a, &b, 5).tier, Tier::T1b);
    }

    #[test]
    fn unknown_provenance_never_auto_classifies() {
        let a = input(0, Origin::Unknown, "x", None);
        let b = input(1, Origin::CameraOriginal, "x", None);
        let c = classify_pair(&a, &b, 5);
        assert!(!c.tier.is_auto_selectable());
        assert_eq!(c.tier, Tier::T2UnknownReview);
    }

    #[test]
    fn cross_device_native_pair_is_its_own_tier() {
        let a = input(0, Origin::CameraOriginal, "OnePlus 15", None);
        let b = input(1, Origin::CameraOriginal, "iPhone Air", None);
        let c = classify_pair(&a, &b, 40);
        assert_eq!(c.tier, Tier::T3CrossDevice);
        assert!(!c.tier.is_auto_selectable());
    }
}
