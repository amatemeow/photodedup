#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Installs the .deb and .rpm release artifacts into throwaway containers of
# several distributions and checks that they install, that the bundled ffmpeg
# runs and decodes a real HEIC photo, and that the app starts.
#
#   scripts/test-linux-packages.sh <package.deb> <package.rpm> <samples-dir>
#
# <samples-dir> holds a few .HEIC files and one video; they are mounted
# read-only. Needs Docker. The packages are x86-64, so on an arm64 Mac the
# containers run under emulation (slow, but the results are the same).
#
# Isolation: the only step with network access is the image build (it has to
# fetch the packages' dependencies); the tests run with no network, a read-only
# root filesystem, every capability dropped, no new privileges, a non-root user
# and memory/process limits (the process limit must stay generous: decoding one
# iPhone HEIC starts a decoder with a thread pool per tile, hundreds of threads
# on a many-core machine, and a limit of 512 made ffmpeg fail). The only host path they can see is <samples-dir>,
# read-only. The Docker socket is never mounted.
set -euo pipefail

[ $# -eq 3 ] || { sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
deb=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
rpm=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
samples=$(cd "$3" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
[ -f "$deb" ] && [ -f "$rpm" ] || { echo "package file missing" >&2; exit 2; }
docker info >/dev/null 2>&1 || { echo "Docker is not running" >&2; exit 2; }

work=$(mktemp -d)
images=()
cleanup() { [ ${#images[@]} -gt 0 ] && docker rmi -f "${images[@]}" >/dev/null 2>&1 || true; rm -rf "$work"; }
trap cleanup EXIT

PLATFORM=linux/amd64
results=()

# name | base image | family | extra packages installed BEFORE ours
CASES=(
  "debian12|debian:12|deb|"
  "ubuntu2204|ubuntu:22.04|deb|"
  "ubuntu2404|ubuntu:24.04|deb|"
  "debian12+distro-ffmpeg|debian:12|deb|ffmpeg"
  "ubuntu2404+distro-ffmpeg|ubuntu:24.04|deb|ffmpeg"
  "fedora|fedora:latest|rpm|"
  "opensuse-leap|opensuse/leap:15.6|rpm|"
)

dockerfile() { # base family extra
  local base=$1 family=$2 extra=$3
  echo "FROM $base"
  case "$family:$base" in
    deb:*)
      cat <<EOF
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends xvfb xauth ca-certificates $extra
COPY pkg/package.deb /tmp/package.deb
RUN apt-get install -y /tmp/package.deb && rm -rf /tmp/package.deb /var/lib/apt/lists/*
EOF
      ;;
    rpm:fedora*)
      cat <<EOF
COPY pkg/package.rpm /tmp/package.rpm
RUN dnf install -y --setopt=install_weak_deps=False xorg-x11-server-Xvfb xorg-x11-xauth /tmp/package.rpm && rm -f /tmp/package.rpm && dnf clean all
EOF
      ;;
    rpm:opensuse*)
      cat <<EOF
COPY pkg/package.rpm /tmp/package.rpm
RUN zypper --non-interactive --no-gpg-checks install --no-recommends --allow-unsigned-rpm xvfb-run xauth /tmp/package.rpm && rm -f /tmp/package.rpm && zypper clean -a
EOF
      ;;
  esac
}

for c in "${CASES[@]}"; do
  IFS='|' read -r name base family extra <<<"$c"
  # ONLY="debian12 fedora" runs just those cases.
  [ -z "${ONLY:-}" ] || [[ " $ONLY " == *" $name "* ]] || continue
  echo "=== $name ($base)"
  ctx="$work/$name"; mkdir -p "$ctx/pkg"
  if [ "$family" = deb ]; then cp "$deb" "$ctx/pkg/package.deb"; else cp "$rpm" "$ctx/pkg/package.rpm"; fi
  dockerfile "$base" "$family" "$extra" > "$ctx/Dockerfile"
  img="photodedup-pkgtest-${name//+/-}"; images+=("$img")

  if ! docker build -q --platform "$PLATFORM" -t "$img" "$ctx" >"$work/$name.build.log" 2>&1; then
    echo "  INSTALL FAILED:"; tail -15 "$work/$name.build.log" | sed 's/^/    /'
    results+=("$name: INSTALL FAILED"); continue
  fi
  echo "  installed"

  if docker run --rm --platform "$PLATFORM" \
      --network none --read-only --tmpfs /tmp:rw,exec,size=512m \
      --cap-drop ALL --security-opt no-new-privileges \
      --pids-limit 4096 --memory 3g --user 1000:1000 \
      -v "$samples:/samples:ro" \
      -v "$here/linux-package-test.in-container.sh:/test.sh:ro" \
      -e "WITH_DISTRO_FFMPEG=$([ -n "$extra" ] && echo 1 || echo 0)" \
      "$img" sh /test.sh >"$work/$name.run.log" 2>&1; then
    results+=("$name: PASS")
  else
    results+=("$name: FAIL")
  fi
  sed 's/^/  /' "$work/$name.run.log"
done

echo; echo "=== summary"; printf '%s\n' "${results[@]}"
printf '%s\n' "${results[@]}" | grep -q -E 'FAIL' && exit 1 || exit 0
