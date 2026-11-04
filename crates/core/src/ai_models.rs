//! The AI model manifest (`models/manifest.json`) and the checks behind `photodedup check-models`.
//!
//! Models are bundled and the app never goes online, so everything that decides whether a model may ship is
//! recorded in the repository: where its weights came from (a pinned commit and the SHA-256 of every source file), the
//! licence of the WEIGHTS and the evidence for it, how the shipped file is built, and the hash of what comes out.
//! The gate refuses a manifest that is incomplete, names a licence we do not accept for core features, or points at
//! build inputs that have changed since it was written. With `--built <dir>` it also checks the built files.

use chrono::NaiveDate;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// SPDX ids accepted for weights that count as "permissive" (the same family `deny.toml` accepts for code).
pub const PERMISSIVE_SPDX: &[&str] = &["Apache-2.0", "MIT", "BSD-2-Clause", "BSD-3-Clause", "ISC", "CC0-1.0", "Unlicense", "Zlib"];

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub models: Vec<Model>,
}

#[derive(Debug, Deserialize)]
pub struct Model {
    pub id: String,
    pub purpose: String,
    /// `core` features must be permissive; an `optional` component may carry a non-commercial licence.
    pub component: String,
    pub licence: Licence,
    pub training_data: TrainingData,
    pub source: Source,
    pub build: Build,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Deserialize)]
pub struct Licence {
    pub spdx: String,
    /// `permissive` or `non-commercial`.
    pub class: String,
    pub evidence: String,
    /// ISO date the licence was read from the model card.
    pub checked: String,
}

#[derive(Debug, Deserialize)]
pub struct TrainingData {
    pub name: String,
    pub released: bool,
    pub note: String,
}

#[derive(Debug, Deserialize)]
pub struct Source {
    pub repo: String,
    pub host: String,
    pub commit: String,
    pub files: Vec<SourceFile>,
}

#[derive(Debug, Deserialize)]
pub struct SourceFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Deserialize)]
pub struct Build {
    pub script: String,
    pub locks: std::collections::BTreeMap<String, Lock>,
    #[serde(default)]
    pub reproduced_on: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Lock {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub sha256: String,
    pub size: u64,
    /// Whether this file goes into the installers (the other files are build by-products kept for comparison).
    pub ships: bool,
}

impl Manifest {
    pub fn parse(json: &str) -> Result<Manifest, String> {
        serde_json::from_str(json).map_err(|e| format!("manifest is not valid: {e}"))
    }

