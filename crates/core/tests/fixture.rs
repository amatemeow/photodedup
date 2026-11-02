//! The engine against the synthetic library from `scripts/make-fixture.py`: one case for everything it
//! handles, nothing personal. Ignored by default; see the script for how to make the folder.
//!
//!     scripts/make-fixture.py /tmp/fixture [--heic-from <folder of HEICs>]
//!     PHOTODEDUP_FIXTURE=/tmp/fixture cargo test --release -p photodedup-core --test fixture -- --ignored

use photodedup_core::defects::{measure, DefectKind};
use photodedup_core::provenance::Origin;
use photodedup_core::pipeline::{run, PipelineOptions, PipelineResult};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

fn fixture() -> Option<PathBuf> {
    std::env::var_os("PHOTODEDUP_FIXTURE").map(PathBuf::from).filter(|p| p.is_dir())
}

fn name(r: &PipelineResult, id: u32) -> String {
    r.files.iter().find(|f| f.id == id).unwrap().file_name.clone()
}

fn names(r: &PipelineResult, ids: impl IntoIterator<Item = u32>) -> BTreeSet<String> {
    ids.into_iter().map(|i| name(r, i)).collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
#[ignore = "needs the synthetic fixture folder (PHOTODEDUP_FIXTURE), see scripts/make-fixture.py"]
fn the_engine_finds_every_case_in_the_synthetic_library() {
    let Some(dir) = fixture() else {
        eprintln!("skipping: PHOTODEDUP_FIXTURE is not set to a folder");
        return;
    };
    let r = run(&dir, &PipelineOptions::default());
    let heic = r.files.iter().filter(|f| f.file_name.ends_with(".HEIC")).count();
    eprintln!("{} files, {heic} HEIC", r.files.len());

    // Exact copy.
    let t0: Vec<_> = r.clusters.iter().filter(|c| c.tier == "T0").collect();
    assert_eq!(t0.len(), 1);
    assert_eq!(names(&r, t0[0].members.clone()), set(&["IMG_1000.jpg", "IMG_1000 (1).jpg"]));

    // A messenger-style copy: the original with camera EXIF is the keeper.
    let t1: Vec<_> = r.clusters.iter().filter(|c| c.tier == "T1").collect();
    assert_eq!(t1.len(), 1);
    assert_eq!(names(&r, t1[0].members.clone()), set(&["IMG_1001.jpg", "IMG-20261001-WA0001.jpg"]));
    assert_eq!(name(&r, t1[0].keeper), "IMG_1001.jpg");

    // The four-frame sequence (and, with HEICs, the two taken moments apart).
    let runs: Vec<BTreeSet<String>> = r.burst_runs.iter().map(|b| names(&r, b.files.clone())).collect();
    assert!(runs.contains(&set(&["IMG_2000.jpg", "IMG_2001.jpg", "IMG_2002.jpg", "IMG_2003.jpg"])), "{runs:?}");
    assert_eq!(runs.len(), if heic >= 2 { 2 } else { 1 }, "{runs:?}");

    // Look-alikes half a minute apart are a review pair, not a sequence.
    let pairs: Vec<BTreeSet<String>> =
        r.review_pairs.iter().filter(|p| format!("{:?}", p.tier) != "T2Burst").map(|p| names(&r, [p.a, p.b])).collect();
    assert!(pairs.contains(&set(&["IMG_3000.jpg", "IMG_3001.jpg"])), "{pairs:?}");

    // The unrelated photos belong to nothing.
    let grouped: BTreeSet<String> = r
        .clusters
        .iter()
        .flat_map(|c| c.members.clone())
        .chain(r.burst_runs.iter().flat_map(|b| b.files.clone()))
        .chain(r.review_pairs.iter().flat_map(|p| [p.a, p.b]))
        .map(|i| name(&r, i))
        .collect();
    for lone in ["IMG_1002.jpg", "IMG_1003.jpg", "IMG_1004.jpg", "IMG_1005.jpg", "download.jpg", "IMG_5000.JPG"] {
        assert!(!grouped.contains(lone), "{lone} should be alone");
    }

    // A camera-looking file name with no EXIF is a copy, never an original; real EXIF makes an original.
    let origin = |n: &str| r.analyzed[&r.files.iter().find(|f| f.file_name == n).unwrap().id].provenance.origin;
    assert_eq!(origin("IMG_5000.JPG"), Origin::Derived);
    assert_eq!(origin("IMG_1002.jpg"), Origin::CameraOriginal);

    // Files with no EXIF at all are analysed like any other.
    for plain in ["download.jpg", "IMG_5000.JPG", "Screenshot 2026-10-05.png"] {
        let id = r.files.iter().find(|f| f.file_name == plain).unwrap().id;
        assert!(r.analyzed.get(&id).is_some_and(|a| a.phash.is_some()), "{plain} was dropped from analysis");
    }

    // Defects: the four bad frames, nothing else (a dark night scene included).
    let mut found: HashMap<String, Vec<DefectKind>> = HashMap::new();
    for f in &r.files {
        let Some(a) = r.analyzed.get(&f.id) else { continue };
        let Some(light) = a.light else { continue };
        let kinds: Vec<_> = measure(&light, a.sharpness).iter().map(|x| x.kind).collect();
        if !kinds.is_empty() {
            found.insert(f.file_name.clone(), kinds);
        }
    }
    let expect: HashMap<String, Vec<DefectKind>> = [
        ("IMG_4000.jpg", DefectKind::Black),
        ("IMG_4001.jpg", DefectKind::Bright),
        ("IMG_4002.jpg", DefectKind::Flat),
        ("IMG_4003.jpg", DefectKind::Blurry),
    ]
    .into_iter()
    .map(|(n, k)| (n.to_string(), vec![k]))
    .collect();
    assert_eq!(found, expect);

    // Videos are listed, and left out of the photo checks.
    assert_eq!(r.files.iter().filter(|f| f.file_name.ends_with(".mp4")).count(), 2);
}
