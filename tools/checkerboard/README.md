# Endless Surfaces prototype

A separate level generator. It neither imports nor modifies `tools/mapgen/`
or the installed Proving Grounds map.

Select **Endless Surfaces (Prototype)** in Freeroam. The default spawn is the
centre of an asphalt square, facing north. Dirt starts 250 m ahead; subsequent
surface changes are 500 m apart. There is also a spawn for each other surface.

Every square is 500 x 500 m. Moving north or east advances through asphalt,
dirt, gravel, sand, ice, grass, then asphalt again. Moving south or west reverses that
sequence. Both axes repeat every 3000 m. Certain diagonal headings can remain
on the same surface; the pattern is a grid, not concentric distance bands.

## Build and install

From the repository root:

```powershell
uv run python tools/checkerboard/build.py
uv run python tools/checkerboard/build.py --install
```

The first command writes `build/checkerboard/beam_endless_surfaces.zip` and an
unpacked copy beside it. `--out DIR` changes that output parent directory.
`--install` also copies the unpacked mod into
`%LOCALAPPDATA%/BeamNG/BeamNG.drive/current/mods/unpacked/beam_endless_surfaces`.
Use either that installed copy or the ZIP, not both. Building is deterministic;
`prototype.json` records terrain and runtime SHA256 hashes.

Prefer installing with the game closed. If installing while it is running,
refresh the mod database and level list before loading:

```lua
core_modmanager.initDB()
core_levels.onFilesChanged({{filename='/levels/endless_surfaces/info.json', type='modified'}})
```

Focus the BeamNG window after issuing a load/reload, and confirm a fresh
`Level loaded in` entry in `beamng.log`. A console acknowledgement does not
confirm that the level actually loaded.

## How wrapping works

The physical terrain is flat, 2048 samples at 5 m spacing, covering 10.24 km
square. Real terrain materials use the stock `ASPHALT`, `DIRT`, `GRAVEL`,
`SAND`, `ICE`, and `GRASS` ground models. Sand has a 10 cm depth layer and grass
has a 5 cm depth layer, matching their stock ground models. The raster uses
exact 500 m boundaries. It starts at z=0 and has no bumps or barriers.

When a group's reference vehicle crosses +/-2000 m on either axis, the group
is translated by whole 3000 m periods to an identical part of the terrain.
The 500 m hysteresis between the canonical half-period and wrap threshold
prevents repeated back-and-forth wrapping near a boundary. The wide buffer and
2500 m view distance keep the physical edge away from ordinary driving.

The runtime reads `core_vehicles.attachedCouplers` every frame and groups the
entire connected assembly. It also groups vehicle objects within 100 m of each
other, transitively, to carry nearby loose cargo and adjacent vehicles along.
Groups containing the player use the player's position; other groups use their
lowest vehicle ID. Distant groups wrap independently.

All destination positions are captured before moving any group member. The
runtime uses `setClusterPosRelRot(-1, ...)` with identity rotation for every
vehicle, covering every cluster, including broken-off parts. It calls no reset,
respawn, safe-teleport, velocity-zeroing, or camera-reset methods. Stock camera
code detects large position jumps; the visual transition still needs testing.

An unavailable coupler graph, oversized group that cannot fit in the terrain
buffer, or engine translation error disables wrapping and pauses physics. This
prevents continuing simulation after a partially completed group translation.
Read `mainLevel.status().fault` before resuming. A partially moved assembly may
need manual recovery. `mainLevel.setEnabled(false)` disables automatic wrapping;
`mainLevel.setEnabled(true)` clears the fault and enables it again.

This is a wrapping prototype, not a native infinite heightfield. Static editor
objects, recorded routes, skid marks and position-based external systems are
not translated. Distant independent vehicles may meet after wrapping onto the
same location. Multiplayer and AI routes across wraps are not implemented.

## Verification

```powershell
uv run python -m compileall -q tools/checkerboard/build.py tools/checkerboard/check.py
uv run --with lupa python tools/checkerboard/check.py
```

`lupa` is only a temporary test dependency; the generator needs the project's
existing NumPy dependency. The checks decode generated terrain and PNG files,
validate all scene JSON, exact boundaries, material assignments, sand/grass depth,
spawn placement, both-axis periodicity, and reproducible ZIP bytes. A mock Lua
engine exercises the real runtime's threshold logic, diagonal/negative wraps,
coupler cycles and chains, nearby cargo, independent vehicles, pause handling,
and partial-translation failures. **Mocks cannot verify BeamNG physics.**

Load the included diagnostic in the GE Lua console:

```lua
extensions.loadAtRoot('/levels/endless_surfaces/verify', 'endlessSurfaces')
endlessSurfaces_verify.inspect()
endlessSurfaces_verify.checkContacts()
```

After a few frames, `endlessSurfaces_verify.result()` contains actual wheel
contact IDs and the expected material for each wheel position. Run this at
each spawn and across boundaries. Expected IDs from the installed
`lua/common/particles.json`: asphalt 10, dirt 15, gravel 19, sand 16, ice 21, grass 20.
Check tire sounds, sand resistance and ice grip from the driving seat as well.

For a paused before/after diagnostic, start near the centre, attach a trailer,
drive forward, and issue:

```lua
endlessSurfaces_verify.beginWrapTest(3000, 0)
```

This deliberately moves the current group while preserving its motion. It
pauses physics, waits for vehicle-VM wheel samples, runs the same translation
used by automatic wrapping, samples again, and restores the prior running
state on success. Keep the game focused; query
`endlessSurfaces_verify.result()` after a few frames. On failure it leaves
physics paused and logs the reason. The wheel samples use `w.coreData`
angular velocity, not the Lua wheel-speed cache that can remain stale while
physics is paused. This diagnostic checks instantaneous state only.

Then drive across automatic wrap boundaries in all four cardinal directions
and diagonally, including repeated wraps, at speed and while turning. Test a
tractor with multiple attached trailers and vehicle-based loose cargo. Inspect
`mainLevel.status().lastWrap`, and check coupling, steering, wheelspin, damage,
sound and camera behavior after simulation resumes. Reload the level and repeat
to verify initialization. Trailer safety and seamless camera/audio behavior
remain unverified until these live checks pass.

## Initial validation, 2026-09-05

Generated-file checks and mocked Lua tests passed, including identical archive
bytes across two builds. The local accessible console did not respond and the
available BeamNG log ended in shutdown; no fresh game load, wheel-contact,
audio, camera, or moving-trailer result is claimed.

The user subsequently reported that the five-surface prototype appeared to work
in game. Grass was then added as the sixth surface, extending the period to
3000 m. This regenerated version still needs its first in-game check.
