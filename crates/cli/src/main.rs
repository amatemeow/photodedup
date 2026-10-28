//! Headless CLI. `scan <path>` runs the full pipeline and prints a summary
//! -- this is both the user-facing entry point for M1 and the harness
//! M1's end-to-end acceptance test (comparing against the reference
//! Japan-trip dataset's known numbers) runs through.

use photodedup_core::pipeline::{run, PipelineOptions};
use photodedup_core::store::Store;
use photodedup_core::throttle::ScanIntensity;
use std::path::PathBuf;
use std::time::Instant;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let Some(cmd) = args.get(1) else {
        print_usage();
        return Ok(());
    };

    match cmd.as_str() {
        "scan" => {
            let Some(path) = args.get(2) else {
                eprintln!("usage: photodedup scan <path> [--mode efficient|default|performance|N] [--no-videos]");
                std::process::exit(2);
            };
            let intensity = parse_mode(&args).unwrap_or_else(|msg| {
                eprintln!("{msg}");
                std::process::exit(2);
            });
            run_scan(&PathBuf::from(path), intensity, !args.iter().any(|a| a == "--no-videos"));
        }
        "apply" => {
            let commit = args.iter().any(|a| a == "--commit");
            let db = db_arg(&args, 2);
            apply_cmd(&db, commit);
        }
        "undo" => {
            let Some(id) = args.get(2) else {
                eprintln!("usage: photodedup undo <batch-id> [db-path]");
                std::process::exit(2);
            };
            undo_cmd(id, &db_arg(&args, 3));
        }
        "batches" => batches_cmd(&db_arg(&args, 2)),
        "serve" => {
            let db_path = args.get(2).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".photodedup-scan.sqlite3"));
            serve(db_path).await?;
        }
        _ => print_usage(),
    }
    Ok(())
}

fn print_usage() {
    println!("photodedup — commands:");
    println!("  scan <path> [--mode efficient|default|performance|N] [--no-videos]   scan a folder, print a summary");
    println!("    --mode efficient     fewest workers + strongest E-core bias -- quietest, slowest (measured ~8x lower CPU power than default)");
    println!("    --mode default       (default) efficiency-core-targeted workers, moderate scheduling priority -- can still use performance cores when it needs to");
    println!("    --mode performance   every logical core, no scheduling downgrade -- fastest, most resource-hungry");
    println!("    --mode N             an explicit worker count, no automatic scheduling hint");
    println!("    --no-videos          leave videos out of the scan entirely (not listed, hashed, probed or matched)");
    println!("  apply [db-path] [--commit]   show what applying your decisions would move (a dry run); with --commit, move it to quarantine");
    println!("  batches [db-path]            list what has been moved to quarantine");
    println!("  undo <batch-id> [db-path]    put a batch back where it came from");
    println!("  serve [db-path]   start the review server (browser fallback -- M4's native shell calls photodedup-server directly instead)");
    println!("(report/apply/undo land with later M1 work -- see ROADMAP.md)");
}

/// Where the built web UI lives: `PHOTODEDUP_UI_DIR` if set, else
/// `ui/dist` in the working directory (the dev checkout layout). Packaged
/// builds will embed the UI instead (M4).
fn find_ui_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("PHOTODEDUP_UI_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("ui/dist"));
    dir.join("index.html").is_file().then_some(dir)
}

/// The database path argument at `idx`, unless it is a flag.
fn db_arg(args: &[String], idx: usize) -> PathBuf {
    args.get(idx)
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".photodedup-scan.sqlite3"))
}

fn open_store(db: &PathBuf) -> Store {
    if !db.exists() {
        eprintln!("no database at {} -- run `photodedup scan <path>` first", db.display());
        std::process::exit(2);
    }
    Store::open(db).unwrap_or_else(|e| {
        eprintln!("could not open {}: {e}", db.display());
        std::process::exit(1);
    })
}

fn fmt_bytes(n: u64) -> String {
    format!("{:.1} MB", n as f64 / 1_048_576.0)
}

/// Dry run unless `--commit`. Nothing is deleted either way: files move
/// into `<folder>/.photodedup-quarantine/<time>/` and can be undone.
fn apply_cmd(db: &PathBuf, commit: bool) {
    let store = open_store(db);
    let Some(scan) = store.list_scans().ok().and_then(|s| s.into_iter().next()) else {
        eprintln!("no scan in {}", db.display());
        std::process::exit(2);
    };
    let plan = match photodedup_core::apply::plan(&store, &scan.id) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("folder: {}", plan.root_path);
    println!("{} file(s), {} would move to quarantine", plan.items.len(), fmt_bytes(plan.total_bytes));
    for i in &plan.items {
        println!("  {}{}", i.relative_path, if i.is_sibling { "   (companion)" } else { "" });
    }
    for b in &plan.blocked {
        println!("  refused: {} -- {}", b.relative_path, b.reason);
    }
    if !commit {
        println!("\ndry run: nothing was moved. Re-run with --commit to move these files (nothing is deleted; `undo` puts them back).");
        return;
    }
    match photodedup_core::apply::execute(&store, &scan.id) {
        Ok(r) => {
            println!("\nmoved {} file(s) ({})", r.moved, fmt_bytes(r.moved_bytes));
            for s in &r.skipped {
                println!("  left in place: {} -- {}", s.relative_path, s.reason);
            }
            if let Some(id) = r.batch_id {
                println!("batch {id}\nundo with: photodedup undo {id}");
            }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn batches_cmd(db: &PathBuf) {
    let store = open_store(db);
    match photodedup_core::apply::list_batches(&store) {
        Ok(b) if b.is_empty() => println!("no batches"),
        Ok(batches) => {
            for b in batches {
                println!("{}  {}  {} file(s) in quarantine ({})  [{}]", b.id, b.created_at, b.in_quarantine, fmt_bytes(b.in_quarantine_bytes), b.state);
            }
        }
        Err(e) => eprintln!("{e}"),
    }
}

fn undo_cmd(id: &str, db: &PathBuf) {
    let store = open_store(db);
    match photodedup_core::apply::undo(&store, id) {
        Ok(r) => {
            println!("restored {} file(s)", r.restored);
            for s in &r.skipped {
                println!("  still in quarantine: {} -- {}", s.relative_path, s.reason);
            }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

async fn serve(db_path: PathBuf) -> anyhow::Result<()> {
    let info = photodedup_server::run(db_path, find_ui_dir()).await?;
    let url = info.url();
    println!("review server running at {url}");
    println!("(Ctrl+C to stop)");
    open_browser(&url);

    tokio::signal::ctrl_c().await?;
    println!("\nstopping.");
    Ok(())
}

/// Best-effort: opens the system browser. A failure here (no GUI, unknown
/// platform, no default browser configured) is never fatal -- the URL is
/// already printed above, so the user can always open it by hand.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = ("open", vec![url]);
    #[cfg(target_os = "linux")]
    let cmd = ("xdg-open", vec![url]);
    #[cfg(windows)]
    let cmd = ("cmd", vec!["/C", "start", "", url]);

    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    {
        let _ = std::process::Command::new(cmd.0).args(cmd.1).status();
    }
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
    value.parse().map_err(|e| format!("--mode: {e}"))
}

fn run_scan(path: &PathBuf, intensity: ScanIntensity, include_videos: bool) {
    let t0 = Instant::now();
    let opts = PipelineOptions {
        intensity,
        include_videos,
        ..PipelineOptions::default()
    };
    println!("mode: {:?} ({} worker(s))", intensity, intensity.worker_count());
    if !include_videos {
        println!("videos: not scanned (--no-videos)");
    }
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
