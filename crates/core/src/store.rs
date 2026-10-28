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
    id INTEGER PRIMARY KEY,
    root_path TEXT NOT NULL,
    started_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY,
    scan_id INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    file_name TEXT NOT NULL,
    media_class TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    mtime_unix INTEGER NOT NULL,
    is_placeholder INTEGER NOT NULL,
    UNIQUE(scan_id, relative_path)
);

CREATE TABLE IF NOT EXISTS analysis (
    file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
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
    id INTEGER PRIMARY KEY,
    scan_id INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    tier TEXT NOT NULL,
    keeper_file_id INTEGER NOT NULL REFERENCES files(id),
    reasons_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS cluster_members (
    cluster_id INTEGER NOT NULL REFERENCES clusters(id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES files(id),
    PRIMARY KEY (cluster_id, file_id)
);

CREATE TABLE IF NOT EXISTS burst_runs (
    id INTEGER PRIMARY KEY,
    scan_id INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    camera_model TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS burst_run_members (
    burst_run_id INTEGER NOT NULL REFERENCES burst_runs(id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES files(id),
    seq INTEGER NOT NULL,
    PRIMARY KEY (burst_run_id, file_id)
);

-- Review-only pairs (T2Burst/T2NativeReview/T3CrossDevice/etc. -- never
-- auto-selectable, see pipeline::PipelineResult::review_pairs). Was
-- computed but never actually persisted before M2 needed to serve it to
-- the "same-device review" UI tab.
CREATE TABLE IF NOT EXISTS review_pairs (
    id INTEGER PRIMARY KEY,
    scan_id INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    tier TEXT NOT NULL,
    file_a_id INTEGER NOT NULL REFERENCES files(id),
    file_b_id INTEGER NOT NULL REFERENCES files(id),
    phash_dist INTEGER NOT NULL,
    reason TEXT NOT NULL
);

-- User decisions: deliberately separate from `clusters` (which is just the
-- last scan's machine-generated suggestion) so a re-scan can't silently
-- discard what a user already decided. Keyed on relative_path, not
-- file_id, so it survives a fresh scan assigning new file ids.
CREATE TABLE IF NOT EXISTS decisions (
    scan_id INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    keep INTEGER NOT NULL,
    decided_at TEXT NOT NULL,
    PRIMARY KEY (scan_id, relative_path)
);
"#;

#[derive(Debug, Clone)]
pub struct StoredFile {
    pub id: i64,
    pub relative_path: String,
    pub file_name: String,
    pub media_class: MediaClassSerde,
    pub size_bytes: u64,
    pub mtime_unix: i64,
    pub is_placeholder: bool,
}

#[derive(Debug, Clone)]
pub struct StoredCluster {
    pub id: i64,
    pub tier: String,
    pub keeper_file_id: i64,
    pub member_file_ids: Vec<i64>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StoredReviewPair {
    pub id: i64,
    pub tier: String,
    pub file_a_id: i64,
    pub file_b_id: i64,
    pub phash_dist: i64,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct StoredBurstRun {
    pub id: i64,
    pub camera_model: String,
    /// In capture-time order (the `seq` column), not file-id order.
    pub member_file_ids: Vec<i64>,
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
    pub id: i64,
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
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    /// Persists a full pipeline run. Returns the new `scan_id`.
    ///
    /// `root` is recorded for display; paths inside are stored *relative*
    /// to it so a scan is portable across machines/mount points.
    pub fn save_scan(&mut self, root: &Path, result: &PipelineResult) -> rusqlite::Result<i64> {
        let tx = self.conn.transaction()?;
        let now = chrono::Utc::now().to_rfc3339();

        tx.execute(
            "INSERT INTO scans (root_path, started_at) VALUES (?1, ?2)",
            params![canonical_root_string(root), now],
        )?;
        let scan_id = tx.last_insert_rowid();

        // pipeline FileId (u32, scan-local) -> sqlite rowid (i64, stable
        // across the lifetime of this scan row).
        let mut id_map: std::collections::HashMap<FileId, i64> = std::collections::HashMap::new();

        for f in &result.files {
            let rel = f.path.strip_prefix(root).unwrap_or(&f.path).to_string_lossy().to_string();
            tx.execute(
                "INSERT INTO files (scan_id, relative_path, file_name, media_class, size_bytes, mtime_unix, is_placeholder)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    scan_id,
                    rel,
                    f.file_name,
                    media_class_str(f.media_class),
                    f.size_bytes,
                    f.mtime_unix,
                    f.is_placeholder as i64,
                ],
            )?;
            id_map.insert(f.id, tx.last_insert_rowid());
        }

        for (file_id, a) in &result.analyzed {
            let Some(&row_id) = id_map.get(file_id) else { continue };
            insert_analysis(&tx, row_id, a)?;
        }

        for c in &result.clusters {
            let Some(&keeper_row) = id_map.get(&c.keeper) else { continue };
            let reasons_json = serde_json::to_string(&c.reasons).unwrap_or_default();
            tx.execute(
                "INSERT INTO clusters (scan_id, tier, keeper_file_id, reasons_json) VALUES (?1, ?2, ?3, ?4)",
                params![scan_id, c.tier, keeper_row, reasons_json],
            )?;
            let cluster_row = tx.last_insert_rowid();
            for m in &c.members {
                if let Some(&mrow) = id_map.get(m) {
                    tx.execute(
                        "INSERT OR IGNORE INTO cluster_members (cluster_id, file_id) VALUES (?1, ?2)",
                        params![cluster_row, mrow],
                    )?;
                }
            }
        }

        for run in &result.burst_runs {
            tx.execute(
                "INSERT INTO burst_runs (scan_id, camera_model) VALUES (?1, ?2)",
                params![scan_id, run.camera_model],
            )?;
            let run_row = tx.last_insert_rowid();
            for (seq, fid) in run.files.iter().enumerate() {
                if let Some(&mrow) = id_map.get(fid) {
                    tx.execute(
                        "INSERT INTO burst_run_members (burst_run_id, file_id, seq) VALUES (?1, ?2, ?3)",
                        params![run_row, mrow, seq as i64],
                    )?;
                }
            }
        }

        for p in &result.review_pairs {
            let (Some(&a_row), Some(&b_row)) = (id_map.get(&p.a), id_map.get(&p.b)) else {
                continue;
            };
            tx.execute(
                "INSERT INTO review_pairs (scan_id, tier, file_a_id, file_b_id, phash_dist, reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![scan_id, tier_str(p.tier), a_row, b_row, p.phash_dist, p.reason],
            )?;
        }

        tx.commit()?;
        Ok(scan_id)
    }

    pub fn list_scans(&self) -> rusqlite::Result<Vec<ScanMeta>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, root_path, started_at FROM scans ORDER BY id DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok(ScanMeta {
                id: row.get(0)?,
                root_path: row.get(1)?,
                started_at: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_scan(&self, scan_id: i64) -> rusqlite::Result<Option<ScanMeta>> {
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
                "SELECT id, root_path, started_at FROM scans WHERE root_path = ?1 ORDER BY id DESC LIMIT 1",
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
    pub fn load_decisions(&self, scan_id: i64) -> rusqlite::Result<HashMap<String, bool>> {
        let mut stmt = self
            .conn
            .prepare("SELECT relative_path, keep FROM decisions WHERE scan_id = ?1")?;
        let rows = stmt.query_map(params![scan_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
        })?;
        rows.collect()
    }

    pub fn get_file(&self, file_id: i64) -> rusqlite::Result<Option<StoredFile>> {
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
    pub fn get_file_full_path(&self, file_id: i64) -> rusqlite::Result<Option<PathBuf>> {
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

    pub fn load_review_pairs(&self, scan_id: i64) -> rusqlite::Result<Vec<StoredReviewPair>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tier, file_a_id, file_b_id, phash_dist, reason
             FROM review_pairs WHERE scan_id = ?1 ORDER BY id",
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

    pub fn load_burst_runs(&self, scan_id: i64) -> rusqlite::Result<Vec<StoredBurstRun>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, camera_model FROM burst_runs WHERE scan_id = ?1 ORDER BY id")?;
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

    pub fn load_analysis(&self, file_id: i64) -> rusqlite::Result<Option<StoredAnalysis>> {
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

    pub fn load_files(&self, scan_id: i64) -> rusqlite::Result<Vec<StoredFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, relative_path, file_name, media_class, size_bytes, mtime_unix, is_placeholder
             FROM files WHERE scan_id = ?1 ORDER BY id",
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

    pub fn load_clusters(&self, scan_id: i64) -> rusqlite::Result<Vec<StoredCluster>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, tier, keeper_file_id, reasons_json FROM clusters WHERE scan_id = ?1 ORDER BY id")?;
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
            .prepare("SELECT file_id FROM cluster_members WHERE cluster_id = ?1 ORDER BY file_id")?;
        for c in &mut clusters {
            let members: Vec<i64> = mstmt
                .query_map(params![c.id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            c.member_file_ids = members;
        }
        Ok(clusters)
    }

    /// Records (or overwrites) a user's explicit keep/quarantine decision
    /// for one file, keyed by its stable relative path rather than a
    /// scan-local file id -- see the `decisions` table's doc comment.
    pub fn record_decision(&self, scan_id: i64, relative_path: &str, keep: bool) -> rusqlite::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO decisions (scan_id, relative_path, keep, decided_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(scan_id, relative_path) DO UPDATE SET keep = excluded.keep, decided_at = excluded.decided_at",
            params![scan_id, relative_path, keep as i64, now],
        )?;
        Ok(())
    }

    pub fn get_decision(&self, scan_id: i64, relative_path: &str) -> rusqlite::Result<Option<bool>> {
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

fn insert_analysis(tx: &rusqlite::Transaction, row_id: i64, a: &Analyzed) -> rusqlite::Result<()> {
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
            },
        )
    }

    #[test]
    fn round_trips_files_and_clusters_through_sqlite() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        let files = store.load_files(scan_id).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].relative_path, "a.jpg");
        assert_eq!(files[0].size_bytes, 1000);

        let clusters = store.load_clusters(scan_id).unwrap();
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

        assert_eq!(store.get_decision(scan_id, "a.jpg").unwrap(), None);
        store.record_decision(scan_id, "a.jpg", true).unwrap();
        assert_eq!(store.get_decision(scan_id, "a.jpg").unwrap(), Some(true));
        store.record_decision(scan_id, "a.jpg", false).unwrap();
        assert_eq!(store.get_decision(scan_id, "a.jpg").unwrap(), Some(false));
    }

    #[test]
    fn two_scans_of_the_same_root_stay_independent() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan1 = store.save_scan(&root, &result).unwrap();
        let scan2 = store.save_scan(&root, &result).unwrap();
        assert_ne!(scan1, scan2);
        assert_eq!(store.load_files(scan1).unwrap().len(), 2);
        assert_eq!(store.load_files(scan2).unwrap().len(), 2);
    }

    #[test]
    fn review_pairs_round_trip() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        let pairs = store.load_review_pairs(scan_id).unwrap();
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

        let runs = store.load_burst_runs(scan_id).unwrap();
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

        let scan = store.get_scan(scan_id).unwrap().unwrap();
        assert_eq!(scan.id, scan_id);
        assert_eq!(store.get_scan(999).unwrap().is_none(), true);
    }

    #[test]
    fn get_file_and_full_path_resolve_correctly() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        let files = store.load_files(scan_id).unwrap();
        let a_id = files.iter().find(|f| f.relative_path == "a.jpg").unwrap().id;

        let fetched = store.get_file(a_id).unwrap().unwrap();
        assert_eq!(fetched.file_name, "a.jpg");

        let full_path = store.get_file_full_path(a_id).unwrap().unwrap();
        assert_eq!(full_path, root.join("a.jpg"));
    }

    #[test]
    fn analysis_round_trips_evidence_and_fields() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        let files = store.load_files(scan_id).unwrap();
        let a_id = files.iter().find(|f| f.relative_path == "a.jpg").unwrap().id;

        let analysis = store.load_analysis(a_id).unwrap().unwrap();
        assert_eq!(analysis.origin, "camera_original");
        assert_eq!(analysis.camera_model.as_deref(), Some("OnePlus 15"));
        assert_eq!(analysis.evidence, vec![Evidence::IntactCameraExif]);

        // File b has no analysis entry in sample_result -- must come back
        // as None, not an error or a default-filled struct.
        let b_id = files.iter().find(|f| f.relative_path == "b.jpg").unwrap().id;
        assert!(store.load_analysis(b_id).unwrap().is_none());
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
    fn load_decisions_returns_everything_recorded_for_one_scan() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan_id = store.save_scan(&root, &result).unwrap();

        store.record_decision(scan_id, "a.jpg", true).unwrap();
        store.record_decision(scan_id, "b.jpg", false).unwrap();

        let decisions = store.load_decisions(scan_id).unwrap();
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions.get("a.jpg"), Some(&true));
        assert_eq!(decisions.get("b.jpg"), Some(&false));
    }

    #[test]
    fn decisions_do_not_leak_across_different_scan_ids() {
        let (root, result) = sample_result();
        let mut store = Store::open_in_memory().unwrap();
        let scan1 = store.save_scan(&root, &result).unwrap();
        let scan2 = store.save_scan(&root, &result).unwrap();

        store.record_decision(scan1, "a.jpg", true).unwrap();
        assert!(store.load_decisions(scan2).unwrap().is_empty());
    }
}
