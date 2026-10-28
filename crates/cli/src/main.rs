//! Headless CLI. `scan <path>` runs the full pipeline and prints a summary
//! -- this is both the user-facing entry point for M1 and the harness
//! M1's end-to-end acceptance test (comparing against the reference
//! Japan-trip dataset's known numbers) runs through.

use photodedup_core::pipeline::{run, PipelineOptions};
use photodedup_core::store::Store;
use photodedup_core::throttle::ScanIntensity;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let Some(cmd) = args.get(1) else {
        print_usage();
        return Ok(());
    };

    match cmd.as_str() {
        "scan" => {
            let Some(path) = args.get(2) else {
                eprintln!("usage: photodedup scan <path> [--mode efficient|default|performance|N]");
                std::process::exit(2);
            };
            let intensity = parse_mode(&args).unwrap_or_else(|msg| {
                eprintln!("{msg}");
                std::process::exit(2);
            });
            run_scan(&PathBuf::from(path), intensity);
        }
        _ => print_usage(),
    }
    Ok(())
}

fn print_usage() {
    println!("photodedup — commands:");
    println!("  scan <path> [--mode efficient|default|performance|N]   scan a folder, print a summary");
    println!("    --mode efficient     fewest workers + strongest E-core bias -- quietest, slowest (measured ~8x lower CPU power than default)");
    println!("    --mode default       (default) efficiency-core-targeted workers, moderate scheduling priority -- can still use performance cores when it needs to");
    println!("    --mode performance   every logical core, no scheduling downgrade -- fastest, most resource-hungry");
    println!("    --mode N             an explicit worker count, no automatic scheduling hint");
    println!("(report/apply/undo land with later M1 work -- see ROADMAP.md)");
}

/// Parses `--mode <value>` out of the CLI args. Returns `Ok(None)` if no
/// `--mode` flag was given (caller should fall back to the default).
fn parse_mode(args: &[String]) -> Result<ScanIntensity, String> {
    let Some(idx) = args.iter().position(|a| a == "--mode") else {
        return Ok(ScanIntensity::default());
    };
    let Some(value) = args.get(idx + 1) else {
        return Err("--mode requires a value: efficient, default, performance, or a number".to_string());
    };
    match value.as_str() {
        "efficient" => Ok(ScanIntensity::Efficient),
        "default" => Ok(ScanIntensity::Default),
        "performance" => Ok(ScanIntensity::Performance),
        n => n
            .parse::<usize>()
            .ok()
            .and_then(NonZeroUsize::new)
            .map(ScanIntensity::Custom)
            .ok_or_else(|| format!("invalid --mode value '{n}': expected efficient, default, performance, or a positive integer")),
    }
}

fn run_scan(path: &PathBuf, intensity: ScanIntensity) {
    let t0 = Instant::now();
    let opts = PipelineOptions {
        intensity,
        ..PipelineOptions::default()
    };
    println!("mode: {:?} ({} worker(s))", intensity, intensity.worker_count());
    let result = run(path, &opts);
    let elapsed = t0.elapsed();

    println!("scanned {} files in {:.1}s", result.files.len(), elapsed.as_secs_f64());
    println!("placeholders skipped: {} ({:.2} GB)", result.placeholder_count, result.placeholder_bytes as f64 / 1e9);
    if !result.refused_managed_libraries.is_empty() {
        println!("refused managed libraries: {:?}", result.refused_managed_libraries);
    }

    let t0_clusters = result.clusters.iter().filter(|c| c.tier == "T0").count();
    let t1_clusters = result.clusters.iter().filter(|c| c.tier == "T1").count();
    let t1b_clusters = result.clusters.iter().filter(|c| c.tier == "T1b").count();

    println!();
    println!("T0 (exact) clusters:   {t0_clusters}");
    println!("T1 (native/derived) clusters: {t1_clusters}");
    println!("T1b (derived/derived) clusters: {t1b_clusters}");

    let burst_files: usize = result.burst_runs.iter().map(|r| r.files.len()).sum();
    println!("burst runs: {} ({} files)", result.burst_runs.len(), burst_files);

    use photodedup_core::classify::Tier;
    let same_device_review = result.review_pairs.iter().filter(|p| p.tier == Tier::T2NativeReview).count();
    let cross_device = result.review_pairs.iter().filter(|p| p.tier == Tier::T3CrossDevice).count();
    println!("same-device review pairs (browse-only): {same_device_review}");
    println!("cross-device review pairs: {cross_device}");

    let high_conf = result.video_matches.iter().filter(|m| m.high_confidence).count();
    println!(
        "video matches: {} ({} high-confidence)",
        result.video_matches.len(),
        high_conf
    );
    println!("orphan videos (no match found): {}", result.orphan_videos.len());
    if !result.unknown_provenance_videos.is_empty() {
        println!(
            "videos with ambiguous provenance (never auto-matched): {}",
            result.unknown_provenance_videos.len()
        );
    }

    let reclaimable: u64 = result
        .clusters
        .iter()
        .flat_map(|c| c.members.iter().filter(|&&m| m != c.keeper))
        .filter_map(|&id| result.files.iter().find(|f| f.id == id))
        .map(|f| f.size_bytes)
        .sum();
    println!("estimated reclaimable (T0+T1+T1b only): {:.2} GB", reclaimable as f64 / 1e9);

    // Deliberately NOT inside the scanned root: that root can be a
    // cloud-synced folder (OneDrive/Dropbox/iCloud Drive), and dropping a
    // stray file into it would get picked up by the sync client. State
    // lives next to where the tool is run from instead -- same lesson the
    // MVP already learned (`~/photo-dedup/` was kept separate from the
    // `originals/` symlink for exactly this reason).
    let db_path = PathBuf::from(".photodedup-scan.sqlite3");
    match Store::open(&db_path) {
        Ok(mut store) => match store.save_scan(path, &result) {
            Ok(scan_id) => println!("\nsaved scan #{scan_id} to {}", db_path.display()),
            Err(e) => eprintln!("warning: failed to save scan to sqlite: {e}"),
        },
        Err(e) => eprintln!("warning: failed to open {}: {e}", db_path.display()),
    }
}
