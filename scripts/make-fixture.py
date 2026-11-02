#!/usr/bin/env python3
"""Builds a synthetic photo library for testing: every case the engine handles, none of it personal.

    scripts/make-fixture.py <out-dir> [--heic-from <folder>]

Needs only Python 3 and ffmpeg. The pictures are generated patterns with real edges (so hashes and
sharpness behave like they do on photos), and the EXIF is written here, byte by byte, so no tool beyond
ffmpeg is involved. Run the engine's fixture test against the result:

    PHOTODEDUP_FIXTURE=<out-dir> cargo test --release -p photodedup-core --test fixture -- --ignored

HEIC cannot be generated, so `--heic-from` copies a few from a folder of your own (they carry whatever
that folder's photos carry, GPS included: keep the output out of git). Without it the HEIC cases are
left out and the test says so.
"""
import argparse
import math
import random
import shutil
import struct
import subprocess
import sys
from pathlib import Path

W, H = 1280, 960
CAMERA = ("FixtureCam", "F1")

# The HEICs `--heic-from` looks for, by the roles they play (two taken moments apart, one on its own).
HEIC_PICKS = ["IMG_3244.HEIC", "IMG_3245.HEIC", "IMG_3796.HEIC"]


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-v", "error", "-y", *map(str, args)], check=True)


def scene(path, seed, dx=0, dy=0, gain=1.0, blur=0.0):
    """A pattern with smooth colour fields and hard-edged boxes, different for every seed."""
    r = random.Random(seed)
    f = [r.uniform(0.004, 0.02) for _ in range(6)]
    ph = [r.uniform(0, 6.28) for _ in range(3)]
    chan = []
    for i in range(3):
        chan.append(
            f"{gain:.3f}*(128+110*sin((X+{dx})*{f[i]:.5f}+{ph[i]:.3f})*cos((Y+{dy})*{f[i + 3]:.5f}))"
        )
    boxes = []
    for _ in range(14):
        w, h = r.randint(40, 260), r.randint(40, 200)
        x, y = r.randint(0, W - w), r.randint(0, H - h)
        c = "0x%06x" % r.randrange(0x1000000)
        boxes.append(f"drawbox=x={x + dx}:y={y + dy}:w={w}:h={h}:color={c}:t=fill")
    vf = ",".join(["format=gbrp", f"geq=r='{chan[0]}':g='{chan[1]}':b='{chan[2]}'", *boxes])
    if blur:
        vf += f",gblur=sigma={blur}"
    ffmpeg("-f", "lavfi", "-i", f"nullsrc=s={W}x{H}", "-vf", vf, "-frames:v", "1", "-q:v", "3", path)


def flat(path, colour):
    ffmpeg("-f", "lavfi", "-i", f"color=c={colour}:s={W}x{H}", "-frames:v", "1", "-q:v", "3", path)


def night(path):
    """Dark on purpose: a dark field with lit windows. Must not be called a defect."""
    r = random.Random(77)
    boxes = []
    for _ in range(60):
        w, h = r.randint(30, 120), r.randint(30, 120)
        boxes.append(f"drawbox=x={r.randint(0, W - w)}:y={r.randint(0, H - h)}:w={w}:h={h}:color=0xffd890:t=fill")
    ffmpeg("-f", "lavfi", "-i", f"color=c=0x050508:s={W}x{H}", "-vf", ",".join(boxes), "-frames:v", "1", "-q:v", "3", path)


def exif(make, model, when, subsec="00"):
    """A minimal EXIF APP1 segment: Make, Model, DateTimeOriginal, SubSecTimeOriginal."""

    def ascii_val(s):
        return s.encode() + b"\0"

    # IFD0 holds Make, Model and a pointer to the Exif IFD; the Exif IFD holds the date and sub-seconds.
    ifd0 = [(0x010F, 2, ascii_val(make)), (0x0110, 2, ascii_val(model)), (0x8769, 4, None)]
    exif_ifd = [(0x9003, 2, ascii_val(when)), (0x9291, 2, ascii_val(subsec))]

    def layout(entries, base, patch=None):
        n = len(entries)
        table_size = 2 + 12 * n + 4
        data_at = base + table_size
        table, data = struct.pack("<H", n), b""
        for tag, typ, val in entries:
            if val is None:  # the Exif IFD pointer
                table += struct.pack("<HHII", tag, typ, 1, patch)
            elif len(val) <= 4:
                table += struct.pack("<HHI", tag, typ, len(val)) + val.ljust(4, b"\0")
            else:
                table += struct.pack("<HHII", tag, typ, len(val), data_at + len(data))
                data += val + (b"\0" if len(val) % 2 else b"")
        return table + struct.pack("<I", 0) + data

    header = b"II*\0" + struct.pack("<I", 8)
    ifd0_len = len(layout(ifd0, 8, patch=0))
    tiff = header + layout(ifd0, 8, patch=8 + ifd0_len) + layout(exif_ifd, 8 + ifd0_len)
    body = b"Exif\0\0" + tiff
    return b"\xff\xe1" + struct.pack(">H", len(body) + 2) + body


