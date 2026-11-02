//! Prints each photo's brightness numbers, sharpness and the measured findings, to choose and check
//! the defect thresholds. Reads the reference set only: `PHOTODEDUP_REFERENCE_SET=<folder> cargo run
//! --release -p photodedup-core --example defect_stats`.
use photodedup_core::defects::{light_stats, measure};
use photodedup_core::pipeline::{run, PipelineOptions};

fn main() {
    let Some(dir) = photodedup_core::reference::photos() else {
        eprintln!("set {} to a folder of photos", photodedup_core::reference::ENV);
        std::process::exit(2);
    };
    let opts = PipelineOptions { include_videos: false, ..PipelineOptions::default() };
    let result = run(&dir, &opts);
    println!("file,mean,dark,bright,spread,sharpness,findings");
    for f in &result.files {
        let Some(a) = result.analyzed.get(&f.id) else { continue };
        let Some(l) = a.light else { continue };
        let found: Vec<String> = measure(&l, a.sharpness).iter().map(|x| format!("{:?}:{:.2}", x.kind, x.strength)).collect();
        println!("{},{:.1},{:.3},{:.3},{:.1},{:.1},{}", f.file_name, l.mean, l.dark, l.bright, l.spread, a.sharpness.unwrap_or(-1.0), found.join("|"));
    }
    let _ = light_stats;
}
