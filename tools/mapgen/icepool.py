"""Generate the Ice Pool level and install it as an unpacked mod.

    python tools/mapgen/icepool.py            # install to the BeamNG mods folder
    python tools/mapgen/icepool.py --out DIR  # write somewhere else instead

An elliptical pool, 327 m north-south by 250 m east-west across its flat floor.
The floor is ice. A 20 cm deep strip of water runs down the long axis and stops
20 m short of the wall at each end. A two-lane ice road circles the water, using
those two 20 m gaps as its turns. The wall starts as a gentle apron and curls up
into a face too steep to drive out of.

Same machinery as build.py (Proving Grounds): the .ter is written directly by
terfile.py, the scene is newline-delimited JSON, and mainLevel.lua is generated.
This file owns only the layout. See docs/level-generation.md for the conventions
it relies on (SpawnSphere flip, node vs cell grids, WaterBlock waterline).
"""

import argparse
import json
import math
import os
import shutil
import sys
import uuid

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import terfile           # noqa: E402
import textures          # noqa: E402
from build import face, write_items, DEFAULT_MODS   # noqa: E402

LEVEL = "ice_pool"
MOD = "beam_ice_pool"

# A namespace of its own, so no object here shares a persistentId with one in
# the Proving Grounds even where the object names coincide ("theTerrain").
NS = uuid.UUID("6f2b1c4e-0000-4000-8000-000000000002")


def pid(name):
    return str(uuid.uuid5(NS, name))


# ---------------------------------------------------------------- world scale
# 1 m cells rather than the Proving Grounds' 2.5 m. The map is small, and the
# steepest face a heightmap can express is rise / cell -- so the finer grid is
# what lets the top of the wall be near vertical instead of merely steep.
GRID = 512
METRES_PER_CELL = 1.0
WORLD_SPAN = GRID * METRES_PER_CELL          # 512 m
ORIGIN = -WORLD_SPAN / 2.0
MAX_HEIGHT = 200.0                           # ~3 mm per uint16 step
FLOOR_Z = 20.0

# ------------------------------------------------------------------ materials
# Index order IS the .ter material table. Every surface a car can reach is ICE;
# the road differs from the floor in colour only, so a sighted helper can see
# the circuit while it drives exactly like the rest of the pool.
MATERIALS = [
    ("ice_floor", "ICE"),
    ("ice_road", "ICE"),
    ("ice_wall", "ICE"),
    ("rim_rock", "ROCK"),
]
M_FLOOR, M_ROAD, M_WALL, M_RIM = range(4)

PALETTE = {
    "ice_floor": ((196, 220, 232), 10, 1),
    "ice_road": ((120, 150, 175), 8, 6),
    "ice_wall": ((170, 200, 218), 14, 1),
    "rim_rock": ((104, 100, 96), 26, 1),
}

# ----------------------------------------------------------------------- pool
# Floor ellipse semi-axes. Long axis north-south (+Y is north).
POOL_HALF_X = 125.0         # 250 m wide
POOL_HALF_Y = 163.5         # 327 m long

# The wall, as a function of distance d outward from the floor's edge. It is a
# skate-park transition: grade = G_MAX * (d / RUN)^P, so the first ten metres are
# an apron under 11 % that any car will roll up and back down, and it curls up
# to 72 degrees at d = RUN. A near-vertical face then takes it the rest of the way
# to WALL_H.
#   d =  5 m  grade  1.4 %   height  0.0 m
#   d = 10 m  grade 11 %     height  0.3 m
#   d = 15 m  grade 38 %     height  1.4 m
#   d = 20 m  grade 89 %     height  4.4 m
#   d = 25 m  grade 174 %    height 10.9 m
#   d = 30 m  grade 300 %    height 22.5 m, then the face
# WALL_H is tall on purpose. Ice limits acceleration, but 300 m of run is enough
# for a quick car to arrive at well over 100 km/h, and going straight up a
# quarter pipe at 150 km/h carries it roughly 88 m -- the face's 84 degrees then
# throws it back into the pool rather than onto the rim, provided the rim is not
# reachable first. 50 m keeps that true for anything short of a dedicated run.
WALL_RUN = 30.0
WALL_G_MAX = 3.0
WALL_P = 3.0
WALL_H = 50.0
FACE_GRADE = 10.0           # 84 degrees: 10 m of rise per 1 m cell
WALL_RUN_H = WALL_G_MAX * WALL_RUN / (WALL_P + 1.0)

