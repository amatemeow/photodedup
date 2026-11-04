//! Times a SigLIP-style vision tower on a folder of photos and prints the embedding size and a similarity check.
//!
//!     vision <vision_model.onnx> <photo-folder>
//!
//! Input is 224x224 RGB, (x/255 - 0.5) / 0.5, NCHW, which is the SigLIP 2 preprocessing.
use ort::session::Session;
use ort::value::Tensor;
use std::time::Instant;
use walkdir::WalkDir;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let env = ort::environment::init().with_telemetry(false).build().unwrap();
    let t = Instant::now();
    let mut s = Session::builder(&env).unwrap().with_intra_threads(4).unwrap().commit_from_file(&a[1]).unwrap();
    println!("load: {:?}", t.elapsed());
    for i in s.inputs() {
        println!("in  {} {:?}", i.name(), i.dtype());
    }
    for o in s.outputs() {
        println!("out {} {:?}", o.name(), o.dtype());
    }
    let in_name = s.inputs()[0].name().to_string();
    let fp16 = format!("{:?}", s.inputs()[0].dtype()).contains("Float16");
    let mut embs: Vec<(String, Vec<f32>)> = vec![];
    let (mut total, mut n) = (0.0, 0);
    for e in WalkDir::new(&a[2]).sort_by_file_name().into_iter().flatten() {
        let ext = e.path().extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
        if !["jpg", "jpeg", "png"].contains(&ext.as_str()) {
            continue;
        }
        let img = image::open(e.path()).unwrap().to_rgb8();
        let t = Instant::now();
        let r = image::imageops::resize(&img, 224, 224, image::imageops::FilterType::Triangle);
        let mut chw = vec![0f32; 3 * 224 * 224];
        for (i, p) in r.pixels().enumerate() {
            for c in 0..3 {
                chw[c * 224 * 224 + i] = (p[c] as f32 / 255.0 - 0.5) / 0.5;
            }
        }
        let out = if fp16 {
            let h: Vec<half_stub::F16> = chw.iter().map(|&x| half_stub::F16::from_f32(x)).collect();
            let input = Tensor::from_array(([1usize, 3, 224, 224], h)).unwrap();
            let o = s.run(ort::inputs![in_name.as_str() => input]).unwrap();
            extract(&o)
        } else {
            let input = Tensor::from_array(([1usize, 3, 224, 224], chw)).unwrap();
            let o = s.run(ort::inputs![in_name.as_str() => input]).unwrap();
            extract(&o)
        };
        total += t.elapsed().as_secs_f64();
        n += 1;
        embs.push((e.file_name().to_string_lossy().into_owned(), out));
    }
    println!("{n} photos, {:.1} ms per photo (resize + inference), embedding dim {}", total / n as f64 * 1e3, embs[0].1.len());
    let norm = |v: &Vec<f32>| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let cos = |x: &Vec<f32>, y: &Vec<f32>| x.iter().zip(y).map(|(a, b)| a * b).sum::<f32>() / (norm(x) * norm(y));
    let mut pairs = vec![];
    for i in 0..embs.len() {
        for j in i + 1..embs.len() {
            pairs.push((cos(&embs[i].1, &embs[j].1), i, j));
        }
    }
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    println!("most similar pairs:");
    for (c, i, j) in pairs.iter().take(6) {
        println!("  {c:.3}  {}  {}", embs[*i].0, embs[*j].0);
    }
    println!("least similar: {:.3}", pairs.last().unwrap().0);
}

fn extract(o: &ort::session::SessionOutputs) -> Vec<f32> {
    // prefer the pooled embedding (pooler_output / image_embeds), else the first output
    let key = ["image_embeds", "pooler_output"].iter().find(|k| o.contains_key(**k)).copied();
    let v = match key {
        Some(k) => &o[k],
        None => &o[0],
    };
    if let Ok((_, d)) = v.try_extract_tensor::<f32>() {
        d.to_vec()
    } else {
        let (_, d) = v.try_extract_tensor::<half_stub::F16>().unwrap();
        d.iter().map(|x| x.to_f32()).collect()
    }
}

mod half_stub {
    pub type F16 = half::f16;
}
