# Generated Levels (`tools/mapgen/`)

A level is not a track. BeamNG's Track Builder (`util_trackBuilder_splineTrack`)
is a spline ribbon generator -- forward/curve/spiral/loop/bezier pieces with
width, height and bank, closed into a circuit by `addClosingPiece()` -- and it
emits `ProceduralMesh` objects laid over whatever world is already loaded. It
owns no terrain, no water and no ground materials, so nothing that is a property
of the *ground* can be built with it. `tools/mapgen/` builds the ground.

The whole point of the directory is that **the World Editor is ImGui and has no
accessibility tree**, exactly like the Track Builder window. The editor is only a
front end; the data underneath is a binary heightmap and newline-delimited JSON,
both of which can be written from Python with the game shut down.

- `terfile.py` -- the `.ter` binary reader/writer
- `textures.py` -- procedural terrain textures (stdlib zlib PNG writer)
- `mapdef.py` -- the Proving Grounds layout and terrain synthesis; see [level-proving-grounds.md](level-proving-grounds.md)
- `build.py` -- assembles the level and installs it to `mods/unpacked/`
- `verify.lua` -- post-load checks for everything the generator had to assume
- `icepool.py` -- a second, self-contained level (Ice Pool); see [level-ice-pool.md](level-ice-pool.md)

This file holds the engine and file-format facts every generated level shares.
The layout of each level, and the reasoning behind it, is in the two files above.

```
python tools/mapgen/build.py              # install to the BeamNG mods folder
python tools/mapgen/terfile.py <file.ter> # round-trip a game terrain
```

## The `.ter` format

Recovered by decoding `levels/smallgrid/smallgrid.ter`, and **verified by
re-emitting three shipped game terrains byte-for-byte** (`smallgrid` 256,
`small_island` 1024, `italy` 4096). That round trip is the whole basis for
trusting the writer, so `terfile.py` keeps it as a runnable check rather than a
comment -- a format recovered by inspection needs a test that fails when the
inspection was wrong.

```
u8               version        7 and 9 both ship; layout is identical
u32              size           heightmap edge, power of two, <= 8192
u16[size*size]   height         metres = raw / 65535 * TerrainBlock.maxHeight
u8 [size*size]   materialIdx    index into the list below
u32              materialCount
  u8 nameLen + name             the TerrainMaterial's internalName
```

The generated terrain uses a deliberately generous `maxHeight` of 2100 m,
which gives approximately 32 mm of vertical resolution per stored step. This
field is the heightmap's encoding scale, not a world or aircraft flight ceiling;
vehicles and scene objects can travel above it.

**The names are `internalName`, not object names.** Confirmed against
`small_island`, whose `.ter` list matches its `art/terrains` material file
entry for entry. Get it wrong and the terrain loads with no surface at all --
not an error, just nothing.

`version` is preserved on read/write rather than normalised. 7 and 9 differ in
nothing this code touches, but rewriting a 9 as a 7 is a change nobody asked for.

## The heightmap is a NODE grid, not a raster of cells

Sample `i` sits at `TerrainBlock.position + i * squareSize`. `mapdef.grid_axes`
originally added the half-cell offset a raster image would want, which shifted
the whole world 1.25 m against its own layout constants -- the ford's hard edge,
specified at x = 1450, resolved in game at 1447.5.

That is cosmetic on its own. What made it expensive is that a threshold falling
mid-cell gets resolved by a node that is **neither side of it**, which is how the
berm acquired a ramp (below). So every lateral threshold that is meant to be a
step -- `CLIMB_HALF_W`, `BERM_OUTER` -- is an exact multiple of
`METRES_PER_CELL`, and the road edge and the wall foot are then the same node
with nothing in between.

## What makes a surface behave

`TerrainMaterial.groundmodelName` -- and nothing else -- decides how a cell
drives. `MUD` is a stock ground model (`art/groundmodels.json` inside
`gameengine.zip`) with real fluid parameters (`fluidDensity` 7000,
`shearStrength` 4000, `defaultDepth` 0.15), so a mud pit is a material index in
the `u8` layer, not geometry.

The generator writes **V1-style** materials (`diffuseMap`, `detailMap`,
`detailSize`). The V2 fields (`baseColorBaseTex` and friends) additionally need a
`TerrainMaterialTextureSet` object; V1 does not, and `groundmodelName` is common
to both. Both field sets exist on a live `TerrainMaterial` -- probed in-game.