# --------------------------------------------------------------------- water
# 20 cm deep, WaterBlock surface level with the floor, so the ice stops at the
# water's edge and the car drops 20 cm into it rather than meeting a kerb.
WATER_DEPTH = 0.20
WATER_END_GAP = 20.0        # water stops this far short of the wall, each end
WATER_HALF_Y = POOL_HALF_Y - WATER_END_GAP     # 143.5
# A 16 m strip. Width is bounded by the TURNS, not the straights: a WaterBlock is
# a box, so its square corners are what the road has to clear as it swings
# round the end of the strip inside the 20 m gap. At 30 m wide the road edge
# passed 1.7 m from those corners; at 16 m it is 4.4 m.
WATER_HALF_X = 8.0

# ---------------------------------------------------------------------- road
# The circuit's centreline is an ellipse concentric with the pool. North-south
# it sits in the middle of the 20 m gap between the water's end and the wall;
# east-west it runs midway between the water and the wall at the pool's widest,
# so the straights have ice to spare on both sides.
ROAD_HALF_W = 4.0           # two 4 m lanes
ROAD_RY = POOL_HALF_Y - WATER_END_GAP / 2.0    # 153.5
ROAD_RX = (WATER_HALF_X + POOL_HALF_X) / 2.0   # 70.0
ROAD_NODE_STEP = 8.0

# --------------------------------------------------------------------- spawn
# South-east quadrant, halfway between the water strip and the eastern wall,
# facing north. At this latitude that lands in the outer (northbound) lane of the
# circuit's east side, which curves about 15 degrees east of north here -- the
# spawn faces due north as specified, not along the road.
SPAWN_Y = -80.0


def floor_half_width(y):
    return POOL_HALF_X * math.sqrt(max(0.0, 1.0 - (y / POOL_HALF_Y) ** 2))


SPAWN_X = (WATER_HALF_X + floor_half_width(SPAWN_Y)) / 2.0

# ---------------------------------------------------------------- atmosphere
TIME_OF_DAY = (14 * 60) / 1440.0 - 0.5   # 14:00; core/solarTimeOfDay.lua's
                                         # timeFromMinutes, noon = 0
CLOUD_COVER = 2.4           # UI slider is 0..3; shipped clear-ish levels sit at 1.23
BREEZE_MPS = 4.5            # Beaufort 3, a gentle breeze
# Blowing TOWARD the north-east. The environment UI builds ground wind as
# (speed * sin(heading), speed * cos(heading)) with +X east and +Y north.
BREEZE_DIR = (math.sqrt(0.5), math.sqrt(0.5))
TEMPERATURE_C = -3.0


# ------------------------------------------------------------------ geometry

def ellipse_signed_distance(px, py, a, b, iters=4):
    """Signed distance to the ellipse (x/a)^2 + (y/b)^2 = 1; negative inside.

    Exact rather than the usual f / |grad f| approximation, because the wall
    profile is a function of this distance and the approximation stretches the
    apron at the ends of a 1.3:1 ellipse. The iteration walks the parametric
    angle toward the foot point; four rounds are far below a millimetre here.
    """
    px = np.abs(np.asarray(px, dtype=np.float64))
    py = np.abs(np.asarray(py, dtype=np.float64))
    tx = np.full(np.broadcast(px, py).shape, math.sqrt(0.5))
    ty = tx.copy()
    for _ in range(iters):
        x, y = a * tx, b * ty
        ex = (a * a - b * b) * tx ** 3 / a
        ey = (b * b - a * a) * ty ** 3 / b
        r = np.hypot(x - ex, y - ey)
        qx, qy = px - ex, py - ey
        q = np.maximum(np.hypot(qx, qy), 1e-12)
        tx = np.clip((qx * r / q + ex) / a, 0.0, 1.0)
        ty = np.clip((qy * r / q + ey) / b, 0.0, 1.0)
        t = np.maximum(np.hypot(tx, ty), 1e-12)
        tx, ty = tx / t, ty / t
    d = np.hypot(px - a * tx, py - b * ty)
    inside = (px / a) ** 2 + (py / b) ** 2 < 1.0
    return np.where(inside, -d, d)


