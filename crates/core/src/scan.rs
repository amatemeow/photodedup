//! Directory walk: builds the file list, detects cloud placeholders
//! (never silently hydrated), hard-refuses managed-library trees, and
//! groups files into sibling sets that must move together as a unit.

use crate::model::{classify_extension, FileId, FileRecord, MediaClass, SiblingSet};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Directory names that mark a managed photo library. Finding one anywhere
/// in the walk causes that whole subtree to be skipped and reported --
/// moving a file out from under a managed library's catalog corrupts it.
/// See `/ROADMAP.md`: this guard ships day one even though *support* for
/// these libraries is deferred.
const MANAGED_LIBRARY_MARKERS: &[&str] = &[".photoslibrary", ".aplibrary", ".lrcat"];

#[derive(Debug, Default)]
pub struct ScanReport {
    pub files: Vec<FileRecord>,
    pub sibling_sets: Vec<SiblingSet>,
    pub placeholder_count: u32,
    pub placeholder_bytes: u64,
    pub refused_managed_libraries: Vec<PathBuf>,
}

pub fn scan(root: &Path) -> ScanReport {
    let mut report = ScanReport::default();
    let mut next_id: FileId = 0;
    let mut raw_files: Vec<(FileId, PathBuf, MediaClass)> = Vec::new();

    // Plain iteration (not `filter_entry`) so a managed-library directory
    // is actually yielded once -- letting us record it -- before we skip
    // its subtree with `skip_current_dir()`. `filter_entry`'s predicate
    // would hide the entry from us entirely, which is exactly backwards
    // when the whole point is to *report* what got refused.
    let mut it = WalkDir::new(root).into_iter();
    loop {
        let entry = match it.next() {
            Some(Ok(e)) => e,
            Some(Err(_)) => continue, // unreadable entry: skip, don't abort the whole scan
            None => break,
        };

        if entry.file_type().is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                if MANAGED_LIBRARY_MARKERS.iter().any(|m| name.ends_with(m)) {
                    report.refused_managed_libraries.push(entry.path().to_path_buf());
                    it.skip_current_dir();
                }
            }
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let Some(class) = classify_extension(ext) else {
            continue; // not a format we handle -- silently skip, not an error
        };

        let Ok(meta) = std::fs::symlink_metadata(path) else {
            continue;
        };
        // Don't follow symlinks into who-knows-where; a dangling or
        // cross-device symlink shouldn't crash the scan.
        let Ok(meta) = std::fs::metadata(path).or_else(|_| Ok::<_, std::io::Error>(meta)) else {
            continue;
        };

        let is_placeholder = is_cloud_placeholder(path, &meta);
        if is_placeholder {
            report.placeholder_count += 1;
            report.placeholder_bytes += meta.len();
        }

        let file_name = path.file_name().unwrap().to_string_lossy().to_string();
        let mtime_unix = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let id = next_id;
        next_id += 1;
        raw_files.push((id, path.to_path_buf(), class));
        report.files.push(FileRecord {
            id,
            path: path.to_path_buf(),
            file_name,
            media_class: class.into(),
            size_bytes: meta.len(),
            mtime_unix,
            is_placeholder,
        });
    }

    report.sibling_sets = group_siblings(&raw_files);
    report
}

/// Groups files by `(parent directory, case-folded stem)`. Within a group,
/// the primary is picked by media-class priority (RAW > Image > Video >
/// Sidecar is wrong for photo+video Live Photo pairs, so priority is
/// Image/Video first since those are what gets hashed/compared; RAW
/// outranks a same-stem JPEG since RAW is the more authentic original;
/// Sidecar never becomes primary). Ties broken by filename for
/// determinism.
fn group_siblings(files: &[(FileId, PathBuf, MediaClass)]) -> Vec<SiblingSet> {
    let mut groups: BTreeMap<(PathBuf, String), Vec<(FileId, PathBuf, MediaClass)>> =
        BTreeMap::new();

    for (id, path, class) in files {
        let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        groups
            .entry((dir, stem))
            .or_default()
            .push((*id, path.clone(), *class));
    }

    fn primary_rank(class: MediaClass) -> u8 {
        match class {
            MediaClass::Image => 0,
            MediaClass::Video => 0, // Live Photo: either half is a fine primary
            MediaClass::Raw => 1,
            MediaClass::Sidecar => 2,
        }
    }

    groups
        .into_values()
        .map(|mut members| {
            members.sort_by(|a, b| {
                primary_rank(a.2)
                    .cmp(&primary_rank(b.2))
                    .then_with(|| a.1.cmp(&b.1))
            });
            let primary = members[0].0;
            let siblings = members[1..].iter().map(|m| m.0).collect();
            SiblingSet { primary, siblings }
        })
        .collect()
}

