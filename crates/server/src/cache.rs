//! The thumbnail and preview cache, with a size budget.
//!
//! Without a limit the cache grows with every photo looked at (a preview is
//! 175-400 KB, a grid thumbnail about 60-100 KB), which on a big library is
//! gigabytes of someone's disk. This wraps the plain read/write functions in
//! `routes::files` (which keep the security rules: owner-only directory,
//! atomic writes, never follow a link) with:
//!
//! - a **budget** in bytes, adjustable at run time, enforced after each write;
//! - **least-recently-used eviction**, recency being the file's modification
//!   time, refreshed on every read. Large previews go before grid thumbnails
//!   (they are the bulk of the bytes, and the grid is what you scroll);
//! - a **protected set**: the files around where the user is looking, which
//!   are never evicted whatever their age.
//!
//! Sizes are measured from the directory, not trusted from a running count,
//! each time eviction runs, so the count cannot drift.

use crate::routes::files::{cache_path, cache_read, cache_write, clear_cache, is_cache_entry_name};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, SystemTime};
use uuid::Uuid;

/// Grid thumbnails and large previews: the only two sizes the UI asks for.
pub const THUMB_SIZE: u32 = 800;
pub const PREVIEW_SIZE: u32 = 1600;

/// Evicting down to this share of the budget leaves room to grow, so eviction
/// is not run again after every single write.
const EVICT_TO_PERCENT: u64 = 90;

/// A read refreshes a file's recency only if it is older than this, so a
/// scroll through the grid is not a syscall per tile.
const TOUCH_AFTER: Duration = Duration::from_secs(60);

pub struct ThumbCache {
    dir: PathBuf,
    budget: AtomicU64,
    used: AtomicU64,
    protected: RwLock<HashSet<Uuid>>,
    evicting: Mutex<()>,
}

struct Entry {
    path: PathBuf,
    id: Uuid,
    px: u32,
    len: u64,
    modified: SystemTime,
}

impl ThumbCache {
    pub fn open(dir: PathBuf, budget_bytes: u64) -> Self {
        let cache = ThumbCache { dir, budget: AtomicU64::new(budget_bytes), used: AtomicU64::new(0), protected: RwLock::new(HashSet::new()), evicting: Mutex::new(()) };
        cache.used.store(cache.entries().iter().map(|e| e.len).sum(), Ordering::Relaxed);
        cache
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }
    pub fn budget(&self) -> u64 {
        self.budget.load(Ordering::Relaxed)
    }

    /// Changes the budget and, if the cache is now over it, trims it at once.
    pub fn set_budget(&self, bytes: u64) {
        self.budget.store(bytes, Ordering::Relaxed);
        self.evict();
    }

    /// The files whose entries must survive eviction: replaces the previous set.
    pub fn protect(&self, ids: HashSet<Uuid>) {
        *self.protected.write().unwrap() = ids;
    }

    pub fn contains(&self, id: &Uuid, size: u32) -> bool {
        std::fs::symlink_metadata(cache_path(&self.dir, id, size)).is_ok_and(|m| m.is_file())
    }

    pub fn read(&self, id: &Uuid, size: u32) -> Option<Vec<u8>> {
        let bytes = cache_read(&self.dir, id, size)?;
        self.touch(id, size);
        Some(bytes)
    }

    pub fn write(&self, id: &Uuid, size: u32, bytes: &[u8]) {
        let existed = self.contains(id, size);
        cache_write(&self.dir, id, size, bytes);
        let used = if existed { self.used() } else { self.used.fetch_add(bytes.len() as u64, Ordering::Relaxed) + bytes.len() as u64 };
        if used > self.budget() {
            self.evict();
        }
    }

    /// Empties the cache (the old entries belong to files that no longer exist).
    pub fn clear(&self) {
        clear_cache(&self.dir);
        self.used.store(0, Ordering::Relaxed);
    }

