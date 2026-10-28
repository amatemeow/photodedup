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
| **Incremental rescan** (detect new/changed files in an already-scanned folder and merge them into the existing scan, without re-hashing everything) | Not needed for one-shot trip folders. Note this is narrower than it sounds now that resume exists (see "Resolved during M2" below): reopening an *unchanged* folder already resumes instantly with no rescan at all -- this item is specifically about a folder that *gained or changed* files since its last scan | SQLite schema records `(size, mtime, inode/file-id)` per file from day one, even though nothing reads it yet |
| **Managed libraries** (Apple Photos `.photoslibrary`, Lightroom catalogs) | Moving a file out from under a managed library corrupts its database — genuinely risky, not just unimplemented | Day one **detects and hard-refuses** to touch these trees; the detector is the hook, support is the roadmap item |
| **External drives / NAS over SMB** | Needs disconnection-survivable apply and throttled, resumable hashing — its own engineering effort | Journalled apply and cross-volume copy-verify-delete already ship in M3; SMB only adds retry/resume on top |
| **AVIF** | Not produced by either phone in the reference dataset | `rav1d` (pure Rust, AV1 is royalty-free) drops into the decoder dispatch next to `zune-jpeg`/`png` |
| **Cross-device matching** | Measured zero hits in the 2718-file reference dataset — two people framing the same scene apparently never hashes close enough | The T3 pair classification tier already exists; it simply never fires on this dataset |
| **Paid macOS notarisation** | $99/yr Apple Developer Program; a Homebrew Cask (free, strips the quarantine xattr) covers most of the same friction | CI signing step is parameterised, not hardcoded to ad-hoc-only, so flipping it on later is a config change |
| **Windows code signing (SmartScreen)** | OV certs now require hardware tokens/HSM; real cost | SignPath.io's free OSS tier is the planned path if users report friction; artifact build is already reproducible via GitHub Actions, which SignPath needs |
| **Windows background-scheduling equivalent** (`SetPriorityClass(..., BELOW_NORMAL_PRIORITY_CLASS)`, `THREAD_MODE_BACKGROUND_BEGIN`) | No Windows target to verify against yet; didn't want a `windows-sys` dependency added speculatively | `platform::noop::NoopScheduler` is the only thing that needs replacing — every caller already goes through the `PlatformScheduler` trait (`crates/core/src/platform/`), untouched by adding a real Windows impl |
| **Scan-intensity (Efficient/Default/Performance) as a persistent app setting** | No UI exists yet (M2+) — currently a `--mode` CLI flag, set fresh per invocation | `ScanIntensity` (`throttle.rs`) is a small `Copy` enum with no state of its own; M2's settings/options surface needs only to read a stored value and pass it into `PipelineOptions`, not touch the scheduling logic itself. **Explicit user requirement, recorded here so it isn't lost before M2 design starts:** this belongs in the app's persistent Options/Settings tab (a standing preference the user sets once), not a per-scan prompt or CLI-only flag — the `--mode` flag stays for scripting/testing, but isn't the primary UX once a UI exists |

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
  Net design: `ScanIntensity::Efficient`/`Default`/`Performance` (user-
  facing `--mode` flag; named to match the convention most apps already
  use for this tradeoff) map to `Background`/`Utility`/no-hint respectively
  (`throttle.rs`), via a `PlatformScheduler` trait with one implementation
  per OS resolved once at startup (`platform/`) rather than scattered
  `#[cfg(...)]` blocks. Windows has no scheduling impl yet (see the
  roadmap table above); POSIX/Linux differentiates tiers via nice value
  only (no QoS-class equivalent exists at that API level).
- **T1-cluster-count discrepancy (179 vs. MVP's 180), root-caused and
  fixed.** Diffing actual cluster membership (not just counts) against
  `~/photo-dedup/decisions_input.json` found one real bug: two messenger-
  derived copies of two *different*, burst-adjacent native photos
  (`IMG20261016074102.jpg`/`...074103.jpg`, 1s apart, same camera) hash
  close enough to each other (d=44) to fire `T1b` -- but each of those two
  derived files already has its own high-confidence native original (d=0
  and d=2). Un-guarded, that `T1b` edge bridged two correct, separate `T1`
  clusters into one incorrect four-file cluster. This is the exact
  burst-vs-duplicate ambiguity the native-native structural rule exists to
  prevent, just unguarded on the derived side (derived files usually lack
  the timestamp that rule depends on). Fixed in `resolve_t1b_edges`
  (`resolve.rs`): a derived file that already resolved to a native
  original via `T1` is excluded from `T1b` matching entirely -- it isn't
  "orphaned," so it must not bridge into an unrelated cluster through a
  weaker link. Regression-tested with the exact file pair. **Result:
  cluster membership now matches the MVP's `decisions_input.json`
  exactly, 182/182, zero diff** (confirmed by full set-diff, not just
  count).
- **Same-device-review-pair count (126 vs. MVP's 125), explained --
  not a bug.** All three differing pairs sit within 1-4 pHash bits of
  `CANDIDATE_MAX_DIST = 45` (confirmed: Python 44/46/48 vs. Rust's
  independently-computed 46/44/44 for the same three pairs), crossing the
  threshold on opposite sides between the two implementations. This is
  the accepted cost of the M0 design decision to match pHash **distance
  bands**, not bit-exact hashes (Pillow's and the `image` crate's Lanczos3
  resize aren't bit-identical implementations -- see `/SPIKES.md` §3).
  Unlike the `T1b` case, this has no safety implication: `T2NativeReview`
  is browse-only, nothing auto-merges or auto-quarantines based on
  membership in it, so a pair landing on one side of a fixed threshold
  instead of the other is cosmetic, not a correctness bug. Not planned to
  be "fixed" -- doing so would require bit-identical resize filters between
  Rust and Python, which M0 already judged not worth pursuing.

## Resolved during M2

- **Mid-review persistence / resume-after-quit.** Prompted by a direct
  question: if a user reviews part of a dataset and quits, can they pick
  up where they left off? Investigating turned up a real, concrete gap --
  the `decisions` table existed and was correctly keyed (`scan_id` +
  stable relative path, not the pipeline's ephemeral per-run `FileId`),
  but nothing exposed it over HTTP (no save endpoint, no read-back
  endpoint), and every `scan` invocation created a brand-new `scan_id`
  with no way to recognize "this folder was already scanned." Fixed:
  - `Store::find_latest_scan_for_root` -- the lookup behind resuming:
    matches on a canonicalized root path, so a trailing slash or a path
    reached through a symlink still finds the same scan.
  - `Store::load_decisions` -- bulk read of everything recorded for a
    scan, for restoring UI state in one round trip rather than one file
    at a time.
  - `GET`/`PUT /api/scans/{id}/decisions` and `GET /api/resume?root=...`
    (`routes/decisions.rs`) -- the actual HTTP surface; `resume` is
    deliberately read-only/side-effect-free, it only looks, a future
    scan-trigger endpoint is what actually runs the pipeline.
  - Design decision (user's call, given three options): opening the app
    on a recognized folder **auto-resumes the last scan** rather than
    silently rescanning or asking the user to pick from a list. An
    explicit rescan stays available as a distinct action.
  Verified live against the real reference dataset, not just unit tests:
  started a server, made real decisions via the API, killed the process
  entirely (a genuinely new PID, a fresh random auth token -- no
  in-memory state carried over), started a fresh server instance, and
  confirmed both the resume lookup and all three decisions survived
  intact. 7 new tests (4 store, 3 server) alongside that live check.