def wall_height(d):
    """Height above the floor at distance d outside the floor ellipse."""
    d = np.maximum(np.asarray(d, dtype=np.float64), 0.0)
    run = np.minimum(d, WALL_RUN)
    h = WALL_RUN_H * (run / WALL_RUN) ** (WALL_P + 1.0)
    face_h = np.minimum(FACE_GRADE * np.maximum(d - WALL_RUN, 0.0),
                        WALL_H - WALL_RUN_H)
    return h + face_h


def road_centreline():
    """Closed centreline, resampled to equal arc length, counter-clockwise from
    due east -- so it runs north up the east side, as the spawn does."""
    th = np.linspace(0.0, 2.0 * math.pi, 20001)
    x, y = ROAD_RX * np.cos(th), ROAD_RY * np.sin(th)
    s = np.concatenate([[0.0], np.cumsum(np.hypot(np.diff(x), np.diff(y)))])
    n = int(round(s[-1] / ROAD_NODE_STEP))
    targets = np.arange(n) * (s[-1] / n)
    return np.interp(targets, s, x), np.interp(targets, s, y), float(s[-1])


def build_terrain():
    """(height float32 [GRID, GRID], material uint8 [GRID, GRID], stats)."""
    axis = ORIGIN + np.arange(GRID) * METRES_PER_CELL
    # Heights on the NODE grid ...
    ny, nx = axis[:, None], axis[None, :]
    # ... materials on the CELL grid, evaluated at each square's centre, which is
    # the convention the Proving Grounds had to measure its way to.
    centres = axis + METRES_PER_CELL / 2.0
    cy, cx = centres[:, None], centres[None, :]

    d_node = ellipse_signed_distance(nx, ny, POOL_HALF_X, POOL_HALF_Y)
    z = FLOOR_Z + wall_height(d_node)
    # The bed is cut only at nodes strictly inside the strip, so the 1 m slope
    # down into it lies under the WaterBlock rather than as a dry dip beside it.
    bed = ((np.abs(nx) <= WATER_HALF_X - METRES_PER_CELL)
           & (np.abs(ny) <= WATER_HALF_Y - METRES_PER_CELL))
    z = np.where(bed, FLOOR_Z - WATER_DEPTH, z)

    d_cell = ellipse_signed_distance(cx, cy, POOL_HALF_X, POOL_HALF_Y)
    h_cell = wall_height(d_cell)
    mat = np.full((GRID, GRID), M_FLOOR, dtype=np.uint8)
    mat[d_cell > 0.0] = M_WALL
    mat[h_cell >= WALL_H - 1e-6] = M_RIM
    road_d = np.abs(ellipse_signed_distance(cx, cy, ROAD_RX, ROAD_RY))
    road = road_d <= ROAD_HALF_W
    mat[road] = M_ROAD

    # The layout promises two things that are easy to break by nudging a
    # constant, so they are measured on every build rather than trusted.
    water_gap_x = np.maximum(np.abs(cx) - WATER_HALF_X, 0.0)
    water_gap_y = np.maximum(np.abs(cy) - WATER_HALF_Y, 0.0)
    to_water = np.hypot(water_gap_x, water_gap_y)
    stats = {
        "road_to_water_m": float(to_water[road].min()),
        "road_to_wall_m": float((-d_cell[road]).min()),
    }
    if stats["road_to_water_m"] <= 0.0:
        raise SystemExit("road touches the water")
    if stats["road_to_wall_m"] <= 0.0:
        raise SystemExit("road runs onto the wall")
    return z.astype(np.float32), mat, stats


