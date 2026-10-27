//! Perceptual hashing: a from-scratch reimplementation of the Python
//! `imagehash` library's `phash(hash_size=16)` and `dhash(hash_size=16)`,
//! validated for distance-band parity against the reference Japan-trip
//! dataset in M0 — see `/SPIKES.md` §3 for the measured numbers and why
//! bit-exactness isn't (and can't be) the bar: Pillow's and the `image`
//! crate's resize filters aren't identical implementations.
//!
//! Two details that are easy to get wrong and silently drift from Python:
//! - Luma conversion must use Pillow's ITU-R 601-2 coefficients
//!   (0.299/0.587/0.114), not the `image` crate's default `to_luma8()`,
//!   which uses Rec. 709 coefficients and measurably shifts the hash.
//! - `imagehash.phash` keeps the DCT's DC term (`dct[:hash_size,
//!   :hash_size]` includes index 0) — do not drop it as some "classic"
//!   pHash write-ups do.
//!
//! See `crates/core/src/exif.rs`-adjacent decode path for why HEIC inputs
//! must already be pre-decoded to RGB before reaching this module: decode
//! happens via a bundled ffmpeg subprocess (no `-map`, see `/SPIKES.md` §1),
//! not here.

use image::{imageops::FilterType, DynamicImage};
use std::f64::consts::PI;

/// A 256-bit hash, stored as two `u128` halves (bits 0..127, 128..255).
/// `Hamming` distance between two of these is what every classification
/// tier (`T0`/`T1`/`T2`/`T3`) is built on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hash256(pub u128, pub u128);

impl Hash256 {
    pub fn distance(&self, other: &Hash256) -> u32 {
        (self.0 ^ other.0).count_ones() + (self.1 ^ other.1).count_ones()
    }
}

/// Pillow's `Image.convert("L")`: ITU-R 601-2 luma, not the `image` crate's
/// Rec. 709 default. Operates on an already-decoded RGB image.
fn to_luma_pillow(img: &DynamicImage) -> Vec<Vec<f64>> {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut out = vec![vec![0.0f64; w]; h];
    for (y, row) in out.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            let p = rgb.get_pixel(x as u32, y as u32);
            *cell = 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
        }
    }
    out
}

fn resize_luma(img: &DynamicImage, w: u32, h: u32) -> Vec<Vec<f64>> {
    let resized = img.resize_exact(w, h, FilterType::Lanczos3);
    to_luma_pillow(&resized)
}

/// 1D DCT-II matching `scipy.fftpack.dct` default (type=2, norm=None):
/// `y_k = 2 * sum_n x_n * cos(pi * k * (2n+1) / (2N))`.
///
/// O(n^2) direct summation — plenty fast at n=64 (one pHash is ~64*64*64*2
/// multiply-adds, negligible next to image decode cost). Worth revisiting
/// with a real FFT-based DCT only if profiling ever shows this matters.
fn dct1d(x: &[f64]) -> Vec<f64> {
    let n = x.len();
    let mut y = vec![0.0f64; n];
    for (k, yk) in y.iter_mut().enumerate() {
        let mut sum = 0.0;
        for (i, &xi) in x.iter().enumerate() {
            sum += xi * ((PI * k as f64 * (2.0 * i as f64 + 1.0)) / (2.0 * n as f64)).cos();
        }
        *yk = 2.0 * sum;
    }
    y
}

/// Separable 2D DCT matching `dct(dct(pixels, axis=0), axis=1)`.
fn dct2d(pixels: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let h = pixels.len();
    let w = pixels[0].len();

    let mut step1 = vec![vec![0.0f64; w]; h];
    for c in 0..w {
        let col: Vec<f64> = (0..h).map(|r| pixels[r][c]).collect();
        let dcol = dct1d(&col);
        for r in 0..h {
            step1[r][c] = dcol[r];
        }
    }

    let mut step2 = vec![vec![0.0f64; w]; h];
    for (r, row) in step2.iter_mut().enumerate() {
        *row = dct1d(&step1[r]);
    }
    step2
}

fn median(vals: &[f64]) -> f64 {
    let mut v = vals.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).expect("hash input contains NaN"));
    let n = v.len();
    if n % 2 == 0 {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    } else {
        v[n / 2]
    }
}

