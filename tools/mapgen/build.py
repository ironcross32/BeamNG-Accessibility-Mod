"""Generate the Proving Grounds level and install it as an unpacked mod.

    python tools/mapgen/build.py            # install to the BeamNG mods folder
    python tools/mapgen/build.py --out DIR  # write somewhere else instead

Nothing here needs the game running. The .ter binary is written directly (see
terfile.py, which round-trips three shipped game terrains byte-for-byte), the
scene is newline-delimited JSON, and the textures are generated. So the whole
level is reproducible from source and the generator is the thing under version
control -- not a 12 MB binary.

Scene layout follows the shipped levels: a MissionGroup of SimGroups, one
items.level.json per group, one JSON object per line.
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

import mapdef            # noqa: E402
import terfile           # noqa: E402
import textures          # noqa: E402

LEVEL = "proving_grounds"
MOD = "beam_proving_grounds"
DEFAULT_MODS = os.path.join(
    os.environ.get("LOCALAPPDATA", ""),
    "BeamNG", "BeamNG.drive", "current", "mods", "unpacked")

# Stable ids across rebuilds. A fresh uuid4 every build would make the level
# look like a different object to anything that remembers persistentIds.
NS = uuid.UUID("6f2b1c4e-0000-4000-8000-000000000001")


def pid(name):
    return str(uuid.uuid5(NS, name))


# A SpawnSphere's own heading is NOT the heading the vehicle gets. Both
# core/levels.lua:400 and spawn.lua:974 place a vehicle with
#     options.rot = quat(spawnPoint:getRotation()) * quat(0, 0, 1, 0)
# and quat(0,0,1,0) is a 180 degree turn about Z. So every spawn sphere in the
# game is written facing BACKWARDS relative to the car that lands on it. Getting
# this wrong is invisible in the files and obvious only from the driving seat --
# and on this map it would matter most at the summit, whose whole purpose is the
# direction it points.
SPAWN_FLIP_180 = True

# Whether the nine numbers are read column-major (the axes are the columns) or
# row-major. The shipped matrices are a pure yaw, so they read identically under
# both conventions apart from the SIGN of the rotation -- one sample cannot tell
# them apart, and neither can reasoning. It is one constant, verified once from
# the seat, in the same spirit as TRAILER_ANGLE_SIGN elsewhere in this project.
# Verify with tools/mapgen/verify.lua after the first load.
SPAWN_MATRIX_COLUMN_MAJOR = True


def face(fx, fy):
    """3x3 rotation matrix, flattened, that makes a spawned vehicle face (fx, fy).

    BeamNG's vehicle forward is +Y, and quatFromEuler(0, 0, psi) maps +Y to
    (sin psi, cos psi) -- measured in-game, not assumed. So the axis that must
    end up as (fx, fy) is the matrix's second basis vector.
    """
    if SPAWN_FLIP_180:
        fx, fy = -fx, -fy
    n = math.hypot(fx, fy) or 1.0
    fx, fy = fx / n, fy / n
    if SPAWN_MATRIX_COLUMN_MAJOR:
        # columns are the axes: column 1 (X) = (fy, -fx), column 2 (Y) = (fx, fy)
        return [fy, fx, 0.0, -fx, fy, 0.0, 0.0, 0.0, 1.0]
    return [fy, -fx, 0.0, fx, fy, 0.0, 0.0, 0.0, 1.0]


def write_items(path, objects):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        for o in objects:
            f.write(json.dumps(o, separators=(",", ":")) + "\n")


def sample_height(z, wx, wy):
    """Terrain height in metres at a world position."""
    # Heights are node-aligned, unlike the material cells. Keeping this lookup
    # on the node convention matters for the navigation road: its graph must
    # follow the steepening terrain closely enough to pass the detector's
    # vertical compatibility check.
    col = int(round((wx - mapdef.ORIGIN) / mapdef.METRES_PER_CELL))
    row = int(round((wy - mapdef.ORIGIN) / mapdef.METRES_PER_CELL))
    col = max(0, min(mapdef.GRID - 1, col))
    row = max(0, min(mapdef.GRID - 1, row))
    return float(z[row, col])


# --------------------------------------------------------------------- scene

def sky_objects():
    return [
        {"name": "theLevelInfo", "class": "LevelInfo", "persistentId": pid("levelinfo"),
         "__parent": "Sky", "enabled": "1", "fogAtmosphereHeight": 240.0,
         "fogColor": [0.741, 0.816, 0.925, 1], "fogDensity": 6e-05,
         "globalEnviromentMap": "BNG_Sky_01_cubemap", "gravity": -9.81,
         "visibleDistance": 12500,
         # A flat curve: two points, because core_environment returns early on a
         # curve with fewer than two and the temperature would freeze.
         "temperatureCurveC": [[0, 18], [1, 18]]},
        {"name": "sunsky", "class": "ScatterSky", "persistentId": pid("sunsky"),
         "__parent": "Sky", "position": [0, 0, 100],
         "ambientScale": [1, 0.894, 0.78, 1],
         "ambientScaleGradientFile": "art/sky_gradients/default/gradient_ambient.png",
         "azimuth": 57.3, "elevation": 144,
         "colorize": [0.22, 0.35, 0.61, 1],
         "colorizeGradientFile": "art/sky_gradients/default/gradient_colorize.png",
         "flareScale": 5, "flareType": "BNG_Sunflare_3",
         "fogScale": [0.396, 0.667, 1, 1],
         "fogScaleGradientFile": "art/sky_gradients/default/gradient_fog.png",
         "mieScattering": 0.00043, "skyBrightness": 40,
         "sunScale": [0.996, 0.812, 0.706, 1],
         "sunScaleGradientFile": "art/sky_gradients/default/gradient_sunscale.png",
         "nightColor": [1, 0.894, 0.78, 1], "nightCubemap": "nightCubemap",
         "nightFogColor": [0.396, 0.667, 1, 1],
         "nightFogGradientFile": "art/sky_gradients/default/gradient_fog.png",
         "nightGradientFile": "art/sky_gradients/default/gradient_ambient.png",
         "useNightCubemap": True, "occlusionScale": 0.025,
         "shadowDarkenColor": [0, 0, 0, 0]},
        {"name": "tod", "class": "TimeOfDay", "persistentId": pid("tod"),
         "__parent": "Sky", "axisTilt": 10, "play": False,
         "startTime": 0.18, "time": 0.18},
        {"name": "CloudLayer_1", "class": "CloudLayer", "__parent": "Sky",
         "persistentId": pid("cloud"), "Textures": [{}, {}, {}], "coverage": 0.6,
         "texture": "art/skies/clouds/clouds_normal_displacement", "windSpeed": 0.4},
    ]


def terrain_object():
    return [{
        "name": "theTerrain", "class": "TerrainBlock", "persistentId": pid("terrain"),
        "__parent": "Terrain",
        "position": [mapdef.ORIGIN, mapdef.ORIGIN, 0],
        "squareSize": mapdef.METRES_PER_CELL,
        "maxHeight": mapdef.MAX_HEIGHT,
        "baseTexSize": 2048,
        "terrainFile": "/levels/%s/%s.ter" % (LEVEL, LEVEL),
        "minimapImage": "levels/%s/%s_minimap.png" % (LEVEL, LEVEL),
    }]


def road_objects(z):
    """Invisible DecalRoads that expose drivable features to AI.

    The asphalt, rumble strips, and gravel remain terrain materials. Drawing a
    second visible road over them would hide or replace those surface cues, so
    this uses BeamNG's stock `road_invisible` helper material. `map.lua` includes
    every DecalRoad with positive drivability in `map.getMap()`, which is the
    graph consumed by both vehicle AI and BeamTel's road detector.

    The climb is nonlinear in Z. Nodes every 50 m keep the graph interpolation
    close to the terrain; a single endpoint-to-endpoint edge would float more
    than 100 m above the middle and fail the detector's 6 m Z tolerance.
    """
    ys = [mapdef.CLIMB_BASE[1], mapdef.CLIMB_Y0]
    py = mapdef.CLIMB_Y0 + mapdef.CLIMB_ROAD_NODE_STEP
    while py < mapdef.CLIMB_Y1:
        ys.append(py)
        py += mapdef.CLIMB_ROAD_NODE_STEP
    ys.extend([mapdef.CLIMB_Y1, mapdef.CLIMB_SUMMIT[1]])

    nodes = [
        [mapdef.CLIMB_X, py, sample_height(z, mapdef.CLIMB_X, py),
         mapdef.CLIMB_ASPHALT_W]
        for py in ys
    ]
    climb = {
        "name": "road_hill_climb",
        "class": "DecalRoad",
        "persistentId": pid("road-hill-climb"),
        "__parent": "Roads",
        "position": nodes[0][:3],
        "drivability": 1.0,
        "improvedSpline": True,
        "material": "road_invisible",
        # Force map.lua to use these height-following control points directly.
        "useSubdivisions": False,
        "nodes": nodes,
    }

    # A separate graph component is intentional: the tunnel is a destination,
    # not part of the spine network, but AI and BeamTel must still recognise the
    # road while driving through it.  Extend the graph over both approaches so
    # detection is already live before the vehicle crosses a portal.
    tunnel_ys = [mapdef.TUNNEL_Y0 - mapdef.TUNNEL_APPROACH]
    tunnel_ys.extend(
        mapdef.TUNNEL_Y0 + step
        for step in range(0, int(mapdef.TUNNEL_LENGTH) + 1, 100)
    )
    tunnel_ys.append(mapdef.TUNNEL_Y1 + mapdef.TUNNEL_APPROACH)
    tunnel_nodes = [
        [mapdef.TUNNEL_X, py,
         sample_height(z, mapdef.TUNNEL_X, py) + 0.05,
         mapdef.TUNNEL_ROAD_HW * 2.0]
        for py in tunnel_ys
    ]
    tunnel = {
        "name": "road_one_kilometre_tunnel",
        "class": "DecalRoad",
        "persistentId": pid("road-one-kilometre-tunnel"),
        "__parent": "Roads",
        "position": tunnel_nodes[0][:3],
        "drivability": 1.0,
        "improvedSpline": True,
        "material": "road_invisible",
        "overObjects": True,
        "useSubdivisions": False,
        "nodes": tunnel_nodes,
    }
    return [climb, tunnel]


def tunnel_objects():
    """Collision shell, lighting, and reverb volume for the 1 km tunnel.

    The shell uses BeamNG's own 64 m Utah tunnel module.  Game archives are
    mounted globally (verified in a running proving_grounds session with
    FS:fileExists), so referencing this stock asset does not copy hundreds of
    kilobytes into the generated mod.  The source mesh runs from local Y -32 to
    +32 and carries its own Colmesh/collision detail.  Sixteen instances scaled
    to 62.5 m therefore meet without gaps and put their end faces exactly 1000 m
    apart.

    SFXSpace is the same mechanism shipped levels use for vehicle tunnel reverb.
    `Level_Tunnel_Closed_Dynamic` exists as a global SFXAmbience datablock and is
    the Utah road tunnel preset, rather than a guessed event name.
    """
    out = []
    for index in range(mapdef.TUNNEL_SEGMENTS):
        centre_y = (mapdef.TUNNEL_Y0 + mapdef.TUNNEL_SEGMENT_LENGTH *
                    (index + 0.5))
        out.append({
            "name": "tunnel_segment_%02d" % (index + 1),
            "class": "TSStatic",
            "persistentId": pid("tunnel-segment-%02d" % (index + 1)),
            "__parent": "Tunnel",
            "position": [mapdef.TUNNEL_X, centre_y,
                         mapdef.BASE_Z - mapdef.TUNNEL_MODULE_ROAD_Z],
            "scale": [1.0, mapdef.TUNNEL_SEGMENT_SCALE_Y, 1.0],
            "shapeName": "/levels/Utah/art/shapes/buildings/ut_tunnel_64m.dae",
            "collisionType": "Collision Mesh",
            "annotation": "ROAD",
            "useInstanceRenderData": True,
        })

    # Warm point lights make the kilometre readable visually without changing
    # the acoustic test.  One every 50 m overlaps comfortably at radius 38 m.
    for index, py in enumerate(np.linspace(
            mapdef.TUNNEL_Y0 + 25.0, mapdef.TUNNEL_Y1 - 25.0, 20)):
        out.append({
            "name": "tunnel_light_%02d" % (index + 1),
            "class": "PointLight",
            "persistentId": pid("tunnel-light-%02d" % (index + 1)),
            "__parent": "Tunnel",
            "position": [mapdef.TUNNEL_X, float(py), mapdef.BASE_Z + 6.2],
            "color": [1.0, 0.78, 0.52, 1.0],
            "brightness": 0.65,
            "radius": 38.0,
            "castShadows": False,
        })
    return out


def audio_objects():
    return [{
        "name": "audio_one_kilometre_tunnel",
        "class": "SFXSpace",
        "persistentId": pid("audio-one-kilometre-tunnel"),
        "__parent": "Audio",
        "position": [mapdef.TUNNEL_X,
                     (mapdef.TUNNEL_Y0 + mapdef.TUNNEL_Y1) / 2.0,
                     mapdef.BASE_Z + 4.0],
        # SFXSpace scale is its full box extent.  Keep it within the physical
        # portals so the ambience engages on entry and clears on exit.
        "scale": [9.0, mapdef.TUNNEL_LENGTH, 8.0],
        "soundAmbience": "Level_Tunnel_Closed_Dynamic",
    }]


def water_objects():
    """Two bounded pools.

    The WATER SURFACE IS `position.z`. The footprint is centred on
    `position.x/y`, and `scale` is the full extent of the collision volume, which
    straddles the surface -- so `getWorldBox` reports a box reaching `scale.z / 2`
    ABOVE the waterline, and that top face is not the water.

    This took three readings to pin down, and the middle one is the instructive
    failure. The first read it as corner-plus-extent, surface at
    `position.z + scale.z`, which put both pools two metres UNDERGROUND -- and
    `verify.lua` asserted exactly `p.z + s.z == 50.00` and passed, because a check
    derived from the code it checks cannot fail. The second asked the engine for
    `getObjBox()` (the unit cube) and `getWorldBox()` (the real AABB) and
    concluded the surface was the box top. That is a real measurement of a real
    quantity, and it is still the wrong quantity: `getWorldBox` is the COLLISION
    VOLUME, and nothing in it says which face the waterline is. It moved the
    pools most of the way up and looked convincing -- the Sump filled and the
    ford stayed dry -- which is why it survived a visual check.
    It was settled by asking the SIMULATION instead of the scene: park a car in a
    pool on a freshly loaded level and count `obj:inWater(cid)` per node.
    In the Sump, `position.z` was 46.75 and the highest wet node measured 46.749
    with the lowest dry one at 46.960. The waterline is `position.z`, to the
    millimetre, and the box top 3.25 m above it is not involved.
    Note the corollary for anyone tempted to test this at runtime: moving a live
    WaterBlock and calling `postApply()` updates the render and leaves the physics
    registration STALE -- afterwards it reports the highest nodes wet and the
    lowest dry, which is not a subtle wrongness but is easy to read as one. Every
    number above comes from a fresh level load.

    A WaterBlock with no textures renders NOTHING -- not an error, an invisible
    pool. The first version set only `baseColor`, on the reasonable-looking
    assumption that a surface material was optional for an untextured level, and
    the ford was a dry orange rectangle with a car sinking into it. There is no
    `surfMaterial` on any shipped water object in the game; the surface is built
    from three CORE textures instead (`rippleTex`, `foamTex`, `depthGradientTex`),
    which live in `core/art/water/` and are therefore mounted for every level
    rather than only while their own level is loaded -- the same self-containment
    argument the berms make against a `.cdae` guardrail.

    Valid textures alone are not enough to make shallow water legible. The
    original clarity/fresnel defaults left the 0.45 m ford looking exactly like
    its exposed dirt bed even though the underwater tint proved the volume was
    present. The values below were checked from an overhead camera in the running
    level. `BNG_Sky_01_cubemap` is a global datablock already used by LevelInfo;
    it supplies the fallback reflection while `fullReflect` stays off, so the
    result does not depend on the user's reflection quality setting.
    """
    out = []
    for name, (cx, cy), hw, hh, depth in (
            ("water_ford", mapdef.FORD, mapdef.FORD_HW, mapdef.FORD_HH, mapdef.FORD_DEPTH),
            ("water_sump", mapdef.SUMP, mapdef.SUMP_R, mapdef.SUMP_R, mapdef.SUMP_DEPTH)):
        # The volume straddles the waterline, so it has to be twice as deep as
        # the water to reach the cut bed. Generous rather than tight: a shipped
        # creek carries a scale.z of 100 for a stream a few metres deep.
        box_h = (depth + 2.0) * 2.0
        out.append({
            "name": name, "class": "WaterBlock", "persistentId": pid(name),
            "__parent": "Water",
            # position.z IS the waterline; x/y are the footprint centre
            "position": [cx, cy, mapdef.WATER_SURFACE_Z],
            "scale": [hw * 2.0, hh * 2.0, box_h],
            "GridElementSize": 5, "density": 1000, "viscosity": 1,
            "baseColor": [50, 120, 175, 255],
            "cubemap": "BNG_Sky_01_cubemap",
            "clarity": 0.2,
            # Shared-asset paths, mounted for every level unlike a level's own
            # art. Taken from italy rather than from east_coast_usa: that older
            # level still names `core/art/water/foam.dds`, which no longer
            # resolves (`FS:fileExists` says false, in a running game, for a path
            # a shipped level uses) -- so a stock level is not on its own proof
            # that a texture path is live.
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
                {"rippleDir": [0, 1], "rippleSpeed": 0.01,
                 "rippleMagnitude": 0.3, "rippleTexScale": [3, 3]},
                {"rippleDir": [1, -1], "rippleSpeed": 0.01,
                 "rippleMagnitude": 0.6, "rippleTexScale": [7, 7]},
                {"rippleDir": [-1, 1], "rippleSpeed": 0.003,
                 "rippleMagnitude": 0.5, "rippleTexScale": [20, 20]},
            ],
            "Waves (vertex undulation)": [{}, {}, {}],
            "Foam": [{}, {}],
        })
    return out


# objectname -> (label, position, facing). The label is used verbatim as the
# spawn point's translationId: an unmatched key falls through the translator
# unchanged, and a readable English string is a better failure mode than a raw
# dotted key being read out.
SPAWN_SPECS = [
    ("spawn_default", "Staging Yard", mapdef.STAGING, (0, 1)),
    ("spawn_mud_basin", "Mud Basin",
     (mapdef.MUD_BASIN[0] + 240, mapdef.MUD_BASIN[1]), (-1, 0)),
    ("spawn_ford", "Shallow Ford",
     (mapdef.FORD[0] - 220, mapdef.FORD[1]), (1, 0)),
    ("spawn_sump", "The Sump",
     (mapdef.SUMP[0], mapdef.SUMP[1] - 200), (0, 1)),
    ("spawn_climb_base", "Hill Climb - Base", mapdef.CLIMB_BASE, (0, 1)),
    ("spawn_climb_summit", "Hill Climb - Summit", mapdef.CLIMB_SUMMIT, (0, -1)),
    ("spawn_tunnel", "One-Kilometre Tunnel", mapdef.TUNNEL_SPAWN, (0, 1)),
    # In front of the GENTLEST lane, on the flat apron, facing north up it.
    ("spawn_susp", "Suspension Straights", mapdef.SUSP_START, (0, 1)),
    # South of the middle rig (the hamster wheel), facing north up the pad --
    # every rig on that pad is entered heading north, so one heading serves all
    # three and the choice of rig is a lane change rather than a manoeuvre.
    ("spawn_sound_stage", "Sound Stage",
     (mapdef.SOUND_STAGE[0], mapdef.STAGE_RIG_Y - mapdef.STAGE_APPROACH_M), (0, 1)),
]


def spawn_points(z):
    """SpawnSphere per section.

    The summit one faces SOUTH -- back down the climb -- so the hill is also a
    descent without having to drive up it first. That is the one spawn whose
    heading carries meaning rather than convention.

    Placing the objects is only half the job: the pickable list in the UI comes
    from info.json's spawnPoints, not from the scene. See spawn_point_info().
    """
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


def spawn_point_info():
    """The `spawnPoints` block for info.json -- the list the UI actually offers.

    Putting SpawnSphere objects in the scene is NOT enough. core/levels.lua:138
    builds the pickable list from info.json alone, and when the key is missing it
    falls through to line 161 and inserts ONE synthetic entry with no objectname,
    translated as `ui.common.default`. So a level with six spawn spheres and no
    spawnPoints block offers exactly one choice called "Default" -- which reads
    as the spheres having failed, when in fact nothing ever asked for them.

    `previews` is filled in by the same parser (it falls back to the level
    preview), so no per-spawn screenshots are needed. `defaultSpawnPointName` has
    to match one objectname here, or the synthetic entry is appended anyway.
    """
    return [{"objectname": name, "translationId": label}
            for name, label, _pos, _dir in SPAWN_SPECS]


# ----------------------------------------------------------------- materials

def terrain_materials():
    """V1-style TerrainMaterials: diffuseMap + groundmodelName, nothing else.

    The V2 fields (baseColor*Tex and friends) need a TerrainMaterialTextureSet
    object alongside them; V1 does not, and groundmodelName -- which is the only
    field that changes how the surface DRIVES -- is common to both.
    """
    doc = {}
    for internal, groundmodel in mapdef.MATERIALS:
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
            "diffuseSize": 250.0,
            "detailSize": 4.0,
            "detailStrength": 0.7,
            "detailDistance": 90.0,
            "useSideProjection": False,
        }
    return doc


# ---------------------------------------------------------------- POIs (Lua)

# Icon names are NOT free text -- they index the game's own icon font, and a name
# that is not in it renders as nothing at all rather than as an error. Every one
# below is a name the shipped GE extensions actually use; `offroad` and
# `checkpoint` were the obvious guesses and are both absent.
POI_SECTIONS = [
    ("staging", "Staging Yard", "flag",
     "Flat asphalt yard. Spawn point and the start of the run north.",
     mapdef.STAGING, (0, 1)),
    ("mud_basin", "Mud Basin", "terrain",
     "Five mud pits in a dirt apron, west of the spine road.",
     (mapdef.MUD_BASIN[0] + 240, mapdef.MUD_BASIN[1]), (-1, 0)),
    ("ford", "Shallow Ford", "water",
     "Water crossing about half a metre deep. Drivable in anything.",
     (mapdef.FORD[0] - 220, mapdef.FORD[1]), (1, 0)),
    ("sump", "The Sump", "warning",
     "Deep water. Driving in will flood and hydrolock a combustion engine.",
     (mapdef.SUMP[0], mapdef.SUMP[1] - 200), (0, 1)),
    ("climb_base", "Hill Climb - Base", "arrow_upward",
     "Foot of the straight climb. Starts at 4 percent and steepens to 60, "
     "with rumble strips along both asphalt edges.",
     mapdef.CLIMB_BASE, (0, 1)),
    ("climb_summit", "Hill Climb - Summit", "arrow_downward",
     "Top of the climb, facing back down it for the descent.",
     mapdef.CLIMB_SUMMIT, (0, -1)),
    ("tunnel", "One-Kilometre Tunnel", "arrow_upward",
     "South portal of a straight 1 kilometre enclosed road tunnel with dynamic vehicle reverb.",
     mapdef.TUNNEL_SPAWN, (0, 1)),
    ("susp", "Suspension Straights", "carToWheels",
     "Three parallel dirt lanes running north, undulating harder left to right, "
     "divided by rumble strips. You arrive in front of the gentlest one.",
     mapdef.SUSP_START, (0, 1)),
] + [
    # One marker per rig rather than one for the pad. Quick travel is the tool
    # you reach for mid-session and its whole value here is that it puts you
    # DOWN LINED UP: STAGE_APPROACH_M south of that rig's own centre, facing
    # north, which is the approach all three share.
    ("stage_" + key, "Sound Stage - " + label, icon, desc,
     (mapdef.SOUND_STAGE[0] + dx, mapdef.STAGE_RIG_Y - mapdef.STAGE_APPROACH_M),
     (0, 1))
    for key, label, _model, _config, dx, _face, _off, icon, desc
    in mapdef.SOUND_STAGE_RIGS
]


def main_level_lua(z):
    """Emit mainLevel.lua carrying the big-map POIs.

    freeroam.lua does extensions.loadAtRoot(<levelDir>/mainLevel, "") on mission
    start, so this file is registered as a real extension and receives engine
    hooks -- onGetRawPoiListForLevel among them. That keeps the POIs inside the
    level folder instead of needing a separate mod extension.

    Positions are generated from mapdef, so a section that moves in the layout
    cannot leave its marker behind.

    It also SPAWNS the sound stage rigs. They have to be spawned rather than
    placed: they are prop vehicles, and a level scene cannot hold one -- a class
    census of gridmap_v2's items.level.json is 7167 TSStatic, 12 SpawnSphere and
    zero BeamNGVehicle, and no shipped level puts a vehicle in its scene at all.
    This file is already a real extension receiving engine hooks, so it is the
    cheapest place to do it, with no new mod behind it.
    """
    rows = []
    for key, name, icon, desc, (px, py), (fx, fy) in POI_SECTIONS:
        rows.append(
            '  {id = "%s", name = "%s", icon = "%s", desc = "%s", '
            'pos = {%.2f, %.2f, %.2f}, dir = {%.4f, %.4f}},'
            % (key, name, icon, desc, px, py,
               sample_height(z, px, py) + 0.6, fx, fy))
    body = "\n".join(rows)

    rigs = []
    for key, label, model, config, dx, (fx, fy), off, _icon, _desc in (
            mapdef.SOUND_STAGE_RIGS):
        # The offset goes along the prop's own facing, so a rig that gets turned round keeps
        # its geometric centre where the layout says it is.
        px = mapdef.SOUND_STAGE[0] + dx + fx * off
        py = mapdef.STAGE_RIG_Y + fy * off
        rigs.append(
            '''  {id = "%s", name = "%s", model = "%s", config = "%s", pos = {%.2f, %.2f, %.2f}, dir = {%.4f, %.4f}},'''
            % (key, label, model, config, px, py,
               sample_height(z, px, py), fx, fy))
    rig_body = "\n".join(rigs)

    return '''-- Proving Grounds: big-map points of interest.
-- GENERATED by tools/mapgen/build.py -- edit the generator, not this file.
--
-- Registered as an extension by freeroam.lua's loadAtRoot on mission start,
-- which is what makes onGetRawPoiListForLevel reach us from inside a level
-- folder. quickTravelPosRotFunction is what lets a marker place the vehicle
-- with a HEADING: the summit marker faces back down the climb so the hill can
-- be driven as a descent without climbing it first.

local M = {}

local LEVEL = "%s"

local SECTIONS = {
%s
}

local function posRotFor(section)
  return function(poi, veh)
    local d = section.dir
    -- quatFromEuler(0, 0, psi) maps +Y to (sin psi, cos psi) -- measured in the
    -- running game, so psi = atan2(fx, fy) and the sign is NOT negated. There is
    -- also no 180 degree flip here: that one belongs to SpawnSphere placement
    -- (spawn.lua:974), and a quickTravel function returns its rotation directly.
    local yaw = math.atan2(d[1], d[2])
    return vec3(section.pos[1], section.pos[2], section.pos[3]),
           quatFromEuler(0, 0, yaw)
  end
end

local function onGetRawPoiListForLevel(levelIdentifier, elements)
  if levelIdentifier ~= LEVEL then return end
  for _, s in ipairs(SECTIONS) do
    table.insert(elements, {
      data = {type = "provingGroundsSection"},
      id = "provingGrounds##" .. s.id,
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

-- ---------------------------------------------------------------- sound stage
-- The three shipped test rigs on the pad east of the Staging Yard. They are
-- SPAWNED, not placed, because a level scene cannot hold a vehicle at all --
-- gridmap_v2's scene is 7167 TSStatic and zero BeamNGVehicle, and no shipped
-- level puts one in. `dir` is the PROP's own facing, which for two of these
-- three is not the direction you drive into it; mapdef.py carries the reason
-- per rig, read out of each jbeam rather than guessed.

local RIGS = {
%s
}

-- Long enough that the mission is genuinely settled, short enough that the rigs
-- are standing before anyone could drive the 400 m to them.
local RIG_SPAWN_DELAY_S = 2.0
-- A rig already this close to its mark IS that rig. This file is re-run by an
-- extension reload as well as by a mission start, and spawning a second hamster
-- wheel inside the first is not a state anything recovers from.
local RIG_MATCH_M = 8.0

local rigsPending = true
local rigTimer = 0

local function rigAlreadyThere(rig)
  for i = 0, be:getObjectCount() - 1 do
    local o = be:getObject(i)
    if o and o:getJBeamFilename() == rig.model then
      local p = vec3(o:getPosition())
      local dx, dy = p.x - rig.pos[1], p.y - rig.pos[2]
      if (dx * dx + dy * dy) < (RIG_MATCH_M * RIG_MATCH_M) then return true end
    end
  end
  return false
end

local function spawnRigs()
  for _, rig in ipairs(RIGS) do
    if not rigAlreadyThere(rig) then
      -- Same heading convention as the POIs above, and NO 180 degree flip: that
      -- one belongs to SpawnSphere placement (spawn.lua:974), while
      -- spawnNewVehicle uses the rotation it is handed directly.
      local yaw = math.atan2(rig.dir[1], rig.dir[2])
      local ok, err = pcall(core_vehicles.spawnNewVehicle, rig.model, {
        pos = vec3(rig.pos[1], rig.pos[2], rig.pos[3]),
        rot = quatFromEuler(0, 0, yaw),
        cling = true,
        autoEnterVehicle = false,
        config = rig.config,
      })
      if not ok then
        log("E", "provingGrounds.spawnRigs",
            tostring(rig.name) .. ": " .. tostring(err))
      end
    end
  end
end

local function onClientStartMission()
  rigsPending = true
  rigTimer = 0
end

local function onUpdate(dtReal)
  if not rigsPending then return end
  -- Wait for the PLAYER's vehicle to exist first. autoEnterVehicle = false ought
  -- to be enough on its own, but the cost of being wrong is a session that opens
  -- with the driver sitting inside a hamster wheel, and waiting removes the
  -- question rather than answering it.
  if be:getPlayerVehicleID(0) < 0 then return end
  rigTimer = rigTimer + dtReal
  if rigTimer < RIG_SPAWN_DELAY_S then return end
  -- Cleared BEFORE the attempt and never after: a throw that left it set would
  -- retry every frame for the rest of the session.
  rigsPending = false
  local ok, err = pcall(spawnRigs)
  if not ok then log("E", "provingGrounds.onUpdate", tostring(err)) end
end

M.onGetRawPoiListForLevel = onGetRawPoiListForLevel
M.onClientStartMission = onClientStartMission
M.onUpdate = onUpdate

return M
''' % (LEVEL, body, rig_body)


# -------------------------------------------------------------------- images

MAT_COLOURS = {
    mapdef.M_GRAVEL: (122, 116, 104),
    mapdef.M_ASPHALT: (58, 58, 62),
    mapdef.M_DIRT: (124, 96, 66),
    mapdef.M_MUD: (62, 47, 33),
    mapdef.M_ROCK: (104, 100, 96),
    mapdef.M_RUMBLE: (170, 96, 88),
}


def overview_image(z, mat, size=1024):
    """Top-down material map, shaded by height. Doubles as preview and minimap.

    Rows are flipped so north (+Y) is at the top, which is how the big map and
    anyone reading it over your shoulder will expect it.
    """
    step = max(1, mapdef.GRID // size)
    m = mat[::step, ::step]
    h = z[::step, ::step]
    img = np.zeros(m.shape + (3,), dtype=np.float64)
    for idx, rgb in MAT_COLOURS.items():
        img[m == idx] = rgb
    rel = (h - mapdef.BASE_Z)
    span = max(1.0, float(rel.max()) - float(rel.min()))
    shade = 0.72 + 0.55 * (rel - float(rel.min())) / span
    img *= shade[..., None]
    return np.clip(img, 0, 255).astype(np.uint8)[::-1]


# ---------------------------------------------------------------------- main

def build(out_root):
    level_dir = os.path.join(out_root, MOD, "levels", LEVEL)
    art_dir = os.path.join(level_dir, "art", "terrains")
    if os.path.isdir(os.path.join(out_root, MOD)):
        shutil.rmtree(os.path.join(out_root, MOD))
    os.makedirs(art_dir, exist_ok=True)

    print("generating terrain %d x %d ..." % (mapdef.GRID, mapdef.GRID))
    z, mat = mapdef.build()

    # ---- terrain binary
    t = terfile.Terrain(
        mapdef.GRID,
        mapdef.to_uint16(z).tobytes(),
        mat.tobytes(),
        [name for name, _ in mapdef.MATERIALS])
    n = terfile.write(os.path.join(level_dir, "%s.ter" % LEVEL), t)
    print("  %s.ter  %.1f MB" % (LEVEL, n / 1e6))

    # The fluid depth map has to sit beside the .ter under exactly this name, or
    # the engine disables EVERY fluid ground type on the terrain -- see
    # mapdef.fluid_depth_map for what that costs and how the scale was inferred.
    textures.write_gray_png(os.path.join(level_dir, "%s.ter.depth.png" % LEVEL),
                            mapdef.fluid_depth_map(mat))

    # ---- textures + images
    textures.generate_all(art_dir)
    ov = overview_image(z, mat)
    textures.write_png(os.path.join(level_dir, "%s_preview.png" % LEVEL), ov)
    textures.write_png(os.path.join(level_dir, "%s_minimap.png" % LEVEL), ov)

    # ---- scene
    mg = os.path.join(level_dir, "main", "MissionGroup")
    write_items(os.path.join(level_dir, "main", "items.level.json"), [
        {"name": "MissionGroup", "class": "SimGroup",
         "persistentId": pid("missiongroup"), "enabled": "1"}])
    # Every declared SimGroup must have an items file. A group named here with
    # no directory written for it makes the loader report
    # "Unable to deserialize file .../<group>/items.level.json" on every load --
    # harmless, but it is noise in the one log a level bug is diagnosed from.
    # CameraBookmarks was such a group: declared, empty, and used by nothing.
    groups = ["Sky", "Terrain", "Roads", "Tunnel", "Audio", "Water",
              "PlayerDropPoints"]
    write_items(os.path.join(mg, "items.level.json"), [
        {"name": g, "class": "SimGroup", "persistentId": pid("group-" + g),
         "__parent": "MissionGroup", "enabled": "1"} for g in groups])
    write_items(os.path.join(mg, "Sky", "items.level.json"), sky_objects())
    write_items(os.path.join(mg, "Terrain", "items.level.json"), terrain_object())
    write_items(os.path.join(mg, "Roads", "items.level.json"), road_objects(z))
    write_items(os.path.join(mg, "Tunnel", "items.level.json"), tunnel_objects())
    write_items(os.path.join(mg, "Audio", "items.level.json"), audio_objects())
    write_items(os.path.join(mg, "Water", "items.level.json"), water_objects())
    write_items(os.path.join(mg, "PlayerDropPoints", "items.level.json"),
                spawn_points(z))

    # ---- materials, info, lua
    with open(os.path.join(art_dir, "main.materials.json"), "w", encoding="utf-8") as f:
        json.dump(terrain_materials(), f, indent=2)

    with open(os.path.join(level_dir, "info.json"), "w", encoding="utf-8") as f:
        json.dump({
            "title": "Proving Grounds",
            "description": ("A flat 5 km plateau with a mud basin, a shallow ford, "
                            "a drowning pool, a three-kilometre hill climb that steepens "
                            "from 4 to 60 percent, three parallel undulating "
                            "dirt lanes for suspension work, a one-kilometre "
                            "road tunnel with dynamic reverb, and a sound stage "
                            "carrying a rolling road, a hamster wheel and a "
                            "tilting roller ramp."),
            "previews": ["%s_preview.png" % LEVEL],
            "size": [int(mapdef.WORLD_SPAN), int(mapdef.WORLD_SPAN)],
            "biome": "Flat",
            "roads": "Asphalt, gravel, dirt, mud, rumble strip",
            "authors": "Generated by tools/mapgen",
            "defaultSpawnPointName": "spawn_default",
            "spawnPoints": spawn_point_info(),
            "supportsTraffic": False,
        }, f, indent=2)

    with open(os.path.join(level_dir, "mainLevel.lua"), "w", encoding="utf-8") as f:
        f.write(main_level_lua(z))

    return level_dir, z, mat


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=DEFAULT_MODS,
                    help="mods/unpacked directory (default: the BeamNG one)")
    args = ap.parse_args()

    level_dir, z, mat = build(args.out)

    print("\nsuspension straights (measured off the sampled heightmap)")
    for name, swl, swa, ripl, ripa, want, got, grade in mapdef.describe_susp(z):
        print("  %-7s swell %4.1f m / %.2f m   ripple %4.1f m / %.2f m   "
              "peak-to-peak %.2f m (want %.2f)  steepest %4.1f %%"
              % (name, swl, swa, ripl, ripa, got or -1, want, (grade or 0) * 100))

    print("\nclimb profile")
    for s, pct, deg, zz in mapdef.describe_climb():
        print("  %5.0f m along: grade %5.1f%% (%4.1f deg)  height %6.1f m"
              % (s, pct, deg, zz))
    print("  summit curve: 60.0%% to 0.0%% over %.1f m; flat height %.1f m"
          % (mapdef.CLIMB_CREST_LENGTH, mapdef.CLIMB_TOP_Z))
    counts = np.bincount(mat.ravel(), minlength=len(mapdef.MATERIALS))
    total = float(mat.size)
    print("\nsurface mix")
    for i, (name, gm) in enumerate(mapdef.MATERIALS):
        print("  %-14s %-8s %5.1f %%" % (name, gm, 100.0 * counts[i] / total))
    print("\nheight range %.1f .. %.1f m" % (z.min(), z.max()))
    print("installed to %s" % level_dir)
    # build() rmtree's the mod folder first, so a rebuild against a RUNNING game deletes the
    # files the mod manager has already indexed: the level vanishes from core_levels.getList(),
    # disappears from the picker, and offers no spawn points at all, with nothing logged. It is
    # recoverable without a restart, but only if you know these two calls exist.
    print("")
    print("If BeamNG was running during this build, the level has dropped out of its list.")
    print("Paste this into the accessible console to put it back (no restart needed):")
    print('  core_modmanager.initDB(); core_levels.onFilesChanged({{filename = '
          '"/levels/%s/info.json", type = "modified"}})' % LEVEL)


if __name__ == "__main__":
    main()
