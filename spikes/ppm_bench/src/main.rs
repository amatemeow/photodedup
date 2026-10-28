use std::time::Instant;
fn main() {
    let png_bytes = std::fs::read("/tmp/t.png").unwrap();
    let t0 = Instant::now();
    let img = image::load_from_memory(&png_bytes).unwrap();
    println!("PNG decode: {:?} ({}x{})", t0.elapsed(), img.width(), img.height());

    let ppm_bytes = std::fs::read("/tmp/t.ppm").unwrap();
    let t0 = Instant::now();
    let img2 = image::load_from_memory(&ppm_bytes).unwrap();
    println!("PPM decode: {:?} ({}x{})", t0.elapsed(), img2.width(), img2.height());
}