## The full surface catalogue, and which ones make a sound

`art/groundmodels.json` inside `gameengine.zip` is the **only** ground model
file the game has -- a sweep of every zip in the install finds no second one, and
no shipped level adds any -- so this is the complete set a `groundmodelName` may
name. 32 models, expanded by their `aliases` into the 60 registrations
`be:getGroundModelCount()` reports live. An alias is a full alternate name, so
`groundmodelName = "grass2"` and `"GRASS"` are the same surface.

**`collisiontype` is the field that decides what a surface SOUNDS like**, and it
is a different question from how it drives. It is a name from the fixed material
list at the top of `lua/common/particles.json` (whose order is the hardcoded id
space -- the file says so in a comment), and `sounds.lua` then branches on that
id. **Only 14 of those ids have a tire branch at all.** Everything else is
driven over in silence: no roll, no skid, no kickup. That is invisible in
`groundmodels.json`, where SNOW looks as complete as GRAVEL.

`wheels.wheels[i].contactMaterialID1` reports this same id, which is what makes
it readable from the vehicle VM -- 10 asphalt, 19 gravel, 15 dirt, and so on.

| groundmodelName | static | sliding | rough | depth | collisiontype | tire sound |
|---|---|---|---|---|---|---|
| ASPHALT | 0.98 | 0.70 | 0 | 0 | ASPHALT | rigid |
| ASPHALT_OLD | 0.96 | 0.67 | 0 | 0 | ASPHALT | rigid |
| ASPHALT_PREPPED | 1.50 | 1.20 | 0 | 0 | ASPHALT | rigid |
| ASPHALT_WET | 0.92 | 0.55 | 0.20 | 0 | ASPHALT_WET | rigid |
| COBBLESTONE | 0.93 | 0.69 | 0.05 | 0 | COBBLESTONE | rigid |
| RUMBLE_STRIP | 0.96 | 0.67 | 0 | 0 | RUMBLE_STRIP | rigid + own layer |
| METAL | 0.85 | 0.65 | 0 | 0 | METAL | rigid |
| METAL_TREAD | 1.00 | 0.68 | 0 | 0 | METAL | rigid |
| WOOD | 0.95 | 0.70 | 0.15 | 0 | WOOD | rigid |
| ICE | 0.40 | 0.20 | 0.10 | 0 | ICE | rigid |
| FRICTIONLESS | 0 | 0 | 0.10 | 0 | ICE | rigid |
| SLIPPERY | 0.60 | 0.20 | 0 | 0 | ASPHALT_WET | rigid |
| KICKPLATE | 2.00 | 2.00 | 0 | 0 | ASPHALT | rigid |
| VOID | 0 | 0 | 0 | 1e7 | ASPHALT | rigid |
| DIRT | 0.70 | 0.73 | 0.42 | 0 | DIRT | loose |
| DIRT_DUSTY | 0.68 | 0.70 | 0.38 | 0 | DIRT_DUSTY | loose |
| DIRT_DUSTY_LOOSE | 0.65 | 0.65 | 0.46 | 0.02 | DIRT_DUSTY | loose |
| GRAVEL | 0.69 | 0.74 | 0.44 | 0 | GRAVEL | loose |
| GRAVEL_WET | 0.62 | 0.60 | 0.38 | 0 | GRAVEL | loose |
| GRASS | 0.61 | 0.65 | 0.43 | 0.05 | GRASS | loose |
| ROCK | 0.93 | 0.65 | 0.15 | 0 | ROCK | loose |
| SAND | 0.60 | 0.60 | 0.50 | 0.10 | SAND | loose |
| MUD | 0.55 | 0.55 | 0.50 | 0.15 | MUD | loose |
| SNOW | 0.65 | 0.40 | 0.30 | 0 | SNOW | **silent** |
| SNOWBANK | 0.60 | 0.60 | 0.50 | 0.25 | SNOW | **silent** |
| BRANCHES_STRONG | 0.60 | 0.60 | 0.50 | 0 | FOLIAGE | **silent** |
| LEAVES_STRONG | 0 | 0 | 0 | 0 | FOLIAGE | **silent** |
| LEAVES_THIN | 0 | 0 | 0 | 1.0 | FOLIAGE_THIN | **silent** |
| PLASTIC | 0.80 | 0.70 | 0 | 0 | PLASTIC | **silent** |
| SOFT_COLLISION_GENERAL | 0 | 0 | 0 | 0 | PLASTIC | **silent** |
| SHOCK_ABSORBER | 0 | 0 | 0 | 0.50 | RUBBER | **silent** |
| SPIKE_STRIP | 0.85 | 0.65 | 0 | 0 | SPIKE_STRIP | **silent** |

