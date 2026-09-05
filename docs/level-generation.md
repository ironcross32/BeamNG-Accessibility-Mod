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
- `mapdef.py` -- the Proving Grounds layout and terrain synthesis
- `build.py` -- assembles the level and installs it to `mods/unpacked/`
- `verify.lua` -- post-load checks for everything the generator had to assume

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

## Proving Grounds layout

5120 m square, 2048 heightmap at 2.5 m/cell (12.6 MB -- the same terrain size
`car_jump_arena` ships). A flat plateau at 50 m with everything cut into or
raised out of it, so **any slope underfoot is information**.

| Section | Where | What |
|---|---|---|
| Staging Yard | (0, -2200) | asphalt pad, default spawn |
| Mud Basin | (-1500, -900) | 5 MUD pits in a DIRT apron |
| Shallow Ford | (1300, -900) | 0.45 m water, drivable |
| The Sump | (1500, 340) | 4.5 m water; hydrolocks the engine |
| Hill Climb base | (0, -780) | foot of the three-kilometre climb |
| Hill Climb summit | (0, 2265) | flat at z=682.15 m, spawn faces back down |
| One-Kilometre Tunnel | (-2000, 460) | south-portal spawn facing north into a 1000 m tunnel |
| Suspension Straights | (-1220, 240) | 3 undulating dirt lanes running north |
| Sound Stage | (400, -2200) | rolling road, hamster wheel, tilt ramp |

### Asphalt is the surface; gravel is the margin

GRAVEL's `roughnessCoefficient` is 0.44 against ASPHALT's 0, so the tyre noise
changes the moment a wheel crosses between them. That is a free boundary cue for
a driver who cannot see the edge, and it costs nothing.

The first version spent it the wrong way round -- gravel was the default and
asphalt was painted on the roads, so **92 % of the map rumbled** and the cue fired
exactly once, on leaving a road, after which everything sounded the same again.
Asphalt is now the driving surface (the roads *and* the plain between them) and
gravel is laid only as a **margin** around something there is a reason to stay
inside: each road, the yard, the climb lane within its berms, each section, and
the map perimeter. A rumble then always means one thing -- "you are leaving the
thing you are on" -- which is the property that makes it learnable at all. The
mix went from 92 % gravel / 0.9 % asphalt to 2.6 % / 90.6 %.

A margin is a **band, not a fill**, so crossing it and carrying on puts you back
on asphalt. That is deliberate: the width of the rumble says whether you clipped
an edge or drove out of a section, and a margin that never ended would be the old
scheme again. `MARGIN_W` is 10 m (25 m at the map perimeter, where there is no
road to reference), several cells wide so it cannot be lost to the same
heightmap quantisation that caught the berm.

**The verge is computed against the UNION of the road network, never per road.**
Growing each rectangle separately lays every road's margin across its
neighbour's carriageway, so each junction gets a gravel bar straight through it
-- a false "you are leaving the road" at precisely the places you are not. Grow
them all, subtract them all, and the band survives only where nothing else is a
road.

The climb keeps its own shoulder (`CLIMB_SHOULDER_W`, 5 m between the asphalt
lane and the rock berm). It is split on each side into one 2.5 m cell of
`RUMBLE_STRIP` immediately outside the asphalt and one 2.5 m cell of gravel
outside that. Drifting wide therefore produces two ordered cues: rumble marks
the asphalt edge, then gravel says the car is continuing toward the berm. The
berm is the structural backstop, and at 54 % the warnings are the half of that
arrangement that is any use.

The visible climb is painted into the terrain, but terrain materials alone do
not create a BeamNG navigation road. The generator therefore adds a 15 m-wide,
two-way `DecalRoad` named `road_hill_climb`, with `drivability = 1` and the stock
`road_invisible` material. This makes the climb part of `map.getMap()` for both
vehicle AI and BeamTel's road detector without drawing a decal over the terrain
or masking the asphalt, rumble-strip, and gravel ground models.

Its control points follow the climb every 50 m, plus the base approach and
summit pad. That spacing is about vertical correctness, not visual smoothness:
the climb profile is nonlinear, while the navigation graph interpolates between
road nodes. Joining only the endpoints would put the graph more than 100 m above
the actual road near the middle, far outside the detector's 6 m overpass
tolerance. `useSubdivisions = false` ensures the graph uses the generated
height-following control points directly.

