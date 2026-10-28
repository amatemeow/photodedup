//! Tier-0 byte-identical grouping. The MVP hashed every file (2718 of
//! them) to find 2 exact matches -- fine at that scale, wasteful at
//! library scale. Bucketing by size first means blake3 only ever runs on
//! files that already collide on size, which is the free optimisation
//! noted in the architecture plan.

use crate::model::FileId;
use blake3::Hasher;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub struct ExactGroup {
    pub hash: [u8; 32],
    pub members: Vec<FileId>,
    pub size_bytes: u64,
}

/// `files`: (id, path, size). Only files whose size collides with at least
/// one other file get hashed at all.
pub fn find_exact_duplicates(files: &[(FileId, &Path, u64)]) -> Vec<ExactGroup> {
    let mut by_size: HashMap<u64, Vec<(FileId, &Path)>> = HashMap::new();
    for &(id, path, size) in files {
        by_size.entry(size).or_default().push((id, path));
    }

    let size_collision_groups: Vec<(u64, Vec<(FileId, &Path)>)> = by_size
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .collect();

    size_collision_groups
        .into_par_iter()
        .flat_map(|(size, members)| {
            let hashed: Vec<(FileId, [u8; 32])> = members
                .into_iter()
                .filter_map(|(id, path)| hash_file(path).ok().map(|h| (id, h)))
                .collect();

            let mut by_hash: HashMap<[u8; 32], Vec<FileId>> = HashMap::new();
            for (id, h) in hashed {
                by_hash.entry(h).or_default().push(id);
            }

            by_hash
                .into_iter()
                .filter(|(_, ids)| ids.len() > 1)
                .map(|(hash, members)| ExactGroup {
                    hash,
                    members,
                    size_bytes: size,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn hash_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut f = File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buf = [0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn identical_content_groups_even_with_different_names() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a.jpg");
        let b = tmp.path().join("b.jpg");
        let c = tmp.path().join("c.jpg"); // different content, same size
        std::fs::write(&a, b"hello world").unwrap();
        std::fs::write(&b, b"hello world").unwrap();
        std::fs::write(&c, b"HELLO WORLD").unwrap();

        let files = [(0u32, a.as_path(), 11u64), (1u32, b.as_path(), 11u64), (2u32, c.as_path(), 11u64)];
        let groups = find_exact_duplicates(&files);
        assert_eq!(groups.len(), 1);
        let mut members = groups[0].members.clone();
        members.sort();
        assert_eq!(members, vec![0, 1]);
    }

    #[test]
    fn different_sizes_never_hashed_against_each_other() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a.jpg");
        let b = tmp.path().join("b.jpg");
        std::fs::write(&a, b"short").unwrap();
        let mut f = std::fs::File::create(&b).unwrap();
        f.write_all(b"a bit longer content").unwrap();

        let files = [(0u32, a.as_path(), 5u64), (1u32, b.as_path(), 21u64)];
        // Different sizes -> no collision bucket -> zero hashing, zero groups.
        assert!(find_exact_duplicates(&files).is_empty());
    }
}
