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
    // `-threads 1` (an input option, so it goes before `-i`). An iPhone HEIC is
    // a grid of dozens of tiles and ffmpeg opens one decoder per tile, each
    // with a thread pool sized to the CPU count: ~650 threads for one photo on
    // a 12-core machine, multiplied by the scan's workers. A still tile has
    // nothing to parallelise, so the pools bought nothing. Measured on the
    // reference set: the same pixels (byte-identical output), ~15x fewer
    // threads, ~15% less CPU, ~17% faster with 4 concurrent decodes. Without
    // the limit a process-count cap (containers, `ulimit -u`) makes the decode
    // fail with "Error while opening decoder".
    let cmd = command.args(["-threads", "1", "-i"]).arg(path);
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

    #[test]
    #[ignore = "requires the private reference photo set (PHOTODEDUP_REFERENCE_SET) + ffmpeg on PATH"]
    fn heic_decode_matches_known_dimensions() {
        let Some(dir) = crate::reference::photos() else { return };
        let img = decode_image(&dir.join("IMG_3684.HEIC"), &crate::tools::ffmpeg(), Some(SchedulingHint::Utility))
            .expect("decode failed");
        assert_eq!((img.width(), img.height()), (4032, 3024));
    }
}
