# Generated Levels: Proving Grounds (`tools/mapgen/mapdef.py`)

Engine and file-format facts shared by every generated level are in [level-generation.md](level-generation.md).

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
