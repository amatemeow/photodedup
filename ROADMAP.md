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
| **More tunable settings** (the video-matching distances: duration tolerance, frame-match distance) | The three image-matching sliders now preview and apply live (see "Live sliders"). Video matching is the one stage that still needs the files: it extracts frames with ffmpeg, so its distances cannot be regrouped from saved data. They would need the per-pair frame distances saved, after which they could join the same mechanism | The thresholds are one `Thresholds` struct with named ranges (`core/src/thresholds.rs`) passed explicitly through the engine, and the `settings` table is a plain key/value store, so another knob is a field, a range and a row in the UI |
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

## Settings tab (called Options until the UI pass)

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
- **Advanced matching** (a collapsed group of three sliders, each with its
  own range, a "reset to default", and "reset all"; changes apply to the
  **next scan**, which the screen says):
  - *Sequence similarity* (pHash distance, 40-100, default 80): how alike
    shots must look to stay in one quick sequence.
  - *Sequence time gap* (1-30 s, default 3): how close in time shots must
    be to start a sequence. It also decides whether two similar shots are
    "in a sequence" or listed under Same-device review.
  - *Copy match distance* (25-60, default 45): how different two photos may
    look and still be checked as possible copies. The riskier one: a higher
    value finds more copies and can suggest wrong ones, so its range stops
    well short of where unrelated photos begin (100+ of 256 bits), and the
    screen says so. **No setting can weaken the structural rule that two
    untouched camera photos are never suggested as copies of each other** --
    that is not a number.
  - **Safety of the values themselves:** limits live in one place
    (`thresholds.rs`) and are enforced on every request; a request is
    checked as a whole (a refused request saves none of its fields), and a
    stored value that is missing, unparseable or out of range falls back to
    its default rather than being trusted.
  - **Wiring is not optional:** the old global constants were removed and
    the values are required parameters (`classify_pair`,
    `detect_burst_runs`, `PipelineOptions::thresholds`), so no code path can
    quietly ignore a setting.
  - **Checked on a pristine copy of the test library** (the 51 files from
    `files.txt`): defaults give 3 sequences over 11 frames, 9 look-alike
    pairs and 10 duplicate groups; strictest similarity gives 2 sequences
    over 7 frames; a 1 s gap gives 2 over 5 frames; a 30 s gap gives 7 over
    21 frames and moves every pair into a sequence; match distance 25 leaves
    2 look-alike pairs and 60 gives 11. **Duplicate groups stayed at 10
    throughout**, and restoring the defaults reproduced the first result
    exactly.

## Test folder

`~/photodedup-test/` (outside the repo, outside every real folder) holds a
52-file copy of real photos and videos covering every review tab, plus
`reset.sh` to rebuild it and clear its database, and a README with the
steps. Scan result for the folder: 2 exact, 8 true duplicates (7 photo
groups + 1 video group), 3 sequences over 11 frames, 9 look-alike pairs, 2
orphan videos, 1 companion sidecar.

*Test-folder note:* pointing the app at `~/photodedup-test` itself (rather
than its `library/` folder) works, and creates the quarantine folder one
level up. That is the guard doing what it should, since the "workspace" is
whichever folder you scan. It happened during first use: 18 files moved,
all accounted for in the manifest, 34 + 18 = the 52 that were created.

### Making the sliders testable (after first use)

First feedback: "hard to tell if sliders work on the test dataset". The cause
was the loop, not the sliders: nothing on screen changed when a slider moved,
re-scanning threw away your choices, and on a 51-file folder the swings are
small. Fixed on three fronts:

