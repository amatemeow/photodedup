use photodedup_core::classify::Tier;
use photodedup_core::pipeline::{run, PipelineOptions};
use std::collections::HashMap;

fn main() {
    let Some(dir) = photodedup_core::reference::photos() else {
        eprintln!("set {} to a folder of photos", photodedup_core::reference::ENV);
        std::process::exit(2);
    };
    // Default intensity (Efficient/Default/Performance) -- this is a
    // diagnostic tool, not a benchmark; no reason to override the
    // background-friendly default just to get a one-off dump faster.
    let opts = PipelineOptions::default();
    let result = run(&dir, &opts);

    let name_of: HashMap<u32, &str> = result.files.iter().map(|f| (f.id, f.file_name.as_str())).collect();

    // Dump T0+T1 clusters: one line per cluster, tier + sorted member filenames + keeper filename.
    let mut t0t1: Vec<(String, String, Vec<String>)> = result
        .clusters
        .iter()
        .filter(|c| c.tier == "T0" || c.tier == "T1")
        .map(|c| {
            let mut members: Vec<String> = c.members.iter().map(|id| name_of[id].to_string()).collect();
            members.sort();
            (c.tier.to_string(), name_of[&c.keeper].to_string(), members)
        })
        .collect();
    t0t1.sort_by(|a, b| a.2.cmp(&b.2));

    println!("=== T0+T1 clusters: {} ===", t0t1.len());
    for (tier, keeper, members) in &t0t1 {
        println!("{tier}\t{keeper}\t{}", members.join(","));
    }

    // Dump same-device review pairs (T2NativeReview).
    let mut review: Vec<(String, String)> = result
        .review_pairs
        .iter()
        .filter(|p| p.tier == Tier::T2NativeReview)
        .map(|p| {
            let mut pair = [name_of[&p.a].to_string(), name_of[&p.b].to_string()];
            pair.sort();
            (pair[0].clone(), pair[1].clone())
        })
        .collect();
    review.sort();

    println!("=== same-device review pairs: {} ===", review.len());
    for (a, b) in &review {
        println!("{a}\t{b}");
    }
}
