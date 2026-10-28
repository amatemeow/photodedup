#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Writes THIRD-PARTY-LICENSES.md for everything the desktop app ships:
#   1. the Rust dependency tree (the engine crates are part of the desktop
#      app's tree, so one run covers both), via `cargo about`;
#   2. the few npm packages whose code is compiled into the UI bundle (Svelte's
#      runtime and its two small helpers), read from the installed packages so
#      the text can't drift from the version that was built.
# Needs `cargo install cargo-about --locked --features cli` and `npm ci` in ui/.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo about generate --manifest-path src-tauri/Cargo.toml --config about.toml about.hbs > THIRD-PARTY-LICENSES.md

{
  echo
  echo "---"
  echo
  echo "# Web interface (JavaScript) components"
  echo
  echo "The interface is built with the packages below; their runtime code is"
  echo "compiled into the app's UI."
  for pkg in svelte esm-env clsx; do
    dir="ui/node_modules/$pkg"
    [ -d "$dir" ] || { echo "missing $dir: run 'npm ci' in ui/ first" >&2; exit 1; }
    version="$(node -p "require('./$dir/package.json').version")"
    file="$(ls "$dir" | grep -i -m1 -E '^licen[cs]e' || true)"
    [ -n "$file" ] || { echo "no licence file in $dir" >&2; exit 1; }
    echo
    echo "## $pkg $version"
    echo
    echo '```text'
    cat "$dir/$file"
    echo
    echo '```'
  done
} >> THIRD-PARTY-LICENSES.md

echo "wrote THIRD-PARTY-LICENSES.md ($(wc -c < THIRD-PARTY-LICENSES.md | tr -d ' ') bytes)"
