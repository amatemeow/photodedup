#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Builds the ffmpeg and ffprobe we ship, from a pinned source tarball, as an
# LGPL build (no GPL components, nothing non-free, no auto-detected extras),
# and installs them as sidecar/ffmpeg-<target-triple> / ffprobe-<triple>,
# the names Tauri's `externalBin` expects.
#
# Why build rather than download: ffmpeg's licence depends on how it was
# configured, most prebuilt binaries are GPL builds, and shipping one would
# make the whole app GPL. Building from a pinned tarball means the exact
# source and configuration are known and can be published beside the release
# (see sidecar/SOURCE.txt, which this script rewrites).
#
#   scripts/build-sidecar.sh            # builds for the machine it runs on
#   WORK_DIR=/some/dir ...              # reuse a download/build folder
#
# Needs a C compiler and make (and nasm/yasm on x86). On Windows run it from
# an MSYS2 shell (MINGW64) with the mingw-w64 toolchain. Only macOS/arm64 has
# been exercised so far.
#
#   SKIP_GATE=1 scripts/build-sidecar.sh   # build only; run the licence gate yourself
#
# The gate runs the Rust CLI, which on Windows is better done outside MSYS2
# (its own `link` command shadows the MSVC linker), hence the switch.
set -euo pipefail

FFMPEG_VERSION="9.0.2"
FFMPEG_SHA256="8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e"
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
EXE=""
case "$TRIPLE" in *windows*) EXE=".exe" ;; esac
WORK="${WORK_DIR:-$(mktemp -d)}"
mkdir -p "$WORK"
JOBS="$( (command -v nproc >/dev/null && nproc) || sysctl -n hw.ncpu 2>/dev/null || echo 4)"

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

TARBALL="$WORK/ffmpeg-${FFMPEG_VERSION}.tar.xz"
if [ ! -f "$TARBALL" ]; then
  echo "downloading $FFMPEG_URL"
  curl -fsSL -o "$TARBALL" "$FFMPEG_URL"
fi
GOT="$(sha256 "$TARBALL")"
if [ "$GOT" != "$FFMPEG_SHA256" ]; then
  echo "checksum mismatch for $TARBALL" >&2
  echo "  expected $FFMPEG_SHA256" >&2
  echo "  got      $GOT" >&2
  exit 1
fi

SRC="$WORK/ffmpeg-${FFMPEG_VERSION}"
[ -d "$SRC" ] || tar -C "$WORK" -xf "$TARBALL"

OUT="$WORK/out-$TRIPLE"
rm -rf "$OUT"
CONFIGURE_FLAGS=(
  --prefix="$OUT"
  --disable-gpl --disable-nonfree --disable-version3   # LGPL (v2.1 or later), nothing stronger
  --disable-autodetect                                 # no stray system libraries get linked in
  --disable-network --disable-doc --disable-ffplay
  --enable-static --disable-shared --enable-pic
)
case "$TRIPLE" in
  x86_64-*|i686-*)
    if ! command -v nasm >/dev/null && ! command -v yasm >/dev/null; then
      echo "nasm (or yasm) is needed to build ffmpeg for $TRIPLE; install it (apt/brew/pacman install nasm)" >&2
      exit 1
    fi ;;
esac

( cd "$SRC" && ./configure "${CONFIGURE_FLAGS[@]}" && make -j"$JOBS" && make install )

mkdir -p "$ROOT/sidecar"
for tool in ffmpeg ffprobe; do
  cp "$OUT/bin/${tool}${EXE}" "$ROOT/sidecar/${tool}-${TRIPLE}${EXE}"
done

# The gate: refuse to continue if what we just built is not redistributable.
if [ -z "${SKIP_GATE:-}" ]; then
  ( cd "$ROOT" && cargo run -q -p photodedup-cli -- check-sidecar \
      "sidecar/ffmpeg-${TRIPLE}${EXE}" "sidecar/ffprobe-${TRIPLE}${EXE}" )
else
  echo "SKIP_GATE set: the licence gate was NOT run. Run it before shipping:"
  echo "  cargo run -q -p photodedup-cli -- check-sidecar sidecar/ffmpeg-${TRIPLE}${EXE} sidecar/ffprobe-${TRIPLE}${EXE}"
fi

# The flags as recorded: the same, but without the temporary build path.
SHOWN_FLAGS=("${CONFIGURE_FLAGS[@]/#--prefix=*/--prefix=<build directory>}")

# Record exactly what shipped (the LGPL asks for the version and a way to get
# the corresponding source).
cat > "$ROOT/sidecar/SOURCE.txt" <<TXT
ffmpeg and ffprobe, as shipped with photodedup
==============================================

Version:  ${FFMPEG_VERSION}
Source:   ${FFMPEG_URL}
SHA-256:  ${FFMPEG_SHA256}
Licence:  GNU LGPL v2.1 or later (see LICENSES/LGPL-2.1-or-later.txt).
          Built WITHOUT --enable-gpl and WITHOUT --enable-nonfree.
Built by: scripts/build-sidecar.sh
Configure: ${SHOWN_FLAGS[*]}

photodedup runs these as separate programs (it never links against them),
and you can replace them: set PHOTODEDUP_FFMPEG / PHOTODEDUP_FFPROBE to your
own build, or put one beside the app.
TXT
echo "installed sidecar/ffmpeg-${TRIPLE}${EXE} and sidecar/ffprobe-${TRIPLE}${EXE}"