    /// Every problem with the manifest itself and with the build inputs it pins (script and lock files, relative to `root`).
    pub fn problems(&self, root: &Path, today: NaiveDate) -> Vec<String> {
        let mut out = Vec::new();
        if self.schema != 1 {
            out.push(format!("schema {} is not understood (this build reads 1)", self.schema));
        }
        if self.models.is_empty() {
            out.push("the manifest lists no models".into());
        }
        let mut ids = HashSet::new();
        for m in &self.models {
            let who = format!("model '{}'", m.id);
            let mut bad = |msg: String| out.push(format!("{who}: {msg}"));
            if m.id.trim().is_empty() || !ids.insert(m.id.clone()) {
                bad("id is empty or used twice".into());
            }
            if m.purpose.trim().is_empty() {
                bad("purpose is empty".into());
            }
            if !matches!(m.component.as_str(), "core" | "optional") {
                bad(format!("component '{}' must be core or optional", m.component));
            }
            match m.licence.class.as_str() {
                "permissive" => {
                    if !PERMISSIVE_SPDX.contains(&m.licence.spdx.as_str()) {
                        bad(format!("licence '{}' is not on the permissive list, so it cannot be classed permissive", m.licence.spdx));
                    }
                }
                "non-commercial" => {
                    if m.component != "optional" {
                        bad("a non-commercial licence is only allowed for an optional component, never core".into());
                    }
                }
                other => bad(format!("licence class '{other}' must be permissive or non-commercial")),
            }
            if m.licence.evidence.trim().is_empty() {
                bad("the licence has no evidence (where it was read)".into());
            }
            match NaiveDate::parse_from_str(&m.licence.checked, "%Y-%m-%d") {
                Ok(d) if d <= today => {}
                Ok(_) => bad("licence 'checked' date is in the future".into()),
                Err(_) => bad("licence 'checked' is not an ISO date".into()),
            }
            if m.training_data.name.trim().is_empty() || m.training_data.note.trim().is_empty() {
                bad("training data must be named and described, even if it is not public".into());
            }
            if !is_hex(&m.source.commit, 40) {
                bad("source commit must be a full 40-character hex id".into());
            }
            if m.source.repo.trim().is_empty() || m.source.host.trim().is_empty() || m.source.files.is_empty() {
                bad("source needs a repo, a host and at least one file".into());
            }
            for f in &m.source.files {
                if !plain_name(&f.path) || !is_hex(&f.sha256, 64) || f.size == 0 {
                    bad(format!("source file '{}' needs a plain name, a 64-hex sha256 and a size", f.path));
                }
            }
            if !root.join(&m.build.script).is_file() {
                bad(format!("build script {} does not exist", m.build.script));
            }
            if m.build.locks.is_empty() {
                bad("no lock files recorded for the build".into());
            }
            for (platform, lock) in &m.build.locks {
                match sha256_file(&root.join(&lock.path)) {
                    Err(e) => bad(format!("{platform} lock {}: {e}", lock.path)),
                    Ok(h) if h != lock.sha256 => bad(format!(
                        "{platform} lock {} changed (sha256 {h}, manifest has {}): rebuild the artifacts and update the manifest together",
                        lock.path, lock.sha256
                    )),
                    Ok(_) => {}
                }
            }
            let mut names = HashSet::new();
            for a in &m.artifacts {
                if !plain_name(&a.name) || !names.insert(a.name.as_str()) || !is_hex(&a.sha256, 64) || a.size == 0 {
                    bad(format!("artifact '{}' needs a unique plain name, a 64-hex sha256 and a size", a.name));
                }
            }
            if !m.artifacts.iter().any(|a| a.ships) {
                bad("no artifact is marked as shipping".into());
            }
        }
        out
    }

    /// Problems with built files in `dir`: every shipping artifact must be there with the recorded size and hash;
    /// a non-shipping one is checked only if present.
    pub fn built_problems(&self, dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        for m in &self.models {
            for a in &m.artifacts {
                let path = dir.join(&a.name);
                if !path.exists() {
                    if a.ships {
                        out.push(format!("{}: {} was not built", m.id, a.name));
                    }
                    continue;
                }
                match (std::fs::metadata(&path), sha256_file(&path)) {
                    (Ok(md), Ok(h)) if md.len() == a.size && h == a.sha256 => {}
                    (Ok(md), Ok(h)) => out.push(format!(
                        "{}: {} is {} bytes with sha256 {h}; the manifest says {} bytes and {}",
                        m.id, a.name, md.len(), a.size, a.sha256
                    )),
                    (_, Err(e)) => out.push(format!("{}: {}: {e}", m.id, a.name)),
                    (Err(e), _) => out.push(format!("{}: {}: {e}", m.id, a.name)),
                }
            }
        }
        out
    }
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A bare file name: nothing that could point outside the folder it is joined to.
fn plain_name(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains(['/', '\\']) && !s.contains('\0')
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const REAL: &str = include_str!("../../../models/manifest.json");

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 4).unwrap()
    }

    fn problems_after(edit: impl FnOnce(&mut Value)) -> Vec<String> {
        let mut v: Value = serde_json::from_str(REAL).unwrap();
        edit(&mut v);
        Manifest::parse(&v.to_string()).unwrap().problems(&root(), today())
    }

    /// The gate on the real manifest: this is what makes a changed lock file or script path fail `cargo test`.
    #[test]
    fn the_committed_manifest_passes_its_own_gate() {
        let m = Manifest::parse(REAL).unwrap();
        assert_eq!(m.problems(&root(), NaiveDate::from_ymd_opt(2099, 1, 1).unwrap()), Vec::<String>::new());
    }

