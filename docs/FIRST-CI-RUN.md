<!-- SPDX-License-Identifier: Apache-2.0 -->
# The first run on GitHub

Neither workflow has run yet. Everything they do was run by hand on one
machine (macOS, Apple Silicon); the Windows, Linux and Intel-Mac legs have
never executed. This is what was checked beforehand, what to expect, and what
to look at first when something is red.

## Checked before the first push (2026-10-29)

- Both workflow files parse, and every pinned action's commit matches the tag
  named in its comment (checked against the repositories with `git ls-remote`).
- The runner labels exist. `macos-13` does not (retired December 2025) and
  `macos-14` is retired on 2026-11-02, so the workflows use `macos-15` (Apple
  Silicon) and `macos-15-intel`.
- The `ci.yml` commands, run locally exactly as written: `npm ci`, `npm run
  check`, `npm test`, `npm run build`, `cargo test --workspace` (also with
  ffmpeg and ffprobe hidden from `PATH`, as on a clean runner),
  `cargo test -p photodedup-server --features embed-ui`.
- `cargo deny` for the engine (with `--all-features`, which the action adds by
  default) and for the desktop shell (with `--config ./deny.toml`, which goes
  before `check`; the action's argument order was read from its source).
- `reuse lint`: every file has licence information.
- Not checkable here: anything on Windows or Linux, and the ffmpeg build on any
  system but this Mac.

## Order

1. **Push a branch and open a pull request.** `ci.yml` runs. Read it first.
2. **Run `release` by hand** (Actions tab, `release`, *Run workflow*). It builds
   every installer and keeps them as artifacts; it cannot publish, because the
   publish job needs a version tag.
3. Only when step 2 is green and you have opened an installer on each system,
   push a `v0.1.0` tag.

## What to expect from `ci.yml`

| Job | Expectation |
|---|---|
| engine and UI | Should pass: same commands as above on Linux. |
| engine tests (macos-15) | Should pass. |
| engine tests (windows-latest) | **Informational**; may fail first time. Read the first compile error or failing test; see "Windows suspects". |
| licences, advisories, reuse | Should pass. |
| desktop shell compiles | macOS likely fine. Linux needs the system libraries it installs. Windows: the first time Tauri and WebView2 are built here. |

## What to expect from the release dry run (likeliest trouble first)

1. **Windows, ffmpeg build.** Built with MSYS2 (mingw-w64) from source, as the
   script documents, then the licence gate runs outside MSYS2. If `configure`
   fails, the log says what it could not find. The step after it prints the
   DLLs the sidecar imports: anything beyond Windows' own system DLLs
   (`KERNEL32`, `msvcrt`, `bcrypt`, and the like) would be missing on a user's
   machine.
2. **Windows, installer.** Tauri downloads WiX for the `.msi`.
3. **Linux, AppImage.** Needs the system libraries the workflow installs; the
   sidecar build needs `nasm`.
4. **Intel Mac.** Same as Apple Silicon plus `nasm`.
5. **ffmpeg build time.** Several minutes per leg; that is normal.

## Windows suspects, if the engine tests or the app misbehave

- **`\\?\` paths.** Windows canonical paths carry that prefix and the scan root
  is canonicalised, so the paths handed to ffmpeg and ffprobe have it. If HEIC
  photos or videos fail to decode only on Windows, strip the prefix before
  passing a path to a child process.
- **Path separators.** Relative paths are stored with the platform separator
  (backslashes on Windows). Decisions and the journal are keyed on them.
- **Junctions.** The workspace guard now also requires the fully resolved path
  to stay inside the folder (a junction is not reported as a symlink). It has
  only been tested with symlinks.
- **No power or priority handling.** The scheduler on Windows is a no-op, so
  the three scan speeds differ only in worker count there.
- **Tests that need a privilege.** Creating a symlink needs one on Windows; the
  tests that do so are limited to Unix.

## Smoke test on a real machine, per system

Install the artifact, then: open the app; check Settings shows ffmpeg found
(the start screen warns if not); scan a small folder with a few HEIC photos and
a video; open a group; mark one photo Quarantine; Apply; find the files in
`.photodedup-quarantine`; undo; quit and reopen to see the scan resumes.

## What to send me

For a red job: the job name, the first error (not the last line), and the
operating system. For a green one that misbehaves: what you did, what you saw,
and the system.

## Settings on GitHub

- Actions must be allowed to run; the release job needs the repository to be
  able to create attestations (public repositories can).
- Dependabot (`.github/dependabot.yml`) opens pull requests when a pinned
  action has a newer version or a runner label is deprecated.