The original climb met the summit at its full grade and the pad
became flat in one 2.5 m cell. Although the heights met, that slope discontinuity
was a hard convex hump in either direction. A 45 m cubic vertical curve now
starts after the full 3000 m climb at y=2220 and eases continuously from 54
percent to zero at the summit centre (y=2265). A proper rounded crest must keep
rising while its grade falls, so the final flat pad is 12.15 m higher at 682.15 m;
the north half remains level for turning around.

### The timed hill-climb mission

The base marker is a native `timeTrial` mission POI rather than a second
level-local marker. Entering it at low speed, including after quick travel,
opens BeamNG's Activity Start prompt; accepting uses the current vehicle,
places it just south of the timed line, and runs north through checkpoints every
250 m to a finish on the flat summit. Recoveries and flips remain available and
add five seconds each. No medal thresholds are presented: official elapsed time
and the native personal best are the score.

The mission files are generated at the mod root under
`gameplay/missions/proving_grounds/timeTrial/hill_climb/`, because gameplay
missions are not level children. `race.race.json` is synthesized from the same
height function as the terrain, including the cubic crest, so checkpoint and
recovery poses cannot drift away from a changed profile.

### The one-kilometre tunnel

The tunnel runs due north at x = -2000, from y = 500 to y = 1500, with its
spawn and big-map POI 40 m before the south portal. It is made from sixteen
instances of BeamNG's stock `ut_tunnel_64m.dae`. The game mounts the Utah asset
globally even while Proving Grounds is loaded (`FS:fileExists` returned true),
so the generated mod can use the real collision tunnel without copying it.

Sixteen unmodified modules would be 1024 m, not one kilometre. Each module's
local Y scale is therefore `62.5 / 64 = 0.9765625`; generated bounds put the
portals at exactly 500 and 1500. A separate invisible, drivable DecalRoad runs
across both approaches and through the bore, making this disconnected road
component available to vehicle AI and BeamTel's road detector.

The stock module's pavement is 0.20 m above its object origin. Placing the
module origins at the plateau height therefore created a square-edged 20 cm lip
at both portals. Every module is now sunk by 0.20 m so the pavement and terrain
approaches meet flush; a ramp is unnecessary when there is no remaining height
difference to climb.

Vehicle reverb comes from an `SFXSpace` filling the 9 x 1000 x 8 m interior.
Its `soundAmbience` is `Level_Tunnel_Closed_Dynamic`, the globally available
preset used by Utah's shipped road tunnel. This is an actual ambience/snapshot
volume, not a looping sound effect: entering it changes the vehicle mix and
leaving it restores the exterior mix. The shell also receives twenty warm
point lights at 50 m spacing.

### The sound stage

Three of the game's own test rigs on a flat asphalt pad east of the Staging
Yard, for listening to an engine work rather than for driving anywhere.

**None of it can be terrain, and that is the first thing worth recording.** A
`.ter` is a heightmap plus one material byte per cell, and **nothing in the
ground model catalogue moves** -- `groundmodelName` picks friction, roughness
and tyre sound and never motion. A rolling surface cannot be a surface at all;
it has to be an object. It also needs no mod: BeamNG ships three, all
`"Type": "Prop"` vehicles.

| Rig | model / config | What it is |
|---|---|---|
| Wheel Roller | `testroller` / `multi` | rolling road; wheels spin, car stays put |
| Hamster Wheel | `large_hamster_wheel` / `unpowered` | 14 m drum on an axle, spun by driving inside |
| Tilt Ramp | `testroller` / `ramp` | adjustable roller ramp, the AWD demo rig |

**The Wheel Roller's rollers are not drums -- they are frictionless troughs**, and
that decides what the rig is good for. `testroller.jbeam` gives the ramp faces
`"frictionCoef": 0` with high-friction rubber (`frictionCoef` 35) only at the two
end rails, so the wheel sits in a dip and spins. Once it is up to speed there is
therefore **essentially no load**: what you hear is a free rev at an indicated
speed, not an engine pulling. The Hamster Wheel is the one that loads properly,
because the car is continuously climbing the inside of a drum and never reaches
the top. Worth knowing before choosing a rig by its name.