def to_uint16(z):
    raw = np.clip(np.asarray(z, dtype=np.float64) / MAX_HEIGHT, 0.0, 1.0) * 65535.0
    return np.round(raw).astype("<u2")


def sample_height(z, wx, wy):
    col = int(round((wx - ORIGIN) / METRES_PER_CELL))
    row = int(round((wy - ORIGIN) / METRES_PER_CELL))
    return float(z[min(max(row, 0), GRID - 1), min(max(col, 0), GRID - 1)])


# --------------------------------------------------------------------- scene

def sky_objects():
    fx, fy = BREEZE_DIR
    return [
        {"name": "theLevelInfo", "class": "LevelInfo", "persistentId": pid("levelinfo"),
         "__parent": "Sky", "enabled": "1", "fogAtmosphereHeight": 300.0,
         "fogColor": [0.72, 0.75, 0.78, 1], "fogDensity": 0.00018,
         "globalEnviromentMap": "BNG_Sky_01_cubemap", "gravity": -9.81,
         "visibleDistance": 4000,
         # Two points: core_environment returns early on fewer and freezes it.
         "temperatureCurveC": [[0, TEMPERATURE_C], [1, TEMPERATURE_C]]},
        {"name": "sunsky", "class": "ScatterSky", "persistentId": pid("sunsky"),
         "__parent": "Sky", "position": [0, 0, 100],
         "ambientScale": [1, 0.96, 0.92, 1],
         "ambientScaleGradientFile": "art/sky_gradients/default/gradient_ambient.png",
         "azimuth": 57.3, "elevation": 144,
         "colorize": [0.22, 0.35, 0.61, 1],
         "colorizeGradientFile": "art/sky_gradients/default/gradient_colorize.png",
         "flareScale": 1, "flareType": "BNG_Sunflare_3",
         "fogScale": [0.6, 0.66, 0.72, 1],
         "fogScaleGradientFile": "art/sky_gradients/default/gradient_fog.png",
         "mieScattering": 0.0012, "skyBrightness": 28,
         "sunScale": [0.8, 0.78, 0.76, 1],
         "sunScaleGradientFile": "art/sky_gradients/default/gradient_sunscale.png",
         "nightColor": [1, 0.894, 0.78, 1], "nightCubemap": "nightCubemap",
         "nightFogColor": [0.396, 0.667, 1, 1],
         "nightFogGradientFile": "art/sky_gradients/default/gradient_fog.png",
         "nightGradientFile": "art/sky_gradients/default/gradient_ambient.png",
         "useNightCubemap": True, "occlusionScale": 0.025,
         "shadowDarkenColor": [0, 0, 0, 0]},
        {"name": "tod", "class": "TimeOfDay", "persistentId": pid("tod"),
         "__parent": "Sky", "axisTilt": 10, "play": False,
         "startTime": TIME_OF_DAY, "time": TIME_OF_DAY},
        {"name": "CloudLayer_1", "class": "CloudLayer", "__parent": "Sky",
         "persistentId": pid("cloud"), "Textures": [{}, {}, {}],
         "coverage": CLOUD_COVER, "exposure": 1.2, "height": 3,
         "baseColor": [0.86, 0.87, 0.89, 1.0],
         "texture": "art/skies/clouds/clouds_normal_displacement",
         # Drift with the breeze. Same (x, y) convention as the ground wind.
         "windDirection": [fx, fy], "windSpeed": 0.3},
    ]


def terrain_object():
    return [{
        "name": "theTerrain", "class": "TerrainBlock", "persistentId": pid("terrain"),
        "__parent": "Terrain",
        "position": [ORIGIN, ORIGIN, 0],
        "squareSize": METRES_PER_CELL,
        "maxHeight": MAX_HEIGHT,
        "baseTexSize": 1024,
        "terrainFile": "/levels/%s/%s.ter" % (LEVEL, LEVEL),
        "minimapImage": "levels/%s/%s_minimap.png" % (LEVEL, LEVEL),
    }]


