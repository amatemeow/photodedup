//! Keeper scoring: within a cluster, which file is the best copy to keep?
//! Port of `~/photo-dedup/scripts/pass6_keeper.py:18-33`'s scoring tuple,
//! as a named, inspectable struct per the architecture plan rather than an
//! anonymous tuple -- each field is meant to be surfaced in the UI's
//! evidence panel later, not just compared.
//!
//! One deliberate simplification versus the MVP: the MVP's `FAMILY_RANK`
//! and `container_pref` were keyed on trip-specific filename families
//! (`iphone_heic`, `android_jpg`, `messenger_img_underscore`, ...). Reading
//! that logic closely, `container_pref` ended up redundant with
//! `is_native` (it was set true for exactly the native families) -- so
//! it contributed no independent signal. It's replaced here with a
//! genuinely independent one: a general image-format quality ranking
//! (lossless/HEVC-class formats outrank JPEG), which generalizes past one
//! trip's filename conventions.

use crate::model::FileId;
use crate::provenance::Origin;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeeperScore {
    // Field order IS comparison priority (derived `Ord` is lexicographic):
    // native > pixel count > byte size > has GPS > container quality.
    is_camera_original: bool,
    pixels: u64,
    size_bytes: u64,
    has_gps: bool,
    container_rank: u8,
}

pub fn container_rank(ext: &str) -> u8 {
    match ext.to_lowercase().as_str() {
        "heic" | "heif" | "dng" | "cr2" | "cr3" | "nef" | "arw" | "raf" | "orf" | "rw2" | "pef" | "srw" => 3,
        "png" | "tif" | "tiff" => 2,
        "mov" | "mp4" | "m4v" => 1, // video: no strong format preference, just above "unknown"
        "jpg" | "jpeg" | "webp" => 1,
        _ => 0,
    }
}

pub fn score(
    origin: Origin,
    pixels: Option<(u32, u32)>,
    size_bytes: u64,
    has_gps: bool,
    ext: &str,
) -> KeeperScore {
    KeeperScore {
        is_camera_original: origin == Origin::CameraOriginal,
        pixels: pixels.map(|(w, h)| w as u64 * h as u64).unwrap_or(0),
        size_bytes,
        has_gps,
        container_rank: container_rank(ext),
    }
}

/// Picks the best-scoring candidate. Ties broken by filename (ascending)
/// for deterministic, human-predictable output rather than input order.
pub fn pick_keeper(candidates: &[(FileId, String, KeeperScore)]) -> FileId {
    candidates
        .iter()
        .max_by(|a, b| a.2.cmp(&b.2).then_with(|| b.1.cmp(&a.1)))
        .map(|(id, _, _)| *id)
        .expect("pick_keeper called with an empty candidate list")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_always_outranks_derived_regardless_of_other_fields() {
        // A tiny native beats a huge, GPS-tagged derived file.
        let native = score(Origin::CameraOriginal, Some((100, 100)), 1_000, false, "jpg");
        let derived = score(Origin::Derived, Some((8000, 6000)), 50_000_000, true, "heic");
        assert!(native > derived);
    }

    #[test]
    fn among_equally_native_files_higher_resolution_wins() {
        let small = score(Origin::CameraOriginal, Some((1000, 1000)), 5_000_000, false, "jpg");
        let big = score(Origin::CameraOriginal, Some((4000, 3000)), 5_000_000, false, "jpg");
        assert!(big > small);
    }

    #[test]
    fn pick_keeper_breaks_ties_by_filename() {
        let s = score(Origin::CameraOriginal, Some((100, 100)), 1000, false, "jpg");
        let candidates = vec![
            (2u32, "b.jpg".to_string(), s),
            (1u32, "a.jpg".to_string(), s),
        ];
        assert_eq!(pick_keeper(&candidates), 1); // "a.jpg" < "b.jpg"
    }
}
