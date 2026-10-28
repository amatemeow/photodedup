#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Draws the placeholder app icon (two overlapping photo frames: a pair of
duplicates) as a 1024x1024 PNG, with no dependencies. Replace with real
artwork whenever there is some; `tauri icon <png>` regenerates every size.

    python3 scripts/make-icon.py src-tauri/icon-source.png
"""
import math, struct, sys, zlib

N = 1024

def rounded_rect(x, y, x0, y0, x1, y1, r):
    """Signed distance (negative inside) to a rounded rectangle."""
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    hx, hy = (x1 - x0) / 2 - r, (y1 - y0) / 2 - r
    dx, dy = abs(x - cx) - hx, abs(y - cy) - hy
    return math.hypot(max(dx, 0), max(dy, 0)) + min(max(dx, dy), 0) - r

def cover(d):
    """Anti-aliased coverage from a signed distance."""
    return min(max(0.5 - d, 0.0), 1.0)

def mix(a, b, t):
    return tuple(a[i] * (1 - t) + b[i] * t for i in range(3))

def pixel(x, y):
    # background: a rounded square with a soft vertical gradient
    bg = cover(rounded_rect(x, y, 40, 40, N - 40, N - 40, 200))
    top, bot = (38, 99, 235), (24, 63, 160)
    col = mix(top, bot, y / N)
    out = (col, bg)
    # back frame (the "original"), translucent white
    back = cover(rounded_rect(x, y, 250, 200, 700, 650, 54))
    if back:
        col = mix(col, (255, 255, 255), 0.45 * back)
    # front frame (the "copy"): solid white with a small "picture" in it
    front = cover(rounded_rect(x, y, 340, 320, 790, 770, 54))
    if front:
        col = mix(col, (255, 255, 255), front)
        # sun and hills inside the front frame
        sun = cover(math.hypot(x - 640, y - 450) - 38)
        col = mix(col, (250, 190, 60), sun * front)
        hill = cover(rounded_rect(x, y, 330, 640, 640, 900, 120)) * front
        col = mix(col, (60, 140, 90), hill)
        hill2 = cover(rounded_rect(x, y, 520, 600, 820, 900, 130)) * front
        col = mix(col, (36, 110, 70), hill2)
    return col, bg

def main(path):
    raw = bytearray()
    for y in range(N):
        raw.append(0)  # PNG filter: none
        for x in range(N):
            (r, g, b), a = pixel(x + 0.5, y + 0.5)
            raw += bytes((int(r), int(g), int(b), int(a * 255)))
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0)) \
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b"")
    open(path, "wb").write(png)

if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "icon-source.png")
