//! Measurement spike: ONNX Runtime + YuNet (face detector) + SFace (face embedder).
//!
//!     onnx_spike <yunet.onnx> <sface.onnx> <photo-folder>
//!
//! Prints the model load time and, per photo, decode / YuNet / SFace times. The photos are the synthetic
//! fixture (no faces), so this measures cost, not accuracy; SFace gets a blank 112x112 crop per photo.
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;
use std::time::Instant;
use walkdir::WalkDir;

fn load(env: &Environment, path: &str) -> Session {
    Session::builder(env).unwrap().with_intra_threads(2).unwrap().commit_from_file(path).unwrap()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let env = ort::environment::init().with_telemetry(false).build().unwrap();
    let t = Instant::now();
    let mut yunet = load(&env, &a[1]);
    let mut sface = load(&env, &a[2]);
    println!("load both models: {:?}", t.elapsed());
    for (n, s) in [("yunet", &yunet), ("sface", &sface)] {
        let i: Vec<_> = s.inputs().iter().map(|i| format!("{}:{:?}", i.name(), i.dtype())).collect();
        println!("{n} inputs {i:?}, {} outputs", s.outputs().len());
    }
    let (mut dec, mut yu, mut sf, mut n) = (0.0, 0.0, 0.0, 0);
    for e in WalkDir::new(&a[3]).into_iter().flatten() {
        let ext = e.path().extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
        if !["jpg", "jpeg", "png"].contains(&ext.as_str()) {
            continue;
        }
        let t = Instant::now();
        let img = image::open(e.path()).unwrap().to_rgb8();
        dec += t.elapsed().as_secs_f64();

        let t = Instant::now();
        let r = image::imageops::resize(&img, 640, 640, image::imageops::FilterType::Triangle);
        let mut chw = vec![0f32; 3 * 640 * 640];
        for (i, p) in r.pixels().enumerate() {
            for c in 0..3 {
                chw[c * 640 * 640 + i] = p[2 - c] as f32; // BGR, 0..255, as the zoo's demo feeds it
            }
        }
        let input = Tensor::from_array(([1usize, 3, 640, 640], chw)).unwrap();
        let out = yunet.run(ort::inputs!["input" => input]).unwrap();
        let _ = out.len();
        yu += t.elapsed().as_secs_f64();

        let t = Instant::now();
        let crop = Tensor::from_array(([1usize, 3, 112, 112], vec![0.5f32; 3 * 112 * 112])).unwrap();
        let name = sface.inputs()[0].name().to_string();
        let emb = sface.run(ort::inputs![name.as_str() => crop]).unwrap();
        let (shape, _) = emb[0].try_extract_tensor::<f32>().unwrap();
        if n == 0 {
            println!("sface output shape {shape:?}");
        }
        sf += t.elapsed().as_secs_f64();
        n += 1;
    }
    let n = n as f64;
    println!(
        "{n} photos | per photo ms: decode {:.1}, yunet(640) {:.1}, sface(1 face) {:.1}",
        dec / n * 1e3,
        yu / n * 1e3,
        sf / n * 1e3
    );
}
