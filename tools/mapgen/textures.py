"""Procedural terrain textures for the generated level.

Deliberately crude. This map is built for players who cannot see it, so the
textures exist to stop the terrain rendering as untextured white -- not to look
good. What they DO carry is a per-surface colour difference big enough that a
sighted person helping to test can tell the sections apart at a glance.

Written with zlib + struct rather than PIL so the generator has no dependency
beyond numpy, which the project already ships.
"""

import struct
import zlib

import numpy as np

PNG_MAGIC = bytes([137, 80, 78, 71, 13, 10, 26, 10])


def write_png(path, rgb):
    """rgb: uint8 array of shape (h, w, 3)."""
    h, w, _ = rgb.shape
    raw = np.zeros((h, w * 3 + 1), dtype=np.uint8)
    raw[:, 1:] = rgb.reshape(h, w * 3)   # filter byte 0 (None) per scanline
    data = zlib.compress(raw.tobytes(), 6)

    def chunk(tag, payload):
        return (struct.pack(">I", len(payload)) + tag + payload
                + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF))

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", data))
        f.write(chunk(b"IEND", b""))


def write_gray_png(path, gray):
    """gray: uint8 array of shape (h, w). Colour type 0, which is what the
    engine's terrain depth map is (checked against four shipped levels)."""
    h, w = gray.shape
    raw = np.zeros((h, w + 1), dtype=np.uint8)
    raw[:, 1:] = gray
    data = zlib.compress(raw.tobytes(), 6)

    def chunk(tag, payload):
        return (struct.pack(">I", len(payload)) + tag + payload
                + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF))

    with open(path, "wb") as f:
        f.write(PNG_MAGIC)
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 0, 0, 0, 0)))
        f.write(chunk(b"IDAT", data))
        f.write(chunk(b"IEND", b""))


def noisy(base_rgb, spread=18, size=512, seed=0, streak=0.0):
    """A flat colour with per-pixel grain, optionally smeared along one axis."""
    rng = np.random.default_rng(seed)
    n = rng.normal(0.0, spread, size=(size, size, 1))
    if streak > 0:
        # cheap directional blur so asphalt/dirt do not read as pure static
        k = max(1, int(streak))
        n = np.apply_along_axis(
            lambda m: np.convolve(m, np.ones(k) / k, mode="same"), 0, n)
    img = np.asarray(base_rgb, dtype=float).reshape(1, 1, 3) + n
    return np.clip(img, 0, 255).astype(np.uint8)


# One entry per terrain material. The colour is the only thing tuned here.
PALETTE = {
    "ground_gravel": ((122, 116, 104), 20, 3),
    "road_asphalt":  ((58, 58, 62),    10, 6),
    "ground_dirt":   ((124, 96, 66),   22, 3),
    "ground_mud":    ((62, 47, 33),    16, 2),
    "ground_rock":   ((104, 100, 96),  26, 1),
    # High-contrast on purpose: this is the one surface a sighted tester needs to
    # pick out at a glance to check the lane dividers landed where they should.
    "ground_rumble": ((170, 96, 88),   28, 1),
}


def generate_all(out_dir, size=512):
    """Write one <name>.png per material. Returns {name: filename}."""
    import os
    os.makedirs(out_dir, exist_ok=True)
    written = {}
    for i, (name, (rgb, spread, streak)) in enumerate(sorted(PALETTE.items())):
        fn = "t_%s.png" % name
        write_png(os.path.join(out_dir, fn), noisy(rgb, spread, size, seed=i, streak=streak))
        written[name] = fn
    return written
