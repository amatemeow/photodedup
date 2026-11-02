//! The only code in the engine allowed to change anything on disk.
//!
//! Two promises, enforced here rather than left to care at each call site:
//!
//! 1. **Nothing outside the workspace.** Every path that is read for a move
//!    or written to is built from the workspace root plus a *validated
//!    relative path* (no `..`, no root or drive prefix, no empty or NUL
//!    components) and checked, one component at a time, to contain no
//!    symlink. A symlinked folder inside the workspace that points outside
//!    cannot be used to reach outside it.
//! 2. **Nothing that can't be undone.** The operations on offer are exactly
//!    four: move a file into the quarantine folder, move it back, create a
//!    folder, and write a small text file inside the quarantine folder.
//!    There is no delete and no overwrite: a move onto an existing path is
//!    refused. The single use of `remove_file` is the tail of a cross-volume
//!    move, after the copy has been verified by hash, so the net effect is
//!    still a move.
//!
//! A time-of-check/time-of-use race (something swapping a folder for a
//! symlink between the check and the rename) is not defended against: this
//! is a single-user tool working on the user's own folder, and each check
//! is repeated immediately before the operation it protects.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The folder, directly under the workspace root, that quarantined files
/// are moved into. The scanner skips it so quarantined copies are never
/// mistaken for part of the library.
pub const QUARANTINE_DIR_NAME: &str = ".photodedup-quarantine";

/// Path components that mean "someone else's database lives here": never
/// move anything out of one.
const PROTECTED_MARKERS: &[&str] = &[".photoslibrary", ".aplibrary", ".lrcat", ".lrdata"];
const PROTECTED_NAMES: &[&str] = &[".git"];

