//! Applies the user's decisions: files marked "quarantine" are moved into
//! `<root>/.photodedup-quarantine/<timestamp>/` (keeping their relative
//! paths), and a batch can be undone. Nothing is ever deleted or
//! overwritten, and nothing outside the scanned folder is ever touched --
//! both enforced by `fsguard`, which this module cannot bypass: it never
//! calls the filesystem to change anything itself.
//!
//! The flow, in order of how much it can hurt:
//! 1. `plan` -- read-only. What would move, and what was refused and why.
//! 2. `execute` -- re-checks every file against what was scanned, writes the
//!    whole plan to the journal *before* moving anything, then moves.
//! 3. `undo` -- puts a batch back, refusing to overwrite anything.
//! 4. `recover` -- after a crash, reconciles the journal with what is
//!    actually on disk.

use crate::fsguard::{GuardError, Workspace};
use crate::store::{Store, StoredBatch};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error(transparent)]
    Store(#[from] rusqlite::Error),
    #[error(transparent)]
    Guard(#[from] GuardError),
    #[error("no such scan")]
    NoSuchScan,
    #[error("no such batch")]
    NoSuchBatch,
    #[error("nothing to apply: no file is marked for quarantine")]
    NothingToApply,
    #[error("that batch has already been undone")]
    AlreadyUndone,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanItem {
    pub relative_path: String,
    pub size_bytes: u64,
    pub mtime_unix: i64,
    /// A companion (RAW, XMP, the video of a Live Photo) moving with a file
    /// the user marked.
    pub is_sibling: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Blocked {
    pub relative_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub root_path: String,
    pub items: Vec<PlanItem>,
    pub blocked: Vec<Blocked>,
    pub total_bytes: u64,
}

/// What a decision-driven apply would do, without touching anything.
///
/// Only files the user explicitly marked quarantine are candidates;
/// undecided files and anything marked keep are never planned. Two refusals
/// protect the user's last copies: a group of look-alikes may not lose every
/// member (the UI prevents this, but the engine does not trust the UI), and
/// cloud-only placeholders are left alone.
pub fn plan(store: &Store, scan_id: &str) -> Result<Plan, ApplyError> {
    let scan = store.get_scan(scan_id)?.ok_or(ApplyError::NoSuchScan)?;
    let ws = Workspace::open(Path::new(&scan.root_path))?;

    let files = store.load_files(scan_id)?;
    let by_id: HashMap<&str, &crate::store::StoredFile> = files.iter().map(|f| (f.id.as_str(), f)).collect();
    let decisions = store.load_decisions(scan_id)?;
    // Groups of two or more: duplicate clusters, sequences, look-alike pairs.
    let clusters = store.load_clusters(scan_id)?;
    let runs = store.load_burst_runs(scan_id)?;
    let pairs = store.load_review_pairs(scan_id)?;
    let orphans = store.load_orphan_videos(scan_id)?;

    // A choice only counts for a file that is still in some group. After the
    // matching settings change, a photo can stop being a duplicate of
    // anything; a "quarantine" chosen back when it was must not move it now.
    let grouped: HashSet<&str> = clusters
        .iter()
        .flat_map(|c| c.member_file_ids.iter().map(String::as_str))
        .chain(runs.iter().flat_map(|r| r.member_file_ids.iter().map(String::as_str)))
        .chain(pairs.iter().flat_map(|p| [p.file_a_id.as_str(), p.file_b_id.as_str()]))
        .chain(orphans.iter().map(|o| o.file_id.as_str()))
        .collect();
    let quarantine: HashSet<&str> = files
        .iter()
        .filter(|f| decisions.get(&f.relative_path) == Some(&false) && grouped.contains(f.id.as_str()))
        .map(|f| f.id.as_str())
        .collect();

    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for c in clusters {
        groups.push(("duplicate group".into(), c.member_file_ids));
    }
    for r in runs {
        groups.push(("quick sequence".into(), r.member_file_ids));
    }
    for p in pairs.into_iter().filter(|p| p.tier != "T2Burst") {
        groups.push(("pair of look-alikes".into(), vec![p.file_a_id, p.file_b_id]));
    }

    let mut blocked: Vec<Blocked> = Vec::new();
    let mut blocked_ids: HashSet<&str> = HashSet::new();
    for (kind, members) in &groups {
        if members.len() >= 2 && members.iter().all(|m| quarantine.contains(m.as_str())) {
            for m in members {
                if blocked_ids.insert(m.as_str()) {
                    if let Some(f) = by_id.get(m.as_str()) {
                        blocked.push(Blocked {
                            relative_path: f.relative_path.clone(),
                            reason: format!("would leave no copy in a {kind}"),
                        });
                    }
                }
            }
        }
    }

    let siblings = store.load_siblings(scan_id)?;
    let mut items: Vec<PlanItem> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |f: &crate::store::StoredFile, is_sibling: bool, items: &mut Vec<PlanItem>, blocked: &mut Vec<Blocked>| {
        if !seen.insert(f.id.clone()) {
            return;
        }
        if f.is_placeholder {
            blocked.push(Blocked { relative_path: f.relative_path.clone(), reason: "cloud-only file (not downloaded); left alone".into() });
        } else if let Err(e) = ws.validate_relative(&f.relative_path) {
            blocked.push(Blocked { relative_path: f.relative_path.clone(), reason: e.to_string() });
        } else {
            items.push(PlanItem { relative_path: f.relative_path.clone(), size_bytes: f.size_bytes, mtime_unix: f.mtime_unix, is_sibling });
        }
    };
    for f in &files {
        if !quarantine.contains(f.id.as_str()) || blocked_ids.contains(f.id.as_str()) {
            continue;
        }
        add(f, false, &mut items, &mut blocked);
        for sib in siblings.get(&f.id).into_iter().flatten() {
            if let Some(sf) = by_id.get(sib.as_str()) {
                add(sf, true, &mut items, &mut blocked);
            }
        }
    }

    let total_bytes = items.iter().map(|i| i.size_bytes).sum();
    Ok(Plan { root_path: ws.root().to_string_lossy().into_owned(), items, blocked, total_bytes })
}

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub relative_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecuteResult {
    /// `None` when nothing could be moved, in which case no batch exists.
    pub batch_id: Option<String>,
    pub moved: usize,
    pub moved_bytes: u64,
    pub skipped: Vec<Skipped>,
    pub blocked: Vec<Blocked>,
}

/// Re-checks one file against what the scan saw. A file that changed (or
/// vanished, or became a link) since the scan is not the file the user
/// reviewed, so it is left alone.
fn still_as_scanned(ws: &Workspace, item: &PlanItem) -> Result<(), String> {
    let path = ws.existing_file(&item.relative_path).map_err(|e| match e {
        GuardError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => "no longer there".to_string(),
        other => other.to_string(),
    })?;
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if meta.len() != item.size_bytes || mtime != item.mtime_unix {
        return Err("changed since it was scanned".into());
    }
    Ok(())
}

/// Moves the planned files into a new quarantine batch.
///
/// Order matters for crash safety: files are checked, the *entire* batch is
/// written to the journal, and only then do files move one at a time, each
/// marked `moved` as it lands. A crash at any point leaves a journal that
/// `recover` can reconcile against the disk.
pub fn execute(store: &Store, scan_id: &str) -> Result<ExecuteResult, ApplyError> {
    let plan = plan(store, scan_id)?;
    if plan.items.is_empty() && plan.blocked.is_empty() {
        return Err(ApplyError::NothingToApply);
    }
    let ws = Workspace::open(Path::new(&plan.root_path))?;

    let mut skipped = Vec::new();
    let mut ready: Vec<&PlanItem> = Vec::new();
    for item in &plan.items {
        match still_as_scanned(&ws, item) {
            Ok(()) => ready.push(item),
            Err(reason) => skipped.push(Skipped { relative_path: item.relative_path.clone(), reason }),
        }
    }
    if ready.is_empty() {
        return Ok(ExecuteResult { batch_id: None, moved: 0, moved_bytes: 0, skipped, blocked: plan.blocked });
    }

    let batch_dir = ws.new_batch_dir(&chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string())?;
    let batch_id = uuid::Uuid::new_v4().to_string();
    let journal: Vec<(String, u64, i64, bool)> =
        ready.iter().map(|i| (i.relative_path.clone(), i.size_bytes, i.mtime_unix, i.is_sibling)).collect();
    store.create_batch(&batch_id, &plan.root_path, &batch_dir, &journal)?;

    let (mut moved, mut moved_bytes) = (0usize, 0u64);
    for (seq, item) in ready.iter().enumerate() {
        // Checked again immediately before the move, not just in the pass above.
        let outcome = still_as_scanned(&ws, item).and_then(|()| ws.quarantine(&batch_dir, &item.relative_path).map_err(|e| e.to_string()));
        match outcome {
            Ok(()) => {
                store.set_item_state(&batch_id, seq as i64, "moved", None)?;
                moved += 1;
                moved_bytes += item.size_bytes;
            }
            Err(reason) => {
                store.set_item_state(&batch_id, seq as i64, "skipped", Some(&reason))?;
                skipped.push(Skipped { relative_path: item.relative_path.clone(), reason });
            }
        }
    }

    write_manifests(store, &ws, &batch_id)?;
    store.set_batch_state(&batch_id, if moved == 0 { "undone" } else { "done" })?;
    Ok(ExecuteResult { batch_id: Some(batch_id), moved, moved_bytes, skipped, blocked: plan.blocked })
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Writes `manifest.csv` and `manifest.json` into the batch folder, so a
/// batch can be understood (and undone by hand) without this program.
fn write_manifests(store: &Store, ws: &Workspace, batch_id: &str) -> Result<(), ApplyError> {
    let batch = store.get_batch(batch_id)?.ok_or(ApplyError::NoSuchBatch)?;
    let moved: Vec<_> = store.load_batch_items(batch_id)?.into_iter().filter(|i| i.state == "moved").collect();

    let mut csv = String::from("original_path,quarantine_path,size_bytes,modified_unix,kind\n");
    let mut json = Vec::new();
    for i in &moved {
        let q = format!("{}/{}", batch.quarantine_dir, i.relative_path);
        let kind = if i.is_sibling { "companion" } else { "marked" };
        csv.push_str(&format!("{},{},{},{},{}\n", csv_field(&i.relative_path), csv_field(&q), i.size_bytes, i.mtime_unix, kind));
        json.push(serde_json::json!({
            "original_path": i.relative_path, "quarantine_path": q, "size_bytes": i.size_bytes,
            "modified_unix": i.mtime_unix, "kind": kind,
        }));
    }
    let doc = serde_json::json!({ "batch_id": batch.id, "created_at": batch.created_at, "root": batch.root_path, "files": json });
    // A manifest already there (recovery re-running) is left as it is.
    let _ = ws.write_in_batch(&batch.quarantine_dir, "manifest.csv", csv.as_bytes());
    let _ = ws.write_in_batch(&batch.quarantine_dir, "manifest.json", serde_json::to_string_pretty(&doc).unwrap_or_default().as_bytes());
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct UndoResult {
    pub restored: usize,
    /// Files left in quarantine, and why. Re-running the undo retries them.
    pub skipped: Vec<Skipped>,
}

/// Puts a batch back where it came from. Never overwrites: if something now
/// sits at a file's original path, that file stays in quarantine and is
/// reported. Works after a re-scan, because batches don't depend on scans.
pub fn undo(store: &Store, batch_id: &str) -> Result<UndoResult, ApplyError> {
    let mut batch = store.get_batch(batch_id)?.ok_or(ApplyError::NoSuchBatch)?;
    if batch.state == "running" {
        recover_batch(store, &batch)?;
        batch = store.get_batch(batch_id)?.ok_or(ApplyError::NoSuchBatch)?;
    }
    if batch.state == "undone" {
        return Err(ApplyError::AlreadyUndone);
    }
    let ws = Workspace::open(Path::new(&batch.root_path))?;

    let mut restored = 0;
    let mut skipped = Vec::new();
    let items = store.load_batch_items(batch_id)?;
    for item in items.iter().rev().filter(|i| i.state == "moved") {
        let result = if !ws.quarantined_exists(&batch.quarantine_dir, &item.relative_path) {
            Err("not found in quarantine".to_string())
        } else if ws.occupied(&item.relative_path) {
            Err("something else is at the original location; left in quarantine".to_string())
        } else {
            ws.restore(&batch.quarantine_dir, &item.relative_path).map_err(|e| e.to_string())
        };
        match result {
            Ok(()) => {
                store.set_item_state(batch_id, item.seq, "restored", None)?;
                restored += 1;
            }
            Err(reason) => {
                store.set_item_state(batch_id, item.seq, "moved", Some(&reason))?;
                skipped.push(Skipped { relative_path: item.relative_path.clone(), reason });
            }
        }
    }
    ws.prune_empty_dirs(&batch.quarantine_dir);
    let still_in_quarantine = store.load_batch_items(batch_id)?.iter().any(|i| i.state == "moved");
    store.set_batch_state(batch_id, if still_in_quarantine { "partially_undone" } else { "undone" })?;
    Ok(UndoResult { restored, skipped })
}

/// Reconciles one interrupted batch with the disk: an item whose file is in
/// quarantine and gone from its place was moved; one still in its place was
/// not; anything else is flagged for a human rather than guessed at.
fn recover_batch(store: &Store, batch: &StoredBatch) -> Result<(), ApplyError> {
    let Ok(ws) = Workspace::open(Path::new(&batch.root_path)) else { return Ok(()) };
    for item in store.load_batch_items(&batch.id)?.into_iter().filter(|i| i.state == "planned") {
        let in_quarantine = ws.quarantined_exists(&batch.quarantine_dir, &item.relative_path);
        let in_place = ws.occupied(&item.relative_path);
        match (in_quarantine, in_place) {
            (true, false) => store.set_item_state(&batch.id, item.seq, "moved", Some("recovered after an interruption"))?,
            (false, true) => store.set_item_state(&batch.id, item.seq, "skipped", Some("not moved (interrupted)"))?,
            _ => store.set_item_state(&batch.id, item.seq, "skipped", Some("interrupted; needs a manual look"))?,
        }
    }
    write_manifests(store, &ws, &batch.id)?;
    store.set_batch_state(&batch.id, "done")?;
    Ok(())
}

/// Call once at startup: finishes off any batch a crash left `running`.
/// Returns how many were reconciled.
pub fn recover(store: &Store) -> Result<usize, ApplyError> {
    let mut n = 0;
    for b in store.load_batches()?.into_iter().filter(|b| b.state == "running") {
        recover_batch(store, &b)?;
        n += 1;
    }
    Ok(n)
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchSummary {
    pub id: String,
    pub created_at: String,
    pub state: String,
    pub quarantine_dir: String,
    pub root_path: String,
    /// Files currently in quarantine from this batch.
    pub in_quarantine: usize,
    pub in_quarantine_bytes: u64,
    pub restored: usize,
}

pub fn list_batches(store: &Store) -> Result<Vec<BatchSummary>, ApplyError> {
    let mut out = Vec::new();
    for b in store.load_batches()? {
        let items = store.load_batch_items(&b.id)?;
        let held: Vec<_> = items.iter().filter(|i| i.state == "moved").collect();
        out.push(BatchSummary {
            id: b.id,
            created_at: b.created_at,
            state: b.state,
            quarantine_dir: b.quarantine_dir,
            root_path: b.root_path,
            in_quarantine: held.len(),
            in_quarantine_bytes: held.iter().map(|i| i.size_bytes).sum(),
            restored: items.iter().filter(|i| i.state == "restored").count(),
        });
    }
    Ok(out)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::fsguard::QUARANTINE_DIR_NAME;
    use crate::model::{FileRecord, MediaClassSerde, SiblingSet};
    use crate::pipeline::{Cluster, PipelineResult};
    use std::collections::BTreeMap;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    struct Fx {
        tmp: tempfile::TempDir,
        root: PathBuf,
        store: Store,
        scan: String,
    }

    /// Real files on disk, scanned into an in-memory store. `clusters` lists
    /// groups by relative path (first is the keeper); `siblings` maps a
    /// primary to its companions.
    fn fixture(files: &[(&str, &[u8])], clusters: &[&[&str]], siblings: &[(&str, &[&str])]) -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let mut records = Vec::new();
        let mut id_of: BTreeMap<&str, u32> = BTreeMap::new();
        for (i, (rel, body)) in files.iter().enumerate() {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, body).unwrap();
            let meta = fs::metadata(&p).unwrap();
            id_of.insert(rel, i as u32);
            records.push(FileRecord {
                id: i as u32,
                path: p.clone(),
                file_name: p.file_name().unwrap().to_string_lossy().into(),
                media_class: MediaClassSerde::Image,
                size_bytes: meta.len(),
                mtime_unix: meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
                is_placeholder: false,
            });
        }
        let result = PipelineResult {
            files: records,
            analyzed: Default::default(),
            clusters: clusters
                .iter()
                .map(|g| Cluster { tier: "T1", members: g.iter().map(|r| id_of[r]).collect(), keeper: id_of[g[0]], reasons: vec![] })
                .collect(),
            review_pairs: vec![],
            burst_runs: vec![],
            video_matches: vec![],
            exact_edges: vec![],
            video_edges: vec![],
            orphan_videos: vec![],
            unknown_provenance_videos: vec![],
            placeholder_count: 0,
            placeholder_bytes: 0,
            refused_managed_libraries: vec![],
            sibling_sets: siblings
                .iter()
                .map(|(p, sibs)| SiblingSet { primary: id_of[p], siblings: sibs.iter().map(|s| id_of[s]).collect() })
                .collect(),
        };
        let mut store = Store::open_in_memory().unwrap();
        let scan = store.save_scan(&root, &result).unwrap();
        Fx { tmp, root, store, scan }
    }

    impl Fx {
        fn decide(&self, rel: &str, keep: bool) {
            self.store.record_decision(&self.scan, rel, keep).unwrap();
        }
        /// Every file under the root except the quarantine folder:
        /// relative path -> (bytes, mtime).
        fn tree(&self) -> BTreeMap<String, (Vec<u8>, std::time::SystemTime)> {
            fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, (Vec<u8>, std::time::SystemTime)>) {
                for e in fs::read_dir(dir).unwrap().flatten() {
                    let p = e.path();
                    if p.file_name().unwrap() == QUARANTINE_DIR_NAME {
                        continue;
                    }
                    if p.is_dir() {
                        walk(&p, root, out);
                    } else {
                        out.insert(
                            p.strip_prefix(root).unwrap().to_string_lossy().into(),
                            (fs::read(&p).unwrap(), fs::metadata(&p).unwrap().modified().unwrap()),
                        );
                    }
                }
            }
            let mut out = BTreeMap::new();
            walk(&self.root, &self.root, &mut out);
            out
        }
        fn exists(&self, rel: &str) -> bool {
            self.root.join(rel).exists()
        }
    }

    const BIG: &[u8] = b"the original, larger file";
    const SMALL: &[u8] = b"copy";

    #[test]
    fn apply_then_undo_returns_the_folder_to_exactly_what_it_was() {
        let fx = fixture(
            &[("a/keep1.jpg", BIG), ("a/dup1.jpg", SMALL), ("b/keep2.jpg", BIG), ("b/sub/dup2.jpg", SMALL), ("solo.jpg", b"alone")],
            &[&["a/keep1.jpg", "a/dup1.jpg"], &["b/keep2.jpg", "b/sub/dup2.jpg"]],
            &[],
        );
        let before = fx.tree();
        fx.decide("a/dup1.jpg", false);
        fx.decide("b/sub/dup2.jpg", false);
        fx.decide("a/keep1.jpg", true);

        let result = execute(&fx.store, &fx.scan).unwrap();
        assert_eq!(result.moved, 2);
        assert!(!fx.exists("a/dup1.jpg") && !fx.exists("b/sub/dup2.jpg"));
        assert!(fx.exists("a/keep1.jpg") && fx.exists("b/keep2.jpg") && fx.exists("solo.jpg"), "only the marked files moved");

        let batch = fx.store.get_batch(result.batch_id.as_ref().unwrap()).unwrap().unwrap();
        assert_eq!(batch.state, "done");
        let qdir = fx.root.join(&batch.quarantine_dir);
        assert_eq!(fs::read(qdir.join("a/dup1.jpg")).unwrap(), SMALL, "layout is preserved inside the batch");
        assert!(qdir.join("manifest.csv").exists() && qdir.join("manifest.json").exists());
        let csv = fs::read_to_string(qdir.join("manifest.csv")).unwrap();
        assert!(csv.contains("a/dup1.jpg") && csv.contains("b/sub/dup2.jpg"));

        let undone = undo(&fx.store, result.batch_id.as_ref().unwrap()).unwrap();
        assert_eq!(undone.restored, 2);
        assert!(undone.skipped.is_empty());
        assert_eq!(fx.tree(), before, "byte-identical, same modification times, same layout");
        assert_eq!(fx.store.get_batch(&batch.id).unwrap().unwrap().state, "undone");
    }

    #[test]
    fn undecided_files_and_files_marked_keep_are_never_planned() {
        let fx = fixture(&[("k.jpg", BIG), ("d.jpg", SMALL), ("u.jpg", SMALL)], &[&["k.jpg", "d.jpg", "u.jpg"]], &[]);
        fx.decide("k.jpg", true);
        fx.decide("d.jpg", false);
        let p = plan(&fx.store, &fx.scan).unwrap();
        assert_eq!(p.items.iter().map(|i| i.relative_path.as_str()).collect::<Vec<_>>(), ["d.jpg"]);
        assert!(p.blocked.is_empty());
    }

    #[test]
    fn nothing_marked_means_nothing_to_apply() {
        let fx = fixture(&[("a.jpg", BIG), ("b.jpg", SMALL)], &[&["a.jpg", "b.jpg"]], &[]);
        assert!(matches!(execute(&fx.store, &fx.scan), Err(ApplyError::NothingToApply)));
        assert!(fx.store.load_batches().unwrap().is_empty());
    }

    #[test]
    fn companions_move_and_return_with_their_photo() {
        let fx = fixture(
            &[("p/k.jpg", BIG), ("p/d.jpg", SMALL), ("p/d.xmp", b"sidecar"), ("p/d.raw", b"rawdata")],
            &[&["p/k.jpg", "p/d.jpg"]],
            &[("p/d.jpg", &["p/d.xmp", "p/d.raw"])],
        );
        let before = fx.tree();
        fx.decide("p/d.jpg", false);

        let p = plan(&fx.store, &fx.scan).unwrap();
        assert_eq!(p.items.len(), 3);
        assert_eq!(p.items.iter().filter(|i| i.is_sibling).count(), 2);

        let r = execute(&fx.store, &fx.scan).unwrap();
        assert_eq!(r.moved, 3);
        assert!(fx.exists("p/k.jpg") && !fx.exists("p/d.xmp") && !fx.exists("p/d.raw"), "no orphaned companion is left behind");

        undo(&fx.store, r.batch_id.as_ref().unwrap()).unwrap();
        assert_eq!(fx.tree(), before);
    }

    #[test]
    fn the_engine_refuses_to_quarantine_every_member_of_a_group_even_if_asked() {
        let fx = fixture(&[("a.jpg", BIG), ("b.jpg", SMALL), ("c.jpg", SMALL), ("d.jpg", SMALL)], &[&["a.jpg", "b.jpg"], &["c.jpg", "d.jpg"]], &[]);
        // a bypassed UI: both members of the first group marked, one of the second
        fx.decide("a.jpg", false);
        fx.decide("b.jpg", false);
        fx.decide("d.jpg", false);

        let r = execute(&fx.store, &fx.scan).unwrap();
        assert_eq!(r.moved, 1);
        assert!(fx.exists("a.jpg") && fx.exists("b.jpg"), "the whole group was left alone");
        assert!(!fx.exists("d.jpg"));
        let mut names: Vec<_> = r.blocked.iter().map(|b| b.relative_path.as_str()).collect();
        names.sort();
        assert_eq!(names, ["a.jpg", "b.jpg"]);
    }

    #[test]
    fn a_quarantine_choice_for_a_file_no_longer_in_any_group_moves_nothing() {
        // `stray.jpg` was marked back when it was grouped; after the matching
        // settings changed it belongs to no group, so the choice is void.
        let fx = fixture(&[("k.jpg", BIG), ("dup.jpg", SMALL), ("stray.jpg", SMALL)], &[&["k.jpg", "dup.jpg"]], &[]);
        fx.decide("dup.jpg", false);
        fx.decide("stray.jpg", false);

        let r = execute(&fx.store, &fx.scan).unwrap();
        assert_eq!(r.moved, 1);
        assert!(!fx.exists("dup.jpg"));
        assert!(fx.exists("stray.jpg"), "a photo that is no longer a duplicate of anything stays put");
    }

    #[test]
    fn a_file_that_changed_since_the_scan_is_skipped_not_moved() {
        let fx = fixture(&[("k.jpg", BIG), ("changed.jpg", SMALL), ("fine.jpg", SMALL)], &[&["k.jpg", "changed.jpg", "fine.jpg"]], &[]);
        fx.decide("changed.jpg", false);
        fx.decide("fine.jpg", false);
        fs::write(fx.root.join("changed.jpg"), b"edited after the scan, so longer").unwrap();

        let r = execute(&fx.store, &fx.scan).unwrap();
        assert_eq!(r.moved, 1);
        assert!(fx.exists("changed.jpg"), "the edited file was left where it was");
        assert!(r.skipped.iter().any(|s| s.relative_path == "changed.jpg" && s.reason.contains("changed")));
        assert_eq!(fs::read(fx.root.join("changed.jpg")).unwrap(), b"edited after the scan, so longer");
    }

    #[test]
    fn a_vanished_file_is_reported_and_creates_no_batch_if_nothing_else_moves() {
        let fx = fixture(&[("k.jpg", BIG), ("gone.jpg", SMALL)], &[&["k.jpg", "gone.jpg"]], &[]);
        fx.decide("gone.jpg", false);
        fs::remove_file(fx.root.join("gone.jpg")).unwrap();
        let r = execute(&fx.store, &fx.scan).unwrap();
        assert_eq!(r.moved, 0);
        assert!(r.batch_id.is_none());
        assert_eq!(r.skipped.len(), 1);
        assert!(fx.store.load_batches().unwrap().is_empty());
    }

    #[test]
    fn applying_twice_does_not_move_anything_twice() {
        let fx = fixture(&[("k.jpg", BIG), ("d.jpg", SMALL)], &[&["k.jpg", "d.jpg"]], &[]);
        fx.decide("d.jpg", false);
        assert_eq!(execute(&fx.store, &fx.scan).unwrap().moved, 1);
        let again = execute(&fx.store, &fx.scan).unwrap();
        assert_eq!(again.moved, 0);
        assert_eq!(fx.store.load_batches().unwrap().len(), 1, "no empty second batch");
    }

    #[test]
    fn a_tampered_path_in_the_scan_data_cannot_reach_outside_the_folder() {
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("precious.jpg");
        fs::write(&victim, b"not yours to move").unwrap();

        let fx = fixture(&[("k.jpg", BIG), ("d.jpg", SMALL)], &[&["k.jpg", "d.jpg"]], &[]);
        // rewrite a stored path so that it climbs out of the root
        fx.store.exec_for_test("UPDATE files SET relative_path = '../../precious.jpg' WHERE relative_path = 'd.jpg'");
        fx.decide("../../precious.jpg", false);

        let p = plan(&fx.store, &fx.scan).unwrap();
        assert!(p.items.is_empty());
        assert!(p.blocked.iter().any(|b| b.reason.contains("refused")));
        assert_eq!(fs::read(&victim).unwrap(), b"not yours to move");
    }

    #[test]
    #[cfg(unix)] // creating a symlink needs a privilege on Windows
    fn a_folder_swapped_for_a_symlink_after_the_scan_is_not_followed() {
        let fx = fixture(&[("k.jpg", BIG), ("sub/d.jpg", SMALL)], &[&["k.jpg", "sub/d.jpg"]], &[]);
        fx.decide("sub/d.jpg", false);

        // After the scan, `sub` becomes a link to a folder elsewhere that holds
        // a file with the SAME name, size and modification time -- so the
        // "changed since the scan" check can't be what saves it; only the
        // refusal to follow links can.
        let outside = tempfile::tempdir().unwrap();
        let recorded_mtime = fs::metadata(fx.root.join("sub/d.jpg")).unwrap().modified().unwrap();
        fs::write(outside.path().join("d.jpg"), SMALL).unwrap();
        fs::File::options().write(true).open(outside.path().join("d.jpg")).unwrap().set_modified(recorded_mtime).unwrap();
        fs::remove_dir_all(fx.root.join("sub")).unwrap();
        symlink(outside.path(), fx.root.join("sub")).unwrap();

        let r = execute(&fx.store, &fx.scan).unwrap();
        assert_eq!(r.moved, 0);
        assert!(r.skipped.iter().any(|s| s.reason.contains("symlink")), "{:?}", r.skipped);
        assert_eq!(fs::read(outside.path().join("d.jpg")).unwrap(), SMALL);
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1, "nothing was created or moved outside");
    }

    #[test]
    fn a_tampered_journal_cannot_make_undo_write_or_read_outside_the_folder() {
        let outside = tempfile::tempdir().unwrap();
        let bait = outside.path().join("bait.txt");
        fs::write(&bait, b"bait").unwrap();
        let fx = fixture(&[("k.jpg", BIG), ("d.jpg", SMALL)], &[&["k.jpg", "d.jpg"]], &[]);

        // a forged batch whose folder and item paths both point outside
        fx.store
            .create_batch("forged", fx.root.to_str().unwrap(), "../../elsewhere", &[("../../escape.txt".to_string(), 1, 0, false)])
            .unwrap();
        fx.store.set_item_state("forged", 0, "moved", None).unwrap();
        fx.store.set_batch_state("forged", "done").unwrap();

        let r = undo(&fx.store, "forged");
        assert!(r.is_err() || r.unwrap().restored == 0);
        assert!(!outside.path().join("escape.txt").exists());
        assert_eq!(fs::read(&bait).unwrap(), b"bait");
        assert!(!fx.root.parent().unwrap().join("escape.txt").exists());
    }

    #[test]
    fn undo_never_overwrites_and_can_be_retried() {
        let fx = fixture(&[("k.jpg", BIG), ("d.jpg", SMALL)], &[&["k.jpg", "d.jpg"]], &[]);
        fx.decide("d.jpg", false);
        let id = execute(&fx.store, &fx.scan).unwrap().batch_id.unwrap();

        fs::write(fx.root.join("d.jpg"), b"a different file saved there since").unwrap();
        let first = undo(&fx.store, &id).unwrap();
        assert_eq!(first.restored, 0);
        assert_eq!(first.skipped.len(), 1);
        assert_eq!(fs::read(fx.root.join("d.jpg")).unwrap(), b"a different file saved there since");
        assert_eq!(fx.store.get_batch(&id).unwrap().unwrap().state, "partially_undone");

        fs::remove_file(fx.root.join("d.jpg")).unwrap();
        assert_eq!(undo(&fx.store, &id).unwrap().restored, 1);
        assert_eq!(fs::read(fx.root.join("d.jpg")).unwrap(), SMALL);
        assert!(matches!(undo(&fx.store, &id), Err(ApplyError::AlreadyUndone)));
    }

    #[test]
    fn undo_still_works_after_the_scan_that_made_the_batch_is_replaced() {
        let mut fx = fixture(&[("k.jpg", BIG), ("d.jpg", SMALL)], &[&["k.jpg", "d.jpg"]], &[]);
        fx.decide("d.jpg", false);
        let id = execute(&fx.store, &fx.scan).unwrap().batch_id.unwrap();

        // a fresh scan replaces the old one wholesale
        let empty = PipelineResult {
            files: vec![],
            analyzed: Default::default(),
            clusters: vec![],
            review_pairs: vec![],
            burst_runs: vec![],
            video_matches: vec![],
            exact_edges: vec![],
            video_edges: vec![],
            orphan_videos: vec![],
            unknown_provenance_videos: vec![],
            placeholder_count: 0,
            placeholder_bytes: 0,
            refused_managed_libraries: vec![],
            sibling_sets: vec![],
        };
        fx.scan = fx.store.save_scan(&fx.root, &empty).unwrap();
        assert!(fx.store.get_batch(&id).unwrap().is_some(), "the journal survives a re-scan");
        assert_eq!(undo(&fx.store, &id).unwrap().restored, 1);
        assert!(fx.exists("d.jpg"));
    }

    #[test]
    fn a_crash_mid_apply_is_reconciled_from_what_is_actually_on_disk() {
        let fx = fixture(&[("k.jpg", BIG), ("d1.jpg", SMALL), ("d2.jpg", SMALL), ("d3.jpg", SMALL)], &[&["k.jpg", "d1.jpg", "d2.jpg", "d3.jpg"]], &[]);
        let ws = Workspace::open(&fx.root).unwrap();
        let batch_dir = ws.new_batch_dir("20260101T000000Z").unwrap();
        let items: Vec<_> = ["d1.jpg", "d2.jpg", "d3.jpg"]
            .iter()
            .map(|r| (r.to_string(), SMALL.len() as u64, 0i64, false))
            .collect();
        fx.store.create_batch("crashed", fx.root.to_str().unwrap(), &batch_dir, &items).unwrap();
        // the process died after moving d1, before even starting d2/d3, and before journalling d1's move
        ws.quarantine(&batch_dir, "d1.jpg").unwrap();

        assert_eq!(recover(&fx.store).unwrap(), 1);
        let by_path: BTreeMap<_, _> = fx.store.load_batch_items("crashed").unwrap().into_iter().map(|i| (i.relative_path, i.state)).collect();
        assert_eq!(by_path["d1.jpg"], "moved");
        assert_eq!(by_path["d2.jpg"], "skipped");
        assert_eq!(by_path["d3.jpg"], "skipped");
        assert!(fx.exists("d2.jpg") && fx.exists("d3.jpg") && !fx.exists("d1.jpg"));
        assert_eq!(fx.store.get_batch("crashed").unwrap().unwrap().state, "done");

        assert_eq!(undo(&fx.store, "crashed").unwrap().restored, 1);
        assert!(fx.exists("d1.jpg"));
        assert_eq!(recover(&fx.store).unwrap(), 0, "nothing left to reconcile");
    }

    #[test]
    fn files_in_cloud_only_state_are_left_alone() {
        let fx = fixture(&[("k.jpg", BIG), ("d.jpg", SMALL)], &[&["k.jpg", "d.jpg"]], &[]);
        fx.store.exec_for_test("UPDATE files SET is_placeholder = 1 WHERE relative_path = 'd.jpg'");
        fx.decide("d.jpg", false);
        let p = plan(&fx.store, &fx.scan).unwrap();
        assert!(p.items.is_empty());
        assert!(p.blocked[0].reason.contains("cloud-only"));
    }

    /// Random decisions over a random-ish tree: whatever is chosen, apply
    /// followed by undo must restore everything exactly.
    #[test]
    fn many_random_apply_and_undo_round_trips_always_restore_everything() {
        let mut seed: u64 = 0x9e3779b97f4a7c15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..20 {
            let n = 4 + (next() % 12) as usize;
            let names: Vec<String> = (0..n).map(|i| format!("d{}/sub{}/photo {i} ü.jpg", i % 3, i % 2)).collect();
            let bodies: Vec<Vec<u8>> = (0..n).map(|i| format!("body {round} {i} {}", next()).into_bytes()).collect();
            let files: Vec<(&str, &[u8])> = names.iter().zip(&bodies).map(|(a, b)| (a.as_str(), b.as_slice())).collect();
            let groups: Vec<Vec<&str>> = names.chunks(3).map(|c| c.iter().map(String::as_str).collect()).collect();
            let group_refs: Vec<&[&str]> = groups.iter().map(Vec::as_slice).collect();
            let fx = fixture(&files, &group_refs, &[]);
            let before = fx.tree();

            for name in &names {
                match next() % 3 {
                    0 => fx.decide(name, false),
                    1 => fx.decide(name, true),
                    _ => {}
                }
            }
            match execute(&fx.store, &fx.scan) {
                Ok(r) => {
                    // whatever moved, no group lost all its members
                    for g in &groups {
                        assert!(g.iter().any(|m| fx.exists(m)) || g.len() < 2, "round {round}: a whole group disappeared");
                    }
                    if let Some(id) = r.batch_id {
                        undo(&fx.store, &id).unwrap();
                    }
                }
                Err(ApplyError::NothingToApply) => {}
                Err(e) => panic!("round {round}: {e}"),
            }
            assert_eq!(fx.tree(), before, "round {round}: the folder is not back to how it started");
        }
    }

    #[test]
    fn nothing_outside_the_root_is_touched_by_a_whole_apply_and_undo() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("neighbour.jpg"), b"next door").unwrap();
        let before = fs::read_dir(outside.path()).unwrap().count();

        // What is in the shared temp folder (the root's parent) before we start.
        let parent_names = |p: &Path| -> std::collections::BTreeSet<String> {
            fs::read_dir(p).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
        };
        let fx = fixture(&[("k.jpg", BIG), ("d.jpg", SMALL)], &[&["k.jpg", "d.jpg"]], &[]);
        let parent = fx.root.parent().unwrap().to_path_buf();
        let known = parent_names(&parent);

        fx.decide("d.jpg", false);
        let id = execute(&fx.store, &fx.scan).unwrap().batch_id.unwrap();
        undo(&fx.store, &id).unwrap();

        // Whatever appeared beside the root *during this test* and looks like ours.
        // (Not "anything named photodedup": other programs may leave such things
        // in a shared temp folder, and parallel tests make their own temp dirs.)
        let added: Vec<String> = parent_names(&parent)
            .difference(&known)
            .filter(|n| n.to_lowercase().contains("photodedup"))
            .cloned()
            .collect();
        assert!(added.is_empty(), "stray files beside the root: {added:?}");
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), before);
        drop(fx.tmp);
    }
}
