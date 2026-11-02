//! Measured checks for photos that are probably not worth keeping: nearly black or washed-out frames,
//! a covered lens, an out-of-focus shot. Plain statistics on the decoded image, no model.
//!
//! Only the numbers are saved with a scan (`LightStats` and the sharpness it already had); the verdicts
//! are worked out from them when asked, so a threshold can change without scanning again. A finding is a
//! suggestion to look, never a reason to move a file: the review lists them and the person decides.

use image::{imageops::FilterType, DynamicImage};
use serde::{Deserialize, Serialize};

/// Brightness numbers of one photo, all on a 0-255 luma scale except the fractions (0-1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LightStats {
    /// Average luma.
    pub mean: f32,
    /// Share of pixels darker than `DARK_LEVEL`.
    pub dark: f32,
    /// Share of pixels brighter than `BRIGHT_LEVEL`.
    pub bright: f32,
    /// Standard deviation of luma: how much is going on in the frame at all.
    pub spread: f32,
}

const DARK_LEVEL: f32 = 24.0;
const BRIGHT_LEVEL: f32 = 232.0;
/// Side of the square the photo is shrunk to for measuring; plenty for a histogram.
const SAMPLE: u32 = 128;

/// Measures a decoded photo. ITU-R 601 luma, like the hashes.
pub fn light_stats(img: &DynamicImage) -> LightStats {
    let small = img.resize_exact(SAMPLE, SAMPLE, FilterType::Triangle).to_rgb8();
    let n = (SAMPLE * SAMPLE) as f32;
    let (mut sum, mut sum_sq, mut dark, mut bright) = (0.0f64, 0.0f64, 0u32, 0u32);
    for p in small.pixels() {
        let y = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
        sum += y as f64;
        sum_sq += (y * y) as f64;
        dark += (y < DARK_LEVEL) as u32;
        bright += (y > BRIGHT_LEVEL) as u32;
    }
    let mean = sum / n as f64;
    let var = (sum_sq / n as f64 - mean * mean).max(0.0);
    LightStats { mean: mean as f32, dark: dark as f32 / n, bright: bright as f32 / n, spread: var.sqrt() as f32 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DefectKind {
    /// Almost every pixel is black: a lens covered by a finger or a pocket shot.
    Black,
    /// Mostly dark, but with some picture in it.
    Dark,
    /// Mostly white: blown out.
    Bright,
    /// Hardly any difference between pixels, though neither dark nor bright: a wall, fog, a lens cap.
    Flat,
    /// Out of focus or moved.
    Blurry,
}

/// Where a finding comes from. A model will add its own source later; the review shows the label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Measured,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Finding {
    pub kind: DefectKind,
    pub source: Source,
    /// 0 (just over the line) to 1 (as bad as it gets): orders the list, not a probability.
    pub strength: f32,
}

/// Laplacian variance (see `hash::sharpness`) below which a photo reads as out of focus. The number
/// depends on how much detail the scene has (a sharp, plain scene can read 100, a sharp busy one 5000),
/// so a cutoff this low only catches heavy blur: a detailed photo blurred by a 12 px radius reads about
/// 100 and a plain one about 5. Finer focus problems are left to the model. Measured with
/// `examples/blur_probe.rs` and `examples/defect_stats.rs`; see ROADMAP.
pub const BLUR_BELOW: f64 = 15.0;
/// Under this average luma the sharpness number says nothing (it is low in any dark frame).
const BLUR_NEEDS_MEAN: f32 = 30.0;

fn finding(kind: DefectKind, strength: f32) -> Finding {
    Finding { kind, source: Source::Measured, strength: strength.clamp(0.0, 1.0) }
}

/// Every measured problem this photo shows, worst first. Empty for a photo that looks fine.
pub fn measure(light: &LightStats, sharpness: Option<f64>) -> Vec<Finding> {
    let mut out = Vec::new();
    // Night shots are dark on purpose: on 1,855 photos of an evening-heavy trip these bounds flag none.
    if light.dark > 0.995 && light.mean < 8.0 {
        out.push(finding(DefectKind::Black, (light.dark - 0.995) / 0.005));
    } else if light.dark > 0.90 && light.mean < 12.0 {
        out.push(finding(DefectKind::Dark, (light.dark - 0.90) / 0.10));
    }
    if light.bright > 0.80 && light.mean > 215.0 {
        out.push(finding(DefectKind::Bright, (light.bright - 0.80) / 0.20));
    }
    // A flat frame that is not already reported as black or white.
    if light.spread < 6.0 && out.is_empty() {
        out.push(finding(DefectKind::Flat, 1.0 - light.spread / 6.0));
    }
    if let Some(s) = sharpness {
        if s < BLUR_BELOW && light.mean >= BLUR_NEEDS_MEAN && out.is_empty() {
            out.push(finding(DefectKind::Blurry, 1.0 - (s / BLUR_BELOW) as f32));
        }
    }
    out.sort_by(|a, b| b.strength.total_cmp(&a.strength));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn solid(color: [u8; 3]) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(200, 150, Rgb(color)))
    }

    fn kinds(img: &DynamicImage, sharp: Option<f64>) -> Vec<DefectKind> {
        measure(&light_stats(img), sharp).iter().map(|f| f.kind).collect()
    }

    #[test]
    fn stats_of_a_black_frame_are_zero_and_all_dark() {
        let s = light_stats(&solid([0, 0, 0]));
        assert_eq!((s.mean, s.dark, s.bright, s.spread), (0.0, 1.0, 0.0, 0.0));
    }

    #[test]
    fn a_black_frame_is_black_not_also_flat_or_blurry() {
        assert_eq!(kinds(&solid([2, 2, 2]), Some(0.0)), [DefectKind::Black]);
    }

    #[test]
    fn a_white_frame_is_bright() {
        assert_eq!(kinds(&solid([250, 250, 250]), None), [DefectKind::Bright]);
    }

    #[test]
    fn a_grey_wall_is_flat() {
        assert_eq!(kinds(&solid([128, 128, 128]), None), [DefectKind::Flat]);
    }

    #[test]
    fn a_night_scene_with_lights_is_not_a_defect() {
        // Mostly dark with a bright patch: spread is high, and under 75% of pixels are dark.
        let mut img = RgbImage::from_pixel(200, 150, Rgb([10, 10, 10]));
        for y in 0..80 {
            for x in 0..200 {
                img.put_pixel(x, y, Rgb([200, 180, 120]));
            }
        }
        assert!(kinds(&DynamicImage::ImageRgb8(img), Some(500.0)).is_empty());
    }

    #[test]
    fn dark_with_some_picture_is_dark_not_black() {
        let mut img = RgbImage::from_pixel(200, 150, Rgb([4, 4, 4]));
        for y in 0..6 {
            for x in 0..200 {
                img.put_pixel(x, y, Rgb([150, 150, 150]));
            }
        }
        assert_eq!(kinds(&DynamicImage::ImageRgb8(img), Some(500.0)), [DefectKind::Dark]);
    }

    #[test]
    fn sharpness_alone_decides_blurry() {
        let mut img = RgbImage::from_pixel(200, 150, Rgb([60, 60, 60]));
        for x in 0..100 {
            for y in 0..150 {
                img.put_pixel(x, y, Rgb([180, 170, 160]));
            }
        }
        let img = DynamicImage::ImageRgb8(img);
        assert_eq!(kinds(&img, Some(3.0)), [DefectKind::Blurry]);
        assert!(kinds(&img, Some(300.0)).is_empty());
        assert!(kinds(&img, None).is_empty(), "no sharpness number, no verdict");
    }

    #[test]
    fn a_white_frame_is_bright_and_nothing_else() {
        let l = LightStats { mean: 250.0, dark: 0.0, bright: 1.0, spread: 1.0 };
        assert_eq!(measure(&l, Some(0.0)).iter().map(|f| f.kind).collect::<Vec<_>>(), [DefectKind::Bright]);
    }

    #[test]
    fn a_dark_frame_is_not_called_blurry() {
        let l = LightStats { mean: 20.0, dark: 0.5, bright: 0.0, spread: 20.0 };
        assert!(measure(&l, Some(2.0)).is_empty());
    }

    #[test]
    fn stronger_findings_come_first() {
        let l = LightStats { mean: 5.0, dark: 1.0, bright: 0.0, spread: 2.0 };
        let f = measure(&l, Some(1.0));
        assert!(f.windows(2).all(|w| w[0].strength >= w[1].strength));
    }
}