- **Each scan records what it was made with** (`scan_settings` table, one row
  per scan; `GET /api/scans/{id}/info`). The review shows, under each tab's
  description, the settings that shaped that tab ("Found with sequence
  similarity 80 and sequence time gap 3 s."). When the saved settings differ
  from the scan's, a banner lists exactly what moved ("sequence time gap:
  3 s -> 10 s") and offers **Scan again**, which repeats the scan on the same
  folder with the same include-videos choice.
- **Re-scanning keeps your choices.** A re-scan of the same folder carries
  over each keep/quarantine decision whose file is still there and looks
  exactly as it did (same size and modification time). A file that was edited
  or removed, or a different folder, inherits nothing, because the choice was
  about a file that is no longer that file. This also removes the earlier
  wart that "Scan again" threw away a review in progress. (It reverses one
  line of the single-live-scan entry above: decisions are no longer lost on a
  re-scan of the same folder.)
- **A second test folder built to show the sliders**, `~/photodedup-slider-demo/`
  (46 real photos, `reset.sh`, README). Its README has a table of what each
  tab should read at each slider position, measured with the engine on those
  exact files: e.g. Same-device review goes 0 / 5 / 7 / 12 / 14 as copy match
  distance goes 25 / 35 / 45 / 55 / 60, and the photos in Quick sequences go
  14 / 24 / 28 / 30 as similarity goes 40 / 60 / 80 / 100.
- **Finding the right test photos was itself informative:** real same-device
  look-alike pairs are 4-30 seconds apart (none within 70 fingerprint bits
  are further apart than that), i.e. just outside the default 3 s sequence
  gap. So the time-gap slider is also what moves a pair between "look-alike"
  and "in a sequence" (10 look-alike pairs at a 1 s gap, 0 at 30 s).
- A near-miss, caught before it ran: my first version of recording the scan
  settings locked the database twice inside one `match`, which would have
  deadlocked every scan at its end. The save is now a small function that
  takes and releases the lock per step.

*User verdict after trying the sliders:* the settings do change things, but
they will only be genuinely usable once moving a slider shows its effect
live (a preview, then applying it without a re-scan). Recorded as the top
priority after M4; the groundwork is in place (settings recorded per scan,
stale-results banner, choices kept across a re-scan, per-slider ranges),
and the stored analysis already holds what a regroup needs.

### Live sliders (preview while dragging, apply on release)

Top priority after M4, built on the groundwork above. Moving a matching slider
now shows what it would change in the current review, and letting go applies
it, with no re-scan. Choices are kept.

- **The grouping half of the pipeline is one pure function**
  (`core/src/grouping.rs`, `group(input, thresholds)`): candidate search,
  classification, the tier resolution, union-find, keeper choice, sequences.
  A scan calls it once at the end; the server calls it again on saved data.
  `Grouping::counts()` gives the numbers each tab would show.
- **What makes regrouping possible is what the scan now saves:** the analysis
  was already stored (fingerprints, capture times, provenance); new is the
  `fixed_edges` table, the links no setting changes (byte-identical copies,
  matched videos), and a `scan_regroup` marker. Video matching therefore stays
  out of the sliders (see the deferred table). `Store::load_grouping_data`
  rebuilds the input; `Store::replace_grouping` swaps the clusters, sequences
  and look-alike pairs in one transaction and records the new settings on the
  scan. Orphan videos and decisions are untouched.
- **Two endpoints:** `GET /api/scans/{id}/regroup/preview?...` (both
  counts, writes nothing, 400 for out-of-range values) and
  `POST /api/scans/{id}/regroup` (groups with the saved settings). A scan
  saved before this existed answers 409; it needs one ordinary re-scan, after
  which it is regroupable. The Settings screen then says "next scan" again.
- **Safety, found while designing it:** a regroup can leave a photo in no
  group at all, while an old "quarantine" choice for it still exists. Applying
  would then have moved a photo that is no longer a duplicate of anything.
  `apply::plan` now counts a choice only for a file that is in some group
  today (tested). The choice itself is kept, so dragging a slider back
  restores it.
- **Verified on real photos, not just tests:** on a scratch copy of the slider
  demo folder (default scan mode), the preview at every slider position in its
  README reproduces the table exactly (7 sequences / 28 photos / 7 look-alikes
  at the defaults; 6 / 19 / 10 at a 1 s gap; 0 look-alikes at distance 25; and
  so on). On a scratch copy of the small test folder (exact copies, a video
  group, sequences), regrouping at unchanged settings reproduced the scan's
  clusters, keepers, reasons, sequences and pairs identically.
- **UI (revised after first use):** the sliders first lived in Settings, with
  a preview on drag and apply on release. That meant leaving the review,
  opening Settings and coming back, so they moved into the review itself:
  a **Fine-tune** button in the toolbar opens a side panel (a sheet at narrow
  widths) with the three sliders. Moving one saves it, regroups the scan and
  reloads the review behind the panel (debounced, latest value wins, never
  two updates at once), so the sidebar counts and the lists change as you
  drag. Your tab and filter stay. **Revert** restores the values from when
  the panel opened, **Defaults** resets them, and a "Since you opened this"
  summary says what changed in the lists. Settings keeps scan speed and
  appearance and points to Fine-tune. A scan that cannot be regrouped
  (saved before this) gets a notice and a Scan again button in the panel.
  The Settings-era preview endpoint is still used, once, when the panel opens
  (it supplies the starting counts and tells a regroupable scan from an old
  one). Checked in headless Chrome: counts change live, Revert restores them,
  axe clean in light, dark and narrow.
- **Known limit:** each regroup gives its groups new ids, so a group open in
  the detail view closes back to the overview while a slider is moved.

## M4: desktop shell and packaging

Built and checked on **macOS/arm64 only**. What was and wasn't verified is
spelled out, because several parts could only be partly tested from here.

### Built

- **The shell** (`src-tauri/`, Tauri v2, its own Cargo workspace so its
  dependency tree stays out of engine builds). It starts the server
  in-process (database in the per-user app-data folder, UI compiled in via the
  server's `embed-ui` feature), opens a native window on it, and **refuses to
  navigate that window anywhere but its own loopback address**. The menu is
  hand-built without the stock Undo item, because the system "Undo" binds
  Cmd+Z and swallows the keystroke before the page's own undo can see it.
- **Native folder picker without leaving the HTTP boundary.** The plan had the
  UI import a Tauri package for this. Instead the shell hands the server a
  callback, `GET /api/shell` says whether one exists, and `POST /api/pick-folder`
  asks for it; the UI shows *Browse...* only when offered. The UI still knows
  nothing about which shell it is in, so swapping the shell (the plan's
  Electron/CEF escape hatch) needs no UI change, and a web page never gets
  access to Tauri's internals.
- **Bundled ffmpeg, as an LGPL build we make ourselves**
  (`scripts/build-sidecar.sh`): pinned 9.0.2 source, SHA-256 verified,
  `--disable-gpl --disable-nonfree --disable-autodetect`, static; installed as
  `sidecar/ffmpeg-<triple>` (what Tauri's `externalBin` expects); then
  `photodedup check-sidecar` refuses anything built with `--enable-gpl` or
  `--enable-nonfree` (it correctly refuses this machine's Homebrew ffmpeg).
  `sidecar/SOURCE.txt` records the exact version, source URL, checksum and
  configure line, and ships in the app with the LGPL text.
  - **Checked against real files:** all 8 real-photo/video tests pass on it
    (HEIC decode and thumbnails, EXIF, ffprobe on the iPhone and OnePlus
    videos, fingerprint parity with the Python original), and a full scan of a
    51-file set gives results identical to the system ffmpeg's.
  - **The engine finds it** next to the executable (`core::tools`), with
    `PHOTODEDUP_FFMPEG` / `PHOTODEDUP_FFPROBE` overrides and `PATH` as the last
    resort. Verified in the packaged app: with no ffmpeg on `PATH` it used its
    own, scanned real HEIC and video files, and produced the expected groups.
  - The start screen **warns when ffmpeg or ffprobe is missing** (before this,
    HEIC photos and videos failed silently).
- **Bundle, ad-hoc signed.** Found while checking: the first bundle was only
  *linker*-signed and `codesign --verify --strict` failed ("code has no
  resources but signature indicates they must be present"), which on Apple
  Silicon can show as "damaged" after download. Fixed with
  `signingIdentity: "-"`; it now verifies (identifier, sealed resources, valid
  on disk). 11 MB without ffmpeg, 52 MB with it.
- **Gates:** `cargo deny` passes for both the engine and the shell's tree (two
  documented exceptions in `deny.toml`: `Apache-2.0 WITH LLVM-exception`, and
  RUSTSEC-2024-0370, an *unmaintained* compile-time macro crate reached only
  through the Linux GTK bindings). `reuse lint` passes on a clean checkout
  (116 files). `cargo about` generates the attribution file (166 licence
  texts) plus the three npm runtime packages compiled into the UI; it ships
  inside the app. Two things these runs caught: a REUSE rule that silently
  ignores any file with `LICENSE` in its name, and my own `.gitignore` pattern
  for the ffmpeg binaries also ignoring the licence text beside them (it would
  never have been committed and the release build would have failed).
- **Docs:** `docs/INSTALL.md` (first-launch steps per OS, including the macOS 15
  "Open Anyway" route, since right-click Open no longer works), README rewritten.

### Not verified, or not done

- **I have not seen the window.** No screen-recording or accessibility access
  from here. Confirmed by other means: the app launches, serves the embedded UI
  on a random loopback port, rejects the API without the token, and isn't
  reachable from another interface. **Unconfirmed, needs a person:** that the
  window opens and looks right, that *Browse...* shows the native dialog,
  that Cmd+Z reaches the page, that the navigation guard blocks external
  links, and what closing the window does on macOS (the app may stay running).
- **Workflow hardening (after an automated review of the first commit):** every
  third-party action is pinned to a full commit SHA with its release in a
  comment (resolved from the real repositories, not guessed); the release build
  job has read-only repository access and no build cache; and only a separate
  publish job holds `contents: write`, uploading files the build job already
  produced and attested. Pinned SHAs need refreshing deliberately (e.g. with
  Dependabot for Actions); they will otherwise go stale.
- **CI and release workflows have never run.** They parse as YAML and use the
  commands that were run locally, nothing more. The Windows and Linux legs and
  the Intel macOS leg are untested (the `macos-13` runner label may be retired).
  `scripts/build-sidecar.sh` has only been run on macOS/arm64; Windows needs an
  MSYS2 shell.
- **Windows WebView2:** the plan says detect it and link to Microsoft's
  installer if missing. Not implemented; Tauri's installer default is to
  download the bootstrapper, which achieves the same thing but is unverified.
- **Distribution:** no Homebrew cask, winget manifest, Scoop bucket or Flathub
  entry; no auto-updater; no notarisation (paid) or SignPath (Windows) setup.
- **ffmpeg provenance:** the tarball is pinned by a SHA-256 taken from its
  first download, which guards against change but not against a bad first
  download. The PGP signature was not checked (no `gpg` here).
- **The ffmpeg build is the full default LGPL feature set** (21 MB per binary).
  A `--disable-everything` build with only the decoders we use would be much
  smaller, but needs careful testing for each format and wasn't attempted.
- **`check-sidecar` reads the build's configure flags.** It cannot prove no GPL
  code is statically linked; `--disable-autodetect` is what prevents that.
- **Placeholders:** the app icon (generated by `scripts/make-icon.py`), the
  bundle identifier `org.photodedup.desktop`, and the repository URL in
  `Cargo.toml` (`REPLACE_ME`) all need real values.
- **UI/UX polish** is still to do and waits on first-hand feedback from using
  the app.

## UI/UX pass

User verdict on the first desktop build: it worked, but "not really good"
looking, and it needed to be more responsive and nicer. Rather than invent a
look, the UI now follows **Apple's Human Interface Guidelines** (the app is
macOS-first and the user wanted a native feel) with **WCAG 2.2 AA** as the
accessibility bar; both degrade sensibly on Windows and Linux (system font
stack, no platform-specific chrome).

Done by first *looking* at the real screens (headless Chrome, light and dark,
three window sizes) with real data. What that showed: a 1440px window that was
mostly empty, tiny cards, state shown only as small bold text, a cluttered
header with a bright "Apply" competing with the real primary action, grey
letterbox bars in the detail view, and a start screen that was a bare form.

- **One design system** (`ui/src/app.css`, `Icon.svelte`): the macOS type scale
  (13px body) on the system font, a 4px spacing grid, semantic colour roles for
  light and dark, and a single set of controls (buttons, segmented controls,
  switch, sliders with filled tracks, badges, sheets, skeleton loaders), with
  visible focus rings and `prefers-reduced-motion` honoured. Colour is never the
  only signal: every state also carries a word or an icon.
- **Structure follows the guidelines:** a toolbar, and a *sidebar* for the five
  categories, each with its own count and progress, plus overall progress. Photos
  are tiles with state overlays (Accepted / Edited / "3 files" / "1 of 4 kept");
  dialogs are sheets; a lone primary action per screen.
- **Detail view:** photos large over a soft blurred backdrop of their own colours
  (no grey bars), a Keep / Quarantine segmented control with meaning-coloured
  sides, quarantined photos desaturated with a "Will be quarantined" label, and a
  sharp preview that fades in over the instant thumbnail.
- **Start screen:** a welcome, a "Continue where you left off" card (from data the
  resume feature already had), a switch for videos, and during a scan a phase
  checklist (photos, then video details, then matching) with the live counts.
- **Narrow windows (720px and up, the shell's minimum):** the sidebar becomes a
  scrolling strip, the toolbar reflows, detail stacks, and overall progress moves
  into the content header.

**Measured, not assumed** (`scripts/ui-audit/`, re-runnable):

- *Accessibility:* axe-core with the WCAG 2.0/2.1/2.2 A and AA rules, on all 15
  screens and states, in light and dark: **0 violations**. The first run found
  real ones, now fixed: tile and photo buttons whose label didn't contain the
  text they show (a screen reader would say something different from what is
  read), dark-mode badge text at 3.53:1 and tertiary text at 4.49:1 against 4.5:1.
  *Automated checks catch only part of this:* reading order, focus order, and
  screen-reader wording still need a person with a screen reader.
- *Responsiveness, on the 2718-file reference set* (451 groups, the biggest
  category 269 groups): open the review 119 ms; switch to the biggest category
  44 ms to the DOM, photos loaded in 84 ms warm or about 1 s cold; **scrolling the
  whole grid at 60 fps with no dropped frame**, cold or warm; open a group 45-56
  ms; a decision shown on screen in 32-37 ms; Enter (accept and next) 22 ms.
  Tiles use `content-visibility: auto`, so a long list costs little (7 MB JS heap
  for 269 tiles).
- *The one slow thing, and what was done about it:* the first sharp preview of a
  photo nobody has asked for yet takes about 5 s (the server makes it on first
  request). The next two groups' previews are now fetched quietly while you look
  at the current one (the next group then appears in about 30 ms instead of
  about 800 ms), and a small "Loading full resolution…" note says why a photo is
  soft in the meantime.
- *A bug the timing probe exposed:* after clicking Keep or Quarantine with the
  mouse, focus stayed on that button, so Enter re-pressed it instead of "accept
  and next". A mouse click now releases focus; a keyboard activation keeps it.

**Not done, or not verified:**

- **I have still not seen the desktop window itself.** Everything above is the
  same UI in headless Chrome. WKWebView (the macOS shell's engine) can differ in
  small ways: `backdrop-filter`, `:has()`, `color-mix()`, `content-visibility`.
  If something looks wrong in the app but not in the screenshots, that is where
  to look.
- No design review by a person with a designer's eye: this follows the
  guidelines faithfully but it is an engineer's reading of them.
- Not done: native macOS window vibrancy and a real title-bar toolbar (needs
  window-effects configuration in Tauri); Windows/Linux visual passes; a
  forced-colours (high-contrast) pass; touch layouts (targets are 26-30px, above
  WCAG's 24px minimum but below Apple's 44pt touch guidance); translations.
- **Next performance improvement:** pre-generate previews in the background after
  a scan (low priority), so even the first photo you open is sharp at once.
- The user-facing wording ("Quarantine", "Apply", "Same-device review", "Orphan
  videos") is unchanged and is still jargon in places; a copy pass belongs with
  the design review.