    #[test]
    fn a_non_commercial_licence_is_refused_for_a_core_model_but_allowed_for_an_optional_one() {
        let p = problems_after(|v| {
            v["models"][0]["licence"]["class"] = "non-commercial".into();
            v["models"][0]["licence"]["spdx"] = "LicenseRef-non-commercial".into();
        });
        assert!(p.iter().any(|s| s.contains("never core")), "{p:?}");
        let p = problems_after(|v| {
            v["models"][0]["licence"]["class"] = "non-commercial".into();
            v["models"][0]["licence"]["spdx"] = "LicenseRef-non-commercial".into();
            v["models"][0]["component"] = "optional".into();
        });
        assert!(p.is_empty(), "{p:?}");
    }

    #[test]
    fn a_licence_outside_the_permissive_list_cannot_be_called_permissive() {
        let p = problems_after(|v| v["models"][0]["licence"]["spdx"] = "CC-BY-NC-4.0".into());
        assert!(p.iter().any(|s| s.contains("not on the permissive list")), "{p:?}");
    }

    #[test]
    fn an_unpinned_or_malformed_source_is_refused() {
        let p = problems_after(|v| v["models"][0]["source"]["commit"] = "main".into());
        assert!(p.iter().any(|s| s.contains("40-character")), "{p:?}");
        let p = problems_after(|v| v["models"][0]["source"]["files"][0]["sha256"] = "abc".into());
        assert!(p.iter().any(|s| s.contains("64-hex")), "{p:?}");
        let p = problems_after(|v| v["models"][0]["source"]["files"][0]["path"] = "../escape".into());
        assert!(p.iter().any(|s| s.contains("plain name")), "{p:?}");
    }

    #[test]
    fn a_changed_lock_file_must_come_with_a_new_manifest() {
        let p = problems_after(|v| v["models"][0]["build"]["locks"]["linux"]["sha256"] = "0".repeat(64).into());
        assert!(p.iter().any(|s| s.contains("changed")), "{p:?}");
    }

    #[test]
    fn missing_evidence_a_future_date_or_no_shipping_artifact_is_refused() {
        let p = problems_after(|v| v["models"][0]["licence"]["evidence"] = "".into());
        assert!(p.iter().any(|s| s.contains("no evidence")), "{p:?}");
        let p = problems_after(|v| v["models"][0]["licence"]["checked"] = "2030-01-01".into());
        assert!(p.iter().any(|s| s.contains("future")), "{p:?}");
        let p = problems_after(|v| {
            for a in v["models"][0]["artifacts"].as_array_mut().unwrap() {
                a["ships"] = false.into();
            }
        });
        assert!(p.iter().any(|s| s.contains("shipping")), "{p:?}");
    }

    #[test]
    fn built_files_must_match_the_recorded_size_and_hash() {
        let dir = tempfile::tempdir().unwrap();
        let mut v: Value = serde_json::from_str(REAL).unwrap();
        let body = b"not really a model";
        let hash = sha256_file(&{
            let p = dir.path().join("probe");
            std::fs::write(&p, body).unwrap();
            p
        })
        .unwrap();
        // Point the shipping artifact at the file we are about to write.
        let shipped = v["models"][0]["artifacts"][0]["name"].as_str().unwrap().to_string();
        v["models"][0]["artifacts"][0]["sha256"] = hash.clone().into();
        v["models"][0]["artifacts"][0]["size"] = (body.len() as u64).into();
        v["models"][0]["artifacts"][1]["name"] = "other.onnx".into();
        let m = Manifest::parse(&v.to_string()).unwrap();

        assert!(m.built_problems(dir.path()).iter().any(|s| s.contains("was not built")));
        std::fs::write(dir.path().join(&shipped), b"tampered bytes....").unwrap();
        assert!(m.built_problems(dir.path()).iter().any(|s| s.contains("manifest says")));
        std::fs::write(dir.path().join(&shipped), body).unwrap();
        assert_eq!(m.built_problems(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn sha256_matches_a_known_value() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("abc");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
