"""Check the generated files and execute runtime.lua against a mock engine.

uv run --with lupa python tools/checkerboard/check.py
Mocks validate wrap decisions and group translation, not BeamNG physics.
"""

import hashlib
import json
from pathlib import Path
import struct
import tempfile
import zipfile
import zlib

from lupa import LuaRuntime
import numpy as np

import build


def read_png(path):
    raw = path.read_bytes()
    assert raw[:8] == b"\x89PNG\r\n\x1a\n"
    w, h, bits, color = struct.unpack_from(">IIBB", raw, 16)
    assert bits == 8
    compressed = bytearray()
    off = 8
    while off < len(raw):
        n = struct.unpack_from(">I", raw, off)[0]
        tag, data = raw[off+4:off+8], raw[off+8:off+8+n]
        assert zlib.crc32(tag + data) & 0xffffffff == struct.unpack_from(">I", raw, off+8+n)[0]
        if tag == b"IDAT":
            compressed.extend(data)
        off += 12 + n
    channels = 1 if color == 0 else 3
    rows = np.frombuffer(zlib.decompress(compressed), np.uint8).reshape(h, w*channels+1)
    assert (rows[:, 0] == 0).all()
    return rows[:, 1:].reshape((h, w) if channels == 1 else (h, w, channels))


def check_files(mod):
    level = mod / "levels" / build.LEVEL
    for path in mod.rglob("*.json"):
        if path.name == "items.level.json":
            for line in path.read_text().splitlines():
                json.loads(line)
        else:
            json.loads(path.read_text())
    raw = (level / (build.LEVEL + ".ter")).read_bytes()
    version, size = struct.unpack_from("<BI", raw)
    assert version == 9 and size == 2048
    n = size * size
    assert not any(raw[5:5+2*n])
    layer = np.frombuffer(raw, np.uint8, n, 5+2*n).reshape(size, size)
    off = 5+3*n
    count = struct.unpack_from("<I", raw, off)[0]
    off += 4
    names = []
    for _ in range(count):
        length = raw[off]
        names.append(raw[off+1:off+1+length].decode())
        off += 1+length
    assert off == len(raw)
    mats = json.loads((level / "art" / "terrains" / "main.materials.json").read_text())
    assert [mats[name]["groundmodelName"] for name in names] == list(build.SURFACES)
    # Compare the encoded raster to a separate integer-cell formulation.
    blocks = (np.arange(size) - 974) // 100
    expected = ((blocks[:, None] + blocks[None, :]) % 6).astype(np.uint8)
    np.testing.assert_array_equal(layer, expected)
    np.testing.assert_array_equal(layer[:, 600:], layer[:, :-600])
    np.testing.assert_array_equal(layer[600:, :], layer[:-600, :])
    depth = read_png(level / (build.LEVEL + ".ter.depth.png"))
    assert (depth[layer == 3] == 245).all()
    assert (depth[layer == 5] == 250).all()
    assert (depth[(layer != 3) & (layer != 5)] == 255).all()
    spawns = [json.loads(line) for line in (level / "main" / "MissionGroup"
              / "PlayerDropPoints" / "items.level.json").read_text().splitlines()]
    for i, spawn in enumerate(spawns):
        x, y, _ = spawn["position"]
        assert layer[int((y-build.ORIGIN)/build.SPACING), int((x-build.ORIGIN)/build.SPACING)] == i
    assert build.surface_index(0, 0) == 0
    assert build.surface_index(0, 249.999) == 0
    assert build.surface_index(0, 250) == 1
    assert build.surface_index(0, -250) == 0
    assert build.surface_index(0, -250.001) == 5
    for path in level.rglob("*.png"):
        read_png(path)
    print("Files: JSON, terrain layout, exact boundaries, periodicity, spawns and PNGs passed")


MOCK = r'''
vehicles = {}
core_vehicles = {attachedCouplers = {}}
playerId = 1
physicsRunning = true
be = {}
function log(...) end
function be:getObjectCount() return #vehicles end
function be:getObject(i) return vehicles[i+1] end
function be:getPlayerVehicleID() return playerId end
function be:getPlayerVehicle()
  for _, v in ipairs(vehicles) do if v.id == playerId then return v end end
end
function be:setPhysicsRunning(value) physicsRunning = value end
function addVehicle(id, x, y, z)
  local v = {id=id, x=x, y=y, z=z or 0, vx=31, vy=12, vz=-2, calls=0}
  function v:getID() return self.id end
  function v:getPosition() return {x=self.x,y=self.y,z=self.z} end
  function v:getVelocity() return {x=self.vx,y=self.vy,z=self.vz} end
  function v:setClusterPosRelRot(ref, x, y, z, qx, qy, qz, qw)
    assert(ref == -1 and qx == 0 and qy == 0 and qz == 0 and qw == 1)
    self.x, self.y, self.z = x,y,z
    self.calls = self.calls + 1
  end
  vehicles[#vehicles+1] = v
  return v
end
'''


