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
| **Resume-recognition beyond exact path match** | `Store::find_latest_scan_for_root` (see "Resolved during M2" below) currently matches on canonicalized path only — a folder renamed, moved to a new parent directory, or re-synced to a new local path by a cloud client (OneDrive/Dropbox/iCloud routinely do this) no longer matches its own prior scan at all, even though every file inside is identical. Not fixed now because it needs a real design decision, not just more code — **explicit user requirement, recorded here so it isn't lost**: recognize the same logical folder by content, not just path | The exact-hash pass (`exacthash.rs`) already computes a blake3 digest for every size-collision bucket, and `files` already stores `(size, mtime)` per file from day one — a content fingerprint (e.g. the multiset of `(size, blake3)` for a sample of files, or file count + total size as a cheap first-pass filter before a full comparison) is additive, not a schema change. The harder part is UX, not storage — **see the suggested prompt copy right below the table.** |
| **User-tunable matching settings** (the pHash distance for splitting quick sequences, the candidate-match distance, the same-sequence time gap, and similar engine thresholds) | Today these are constants chosen by eye or from calibration on one dataset (`BURST_VISUAL_MAX_DIST`, `CANDIDATE_MAX_DIST`, `BURST_GAP_SECONDS`); the right values depend on the person's shooting style, and no single number is correct for everyone. Needs a design decision: which knobs are worth exposing, sensible limits, and a way to preview the effect (re-grouping from stored hashes needs no re-scan) so tuning isn't blind | Each is already a named constant with its calibration recorded beside it, and the sequence split runs over stored hashes, so a setting could re-group instantly without touching the rest of the pipeline. Belongs in the same Options tab as the scan-intensity setting, as a clearly marked "advanced" section |
| **Several scans alive at once** (per-folder history, switching between scans, comparing two) | For now exactly one scan is live: saving a new one deletes the previous scan, including its decisions (user's call). Needs a real design decision about when a scan may be discarded, and what a re-scan of the same folder does with the decisions already made on it (carry over by relative path?) | Every database id is a UUID, so nothing in the API assumes there is only one scan. `save_scan` is the single place that deletes the old one (`delete_all_scans`), and decisions are already keyed by relative path rather than file id, so carrying them across a re-scan is additive |

**Suggested UX for the resume-recognition item above (don't lose this when it gets built):**
Silently auto-matching on content is a real risk *specifically in a dedup
tool* — two genuinely different folders can legitimately share a lot of
overlapping photos. This must be a confirmation, never a silent
assumption, consistent with this project's never-auto-decide-without-
evidence posture elsewhere (the native-native burst rule, `T1b`'s guard,
"never delete"). Proposed dialog copy:

> This looks like your previous scan of **`<old path>`**, now at
> **`<new path>`**.
> [ Resume that scan ]   [ Start a new scan ]

Triggered when path-matching finds nothing but the content fingerprint
strongly matches exactly one prior scan. If it matches more than one
prior scan, or matches none strongly, fall through to a normal fresh
scan without asking — an ambiguous or weak signal isn't worth surfacing
as a choice.

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
  never auto-merges or auto-quarantines anything (the review UI lets the
  user decide on these pairs, with everything kept by default), so a pair landing on one side of a fixed threshold
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
    reached through a symlink still finds the same scan. **Known limit,
    tracked in the deferred table above:** path-only, so a folder that
    gets renamed or moved (a cloud sync client re-pathing it, say) won't
    be recognized even though its contents are identical.
  - `Store::load_decisions` -- bulk read of everything recorded for a
    scan, for restoring UI state in one round trip rather than one file
    at a time.
  - `GET`/`PUT /api/scans/{id}/decisions` and `GET /api/resume?root=...`
    (`routes/decisions.rs`) -- the actual HTTP surface; `resume` is
    deliberately read-only/side-effect-free, it only looks, the
    scan-trigger endpoint (below) is what actually runs the pipeline.
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

- **Scan trigger, progress streaming, cancel** (`routes/jobs.rs`).
  - `POST /api/scans/start {root, mode?}` runs the pipeline on a blocking
    thread and returns `202 {job_id}`. `mode` takes the same spellings as
    the CLI's `--mode` (`ScanIntensity` implements `FromStr`, so there is
    one parser). 400 for a bad folder or mode; 409 if a scan is already
    running.
  - **One scan at a time, deliberately:** the pipeline is throttled to be
    a polite background citizen, and two concurrent scans would defeat
    that. Only the latest job is kept in memory -- a finished scan's real
    record is its row in the store. Job ids increase, so a stale client
    can't read a newer job as its own.
  - `GET /api/jobs/{id}/events` (SSE) sends the current status at once,
    then every change, and closes right after the terminal event.
    `GET /api/jobs/{id}` returns the same status as a snapshot, for a UI
    that reconnects or reloads (`EventSource` can't replay what it
    missed). Status is `running` + phase (`scanning`, `analyzing {done,total}`
    for photos, `analyzing_videos {done,total}`, `exact_duplicates`,
    `matching`, `videos {done,total}` for matching, `clustering`),
    `done {scan_id}`, `cancelled`, or `failed {message}`.
    Progress rides a `tokio::sync::watch` channel, which keeps only the
    latest value, so a per-file callback can't flood a slow client.
  - `POST /api/jobs/{id}/cancel` sets a flag the pipeline polls
    (`pipeline::run_cancellable`); it returns 202 at once and the status
    flips to `cancelled` at the next checkpoint. 409 if the job already
    finished, 404 if it isn't the current job. A cancelled scan saves
    nothing: a half-analyzed result isn't a scan anyone could resume.
    Measured on the reference dataset: cancel took effect in under a
    second mid-`analyzing`. **Known limit:** the flag is checked per file
    during `analyzing`, per derived video during video matching, and
    between the later phases; the exact-hash pass has no internal
    checkpoints, so a cancel landing inside it waits for that pass to end.
  - **Path exception:** `root` is the one filesystem path that comes from
    a request (picking a folder is the point). It is still behind the
    session token, canonicalized, and must be an existing directory; file
    *serving* still resolves only through the database.
  - `photodedup serve` no longer requires an existing database: with a
    scan trigger, a UI can start from an empty one.
  - Verified live on the reference dataset (2718 files, release build,
    default tier): all six phases streamed, the stream closed on `done`,
    the saved scan has 182 clusters (matching the M1 parity result), and
    `/api/resume` found it. Debug builds are unusable for real-dataset
    runs (30 files in 15 minutes versus about two minutes for the whole
    set in release), so always use `--release` for those.

- **UI scaffold** (`ui/`: Svelte 5 + Vite + TypeScript). First slice is the
  start flow only: folder path -> resume lookup (offers Resume / Scan
  again for a known folder) -> live scan progress over SSE with Cancel ->
  "ready" with the cluster count (the review screen and the Options tab
  came later; see below). `photodedup serve`
  serves `ui/dist` (or `PHOTODEDUP_UI_DIR`) at `/`; the UI takes the
  session token from the URL, keeps it in `sessionStorage`, and strips it
  from the address bar. Packaged builds will embed the UI instead (M4).
  - **No native folder picker in the browser fallback:** a web page can't
    get a real filesystem path from a file dialog, so the path is typed or
    pasted until the Tauri shell (M4) supplies a native dialog.
  - **Dependency notes:** `typescript` is pinned to 6.x because
    `svelte-check` 4.7.7 doesn't accept 7 yet -- revisit when it does.
    `npm audit` reports two low-severity advisories (prototype pollution
    in `deepmerge`, via `@sveltejs/vite-plugin-svelte`, no fix available).
    Build-time only; nothing from it ships in `ui/dist`.
  - Click-through confirmed in a real browser by the user (start flow,
    scan, progress).

- **One live scan, replaced when the next one completes; UUID ids
  everywhere.** `Store::save_scan` now deletes every earlier scan (files,
  analysis, clusters, review pairs, burst runs **and decisions**) in the
  same transaction that writes the new one, so the old scan goes away only
  once the new one is fully saved. A cancelled or failed scan never reaches
  `save_scan`, and a failure partway through rolls back and leaves the old
  scan intact (tested). The generated thumbnails are cleared with it.
  - **Consequence to keep in mind:** "Scan again" on a known folder throws
    away the decisions made on it. The UI says so before it happens.
    Carrying decisions across a re-scan (they are keyed by relative path,
    so it is possible) is on the deferred table with multi-scan support.
  - **Every database id is a random UUID** (scans, files, clusters, review
    pairs, burst runs), not a serial. Found while doing the above: with
    serial ids, emptying the table makes SQLite reuse id 1, so a stale tab
    holding the old scan id would silently land on the new scan and write
    decisions into it. A UUID is never reused, so a stale id is simply
    "not found": every scan-scoped route now returns 404 for an unknown
    scan (tested), instead of an empty list or a raw foreign-key error.
    The pipeline's own `FileId` (a `u32`) stays in memory only.
  - Lists are read back in insertion order (`ORDER BY rowid`), since UUIDs
    have no order.
  - File ids arrive in URLs as a parsed `Uuid`, so a malformed or
    path-like id (`..%2F..`, `999`) is rejected before it reaches the
    database or a cache file name (tested). The thumbnail cache is named
    `<uuid>_<size>.jpg`, and `clear_cache` deletes only files with exactly
    that shape.
  - **Pre-UUID databases are reset on open** (with a message on stderr):
    the project is unreleased, a scan is regenerable, and a migration for
    throwaway development databases wasn't worth carrying. Add a real
    migration before any release that has databases worth keeping.
  - A near-miss worth recording: my first version of the cache clear
    deleted every file in whatever directory it was given, and a test
    pointed it at the system temp dir and deleted other tests' databases.
    It now matches cache file names only, and tests use a private temp
    directory.

- **Review screen** (`ui/src/Review.svelte`, `Overview.svelte`,
  `Detail.svelte`, state in `lib/review.svelte.ts`). Master-detail rather
  than a wizard: an overview grid of every cluster with a per-cluster
  state (undecided / accepted / changed / partly decided), filter chips,
  and a reviewed counter; opening a cluster shows its members side by side
  with prev/next inside the current filter, and the overview is always one
  click (or Esc) away, so earlier choices can be revisited at any time.
  - **Decisions are per file, so any number of a group's members can be
    kept, and a file marked Keep is never quarantined** -- not by a bulk
    action, not by another group's suggestion. The engine's pick is only a
    suggestion until the user acts on it. The one thing refused is
    marking *every* member of a group Quarantine: each group, and every
    other group a shared file belongs to, must keep at least one copy.
    "Accept all suggestions" only touches groups nobody has decided yet,
    behind a confirmation that says how many files it would mark for
    quarantine. Undo is multi-level, on Cmd+Z / Ctrl+Z.
  - Keyboard (all of it also has a visible control): Cmd/Ctrl+Z undoes,
    arrows or `j`/`k` move, `1`-`9` toggle keep, `Enter` accepts and
    advances, `Space` opens the large preview, `Esc` backs out, `?` lists
    them.
  - New API: `POST /api/scans/{id}/decisions/bulk` (one transaction) and
    `DELETE /api/scans/{id}/decisions?relative_path=` (back to undecided,
    for undo). Applying them (move to quarantine, with undo) is M3, below.
  - The decision logic has unit tests (`npm test`); **the screen itself is
    not yet verified in a real browser.** Still to build: virtualizing the overview grid for libraries much larger than the 182 clusters tested.

- **Review split into the MVP's categories** (tabs, each with its own
  count and one-line explanation): *Exact duplicates* (T0 clusters),
  *True duplicates* (T1/T1b clusters, video matches included), *Quick
  sequences*, *Same-device review* and *Orphan videos* (all five are
  decided the same way; see "Every category is editable" below). Checked against the reference dataset: 2 / 180 /
  269 sequences over 902 frames (was 368 time-only runs over 1332, the
  MVP's number; see below), 126 same-device pairs (the known 126-vs-125
  difference), 2 orphan videos.
  - **Quick sequences** suggest only the sharpest frame (as the MVP defaulted), any
    number of frames can be kept, and the detail view has *Keep all* and
    *Keep suggested only*. A frame that is also a duplicate group's
    suggested keeper is suggested-kept in the burst too, so thinning a
    burst can't undo a duplicate group's choice. **Thinning every burst at
    once is aggressive** (the MVP measured about 964 extra files / 4.8 GB
    on this dataset), so "Accept all suggestions" shows how many files it
    would mark before it does anything.
  - **Same-device review excludes burst-tier pairs** (534 of the 660
    stored review pairs): those are neighbours inside a burst run, which
    the Burst tab already shows.
  - **Orphan videos persist** (new `orphan_videos` table), in two labelled
    sections: *no original found* (the 2 re-encoded clips) and *source not
    identified*. The pipeline had always computed both lists but never
    saved them.
  - **Video matching reports progress** (`videos {done,total}`, counted in
    derived videos finished) and checks for cancellation per video. The
    bar was previously empty for the whole video phase.
  - **Engine gap, now fixed: iPhone videos were classified `unknown`.**
    Apple writes `com.apple.quicktime.creationdate` with the offset
    lacking a colon (`+0900`), which RFC 3339 parsing rejects, and the
    code took the first tag *present* rather than the first that *parsed*,
    so `creation_time` beside it was never tried. With no timestamp, make
    + model + time ("intact camera identity") never held, and 8 genuine
    iPhone Air clips fell to `unknown` at 0.12 confidence: never compared,
    and unmatchable as originals. Both are fixed (`metadata.rs`), with a
    unit test over every timestamp form and the real-file test now
    asserting an iPhone clip classifies as a camera original.
    **Parity baseline after the fix, unchanged:** 2 exact, 180 true
    duplicates, 6 video matches (4 high-confidence), 2 orphan videos, 126
    same-device pairs. The "source not identified" section of the Orphan
    videos tab is now empty on this dataset (it stays, for videos whose
    metadata really is missing). A full scan took 250 s in that run, but
    other ffmpeg work was running at the same time, so treat it as an
    upper bound, not a measurement.