/// `imagehash.phash(img, hash_size=16)`: resize to 64x64 (highfreq_factor=4
/// is baked in, matching the Python call site), DCT, top-left 16x16 block
/// (DC term included), threshold against that block's own median.
pub fn phash256(img: &DynamicImage) -> Hash256 {
    const HASH_SIZE: usize = 16;
    const IMG_SIZE: u32 = (HASH_SIZE * 4) as u32; // highfreq_factor=4, as pass3_phash.py calls it

    let luma = resize_luma(img, IMG_SIZE, IMG_SIZE);
    let dct = dct2d(&luma);

    let mut low = Vec::with_capacity(HASH_SIZE * HASH_SIZE);
    for row in dct.iter().take(HASH_SIZE) {
        low.extend_from_slice(&row[..HASH_SIZE]);
    }
    let med = median(&low);

    pack_bits(dct.iter().take(HASH_SIZE).flat_map(|row| row[..HASH_SIZE].iter().map(move |&v| v > med)))
}

/// `imagehash.dhash(img, hash_size=16)`: resize to 17x16, threshold each
/// pixel against its left neighbour.
pub fn dhash256(img: &DynamicImage) -> Hash256 {
    const HASH_SIZE: usize = 16;
    let luma = resize_luma(img, (HASH_SIZE + 1) as u32, HASH_SIZE as u32);

    pack_bits(
        luma.iter()
            .take(HASH_SIZE)
            .flat_map(|row| (0..HASH_SIZE).map(move |c| row[c + 1] > row[c])),
    )
}

fn pack_bits(bits: impl Iterator<Item = bool>) -> Hash256 {
    let mut lo: u128 = 0;
    let mut hi: u128 = 0;
    for (idx, bit) in bits.enumerate() {
        if bit {
            if idx < 128 {
                lo |= 1 << idx;
            } else {
                hi |= 1 << (idx - 128);
            }
        }
    }
    Hash256(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn solid(w: u32, h: u32, color: [u8; 3]) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb(color)))
    }

    #[test]
    fn identical_images_hash_to_distance_zero() {
        let img = solid(256, 256, [120, 60, 200]);
        assert_eq!(phash256(&img).distance(&phash256(&img)), 0);
        assert_eq!(dhash256(&img).distance(&dhash256(&img)), 0);
    }

    #[test]
    fn solid_vs_checkerboard_is_far_apart() {
        let solid_img = solid(256, 256, [128, 128, 128]);
        let mut checker = RgbImage::new(256, 256);
        for y in 0..256 {
            for x in 0..256 {
                let v = if (x / 16 + y / 16) % 2 == 0 { 0 } else { 255 };
                checker.put_pixel(x, y, Rgb([v, v, v]));
            }
        }
        let checker_img = DynamicImage::ImageRgb8(checker);
        let d = phash256(&solid_img).distance(&phash256(&checker_img));
        // A flat field vs. a checkerboard should differ in most DCT bits.
        assert!(d > 50, "expected a large distance, got {d}");
    }

    /// Regression test against the real Python pipeline's output, pinned to
    /// the reference Japan-trip dataset. Ignored by default since that
    /// dataset is personal data, not something to commit to this repo —
    /// run explicitly with `cargo test -- --ignored` on a machine that has
    /// `~/photo-dedup/originals/` (a symlink to the source photos) and
    /// `~/photo-dedup/phash_results.json` (the Python-computed ground
    /// truth). See `/SPIKES.md` §3 for the measured numbers this pins to.
    #[test]
    #[ignore = "requires the personal ~/photo-dedup reference dataset"]
    fn parity_against_python_pipeline() {
        use std::path::Path;

        let base = dirs_originals();
        if !base.exists() {
            eprintln!("skipping: {} not found", base.display());
            return;
        }

        let cases: &[(&str, &str, &str, u32, u32)] = &[
            // (label, file_a, file_b, max_phash_dist, python_phash_dist)
            ("true-dup", "IMG20261015084747.jpg", "IMG_3465.JPG", 10, 0),
            (
                "burst-sibling",
                "IMG20261019101441.jpg",
                "IMG20261019101442.jpg",
                40,
                30,
            ),
        ];

        for (label, a, b, max_dist, python_dist) in cases {
            let pa = phash256(&image::open(base.join(a)).unwrap());
            let pb = phash256(&image::open(base.join(b)).unwrap());
            let d = pa.distance(&pb);
            assert!(
                d <= *max_dist,
                "{label}: rust phash_dist={d} too far from python's {python_dist} (band limit {max_dist})"
            );
        }

        fn dirs_originals() -> std::path::PathBuf {
            Path::new(&std::env::var("HOME").unwrap())
                .join("photo-dedup")
                .join("originals")
        }
    }
}
