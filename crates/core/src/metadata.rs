//! Raw per-file metadata extraction. Two paths, each covering its format
//! completely rather than partially (see `/SPIKES.md` §2 and §4 for why):
//!
//! - **Images** (JPEG, HEIC): `nom-exif`, pure Rust, one code path for both
//!   formats and both camera vendors.
//! - **Video** (MP4, MOV): `ffprobe` subprocess — `nom-exif`'s `TrackInfo`
//!   path gets Apple's vendor atoms but has no mapping for Android's
//!   (`com.oplus.product.model` silently absent, no error), so it isn't a
//!   complete substitute on a dataset with both phones.
//!
//! This module only extracts and normalizes raw fields. Turning them into a
//! native/derived judgment is `provenance::evaluate`.

use crate::platform::{platform, SchedulingHint};
use chrono::NaiveDateTime;
use nom_exif::{Exif, ExifIter, ExifTag, MediaParser, MediaSource};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("nom-exif error: {0}")]
    NomExif(#[from] nom_exif::Error),
    #[error("ffprobe failed to run (is ffmpeg installed?): {0}")]
    FfprobeSpawn(#[source] std::io::Error),
    #[error("ffprobe exited with non-zero status for {0}")]
    FfprobeStatus(String),
    #[error("ffprobe output was not valid JSON: {0}")]
    FfprobeJson(#[from] serde_json::Error),
}

/// Raw metadata as extracted, before any native/derived judgment. Field
/// names deliberately stay close to what the MVP's `inventory_final.json`
/// recorded (see the prototype's `pass1_derive.py`, not part of this repo) so the parity
/// comparison in M1's acceptance test is a straightforward diff.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub make: Option<String>,
    pub model: Option<String>,
    /// `Software`/encoder tag — often how a re-encoder identifies itself.
    pub software: Option<String>,
    pub datetime_original: Option<NaiveDateTime>,
    pub subsec_original: Option<String>,
    pub has_gps: bool,
    pub has_maker_note: bool,
    pub has_lens_info: bool,
    pub duration_seconds: Option<f64>,
    /// True only when the exif/track source was actually readable and
    /// produced at least one tag — distinguishes "no camera metadata"
    /// (derived-leaning evidence) from "we couldn't read metadata at all"
    /// (should not silently count as either).
    pub metadata_readable: bool,
}

impl FileMetadata {
    /// `camera_model` as the MVP recorded it (e.g. `pairs_t1.json`'s
    /// `camera_a`/`camera_b`): the `Model` tag alone, falling back to
    /// `Make`. Both reference phones already put the full distinguishing
    /// name in `Model` ("OnePlus 15", "iPhone Air") — concatenating `Make`
    /// would produce "Apple iPhone Air", which doesn't match what grouping
    /// by camera should compare against.
    pub fn camera_model(&self) -> Option<String> {
        self.model.clone().or_else(|| self.make.clone())
    }
}

pub fn extract_image(path: &Path) -> Result<FileMetadata, MetadataError> {
    let ms = MediaSource::file_path(path)?;
    if !ms.has_exif() {
        return Ok(FileMetadata {
            metadata_readable: true,
            ..Default::default()
        });
    }
    let mut parser = MediaParser::new();
    let iter: ExifIter = parser.parse(ms)?;
    let gps_present = iter.clone().parse_gps_info().ok().flatten().is_some();
    let exif: Exif = iter.into();

    let get_str = |tag: ExifTag| exif.get(tag).and_then(|v| v.as_str()).map(str::to_string);
    let has_maker_note = exif.get(ExifTag::MakerNote).is_some();
    let has_lens_info = exif.get(ExifTag::LensModel).is_some() || exif.get(ExifTag::LensSpecification).is_some();

    let (datetime_original, subsec_original) = exif
        .get(ExifTag::DateTimeOriginal)
        .and_then(|v| v.as_time_components())
        .map(|(ndt, _offset)| ndt)
        .map(|ndt| (Some(ndt), get_str(ExifTag::SubSecTimeOriginal)))
        .unwrap_or((None, None));

    Ok(FileMetadata {
        width: exif.get(ExifTag::ExifImageWidth).and_then(|v| v.as_u32()),
        height: exif.get(ExifTag::ExifImageHeight).and_then(|v| v.as_u32()),
        make: get_str(ExifTag::Make),
        model: get_str(ExifTag::Model),
        software: get_str(ExifTag::Software),
        datetime_original,
        subsec_original,
        has_gps: gps_present,
        has_maker_note,
        has_lens_info,
        duration_seconds: None,
        metadata_readable: true,
    })
}

/// Shells out to `ffprobe -show_format -show_streams -print_format json`
/// and pulls the fields the provenance model needs. Deliberately permissive
/// about which exact tag key supplies Make/Model/CreateDate -- different
/// vendors (and format vs. stream tags) use different keys; see
/// `/SPIKES.md` §2 for the two concrete tag sets this was validated
/// against.
pub fn extract_video(path: &Path, ffprobe_bin: &str, hint: Option<SchedulingHint>) -> Result<FileMetadata, MetadataError> {
    let mut command = Command::new(ffprobe_bin);
    let cmd = command.args([
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ]);
    if let Some(hint) = hint {
        platform().configure_child_process(cmd, hint);
    }
    let output = cmd.arg(path).output().map_err(MetadataError::FfprobeSpawn)?;

    if !output.status.success() {
        return Err(MetadataError::FfprobeStatus(path.display().to_string()));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let format_tags = json
        .get("format")
        .and_then(|f| f.get("tags"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let tag_str = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| format_tags.get(k).and_then(|v| v.as_str()))
            .map(str::to_string)
    };

    let make = tag_str(&["com.apple.quicktime.make"]);
    let model = tag_str(&[
        "com.apple.quicktime.model",
        "com.oplus.product.model",
    ]);
    let software = tag_str(&["com.apple.quicktime.software", "com.android.version"]);
    let has_gps = tag_str(&[
        "location",
        "location-eng",
        "com.apple.quicktime.location.ISO6709",
    ])
    .is_some();

    // First tag that is both present AND parseable: taking the first
    // present one alone left iPhone clips without a timestamp whenever
    // `creationdate` had a format we couldn't read, even though
    // `creation_time` beside it was fine.
    let datetime_original = ["com.apple.quicktime.creationdate", "creation_time"]
        .iter()
        .find_map(|k| tag_str(&[k]).and_then(|s| parse_video_timestamp(&s)));

    // dimensions from the first video stream
    let (width, height) = json
        .get("streams")
        .and_then(|s| s.as_array())
        .and_then(|streams| {
            streams
                .iter()
                .find(|s| s.get("codec_type").and_then(|v| v.as_str()) == Some("video"))
        })
        .map(|v| {
            (
                v.get("width").and_then(|w| w.as_u64()).map(|w| w as u32),
                v.get("height").and_then(|h| h.as_u64()).map(|h| h as u32),
            )
        })
        .unwrap_or((None, None));

    Ok(FileMetadata {
        width,
        height,
        make,
        model,
        software,
        datetime_original,
        subsec_original: None,
        has_gps,
        has_maker_note: false,
        has_lens_info: tag_str(&["com.oplus.lens.model"]).is_some(),
        duration_seconds: json
            .get("format")
            .and_then(|f| f.get("duration"))
            .and_then(|d| d.as_str())
            .and_then(|s| s.parse::<f64>().ok()),
        metadata_readable: true,
    })
}

/// Video `creation_time`/`creationdate` tags show up in two shapes:
/// `2026-10-20T06:46:39.000000Z` (ffprobe's normalized UTC form) and
/// `2026-10-15T08:31:38+0900` (Apple's own, with a timezone offset).
fn parse_video_timestamp(s: &str) -> Option<NaiveDateTime> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.naive_utc())
        .ok()
        // Apple writes the offset without a colon (`+0900`), which RFC 3339
        // parsing rejects.
        .or_else(|| chrono::DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%z").map(|dt| dt.naive_utc()).ok())
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dataset_dir() -> Option<PathBuf> {
        crate::reference::photos()
    }

    #[test]
    fn video_timestamps_parse_in_every_form_the_cameras_write() {
        let utc = |s| parse_video_timestamp(s).map(|t| t.to_string());
        // RFC 3339 with Z (OnePlus, and iPhone's `creation_time`).
        assert_eq!(utc("2026-10-14T23:31:38.000000Z").as_deref(), Some("2026-10-14 23:31:38"));
        // Apple's `creationdate`: offset with no colon. Same instant as above.
        assert_eq!(utc("2026-10-15T08:31:38+0900").as_deref(), Some("2026-10-14 23:31:38"));
        // No offset at all: taken as written.
        assert_eq!(utc("2026-10-15T08:31:38").as_deref(), Some("2026-10-15 08:31:38"));
        assert_eq!(utc("not a date"), None);
    }

    #[test]
    fn camera_model_combines_make_and_model() {
        let m = FileMetadata {
            make: Some("Apple".into()),
            model: Some("iPhone Air".into()),
            ..Default::default()
        };
        assert_eq!(m.camera_model().as_deref(), Some("iPhone Air")); // Model wins over Make
    }

    #[test]
    #[ignore = "requires the private reference photo set (PHOTODEDUP_REFERENCE_SET)"]
    fn heic_extracts_full_camera_metadata() {
        let Some(dir) = dataset_dir() else { return };
        let meta = extract_image(&dir.join("IMG_3684.HEIC")).unwrap();
        assert_eq!(meta.make.as_deref(), Some("Apple"));
        assert_eq!(meta.model.as_deref(), Some("iPhone Air"));
        assert!(meta.has_gps);
        assert!(meta.datetime_original.is_some());
    }

    #[test]
    #[ignore = "requires the private reference photo set (PHOTODEDUP_REFERENCE_SET)"]
    fn messenger_jpg_has_no_camera_identity() {
        let Some(dir) = dataset_dir() else { return };
        let meta = extract_image(&dir.join("IMG_3465.JPG")).unwrap();
        // Has *some* EXIF (resolution/thumbnail), but no Make/Model/
        // DateTimeOriginal -- the 181-file lesson from PROJECT_CONTEXT.md.
        assert!(meta.make.is_none());
        assert!(meta.model.is_none());
    }

    #[test]
    #[ignore = "requires the private reference photo set (PHOTODEDUP_REFERENCE_SET) + ffprobe on PATH"]
    fn oneplus_video_extracts_vendor_model_via_ffprobe() {
        let Some(dir) = dataset_dir() else { return };
        let meta = extract_video(&dir.join("VID20261020154635.mp4"), &crate::tools::ffprobe(), Some(SchedulingHint::Utility)).unwrap();
        assert_eq!(meta.model.as_deref(), Some("OnePlus 15"));
        assert!(meta.datetime_original.is_some());
    }

    #[test]
    #[ignore = "requires the private reference photo set (PHOTODEDUP_REFERENCE_SET) + ffprobe on PATH"]
    fn iphone_video_extracts_make_and_model_via_ffprobe() {
        let Some(dir) = dataset_dir() else { return };
        let mov = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().map(|e| e.eq_ignore_ascii_case("mov")).unwrap_or(false));
        let Some(mov) = mov else { return };
        let meta = extract_video(&mov, &crate::tools::ffprobe(), Some(SchedulingHint::Utility)).unwrap();
        assert_eq!(meta.make.as_deref(), Some("Apple"));
        assert!(meta.datetime_original.is_some(), "Apple's +0900 creationdate must parse");
        // Make + model + time is what makes a clip an intact camera original.
        let p = crate::provenance::evaluate(&meta, "IMG_0001.MOV");
        assert_eq!(p.origin, crate::provenance::Origin::CameraOriginal);
    }
}
