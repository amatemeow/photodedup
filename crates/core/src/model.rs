//! Shared data types used across scan/classify/cluster/keeper. Kept
//! separate from any one of those modules since all of them need it.

use crate::hash::Hash256;
use crate::provenance::Provenance;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type FileId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MediaClass {
    Raw,
    Image,
    Video,
    Sidecar,
}

/// Extension-based classification for sibling grouping (see `scan.rs`).
/// Not exhaustive of every camera's RAW format, but covers what a
/// consumer-facing tool needs; extend this list rather than special-case
/// around it.
pub fn classify_extension(ext: &str) -> Option<MediaClass> {
    let ext = ext.to_lowercase();
    Some(match ext.as_str() {
        "cr2" | "cr3" | "nef" | "arw" | "dng" | "raf" | "orf" | "rw2" | "pef" | "srw" => {
            MediaClass::Raw
        }
        "jpg" | "jpeg" | "heic" | "heif" | "png" | "tif" | "tiff" | "webp" => MediaClass::Image,
        "mov" | "mp4" | "m4v" => MediaClass::Video,
        "xmp" | "aae" | "json" | "thm" | "dop" | "pp3" => MediaClass::Sidecar,
        _ => return None,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: FileId,
    pub path: PathBuf,
    pub file_name: String,
    pub media_class: MediaClassSerde,
    pub size_bytes: u64,
    pub mtime_unix: i64,
    pub is_placeholder: bool,
}

/// `MediaClass` isn't itself `Serialize`/`Deserialize` to keep `model.rs`
/// free of a serde dependency on the enum used purely for in-memory
/// grouping logic; this mirror exists only where persistence needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaClassSerde {
    Raw,
    Image,
    Video,
    Sidecar,
}

impl From<MediaClass> for MediaClassSerde {
    fn from(c: MediaClass) -> Self {
        match c {
            MediaClass::Raw => MediaClassSerde::Raw,
            MediaClass::Image => MediaClassSerde::Image,
            MediaClass::Video => MediaClassSerde::Video,
            MediaClass::Sidecar => MediaClassSerde::Sidecar,
        }
    }
}

/// A group of files that must move together on any decision: a RAW+JPEG+
/// XMP triple, a HEIC+MOV Live Photo pair, etc. (see `scan.rs`'s grouping
/// logic). `primary` is the file that gets hashed/compared/classified;
/// the rest ride along.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiblingSet {
    pub primary: FileId,
    pub siblings: Vec<FileId>,
}

impl SiblingSet {
    pub fn all_members(&self) -> impl Iterator<Item = FileId> + '_ {
        std::iter::once(self.primary).chain(self.siblings.iter().copied())
    }
}

/// Per-file analysis results, keyed by the primary `FileId` of its sibling
/// set (siblings don't get their own entry -- they're not independently
/// classified, see `SiblingSet`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzedFile {
    pub file_id: FileId,
    pub provenance: Provenance,
    pub phash: Option<Hash256Serde>,
    pub dhash: Option<Hash256Serde>,
    pub exact_hash: Option<[u8; 32]>,
    pub sharpness: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera_model: Option<String>,
    pub capture_time: Option<chrono::NaiveDateTime>,
}

/// Serializable mirror of `Hash256` (which deliberately stays a plain
/// bit-twiddling type with no serde dependency of its own).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Hash256Serde(pub u128, pub u128);

impl From<Hash256> for Hash256Serde {
    fn from(h: Hash256) -> Self {
        Hash256Serde(h.0, h.1)
    }
}
impl From<Hash256Serde> for Hash256 {
    fn from(h: Hash256Serde) -> Self {
        Hash256(h.0, h.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_classification_covers_reference_dataset_formats() {
        assert_eq!(classify_extension("HEIC"), Some(MediaClass::Image));
        assert_eq!(classify_extension("jpg"), Some(MediaClass::Image));
        assert_eq!(classify_extension("MP4"), Some(MediaClass::Video));
        assert_eq!(classify_extension("mov"), Some(MediaClass::Video));
        assert_eq!(classify_extension("exe"), None);
    }
}