**A rigid surface goes quiet the moment the wheel sinks into it.** Every rigid
branch is gated on `wd.contactDepth == 0` as well as the material id, so an
asphalt cell under standing fluid stops sounding like asphalt without sounding
like anything else. The loose branches carry no such gate -- their volume and
pitch are *driven* by depth, which is what the `.ter.depth.png` feeds.

**A loose surface also throws a kickup one-shot** (`event:>Surfaces>kickup_<x>`)
above about 2 m/s of wheel periphery speed. Asphalt has one too; the other rigid
surfaces do not.

So for a map read by ear, the surfaces worth spending on are the fourteen with a
branch, and they fall into six audibly distinct families: **hard smooth**
(asphalt, and cobblestone as a rougher variant of it), **hard resonant** (metal,
wood), **slick** (ice, wet asphalt), **loose dry** (gravel, dirt, dirt dusty),
**loose soft** (sand, mud, grass) and **rumble strip**, which is the only one
with a layer of its own on top of asphalt. Snow is the trap: it drives
convincingly and says nothing.

`roughnessCoefficient` is consumed C++-side -- Lua only registers it -- and is
the tire vibration, separate from the sound branch. GRAVEL's 0.44 against
ASPHALT's 0 is what makes the verge cue work even before the sound changes.

## Three things the files cannot tell you

All were found by reading the engine's Lua, not by guessing, and all are the
kind of error that is invisible in the source and obvious from the seat.

1. **A SpawnSphere faces backwards.** `core/levels.lua:400` and `spawn.lua:974`
   both place a vehicle with `quat(spawnPoint:getRotation()) * quat(0,0,1,0)`,
   and `quat(0,0,1,0)` is 180 degrees about Z. So the matrix written into the
   file must encode the *opposite* of the heading you want. `SPAWN_FLIP_180`.

2. **The rotation matrix convention is undecidable from the shipped data.**
   Every shipped matrix is a pure yaw, so row-major and column-major readings are
   transposes -- identical apart from the sign of the rotation -- and one sample
   cannot separate them. Left as `SPAWN_MATRIX_COLUMN_MAJOR` and **settled in
   game**: column-major is correct. Measured with the engine's own placement
   expression, `spawn_default` faces (0.00, 1.00) and `spawn_climb_summit` faces
   (0.00, -1.00), which is the whole point of the summit spawn.

3. **Putting SpawnSphere objects in the scene does not make them pickable.**
   `core/levels.lua:138` builds the spawn list from `info.json`'s `spawnPoints`
   array alone; the scene is only consulted afterwards, to resolve each
   `objectname`. When the key is missing the parser falls through to line 161 and
   inserts **one** synthetic entry with no `objectname`, translated as
   `ui.common.default`. So a level with six spawn spheres and no `spawnPoints`
   block offers exactly one choice, called "Default" -- which reads as the
   spheres having failed, when in fact nothing ever asked for them.
   `defaultSpawnPointName` must match one `objectname` in that array or the
   synthetic entry is appended anyway. `previews` is optional: the same parser
   fills it from the level preview.

By contrast `quatFromEuler(0, 0, psi)` **was** settled, by measurement rather
than reasoning: it maps `+Y` to `(sin psi, cos psi)`, so the POI heading is
`atan2(fx, fy)` un-negated, and there is no 180 flip on that path because a
`quickTravelPosRotFunction` returns its rotation directly.

## POIs live in the level's own `mainLevel.lua`

`freeroam.lua:187` runs `extensions.loadAtRoot(<levelDir>/mainLevel, "")` on
mission start, so a level folder's `mainLevel.lua` is a **real extension** and
receives engine hooks -- `onGetRawPoiListForLevel` among them. That keeps the
big-map markers inside the level instead of needing a separate mod extension,
and it is why `smallgrid/mainLevel.lua` can define `onEnvironmentGetOtherButtons`.

The marker shape is `{data, id, markerInfo = {bigmapMarker = {pos, icon, name,
description, quickTravelPosRotFunction}}}`. `quickTravelPosRotFunction` is the
one that matters here: it returns **position and rotation**, which is what lets
the summit marker drop you facing back down the climb so the hill is a descent
without having to drive up it first.