#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("refused: {0}")]
    Unsafe(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

fn unsafe_path(why: impl Into<String>) -> GuardError {
    GuardError::Unsafe(why.into())
}

/// A canonicalized workspace root. Constructing one is the only way to get
/// the mutating operations.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn open(root: &Path) -> Result<Self, GuardError> {
        let root = fs::canonicalize(root).map_err(|e| unsafe_path(format!("workspace {} is not usable: {e}", root.display())))?;
        if !root.is_dir() {
            return Err(unsafe_path(format!("workspace {} is not a folder", root.display())));
        }
        Ok(Workspace { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A relative path that stays inside the workspace by construction:
    /// only plain names, nothing empty, nothing inside our own quarantine
    /// folder, nothing inside a managed library or `.git`.
    pub fn validate_relative(&self, rel: &str) -> Result<PathBuf, GuardError> {
        if rel.is_empty() || rel.contains('\0') {
            return Err(unsafe_path("empty or malformed path"));
        }
        let p = Path::new(rel);
        let mut out = PathBuf::new();
        for c in p.components() {
            match c {
                Component::Normal(name) => {
                    let n = name.to_string_lossy();
                    if PROTECTED_NAMES.contains(&n.as_ref()) || PROTECTED_MARKERS.iter().any(|m| n.ends_with(m)) {
                        return Err(unsafe_path(format!("{rel} is inside a protected folder ({n})")));
                    }
                    out.push(name);
                }
                _ => return Err(unsafe_path(format!("{rel} is not a plain relative path"))),
            }
        }
        if out.as_os_str().is_empty() {
            return Err(unsafe_path("empty path"));
        }
        if out.components().next().map(|c| c.as_os_str() == QUARANTINE_DIR_NAME).unwrap_or(false) {
            return Err(unsafe_path(format!("{rel} is already in the quarantine folder")));
        }
        Ok(out)
    }

    /// Validates a batch folder as stored in the journal: exactly
    /// `.photodedup-quarantine/<name>` with a name of digits, `T`, `Z`, `-`.
    pub fn validate_batch_dir(&self, batch_rel: &str) -> Result<PathBuf, GuardError> {
        let mut parts = Path::new(batch_rel).components();
        let ok = matches!(
            (parts.next(), parts.next(), parts.next()),
            (Some(Component::Normal(a)), Some(Component::Normal(b)), None)
                if a == QUARANTINE_DIR_NAME
                    && !b.is_empty()
                    && b.to_string_lossy().chars().all(|c| c.is_ascii_digit() || matches!(c, 'T' | 'Z' | '-'))
        );
        if !ok {
            return Err(unsafe_path(format!("{batch_rel} is not a quarantine batch folder")));
        }
        Ok(PathBuf::from(batch_rel))
    }

    /// Walks `rel`'s components under the root, requiring each existing one
    /// to be a real directory (and the last a real file when `want_file`),
    /// never a symlink.
    fn check_no_symlinks(&self, rel: &Path, want_file: bool) -> Result<PathBuf, GuardError> {
        let mut cur = self.root.clone();
        let comps: Vec<_> = rel.components().collect();
        for (i, c) in comps.iter().enumerate() {
            cur.push(c.as_os_str());
            let meta = fs::symlink_metadata(&cur)?;
            if meta.file_type().is_symlink() {
                return Err(unsafe_path(format!("{} is a symlink", cur.display())));
            }
            let last = i + 1 == comps.len();
            if last && want_file {
                if !meta.is_file() {
                    return Err(unsafe_path(format!("{} is not a regular file", cur.display())));
                }
            } else if !meta.is_dir() {
                return Err(unsafe_path(format!("{} is not a folder", cur.display())));
            }
        }
        self.require_inside(&cur)?;
        Ok(cur)
    }

    /// The fully resolved path must still be under the workspace root. This
    /// is a second line of defence behind the symlink checks: on Windows a
    /// directory *junction* is not reported as a symlink, yet leads anywhere,
    /// and resolving the path is what exposes where it really goes.
    fn require_inside(&self, path: &Path) -> Result<(), GuardError> {
        let real = fs::canonicalize(path)?;
        if real.starts_with(&self.root) {
            Ok(())
        } else {
            Err(unsafe_path(format!("{} leads outside the workspace", path.display())))
        }
    }

    /// The real path of an existing regular file inside the workspace.
    pub fn existing_file(&self, rel: &str) -> Result<PathBuf, GuardError> {
        let rel = self.validate_relative(rel)?;
        self.check_no_symlinks(&rel, true)
    }

    /// Creates `rel` (a folder path under the root) one level at a time,
    /// refusing to pass through a symlink. Returns the real path.
    fn ensure_dir(&self, rel: &Path) -> Result<PathBuf, GuardError> {
        let mut cur = self.root.clone();
        for c in rel.components() {
            cur.push(c.as_os_str());
            match fs::symlink_metadata(&cur) {
                Ok(m) if m.file_type().is_symlink() => return Err(unsafe_path(format!("{} is a symlink", cur.display()))),
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Err(unsafe_path(format!("{} exists and is not a folder", cur.display()))),
                Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(&cur)?,
                Err(e) => return Err(e.into()),
            }
        }
        self.require_inside(&cur)?;
        Ok(cur)
    }

    /// Creates a fresh batch folder `.photodedup-quarantine/<name>`, adding
    /// a numeric suffix if that name is taken. Returns its relative path.
    pub fn new_batch_dir(&self, name: &str) -> Result<String, GuardError> {
        let base = format!("{QUARANTINE_DIR_NAME}/{name}");
        self.validate_batch_dir(&base)?;
        self.ensure_dir(Path::new(QUARANTINE_DIR_NAME))?;
        for n in 0..1000 {
            let rel = if n == 0 { base.clone() } else { format!("{base}-{n}") };
            let p = self.root.join(&rel);
            if fs::symlink_metadata(&p).is_err() {
                fs::create_dir(&p)?;
                return Ok(rel);
            }
        }
        Err(unsafe_path("could not find a free quarantine folder name"))
    }

    /// Moves a file from its place in the workspace into a batch folder,
    /// keeping its relative path. Refuses to overwrite.
    pub fn quarantine(&self, batch_rel: &str, rel: &str) -> Result<(), GuardError> {
        let batch = self.validate_batch_dir(batch_rel)?;
        let from = self.existing_file(rel)?;
        let rel = self.validate_relative(rel)?;
        let to_dir = match rel.parent() {
            Some(p) if !p.as_os_str().is_empty() => self.ensure_dir(&batch.join(p))?,
            _ => self.ensure_dir(&batch)?,
        };
        let to = to_dir.join(rel.file_name().expect("validated: has a file name"));
        move_no_overwrite(&from, &to)
    }

    /// The inverse: moves a quarantined file back to its original place,
    /// recreating folders as needed. Refuses to overwrite whatever is there.
    pub fn restore(&self, batch_rel: &str, rel: &str) -> Result<(), GuardError> {
        let batch = self.validate_batch_dir(batch_rel)?;
        let rel = self.validate_relative(rel)?;
        let from = self.check_no_symlinks(&batch.join(&rel), true)?;
        let to_dir = match rel.parent() {
            Some(p) if !p.as_os_str().is_empty() => self.ensure_dir(p)?,
            _ => self.root.clone(),
        };
        let to = to_dir.join(rel.file_name().expect("validated: has a file name"));
        move_no_overwrite(&from, &to)
    }

    /// The real path of a quarantined file, for showing it (a thumbnail) without restoring it.
    pub fn quarantined_file(&self, batch_rel: &str, rel: &str) -> Result<PathBuf, GuardError> {
        let batch = self.validate_batch_dir(batch_rel)?;
        let rel = self.validate_relative(rel)?;
        self.check_no_symlinks(&batch.join(rel), true)
    }

    /// Whether a quarantined copy exists (without following links).
    pub fn quarantined_exists(&self, batch_rel: &str, rel: &str) -> bool {
        match (self.validate_batch_dir(batch_rel), self.validate_relative(rel)) {
            (Ok(b), Ok(r)) => self.check_no_symlinks(&b.join(r), true).is_ok(),
            _ => false,
        }
    }

    /// Whether something (anything) occupies `rel` in the workspace.
    pub fn occupied(&self, rel: &str) -> bool {
        self.validate_relative(rel).map(|r| fs::symlink_metadata(self.root.join(r)).is_ok()).unwrap_or(true)
    }

    /// Writes a small text file directly inside a batch folder (manifests).
    /// Never overwrites an existing file.
    pub fn write_in_batch(&self, batch_rel: &str, name: &str, contents: &[u8]) -> Result<(), GuardError> {
        let batch = self.validate_batch_dir(batch_rel)?;
        if name.contains(['/', '\\', '\0']) || name.is_empty() || name == "." || name == ".." {
            return Err(unsafe_path("bad file name"));
        }
        let dir = self.check_no_symlinks(&batch, false)?;
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(dir.join(name))?;
        io::Write::write_all(&mut f, contents)?;
        Ok(())
    }

    /// Removes folders left empty inside a batch folder after an undo.
    /// `remove_dir` only succeeds on an empty folder, so nothing that holds
    /// a file can be removed, and the batch folder itself is left alone.
    pub fn prune_empty_dirs(&self, batch_rel: &str) {
        let Ok(batch) = self.validate_batch_dir(batch_rel) else { return };
        let Ok(dir) = self.check_no_symlinks(&batch, false) else { return };
        fn prune(dir: &Path) {
            let Ok(entries) = fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    prune(&e.path());
                    let _ = fs::remove_dir(e.path()); // fails (harmlessly) unless empty
                }
            }
        }
        prune(&dir);
    }
}

