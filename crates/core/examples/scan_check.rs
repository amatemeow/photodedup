use photodedup_core::scan::scan;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::var("HOME").unwrap())
        .join("photo-dedup")
        .join("originals");
    let report = scan(&dir);
    println!("files: {}", report.files.len());
    println!("sibling_sets: {}", report.sibling_sets.len());
    println!("multi-member sibling sets: {}", report.sibling_sets.iter().filter(|s| !s.siblings.is_empty()).count());
    println!("placeholders: {} ({} bytes)", report.placeholder_count, report.placeholder_bytes);
    println!("refused managed libraries: {:?}", report.refused_managed_libraries);
    let by_class = report.files.iter().fold(std::collections::HashMap::new(), |mut m, f| {
        *m.entry(format!("{:?}", f.media_class)).or_insert(0) += 1;
        m
    });
    println!("by class: {:?}", by_class);
}