`mainLevel.lua` is generated from `mapdef.py`, so a section that moves in the
layout cannot leave its marker behind.

## Level discovery

`core_levels._findAvailableLevels` prefers a `main/` **directory** over a
`main.level.json` file over any `.mis`, so the modern skeleton is
`levels/<name>/main/items.level.json` plus `main/MissionGroup/<Group>/items.level.json`,
one JSON object per line. `core_levels.getList()` caches; a newly installed
unpacked mod needs `core_modmanager.initDB()` and a cache drop before it appears.
`core_levels.onFilesChanged({{filename=..., type="modified"}})` is the safe drop.

**`FS:updateDirectoryWatchers()` and `FS:triggerFilesChanged()` CRASH the game**
(`A crash has happened in C++ code on main thread`, no Lua error first). They
look like the obvious way to make the engine re-read a mod folder and they are
not; there is no Lua-side way to force that, and the answer to "is the engine
serving me a stale file" is almost always "no, the reload has not finished" --
see below.

**Reloading the level in place works. What does NOT work is checking whether it
did.** `freeroam_freeroam.startFreeroam(<the level already loaded>)` logs
`Delaying start of freeroam until current level is unloaded...` and then queues
behind an unload; measured four times in one session, it completed in 13 to 16
seconds every time. An earlier version of this note claimed it "sometimes parks
and the unload never comes, recovery is a game restart". That was wrong, and both
halves of the evidence for it were instrument failures on this side:

- **The scene probe was chosen where both answers agree.** A generated feature
  was sampled at a point where its own profile is mathematically zero -- the
  suspension straights' harsh lane at 500 m along, which is a whole number of
  cycles for both its 25 m swell and its 10 m ripple -- so it read `BASE_Z` on
  the new terrain exactly as it would have on the old. Reading 49.997 four
  minutes apart looked like conclusive proof that nothing had loaded. **A probe
  point for "did this change" must be one where the two cases DIFFER**, which
  for a periodic profile means deliberately off a node of the wave.
- **The log offset was captured after the event it was watching for.** The
  `Delaying` line and the whole load were already behind the byte offset, so
  "zero new bytes since the request" was a statement about the offset, not about
  the game.

The real diagnostic is `grep "Level loaded in" beamng.log` over the WHOLE file
and comparing timestamps against `os.clockhp()` at the moment of the request --
never a tail from an offset taken later, and never a scene query at a point that
cannot distinguish the two builds. Getting this wrong is expensive in a way that
goes beyond wasted time: a false "it never loaded" sent the user to reload the
level by hand, and the manual load then looked like the thing that had worked.

**Rebuilding while the game is running REMOVES the level from the list.** `build.py` starts by
`rmtree`-ing the whole mod folder, so a rebuild against a live game deletes the files the mod
manager has already indexed and the level simply vanishes -- `core_levels.getList()` stops
returning it, it disappears from the level picker, and (the symptom that gets reported) it offers
no spawn points at all, because there is nothing left to read them from. Nothing logs an error;
the level is just gone. It is recoverable without restarting, with the same two calls a freshly
installed unpacked mod needs:

```lua
core_modmanager.initDB()
core_levels.onFilesChanged({{filename = "/levels/proving_grounds/info.json", type = "modified"}})
```

Verified: straight after that, `getList()` carried the entry again with all eight spawn points.
The safe habit is to rebuild with the game shut down, or to run those two lines afterwards.

**Do not clear the level-list cache with `extensions.reload("core_levels")`.** It unloads
the extension's dependents (`scenario_scenariosLoader`, and
`ui_busRouteSelector_general` behind it) and leaves level *switching* wedged --
`startLevel` then returns silently and `serverConnection.disconnect()` does not
take. Nothing in the log says so; the only symptom is that loading a level does
nothing at all. A game restart clears it.

## The material layer is a CELL raster; the heightmap is a NODE grid

The two layers of a `.ter` are not the same kind of grid, and a mask written for
one is wrong for the other. Heights are nodes (sample `i` sits exactly at
`position + i * squareSize`), so an inclusive mask is right: both edge nodes
belong to the thing. Materials are one byte per **square**, so an inclusive mask
claims an extra column at each end and neighbouring regions fight over the
boundary -- last painter wins a cell of the other.

