"""Reader/writer for BeamNG.drive's binary .ter terrain format.

The format was recovered by decoding levels/smallgrid/smallgrid.ter and is
verified byte-for-byte by round_trip_check() below -- run this module directly
to re-assert that against a real game file.

    u8                version         (7 or 9 in the shipped levels)
    u32               size            heightmap edge, a power of two
    u16[size*size]    height          row-major; metres = raw / 65535 * maxHeight
    u8 [size*size]    materialIdx     index into the name list that follows
    u32               materialCount
      u8 nameLen + nameLen bytes      the TerrainMaterial's *internalName*

The material names are `internalName` values, NOT object names -- confirmed
against small_island, whose .ter list matches its art/terrains material file
exactly. Get that wrong and the terrain loads with no surface at all.
"""

import struct

SUPPORTED_VERSIONS = (7, 9)
DEFAULT_VERSION = 9


class Terrain:
    def __init__(self, size, heights, material_idx, material_names, version=DEFAULT_VERSION):
        self.version = version
        self.size = size
        self.heights = heights            # bytes/bytearray, size*size uint16 LE
        self.material_idx = material_idx  # bytes/bytearray, size*size uint8
        self.material_names = list(material_names)


def read(path):
    raw = open(path, "rb").read()
    version = raw[0]
    if version not in SUPPORTED_VERSIONS:
        raise ValueError("unexpected .ter version %d (known: %s)" % (version, SUPPORTED_VERSIONS))
    size = struct.unpack_from("<I", raw, 1)[0]
    off = 5
    heights = raw[off:off + size * size * 2]
    off += size * size * 2
    material_idx = raw[off:off + size * size]
    off += size * size
    count = struct.unpack_from("<I", raw, off)[0]
    off += 4
    names = []
    for _ in range(count):
        n = raw[off]
        off += 1
        names.append(raw[off:off + n].decode("utf-8"))
        off += n
    if off != len(raw):
        raise ValueError("trailing bytes: consumed %d of %d" % (off, len(raw)))
    return Terrain(size, heights, material_idx, names, version)


def write(path, terrain):
    size = terrain.size
    if len(terrain.heights) != size * size * 2:
        raise ValueError("heights wrong length")
    if len(terrain.material_idx) != size * size:
        raise ValueError("material_idx wrong length")
    out = bytearray([terrain.version])
    out += struct.pack("<I", size)
    out += terrain.heights
    out += terrain.material_idx
    out += struct.pack("<I", len(terrain.material_names))
    for name in terrain.material_names:
        b = name.encode("utf-8")
        if len(b) > 255:
            raise ValueError("material name too long: %s" % name)
        out += bytes([len(b)]) + b
    with open(path, "wb") as f:
        f.write(out)
    return len(out)


def round_trip_check(path):
    """Parse a real game .ter and re-emit it; the bytes must be identical."""
    import tempfile, os
    original = open(path, "rb").read()
    t = read(path)
    fd, tmp = tempfile.mkstemp(suffix=".ter")
    os.close(fd)
    try:
        write(tmp, t)
        return open(tmp, "rb").read() == original, t
    finally:
        os.unlink(tmp)


if __name__ == "__main__":
    import sys
    target = sys.argv[1] if len(sys.argv) > 1 else None
    if not target:
        print("usage: python terfile.py <path to a .ter>")
        raise SystemExit(2)
    ok, t = round_trip_check(target)
    print("size=%d materials=%s" % (t.size, t.material_names))
    print("ROUND TRIP IDENTICAL:", ok)
    raise SystemExit(0 if ok else 1)
