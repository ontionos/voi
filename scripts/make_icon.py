#!/usr/bin/env python3
"""Render Voi's app icon: a glowing red dot on white.

Generated rather than committed as a binary, so the shape is defined by the
numbers below and can be adjusted without an image editor. Writes a full
.iconset and hands it to iconutil.

Anti-aliasing is analytic — coverage from the signed distance to each edge —
rather than supersampled, which keeps a 1024px render under a couple of
seconds in plain Python.
"""

import math
import pathlib
import shutil
import struct
import subprocess
import sys
import zlib

# Fractions of the icon's width.
TILE_INSET = 0.09          # macOS leaves the outer edge clear
TILE_RADIUS = 0.185        # rounded-square corner
DOT_RADIUS = 0.155
GLOW_RADIUS = 0.40         # where the halo has faded to nothing
GLOW_FALLOFF = 0.085       # how tightly the halo hugs the dot

DOT = (0.92, 0.19, 0.16)
GLOW = (1.00, 0.24, 0.19)


def rounded_rect_distance(x, y, half, radius):
    """Signed distance to a rounded square centred on the origin."""
    dx = abs(x) - (half - radius)
    dy = abs(y) - (half - radius)
    outside = math.hypot(max(dx, 0.0), max(dy, 0.0))
    inside = min(max(dx, dy), 0.0)
    return outside + inside - radius


def coverage(distance, feather):
    """1 well inside, 0 well outside, a smooth ramp across the edge."""
    return min(max(0.5 - distance / feather, 0.0), 1.0)


def render(size):
    px = 1.0 / size
    half = 0.5 - TILE_INSET
    rows = []

    for j in range(size):
        y = (j + 0.5) * px - 0.5
        row = bytearray()
        for i in range(size):
            x = (i + 0.5) * px - 0.5

            tile = coverage(rounded_rect_distance(x, y, half, TILE_RADIUS), px)
            if tile <= 0.0:
                row += b"\x00\x00\x00\x00"
                continue

            d = math.hypot(x, y)
            dot = coverage(d - DOT_RADIUS, px)
            # Gaussian halo, clipped to the tile so it cannot bleed off the icon.
            glow = math.exp(-((max(d - DOT_RADIUS, 0.0) / GLOW_FALLOFF) ** 2))
            if d > GLOW_RADIUS:
                glow = 0.0

            r, g, b = 1.0, 1.0, 1.0
            for weight, (cr, cg, cb) in ((glow * 0.55, GLOW), (dot, DOT)):
                r += (cr - r) * weight
                g += (cg - g) * weight
                b += (cb - b) * weight

            row += bytes(
                (
                    round(r * 255),
                    round(g * 255),
                    round(b * 255),
                    round(tile * 255),
                )
            )
        rows.append(row)
    return rows


def write_png(path, size, rows):
    raw = b"".join(b"\x00" + bytes(row) for row in rows)

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    path.write_bytes(png)


def main():
    out = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/Voi.iconset")
    out.mkdir(parents=True, exist_ok=True)

    # (pixel size, iconset names it satisfies)
    wanted = {
        16: ["icon_16x16.png"],
        32: ["icon_16x16@2x.png", "icon_32x32.png"],
        64: ["icon_32x32@2x.png"],
        128: ["icon_128x128.png"],
        256: ["icon_128x128@2x.png", "icon_256x256.png"],
        512: ["icon_256x256@2x.png", "icon_512x512.png"],
        1024: ["icon_512x512@2x.png"],
    }

    for size, names in sorted(wanted.items()):
        rows = render(size)
        for name in names:
            write_png(out / name, size, rows)
        print(f"  {size}px", flush=True)

    icns = out.with_suffix(".icns")
    subprocess.run(["iconutil", "-c", "icns", str(out), "-o", str(icns)], check=True)
    # The iconset was scaffolding for iconutil; the .icns is the output.
    shutil.rmtree(out)
    print(icns)


if __name__ == "__main__":
    main()
