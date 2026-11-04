# models

The AI models the app ships. The model files are **built, not committed** (a source is 1.5 GB and GitHub rejects
files over 100 MB), and the app never goes online: models are bundled in the installers.

`manifest.json` is the one record of what may ship. For each model it holds:

- the **licence of the weights** (not of the code around them), its class (`permissive` or `non-commercial`), the
  evidence it was read from, and the date it was checked;
- the **training data**, named and described, including whether it is public;
- the **source**: host, repository, a full commit and the SHA-256 and size of every source file;
- the **build**: the script, and the SHA-256 of the hash-locked tool lists it runs with;
- the **artifacts**: the SHA-256 and size of what the build produces, and which of them go into the installers.

`photodedup check-models` (run from the repository root) refuses a manifest that is incomplete, has an unpinned
source, classes a licence as permissive when it is not on the list, allows a non-commercial licence in a `core`
component, or points at lock files that have changed since the manifest was written. `--built <dir>` also checks
the built files against the recorded hashes. `cargo test` runs the first part on every push.

## Rebuilding and proving a model

    scripts/siglip2/lock.sh            # only when the tools should change
    uv venv .venv-siglip2
    uv pip install --python .venv-siglip2/bin/python --require-hashes -r scripts/siglip2/requirements-linux.txt   # or -macos.txt
    .venv-siglip2/bin/python -I scripts/export-siglip2.py <work-dir> <out-dir>
    cargo run -q -p photodedup-cli -- check-models --built <out-dir>

The script downloads Google's weights at the pinned commit, refuses any file whose hash differs from the manifest,
builds the ONNX files, and compares them with the hashes in the manifest. The same recipe gave byte-identical files
on macOS arm64, Ubuntu 24.04 arm64 and Ubuntu 24.04 x86-64.

## Changing a model

A new upstream version, a new tool lock or a changed script is a deliberate edit: rebuild, update the manifest
(commit, hashes, `checked` date, evidence) in the same commit, and run the gate. Never edit a hash to make a build
pass without knowing why it changed.
