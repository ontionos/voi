#!/usr/bin/env python3
"""Write Voi's app icon for Windows: the Mac's icon (make_icon.py), drawn at
the sizes Windows asks for and packed into one .ico.

    python3 scripts/make_ico.py assets/voi.ico

The .ico is committed, because the Windows build compiles it into Voi.exe
and a Windows machine need not have Python. Run this again after changing
make_icon.py.
"""

import io
import pathlib
import struct
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import make_icon  # noqa: E402

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]


def png_bytes(size):
    rows = make_icon.render(size)
    buf = io.BytesIO()

    class Sink:
        def write_bytes(self, data):
            buf.write(data)

    make_icon.write_png(Sink(), size, rows)
    return buf.getvalue()


def main():
    out = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "assets/voi.ico")
    images = [(size, png_bytes(size)) for size in SIZES]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    entries, data = b"", b""
    for size, png in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset + len(data))
        data += png
    out.write_bytes(header + entries + data)
    print(f"{out}: {', '.join(str(s) for s, _ in images)} px, {out.stat().st_size} bytes")


if __name__ == "__main__":
    main()
