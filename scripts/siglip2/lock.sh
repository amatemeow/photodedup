#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Regenerates the hash-locked requirement files next to requirements.in. Run it when requirements.in changes or to
# move to newer tool versions on purpose; the export is only comparable between builds that use the same lock.
set -euo pipefail
cd "$(dirname "$0")"
# uv treats an existing output file as version preferences; start clean so the result follows requirements.in alone.
rm -f requirements-linux.txt
[ "$(uname -sm)" = "Darwin arm64" ] && rm -f requirements-macos.txt
# The macOS lock is resolved for the machine it runs on (Apple silicon): uv assumes an older macOS than the newest
# torch wheels need when asked for the platform by name, which would silently pick older tool versions.
if [ "$(uname -sm)" = "Darwin arm64" ]; then
  uv pip compile -q --generate-hashes --python-version 3.12 requirements.in -o requirements-macos.txt
else
  echo "not an Apple-silicon Mac: leaving requirements-macos.txt as it is" >&2
fi
# PyTorch's own CPU index; the other packages still come from PyPI, and a version only counts if a hash matches.
uv pip compile -q --generate-hashes --python-version 3.12 --python-platform x86_64-unknown-linux-gnu \
  --extra-index-url https://download.pytorch.org/whl/cpu --index-strategy unsafe-best-match \
  requirements.in -o requirements-linux.txt
