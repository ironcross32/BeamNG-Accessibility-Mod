"""Build the standalone Endless Surfaces prototype; no Proving Grounds imports.

uv run python tools/checkerboard/build.py
uv run python tools/checkerboard/build.py --install
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import uuid
import zipfile
import zlib

import numpy as np

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
LEVEL = "endless_surfaces"
MOD = "beam_endless_surfaces"
GRID = 2048
SPACING = 5.0
ORIGIN = -5120.0
BLOCK = 500.0
SURFACES = ("ASPHALT", "DIRT", "GRAVEL", "SAND", "ICE", "GRASS")
PERIOD = BLOCK * len(SURFACES)
WRAP_LIMIT = PERIOD / 2 + BLOCK
COLORS = ((55, 58, 64), (128, 88, 52), (139, 137, 130),
          (214, 185, 121), (174, 222, 237), (77, 125, 52))
NS = uuid.UUID("c33a1354-e7db-4113-bcc5-a3815bb2bed2")


def pid(name):
    return str(uuid.uuid5(NS, name))


def surface_index(x, y):
    # Half-open material cells, with the origin at the centre of asphalt.
    return (np.floor((np.asarray(x) + BLOCK / 2) / BLOCK).astype(int)
            + np.floor((np.asarray(y) + BLOCK / 2) / BLOCK).astype(int)) % len(SURFACES)


def write_png(path, pixels):
    pixels = np.asarray(pixels, dtype=np.uint8)
    h, w = pixels.shape[:2]
    channels = 1 if pixels.ndim == 2 else 3
    raw = np.zeros((h, w * channels + 1), dtype=np.uint8)
    raw[:, 1:] = pixels.reshape(h, w * channels)

    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff))

    path.write_bytes(b"\x89PNG\r\n\x1a\n"
                     + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8,
                                                  0 if channels == 1 else 2, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(raw.tobytes(), 6))
                     + chunk(b"IEND", b""))


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def write_items(path, objects):
    path.parent.mkdir(parents=True, exist_ok=True)
    for obj in objects:
        obj.setdefault("persistentId", pid(obj["name"]))
    path.write_text("".join(json.dumps(o, separators=(",", ":")) + "\n"
                            for o in objects), encoding="utf-8")


def build(out):
    mod_dir = out.resolve() / MOD
    level = mod_dir / "levels" / LEVEL
    art = level / "art" / "terrains"
    art.mkdir(parents=True, exist_ok=True)
    names = ["es_" + s.lower() for s in SURFACES]
    axis = ORIGIN + np.arange(GRID) * SPACING
    layer = surface_index(axis[None, :], axis[:, None]).astype(np.uint8)

    # BeamNG v9 .ter: flat node heights, per-cell material indices, name table.
    terrain = bytearray(struct.pack("<BI", 9, GRID))
    terrain.extend(bytes(GRID * GRID * 2))
    terrain.extend(layer.tobytes())
    terrain.extend(struct.pack("<I", len(names)))
    for name in names:
        encoded = name.encode("utf-8")
        terrain.extend(bytes([len(encoded)]) + encoded)
    (level / (LEVEL + ".ter")).write_bytes(terrain)
    # Depth PNG convention: 255 means solid, each decrement is one centimetre.
    depth = np.full(layer.shape, 255, dtype=np.uint8)
    depth[layer == SURFACES.index("SAND")] = 245
    depth[layer == SURFACES.index("GRASS")] = 250
    write_png(level / (LEVEL + ".ter.depth.png"), depth)

    materials = {}
    for i, (name, ground, color) in enumerate(zip(names, SURFACES, COLORS)):
        rng = np.random.default_rng(i)
        grain = rng.normal(0, 8, (256, 256, 1))
        write_png(art / (name + ".png"), np.clip(np.array(color) + grain, 0, 255))
        texture = f"/levels/{LEVEL}/art/terrains/{name}.png"
        materials[name] = {
            "name": name, "internalName": name, "class": "TerrainMaterial",
            "persistentId": pid(name), "groundmodelName": ground,
            "diffuseMap": texture, "detailMap": texture,
            "diffuseSize": 500, "detailSize": 4, "detailStrength": 0.5,
            "detailDistance": 100, "useSideProjection": False,
        }
    write_json(art / "main.materials.json", materials)
    palette = np.array(COLORS, dtype=np.uint8)
    write_png(level / "minimap.png", palette[layer[::4, ::4]][::-1])
    preview_axis = np.linspace(-PERIOD / 2, PERIOD / 2, 600, endpoint=False)
    preview = palette[surface_index(preview_axis[None, :], preview_axis[:, None])]
    write_png(level / "preview.png", preview[::-1])

    main = level / "main"
    write_items(main / "items.level.json", [{"name": "MissionGroup", "class": "SimGroup"}])
    groups = ("Terrain", "Sky", "PlayerDropPoints")
    write_items(main / "MissionGroup" / "items.level.json", [
        {"name": group, "class": "SimGroup", "__parent": "MissionGroup"}
        for group in groups])
    write_items(main / "MissionGroup" / "Terrain" / "items.level.json", [{
        "name": "theTerrain", "class": "TerrainBlock", "__parent": "Terrain",
        "position": [ORIGIN, ORIGIN, 0], "squareSize": SPACING,
        "maxHeight": 256, "baseTexSize": 2048,
        "terrainFile": f"/levels/{LEVEL}/{LEVEL}.ter",
        "minimapImage": f"levels/{LEVEL}/minimap.png",
    }])
    write_items(main / "MissionGroup" / "Sky" / "items.level.json", [
        {"name": "theLevelInfo", "class": "LevelInfo", "__parent": "Sky",
         "gravity": -9.81, "visibleDistance": 2500, "fogDensity": 0.0006,
         "fogAtmosphereHeight": 1000, "fogColor": [0.74, 0.82, 0.93, 1],
         "globalEnviromentMap": "BNG_Sky_01_cubemap",
         "temperatureCurveC": [[0, 18], [1, 18]]},
        {"name": "sunsky", "class": "ScatterSky", "__parent": "Sky",
         "azimuth": 57.3, "elevation": 144, "skyBrightness": 40,
         "ambientScale": [1, 0.894, 0.78, 1],
         "ambientScaleGradientFile": "art/sky_gradients/default/gradient_ambient.png",
         "colorize": [0.22, 0.35, 0.61, 1],
         "colorizeGradientFile": "art/sky_gradients/default/gradient_colorize.png",
         "fogScale": [0.396, 0.667, 1, 1],
         "fogScaleGradientFile": "art/sky_gradients/default/gradient_fog.png",
         "sunScale": [0.996, 0.812, 0.706, 1],
         "sunScaleGradientFile": "art/sky_gradients/default/gradient_sunscale.png",
         "nightCubemap": "nightCubemap", "useNightCubemap": True,
         "nightGradientFile": "art/sky_gradients/default/gradient_ambient.png",
         "nightFogGradientFile": "art/sky_gradients/default/gradient_fog.png",
         "mieScattering": 0.00043},
        {"name": "tod", "class": "TimeOfDay", "__parent": "Sky",
         "axisTilt": 10, "play": False, "startTime": 0.18, "time": 0.18},
    ])
    spawns = []
    spawn_info = []
    for i, surface in enumerate(SURFACES):
        name = "spawn_" + surface.lower()
        spawns.append({"name": name, "class": "SpawnSphere", "__parent": "PlayerDropPoints",
                       "position": [0, (i * BLOCK + PERIOD / 2) % PERIOD - PERIOD / 2, 0.5],
                       "dataBlock": "SpawnSphereMarker", "radius": 5,
                       # SpawnSphere applies another half turn: the car faces north.
                       "rotationMatrix": [-1, 0, 0, 0, -1, 0, 0, 0, 1]})
        spawn_info.append({"objectname": name, "translationId": surface.title(),
                           "preview": "preview.png"})
    write_items(main / "MissionGroup" / "PlayerDropPoints" / "items.level.json", spawns)
    write_json(level / "info.json", {
        "title": "Endless Surfaces (Prototype)",
        "description": "500 m asphalt, dirt, gravel, sand, ice and grass squares. "
                       "Vehicle groups wrap through a repeating flat terrain.",
        "authors": "BeamTel", "size": [int(GRID * SPACING)] * 2,
        "previews": ["preview.png"], "defaultSpawnPointName": "spawn_asphalt",
        "spawnPoints": spawn_info, "supportsTraffic": False,
    })
    config = ("local SURFACES = {" + ", ".join(json.dumps(s) for s in SURFACES) + "}\n"
              f"local PERIOD = {PERIOD}\nlocal WRAP_LIMIT = {WRAP_LIMIT}\n"
              f"local BLOCK = {BLOCK}\nlocal TERRAIN_MIN = {ORIGIN}\n"
              f"local TERRAIN_MAX = {ORIGIN + (GRID - 1) * SPACING}\n")
    (level / "mainLevel.lua").write_text(config + (HERE / "runtime.lua").read_text(
        encoding="utf-8"), encoding="utf-8")
    shutil.copyfile(HERE / "verify.lua", level / "verify.lua")
    digest = hashlib.sha256(terrain).hexdigest()
    write_json(mod_dir / "prototype.json", {"level": LEVEL, "block_m": BLOCK,
               "period_m": PERIOD, "wrap_limit_m": WRAP_LIMIT, "terrain_sha256": digest,
               "runtime_sha256": hashlib.sha256((level / "mainLevel.lua").read_bytes()).hexdigest()})
    archive = out.resolve() / (MOD + ".zip")
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
        for path in sorted(mod_dir.rglob("*")):
            if path.is_file():
                info = zipfile.ZipInfo(path.relative_to(mod_dir).as_posix(), (2026, 9, 5, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                z.writestr(info, path.read_bytes())
    print(f"Built {archive}\nTerrain SHA256: {digest}")
    return mod_dir, archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=ROOT / "build" / "checkerboard",
                        help="Output parent directory; generated mod gets its own subdirectory")
    parser.add_argument("--install", action="store_true", help="Also install to BeamNG's unpacked mods")
    args = parser.parse_args()
    mod_dir, _ = build(args.out)
    if args.install:
        destination = (Path(os.environ["LOCALAPPDATA"]) / "BeamNG" / "BeamNG.drive"
                       / "current" / "mods" / "unpacked" / MOD)
        shutil.copytree(mod_dir, destination, dirs_exist_ok=True)
        print(f"Installed {destination}")


if __name__ == "__main__":
    main()