def stamp(path, when, make=CAMERA[0], model=CAMERA[1], subsec="00"):
    """Writes camera EXIF into a JPEG (after its SOI marker). `when` is `YYYY:MM:DD HH:MM:SS`."""
    data = Path(path).read_bytes()
    assert data[:2] == b"\xff\xd8"
    Path(path).write_bytes(data[:2] + exif(make, model, when, subsec) + data[2:])


def video(path, seconds, created, colour_seed):
    codec = ["-c:v", "libx264", "-pix_fmt", "yuv420p"] if has_encoder("libx264") else ["-c:v", "mpeg4"]
    ffmpeg(
        "-f", "lavfi", "-i", f"testsrc2=s=320x240:r=15:d={seconds}",
        "-vf", f"hue=h={colour_seed}", *codec,
        "-metadata", f"creation_time={created}", path,
    )


def has_encoder(name):
    out = subprocess.run(["ffmpeg", "-v", "quiet", "-encoders"], capture_output=True, text=True).stdout
    return name in out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--heic-from")
    a = ap.parse_args()
    out = Path(a.out)
    if out.exists() and any(out.iterdir()):
        sys.exit(f"{out} is not empty: pick a new folder")
    out.mkdir(parents=True, exist_ok=True)
    p = lambda name: out / name

    # Six unrelated photos, hours apart: nothing should group.
    for i in range(6):
        scene(p(f"IMG_{1000 + i}.jpg"), seed=100 + i)
        stamp(p(f"IMG_{1000 + i}.jpg"), f"2026:10:01 {9 + 2 * i:02d}:00:00")

    # An exact copy of one of them.
    shutil.copyfile(p("IMG_1000.jpg"), p("IMG_1000 (1).jpg"))

    # A messenger-style copy of another: smaller, re-encoded, no EXIF. The original is the keeper.
    ffmpeg("-i", p("IMG_1001.jpg"), "-vf", "scale=960:-1", "-q:v", "6", p("IMG-20261001-WA0001.jpg"))

    # A quick sequence: four frames of one scene, two seconds apart, drifting a little.
    for k in range(4):
        scene(p(f"IMG_2{k:03d}.jpg"), seed=200, dx=k * 5, gain=1.0 + 0.01 * k)
        stamp(p(f"IMG_2{k:03d}.jpg"), f"2026:10:02 12:00:{2 * k:02d}", subsec=f"{k}0")

    # Look-alikes: the same scene twice, half a minute apart (too far for a sequence).
    scene(p("IMG_3000.jpg"), seed=300)
    scene(p("IMG_3001.jpg"), seed=300, dx=14)
    stamp(p("IMG_3000.jpg"), "2026:10:03 15:00:00")
    stamp(p("IMG_3001.jpg"), "2026:10:03 15:00:30")

    # Defects the checks should find ...
    flat(p("IMG_4000.jpg"), "black")
    flat(p("IMG_4001.jpg"), "0xf8f8f8")
    flat(p("IMG_4002.jpg"), "0x808080")
    scene(p("IMG_4003.jpg"), seed=400, blur=40)
    # ... and a dark photo they must leave alone.
    night(p("IMG_4004.jpg"))
    for i, hour in enumerate(range(8, 13)):
        stamp(p(f"IMG_400{i}.jpg"), f"2026:10:04 {hour:02d}:00:00")

    # Files with no EXIF at all: a JPEG, and a PNG screenshot.
    scene(p("download.jpg"), seed=500)
    # ... one that wears a camera's file name but has no EXIF: it must read as a copy, not as an original.
    scene(p("IMG_5000.JPG"), seed=510)
    ffmpeg("-f", "lavfi", "-i", f"testsrc2=s=1170x2532", "-frames:v", "1", p("Screenshot 2026-10-05.png"))

    # Videos: two clips of different length, with creation times.
    video(p("VID_0001.mp4"), 2, "2026-10-06T10:00:00Z", 0)
    video(p("VID_0002.mp4"), 3, "2026-10-06T11:00:00Z", 90)

    if a.heic_from:
        src = Path(a.heic_from).expanduser()
        found = [n for n in HEIC_PICKS if (src / n).is_file()]
        for n in found:
            shutil.copyfile(src / n, p(n))
        print(f"copied {len(found)} of {len(HEIC_PICKS)} HEIC files from {src}")
    else:
        print("no --heic-from: the HEIC cases are left out")
    print(f"wrote {sum(1 for f in out.iterdir())} files to {out}")


if __name__ == "__main__":
    main()
