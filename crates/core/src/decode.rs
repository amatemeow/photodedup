//! Image pixel decode dispatch. JPEG/PNG go straight through the `image`
//! crate; HEIC/HEIF go through the bundled ffmpeg (piped, no temp file) --
//! see `/SPIKES.md` §1 for why this needs **no explicit `-map`**: modern
//! iPhone HEIC files are multi-stream (a full-res tile grid, a depth-map
//! tile grid, a 10-bit HDR variant), and an explicit stream/group mapping
//! grabs a single raw tile or unassembled tiles instead of the composite
//! image. ffmpeg's own default stream selection gets it right.

use crate::platform::{platform, SchedulingHint};
use image::DynamicImage;
use std::path::Path;
use std::process::Command;

pub fn decode_image(path: &Path, ffmpeg_bin: &str, hint: Option<SchedulingHint>) -> Option<DynamicImage> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" => image::open(path).ok(),
        "heic" | "heif" => decode_heic_via_ffmpeg(path, ffmpeg_bin, hint),
        _ => None,
    }
}

fn decode_heic_via_ffmpeg(path: &Path, ffmpeg_bin: &str, hint: Option<SchedulingHint>) -> Option<DynamicImage> {
    let mut command = Command::new(ffmpeg_bin);
    let cmd = command.arg("-i").arg(path);
    if let Some(hint) = hint {
        // See `platform::PlatformScheduler::configure_child_process`'s
        // doc comment: thread-level QoS does NOT reach this child process
        // on its own -- this is the actual fix, not the Rust-thread hint.
        platform().configure_child_process(cmd, hint);
    }
    let output = cmd
        // Deliberately NO `-map` -- see module docs. Pipe format is `ppm`
        // (raw, uncompressed), not `png`: PNG's DEFLATE compression costs
        // ~1.1s round-trip (ffmpeg encode + Rust decode) on a 12MP source
        // -- all of it wasted, since this is a local pipe where the extra
        // bytes are free and compression CPU is not. Measured >10x faster
        // end to end with `ppm` on the reference dataset.
        .args(["-f", "image2pipe", "-vcodec", "ppm", "-frames:v", "1", "-"])
        .output()
        .ok()?;
    if output.stdout.is_empty() {
        return None;
    }
    image::load_from_memory(&output.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[ignore = "requires the personal ~/photo-dedup reference dataset + ffmpeg on PATH"]
    fn heic_decode_matches_known_dimensions() {
        let dir = PathBuf::from(std::env::var("HOME").unwrap())
            .join("photo-dedup")
            .join("originals");
        if !dir.exists() {
            return;
        }
        let img = decode_image(&dir.join("IMG_3684.HEIC"), &crate::tools::ffmpeg(), Some(SchedulingHint::Utility))
            .expect("decode failed");
        assert_eq!((img.width(), img.height()), (4032, 3024));
    }
}
