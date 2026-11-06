# photodedup

A cross-platform (macOS, Windows, Linux), self-contained, open-source app for
finding and reviewing duplicate photos and videos in a folder. Point it at a
location, review what it found, choose what to keep. **Nothing is ever
deleted**: files you set aside are moved into a quarantine folder inside the
folder you scanned, and every move can be undone.

It grew out of a working single-purpose script used on one trip's photos (see
`SPIKES.md` for what carried over and what was validated).

## What it does

- **Finds** exact copies, re-saved and forwarded copies (messenger re-encodes),
  look-alike shots, and runs of near-identical shots taken seconds apart; video
  copies are matched too.
- **Never trusts a hash alone.** Two untouched camera photos are *never*
  suggested as copies of each other, however similar they look (a structural
  rule, not a threshold), because burst frames and true duplicates look alike.
- **Lets you decide.** Five tabs (exact duplicates, true duplicates, quick
  sequences, same-device look-alikes, orphan videos), a per-file keep or
  quarantine choice (keep as many as you like), undo, keyboard shortcuts, and a
  dry-run preview before anything moves.
- **Stays inside your folder.** Every change to your files goes through one
  guarded module that refuses anything outside the scanned folder (no `..`, no
  symlinks followed, nothing in photo libraries or `.git`), never deletes, and
  never overwrites.
- **Is polite to your computer.** Three scan speeds (Efficient, Default,
  Performance) trade time for fans and heat.
- **Helps review the whole library.** Browse every scanned file, manage
  quarantine, and review possible defects detected by classical image checks.

## Status

Preparing the first release, **0.1.0**. Engine tests pass on Linux, macOS and
Windows; the UI and desktop shell compile in CI. Installer dry runs have
produced macOS DMGs (Apple Silicon and Intel), a Windows MSI, and Linux
AppImage, deb and rpm packages, with bundled LGPL ffmpeg/ffprobe. Linux
packages have also been tested in isolated containers.

The current release candidate still needs a full installer dry run with
public-repository attestations and installer smoke tests. The tag-only
publish job has not yet been exercised. See [the release checklist](docs/RELEASING.md).
Paid-certificate signing/notarisation and package-manager distribution remain
deferred; [installation notes](docs/INSTALL.md) explain first-launch warnings.
AI model export tooling exists, but model inference and People are not part
of 0.1.0. See `ROADMAP.md` for the remaining work and known limitations.

## Running it

### In a browser (development)

```
cd ui && npm ci && npm run build && cd ..
cargo run --release -p photodedup-cli -- serve ~/photodedup.sqlite3
```

Open the address it prints and scan a folder. It needs `ffmpeg` and `ffprobe` on
your `PATH` for HEIC photos and videos (or set `PHOTODEDUP_FFMPEG` and
`PHOTODEDUP_FFPROBE`); the start screen warns you if they are missing. Keep the
database outside any folder you scan.

### The desktop app

```
scripts/build-sidecar.sh                 # builds the LGPL ffmpeg/ffprobe we ship
scripts/third-party-licenses.sh          # the attribution file that ships inside the app
cd src-tauri
npx @tauri-apps/cli@2.12.4 build --bundles app --config tauri.release.conf.json
```

(Leave out `--config …` to build without the bundled ffmpeg and use your own.)
First-launch steps per operating system are in `docs/INSTALL.md`.

### From the command line

```
photodedup scan <folder> [--mode efficient|default|performance|N] [--no-videos]
photodedup apply  [db] [--commit]      # dry run unless --commit
photodedup batches [db]                # what is in quarantine
photodedup undo <batch-id> [db]
photodedup serve [db] [--open]         # the review UI in a browser (prints the address; --open opens it)
photodedup check-sidecar <binary>...   # refuse a GPL ffmpeg
```

## Developing

```
cargo test --workspace                 # engine and server, no external data needed
cargo deny check                       # licence, advisory and source gate
cargo deny --manifest-path src-tauri/Cargo.toml check
cd ui && npm ci && npm run check && npm test
```

`scripts/ui-audit/` has screenshot, accessibility (axe-core, WCAG 2.2 AA) and
interaction-timing tools for the UI (see its README).

Use a generated library and a temporary database for integration tests:

```
python3 scripts/make-fixture.py /tmp/photodedup-fixture
PHOTODEDUP_FIXTURE=/tmp/photodedup-fixture cargo test --release -p photodedup-core --test fixture -- --ignored
```

The generator needs ffmpeg/ffprobe; HEIC cases additionally need sample HEICs
supplied with `--heic-from` (keep personal samples out of Git). A few legacy
ignored tests still reference a private photo library; they are not the
recommended test path. "The prototype" in code comments means the earlier
single-purpose Python version, which is not part of this repository. The
desktop shell is a separate Cargo workspace so its large dependency tree
stays out of engine builds.

Layout: `crates/core` (the engine, no I/O policy), `crates/server` (local HTTP
API), `crates/cli`, `ui/` (Svelte, talks only HTTP), `src-tauri/` (the native
shell), `sidecar/` (ffmpeg build notes and per-platform binaries),
`docs/`, `ROADMAP.md`, `SPIKES.md`.

## Licence

Apache-2.0, see `LICENSE`. Dependency licences are checked by `cargo deny`
(`deny.toml`) and by the REUSE metadata in `REUSE.toml`.

The bundled **ffmpeg and ffprobe are separate programs, licensed LGPL-2.1+**,
built from the pinned source named in `sidecar/SOURCE.txt` without any GPL or
non-free component, and you can replace them with your own (see above).
