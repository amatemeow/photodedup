//! The matching thresholds a person may tune, with the range each is allowed
//! to take. Kept in one place so the engine, the settings API and the UI
//! agree on what is valid, and so a stored or requested value can never put
//! the engine outside a range that was thought through.
//!
//! What is deliberately NOT here: the rule that two camera originals are
//! never auto-collapsed. That is structural (see `classify.rs`), not a
//! number, so no setting can weaken it.

use serde::Serialize;

/// An allowed range and its default, for one setting.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Range {
    pub min: i64,
    pub max: i64,
    pub default: i64,
}

impl Range {
    pub fn contains(&self, v: i64) -> bool {
        (self.min..=self.max).contains(&v)
    }
}

/// How far apart (of 256 fingerprint bits) two photos may look and still be
/// considered as possible copies of each other. True copies measured ~20 and
/// unrelated photos 100+, so this is a generous net; the upper limit stops
/// well short of the range where unrelated photos start to qualify.
pub const MATCH_DISTANCE: Range = Range { min: 25, max: 60, default: 45 };

/// How alike (same scale) frames of a quick sequence must look to stay in
/// the same group. Chosen by eye on real runs; see `resolve.rs`.
pub const SEQUENCE_SIMILARITY: Range = Range { min: 40, max: 100, default: 80 };

/// Seconds between shots that still count as one quick sequence.
pub const SEQUENCE_GAP_SECONDS: Range = Range { min: 1, max: 30, default: 3 };

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    pub match_distance: u32,
    pub sequence_similarity: u32,
    pub sequence_gap_seconds: i64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            match_distance: MATCH_DISTANCE.default as u32,
            sequence_similarity: SEQUENCE_SIMILARITY.default as u32,
            sequence_gap_seconds: SEQUENCE_GAP_SECONDS.default,
        }
    }
}

impl Thresholds {
    /// Err names the first value outside its range.
    pub fn validate(&self) -> Result<(), String> {
        let checks = [
            ("match distance", self.match_distance as i64, MATCH_DISTANCE),
            ("sequence similarity", self.sequence_similarity as i64, SEQUENCE_SIMILARITY),
            ("sequence time gap", self.sequence_gap_seconds, SEQUENCE_GAP_SECONDS),
        ];
        for (name, v, r) in checks {
            if !r.contains(v) {
                return Err(format!("{name} must be between {} and {} (got {v})", r.min, r.max));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_values_the_engine_was_calibrated_with() {
        let d = Thresholds::default();
        assert_eq!((d.match_distance, d.sequence_similarity, d.sequence_gap_seconds), (45, 80, 3));
        assert!(d.validate().is_ok());
    }

    #[test]
    fn every_default_sits_inside_its_own_range() {
        for r in [MATCH_DISTANCE, SEQUENCE_SIMILARITY, SEQUENCE_GAP_SECONDS] {
            assert!(r.contains(r.default) && r.min < r.max);
        }
    }

    #[test]
    fn values_outside_a_range_are_rejected_with_a_message_that_says_which() {
        let ok = Thresholds::default();
        assert!(Thresholds { match_distance: 24, ..ok }.validate().unwrap_err().contains("match distance"));
        assert!(Thresholds { match_distance: 61, ..ok }.validate().is_err());
        assert!(Thresholds { sequence_similarity: 101, ..ok }.validate().unwrap_err().contains("sequence similarity"));
        assert!(Thresholds { sequence_gap_seconds: 0, ..ok }.validate().unwrap_err().contains("time gap"));
        assert!(Thresholds { sequence_gap_seconds: 31, ..ok }.validate().is_err());
        assert!(Thresholds { match_distance: 25, sequence_similarity: 40, sequence_gap_seconds: 1 }.validate().is_ok(), "limits are inclusive");
    }

    /// The top of the match range must stay well below where unrelated photos
    /// begin (100+ of 256 bits in the reference data).
    #[test]
    fn the_match_range_never_reaches_the_distance_of_unrelated_photos() {
        assert!(MATCH_DISTANCE.max < 100 / 1);
        assert!(MATCH_DISTANCE.max * 2 <= 120);
    }
}
