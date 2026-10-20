#!/usr/bin/env python3
"""Draw the picture behind the two icons in the disk image window.

An arrow from one to the other, which is the whole instruction: drag this
onto that. Generated for the same reason the app icon is — the shape lives
in the numbers below rather than in a file nobody can edit.

Written at 1x and 2x and combined into one TIFF, because a flat PNG at
window size is upscaled on a retina display and the arrow goes soft.

Anti-aliasing is analytic: coverage comes from the signed distance to the
shape, and only the arrow's own few hundred pixels need it.
"""

import math
import pathlib
import struct
import subprocess
import sys
import tempfile
import zlib

# Content area of the window, in points. Everything below is in these units;
# the renderer multiplies by the scale it is drawing at.
WIDTH = 520
HEIGHT = 340

# Where the icons sit. Kept here rather than in the shell script so the
# drawing and the icon placement cannot drift apart — dmg.sh reads them back
# out of this file.
ICON_Y = 148
APP_X = 130
DEST_X = 390

# The arrow, centred on the icons' line.
TAIL_X = 214
HEAD_X = 306
SHAFT = 9.0                # thickness of the line
HEAD_LEN = 30.0            # how far back from the point the barbs start
HEAD_HALF = 16.0           # half the width across the barbs
ROUND = 2.5                # corner rounding on the head

# Paper and ink. A light warm grey rather than white, so the app icon — which
# is a red dot on a white tile — keeps its edge instead of melting into the
# background.
TOP = (0.976, 0.973, 0.969)
BOTTOM = (0.941, 0.937, 0.929)
ARROW = (0.788, 0.780, 0.769)


# Ordered dither, a quarter of a value each way, in the usual 2×2 order.
BAYER = (-0.375, 0.125, 0.375, -0.125)


def capsule_distance(x, y, ax, ay, bx, by, radius):
    """Signed distance to a thick line with round ends."""
    px, py = x - ax, y - ay
    dx, dy = bx - ax, by - ay
    t = (px * dx + py * dy) / (dx * dx + dy * dy)
    t = min(max(t, 0.0), 1.0)
    return math.hypot(px - dx * t, py - dy * t) - radius


def head_distance(x, y, tip_x, mid_y, length, half, radius):
    """Signed distance to the triangular head, corners rounded by `radius`.

    Three half-planes, taken at their maximum. Inside a convex shape that is
    exactly the distance to the boundary; outside it only underestimates near
    the corners, and the corners are rounded off anyway.
    """
    # Shrunk by the rounding, so growing it back leaves the size asked for.
    back = tip_x - length + radius
    tip = tip_x - radius
    barb = half - radius

    # The flat back edge, its outward normal pointing left.
    d = back - x

    # The two slanted edges, each as a barb and the outward normal of the
    # line running from it to the tip.
    for side in (-1.0, 1.0):
        ey = mid_y + side * barb
        vx, vy = tip - back, mid_y - ey
        n = math.hypot(vx, vy)
        # Rotate the edge direction outwards: up for the top edge, down for
        # the bottom one.
        d = max(d, ((x - back) * (side * vy) - (y - ey) * (side * vx)) / n * -1.0)

    return d - radius


# How far the picture extends past the window it was designed for.
#
# Finder draws a background at its natural size from the top-left corner and
# never scales it, so a window dragged wider or taller than the picture shows
# bare white past its edge. Painted this far out, a stretched window is the
# same window, bigger.
CANVAS_W = 2000
CANVAS_H = 1250


def render(scale):
    w, h = CANVAS_W * scale, CANVAS_H * scale
    feather = 1.0  # one device pixel

    # The arrow's box in device pixels, with room for its soft edge. Only these
    # pixels need the distance field; everything else is the paper colour.
    x0 = int((TAIL_X - SHAFT) * scale) - 2
    x1 = int((HEAD_X + ROUND) * scale) + 2
    y0 = int((ICON_Y - HEAD_HALF - ROUND) * scale) - 2
    y1 = int((ICON_Y + HEAD_HALF + ROUND) * scale) + 2

    def pixel(c, n):
        return bytes(max(0, min(255, round(v * 255 + n))) for v in c)

    rows = []
    flat = {}
    for j in range(h):
        y = (j + 0.5) / scale
        # The gradient runs down the designed height and then holds its last
        # colour, so a taller window does not keep getting darker.
        t = min(y / HEIGHT, 1.0)
        if t >= 1.0 and j > y1:
            # Below the gradient every row is the same flat colour. No dither:
            # there is nothing there to band.
            if "row" not in flat:
                flat["row"] = pixel(BOTTOM, 0.0) * w
            rows.append(flat["row"])
            continue

        base = tuple(TOP[k] + (BOTTOM[k] - TOP[k]) * t for k in range(3))
        # A gradient this gentle crosses only nine values from top to bottom,
        # which shows up as bands. A quarter-step of ordered dither breaks them
        # up and is itself invisible.
        even = pixel(base, BAYER[(j & 1) * 2])
        odd = pixel(base, BAYER[(j & 1) * 2 + 1])
        row = bytearray((even + odd) * (w // 2) + (even if w % 2 else b""))

        if y0 <= j <= y1:
            for i in range(max(0, x0), min(w, x1 + 1)):
                x = (i + 0.5) / scale
                shaft = capsule_distance(
                    x, y, TAIL_X, ICON_Y, HEAD_X - HEAD_LEN + 4.0, ICON_Y, SHAFT / 2.0
                )
                head = head_distance(x, y, HEAD_X, ICON_Y, HEAD_LEN, HEAD_HALF, ROUND)
                ink = min(max(0.5 - min(shaft, head) * scale / feather, 0.0), 1.0)
                if ink <= 0.0:
                    continue
                c = tuple(base[k] + (ARROW[k] - base[k]) * ink for k in range(3))
                row[i * 3 : i * 3 + 3] = pixel(c, BAYER[(j & 1) * 2 + (i & 1)])
        rows.append(bytes(row))
    return w, h, rows


def write_png(path, w, h, rows):
    raw = b"".join(b"\x00" + bytes(row) for row in rows)

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    png = b"\x89PNG\r\n\x1a\n"
    # Colour type 2: RGB, no alpha. The window behind it is never transparent.
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 6))
    png += chunk(b"IEND", b"")
    path.write_bytes(png)


def main():
    out = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/dmg-background.tiff")
    out.parent.mkdir(parents=True, exist_ok=True)

    # The two PNGs only exist to be joined; they are nobody's output, so they
    # are written somewhere that disappears.
    with tempfile.TemporaryDirectory() as room:
        parts = []
        for scale in (1, 2):
            w, h, rows = render(scale)
            png = pathlib.Path(room) / f"{out.stem}@{scale}x.png"
            write_png(png, w, h, rows)
            parts.append(str(png))
            print(f"  {w}×{h}", flush=True)

        # One file holding both sizes, so Finder picks the right one per display.
        subprocess.run(["tiffutil", "-cathidpicheck", *parts, "-out", str(out)], check=True)
    print(out)


if __name__ == "__main__":
    main()
