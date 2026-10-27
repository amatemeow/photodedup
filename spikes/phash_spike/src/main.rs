// Spike: does a from-scratch Rust pHash/dHash, replicating the `imagehash`
// library's exact algorithm (phash hash_size=16, highfreq_factor=4; dhash
// hash_size=16), land in the same DISTANCE BANDS as the Python pipeline's
// already-computed results? Bit-exact match is not the bar (resize filters
// differ between Pillow and the `image` crate) -- see PROJECT_CONTEXT.md's
// calibration: true-dup ~20, burst-sibling ~18-20, unrelated ~100-134.

use image::{imageops::FilterType, DynamicImage, GenericImageView};
use std::f64::consts::PI;

/// Pillow's `convert("L")` luma formula (ITU-R 601-2), not the `image`
/// crate's default (which uses Rec. 709 coefficients) -- matters for parity.
fn to_luma_pillow(img: &DynamicImage) -> Vec<Vec<f64>> {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let mut out = vec![vec![0.0f64; w as usize]; h as usize];
    for y in 0..h {
        for x in 0..w {
            let p = rgb.get_pixel(x, y);
            let l = 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
            out[y as usize][x as usize] = l;
        }
    }
    out
}

fn resize_luma(img: &DynamicImage, w: u32, h: u32) -> Vec<Vec<f64>> {
    let resized = img.resize_exact(w, h, FilterType::Lanczos3);
    to_luma_pillow(&resized)
}

/// 1D DCT-II, scipy.fftpack.dct default (type=2, norm=None):
/// y_k = 2 * sum_n x_n * cos(pi * k * (2n+1) / (2N))
fn dct1d(x: &[f64]) -> Vec<f64> {
    let n = x.len();
    let mut y = vec![0.0f64; n];
    for k in 0..n {
        let mut sum = 0.0;
        for (i, &xi) in x.iter().enumerate() {
            sum += xi * ((PI * k as f64 * (2.0 * i as f64 + 1.0)) / (2.0 * n as f64)).cos();
        }
        y[k] = 2.0 * sum;
    }
    y
}

/// 2D separable DCT matching `dct(dct(pixels, axis=0), axis=1)`:
/// axis=0 first (down each column), then axis=1 (across each row).
fn dct2d(pixels: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let h = pixels.len();
    let w = pixels[0].len();

    // axis=0: DCT each column
    let mut step1 = vec![vec![0.0f64; w]; h];
    for c in 0..w {
        let col: Vec<f64> = (0..h).map(|r| pixels[r][c]).collect();
        let dcol = dct1d(&col);
        for r in 0..h {
            step1[r][c] = dcol[r];
        }
    }

    // axis=1: DCT each row of the result
    let mut step2 = vec![vec![0.0f64; w]; h];
    for r in 0..h {
        let drow = dct1d(&step1[r]);
        step2[r] = drow;
    }
    step2
}

fn median(vals: &mut [f64]) -> f64 {
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = vals.len();
    if n % 2 == 0 {
        (vals[n / 2 - 1] + vals[n / 2]) / 2.0
    } else {
        vals[n / 2]
    }
}

/// imagehash.phash(img, hash_size=16): resize to 64x64 (highfreq_factor=4),
/// DCT, take top-left 16x16 block (DC term included, NOT dropped), threshold
/// by the block's median.
fn phash256(img: &DynamicImage) -> u128 {
    let hash_size = 16usize;
    let img_size = (hash_size * 4) as u32; // 64
    let luma = resize_luma(img, img_size, img_size);
    let dct = dct2d(&luma);

    let mut low: Vec<f64> = Vec::with_capacity(hash_size * hash_size);
    for r in 0..hash_size {
        for c in 0..hash_size {
            low.push(dct[r][c]);
        }
    }
    let med = median(&mut low.clone());

    let mut bits: u128 = 0;
    let mut idx = 0usize;
    for r in 0..hash_size {
        for c in 0..hash_size {
            let v = dct[r][c];
            if v > med && idx < 128 {
                bits |= 1 << idx;
            }
            idx += 1;
        }
    }
    bits
}

/// 256 bits doesn't fit u128 -- use two u128 halves (bit 0..127 and 128..255).
fn phash256_full(img: &DynamicImage) -> (u128, u128) {
    let hash_size = 16usize;
    let img_size = (hash_size * 4) as u32;
    let luma = resize_luma(img, img_size, img_size);
    let dct = dct2d(&luma);

    let mut low: Vec<f64> = Vec::with_capacity(hash_size * hash_size);
    for r in 0..hash_size {
        for c in 0..hash_size {
            low.push(dct[r][c]);
        }
    }
    let med = median(&mut low.clone());

    let mut lo: u128 = 0;
    let mut hi: u128 = 0;
    let mut idx = 0usize;
    for r in 0..hash_size {
        for c in 0..hash_size {
            let v = dct[r][c];
            let bit = v > med;
            if bit {
                if idx < 128 {
                    lo |= 1 << idx;
                } else {
                    hi |= 1 << (idx - 128);
                }
            }
            idx += 1;
        }
    }
    (lo, hi)
}

fn dhash256_full(img: &DynamicImage) -> (u128, u128) {
    let hash_size = 16usize;
    let luma = resize_luma(img, (hash_size + 1) as u32, hash_size as u32);
    let mut lo: u128 = 0;
    let mut hi: u128 = 0;
    let mut idx = 0usize;
    for r in 0..hash_size {
        for c in 0..hash_size {
            let bit = luma[r][c + 1] > luma[r][c];
            if bit {
                if idx < 128 {
                    lo |= 1 << idx;
                } else {
                    hi |= 1 << (idx - 128);
                }
            }
            idx += 1;
        }
    }
    (lo, hi)
}

fn hamming256(a: (u128, u128), b: (u128, u128)) -> u32 {
    (a.0 ^ b.0).count_ones() + (a.1 ^ b.1).count_ones()
}

fn load(path: &str) -> DynamicImage {
    image::open(path).unwrap_or_else(|e| panic!("failed to open {path}: {e}"))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: phash_spike <label> <imgA> <imgB>");
        std::process::exit(1);
    }
    let label = &args[1];
    let a = load(&args[2]);
    let b = load(&args[3]);

    let pa = phash256_full(&a);
    let pb = phash256_full(&b);
    let da = dhash256_full(&a);
    let db = dhash256_full(&b);

    let pdist = hamming256(pa, pb);
    let ddist = hamming256(da, db);

    println!("{label}: phash_dist={pdist} dhash_dist={ddist}");
    let _ = phash256(&a); // silence unused warning path
}