/// Renames `from` to `to`, refusing if `to` exists. Across devices (where a
/// rename can't work) it copies, verifies the copy by hash, and only then
/// removes the source.
fn move_no_overwrite(from: &Path, to: &Path) -> Result<(), GuardError> {
    if fs::symlink_metadata(to).is_ok() {
        return Err(unsafe_path(format!("{} already exists; not overwriting", to.display())));
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => copy_verify_delete(from, to),
        Err(e) => Err(e.into()),
    }
}

fn hash_file(path: &Path) -> io::Result<blake3::Hash> {
    let mut hasher = blake3::Hasher::new();
    let mut f = fs::File::open(path)?;
    io::copy(&mut f, &mut hasher)?;
    Ok(hasher.finalize())
}

/// The cross-volume path: copy to a temporary name beside the destination,
/// compare content hashes, rename into place, and only then remove the
/// source. If anything fails the source is untouched and the temporary copy
/// is removed.
pub(crate) fn copy_verify_delete(from: &Path, to: &Path) -> Result<(), GuardError> {
    let tmp = to.with_extension(format!("{}.photodedup-part", to.extension().and_then(|e| e.to_str()).unwrap_or("")));
    let cleanup = |e: GuardError| {
        let _ = fs::remove_file(&tmp);
        e
    };
    {
        let mut src = fs::File::open(from)?;
        let mut dst = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        io::copy(&mut src, &mut dst).map_err(|e| cleanup(e.into()))?;
        dst.sync_all().map_err(|e| cleanup(e.into()))?;
        if let Ok(modified) = fs::metadata(from).and_then(|m| m.modified()) {
            let _ = dst.set_modified(modified);
        }
    }
    let (a, b) = (hash_file(from).map_err(|e| cleanup(e.into()))?, hash_file(&tmp).map_err(|e| cleanup(e.into()))?);
    if a != b {
        return Err(cleanup(unsafe_path("copy did not match the original; source left untouched")));
    }
    fs::rename(&tmp, to).map_err(|e| cleanup(e.into()))?;
    fs::remove_file(from)?; // the copy is verified and in place: this completes the move
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn ws() -> (tempfile::TempDir, Workspace) {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::open(tmp.path()).unwrap();
        (tmp, ws)
    }
    #[test]
    fn a_resolved_path_that_leaves_the_workspace_is_refused_even_if_no_symlink_is_reported() {
        // The symlink checks are the first line; this is the second. It is called
        // directly on a link to show it catches an escape the first would miss (what
        // a Windows junction looks like to the checks above).
        let (tmp, ws) = ws();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("inside")).unwrap();
        symlink(outside.path(), tmp.path().join("inside/door")).unwrap();

        assert!(ws.require_inside(&ws.root().join("inside")).is_ok());
        let err = ws.require_inside(&ws.root().join("inside/door")).unwrap_err();
        assert!(err.to_string().contains("leads outside"), "{err}");
    }

    fn put(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    #[test]
    fn paths_that_could_leave_the_workspace_are_rejected() {
        let (_t, ws) = ws();
        for bad in ["", "..", "../x", "a/../../x", "/etc/passwd", "a/./b/../../../c", ".", "a\0b"] {
            assert!(ws.validate_relative(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(ws.validate_relative("a/b/c.jpg").is_ok());
        assert!(ws.validate_relative("spaces and ünïcode/ph oto.JPG").is_ok());
    }

    #[test]
    fn managed_libraries_git_and_the_quarantine_itself_are_off_limits() {
        let (_t, ws) = ws();
        for bad in ["Photos.photoslibrary/x.jpg", "a/Catalog.lrcat/x", ".git/config", "a/.git/x", ".photodedup-quarantine/20260101T000000Z/x.jpg"] {
            assert!(ws.validate_relative(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn a_batch_folder_must_have_exactly_the_expected_shape() {
        let (_t, ws) = ws();
        assert!(ws.validate_batch_dir(".photodedup-quarantine/20260101T120000Z").is_ok());
        assert!(ws.validate_batch_dir(".photodedup-quarantine/20260101T120000Z-2").is_ok());
        for bad in ["", ".photodedup-quarantine", ".photodedup-quarantine/..", ".photodedup-quarantine/a/b", "other/20260101", "../.photodedup-quarantine/x", ".photodedup-quarantine/evil name", "/abs"] {
            assert!(ws.validate_batch_dir(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn a_symlinked_folder_pointing_outside_cannot_be_used_to_reach_outside() {
        let (tmp, ws) = ws();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.jpg"), b"outside").unwrap();
        symlink(outside.path(), tmp.path().join("link")).unwrap();

        assert!(ws.existing_file("link/secret.jpg").is_err());
        let batch = ws.new_batch_dir("20260101T000000Z").unwrap();
        assert!(ws.quarantine(&batch, "link/secret.jpg").is_err());
        assert!(outside.path().join("secret.jpg").exists(), "the outside file was not touched");
    }

    #[test]
    fn a_symlinked_file_is_never_moved() {
        let (tmp, ws) = ws();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("target.jpg"), b"x").unwrap();
        symlink(outside.path().join("target.jpg"), tmp.path().join("l.jpg")).unwrap();
        let batch = ws.new_batch_dir("20260101T000000Z").unwrap();
        assert!(ws.quarantine(&batch, "l.jpg").is_err());
        assert!(tmp.path().join("l.jpg").exists());
    }

    #[test]
    fn a_symlinked_quarantine_folder_is_refused() {
        let (tmp, ws) = ws();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), tmp.path().join(QUARANTINE_DIR_NAME)).unwrap();
        assert!(ws.new_batch_dir("20260101T000000Z").is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0, "nothing was created outside");
    }

    #[test]
    fn quarantine_then_restore_is_byte_identical_and_keeps_the_folder_layout() {
        let (tmp, ws) = ws();
        put(tmp.path(), "a/b/pic one.jpg", b"first");
        put(tmp.path(), "top.jpg", b"second");
        let before = fs::metadata(tmp.path().join("a/b/pic one.jpg")).unwrap().modified().unwrap();

        let batch = ws.new_batch_dir("20260101T000000Z").unwrap();
        ws.quarantine(&batch, "a/b/pic one.jpg").unwrap();
        ws.quarantine(&batch, "top.jpg").unwrap();
        assert!(!tmp.path().join("top.jpg").exists());
        assert_eq!(fs::read(tmp.path().join(&batch).join("a/b/pic one.jpg")).unwrap(), b"first");

        ws.restore(&batch, "top.jpg").unwrap();
        ws.restore(&batch, "a/b/pic one.jpg").unwrap();
        assert_eq!(fs::read(tmp.path().join("top.jpg")).unwrap(), b"second");
        assert_eq!(fs::read(tmp.path().join("a/b/pic one.jpg")).unwrap(), b"first");
        assert_eq!(fs::metadata(tmp.path().join("a/b/pic one.jpg")).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn nothing_is_ever_overwritten_in_either_direction() {
        let (tmp, ws) = ws();
        put(tmp.path(), "x.jpg", b"original");
        let batch = ws.new_batch_dir("20260101T000000Z").unwrap();
        ws.quarantine(&batch, "x.jpg").unwrap();

        // something new now sits where the file used to be
        put(tmp.path(), "x.jpg", b"newer file");
        assert!(ws.restore(&batch, "x.jpg").is_err());
        assert_eq!(fs::read(tmp.path().join("x.jpg")).unwrap(), b"newer file");
        assert!(ws.quarantined_exists(&batch, "x.jpg"), "the quarantined copy is still there");

        // and quarantining onto an occupied quarantine slot is refused too
        fs::remove_file(tmp.path().join("x.jpg")).unwrap();
        put(tmp.path(), "x.jpg", b"third");
        assert!(ws.quarantine(&batch, "x.jpg").is_err());
        assert_eq!(fs::read(tmp.path().join("x.jpg")).unwrap(), b"third");
    }

    #[test]
    fn manifests_are_written_only_inside_a_batch_and_never_overwritten() {
        let (tmp, ws) = ws();
        let batch = ws.new_batch_dir("20260101T000000Z").unwrap();
        ws.write_in_batch(&batch, "manifest.csv", b"a,b").unwrap();
        assert!(ws.write_in_batch(&batch, "manifest.csv", b"again").is_err());
        assert!(ws.write_in_batch(&batch, "../escape.txt", b"x").is_err());
        assert!(ws.write_in_batch("../elsewhere", "m.csv", b"x").is_err());
        assert_eq!(fs::read(tmp.path().join(&batch).join("manifest.csv")).unwrap(), b"a,b");
    }

    #[test]
    fn batch_folder_names_never_collide() {
        let (_t, ws) = ws();
        let a = ws.new_batch_dir("20260101T000000Z").unwrap();
        let b = ws.new_batch_dir("20260101T000000Z").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn pruning_removes_only_empty_folders() {
        let (tmp, ws) = ws();
        put(tmp.path(), "d/e/f.jpg", b"x");
        put(tmp.path(), "g/h.jpg", b"y");
        let batch = ws.new_batch_dir("20260101T000000Z").unwrap();
        ws.quarantine(&batch, "d/e/f.jpg").unwrap();
        ws.quarantine(&batch, "g/h.jpg").unwrap();
        ws.restore(&batch, "d/e/f.jpg").unwrap();
        ws.prune_empty_dirs(&batch);
        let q = tmp.path().join(&batch);
        assert!(!q.join("d").exists(), "the emptied folder is gone");
        assert!(q.join("g/h.jpg").exists(), "a folder holding a file is kept");
    }

    #[test]
    fn the_cross_volume_path_copies_verifies_then_removes_the_source() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a.jpg");
        let to = tmp.path().join("b.jpg");
        fs::write(&from, b"payload bytes").unwrap();
        copy_verify_delete(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(fs::read(&to).unwrap(), b"payload bytes");
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1, "no temporary copy left behind");
    }

    #[test]
    fn a_failed_cross_volume_copy_leaves_the_source_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a.jpg");
        fs::write(&from, b"payload").unwrap();
        // destination folder doesn't exist: the copy can't even start
        let to = tmp.path().join("missing-dir/b.jpg");
        assert!(copy_verify_delete(&from, &to).is_err());
        assert_eq!(fs::read(&from).unwrap(), b"payload");
    }
}
