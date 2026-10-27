//! Generalises the MVP's `is_messenger_derived` boolean
//! (`~/photo-dedup/scripts/pass4_classify.py:53-54`) into scored,
//! explainable evidence. Two hard-won lessons from the MVP session are
//! encoded directly in this module's shape, not left as comments to be
//! forgotten:
//!
//! 1. **Filename pattern is evidence, never decisive.** 181 files in the
//!    reference dataset were named `IMG_####.JPG` — indistinguishable by
//!    name from a genuine iPhone original — but had zero EXIF; they were
//!    messenger saves, and iOS assigned the next sequential number
//!    regardless of true origin. `Evidence::FilenamePattern` exists and is
//!    low-weight; nothing may classify on it alone.
//! 2. **Two camera-originals are never auto-collapsed, regardless of hash
//!    distance.** This is enforced in the classifier
//!    (`crates/core/src/classify.rs`, not here) by construction: it simply
//!    has no code path that takes two `Origin::CameraOriginal` provenances
//!    to a T1 (auto-select-keeper) outcome. See `/SPIKES.md`-adjacent
//!    calibration notes in the original `PROJECT_CONTEXT.md` for *why*:
//!    genuine burst siblings and true duplicates can share the same pHash
//!    distance band.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    CameraOriginal,
    Derived,
    Unknown,
}

/// One piece of evidence toward a provenance judgment, carried through to
/// the UI verbatim — trust in a destructive tool comes from the user being
/// able to see *why* a file was judged derived, not just a verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Evidence {
    /// Make + Model + DateTimeOriginal (ideally SubSecTimeOriginal) intact.
    IntactCameraExif,
    GpsPresent,
    LensMetadataPresent,
    MakerNotePresent,

    NoExifAtAll,
    /// Only the filesystem mtime/ctime is available — not derived from the
    /// capture itself.
    TimestampFromFilesystemOnly,
    /// A recognizable re-encoder JPEG quantization-table or dimension-cap
    /// signature (e.g. WhatsApp caps at 1600px, Telegram at 1280px).
    KnownReencoderSignature { name: String },
    /// `Software`/encoder EXIF tag naming a known re-encoder or editor.
    EncoderTag { name: String },
    /// `file (1).jpg`, `(conflicted copy)`, `-DESKTOP-ABC` — a cloud-sync
    /// conflict copy, a rich duplicate source in real libraries.
    SyncConflictFilename,
    /// PNG at an exact common screen resolution, no EXIF.
    ScreenshotIndicators,
    /// Dimensions are an exact common downscale of a candidate original
    /// (e.g. exactly 1600px wide when the candidate is 4032px).
    DownscaleOfCandidate,
    /// A Google Takeout `.json` sidecar supplies a timestamp this file
    /// itself lacks.
    TakeoutSidecarTimestamp,
    /// The candidate original has an embedded motion-photo/Live-Photo
    /// video that this file is missing — a sign of a lossy re-save.
    MissingEmbeddedMotionVideo,

    /// Low-weight signal only — see module docs. Never decisive alone.
    FilenamePattern { note: String },
}

impl Evidence {
    /// Rough directional weight: positive pushes toward `CameraOriginal`,
    /// negative toward `Derived`. `FilenamePattern` is deliberately small
    /// relative to everything else — it should never flip a verdict alone.
    pub fn weight(&self) -> f32 {
        match self {
            Evidence::IntactCameraExif => 3.0,
            Evidence::GpsPresent => 1.0,
            Evidence::LensMetadataPresent => 1.0,
            Evidence::MakerNotePresent => 1.5,

            Evidence::NoExifAtAll => -3.0,
            Evidence::TimestampFromFilesystemOnly => -2.0,
            Evidence::KnownReencoderSignature { .. } => -3.0,
            Evidence::EncoderTag { .. } => -2.5,
            Evidence::SyncConflictFilename => -1.5,
            Evidence::ScreenshotIndicators => -3.0,
            Evidence::DownscaleOfCandidate => -2.0,
            Evidence::TakeoutSidecarTimestamp => -0.5,
            Evidence::MissingEmbeddedMotionVideo => -1.0,

            Evidence::FilenamePattern { .. } => -0.3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub origin: Origin,
    pub confidence: f32,
    pub evidence: Vec<Evidence>,
}

impl Provenance {
    /// Fold evidence into a verdict. Deliberately simple (sum of weights,
    /// thresholded) rather than a learned model — explainability matters
    /// more than precision for a destructive tool, and the thresholds are
    /// easy to recalibrate against the reference dataset once `classify.rs`
    /// lands.
    pub fn from_evidence(evidence: Vec<Evidence>) -> Self {
        let score: f32 = evidence.iter().map(Evidence::weight).sum();
        let origin = if score >= 2.0 {
            Origin::CameraOriginal
        } else if score <= -2.0 {
            Origin::Derived
        } else {
            Origin::Unknown
        };
        let confidence = (score.abs() / 6.0).min(1.0);
        Provenance {
            origin,
            confidence,
            evidence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intact_exif_alone_is_camera_original() {
        let p = Provenance::from_evidence(vec![Evidence::IntactCameraExif, Evidence::GpsPresent]);
        assert_eq!(p.origin, Origin::CameraOriginal);
    }

    #[test]
    fn filename_pattern_alone_never_decides() {
        // The 181-file lesson: a native-looking filename with nothing else
        // must not classify as CameraOriginal.
        let p = Provenance::from_evidence(vec![Evidence::FilenamePattern {
            note: "IMG_####.JPG".into(),
        }]);
        assert_ne!(p.origin, Origin::CameraOriginal);
    }

    #[test]
    fn no_exif_is_derived() {
        let p = Provenance::from_evidence(vec![
            Evidence::NoExifAtAll,
            Evidence::TimestampFromFilesystemOnly,
        ]);
        assert_eq!(p.origin, Origin::Derived);
    }
}
