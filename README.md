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

## Status

The engine, review UI, apply/undo, and options are built and tested. The
desktop app (`src-tauri/`) builds and runs on macOS/arm64 with a bundled LGPL
ffmpeg. **Not yet done:** Windows and Linux packaging, paid-certificate
signing, a Homebrew cask, the installer workflows have never run on GitHub
(`docs/FIRST-CI-RUN.md` says what to expect), and the interface still needs a
design pass. See `ROADMAP.md` for what is deferred,
and what is known to be rough, in detail.

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

`cargo test --workspace -- --ignored` additionally runs the tests that need the
personal reference photo set and ffmpeg. The desktop shell is a separate Cargo
workspace so its large dependency tree stays out of engine builds.

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