- **"Burst" runs are now split by what the photos look like, and renamed
  "Quick sequences".** Two problems, both raised from real use:
  - *They aren't camera bursts.* A run is just photos from one camera
    within `BURST_GAP_SECONDS` (3 s) of each other, and on this dataset
    they were taken by hand in quick succession, not in the camera's burst
    mode. The tab is renamed and its text says how sequences are found.
    The internal names (`burst_runs`, `Tab = 'burst'`) are unchanged.
  - *A run mixed unrelated shots* (a portrait, a full-body shot, a
    different pose), so "keep the sharpest of the run" was misleading. A
    quarter of consecutive frames inside time-only runs were at
    chance-level difference (110+ of 256 bits). Each run is now split into
    groups of frames that look alike (`split_runs_by_similarity`, single
    linkage at pHash distance <= `BURST_VISUAL_MAX_DIST` = 80, regrouping
    by look rather than adjacency, so portrait / full-body / portrait
    becomes two groups). Groups need two or more frames; a frame left
    alone is simply kept, as is a frame with no hash.
  - **The threshold is a judgment call, not a derived number.** Distances
    inside runs form a continuum (0 to 150) with no natural gap, so it was
    chosen by looking at contact sheets of real runs: at 80 a nine-frame
    run of one pose stayed whole, a full-body shot and a waist-up portrait
    on the same stairs separated, and a wide shot plus five different
    portrait poses fragmented (intended: different poses are different
    groups); at 60 same-pose runs over-split, and past 90 it nears the
    100-134 range of unrelated photos. Over-splitting is the cheap error
    (a frame is kept), under-splitting is the misleading one. pHash is
    coarse for "same shot, subject moved", so some near-identical frames
    after a small reframe will still separate. A sturdier similarity
    measure (local features, or embeddings) would help; that is a larger
    piece of work and isn't planned yet.
  - **Parity baseline changed on purpose:** burst runs were 368 runs over
    1332 files (exact match with the MVP, time-only); they are now **269
    sequences over 902 frames**. Every sequence still has a sharpest-frame
    suggestion. Tests pin the splitting rules (look-alike
    stays together, different shot splits, alternating shots regroup, a
    lone or hash-less frame is dropped, the threshold is inclusive, a
    slowly changing sequence chains).

