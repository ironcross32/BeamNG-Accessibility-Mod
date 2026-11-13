"""Write icon-source.png, the placeholder app icon, with the standard library only.

Regenerate the icon set from it with (from rust/):
    uv run python src-tauri/icons/make_source_icon.py
    npx tauri icon src-tauri/icons/icon-source.png -o src-tauri/icons
"""

import os
import struct
import zlib

SIZE = 512


def pixel(x, y):
    # Dark rounded square with a light concentric ring: a compass-like mark.
    cx = cy = (SIZE - 1) / 2
    r = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
    margin = 24
    corner = 96
    dx = max(margin + corner - x, 0, x - (SIZE - 1 - margin - corner))
    dy = max(margin + corner - y, 0, y - (SIZE - 1 - margin - corner))
    if (dx * dx + dy * dy) ** 0.5 > corner:
        return (0, 0, 0, 0)
    if 150 <= r <= 190 or (abs(x - cx) < 16 and y < cy and r < 150):
        return (240, 200, 60, 255)
    return (24, 40, 72, 255)


def main():
    rows = bytearray()
    for y in range(SIZE):
        rows.append(0)
        for x in range(SIZE):
            rows.extend(pixel(x, y))

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(rows), 9))
    png += chunk(b"IEND", b"")
    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "icon-source.png")
    with open(out, "wb") as fh:
        fh.write(png)
    print(out)


if __name__ == "__main__":
    main()
