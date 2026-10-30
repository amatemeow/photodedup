#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
#
# Runs INSIDE a throwaway container (see test-linux-packages.sh): no network,
# read-only root filesystem, samples mounted read-only at /samples.
fail=0
pass() { echo "PASS  $*"; }
bad()  { echo "FAIL  $*"; fail=1; }

# 1. What the package put in /usr/bin, and that none of it clashes with ffmpeg.
for f in photodedup-app photodedup-ffmpeg photodedup-ffprobe; do
  [ -x "/usr/bin/$f" ] && pass "/usr/bin/$f is installed" || bad "/usr/bin/$f is missing"
done
if command -v dpkg >/dev/null 2>&1; then owner=$(dpkg -S /usr/bin/ffmpeg 2>/dev/null || true)
else owner=$(rpm -qf /usr/bin/ffmpeg 2>/dev/null || true); fi
if [ "${WITH_DISTRO_FFMPEG:-0}" = 1 ]; then
  case "$owner" in *photodedup*) bad "the distro's /usr/bin/ffmpeg now belongs to photodedup: $owner" ;;
    "") bad "the distro's ffmpeg was expected in /usr/bin/ffmpeg but is not there" ;;
    *) pass "distro ffmpeg untouched and still owned by: $owner" ;; esac
else
  [ -e /usr/bin/ffmpeg ] && bad "a bare /usr/bin/ffmpeg exists" || pass "no bare /usr/bin/ffmpeg"
fi

# 2. Every shared library the app and the sidecars need is present.
for f in photodedup-app photodedup-ffmpeg photodedup-ffprobe; do
  missing=$(ldd "/usr/bin/$f" 2>&1 | grep 'not found' || true)
  [ -z "$missing" ] && pass "$f: all libraries resolve" || bad "$f: missing libraries: $missing"
done

# 3. The bundled ffmpeg runs, and decodes a real HEIC photo. No explicit -map:
#    see SPIKES.md (multi-stream HEIC) and crates/core/src/decode.rs.
/usr/bin/photodedup-ffmpeg -version 2>&1 | head -1
/usr/bin/photodedup-ffprobe -version 2>&1 | head -1
n=0
for h in /samples/*.HEIC /samples/*.heic; do
  [ -f "$h" ] || continue
  n=$((n + 1)); out=/tmp/out$n.jpg
  if /usr/bin/photodedup-ffmpeg -v error -y -i "$h" -frames:v 1 "$out" 2>/tmp/ffmpeg.err \
     && [ "$(head -c 2 "$out" | od -An -tx1 | tr -d ' ')" = ffd8 ]; then
    pass "HEIC decode: $(basename "$h") -> JPEG, $(wc -c < "$out") bytes"
  else
    bad "HEIC decode failed: $(basename "$h"): $(head -3 /tmp/ffmpeg.err)"
  fi
done
[ "$n" -gt 0 ] || bad "no HEIC samples were mounted"
for v in /samples/*.MP4 /samples/*.mp4 /samples/*.MOV /samples/*.mov; do
  [ -f "$v" ] || continue
  d=$(/usr/bin/photodedup-ffprobe -v error -show_entries format=duration -of csv=p=0 "$v" 2>&1)
  case "$d" in [0-9]*) pass "ffprobe reads $(basename "$v"): ${d}s" ;; *) bad "ffprobe on $(basename "$v"): $d" ;; esac
  break
done

# 4. The app starts under a virtual display and stays up. WebKit's own sandbox
#    needs privileges this container deliberately does not have, so it is
#    switched off here (inside the throwaway container only).
export HOME=/tmp/home XDG_RUNTIME_DIR=/tmp/xdg
mkdir -p "$HOME" "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  LIBGL_ALWAYS_SOFTWARE=1 timeout 40 xvfb-run -a /usr/bin/photodedup-app >/tmp/app.log 2>&1
rc=$?
if [ "$rc" = 124 ]; then pass "photodedup-app started and was still running after 40 s"
else bad "photodedup-app exited early (status $rc): $(tail -5 /tmp/app.log | tr '\n' ' ')"; fi

exit $fail