`autoAdjust.lua` is why the roller needs no aiming: once a second it takes the
nearest vehicle within 100 m, asks it for its own track width and wheelbase over
`chassisData`, and resizes itself to fit. Park near it and it fits itself to your
car.

**The rigs are SPAWNED, not placed, because a level scene cannot hold a vehicle.**
A class census of `gridmap_v2`'s `items.level.json` is 7167 `TSStatic`, 12
`SpawnSphere` and **zero** `BeamNGVehicle`, and no shipped level puts one in its
scene -- these props are all spawned at runtime. `mainLevel.lua` is already a
real extension receiving engine hooks, so it does it there, two seconds into the
mission and only once a **player vehicle exists**: `autoEnterVehicle = false`
ought to be enough on its own, but the cost of being wrong is a session that
opens with the driver sitting inside a hamster wheel, and waiting removes the
question instead of answering it. `rigAlreadyThere` makes it idempotent, because
the file is re-run by an extension reload as well as by a mission start and a
second hamster wheel spawned inside the first is not a recoverable state.

**Every rig is entered heading NORTH, and that one rule is why all three facings
differ.** A prop's facing is where its own `+Y` points, which for two of these is
not the direction you drive into it -- each was read out of the shipped jbeam,
not guessed, and each is asserted by `verify.lua`:

- **Wheel Roller, faced north.** `autoAdjust` sizes track width along local X and
  wheelbase along local Y, so its `+Y` *must* lie along the car's length. This is
  the one where the facing changes what the rig does rather than where its door
  is.
- **Hamster Wheel, faced east.** The drum's circle is in the local Y-Z plane
  (radius 7.17 m) and its axle runs along local X, with a drive-in ramp at each
  **end of that axle** (`r*_2` nodes at |x| 11.5-12.7, z -0.1). So you enter
  *across* the drum and then **turn 90 degrees** to drive around the inside of
  it. Faced east, the two ramps face north and south.
- **Tilt Ramp, faced south.** `testroller_tiltramp.jbeam` names its node groups:
  `onramp_R`/`onramp_L` sit at local **+Y** (with fixed ground nodes at z = 0)
  and `offramp_*` at local -Y. You drive on at +Y travelling in -Y, so the prop
  has to be turned around to be entered northbound.

**Two placement numbers were wrong and both were caught by measurement, not by reading.** They
are worth recording because neither is visible in the source:

- **The approach distance must stay inside `RAMP_SEARCH_M` (70 m).** At the first value tried,
  90 m, `rampTruth()` answered *"no ramp among 0 objects within 70 metres"* while parked in clear
  sight of all three rigs -- which reads exactly like an instrument that does not support them.
  `STAGE_APPROACH_M` is 45 m.
- **A rig's spawn position is its REFERENCE NODE, not its centre.** The hamster wheel's drum sits
  **7.22 m** from the node the spawner places, so spawning it at x = 400 put the drum, both ramps
  and the resolved mouth at x = 392.78 while the POI kept dropping the driver at 400 -- 7.2 m to
  one side of a 6.74 m mouth. `FWD_OFFSET_M` cancels it, applied along the prop's own facing so
  the correction survives someone turning the rig round.

**Do not test a placement with `spawn.safeTeleport`.** Moving a rig by hand to check a facing
does not do what it looks like: asked for (407.22, -2200) it put the wheel at (410.57, -2195.30),
because `placeVehicle` searches outward whenever the requested box overlaps anything -- the same
relocation `vehicleScanner`'s align documents. Delete and respawn instead, or the geometry you
then measure is the geometry of somewhere else.

