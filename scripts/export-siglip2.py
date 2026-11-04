#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Builds the SigLIP 2 image encoder we ship, from Google's own weights, as an ONNX file.

    uv venv .venv-siglip2 && uv pip install --python .venv-siglip2/bin/python --require-hashes -r scripts/siglip2/requirements.txt
    .venv-siglip2/bin/python -I scripts/export-siglip2.py <work-dir> <out-dir>

(`requirements-linux.txt` on Linux, `requirements-macos.txt` on an Apple-silicon Mac; scripts/siglip2/lock.sh makes both.)

Why build rather than download a ready-made ONNX: Google publishes only the PyTorch weights (model.safetensors).
Every ONNX copy on the internet is somebody's conversion, and nothing ties it to Google's file. Here the source is
pinned to one Hugging Face commit and checked against the SHA-256 recorded in models/manifest.json BEFORE anything is read from it,
and the conversion is this script with hash-locked tools (scripts/siglip2/requirements.txt), so anyone can redo it
and compare the result with the hash in the model manifest.

Only the image tower is exported (input: 1x3x224x224 float32, (x/255-0.5)/0.5; output: `embedding`, 1x768), twice:
siglip2-vision-fp32.onnx, and siglip2-vision-fp16.onnx, whose weights are converted to half precision (inputs and
outputs stay float32). The script checks the fp16 file against the fp32 one in onnxruntime, the runtime we ship. The text
tower is not needed at run time: the prompts for defect checks are embedded once at build time.