def check_runtime(mod):
    source = (mod / "levels" / build.LEVEL / "mainLevel.lua").read_text()
    lua = LuaRuntime()
    lua.execute(MOCK)
    lua.globals().source = source
    lua.execute(r'''
function fresh()
  vehicles = {}; core_vehicles = {attachedCouplers={}}; physicsRunning = true
  return assert(load(source))()
end
local m = fresh()
assert(m.status().period == 3000 and m.status().wrapLimit == 2000)
assert(m.surfaceAt(0,-500) == 'GRASS' and m.surfaceAt(0,2500) == 'GRASS')
local a = addVehicle(1,2000,0)
m.onUpdate(0.01,0.01); assert(a.calls == 0)
a.x=2000.5
m.onUpdate(0.01,0); assert(a.calls == 0)
m.onUpdate(0.01,0.01); assert(a.x == -999.5 and a.vx == 31)
m.onUpdate(0.01,0.01); assert(a.calls == 1) -- no oscillation
for _, xy in ipairs({{-2001,0},{0,2001},{0,-2001},{2001,-2001}}) do
  m=fresh(); a=addVehicle(1,xy[1],xy[2]); m.onUpdate(0.01,0.01)
  assert(math.abs(a.x) < 1500 and math.abs(a.y) < 1500)
  assert(m.surfaceAt(a.x,a.y) == m.surfaceAt(xy[1],xy[2]))
end
-- Three trailers, a cyclic/duplicate coupler graph, and uncoupled cargo.
m=fresh(); a=addVehicle(1,2010,2010)
local b=addVehicle(2,1990,2005)
local c=addVehicle(3,1450,2000)
local cargo=addVehicle(4,1990,2005,3)
local parked=addVehicle(5,0,0)
core_vehicles.attachedCouplers={{1,2},{2,3},{3,1},{1,2}}
m.onUpdate(0.01,0.01)
assert(a.x==-990 and b.x==-1010 and c.x==-1550 and cargo.x==-1010)
assert(parked.x==0 and parked.calls==0)
assert(a.calls==1 and b.calls==1 and c.calls==1 and cargo.calls==1)
assert(#m.status().lastWrap.vehicles==4 and m.status().lastWrap.maxVelocityChange==0)
assert(a.x-b.x==20 and a.y-b.y==5 and cargo.z==3)
-- A remote independent vehicle gets its own wrap.
m=fresh(); a=addVehicle(1,0,0); b=addVehicle(2,-2010,0)
m.onUpdate(0.01,0.01); assert(a.calls==0 and b.x==990)
-- Refuse a missing engine graph instead of silently abandoning a trailer.
m=fresh(); a=addVehicle(1,2010,0); core_vehicles={}
m.onUpdate(0.01,0.01)
assert(a.calls==0 and not physicsRunning and m.status().fault)
-- Oversized connected assemblies are rejected before any member is moved.
m=fresh(); a=addVehicle(1,2010,0); b=addVehicle(2,-4500,0)
core_vehicles.attachedCouplers={{1,2}}
m.onUpdate(0.01,0.01)
assert(a.calls==0 and b.calls==0 and not physicsRunning)
-- Fault after a partial move must stop simulation and further automatic wraps.
m=fresh(); a=addVehicle(1,2010,0); b=addVehicle(2,1990,0)
function b:setClusterPosRelRot() error('simulated engine failure') end
m.onUpdate(0.01,0.01)
assert(not physicsRunning and not m.status().enabled and m.status().fault)
local calls=a.calls; m.onUpdate(0.01,0.01); assert(a.calls==calls)
''')
    verifier = (build.HERE / "verify.lua").read_text()
    lua.execute("return assert(load(...))", verifier)
    print("Lua: syntax, boundaries, negative/diagonal wraps, trailer chains, cargo, pause and failure handling passed")


def main():
    with tempfile.TemporaryDirectory(prefix="beam-checkerboard-") as temp:
        out = Path(temp)
        mod, archive = build.build(out)
        check_files(mod)
        check_runtime(mod)
        before = hashlib.sha256(archive.read_bytes()).digest()
        build.build(out)
        assert before == hashlib.sha256(archive.read_bytes()).digest()
        with zipfile.ZipFile(archive) as z:
            assert z.testzip() is None
        print("Reproducible ZIP and archive integrity passed")


if __name__ == "__main__":
    main()
