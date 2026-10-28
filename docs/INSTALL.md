<!-- SPDX-License-Identifier: Apache-2.0 -->
# Installing photodedup

photodedup is not signed with a paid developer certificate, so each system
shows a warning the first time you open it. That is the operating system
being cautious about software it hasn't seen before, not a problem with the
download. What to do, per system:

## macOS

**Easiest: Homebrew** (planned; not published yet). `brew install --cask
photodedup` installs without any warning, because Homebrew doesn't mark the
app as downloaded from the internet.

**From a download** (`.dmg`): drag photodedup into Applications and open it.
macOS will refuse the first time ("cannot be opened because the developer
cannot be verified"). Then:

1. Open **System Settings → Privacy & Security**.
2. Scroll to the message about photodedup being blocked and click
   **Open Anyway**, then confirm.

On macOS 15 and later the old "right-click → Open" shortcut no longer works
for this; the Settings route above is the one that does.

The app is signed with an *ad-hoc* signature, which Apple Silicon requires in
order to run at all; it does not remove the first-launch warning.

## Windows

Windows SmartScreen shows "Windows protected your PC". Click **More info**,
then **Run anyway**.

photodedup uses the Microsoft Edge WebView2 runtime that ships with Windows 10
and 11. If your system doesn't have it, the app will say so and link to
Microsoft's installer.

## Linux

Download the `.AppImage`, make it executable (`chmod +x photodedup*.AppImage`),
and run it. There is no signing step on Linux. AppImage is the supported
format: distribution `.deb`/`.rpm` packages depend on the WebKitGTK version
your system happens to have.

## Verifying a download

Each release lists SHA-256 checksums; compare with `shasum -a 256 <file>`
(macOS/Linux) or `Get-FileHash <file>` (Windows). Releases also carry build
provenance attestations: `gh attestation verify <file> --repo <owner>/<repo>`.

## What the app needs

- **ffmpeg and ffprobe** read HEIC photos and videos. Release builds bundle
  them (an LGPL build; the exact source is listed in `ffmpeg-SOURCE.txt`
  inside the app). If you build from source, install ffmpeg yourself or set
  `PHOTODEDUP_FFMPEG` / `PHOTODEDUP_FFPROBE` to your own copies. The start
  screen warns you if either is missing.
- Your scan database and thumbnails live in the per-user application-data
  folder (`~/Library/Application Support/org.photodedup.desktop` on macOS,
  `%APPDATA%\org.photodedup.desktop` on Windows,
  `~/.local/share/org.photodedup.desktop` on Linux), never inside a folder you
  scan.

## Your files

Nothing is ever deleted. Files you set aside are moved into a
`.photodedup-quarantine` folder inside the folder you scanned, and Undo puts
them back. The app only ever changes files inside the folder you pointed it
at.