Safe formats only: safetensors cannot carry code, and torch is never asked to unpickle anything.
"""
import hashlib
import json
import os
import sys
import urllib.request
from pathlib import Path

# The pins live in models/manifest.json (one copy, read by `photodedup check-models` too). Anything this script
# downloads must match the SHA-256 recorded there; anything it builds is compared with the hashes recorded there.
MANIFEST = Path(__file__).resolve().parent.parent / "models" / "manifest.json"
MODEL_ID = "siglip2-base-patch16-224-vision"
_model = next(m for m in json.loads(MANIFEST.read_text())["models"] if m["id"] == MODEL_ID)
REPO = _model["source"]["repo"]
COMMIT = _model["source"]["commit"]
SOURCES = {f["path"]: (f["sha256"], f["size"]) for f in _model["source"]["files"]}
EXPECTED = {a["name"]: a["sha256"] for a in _model["artifacts"]}
OPSET = 17


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch(work):
    """Downloads each source into `work` unless it is already there with the right hash; refuses anything else."""
    for name, (want, size) in SOURCES.items():
        path = work / name
        if not (path.exists() and path.stat().st_size == size and sha256_of(path) == want):
            url = f"https://huggingface.co/{REPO}/resolve/{COMMIT}/{name}"
            print(f"downloading {name} ({size} bytes)", flush=True)
            part = path.with_name(name + ".part")
            urllib.request.urlretrieve(url, part)
            got = sha256_of(part)
            if got != want:
                part.unlink()
                sys.exit(f"{name}: SHA-256 is {got}, expected {want}; refusing to use it")
            part.replace(path)
        print(f"ok {name} sha256 {want}")


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    work, out_dir = Path(sys.argv[1]), Path(sys.argv[2])
    out = out_dir / "siglip2-vision-fp32.onnx"
    out16 = out_dir / "siglip2-vision-fp16.onnx"
    work.mkdir(parents=True, exist_ok=True)
    fetch(work)

    # Imported late so a hash mismatch above stops the run before any of the heavy libraries load.
    import numpy as np
    import onnx
    import onnxruntime
    import torch
    from onnxconverter_common import float16
    from transformers import SiglipVisionModel

    torch.manual_seed(0)
    torch.use_deterministic_algorithms(True)
    model = SiglipVisionModel.from_pretrained(work, use_safetensors=True, local_files_only=True, torch_dtype=torch.float32).eval()

    class ImageEncoder(torch.nn.Module):
        """Pooled image embedding only: the 196x768 patch tokens are not used and would double the output size."""

        def __init__(self, m):
            super().__init__()
            self.m = m

        def forward(self, pixel_values):
            return self.m(pixel_values=pixel_values).pooler_output

    enc = ImageEncoder(model).eval()
    example = torch.zeros(1, 3, 224, 224)
    out.parent.mkdir(parents=True, exist_ok=True)
    with torch.no_grad():
        torch.onnx.export(
            enc,
            (example,),
            str(out),
            input_names=["pixel_values"],
            output_names=["embedding"],
            dynamic_axes={"pixel_values": {0: "batch"}, "embedding": {0: "batch"}},
            opset_version=OPSET,
            dynamo=False,
        )
    onnx.checker.check_model(str(out))
    m = onnx.load(str(out), load_external_data=False)
    external = [t.name for t in m.graph.initializer if t.data_location == onnx.TensorProto.EXTERNAL]
    if external:
        sys.exit(f"export wrote external data files ({len(external)} tensors); expected one self-contained file")

    # Fixed probe: the same input every time, so two builds can be compared by value as well as by hash.
    probe = torch.from_numpy(np.linspace(-1, 1, 3 * 224 * 224, dtype=np.float32).reshape(1, 3, 224, 224))
    with torch.no_grad():
        ref = enc(probe).numpy()
    print(f"wrote {out} ({out.stat().st_size} bytes)")
    print(f"sha256 {sha256_of(out)}")

    # Weights to half precision; keep_io_types leaves the input and output float32 so callers need no change. The
    # patch-embedding Conv stays float32 (about 2 MB): with it converted, the converter leaves that first node with a
    # float32 input and float16 weights and the file does not load (the check below caught exactly that).
    block = ["Conv", *float16.DEFAULT_OP_BLOCK_LIST]
    onnx.save(float16.convert_float_to_float16(onnx.load(str(out)), keep_io_types=True, op_block_list=block), str(out16))
    onnx.checker.check_model(str(out16))
    print(f"wrote {out16} ({out16.stat().st_size} bytes)")
    print(f"sha256 {sha256_of(out16)}")

    def run(path):
        sess = onnxruntime.InferenceSession(str(path), providers=["CPUExecutionProvider"])
        return sess.run(None, {"pixel_values": probe.numpy()})[0]

    a, b = run(out), run(out16)
    cos = float((a * b).sum() / (np.linalg.norm(a) * np.linalg.norm(b)))
    print(f"onnxruntime fp32 vs torch: max abs {np.abs(a - ref).max():.2e}; fp16 vs fp32: cosine {cos:.6f}, max abs {np.abs(a - b).max():.2e}")
    # fp32 must match torch almost exactly. fp16 only has to stay close: onnxruntime's half-precision kernels differ per
    # platform (the same file scored 0.999998 on macOS and 0.99977 on Linux arm64 with this artificial ramp input), so
    # this threshold catches a broken conversion, not rounding. How fp16 behaves on real photos is a separate test.
    if np.abs(a - ref).max() > 1e-3 or cos < 0.999:
        sys.exit("exported files do not agree with the reference; not trusting them")
    print(f"probe embedding sha256 {hashlib.sha256(ref.tobytes()).hexdigest()} first4 {ref[0, :4].tolist()}")
    print(f"torch {torch.__version__} onnx {onnx.__version__} opset {OPSET} threads {torch.get_num_threads()} {os.uname().machine}")

    # The result must be exactly what the manifest records. If it is not, either the recipe changed on purpose (update
    # the manifest in the same commit, then run `photodedup check-models --built <out-dir>`) or something is wrong.
    differs = [f"{p.name}: built {sha256_of(p)}, manifest {EXPECTED[p.name]}" for p in (out, out16) if sha256_of(p) != EXPECTED[p.name]]
    if differs:
        sys.exit("built files differ from models/manifest.json:\n  " + "\n  ".join(differs))
    print("built files match models/manifest.json")


if __name__ == "__main__":
    main()
