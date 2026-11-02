//! How the sharpness number falls as a photo is blurred, to choose the "out of focus" cutoff.
//! `cargo run --release -p photodedup-core --example blur_probe -- <jpg>...`
use photodedup_core::hash::sharpness;

fn main() {
    for path in std::env::args().skip(1) {
        let img = image::open(&path).expect("a readable image");
        // A long edge of about 4000 px like a phone photo: blur radii below are in those pixels.
        let row: Vec<String> = [0.0f32, 2.0, 5.0, 12.0, 30.0]
            .iter()
            .map(|&s| {
                let blurred = if s == 0.0 { img.clone() } else { img.blur(s) };
                format!("s{s}={:.0}", sharpness(&blurred))
            })
            .collect();
        println!("{path}: {} ({}x{})", row.join(" "), img.width(), img.height());
    }
}
