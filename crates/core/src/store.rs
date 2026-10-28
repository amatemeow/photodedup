//! SQLite persistence. Normalizes a `pipeline::PipelineResult` into tables
//! keyed on relative path (not the pipeline's ephemeral per-run `FileId`,
//! which is just a scan-local index) so results survive across runs and
//! support resume/incremental rescan later (see `/ROADMAP.md`'s
//! "Incremental rescan" row -- the `size`/`mtime` columns exist from day
//! one specifically so that hook is cheap to add without a schema change).

use crate::classify::Tier;
use crate::model::{FileId, MediaClassSerde};
use crate::pipeline::{Analyzed, PipelineResult};
use crate::provenance::{Evidence, Origin};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Store {
    conn: Connection,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS scans (
    -- A random UUID, not a serial: an id is never reused, so a client
    -- holding the id of a replaced scan (a stale tab) can only get "not
    -- found", never land on a newer scan or write decisions into it.
    id TEXT PRIMARY KEY,
    root_path TEXT NOT NULL,
    started_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    id TEXT PRIMARY KEY,
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    file_name TEXT NOT NULL,
    media_class TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    mtime_unix INTEGER NOT NULL,
    is_placeholder INTEGER NOT NULL,
    UNIQUE(scan_id, relative_path)
);

CREATE TABLE IF NOT EXISTS analysis (
    file_id TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    origin TEXT NOT NULL,
    confidence REAL NOT NULL,
    evidence_json TEXT NOT NULL,
    phash BLOB,
    sharpness REAL,
    width INTEGER,
    height INTEGER,
    camera_model TEXT,
    capture_time TEXT,
    duration_seconds REAL
);

CREATE TABLE IF NOT EXISTS clusters (
    id TEXT PRIMARY KEY,
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    tier TEXT NOT NULL,
    keeper_file_id TEXT NOT NULL REFERENCES files(id),
    reasons_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS cluster_members (
    cluster_id TEXT NOT NULL REFERENCES clusters(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id),
    PRIMARY KEY (cluster_id, file_id)
);

CREATE TABLE IF NOT EXISTS burst_runs (
    id TEXT PRIMARY KEY,
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    camera_model TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS burst_run_members (
    burst_run_id TEXT NOT NULL REFERENCES burst_runs(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id),
    seq INTEGER NOT NULL,
    PRIMARY KEY (burst_run_id, file_id)
);

-- Every id below is a random UUID (see `scans`): never reused, never
-- mistakable for another row. Lists are read back in insertion order
-- (`ORDER BY rowid`), since UUIDs carry no order.
--
-- Review-only pairs (T2Burst/T2NativeReview/T3CrossDevice/etc. -- never
-- auto-selectable, see pipeline::PipelineResult::review_pairs). Was
-- computed but never actually persisted before M2 needed to serve it to
-- the "same-device review" UI tab.
CREATE TABLE IF NOT EXISTS review_pairs (
    id TEXT PRIMARY KEY,
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    tier TEXT NOT NULL,
    file_a_id TEXT NOT NULL REFERENCES files(id),
    file_b_id TEXT NOT NULL REFERENCES files(id),
    phash_dist INTEGER NOT NULL,
    reason TEXT NOT NULL
);

-- App-level preferences (key/value). Not tied to a scan: they outlive every
-- scan, and belong to whoever owns this database rather than to a browser.
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- The matching settings a scan was made with, so the review can say so and
-- notice when the saved settings have since moved on. One row per scan.
CREATE TABLE IF NOT EXISTS scan_settings (
    scan_id TEXT PRIMARY KEY REFERENCES scans(id) ON DELETE CASCADE,
    match_distance INTEGER NOT NULL,
    sequence_similarity INTEGER NOT NULL,
    sequence_gap_seconds INTEGER NOT NULL,
    include_videos INTEGER NOT NULL
);

-- Companions of a primary file (RAW, XMP, the MOV of a Live Photo): they
-- move with it when a decision is applied.
CREATE TABLE IF NOT EXISTS file_siblings (
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    primary_file_id TEXT NOT NULL REFERENCES files(id),
    sibling_file_id TEXT NOT NULL REFERENCES files(id),
    PRIMARY KEY (primary_file_id, sibling_file_id)
);

-- The apply journal. Deliberately NOT tied to a scan: a scan is replaced by
-- the next one, but a batch must stay undoable for as long as its files sit
-- in quarantine. Rows are written BEFORE any file moves (write-ahead), so a
-- crash mid-apply can be reconciled from what is actually on disk.
CREATE TABLE IF NOT EXISTS apply_batches (
    id TEXT PRIMARY KEY,
    root_path TEXT NOT NULL,
    -- `.photodedup-quarantine/<timestamp>`, relative to root_path
    quarantine_dir TEXT NOT NULL,
    created_at TEXT NOT NULL,
    -- running | done | partially_undone | undone
    state TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS apply_items (
    batch_id TEXT NOT NULL REFERENCES apply_batches(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    relative_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    mtime_unix INTEGER NOT NULL,
    is_sibling INTEGER NOT NULL,
    -- planned | moved | skipped | restored
    state TEXT NOT NULL,
    note TEXT,
    PRIMARY KEY (batch_id, seq)
);

-- Videos with no counterpart, shown in their own tab in their own tab: `no_match` (a derived video
-- with no native original found) or `unknown_origin` (provenance
-- couldn't be established, so it was never a match candidate).
CREATE TABLE IF NOT EXISTS orphan_videos (
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id),
    reason TEXT NOT NULL,
    PRIMARY KEY (scan_id, file_id)
);

-- User decisions: deliberately separate from `clusters` (which is just the
-- last scan's machine-generated suggestion) so a re-scan can't silently
-- discard what a user already decided. Keyed on relative_path, not
-- file_id, so it survives a fresh scan assigning new file ids.
CREATE TABLE IF NOT EXISTS decisions (
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    keep INTEGER NOT NULL,
    decided_at TEXT NOT NULL,
    PRIMARY KEY (scan_id, relative_path)
);
"#;

#[derive(Debug, Clone)]
pub struct StoredFile {
    pub id: String,
    pub relative_path: String,
    pub file_name: String,
    pub media_class: MediaClassSerde,
    pub size_bytes: u64,
    pub mtime_unix: i64,
    pub is_placeholder: bool,
}

#[derive(Debug, Clone)]
pub struct StoredCluster {
    pub id: String,
    pub tier: String,
    pub keeper_file_id: String,
    pub member_file_ids: Vec<String>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StoredReviewPair {
    pub id: String,
    pub tier: String,
    pub file_a_id: String,
    pub file_b_id: String,
    pub phash_dist: i64,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct StoredBurstRun {
    pub id: String,
    pub camera_model: String,
    /// In capture-time order (the `seq` column), not file-id order.
    pub member_file_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StoredBatch {
    pub id: String,
    pub root_path: String,
    pub quarantine_dir: String,
    pub created_at: String,
    pub state: String,
}

#[derive(Debug, Clone)]
pub struct StoredBatchItem {
    pub seq: i64,
    pub relative_path: String,
    pub size_bytes: u64,
    pub mtime_unix: i64,
    pub is_sibling: bool,
    pub state: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StoredOrphanVideo {
    pub file_id: String,
    /// `no_match` or `unknown_origin`.
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct StoredAnalysis {
    pub origin: String,
    pub confidence: f64,
    pub evidence: Vec<Evidence>,
    pub sharpness: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub camera_model: Option<String>,
    pub capture_time: Option<String>,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ScanMeta {
    pub id: String,
    pub root_path: String,
    pub started_at: String,
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        drop_legacy_schema(&conn)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    /// Persists a full pipeline run as **the one live scan**. Returns the
    /// new `scan_id`.
    ///
    /// Every earlier scan -- files, analysis, clusters, review pairs,
    /// burst runs and decisions -- is deleted in the same transaction, so
    /// the old scan goes away only once the new one is fully written; a
    /// failure partway rolls back and leaves the old scan intact. Keeping
    /// several scans alive side by side is deferred (see ROADMAP.md).
    ///
    /// `root` is recorded for display; paths inside are stored *relative*
    /// to it so a scan is portable across machines/mount points.
    pub fn save_scan(&mut self, root: &Path, result: &PipelineResult) -> rusqlite::Result<String> {
        let tx = self.conn.transaction()?;
        let scan_id = uuid::Uuid::new_v4().to_string();
        let new_root = canonical_root_string(root);

        // Choices made on the scan about to be replaced, kept so a re-scan of
        // the same folder doesn't make the user redo their work. Read now,
        // before the old rows are deleted; applied only to files that have
        // not changed (see below).
        let carried: Vec<(String, bool, String, u64, i64)> = {
            let mut stmt = tx.prepare(
                "SELECT d.relative_path, d.keep, d.decided_at, f.size_bytes, f.mtime_unix
                 FROM decisions d
                 JOIN scans s ON s.id = d.scan_id
                 JOIN files f ON f.scan_id = d.scan_id AND f.relative_path = d.relative_path
                 WHERE s.root_path = ?1",
            )?;
            let rows = stmt.query_map(params![new_root], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0, r.get::<_, String>(2)?, r.get::<_, i64>(3)? as u64, r.get::<_, i64>(4)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };

        delete_all_scans(&tx)?;
        let now = chrono::Utc::now().to_rfc3339();

        tx.execute(
            "INSERT INTO scans (id, root_path, started_at) VALUES (?1, ?2, ?3)",
            params![scan_id, new_root, now],
        )?;

        // pipeline FileId (u32, scan-local, in-memory only) -> the file's
        // database id (a UUID).
        let mut id_map: std::collections::HashMap<FileId, String> = std::collections::HashMap::new();
        let new_id = || uuid::Uuid::new_v4().to_string();

        for f in &result.files {
            let rel = f.path.strip_prefix(root).unwrap_or(&f.path).to_string_lossy().to_string();
            let file_row = new_id();
            tx.execute(
                "INSERT INTO files (id, scan_id, relative_path, file_name, media_class, size_bytes, mtime_unix, is_placeholder)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    file_row,
                    scan_id,
                    rel,
                    f.file_name,
                    media_class_str(f.media_class),
                    f.size_bytes,
                    f.mtime_unix,
                    f.is_placeholder as i64,
                ],
            )?;
            id_map.insert(f.id, file_row);
        }

        for (file_id, a) in &result.analyzed {
            let Some(row_id) = id_map.get(file_id) else { continue };
            insert_analysis(&tx, row_id, a)?;
        }

        for c in &result.clusters {
            let Some(keeper_row) = id_map.get(&c.keeper) else { continue };
            let reasons_json = serde_json::to_string(&c.reasons).unwrap_or_default();
            let cluster_row = new_id();
            tx.execute(
                "INSERT INTO clusters (id, scan_id, tier, keeper_file_id, reasons_json) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![cluster_row, scan_id, c.tier, keeper_row, reasons_json],
            )?;
            for m in &c.members {
                if let Some(mrow) = id_map.get(m) {
                    tx.execute(
                        "INSERT OR IGNORE INTO cluster_members (cluster_id, file_id) VALUES (?1, ?2)",
                        params![cluster_row, mrow],
                    )?;
                }
            }
        }

        for run in &result.burst_runs {
            let run_row = new_id();
            tx.execute(
                "INSERT INTO burst_runs (id, scan_id, camera_model) VALUES (?1, ?2, ?3)",
                params![run_row, scan_id, run.camera_model],
            )?;
            for (seq, fid) in run.files.iter().enumerate() {
                if let Some(mrow) = id_map.get(fid) {
                    tx.execute(
                        "INSERT INTO burst_run_members (burst_run_id, file_id, seq) VALUES (?1, ?2, ?3)",
                        params![run_row, mrow, seq as i64],
                    )?;
                }
            }
        }

        for p in &result.review_pairs {
            let (Some(a_row), Some(b_row)) = (id_map.get(&p.a), id_map.get(&p.b)) else {
                continue;
            };
            tx.execute(
                "INSERT INTO review_pairs (id, scan_id, tier, file_a_id, file_b_id, phash_dist, reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![new_id(), scan_id, tier_str(p.tier), a_row, b_row, p.phash_dist, p.reason],
            )?;
        }

        // A choice is carried over only if the file is still there and looks
        // exactly as it did when the choice was made (same size and
        // modification time). A file that was edited or replaced since is a
        // different file now, and the choice was about the old one.
        let still_same: std::collections::HashMap<String, (u64, i64)> = result
            .files
            .iter()
            .map(|f| (f.path.strip_prefix(root).unwrap_or(&f.path).to_string_lossy().to_string(), (f.size_bytes, f.mtime_unix)))
            .collect();
        for (rel, keep, decided_at, size, mtime) in carried {
            if still_same.get(&rel) == Some(&(size, mtime)) {
                tx.execute(
                    "INSERT INTO decisions (scan_id, relative_path, keep, decided_at) VALUES (?1, ?2, ?3, ?4)",
                    params![scan_id, rel, keep as i64, decided_at],
                )?;
            }
        }

        for set in &result.sibling_sets {
            let Some(primary) = id_map.get(&set.primary) else { continue };
            for sib in &set.siblings {
                if let Some(sib_row) = id_map.get(sib) {
                    tx.execute(
                        "INSERT OR IGNORE INTO file_siblings (scan_id, primary_file_id, sibling_file_id) VALUES (?1, ?2, ?3)",
                        params![scan_id, primary, sib_row],
                    )?;
                }
            }
        }

        for (ids, reason) in [(&result.orphan_videos, "no_match"), (&result.unknown_provenance_videos, "unknown_origin")] {
            for fid in ids {
                if let Some(row) = id_map.get(fid) {
                    tx.execute(
                        "INSERT OR IGNORE INTO orphan_videos (scan_id, file_id, reason) VALUES (?1, ?2, ?3)",
                        params![scan_id, row, reason],
                    )?;
                }
            }
        }

        tx.commit()?;
        Ok(scan_id)
    }

    pub fn list_scans(&self) -> rusqlite::Result<Vec<ScanMeta>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, root_path, started_at FROM scans ORDER BY started_at DESC, rowid DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok(ScanMeta {
                id: row.get(0)?,
                root_path: row.get(1)?,
                started_at: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_scan(&self, scan_id: &str) -> rusqlite::Result<Option<ScanMeta>> {
        self.conn
            .query_row(
                "SELECT id, root_path, started_at FROM scans WHERE id = ?1",
                params![scan_id],
                |row| {
                    Ok(ScanMeta {
                        id: row.get(0)?,
                        root_path: row.get(1)?,
                        started_at: row.get(2)?,
                    })
                },
            )
            .optional()
    }

    /// The most recent scan of `root`, if one exists -- the lookup behind
    /// "resume where I left off": opening the app on a folder it
    /// recognizes should load that scan (and its decisions) instead of
    /// re-running the pipeline. Matches on the same canonicalized path
    /// `save_scan` stores, so a trailing slash or a path reached through a
    /// symlink still finds the same scan.
    pub fn find_latest_scan_for_root(&self, root: &Path) -> rusqlite::Result<Option<ScanMeta>> {
        let root_str = canonical_root_string(root);
        self.conn
            .query_row(
                "SELECT id, root_path, started_at FROM scans WHERE root_path = ?1 ORDER BY started_at DESC, rowid DESC LIMIT 1",
                params![root_str],
                |row| {
                    Ok(ScanMeta {
                        id: row.get(0)?,
                        root_path: row.get(1)?,
                        started_at: row.get(2)?,
                    })
                },
            )
            .optional()
    }

    /// All of one scan's recorded decisions at once, keyed by relative
    /// path -- what a reopened UI needs to restore "which files did I
    /// already mark keep/quarantine" in one round trip, rather than
    /// asking `get_decision` once per file.
    pub fn load_decisions(&self, scan_id: &str) -> rusqlite::Result<HashMap<String, bool>> {
        let mut stmt = self
            .conn
            .prepare("SELECT relative_path, keep FROM decisions WHERE scan_id = ?1")?;
        let rows = stmt.query_map(params![scan_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
        })?;
        rows.collect()
    }

    pub fn get_file(&self, file_id: &str) -> rusqlite::Result<Option<StoredFile>> {
        self.conn
            .query_row(
                "SELECT id, relative_path, file_name, media_class, size_bytes, mtime_unix, is_placeholder
                 FROM files WHERE id = ?1",
                params![file_id],
                |row| {
                    Ok(StoredFile {
                        id: row.get(0)?,
                        relative_path: row.get(1)?,
                        file_name: row.get(2)?,
                        media_class: media_class_from_str(&row.get::<_, String>(3)?),
                        size_bytes: row.get::<_, i64>(4)? as u64,
                        mtime_unix: row.get(5)?,
                        is_placeholder: row.get::<_, i64>(6)? != 0,
                    })
                },
            )
            .optional()
    }

    /// The real, absolute path to a file on disk -- joins the owning
    /// scan's recorded `root_path` with the file's stored *relative*
    /// path. This is the only sanctioned way a file id turns into a
    /// filesystem path; callers serving file content must go through
    /// this (never take a path from a request directly) -- see the
    /// architecture plan's server security section.
    pub fn get_file_full_path(&self, file_id: &str) -> rusqlite::Result<Option<PathBuf>> {
        self.conn
            .query_row(
                "SELECT s.root_path, f.relative_path FROM files f
                 JOIN scans s ON s.id = f.scan_id WHERE f.id = ?1",
                params![file_id],
                |row| {
                    let root: String = row.get(0)?;
                    let rel: String = row.get(1)?;
                    Ok(Path::new(&root).join(rel))
                },
            )
            .optional()
    }

    /// A file's scan root and its path relative to it -- the two halves
    /// `Workspace` needs to open it safely.
    pub fn get_file_location(&self, file_id: &str) -> rusqlite::Result<Option<(String, String)>> {
        self.conn
            .query_row(
                "SELECT s.root_path, f.relative_path FROM files f JOIN scans s ON s.id = f.scan_id WHERE f.id = ?1",
                params![file_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    pub fn load_review_pairs(&self, scan_id: &str) -> rusqlite::Result<Vec<StoredReviewPair>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tier, file_a_id, file_b_id, phash_dist, reason
             FROM review_pairs WHERE scan_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![scan_id], |row| {
            Ok(StoredReviewPair {
                id: row.get(0)?,
                tier: row.get(1)?,
                file_a_id: row.get(2)?,
                file_b_id: row.get(3)?,
                phash_dist: row.get(4)?,
                reason: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn load_burst_runs(&self, scan_id: &str) -> rusqlite::Result<Vec<StoredBurstRun>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, camera_model FROM burst_runs WHERE scan_id = ?1 ORDER BY rowid")?;
        let mut runs: Vec<StoredBurstRun> = stmt
            .query_map(params![scan_id], |row| {
                Ok(StoredBurstRun {
                    id: row.get(0)?,
                    camera_model: row.get(1)?,
                    member_file_ids: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;

        let mut mstmt = self
            .conn
            .prepare("SELECT file_id FROM burst_run_members WHERE burst_run_id = ?1 ORDER BY seq")?;
        for r in &mut runs {
            r.member_file_ids = mstmt
                .query_map(params![r.id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(runs)
    }

    pub fn get_setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
            .optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Test-only escape hatch for rewriting rows the way a tampered
    /// database would look.
    #[cfg(test)]
    pub(crate) fn exec_for_test(&self, sql: &str) {
        self.conn.execute(sql, []).unwrap();
    }

    pub fn record_scan_settings(&self, scan_id: &str, t: &crate::thresholds::Thresholds, include_videos: bool) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO scan_settings (scan_id, match_distance, sequence_similarity, sequence_gap_seconds, include_videos)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![scan_id, t.match_distance, t.sequence_similarity, t.sequence_gap_seconds, include_videos as i64],
        )?;
        Ok(())
    }

    /// The settings a scan was made with; `None` for a scan saved before
    /// they were recorded.
    pub fn get_scan_settings(&self, scan_id: &str) -> rusqlite::Result<Option<(crate::thresholds::Thresholds, bool)>> {
        self.conn
            .query_row(
                "SELECT match_distance, sequence_similarity, sequence_gap_seconds, include_videos FROM scan_settings WHERE scan_id = ?1",
                params![scan_id],
                |r| {
                    Ok((
                        crate::thresholds::Thresholds {
                            match_distance: r.get(0)?,
                            sequence_similarity: r.get(1)?,
                            sequence_gap_seconds: r.get(2)?,
                        },
                        r.get::<_, i64>(3)? != 0,
                    ))
                },
            )
            .optional()
    }

    /// Primary file id -> ids of the companions that travel with it.
    pub fn load_siblings(&self, scan_id: &str) -> rusqlite::Result<HashMap<String, Vec<String>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT primary_file_id, sibling_file_id FROM file_siblings WHERE scan_id = ?1 ORDER BY rowid")?;
        let rows = stmt.query_map(params![scan_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows {
            let (p, s) = row?;
            out.entry(p).or_default().push(s);
        }
        Ok(out)
    }

    /// Writes a batch and all its items in one transaction, before any file
    /// is touched (the write-ahead rule). Items start as `planned`.
    pub fn create_batch(
        &self,
        id: &str,
        root_path: &str,
        quarantine_dir: &str,
        items: &[(String, u64, i64, bool)], // relative path, size, mtime, is_sibling
    ) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "INSERT INTO apply_batches (id, root_path, quarantine_dir, created_at, state) VALUES (?1, ?2, ?3, ?4, 'running')",
            params![id, root_path, quarantine_dir, chrono::Utc::now().to_rfc3339()],
        )?;
        for (seq, (rel, size, mtime, sib)) in items.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO apply_items (batch_id, seq, relative_path, size_bytes, mtime_unix, is_sibling, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'planned')",
                params![id, seq as i64, rel, *size as i64, mtime, *sib as i64],
            )?;
        }
        tx.commit()
    }

    pub fn set_item_state(&self, batch_id: &str, seq: i64, state: &str, note: Option<&str>) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE apply_items SET state = ?3, note = ?4 WHERE batch_id = ?1 AND seq = ?2",
            params![batch_id, seq, state, note],
        )?;
        Ok(())
    }

    pub fn set_batch_state(&self, batch_id: &str, state: &str) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE apply_batches SET state = ?2 WHERE id = ?1", params![batch_id, state])?;
        Ok(())
    }

    pub fn load_batches(&self) -> rusqlite::Result<Vec<StoredBatch>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, root_path, quarantine_dir, created_at, state FROM apply_batches ORDER BY created_at DESC, rowid DESC")?;
        let rows = stmt.query_map([], batch_from_row)?;
        rows.collect()
    }

    pub fn get_batch(&self, id: &str) -> rusqlite::Result<Option<StoredBatch>> {
        self.conn
            .query_row(
                "SELECT id, root_path, quarantine_dir, created_at, state FROM apply_batches WHERE id = ?1",
                params![id],
                batch_from_row,
            )
            .optional()
    }

    pub fn load_batch_items(&self, batch_id: &str) -> rusqlite::Result<Vec<StoredBatchItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, relative_path, size_bytes, mtime_unix, is_sibling, state, note FROM apply_items WHERE batch_id = ?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![batch_id], |r| {
            Ok(StoredBatchItem {
                seq: r.get(0)?,
                relative_path: r.get(1)?,
                size_bytes: r.get::<_, i64>(2)? as u64,
                mtime_unix: r.get(3)?,
                is_sibling: r.get::<_, i64>(4)? != 0,
                state: r.get(5)?,
                note: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn load_orphan_videos(&self, scan_id: &str) -> rusqlite::Result<Vec<StoredOrphanVideo>> {
        let mut stmt = self
            .conn
            .prepare("SELECT file_id, reason FROM orphan_videos WHERE scan_id = ?1 ORDER BY rowid")?;
        let rows = stmt.query_map(params![scan_id], |row| {
            Ok(StoredOrphanVideo { file_id: row.get(0)?, reason: row.get(1)? })
        })?;
        rows.collect()
    }

    pub fn load_analysis(&self, file_id: &str) -> rusqlite::Result<Option<StoredAnalysis>> {
        self.conn
            .query_row(
                "SELECT origin, confidence, evidence_json, sharpness, width, height, camera_model, capture_time, duration_seconds
                 FROM analysis WHERE file_id = ?1",
                params![file_id],
                |row| {
                    let evidence_json: String = row.get(2)?;
                    let evidence: Vec<Evidence> = serde_json::from_str(&evidence_json).unwrap_or_default();
                    Ok(StoredAnalysis {
                        origin: row.get(0)?,
                        confidence: row.get(1)?,
                        evidence,
                        sharpness: row.get(3)?,
                        width: row.get(4)?,
                        height: row.get(5)?,
                        camera_model: row.get(6)?,
                        capture_time: row.get(7)?,
                        duration_seconds: row.get(8)?,
                    })
                },
            )
            .optional()
    }

    pub fn load_files(&self, scan_id: &str) -> rusqlite::Result<Vec<StoredFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, relative_path, file_name, media_class, size_bytes, mtime_unix, is_placeholder
             FROM files WHERE scan_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![scan_id], |row| {
            Ok(StoredFile {
                id: row.get(0)?,
                relative_path: row.get(1)?,
                file_name: row.get(2)?,
                media_class: media_class_from_str(&row.get::<_, String>(3)?),
                size_bytes: row.get::<_, i64>(4)? as u64,
                mtime_unix: row.get(5)?,
                is_placeholder: row.get::<_, i64>(6)? != 0,
            })
        })?;
        rows.collect()
    }

    pub fn load_clusters(&self, scan_id: &str) -> rusqlite::Result<Vec<StoredCluster>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, tier, keeper_file_id, reasons_json FROM clusters WHERE scan_id = ?1 ORDER BY rowid")?;
        let mut clusters: Vec<StoredCluster> = stmt
            .query_map(params![scan_id], |row| {
                let reasons_json: String = row.get(3)?;
                let reasons: Vec<String> = serde_json::from_str(&reasons_json).unwrap_or_default();
                Ok(StoredCluster {
                    id: row.get(0)?,
                    tier: row.get(1)?,
                    keeper_file_id: row.get(2)?,
                    member_file_ids: Vec::new(),
                    reasons,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;

        let mut mstmt = self
            .conn
            .prepare("SELECT file_id FROM cluster_members WHERE cluster_id = ?1 ORDER BY rowid")?;
        for c in &mut clusters {
            let members: Vec<String> = mstmt
                .query_map(params![c.id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            c.member_file_ids = members;
        }
        Ok(clusters)
    }

    /// Records (or overwrites) a user's explicit keep/quarantine decision
    /// for one file, keyed by its stable relative path rather than a
    /// scan-local file id -- see the `decisions` table's doc comment.
    pub fn record_decision(&self, scan_id: &str, relative_path: &str, keep: bool) -> rusqlite::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO decisions (scan_id, relative_path, keep, decided_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(scan_id, relative_path) DO UPDATE SET keep = excluded.keep, decided_at = excluded.decided_at",
            params![scan_id, relative_path, keep as i64, now],
        )?;
        Ok(())
    }

    /// Records many decisions in one transaction -- all or none, so a
    /// bulk "accept suggestions" can't leave a cluster half-decided if it
    /// fails midway.
    pub fn record_decisions(&self, scan_id: &str, decisions: &[(String, bool)]) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (path, keep) in decisions {
            self.record_decision(scan_id, path, *keep)?;
        }
        tx.commit()
    }

    /// Forgets a decision, returning the file to "undecided" -- what undo
    /// needs when the thing being undone was the first decision made.
    pub fn clear_decision(&self, scan_id: &str, relative_path: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM decisions WHERE scan_id = ?1 AND relative_path = ?2",
            params![scan_id, relative_path],
        )?;
        Ok(())
    }

    pub fn get_decision(&self, scan_id: &str, relative_path: &str) -> rusqlite::Result<Option<bool>> {
        self.conn
            .query_row(
                "SELECT keep FROM decisions WHERE scan_id = ?1 AND relative_path = ?2",
                params![scan_id, relative_path],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map(|v| v.map(|k| k != 0))
    }
}

/// Databases made before scan ids became UUIDs have an integer `scans.id`
/// and integer `scan_id` columns, which can't hold one. The project is
/// pre-release and a scan is regenerable (about two minutes for 2700
/// files), so rather than carry a migration for throwaway development
/// databases, drop the old tables -- and say so, since decisions in them
/// go too.
fn drop_legacy_schema(conn: &Connection) -> rusqlite::Result<()> {
    let legacy: bool = conn
        .query_row("SELECT type FROM pragma_table_info('scans') WHERE name = 'id'", [], |r| r.get::<_, String>(0))
        .optional()?
        .is_some_and(|t| t.eq_ignore_ascii_case("INTEGER"));
    if legacy {
        eprintln!("photodedup: old database format -- discarding its scans and decisions (re-scan to rebuild)");
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        for t in [
            "decisions", "cluster_members", "burst_run_members", "review_pairs", "analysis", "clusters", "burst_runs", "files", "scans",
        ] {
            conn.execute(&format!("DROP TABLE IF EXISTS {t}"), [])?;
        }
        conn.pragma_update(None, "foreign_keys", "ON")?;
    }
    Ok(())
}

/// Deletes every scan and everything hanging off it. Explicit, child
/// tables first, rather than leaning on `ON DELETE CASCADE`: several
/// references (`cluster_members.file_id`, `clusters.keeper_file_id`, ...)
/// point at `files` without a cascade, so the order matters.
fn delete_all_scans(tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
    for table in [
        "scan_settings",
        "file_siblings",
        "orphan_videos",
        "cluster_members",
        "burst_run_members",
        "review_pairs",
        "analysis",
        "clusters",
        "burst_runs",
        "decisions",
        "files",
        "scans",
    ] {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
    }
    Ok(())
}

fn batch_from_row(r: &rusqlite::Row) -> rusqlite::Result<StoredBatch> {
    Ok(StoredBatch { id: r.get(0)?, root_path: r.get(1)?, quarantine_dir: r.get(2)?, created_at: r.get(3)?, state: r.get(4)? })
}

fn insert_analysis(tx: &rusqlite::Transaction, row_id: &str, a: &Analyzed) -> rusqlite::Result<()> {
    let evidence_json = serde_json::to_string(&a.provenance.evidence).unwrap_or_default();
    let phash_blob: Option<Vec<u8>> = a.phash.map(|h| {
        let mut buf = Vec::with_capacity(32);
        buf.extend_from_slice(&h.0.to_le_bytes());
        buf.extend_from_slice(&h.1.to_le_bytes());
        buf
    });
    tx.execute(
        "INSERT INTO analysis (file_id, origin, confidence, evidence_json, phash, sharpness, width, height, camera_model, capture_time, duration_seconds)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            row_id,
            origin_str(a.provenance.origin),
            a.provenance.confidence,
            evidence_json,
            phash_blob,
            a.sharpness,
            a.width,
            a.height,
            a.camera_model,
            a.capture_time.map(|t| t.to_string()),
            a.duration_seconds,
        ],
    )?;
    Ok(())
}

fn media_class_str(c: MediaClassSerde) -> &'static str {
    match c {
        MediaClassSerde::Raw => "raw",
        MediaClassSerde::Image => "image",
        MediaClassSerde::Video => "video",
        MediaClassSerde::Sidecar => "sidecar",
    }
}

fn media_class_from_str(s: &str) -> MediaClassSerde {
    match s {
        "raw" => MediaClassSerde::Raw,
        "video" => MediaClassSerde::Video,
        "sidecar" => MediaClassSerde::Sidecar,
        _ => MediaClassSerde::Image,
    }
}

fn origin_str(o: Origin) -> &'static str {
    match o {
        Origin::CameraOriginal => "camera_original",
        Origin::Derived => "derived",
        Origin::Unknown => "unknown",
    }
}

/// Canonicalizes a root path for storage/lookup -- so a trailing slash, a
/// `..` component, or a path reached through a symlink still matches the
/// same stored scan on a later lookup (`find_latest_scan_for_root`). Falls
/// back to the path as given if canonicalization fails (e.g. the folder
/// no longer exists at lookup time) rather than erroring -- this is a
/// best-effort normalization, not something that should ever block a
/// scan from completing.
fn canonical_root_string(root: &Path) -> String {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()).to_string_lossy().to_string()
}

fn tier_str(t: Tier) -> &'static str {
    match t {
        Tier::T1 => "T1",
        Tier::T1b => "T1b",
        Tier::T2Burst => "T2Burst",
        Tier::T2NativeReview => "T2NativeReview",
        Tier::T3CrossDevice => "T3CrossDevice",
        Tier::T2DerivedReview => "T2DerivedReview",
        Tier::T2UnknownReview => "T2UnknownReview",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileRecord, MediaClassSerde};
    use crate::pipeline::{Analyzed, Cluster};
    use crate::provenance::{Evidence, Provenance};
    use std::path::PathBuf;

    fn sample_result() -> (PathBuf, PipelineResult) {
        let root = PathBuf::from("/tmp/fake-root");
        let files = vec![
            FileRecord {
                id: 0,
                path: root.join("a.jpg"),
                file_name: "a.jpg".into(),
                media_class: MediaClassSerde::Image,
                size_bytes: 1000,
                mtime_unix: 1700000000,
                is_placeholder: false,
            },
            FileRecord {
                id: 1,
                path: root.join("b.jpg"),
                file_name: "b.jpg".into(),
                media_class: MediaClassSerde::Image,
                size_bytes: 500,
                mtime_unix: 1700000001,
                is_placeholder: false,
            },
        ];

        let mut analyzed = std::collections::HashMap::new();
        analyzed.insert(
            0,
            Analyzed {
                provenance: Provenance {
                    origin: Origin::CameraOriginal,
                    confidence: 0.9,
                    evidence: vec![Evidence::IntactCameraExif],
                },
                phash: Some(crate::hash::Hash256(123, 456)),
                sharpness: Some(42.0),
                width: Some(4032),
                height: Some(3024),
                size_bytes: 1000,
                camera_model: Some("OnePlus 15".into()),
                capture_time: None,
                duration_seconds: None,
            },
        );

        let clusters = vec![Cluster {
            tier: "T1",
            members: vec![0, 1],
            keeper: 0,
            reasons: vec!["native_vs_derived"],
        }];

        let review_pairs = vec![crate::classify::PairClassification {
            a: 0,
            b: 1,
            a_origin: Origin::CameraOriginal,
            b_origin: Origin::CameraOriginal,
            phash_dist: 44,
            dt_seconds: Some(5),
            tier: crate::classify::Tier::T2NativeReview,
            reason: "native_native_review",
        }];

        let burst_runs = vec![crate::resolve::BurstRun {
            camera_model: "OnePlus 15".into(),
            files: vec![0, 1],
        }];

        (
            root,
            PipelineResult {
                files,
                analyzed,
                clusters,
                review_pairs,
                burst_runs,
                video_matches: vec![],
                orphan_videos: vec![],
                unknown_provenance_videos: vec![],
                placeholder_count: 0,
                placeholder_bytes: 0,
                refused_managed_libraries: vec![],
                sibling_sets: vec![],
            },
        )
    }

    #[test]
    fn round_trips_files_and_clusters_through_sqlite() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        let files = store.load_files(&scan_id).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].relative_path, "a.jpg");
        assert_eq!(files[0].size_bytes, 1000);

        let clusters = store.load_clusters(&scan_id).unwrap();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].tier, "T1");
        assert_eq!(clusters[0].member_file_ids.len(), 2);
        assert_eq!(clusters[0].reasons, vec!["native_vs_derived".to_string()]);
    }

    #[test]
    fn decisions_survive_overwrite_and_are_keyed_by_path_not_file_id() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        assert_eq!(store.get_decision(&scan_id, "a.jpg").unwrap(), None);
        store.record_decision(&scan_id, "a.jpg", true).unwrap();
        assert_eq!(store.get_decision(&scan_id, "a.jpg").unwrap(), Some(true));
        store.record_decision(&scan_id, "a.jpg", false).unwrap();
        assert_eq!(store.get_decision(&scan_id, "a.jpg").unwrap(), Some(false));
    }

    #[test]
    fn a_new_scan_replaces_the_previous_one_completely() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan1 = store.save_scan(&root, &result).unwrap();
        let old_file = store.load_files(&scan1).unwrap()[0].id.clone();
        store.record_decision(&scan1, "a.jpg", true).unwrap();

        // A different root: replacement is global, not per-folder.
        let other = std::env::temp_dir().join("photodedup-other-root");
        let scan2 = store.save_scan(&other, &result).unwrap();

        let scans = store.list_scans().unwrap();
        assert_eq!(scans.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), vec![scan2.clone()]);
        assert!(store.load_files(&scan1).unwrap().is_empty());
        assert!(store.load_clusters(&scan1).unwrap().is_empty());
        assert!(store.load_decisions(&scan1).unwrap().is_empty());
        assert!(store.load_decisions(&scan2).unwrap().is_empty(), "decisions belong to the replaced scan");
        assert_eq!(store.load_files(&scan2).unwrap().len(), 2);
        assert_ne!(scan1, scan2, "scan ids are never reused, so a stale client can't land on the new scan");
        assert!(store.find_latest_scan_for_root(&root).unwrap().is_none());
        // Nothing orphaned: the old file and its analysis went with it.
        assert!(store.get_file(&old_file).unwrap().is_none());
        assert!(store.load_analysis(&old_file).unwrap().is_none());
    }

    #[test]
    fn a_database_with_the_old_integer_scan_ids_is_reset_and_then_usable() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        {
            let conn = Connection::open(tmp.path()).unwrap();
            conn.execute_batch(
                "CREATE TABLE scans (id INTEGER PRIMARY KEY, root_path TEXT NOT NULL, started_at TEXT NOT NULL);
                 INSERT INTO scans (root_path, started_at) VALUES ('/old', 'then');",
            )
            .unwrap();
        }

        let mut store = Store::open(tmp.path()).unwrap();
        assert!(store.list_scans().unwrap().is_empty(), "legacy rows are discarded");

        let (root, result) = sample_result();
        let id = store.save_scan(&root, &result).unwrap();
        assert_eq!(store.list_scans().unwrap().len(), 1);
        assert_eq!(id.len(), 36, "a UUID");

        // Reopening a current-format database must NOT reset it.
        drop(store);
        let store = Store::open(tmp.path()).unwrap();
        assert_eq!(store.list_scans().unwrap().len(), 1);
    }

    #[test]
    fn orphan_videos_round_trip_with_their_reason_and_go_with_the_scan() {
        let (root, mut result) = sample_result();
        result.orphan_videos = vec![result.files[0].id];
        result.unknown_provenance_videos = vec![result.files[1].id];
        let mut store = Store::open_in_memory().unwrap();
        let scan = store.save_scan(&root, &result).unwrap();

        let got = store.load_orphan_videos(&scan).unwrap();
        let reasons: Vec<&str> = got.iter().map(|o| o.reason.as_str()).collect();
        assert_eq!(reasons, vec!["no_match", "unknown_origin"]);

        store.save_scan(&root, &result).unwrap();
        assert!(store.load_orphan_videos(&scan).unwrap().is_empty(), "the replaced scan's rows are gone");
    }

    #[test]
    fn settings_round_trip_and_survive_a_new_scan() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        assert_eq!(store.get_setting("scan_intensity").unwrap(), None);
        store.set_setting("scan_intensity", "efficient").unwrap();
        store.set_setting("scan_intensity", "performance").unwrap();
        assert_eq!(store.get_setting("scan_intensity").unwrap().as_deref(), Some("performance"));

        store.save_scan(&root, &result).unwrap();
        store.save_scan(&root, &result).unwrap();
        assert_eq!(store.get_setting("scan_intensity").unwrap().as_deref(), Some("performance"), "preferences are not scan data");
    }

    #[test]
    fn a_rescan_of_the_same_folder_keeps_choices_for_files_that_have_not_changed() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let first = store.save_scan(&root, &result).unwrap();
        store.record_decision(&first, "a.jpg", false).unwrap();
        store.record_decision(&first, "b.jpg", true).unwrap();

        let second = store.save_scan(&root, &result).unwrap();
        let carried = store.load_decisions(&second).unwrap();
        assert_eq!(carried.get("a.jpg"), Some(&false));
        assert_eq!(carried.get("b.jpg"), Some(&true));
        assert!(store.load_decisions(&first).unwrap().is_empty(), "the old scan is gone");
    }

    #[test]
    fn a_choice_is_not_carried_to_a_file_that_changed_vanished_or_belongs_to_another_folder() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let first = store.save_scan(&root, &result).unwrap();
        store.record_decision(&first, "a.jpg", false).unwrap();
        store.record_decision(&first, "b.jpg", false).unwrap();

        // a.jpg was edited (size changed); b.jpg is gone
        let (_, mut changed) = sample_result();
        changed.files[0].size_bytes += 1;
        changed.files.truncate(1);
        let second = store.save_scan(&root, &changed).unwrap();
        assert!(store.load_decisions(&second).unwrap().is_empty());

        // a different folder never inherits another folder's choices
        store.record_decision(&second, "a.jpg", false).unwrap();
        let other = std::env::temp_dir().join("photodedup-a-different-root");
        let third = store.save_scan(&other, &result).unwrap();
        assert!(store.load_decisions(&third).unwrap().is_empty());
    }

    #[test]
    fn the_settings_a_scan_was_made_with_are_recorded_and_go_with_the_scan() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let id = store.save_scan(&root, &result).unwrap();
        assert!(store.get_scan_settings(&id).unwrap().is_none(), "an unrecorded scan says so");

        let t = crate::thresholds::Thresholds { match_distance: 30, sequence_similarity: 60, sequence_gap_seconds: 7 };
        store.record_scan_settings(&id, &t, false).unwrap();
        assert_eq!(store.get_scan_settings(&id).unwrap(), Some((t, false)));

        let next = store.save_scan(&root, &result).unwrap();
        assert!(store.get_scan_settings(&id).unwrap().is_none());
        assert!(store.get_scan_settings(&next).unwrap().is_none());
    }

    #[test]
    fn a_failed_save_leaves_the_previous_scan_untouched() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan1 = store.save_scan(&root, &result).unwrap();
        store.record_decision(&scan1, "a.jpg", true).unwrap();

        // Two files with one relative path violates UNIQUE(&scan_id,
        // relative_path) partway through the insert -- after the old
        // scan's rows were already deleted inside the transaction.
        let (_, mut bad) = sample_result();
        let dup = bad.files[0].clone();
        bad.files.push(dup);
        assert!(store.save_scan(&root, &bad).is_err());

        assert_eq!(store.list_scans().unwrap().iter().map(|s| s.id.clone()).collect::<Vec<_>>(), vec![scan1.clone()]);
        assert_eq!(store.load_files(&scan1).unwrap().len(), 2);
        assert_eq!(store.load_clusters(&scan1).unwrap().len(), 1);
        assert_eq!(store.get_decision(&scan1, "a.jpg").unwrap(), Some(true));
    }

    #[test]
    fn no_rows_survive_from_a_replaced_scan_in_any_table() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        store.save_scan(&root, &result).unwrap();
        let counts = |store: &Store| -> Vec<i64> {
            ["files", "analysis", "clusters", "cluster_members", "burst_runs", "burst_run_members", "review_pairs", "decisions"]
                .iter()
                .map(|t| store.conn.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0)).unwrap())
                .collect()
        };
        let live = store.list_scans().unwrap()[0].id.clone();
        store.record_decision(&live, "a.jpg", true).unwrap();
        let first = counts(&store);
        store.save_scan(&root, &result).unwrap();
        let second = counts(&store);
        // Same content saved again: identical counts everywhere, so nothing
        // from the old scan is left behind. The one decision made on it is
        // present exactly once, carried to the new scan (same folder, file
        // unchanged), not duplicated.
        assert_eq!(&first[..7], &second[..7]);
        assert_eq!(second[7], 1);
    }

    #[test]
    fn review_pairs_round_trip() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        let pairs = store.load_review_pairs(&scan_id).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].tier, "T2NativeReview");
        assert_eq!(pairs[0].phash_dist, 44);
        assert_eq!(pairs[0].reason, "native_native_review");
    }

    #[test]
    fn burst_runs_round_trip_in_capture_order() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        let runs = store.load_burst_runs(&scan_id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].camera_model, "OnePlus 15");
        assert_eq!(runs[0].member_file_ids.len(), 2);
    }

    #[test]
    fn list_and_get_scan_metadata() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        let scans = store.list_scans().unwrap();
        assert_eq!(scans.len(), 1);
        assert_eq!(scans[0].id, scan_id);
        assert_eq!(scans[0].root_path, root.to_string_lossy());

        let scan = store.get_scan(&scan_id).unwrap().unwrap();
        assert_eq!(scan.id, scan_id);
        assert_eq!(store.get_scan("no-such-scan").unwrap().is_none(), true);
    }

    #[test]
    fn get_file_and_full_path_resolve_correctly() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        let files = store.load_files(&scan_id).unwrap();
        let a_id = files.iter().find(|f| f.relative_path == "a.jpg").unwrap().id.clone();

        let fetched = store.get_file(&a_id).unwrap().unwrap();
        assert_eq!(fetched.file_name, "a.jpg");

        let full_path = store.get_file_full_path(&a_id).unwrap().unwrap();
        assert_eq!(full_path, root.join("a.jpg"));
    }

    #[test]
    fn analysis_round_trips_evidence_and_fields() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        let files = store.load_files(&scan_id).unwrap();
        let a_id = files.iter().find(|f| f.relative_path == "a.jpg").unwrap().id.clone();

        let analysis = store.load_analysis(&a_id).unwrap().unwrap();
        assert_eq!(analysis.origin, "camera_original");
        assert_eq!(analysis.camera_model.as_deref(), Some("OnePlus 15"));
        assert_eq!(analysis.evidence, vec![Evidence::IntactCameraExif]);

        // File b has no analysis entry in sample_result -- must come back
        // as None, not an error or a default-filled struct.
        let b_id = files.iter().find(|f| f.relative_path == "b.jpg").unwrap().id.clone();
        assert!(store.load_analysis(&b_id).unwrap().is_none());
    }

    #[test]
    fn find_latest_scan_for_root_finds_the_most_recent_one() {
        // A real temp dir, not the fixed "/tmp/fake-root" sample_result
        // uses elsewhere -- canonicalization needs a path that actually
        // exists to do anything, and this test's whole point is that the
        // lookup works through canonicalization.
        let tmp = tempfile::TempDir::new().unwrap();
        let (_, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();

        assert!(store.find_latest_scan_for_root(tmp.path()).unwrap().is_none());

        let scan1 = store.save_scan(tmp.path(), &result).unwrap();
        let found = store.find_latest_scan_for_root(tmp.path()).unwrap().unwrap();
        assert_eq!(found.id, scan1);

        // Re-scanning the same root -- the lookup must follow to the
        // newest scan, not stay pinned to the first one found.
        let scan2 = store.save_scan(tmp.path(), &result).unwrap();
        assert_ne!(scan1, scan2);
        let found = store.find_latest_scan_for_root(tmp.path()).unwrap().unwrap();
        assert_eq!(found.id, scan2);
    }

    #[test]
    fn find_latest_scan_matches_through_a_trailing_slash() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (_, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(tmp.path(), &result).unwrap();

        let mut with_slash = tmp.path().to_path_buf();
        with_slash.push(""); // appends a trailing separator
        let found = store.find_latest_scan_for_root(&with_slash).unwrap().unwrap();
        assert_eq!(found.id, scan_id);
    }

    #[test]
    fn bulk_record_and_clear_decision_round_trip() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();
        store
            .record_decisions(&scan_id, &[("a.jpg".into(), true), ("b.jpg".into(), false)])
            .unwrap();
        assert_eq!(store.load_decisions(&scan_id).unwrap().len(), 2);
        store.clear_decision(&scan_id, "a.jpg").unwrap();
        assert_eq!(store.get_decision(&scan_id, "a.jpg").unwrap(), None);
        assert_eq!(store.get_decision(&scan_id, "b.jpg").unwrap(), Some(false));
        // Clearing something never decided is not an error.
        store.clear_decision(&scan_id, "nope.jpg").unwrap();
    }

    #[test]
    fn load_decisions_returns_everything_recorded_for_one_scan() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        store.record_decision(&scan_id, "a.jpg", true).unwrap();
        store.record_decision(&scan_id, "b.jpg", false).unwrap();

        let decisions = store.load_decisions(&scan_id).unwrap();
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions.get("a.jpg"), Some(&true));
        assert_eq!(decisions.get("b.jpg"), Some(&false));
    }

    #[test]
    fn decisions_are_scoped_to_their_scan_id() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan = store.save_scan(&root, &result).unwrap();

        store.record_decision(&scan, "a.jpg", true).unwrap();
        assert_eq!(store.load_decisions(&scan).unwrap().len(), 1);
        assert!(store.load_decisions("some-other-scan").unwrap().is_empty());
    }
}
