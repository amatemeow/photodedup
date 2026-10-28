# Roadmap

This is the plan of record for work explicitly deferred past the initial
trip-folder-scale app. It is maintained, not a one-time note — update the
"Door left open by" column whenever an implementation choice changes it.
Full architecture and milestone plan: see the session's approved plan at
commit time (M0–M4) and `SPIKES.md` for the M0 feasibility results that
shaped these decisions.

| Item | Why deferred | Door left open by |
|---|---|---|
| **Library scale, 100k+ files** | Blocked search (4×u64 XOR+popcount under rayon) reaches ~50k files in a few seconds with no index; trip-sized (thousands) is the stated first target | `CandidateIndex` trait (`crates/core/src/search.rs`) — swap the brute-force impl for multi-index LSH (256 bits → 16-bit bands) or a BK-tree without touching anything above it |
| **Bounded thumbnail cache** | 219 MB for 2273 files in the MVP does not extrapolate (≈10 GB at 100k files) | Content-addressed thumb store lives behind one module; serve 256px grid thumbs + on-demand 1024px previews from day one so the cache policy is a localized change |
| **Incremental rescan** | Not needed for one-shot trip folders | SQLite schema records `(size, mtime, inode/file-id)` per file from day one, even though nothing reads it yet |
| **Managed libraries** (Apple Photos `.photoslibrary`, Lightroom catalogs) | Moving a file out from under a managed library corrupts its database — genuinely risky, not just unimplemented | Day one **detects and hard-refuses** to touch these trees; the detector is the hook, support is the roadmap item |
| **External drives / NAS over SMB** | Needs disconnection-survivable apply and throttled, resumable hashing — its own engineering effort | Journalled apply and cross-volume copy-verify-delete already ship in M3; SMB only adds retry/resume on top |
| **AVIF** | Not produced by either phone in the reference dataset | `rav1d` (pure Rust, AV1 is royalty-free) drops into the decoder dispatch next to `zune-jpeg`/`png` |
| **Cross-device matching** | Measured zero hits in the 2718-file reference dataset — two people framing the same scene apparently never hashes close enough | The T3 pair classification tier already exists; it simply never fires on this dataset |
| **Paid macOS notarisation** | $99/yr Apple Developer Program; a Homebrew Cask (free, strips the quarantine xattr) covers most of the same friction | CI signing step is parameterised, not hardcoded to ad-hoc-only, so flipping it on later is a config change |
| **Windows code signing (SmartScreen)** | OV certs now require hardware tokens/HSM; real cost | SignPath.io's free OSS tier is the planned path if users report friction; artifact build is already reproducible via GitHub Actions, which SignPath needs |
| **Windows background-scheduling equivalent** (`SetPriorityClass(..., BELOW_NORMAL_PRIORITY_CLASS)`, `THREAD_MODE_BACKGROUND_BEGIN`) | No Windows target to verify against yet; didn't want a `windows-sys` dependency added speculatively | `platform::noop::NoopScheduler` is the only thing that needs replacing — every caller already goes through the `PlatformScheduler` trait (`crates/core/src/platform/`), untouched by adding a real Windows impl |
| **T1 cluster / same-device-review-pair count, off by one vs. the MVP** | 179 vs MVP's 180 T1 clusters, 126 vs 125 review pairs, consistently reproducible on the reference dataset but not yet root-caused | Not yet — needs a direct diff against the MVP's `t1_groups.json`/`pairs_t2.json` to find which single pair classifies differently and why, before M1 can claim exact parity rather than "very close" |

## Resolved during M0 (moved out of "deferred", not into the table above)

- **exiftool** — dropped entirely. `ffprobe` (part of the already-bundled
  ffmpeg) covers everything it was used for, including vendor-specific
  video tags (OnePlus `com.oplus.product.model`, Apple
  `com.apple.quicktime.model`) — see `SPIKES.md` §2.
- **`libheif-rs`** (and its LGPL-3.0 static-linking question) — dropped.
  ffmpeg's own HEIF demuxer decodes HEIC correctly via subprocess, so
  there's no linking decision to make. See `SPIKES.md` §1 for the
  stream-group gotcha that made this non-obvious.

## Resolved during M1

- **`T1b` (derived↔derived auto-dedup, no surviving original)** —
  implemented in `classify.rs`. The MVP sent this case to review-only; the
  generalized `Origin` provenance model makes it a safe auto-dedup (keep
  the larger) when no camera-original survives in the pair.
- **Full pipeline + SQLite persistence** — `scan → metadata → provenance →
  hash → candidate search → classify → resolve → cluster → keeper →
  store`, all in `crates/core/src/`, parity-validated against the
  reference dataset (see the M1 section of project memory for exact
  numbers). `store.rs` persists scan results keyed by relative path, not
  the pipeline's ephemeral per-run `FileId`, so results and user decisions
  survive across re-scans.
- **Resource throttling / background-friendliness** — genuinely resolved,
  not just attempted, after three real measurement rounds (`sudo
  powermetrics`, not assumption) on the reference 8-performance+4-
  efficiency-core machine:
  1. A thread-count cap alone did nothing for heat — core *type*
     placement, not core *count*, drives power draw on Apple Silicon.
  2. `QOS_CLASS_UTILITY` set on Rust worker threads did nothing either,
     because **almost all of this engine's CPU work happens inside
     ffmpeg/ffprobe child processes**, and thread-level QoS does not
     propagate across `fork`+`exec`. Fixed via `pre_exec`, setting QoS in
     the child before `exec()` (`platform::PlatformScheduler::
     configure_child_process`).
  3. Even with the child-process fix, `Utility` left one P-core cluster
     at ~75-99% utilization under real load — it lowers priority but
     doesn't hard-confine to efficiency cores under sustained demand.
     `QOS_CLASS_BACKGROUND` measurably did (P-cores parked at 0MHz,
     E-cluster saturated, ~8x lower CPU package power: 26-28W → 3.4W) —
     at real, substantial wall-clock cost.
  Net design: `ScanIntensity::Economy`/`Default`/`AllIn` (user-facing
  `--mode` flag) map to `Background`/`Utility`/no-hint respectively
  (`throttle.rs`), via a `PlatformScheduler` trait with one implementation
  per OS resolved once at startup (`platform/`) rather than scattered
  `#[cfg(...)]` blocks. Windows has no scheduling impl yet (see the
  roadmap table above); POSIX/Linux differentiates tiers via nice value
  only (no QoS-class equivalent exists at that API level).