def road_objects():
    """The circuit as an invisible, looped DecalRoad for AI and road detection.

    The ice underneath is the surface: `road_invisible` carries no groundType, so
    it changes neither the look nor the grip. `looped` closes the graph in
    map.lua (it adds the last-to-first edge itself), and lanes are declared
    explicitly -- one each way -- rather than inferred from the width.
    """
    xs, ys, _length = road_centreline()
    nodes = [[float(x), float(y), FLOOR_Z, ROAD_HALF_W * 2.0]
             for x, y in zip(xs, ys)]
    return [{
        "name": "road_ice_circuit",
        "class": "DecalRoad",
        "persistentId": pid("road-ice-circuit"),
        "__parent": "Roads",
        "position": nodes[0][:3],
        "drivability": 1.0,
        "improvedSpline": True,
        "material": "road_invisible",
        "useSubdivisions": False,
        "looped": True,
        "autoLanes": False,
        "lanesLeft": 1,
        "lanesRight": 1,
        "nodes": nodes,
    }]


def water_objects():
    """The strip. position.z IS the waterline (see build.water_objects for how
    that was measured); x/y are the footprint centre and scale the full extent.
    Styling is the Proving Grounds' ford, which was checked for visibility."""
    depth = WATER_DEPTH
    return [{
        "name": "water_strip", "class": "WaterBlock", "persistentId": pid("water_strip"),
        "__parent": "Water",
        "position": [0.0, 0.0, FLOOR_Z],
        "scale": [WATER_HALF_X * 2.0, WATER_HALF_Y * 2.0, (depth + 2.0) * 2.0],
        "GridElementSize": 5, "density": 1000, "viscosity": 1,
        "baseColor": [50, 120, 175, 255],
        "cubemap": "BNG_Sky_01_cubemap",
        "clarity": 0.2,
        "rippleTex": "/assets/materials/tileable/water/water_effects/ripple3_nm.normal.dds",
        "foamTex": "/assets/materials/tileable/water/water_effects/foam2_b.color.dds",
        "depthGradientTex": "/assets/materials/tileable/water/depthcolor_ramp/depthcolor_ramp_italy_rivers_b.png",
        "overallRippleMagnitude": 0.6,
        "overallWaveMagnitude": 0.0,
        "fresnelBias": 0.45, "fresnelPower": 4.0,
        "reflectivity": 0.65,
        "specularPower": 900, "fullReflect": False,
        "waterFogDensity": 0.5, "waterFogDensityOffset": 0.3,
        "foamMaxDepth": 0.5, "foamAmbientLerp": 0.8,
        "wetDarkening": 0.5, "wetDepth": 0.5,
        "Ripples (texture animation)": [
            {"rippleDir": [1, 1], "rippleSpeed": 0.01,
             "rippleMagnitude": 0.3, "rippleTexScale": [3, 3]},
            {"rippleDir": [1, -1], "rippleSpeed": 0.01,
             "rippleMagnitude": 0.6, "rippleTexScale": [7, 7]},
            {"rippleDir": [-1, 1], "rippleSpeed": 0.003,
             "rippleMagnitude": 0.5, "rippleTexScale": [20, 20]},
        ],
        "Waves (vertex undulation)": [{}, {}, {}],
        "Foam": [{}, {}],
    }]


SPAWN_SPECS = [
    ("spawn_default", "South-East Ice", (SPAWN_X, SPAWN_Y), (0, 1)),
]


def spawn_points(z):
    out = []
    for name, _label, (px, py), (fx, fy) in SPAWN_SPECS:
        out.append({
            "name": name, "class": "SpawnSphere", "persistentId": pid(name),
            "__parent": "PlayerDropPoints", "dataBlock": "SpawnSphereMarker",
            "position": [px, py, sample_height(z, px, py) + 0.6],
            "rotationMatrix": face(fx, fy),
            "autoplaceOnSpawn": "0", "enabled": "1", "radius": 6,
            "homingCount": "0", "lockCount": "0",
            "indoorWeight": "1", "outdoorWeight": "1", "sphereWeight": "1",
        })
    return out


