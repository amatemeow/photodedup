//! The optional private reference photo set.
//!
//! Some regression tests and two diagnostic examples run against a real
//! photo library the maintainers validated the engine on (thousands of files,
//! two phones, HEIC and video). It is personal data and is not distributed, so
//! those tests are `#[ignore]`d. To run them, point this variable at a folder
//! that has the files they name:
//!
//! ```text
//! PHOTODEDUP_REFERENCE_SET=/path/to/photos cargo test -- --ignored
//! ```
//!
//! Without it they skip, and nothing else depends on it.

use std::path::PathBuf;

/// The environment variable that names the reference photo folder.
pub const ENV: &str = "PHOTODEDUP_REFERENCE_SET";

/// The reference photo folder, if `PHOTODEDUP_REFERENCE_SET` names one that exists.
pub fn photos() -> Option<PathBuf> {
    folder_from(std::env::var_os(ENV).map(PathBuf::from))
}

fn folder_from(value: Option<PathBuf>) -> Option<PathBuf> {
    value.filter(|p| p.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_or_missing_means_no_reference_set() {
        assert_eq!(folder_from(None), None);
        assert_eq!(folder_from(Some(PathBuf::from("/definitely/not/a/real/folder"))), None);
    }

    #[test]
    fn an_existing_folder_is_used_and_a_file_is_not() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(folder_from(Some(dir.path().to_path_buf())), Some(dir.path().to_path_buf()));
        let file = dir.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(folder_from(Some(file)), None, "a path that is not a folder is ignored");
    }
}
