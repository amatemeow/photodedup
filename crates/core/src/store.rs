//! SQLite persistence. Normalizes a `pipeline::PipelineResult` into tables
//! keyed on relative path (not the pipeline's ephemeral per-run `FileId`,
//! which is just a scan-local index) so results survive across runs and
//! support resume/incremental rescan later (see `/ROADMAP.md`'s
//! "Incremental rescan" row -- the `size`/`mtime` columns exist from day
//! one specifically so that hook is cheap to add without a schema change).

use crate::model::{FileId, MediaClassSerde};
use crate::pipeline::{Analyzed, PipelineResult};
use crate::provenance::Origin;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

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
            params![root.to_string_lossy(), now],
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

        tx.commit()?;
        Ok(scan_id)
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

        (
            root,
            PipelineResult {
                files,
                analyzed,
                clusters,
                review_pairs: vec![],
                burst_runs: vec![],
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
}