    fn touch(&self, id: &Uuid, size: u32) {
        let path = cache_path(&self.dir, id, size);
        let Ok(meta) = std::fs::symlink_metadata(&path) else { return };
        if !meta.is_file() || meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age < TOUCH_AFTER) {
            return;
        }
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
    }

    /// Every real cache entry on disk (never a temp file, a link or a stranger).
    fn entries(&self) -> Vec<Entry> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        dir.flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !is_cache_entry_name(&name) || !e.file_type().is_ok_and(|t| t.is_file()) {
                    return None;
                }
                let (id, px) = name.strip_suffix(".jpg")?.split_once('_')?;
                let meta = e.metadata().ok()?;
                Some(Entry { path: e.path(), id: Uuid::parse_str(id).ok()?, px: px.parse().ok()?, len: meta.len(), modified: meta.modified().ok()? })
            })
            .collect()
    }

    /// Brings the cache back under budget by deleting the least recently used
    /// entries, previews before grid thumbnails, sparing the protected files.
    /// If only protected files are left, the cache stays over budget rather
    /// than throw away what is on screen.
    fn evict(&self) {
        let Ok(_one_at_a_time) = self.evicting.try_lock() else { return };
        let mut entries = self.entries();
        let mut total: u64 = entries.iter().map(|e| e.len).sum();
        self.used.store(total, Ordering::Relaxed);
        let budget = self.budget();
        if total <= budget {
            return;
        }
        let target = budget / 100 * EVICT_TO_PERCENT;
        let protected = self.protected.read().unwrap().clone();
        entries.retain(|e| !protected.contains(&e.id));
        entries.sort_by_key(|e| (e.px <= THUMB_SIZE, e.modified)); // previews first, oldest first
        for e in entries {
            if total <= target {
                break;
            }
            if std::fs::remove_file(&e.path).is_ok() {
                total -= e.len;
            }
        }
        self.used.store(total, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(budget: u64) -> (tempfile::TempDir, ThumbCache) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = crate::routes::files::init_cache_dir(&tmp.path().join("scan.sqlite3")).unwrap();
        (tmp, ThumbCache::open(dir, budget))
    }
    /// Writes `n` bytes under `id`/`px` and backdates it so ordering is certain.
    fn put(c: &ThumbCache, id: &Uuid, px: u32, n: usize, age_secs: u64) {
        c.write(id, px, &vec![0u8; n]);
        let f = std::fs::OpenOptions::new().write(true).open(cache_path(c.dir(), id, px)).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_secs)).unwrap();
    }

    #[test]
    fn staying_within_the_budget_evicts_nothing() {
        let (_t, c) = cache(10_000);
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        c.write(&a, 800, &[0; 3000]);
        c.write(&b, 800, &[0; 3000]);
        assert!(c.contains(&a, 800) && c.contains(&b, 800));
        assert_eq!(c.used(), 6000);
    }

    #[test]
    fn going_over_the_budget_removes_the_least_recently_used_first() {
        let (_t, c) = cache(10_000);
        let (old, mid, new) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        put(&c, &old, 800, 4000, 300);
        put(&c, &mid, 800, 4000, 200);
        put(&c, &new, 800, 4000, 100); // 12 000 > 10 000: trims to 9 000 or less
        assert!(!c.contains(&old, 800), "the oldest goes first");
        assert!(c.contains(&mid, 800) && c.contains(&new, 800));
        assert!(c.used() <= 9_000);
    }

    #[test]
    fn large_previews_are_given_up_before_grid_thumbnails_even_when_newer() {
        let (_t, c) = cache(10_000);
        let (thumb, preview) = (Uuid::new_v4(), Uuid::new_v4());
        put(&c, &thumb, 800, 4000, 500); // old, but a grid thumbnail
        put(&c, &preview, 1600, 4000, 10); // new, but a preview
        put(&c, &Uuid::new_v4(), 800, 4000, 5);
        assert!(!c.contains(&preview, 1600), "the preview goes first");
        assert!(c.contains(&thumb, 800));
    }

    #[test]
    fn protected_files_survive_however_old_and_the_rest_make_room() {
        let (_t, c) = cache(10_000);
        let (keep, other, newest) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        c.protect([keep].into());
        put(&c, &keep, 1600, 4000, 900); // the oldest of all
        put(&c, &other, 800, 4000, 100);
        put(&c, &newest, 800, 4000, 50);
        assert!(c.contains(&keep, 1600), "what is on screen is never evicted");
        assert!(!c.contains(&other, 800));
    }

    #[test]
    fn it_stays_over_budget_rather_than_evict_protected_files() {
        let (_t, c) = cache(1_000);
        let a = Uuid::new_v4();
        c.protect([a].into());
        c.write(&a, 1600, &[0; 5000]);
        assert!(c.contains(&a, 1600));
    }

    #[test]
    fn lowering_the_budget_trims_at_once_and_reading_refreshes_recency() {
        let (_t, c) = cache(100_000);
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        put(&c, &a, 800, 4000, 300);
        put(&c, &b, 800, 4000, 200);
        assert!(c.read(&a, 800).is_some()); // a is now the more recently used
        c.set_budget(5_000);
        assert!(c.contains(&a, 800) && !c.contains(&b, 800), "reading a made b the older one");
    }

    #[test]
    fn opening_counts_what_is_already_there_and_stray_files_are_never_touched() {
        let (t, c) = cache(1_000_000);
        let a = Uuid::new_v4();
        c.write(&a, 800, &[0; 700]);
        let stray = c.dir().join("notes.txt");
        std::fs::write(&stray, vec![0u8; 50_000]).unwrap();
        let reopened = ThumbCache::open(c.dir().to_path_buf(), 100);
        assert_eq!(reopened.used(), 700, "only cache entries are counted");
        reopened.set_budget(1);
        assert!(stray.exists(), "a file that is not a cache entry is left alone");
        drop(t);
    }
}