Not cosmetic. With inclusive masks each lane took the node at exactly
`cx +/- LANE_HW` at *both* ends, so the 5 m gap between two lanes was left
holding a **single** cell of rumble strip, while the outer strips -- with no lane
on the far side to eat into them -- kept both of theirs. The dividers would have
been 2.5 m in the middle of the section and 5 m at its edges: two different cues
for one boundary, on the one feature whose whole job is to say which lane you are
in. Mixing the conventions between the section and its own gravel margin also
left a 2.5 m sliver of stray asphalt outside the outer strip, on one flank only,
because the inclusive and half-open edges disagree at just one end.

`mapdef._cells(x, lo, hi)` is the half-open form, and every material mask in the
straights uses it.

**Measured in the running game, and it had to be** -- the two readings differ by
only 1.25 m, which is well inside anything a top-down look or a scene query could
resolve, and the shipped terrains do not distinguish them either. The only
instrument is `wheels.wheels[i].contactMaterialID1` (there is no position-to-
material query on `TerrainBlock` at all). Caught on a single frame with the car
straddling the medium lane's west divider:

    FR x = -1208.32  mat = 29 (RUMBLE_STRIP)
    RR x = -1207.13  mat = 15 (DIRT)

The cell reading puts that boundary at exactly **-1207.50** and the node reading
at **-1208.75**. The rumble hit at -1208.32 is on the dirt side of the node
prediction, so the node reading is excluded and the cell reading is confirmed:
`materialIdx[row][col]` governs the square `[origin + col * squareSize,
origin + (col+1) * squareSize)`. Note the probe must use each WHEEL's own world x
-- `spawn.safeTeleport` relocated the car 3.6 m from the requested position in
the first attempt, so the position asked for says nothing about where the sample
was taken.

## The waterline is the position plane, and the surface needs visible styling

Two independent traps, and each produced a pool that was invisible AND dry while
every number the generator could check said 50.00.

**A `WaterBlock` collision volume is a box centred on `position`, with `scale`
as its full extent, but its waterline is `position.z` -- not the top of that
box.** `getObjBox()` and `getWorldBox()` describe collision bounds; neither says
which plane is the water surface. This was settled against the simulation on a
freshly loaded level: in the Sump, with `position.z` at 46.75, the highest wet
vehicle node was 46.749 and the lowest dry one was 46.960. Moving a live block
and calling `postApply()` leaves stale physics registration, so only a fresh
load is valid for that test. `verify.lua` compares `position.z` with the
independently sampled terrain bed, then finishes with an `obj:inWater(cid)` node
count.

**A `WaterBlock` with no textures renders nothing** -- not an error in the scene,
just an invisible pool. `surfMaterial` is empty on every shipped water object in
the game; the surface is built from `rippleTex`, `foamTex` and
`depthGradientTex`, and a missing one logs
`engine::WaterObject::initTextures| Texture missing:` with an **empty name**,
which is the only evidence there is. Note that a stock level is not proof a path
is live: `east_coast_usa` still names `core/art/water/foam.dds`, for which
`FS:fileExists` answers false in a running game. The paths used here are italy's
`/assets/materials/tileable/water/...`, which are shared assets and therefore
mounted for every level -- the same self-containment argument the berms make
against a `.cdae` guardrail.

**A working volume can still look completely dry.** The first textured style
used clarity 0.5, fresnel bias 0.2, and no fallback cubemap. Both blocks existed,
were render-enabled, had valid textures, and produced underwater fog, but an
overhead capture of the ford showed only its dirt bed. The shipped water objects
all provide a fallback cubemap because `fullReflect = false` uses it when planar
reflections are disabled or unavailable. The generated blocks use the global
`BNG_Sky_01_cubemap`, clarity 0.2, fresnel bias 0.45, reflectivity 0.65, and a
slightly stronger ripple magnitude. That combination was checked in the running
level: it makes the ford distinctly blue and textured without requiring costly
planar reflections. `verify.lua` reports these style fields separately from the
wetness check so a visually dry regression cannot hide behind correct physics.

## Deep water really does kill the engine

No custom code. `combustionEngine.lua:329` `checkHydroLocking` raises
`floodLevel` while **all** of the engine's `waterDamage` nodes report
`obj:inWater`, then calls `device:lockUp()` past the threshold. The Sump is
dished rather than walled so driving in is a decision, not an ambush.