#[cfg(unix)]
fn is_cloud_placeholder(_path: &Path, meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // macOS: a dataless (not-yet-downloaded) iCloud/OneDrive file reports
    // zero allocated blocks despite a nonzero logical size. Best-effort on
    // Linux too (most sync clients there fully materialize files, so this
    // rarely fires, but it's a free check when it does) -- see /ROADMAP.md.
    meta.blocks() == 0 && meta.size() > 0
}

#[cfg(windows)]
fn is_cloud_placeholder(_path: &Path, meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    is_cloud_placeholder_attrs(meta.file_attributes())
}

/// The actual bit-check logic, factored out so it's unit-testable on any
/// host OS even though the attribute *fetch* above is Windows-only.
#[allow(dead_code)]
fn is_cloud_placeholder_attrs(attrs: u32) -> bool {
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    attrs
        & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    #[cfg(windows)]
    fn windows_placeholder_bit_logic() {
        assert!(is_cloud_placeholder_attrs(0x0004_0000));
        assert!(is_cloud_placeholder_attrs(0x0000_1000));
        assert!(!is_cloud_placeholder_attrs(0x0000_0020)); // FILE_ATTRIBUTE_ARCHIVE only
    }

    // On non-Windows hosts the bit-logic function is still compiled (it has
    // no cfg on itself) but is unused outside tests, so exercise it here to
    // keep it from bitrotting silently between Windows CI runs.
    #[test]
    #[cfg(not(windows))]
    fn windows_placeholder_bit_logic_compiles_and_works_off_windows_too() {
        assert!(is_cloud_placeholder_attrs(0x0004_0000));
        assert!(!is_cloud_placeholder_attrs(0));
    }

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, b"x").unwrap();
        p
    }

    #[test]
    fn sibling_grouping_links_raw_jpeg_and_sidecar() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "IMG_0001.CR3");
        touch(tmp.path(), "IMG_0001.JPG");
        touch(tmp.path(), "IMG_0001.XMP");
        touch(tmp.path(), "IMG_0002.JPG"); // unrelated, own group

        let report = scan(tmp.path());
        assert_eq!(report.files.len(), 4);
        assert_eq!(report.sibling_sets.len(), 2);

        let big_set = report
            .sibling_sets
            .iter()
            .find(|s| s.siblings.len() == 2)
            .expect("IMG_0001 group should have 2 siblings");
        // Primary should be the JPEG (Image), not the RAW or the sidecar.
        let primary_file = report.files.iter().find(|f| f.id == big_set.primary).unwrap();
        assert!(primary_file.file_name.ends_with(".JPG"));
    }

    #[test]
    fn live_photo_heic_mov_pair_groups_together() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "IMG_5000.HEIC");
        touch(tmp.path(), "IMG_5000.MOV");

        let report = scan(tmp.path());
        assert_eq!(report.sibling_sets.len(), 1);
        assert_eq!(report.sibling_sets[0].siblings.len(), 1);
    }

    #[test]
    fn managed_library_is_refused_not_scanned() {
        let tmp = TempDir::new().unwrap();
        let lib = tmp.path().join("Photos Library.photoslibrary");
        fs::create_dir(&lib).unwrap();
        touch(&lib, "inside.jpg");
        touch(tmp.path(), "outside.jpg");

        let report = scan(tmp.path());
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.files[0].file_name, "outside.jpg");
        assert_eq!(report.refused_managed_libraries.len(), 1);
    }

    #[test]
    fn non_media_extensions_are_skipped_without_erroring() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "readme.txt");
        touch(tmp.path(), "photo.jpg");
        let report = scan(tmp.path());
        assert_eq!(report.files.len(), 1);
    }
}