The pad is a **material feature only** -- the plain here is already dead level
asphalt, which is exactly what a rig wants -- so the whole of what is built is
the gravel margin, under the same rule as everything else on this map ("you are
leaving the thing you are on"). One POI per rig rather than one for the pad,
because quick travel is the tool used mid-session and its value here is that it
puts you down **lined up**: 90 m south of that rig's own centre, facing north.

**Do not test this with `extensions.reload("mainLevel")`.** The level copy is not
on the extensions search path, so the reload *unloads* it and then fails to find
it -- taking the big-map POIs down with it for the rest of the session, silently.
`extensions.loadAtRoot("/levels/<name>/mainLevel", "")`, the call `freeroam.lua`
itself makes, is the one that works.

### The suspension straights

Three parallel lanes running due **north**, 1000 m long, 15 m wide, on a 20 m
pitch, each undulating harder than the last. Cardinal because every other long
feature here runs north (the spine road, the climb), so "drive north" is the one
heading that needs no reference -- and a lane you can only hold by steering is
not a suspension test. A 120 m flat apron in front of them is the baseline you
set the car up on.

| lane | swell | ripple | peak-to-peak | steepest |
|---|---|---|---|---|
| gentle (west) | 55 m / 0.18 m | 20 m / 0.05 m | 0.46 m | 3.4 % |
| medium | 40 m / 0.35 m | 15 m / 0.11 m | 0.89 m | 9.2 % |
| harsh (east) | 25 m / 0.60 m | 10 m / 0.20 m | 1.54 m | 22.1 % |

Two components per lane on purpose: the long swell is a body-control test, the
short ripple is a wheel-control test, and a single sine is only one of those.
Severity is raised by shortening the wavelengths *and* growing the amplitudes,
because what a suspension feels is their ratio.

**A real washboard cannot be built out of this heightmap, at any amplitude.**
At 2.5 m per cell the Nyquist limit is a 5 m wavelength and four samples per
cycle (10 m) is the shortest that still reconstructs as a wave rather than as a
triangle of arbitrary amplitude -- so the 0.3-1 m ripple that makes a gravel road
roar is simply not expressible. What is expressible is the whoop/swell band from
10 m up, which at 50-100 km/h excites 1.4-2.8 Hz: body heave and pitch, where
dampers actually work. The high-frequency end comes from the rumble strips
instead, which can supply it because the engine synthesises that from the ground
model rather than from geometry.

**Every wavelength is an exact multiple of `METRES_PER_CELL`**, and that is not
tidiness. A wavelength that does not divide by the cell size beats against the
node grid: the sampled crest drifts in and out of phase, amplitude varies by up
to 30 % along the lane, and no two cycles are the same bump -- so the lane's
severity changes along its own length and cannot be compared with the lane beside
it, which is the entire point of having three.

**...and no lane's swell wavelength is an integer multiple of its ripple's.**
That locks the two components into one phase relationship for the whole lane, and
which one you get is luck. The gentle lane was first specified at 60 m over 20 m,
exactly 3:1, which puts the ripple at its trough on every swell crest and
cancelled 29 % of the amplitude: 0.33 m peak-to-peak where the two amplitudes
were chosen to give 0.46. Nothing in the constants says so, and it reads exactly
like the node grid having eaten the wave -- which it had not, since the
*continuous* profile measures 0.326 as well. This is why `describe_susp` compares
the sampled lane against a densely-sampled ideal rather than against
`2 * (A_swell + A_ripple)`: the naive bound tests the generator's own arithmetic,
where the honest one tests whether the wave survived the grid.

**The dividers are rumble strips, and rumble means something different from
gravel.** Gravel says "you are leaving the thing you are on"; the strips say
"you are crossing from one lane into the next". Two cues for two questions beats
one cue used for both, and the strips are the only surface on the map a wheel can
be on while still being somewhere legitimate. They bound the outer lanes as well
as separating the inner ones, so drifting either way is announced -- and they run
through the **apron** too, so the lane you want can be found and counted off
while stationary rather than discovered once the test has started.

`RUMBLE_STRIP` works here **only because it is a synthesised effect, not
geometry**. Its groundmodel carries `roughnessCoefficient` 0, so nothing in the
data suggests it does anything -- but `sounds.lua` drives
`event:>Surfaces>roll_rumblestrip` from `wd.peakForce` and `obj:getPeakPeriod()`,
both C++ side, and hirochi_raceway's strip is a *flat* decal-road material
carrying `groundType: "RUMBLE_STRIP"` with no bumps modelled anywhere. (On a mesh
material the field is `groundType`; on a `TerrainMaterial` it is
`groundmodelName`. Same value.)

**The strip between two lanes is the linear interpolation between them.** The
lanes carry different profiles so their edges sit at different heights, up to a
metre apart on the harsh lane; left as a step that is a kerb you hit sideways at
speed. The lateral blend weight falls linearly to zero over exactly the separator
width, so two neighbouring weights sum to 1 across the strip: banked, continuous,
and telling you which way you drifted. Each lane is otherwise **flat across its
own width**, which is what makes a run repeatable -- both wheels get the same
input and the section measures heave, not roll.

Amplitude is windowed to zero at both ends over 50 m. Without that the entry is
a step, because a sine is at its steepest as it crosses zero -- an untapered lane
would open with its worst gradient at the moment the car arrives at speed.

### The material layer is a CELL raster; the heightmap is a NODE grid

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

### The climb

`grade(s) = G0 + (G1 - G0) * (s/L)^P`, with `P = 2`. The progressive run-up
keeps the severe grades near the end so ordinary vehicles retain momentum for
the finish. The climb reaches its 54 percent maximum only at the end of the
3000 m profile, immediately before the separate summit vertical curve begins.

| along | grade | height |
|---|---|---|
| 0 m | 4.0 % | 50 m |
| 750 m | 7.1 % | 88 m |
| 1500 m | 16.5 % | 173 m |
| 2250 m | 32.1 % | 351 m |
| 3000 m | 54.0 % | 670 m |

The former 60 percent finish exceeded the sustainable traction limit of ordinary
front-wheel-drive cars once uphill weight transfer unloaded the driven axle. The
54 percent (28.4 degree) maximum is still a demanding power and momentum test,
but the road remains asphalt throughout rather than becoming a trick surface.
Dirt and gravel versions can use the same profile later.

Walls are **terrain berms**, not `TSStatic` guardrails: 12 m of rock rising over a
single cell at 12.5 m either side of the road, flat-topped out to 22.5 m. A mesh rail
would mean referencing a `.cdae` out of another level's zip, which is only
mounted when that level is loaded. A berm is self-contained, and to a driver who
cannot see it, it stops the car the same way.

**The berm has no lateral ramp at all**: every node outside `CLIMB_HALF_W`
stands at full `BERM_H`, so the face is one cell wide by construction rather than
by a constant that has to be kept equal to the cell size.

That is the third version of this wall, and each failure was less visible than
the last. The first ramped 3.5 m linearly across the whole 8 m shoulder and
measured **31-44 %** in game -- *shallower than the 54 % at the top of the climb
it was meant to contain*. The second raised it to 7 m over "one cell" of lateral
run, which reads as a wall in the source and was still driven over: with
`CLIMB_HALF_W` at 12.0 and `BERM_RISE` at 2.5, the ramp spanned **two** nodes,
and measured mid-climb the outer of the two was **84 %** -- a launch ramp with a
7 m lip, sitting exactly where a car deflected by the 196 % inner face arrives. A
wall that is steep for its first metre and climbable for its second is not a
wall, and nothing in the constants said so.

Terrain cannot be vertical: at `METRES_PER_CELL` the steepest face expressible is
`BERM_H / METRES_PER_CELL`, so steepness is bought with **height** and with
nothing else. 12 m over one 2.5 m cell measures 480 % (78 degrees), and the
height is also what stops a car that gets airborne off the face from clearing the
top -- which 7 m did not. `BERM_H` may not be lowered without raising the
heightmap resolution.

**The summit pad is a walled bowl with one gap.** The pad is 90 m across and the
berms are only 45 m apart, so assigning the pad height erases them -- which left
a flat asphalt platform 340 m above the plain, unfenced on three sides, at the
one place on the map where a car arrives at full throttle and has to stop. The
wall is the same one-cell `BERM_H` step, and the gap is exactly the road.

### The waterline is the position plane, and the surface needs visible styling

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

### Deep water really does kill the engine

No custom code. `combustionEngine.lua:329` `checkHydroLocking` raises
`floodLevel` while **all** of the engine's `waterDamage` nodes report
`obj:inWater`, then calls `device:lockUp()` past the threshold. The Sump is
dished rather than walled so driving in is a decision, not an ambush.
