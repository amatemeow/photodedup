# M0 spike results

Run 2026-10-27 against the real Japan trip dataset
(`~/photo-dedup/originals/`, 2718 files) with Homebrew ffmpeg 9.0.2 on macOS.
These three answers were blocking before writing the real engine — see
`ROADMAP.md` and the architecture plan for what they decide.

## 1. HEIC decode via ffmpeg — works; exiftool/libheif-rs avoided

`ffmpeg -i in.heic -frames:v 1 -update 1 out.png`, **with no explicit `-map`**,
correctly decodes the full-resolution image. Verified on 6 files (iPhone
4032×3024 and 5712×4284 originals): output dimensions matched inventory
EXIF exactly, including portrait rotation applied automatically (matches
Pillow's `exif_transpose`). Visual spot-check against the Pillow-generated
thumbnail for `IMG_3684.HEIC` confirmed identical content.

**Gotcha, worth encoding as a comment in the real decoder:** modern iPhone
HEIC files are multi-stream — a full-res tile grid (48 tiles for a 4032×3024
shot), a separate depth-map tile grid, and a 10-bit HDR variant, each its own
"stream group". **Explicitly mapping a stream (`-map 0:0`) grabs a single
512×512 tile — a nearly-black fragment of the image, not the composite.**
Explicitly mapping a stream group (`-map 0:g:0`) dumps all 48 tiles as
separate frames instead of assembling them. The correct invocation is the
*default* stream selection (no `-map` at all) — ffmpeg's own HEIF demuxer
picks the group flagged `(default)` and assembles it internally. This cost
most of the spike's time and will cost real debugging time again if an
AI-assisted pass "cleans up" the invocation to add an explicit `-map`.

**Decision:** drop `libheif-rs` from the dependency list entirely. This also
resolves the LGPL-3.0 static-linking concern raised in the architecture
plan — ffmpeg is invoked as a subprocess regardless, so there's no linking
question for HEIC either.

## 2. Video vendor metadata via ffprobe — works, no exiftool needed

The MVP needed exiftool *without* `-fast2` specifically because OnePlus
video vendor tags live outside what `-fast2` reads. `ffprobe -show_format`
surfaces them directly, with more detail than the MVP's exiftool pass used:

OnePlus 15 (`VID20261020154635.mp4`):
```
com.oplus.product.model        OnePlus 15
com.oplus.lens.model           back_main
com.oplus.lens.focal_length    23.477999mm
com.oplus.lens.max_aperture_value  f/1.88
creation_time                  2026-10-20T06:46:39.000000Z
location                       +34.6604+135.5059/
```

iPhone Air (`IMG_3230.MOV`):
```
com.apple.quicktime.make           Apple
com.apple.quicktime.model          iPhone Air
com.apple.quicktime.creationdate   2026-10-15T08:31:38+0900
com.apple.quicktime.location.ISO6709   +35.0081+135.7901+070.811/
```

**Decision:** exiftool is dropped entirely. `ffprobe -print_format json
-show_format -show_streams` (same ffmpeg sidecar already bundled for frame
extraction) covers both image and video metadata extraction; `kamadak-exif`
remains for the hot path on plain JPEG/TIFF where shelling out per-file
would be wasteful. This removes the GPL/Artistic-dual-licensed, ~60 MB
Windows dependency the architecture plan flagged as the heaviest and most
license-awkward piece of the MVP.

## 3. pHash/dHash distance-band parity — confirmed, closer than required

Reimplemented `imagehash.phash(img, hash_size=16)` and
`imagehash.dhash(img, hash_size=16)` from scratch in Rust (`image` crate for
decode/resize, hand-rolled DCT-II to match `scipy.fftpack.dct` type=2 exactly,
Pillow's ITU-R 601-2 luma coefficients instead of the `image` crate's
default Rec. 709 ones — this substitution matters for parity). Ran against
three pairs with known Python-computed ground truth, spanning the
calibrated bands from `PROJECT_CONTEXT.md`:

| pair | Python phash | Rust phash | Python dhash | Rust dhash |
|---|---|---|---|---|
| true duplicate (`IMG20261015084747.jpg` / `IMG_3465.JPG`) | 0 | **0** | 0 | **0** |
| burst sibling (`IMG20261019101441.jpg` / `...101442.jpg`) | 30 | **30** | 29 | **31** |
| native/native review (`IMG_4045.HEIC` / `IMG_4049.HEIC`) | 44 | **42** | 33 | **28** |

phash landed exact or within 2; dhash within single digits. Every pair stays
in the same classification band — the acceptance bar the plan set (bands,
not bit-exact hashes, since resize filters differ) is cleared with margin.
These three pairs are now encoded as permanent regression tests in
`crates/core/src/hash.rs`.

## Net effect on the dependency list

Both spikes removed a dependency rather than needing a new one:
**exiftool and libheif-rs are out.** The only native (non-pure-Rust)
dependency is the ffmpeg sidecar, used for HEIC and video frame decode —
exactly what was already being bundled for video.