def terrain_materials():
    doc = {}
    for internal, groundmodel in MATERIALS:
        obj = "%s-%s" % (internal, pid(internal))
        doc[obj] = {
            "name": obj,
            "internalName": internal,
            "class": "TerrainMaterial",
            "persistentId": pid("mat-" + internal),
            "groundmodelName": groundmodel,
            "annotation": "NATURE",
            "diffuseMap": "/levels/%s/art/terrains/t_%s.png" % (LEVEL, internal),
            "detailMap": "/levels/%s/art/terrains/t_%s.png" % (LEVEL, internal),
            "diffuseSize": 100.0,
            "detailSize": 4.0,
            "detailStrength": 0.5,
            "detailDistance": 90.0,
            "useSideProjection": False,
        }
    return doc


def write_textures(out_dir, size=512):
    os.makedirs(out_dir, exist_ok=True)
    for i, (name, (rgb, spread, streak)) in enumerate(sorted(PALETTE.items())):
        textures.write_png(os.path.join(out_dir, "t_%s.png" % name),
                           textures.noisy(rgb, spread, size, seed=100 + i,
                                          streak=streak))


# ---------------------------------------------------------------- mainLevel.lua

def main_level_lua(z):
    """POIs plus the breeze.

    Ground wind is not a scene field: core_environment holds it in a module
    local, zeroes it on onClientEndMission and re-applies it to every vehicle on
    spawn and reset. So it is set here, once the player's vehicle exists --
    waiting sidesteps the question of whether this extension's mission-start hook
    runs before or after core_environment's own, which re-applies whatever it is
    holding at that moment.
    """
    xs, ys, _ = road_centreline()
    north = (0.0, ROAD_RY)
    south = (0.0, -ROAD_RY)
    pois = [
        ("start", "South-East Ice", "flag",
         "The start: halfway between the water and the east wall, facing north "
         "in the circuit's northbound lane.",
         (SPAWN_X, SPAWN_Y), (0.0, 1.0)),
        ("north_turn", "North Turn", "arrow_upward",
         "Middle of the northern turn, in the 20 metre gap between the water and "
         "the wall, facing west (anticlockwise).", north, (-1.0, 0.0)),
        ("south_turn", "South Turn", "arrow_downward",
         "Middle of the southern turn, facing east (anticlockwise).",
         south, (1.0, 0.0)),
    ]
    rows = []
    for key, name, icon, desc, (px, py), (fx, fy) in pois:
        rows.append(
            '  {id = "%s", name = "%s", icon = "%s", desc = "%s", '
            'pos = {%.2f, %.2f, %.2f}, dir = {%.4f, %.4f}},'
            % (key, name, icon, desc, px, py,
               sample_height(z, px, py) + 0.6, fx, fy))
    wx = BREEZE_MPS * BREEZE_DIR[0]
    wy = BREEZE_MPS * BREEZE_DIR[1]
    return '''-- Ice Pool: big-map points of interest and the breeze.
-- GENERATED by tools/mapgen/icepool.py -- edit the generator, not this file.

local M = {}

local LEVEL = "%s"

local SECTIONS = {
%s
}

-- Toward the north-east, m/s, +X east +Y north (the environment UI's own
-- convention: x = speed * sin(heading), y = speed * cos(heading)).
local WIND = {%.4f, %.4f, 0}
local WIND_DELAY_S = 1.0

local windPending = true
local windTimer = 0

local function posRotFor(section)
  return function(poi, veh)
    local d = section.dir
    -- quatFromEuler(0, 0, psi) maps +Y to (sin psi, cos psi); no 180 flip here,
    -- that belongs to SpawnSphere placement only.
    return vec3(section.pos[1], section.pos[2], section.pos[3]),
           quatFromEuler(0, 0, math.atan2(d[1], d[2]))
  end
end

local function onGetRawPoiListForLevel(levelIdentifier, elements)
  if levelIdentifier ~= LEVEL then return end
  for _, s in ipairs(SECTIONS) do
    table.insert(elements, {
      data = {type = "icePoolSection"},
      id = "icePool##" .. s.id,
      markerInfo = {
        bigmapMarker = {
          pos = vec3(s.pos[1], s.pos[2], s.pos[3]),
          icon = s.icon,
          name = s.name,
          description = s.desc,
          quickTravelPosRotFunction = posRotFor(s),
        },
      },
    })
  end
end

local function onClientStartMission()
  windPending = true
  windTimer = 0
end

local function onUpdate(dtReal)
  if not windPending then return end
  if be:getPlayerVehicleID(0) < 0 then return end
  windTimer = windTimer + dtReal
  if windTimer < WIND_DELAY_S then return end
  -- Cleared before the attempt so a throw cannot retry every frame.
  windPending = false
  local ok, err = pcall(core_environment.setGroundWind, WIND[1], WIND[2], WIND[3])
  if not ok then log("E", "icePool.wind", tostring(err)) end
end

M.onGetRawPoiListForLevel = onGetRawPoiListForLevel
M.onClientStartMission = onClientStartMission
M.onUpdate = onUpdate

return M
''' % (LEVEL, "\n".join(rows), wx, wy)


