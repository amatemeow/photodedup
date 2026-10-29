//! Generalises the MVP's `is_messenger_derived` boolean
//! (the prototype's `pass4_classify.py`, not part of this repo) into scored,
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

use crate::metadata::FileMetadata;
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

/// Known re-encoder output caps: the longer edge of a WhatsApp- or
/// Telegram-re-encoded image is capped at a known pixel count. This is a
/// weak, single-file-only signal (a genuinely small native photo would
/// trip it too) — paired deliberately with `NoExifAtAll`/no-camera-identity
/// elsewhere so it never fires alone with enough weight to decide anything.
const WHATSAPP_MAX_EDGE: u32 = 1600;
const TELEGRAM_MAX_EDGE: u32 = 1280;

fn longer_edge(meta: &FileMetadata) -> Option<u32> {
    match (meta.width, meta.height) {
        (Some(w), Some(h)) => Some(w.max(h)),
        _ => None,
    }
}

/// Build the single-file evidence set for one file's metadata. Evidence
/// that needs a *candidate original* to compare against (exact-downscale
/// detection, missing embedded motion video, Takeout sidecar lookups) is
/// deliberately out of scope here — that's pairwise, and belongs in
/// `classify.rs`'s pair-level refinement, not per-file evaluation.
pub fn evidence_from_metadata(meta: &FileMetadata, filename: &str) -> Vec<Evidence> {
    let mut ev = Vec::new();

    let has_camera_identity =
        meta.make.is_some() && meta.model.is_some() && meta.datetime_original.is_some();

    if has_camera_identity {
        ev.push(Evidence::IntactCameraExif);
    } else if meta.make.is_none() && meta.model.is_none() && meta.datetime_original.is_none() {
        // Note: some messenger re-saves still carry *some* EXIF (resolution,
        // ExifVersion, a thumbnail) -- the absence of Make/Model/
        // DateTimeOriginal specifically is the signal, not "any EXIF at
        // all". See PROJECT_CONTEXT.md's 181-file `IMG_####.JPG` finding
        // and metadata.rs's `messenger_jpg_has_no_camera_identity` test.
        ev.push(Evidence::NoExifAtAll);
        ev.push(Evidence::TimestampFromFilesystemOnly);
    }

    if meta.has_gps {
        ev.push(Evidence::GpsPresent);
    }
    if meta.has_lens_info {
        ev.push(Evidence::LensMetadataPresent);
    }
    if meta.has_maker_note {
        ev.push(Evidence::MakerNotePresent);
    }

    if let Some(sw) = &meta.software {
        let lower = sw.to_lowercase();
        for known in ["whatsapp", "capcut", "telegram", "messenger"] {
            if lower.contains(known) {
                ev.push(Evidence::EncoderTag { name: sw.clone() });
                break;
            }
        }
    }

    if !has_camera_identity {
        if let Some(edge) = longer_edge(meta) {
            if edge == WHATSAPP_MAX_EDGE {
                ev.push(Evidence::KnownReencoderSignature {
                    name: format!("WhatsApp-style cap ({WHATSAPP_MAX_EDGE}px)"),
                });
            } else if edge == TELEGRAM_MAX_EDGE {
                ev.push(Evidence::KnownReencoderSignature {
                    name: format!("Telegram-style cap ({TELEGRAM_MAX_EDGE}px)"),
                });
            }
        }
    }

    let lower_name = filename.to_lowercase();
    if lower_name.contains("conflicted copy")
        || lower_name.contains("-desktop-")
        || is_parenthesized_duplicate_suffix(&lower_name)
    {
        ev.push(Evidence::SyncConflictFilename);
    }

    // Low-weight, native-looking filename pattern with no corroborating
    // camera identity -- the 181-file lesson: never decisive alone (see
    // `Evidence::weight`), but worth recording for the UI's explanation.
    if !has_camera_identity && (lower_name.starts_with("img_") || lower_name.starts_with("img")) {
        ev.push(Evidence::FilenamePattern {
            note: "native-looking filename but no corroborating camera EXIF".into(),
        });
    }

    ev
}

/// `file (1).jpg`, `file(2).png`, etc.
fn is_parenthesized_duplicate_suffix(lower_name: &str) -> bool {
    let Some(stem) = lower_name.rsplit_once('.').map(|(s, _)| s) else {
        return false;
    };
    let trimmed = stem.trim_end();
    trimmed.ends_with(')')
        && trimmed
            .rfind('(')
            .map(|open| {
                trimmed[open + 1..trimmed.len() - 1]
                    .chars()
                    .all(|c| c.is_ascii_digit())
                    && trimmed[open + 1..trimmed.len() - 1].len() > 0
            })
            .unwrap_or(false)
}

/// Convenience: extract evidence and fold it into a verdict in one call.
pub fn evaluate(meta: &FileMetadata, filename: &str) -> Provenance {
    Provenance::from_evidence(evidence_from_metadata(meta, filename))
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

    #[test]
    fn parenthesized_duplicate_suffix_detection() {
        assert!(is_parenthesized_duplicate_suffix("photo (1).jpg"));
        assert!(is_parenthesized_duplicate_suffix("photo(2).png"));
        assert!(!is_parenthesized_duplicate_suffix("img_1234.jpg"));
        assert!(!is_parenthesized_duplicate_suffix("event (summer).jpg"));
    }

    /// Replicates PROJECT_CONTEXT.md's single most load-bearing empirical
    /// finding end-to-end, through the real extraction + evidence +
    /// classification pipeline rather than hand-constructed evidence:
    /// 181 files named `IMG_####.JPG` (so indistinguishable from a native
    /// iPhone filename by name alone) carry zero EXIF and must classify
    /// `Derived`, not `CameraOriginal`. If this regresses, the "never
    /// auto-collapse two camera-originals" safety rule in classify.rs would
    /// stop protecting exactly the files it exists to protect.
    #[test]
    #[ignore = "requires the private reference photo set (PHOTODEDUP_REFERENCE_SET)"]
    fn filename_native_but_exif_absent_classifies_derived_on_real_dataset() {
        use crate::metadata::extract_image;

        let Some(dir) = crate::reference::photos() else {
            eprintln!("skipping: {} is not set to a folder", crate::reference::ENV);
            return;
        };

        let mut zero_exif_img_pattern = 0u32;
        let mut wrongly_camera_original = 0u32;

        for entry in std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let lower = name.to_lowercase();
            if !lower.starts_with("img_") || !lower.ends_with(".jpg") {
                continue;
            }
            let Ok(meta) = extract_image(&path) else {
                continue;
            };
            if meta.make.is_none() && meta.model.is_none() && meta.datetime_original.is_none() {
                zero_exif_img_pattern += 1;
                let p = evaluate(&meta, &name);
                if p.origin == Origin::CameraOriginal {
                    wrongly_camera_original += 1;
                    eprintln!("FALSE POSITIVE: {name} classified CameraOriginal with zero EXIF");
                }
            }
        }

        eprintln!("zero-EXIF IMG_####.JPG files found: {zero_exif_img_pattern} (MVP found 181)");
        assert!(
            zero_exif_img_pattern >= 150,
            "expected roughly 181 zero-EXIF IMG_*.jpg files per PROJECT_CONTEXT.md, found {zero_exif_img_pattern}"
        );
        assert_eq!(
            wrongly_camera_original, 0,
            "{wrongly_camera_original} zero-EXIF files were misclassified as CameraOriginal"
        );
    }
}