- **"Include videos" flag.** A scan can leave videos out entirely
  (`PipelineOptions::include_videos`, default on; `include_videos` in the
  scan request; `--no-videos` on the CLI; a checkbox on the start screen,
  remembered in the browser). Off means videos, and any sidecar travelling
  with one, are dropped from the scan report before anything downstream
  can touch them: not listed, not hashed in the exact-duplicate pass (video
  files are the big ones), not probed, not matched. Checked on the
  reference dataset (456 of 2718 files are videos): 2262 files scanned,
  video matches and orphans 0, true duplicates 175 (the 5 video clusters
  gone), sequences and same-device pairs unchanged at 269 / 902 and 126.
  - A saved scan doesn't record whether videos were included, so the Orphan
    videos tab just says "No orphan videos" either way. Recording it per
    scan (so the UI can say "videos weren't scanned") needs a schema
    change; not done yet.
  - The checkbox stays on the start screen on purpose (user's call): it is
    a choice about *this* scan, not a standing preference, so it does not
    belong in Options.
  - Timing is not claimed: the 179 s no-video run is only comparable to a
    run with videos made on an otherwise idle machine, and the one I have
    (250 s) wasn't.

- **Every category is editable, and the overview is sortable by date.**
  - *Editable:* Same-device review and Orphan videos used to be browse-only
    (as in the MVP). They are now decided like the rest, with **everything
    kept by default**: nothing in them is suggested for removal, so
    "Accept suggestion" there means "keep all". A pair is one group of two
    (both photos shown on its card); an orphan video is a group of one.
  - *The keep-at-least-one rule* applies to groups of two or more, so one
    photo of a pair can be dismissed but not both. A lone orphan video has
    no other copy, so quarantining it is simply the user's explicit call.
    Nothing is deleted either way; quarantine is a move, applied in M3.
  - *Sorting:* oldest first (default) or newest first, remembered in the
    browser, applied to every tab and to prev/next in the detail view.
    Groups with no date go last in both directions, and groups with the
    same date keep the engine's order. The date is derived from the
    **original**: a cluster takes its suggested keeper's capture time
    (falling back to the earliest any member has), a sequence its earliest
    frame, a pair the earlier of its two, because a re-saved or forwarded
    copy carries the date of that save, not of the shot. Checked on the
    reference dataset: all 182 clusters, 269 sequences and 660 pairs are
    dated; the 2 orphan videos (re-encoded clips) have no capture time and
    sort last.
  - **Known imprecision:** image times are camera-local wall-clock time
    with no zone, while video times come from UTC `creation_time`, so a
    video and a photo from the same moment can differ by the time-zone
    offset (9 h in Japan) when sorted together. Within one kind, order is
    exact. Normalizing needs the offset of the shot (GPS or the Apple
    `creationdate` offset); not done.
  - Cards and the detail header show the date. The sort choice stays on the
    review screen on purpose (user's call): it is a way of looking at the
    current review, not a standing preference.
  - **Photos and videos are separate phases with separate counts.** The
    scan screen used to say "N of M photos and videos", which was wrong
    however it was worded: they aren't processed together (photos first,
    then each video's metadata, then video matching) and they cost very
    differently. The engine now reports `analyzing` for photos only, then
    `analyzing_videos`, then `videos` (matching); the screen says "N of M
    photos" or "N of M videos" accordingly, and when videos are excluded
    neither video phase is announced.

## M3: applying decisions (quarantine, undo)

The first thing in this project that moves a user's files, built around two
promises the user asked for explicitly: **the app never leaves the folder it
was pointed at, and never makes a change that can't be undone.**

- **One guarded door** (`core/src/fsguard.rs`). Nothing else in the engine
  changes the filesystem. A `Workspace` is a canonicalized root; every path
  is a *validated relative path* (plain names only: no `..`, no root or
  drive prefix, no empty or NUL, nothing inside a managed library, `.git`
  or our own quarantine) checked component by component to contain no
  symlink. The only operations: move a file into the quarantine folder,
  move it back, create folders, write a manifest inside a batch folder.
  **There is no delete and no overwrite**: a move onto an existing path is
  refused in both directions. The one `remove_file` is the tail of a
  cross-volume move, after the copy is verified by hash, so the net effect
  is still a move. Journal rows are re-validated on every use, so a
  tampered database can't redirect a move or an undo.
- **Quarantine layout:** `<root>/.photodedup-quarantine/<UTC timestamp>/`,
  preserving each file's relative path, with `manifest.csv` and
  `manifest.json` beside the files so a batch can be understood and undone
  by hand. The scanner skips this folder, so a re-scan never mistakes
  quarantined copies for library files.
- **Dry run first** (`apply::plan`): read-only. Only files the user
  explicitly marked Quarantine are candidates; undecided files and anything
  marked Keep are never planned. Companions (RAW, XMP, the video of a Live
  Photo) are planned with their photo; sibling sets are now saved in the
  database for that. Refused, with a reason shown: a group of two or more
  where every member is marked (the UI prevents it, the engine doesn't
  trust the UI), cloud-only placeholders, unsafe paths.
- **Journalled apply** (`apply::execute`): each file is re-checked against
  what the scan saw (size and modification time; a changed, vanished or
  link-swapped file is skipped and reported, not moved), the **whole batch
  is written to the journal before anything moves**, then files move one at
  a time, each marked `moved` as it lands. Needs `{"confirm": true}` over
  HTTP. Applying twice moves nothing twice and leaves no empty batch.
- **Undo** (`apply::undo`): restores a batch, never overwriting; if
  something now sits at a file's original place that file stays in
  quarantine and is reported, and undo can be repeated. It works after a
  re-scan, because the journal tables (`apply_batches`, `apply_items`)
  deliberately have no link to scans.
- **Crash recovery** (`apply::recover`, run at server start): a batch left
  `running` is reconciled with what is actually on disk (in quarantine and
  gone from its place = moved; still in place = not moved; anything else is
  flagged for a human, not guessed at).
- **Reads are contained too:** the thumbnail, preview and original routes
  open files through the same guard, so a file replaced by a link to
  somewhere else after the scan can't be used to read outside the folder
  (tested over HTTP). A scan is refused if the database's thumbnail cache
  would sit inside the folder being scanned.
- **UI:** an *Apply (N)…* button opens a panel with the dry-run list (and
  anything refused), an explicit "Move N files to quarantine" confirmation,
  and a list of previous batches with Undo. After a move the review shows a
  notice that it no longer matches the folder; scan again to refresh.
  **CLI:** `apply [db] [--commit]` (dry run by default), `batches`, `undo`.
- **How it was tested:** adversarial unit tests (traversal strings,
  symlinked folders and files, a symlinked quarantine folder, forged
  journal rows, pre-existing destinations, changed/vanished files, a
  simulated crash, 20 randomized apply/undo round trips that must restore
  the folder exactly) -- and the tests were themselves checked by
  deliberately breaking three guards (allowing `..`, allowing overwrite,
  following symlinks) and confirming tests fail; one test that passed for
  the wrong reason was found that way and fixed. Plus a real round trip on
  a scratch copy of 28 reference photos (including a re-scan between apply
  and undo): sha256 and modification time identical afterwards, reference
  dataset untouched.

**Known limits, not hidden:**
- *Windows:* the plan's long-path (`\\?\`) prefix, reserved device names and
  trailing dots/spaces are **not implemented**, and none of this has been
  run on Windows. The symlink tests are Unix-only.
- *The cross-volume copy-verify-delete path* is unit-tested on one volume
  (copy, hash check, cleanup on failure) but has not run across two real
  volumes. In practice the quarantine folder lives inside the scanned
  folder, so it is rarely needed.
- *Verification is size + modification time, not a re-hash.* The plan said
  "re-hash if cheap"; a same-size, same-second edit would not be noticed.
- *A time-of-check/time-of-use race* (a folder swapped for a link between
  the check and the rename) is not defended against; each check is repeated
  immediately before the operation it guards. Single-user tool, own folder.
- *Apply holds the database lock while it moves files*, so other requests
  wait for the batch; fine for renames, slow for a huge cross-volume copy.
  No progress is reported during an apply.
- After a full undo the batch folder keeps its two manifests. Harmless, and
  a record of what happened, but it does leave a small folder behind.
- Re-scanning replaces the scan and the decisions made on it (see the
  single-live-scan entry), so files still in quarantine from a batch can
  be undone but any decisions not yet applied are lost.

## Options tab

Only *permanent* preferences live here (user's decision): per-scan choices
(**Include videos**, on the start screen) and view choices (**sort order**,
on the review screen) stay where they are used.

- **Scan speed** (Efficient / Default / Performance) is a standing
  preference, saved in the database's `settings` table (not in the browser,
  so it follows the data and works however the app is opened, including the
  future desktop shell), via `GET`/`PUT /api/settings`. Each tier shows what
  it really does on this machine (worker counts, 2 / 4 / 12 on the 12-core
  development Mac) with the measured trade-offs worded as measurements, not
  promises: Efficient about 8x less processor power than Default in testing
  on Apple Silicon but much slower; Default polite; Performance fast and
  hot. It saves the moment a choice is made.
- **Precedence when a scan starts:** a mode named in the request (scripts,
  tests) wins, then the saved preference, then Default. A corrupt stored
  value is ignored, never trusted. The CLI `--mode` flag is unchanged.
- The start screen shows which speed the next scan will use, with a link to
  Options.
- **Still deferred, and meant to live here later** as an "advanced" section:
  the matching thresholds (see the deferred table). The `settings` table is
  a plain key/value store, so adding one is a new key and a row in the UI.

## Test folder

`~/photodedup-test/` (outside the repo, outside every real folder) holds a
52-file copy of real photos and videos covering every review tab, plus
`reset.sh` to rebuild it and clear its database, and a README with the
steps. Scan result for the folder: 2 exact, 8 true duplicates (7 photo
groups + 1 video group), 3 sequences over 11 frames, 9 look-alike pairs, 2
orphan videos, 1 companion sidecar.