# -------------------------------------------------------------------- images

MAT_COLOURS = {
    M_FLOOR: PALETTE["ice_floor"][0],
    M_ROAD: PALETTE["ice_road"][0],
    M_WALL: PALETTE["ice_wall"][0],
    M_RIM: PALETTE["rim_rock"][0],
}


def overview_image(z, mat):
    """Top-down map, north at the top, water drawn blue, shaded by height."""
    img = np.zeros(mat.shape + (3,), dtype=np.float64)
    for idx, rgb in MAT_COLOURS.items():
        img[mat == idx] = rgb
    axis = ORIGIN + (np.arange(GRID) + 0.5) * METRES_PER_CELL
    wet = ((np.abs(axis[None, :]) < WATER_HALF_X)
           & (np.abs(axis[:, None]) < WATER_HALF_Y))
    img[wet] = (50, 120, 175)
    rel = z - FLOOR_Z
    shade = 0.8 + 0.35 * np.clip(rel / WALL_H, 0.0, 1.0)
    img *= shade[..., None]
    return np.clip(img, 0, 255).astype(np.uint8)[::-1]


# ---------------------------------------------------------------------- main

def build(out_root):
    level_dir = os.path.join(out_root, MOD, "levels", LEVEL)
    art_dir = os.path.join(level_dir, "art", "terrains")
    if os.path.isdir(os.path.join(out_root, MOD)):
        shutil.rmtree(os.path.join(out_root, MOD))
    os.makedirs(art_dir, exist_ok=True)

    z, mat, stats = build_terrain()

    t = terfile.Terrain(GRID, to_uint16(z).tobytes(), mat.tobytes(),
                        [name for name, _ in MATERIALS])
    n = terfile.write(os.path.join(level_dir, "%s.ter" % LEVEL), t)
    print("  %s.ter  %.1f MB" % (LEVEL, n / 1e6))

    # No fluid surfaces here, but the engine logs a warning and disables fluid
    # ground types without this file, so it is written all-solid (255).
    textures.write_gray_png(os.path.join(level_dir, "%s.ter.depth.png" % LEVEL),
                            np.full(mat.shape, 255, dtype=np.uint8))

    write_textures(art_dir)
    ov = overview_image(z, mat)
    textures.write_png(os.path.join(level_dir, "%s_preview.png" % LEVEL), ov)
    textures.write_png(os.path.join(level_dir, "%s_minimap.png" % LEVEL), ov)

    mg = os.path.join(level_dir, "main", "MissionGroup")
    write_items(os.path.join(level_dir, "main", "items.level.json"), [
        {"name": "MissionGroup", "class": "SimGroup",
         "persistentId": pid("missiongroup"), "enabled": "1"}])
    groups = ["Sky", "Terrain", "Roads", "Water", "PlayerDropPoints"]
    write_items(os.path.join(mg, "items.level.json"), [
        {"name": g, "class": "SimGroup", "persistentId": pid("group-" + g),
         "__parent": "MissionGroup", "enabled": "1"} for g in groups])
    write_items(os.path.join(mg, "Sky", "items.level.json"), sky_objects())
    write_items(os.path.join(mg, "Terrain", "items.level.json"), terrain_object())
    write_items(os.path.join(mg, "Roads", "items.level.json"), road_objects())
    write_items(os.path.join(mg, "Water", "items.level.json"), water_objects())
    write_items(os.path.join(mg, "PlayerDropPoints", "items.level.json"),
                spawn_points(z))

    with open(os.path.join(art_dir, "main.materials.json"), "w", encoding="utf-8") as f:
        json.dump(terrain_materials(), f, indent=2)

    with open(os.path.join(level_dir, "info.json"), "w", encoding="utf-8") as f:
        json.dump({
            "title": "Ice Pool",
            "description": ("An elliptical pool of ice, 327 by 250 metres, walled "
                            "by a slope that starts gentle and curls up too steep "
                            "to escape. A 20 centimetre strip of water runs down "
                            "its length, circled by a two-lane ice road."),
            "previews": ["%s_preview.png" % LEVEL],
            "size": [int(WORLD_SPAN), int(WORLD_SPAN)],
            "biome": "Ice",
            "roads": "Ice",
            "authors": "Generated by tools/mapgen",
            "defaultSpawnPointName": "spawn_default",
            "spawnPoints": [{"objectname": name, "translationId": label}
                            for name, label, _pos, _dir in SPAWN_SPECS],
            "supportsTraffic": False,
        }, f, indent=2)

    with open(os.path.join(level_dir, "mainLevel.lua"), "w", encoding="utf-8") as f:
        f.write(main_level_lua(z))

    return level_dir, z, mat, stats


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=DEFAULT_MODS,
                    help="mods/unpacked directory (default: the BeamNG one)")
    args = ap.parse_args()

    level_dir, z, mat, stats = build(args.out)
    _, _, length = road_centreline()

    print("\nwall profile (distance outside the floor edge)")
    for d in (0, 5, 10, 15, 20, 25, 29, WALL_RUN + 1, WALL_RUN + 2):
        h = float(wall_height(d))
        g = float(wall_height(d + 0.5) - wall_height(d - 0.5)) if d > 0 else 0.0
        print("  %5.1f m  height %5.1f m  grade %6.1f %% (%4.1f deg)"
              % (d, h, g * 100.0, math.degrees(math.atan(g))))
    print("\ncircuit  %.0f m long, centreline %.1f x %.1f m semi-axes, "
          "tightest radius %.1f m" % (length, ROAD_RX, ROAD_RY, ROAD_RX ** 2 / ROAD_RY))
    print("  clearance road->water %.1f m, road->wall toe %.1f m"
          % (stats["road_to_water_m"], stats["road_to_wall_m"]))
    print("water    %.0f x %.0f m, %.0f cm deep, ends %.0f m short of the wall"
          % (WATER_HALF_X * 2, WATER_HALF_Y * 2, WATER_DEPTH * 100, WATER_END_GAP))
    print("spawn    (%.1f, %.1f) facing north" % (SPAWN_X, SPAWN_Y))
    counts = np.bincount(mat.ravel(), minlength=len(MATERIALS))
    print("\nsurface mix")
    for i, (name, gm) in enumerate(MATERIALS):
        print("  %-10s %-5s %5.1f %%" % (name, gm, 100.0 * counts[i] / mat.size))
    print("\nheight range %.2f .. %.2f m" % (z.min(), z.max()))
    print("installed to %s" % level_dir)
    print("")
    print("If BeamNG was running during this build, paste this into the console:")
    print('  core_modmanager.initDB(); core_levels.onFilesChanged({{filename = '
          '"/levels/%s/info.json", type = "modified"}})' % LEVEL)


if __name__ == "__main__":
    main()
