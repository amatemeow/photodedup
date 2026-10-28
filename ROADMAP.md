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
| **Resume-recognition beyond exact path match** | `Store::find_latest_scan_for_root` (see "Resolved during M2" below) currently matches on canonicalized path only — a folder renamed, moved to a new parent directory, or re-synced to a new local path by a cloud client (OneDrive/Dropbox/iCloud routinely do this) no longer matches its own prior scan at all, even though every file inside is identical. Not fixed now because it needs a real design decision, not just more code — **explicit user requirement, recorded here so it isn't lost**: recognize the same logical folder by content, not just path | The exact-hash pass (`exacthash.rs`) already computes a blake3 digest for every size-collision bucket, and `files` already stores `(size, mtime)` per file from day one — a content fingerprint (e.g. the multiset of `(size, blake3)` for a sample of files, or file count + total size as a cheap first-pass filter before a full comparison) is additive, not a schema change. The harder part is UX, not storage — **see the suggested prompt copy right below the table.** |
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
    missed). Status is `running` + phase (`scanning`, `analyzing
    {done,total}`, `exact_duplicates`, `matching`, `videos`,
    `clustering`), `done {scan_id}`, `cancelled`, or `failed {message}`.
    Progress rides a `tokio::sync::watch` channel, which keeps only the
    latest value, so a per-file callback can't flood a slow client.
  - `POST /api/jobs/{id}/cancel` sets a flag the pipeline polls
    (`pipeline::run_cancellable`); it returns 202 at once and the status
    flips to `cancelled` at the next checkpoint. 409 if the job already
    finished, 404 if it isn't the current job. A cancelled scan saves
    nothing: a half-analyzed result isn't a scan anyone could resume.
    Measured on the reference dataset: cancel took effect in under a
    second mid-`analyzing`. **Known limit:** the flag is checked per file
    during `analyzing` and between later phases, but the exact-hash pass
    and video matching have no internal checkpoints, so a cancel landing
    inside one waits for that phase to end (video matching is the slow
    one). Add checkpoints there if it proves noticeable.
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
  "ready" with the cluster count. The review screen and the Options tab
  (persistent scan intensity) are still to build. `photodedup serve`
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
  - **Decisions are per file, so any number of a cluster's members can be
    kept.** The engine's pick is only a suggestion until the user acts on
    it. The one rule: the last kept member of a cluster can't be
    quarantined. "Accept all suggestions" only touches clusters nobody has
    decided yet, behind a confirmation. Undo is multi-level.
  - Keyboard (all of it also has a visible control): arrows or `j`/`k`
    move, `1`-`9` toggle keep, `Enter` accepts and advances, `Space`
    opens the large preview, `Esc` backs out, `u` undoes, `?` lists them.
  - New API: `POST /api/scans/{id}/decisions/bulk` (one transaction) and
    `DELETE /api/scans/{id}/decisions?relative_path=` (back to undecided,
    for undo). Quarantine itself is not built: decisions are only
    recorded; applying them (move to quarantine, with undo) is M3.
  - The decision logic has unit tests (`npm test`); **the screen itself is
    not yet verified in a real browser.** Still to build: review pairs and
    burst runs as browse-only views, the Options tab, and virtualizing the
    overview grid for libraries much larger than the 182 clusters tested.

