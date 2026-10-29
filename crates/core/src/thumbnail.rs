//! Decode -> resize -> JPEG-encode, as bytes. Pure computation, no I/O
//! policy of its own (no cache directory, no cache-hit/miss logic) --
//! that's the server's job (`crates/server`), consistent with `core`
//! having no I/O policy. Exists because the review UI needs to display
//! HEIC/video-frame content in a browser that mostly can't render HEIC
//! directly (Safari is the one exception) -- everything shown in the grid
//! or lightbox goes through here and comes out as a JPEG, regardless of
//! source format.

use crate::decode::decode_image;
use crate::platform::SchedulingHint;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageEncoder};
use std::path::Path;
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum ThumbnailError {
    #[error("unsupported or undecodable source file")]
    DecodeFailed,
    #[error("JPEG encode failed: {0}")]
    Encode(#[from] image::ImageError),
    #[error("ffmpeg frame extraction failed")]
    FrameExtractFailed,
}

/// JPEG bytes for an image file, resized so its longer edge is at most
/// `max_dimension` (aspect-preserving; never upscales -- a source smaller
/// than `max_dimension` is returned as-is at its own size).
pub fn image_thumbnail(path: &Path, ffmpeg_bin: &str, max_dimension: u32, hint: Option<SchedulingHint>) -> Result<Vec<u8>, ThumbnailError> {
    let img = decode_image(path, ffmpeg_bin, hint).ok_or(ThumbnailError::DecodeFailed)?;
    encode_jpeg(&resize_capped(&img, max_dimension), quality_for(max_dimension))
}

/// JPEG bytes for one frame of a video, seeked to `fraction` of its
/// `duration_seconds` (e.g. 0.1 for a representative early frame, matching
/// `video_match.rs`'s own seek points). Used for grid thumbnails and as a
/// static poster frame before video playback starts in the lightbox.
pub fn video_frame_thumbnail(
    path: &Path,
    duration_seconds: f64,
    fraction: f64,
    ffmpeg_bin: &str,
    max_dimension: u32,
    hint: Option<SchedulingHint>,
) -> Result<Vec<u8>, ThumbnailError> {
    let ts = (duration_seconds * fraction).max(0.0);
    let mut command = Command::new(ffmpeg_bin);
    let cmd = command.args(["-ss", &format!("{ts:.2}"), "-i"]).arg(path);
    if let Some(hint) = hint {
        crate::platform::platform().configure_child_process(cmd, hint);
    }
    let output = cmd
        .args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "ppm", "-"])
        .output()
        .map_err(|_| ThumbnailError::FrameExtractFailed)?;
    if output.stdout.is_empty() {
        return Err(ThumbnailError::FrameExtractFailed);
    }
    let img = image::load_from_memory(&output.stdout).map_err(|_| ThumbnailError::FrameExtractFailed)?;
    encode_jpeg(&resize_capped(&img, max_dimension), quality_for(max_dimension))
}

fn resize_capped(img: &DynamicImage, max_dimension: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w <= max_dimension && h <= max_dimension {
        return img.clone();
    }
    img.resize(max_dimension, max_dimension, FilterType::Lanczos3)
}

/// Grid thumbnails are small on screen, so a lighter quality is invisible
/// there and saves about a third of the bytes (quality 75-85 is the usual
/// sweet spot for thumbnails); the large previews keep the higher one.
fn quality_for(max_dimension: u32) -> u8 {
    if max_dimension <= 1000 {
        78
    } else {
        85
    }
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>, ThumbnailError> {
    let rgb = img.to_rgb8();
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality).write_image(&rgb, rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn resize_capped_never_upscales_a_small_image() {
        let small = DynamicImage::ImageRgb8(RgbImage::from_pixel(10, 10, Rgb([1, 2, 3])));
        let out = resize_capped(&small, 256);
        assert_eq!(out.dimensions(), (10, 10));
    }

    #[test]
    fn resize_capped_shrinks_a_large_image_to_fit() {
        let large = DynamicImage::ImageRgb8(RgbImage::from_pixel(4000, 2000, Rgb([1, 2, 3])));
        let out = resize_capped(&large, 256);
        let (w, h) = out.dimensions();
        assert!(w <= 256 && h <= 256);
        // aspect ratio preserved (2:1)
        assert!((w as f64 / h as f64 - 2.0).abs() < 0.1);
    }

    #[test]
    fn encode_jpeg_produces_valid_jpeg_bytes() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(32, 32, Rgb([200, 100, 50])));
        let bytes = encode_jpeg(&img, 80).unwrap();
        assert!(bytes.starts_with(&[0xFF, 0xD8])); // JPEG SOI marker
    }

    #[test]
    #[ignore = "requires the private reference photo set (PHOTODEDUP_REFERENCE_SET) + ffmpeg on PATH"]
    fn heic_thumbnail_is_valid_jpeg_within_bounds() {
        let Some(dir) = crate::reference::photos() else { return };
        let bytes = image_thumbnail(&dir.join("IMG_3684.HEIC"), &crate::tools::ffmpeg(), 256, None).unwrap();
        assert!(bytes.starts_with(&[0xFF, 0xD8]));
        let decoded = image::load_from_memory(&bytes).unwrap();
        let (w, h) = decoded.dimensions();
        assert!(w <= 256 && h <= 256);
    }
}
