# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

BEAM is a screen reader mod for BeamNG.drive — an accessibility tool that converts real-time vehicle telemetry into audio cues, spatial compass clicks, and speech output for blind and visually impaired players. It runs on Windows 10+ and requires administrator privileges (for keyboard hooks).

## Build & Run Commands

**Package manager:** uv (with `uv.lock` for deterministic resolution)

```bash
# Install dependencies
uv sync

# Run main telemetry receiver (requires admin)
python beamtel.py

# Run configuration GUI
python configurator.py

# Run diagnostic listeners
python diagnostic/beam_listener.py
python diagnostic/diagnostic_listener.py
```

**Build executables with Nuitka** (build settings are embedded as Nuitka pragmas at the top of each file):
```bash
nuitka beamtel.py
nuitka configurator.py
```

There are no tests in this codebase.

## Architecture

### Two-Part System

The project has two halves that communicate via UDP:

1. **Python side** (this repo root) — Receives telemetry, generates audio, handles speech output
2. **Lua mod** (`bng_mod/`) — A directory junction to `%LOCALAPPDATA%/BeamNG/BeamNG.drive/current/mods/unpacked/bng_screenreader_mod/`. Edits to `bng_mod/` are live in the game.

### Python Components

The per-module detail — what each file owns, and the reasoning behind the decisions that are not obvious from the code — lives in @docs/python-components.md.

Summary: **beamtel.py** (main entry point; UDP telemetry listener, F9 keyboard command system, drives audio and speech), **audio.py** (procedural audio synthesis engine — compass clicks, articulation/implement/docking tones, terrain sonification), **hrtf.py** (MIT KEMAR binaural panning), **configurator.py** (wxPython config GUI), **speech.py** (Prism-backed speech output), **ai_describer.py** (vision-model screen description), **nvda_ws_speaker.py** (WebSocket bridge on 8765), **mcp_server.py** (loopback MCP automation server, off by default; includes `camera_control` and `screenshot`, which let an agent place the eye and write a PNG to disk for itself), **updater.py** (GitHub-only auto-updater: startup release check, self-replacement via a detached helper, and the pending-update flag that carries the mod install across the restart), **secretstore.py** (DPAPI at-rest protection for the config's API keys), **bnh_logger.py** (rotating file logger), **diagnostic/** (standalone UDP listeners and state-machine simulators).

### Lua Mod Components (`bng_mod/`)

Entry point: `scripts/bng_screenreader_mod/modScript.lua` — loads all GE extensions.

Grouped by function; each file below carries the full reasoning for its area.

- @docs/lua-telemetry-protocol.md — `lua/vehicle/protocols/796F6C6F313035.lua`, the custom 60 Hz extended telemetry struct on port 4444: `actualSteering`, the ramp hydraulics push, and the loader implement block (`implementFlags` and friends).
- @docs/lua-geometry.md — `vehicleGeometry.lua` (per-vehicle derived node geometry: extents, hull cids, contact bands, vertical occupancy histogram) and `rampGeometry.lua` (the drive-in mouth of a ramp-equipped machine). Both are libraries with no ports of their own. Note **`nd.group` does not exist at runtime** (it is `nd.firstGroup`, an index into `v.data.groups`), which made rampGeometry's group tier dead code on every vehicle in the game; that a machine whose mouth cannot be INFERRED gets a **declared** one — five node names per mouth, and the hamster wheel has **two**, chosen per query by which is nearer the driver; and that a floor fitted through **non-collision** nodes is not a floor, so the pitch and lip height are **withheld** rather than published (the tilt ramp announced “ramp down 33 degrees” while sitting dead level).
- @docs/lua-vehicle-scanner.md — `vehicleScanner.lua`: nearest vehicle detection, target cycling, coupler compatibility matching, alignment teleport. Ports 4445/4448.
- @docs/lua-extensions.md — `beamtelAI.lua`, `obstacleDetector.lua`, `cameraInfo.lua`, `terrainScanner.lua`, `cannonShot.lua`, `trailerAngle.lua`, `nodeGrabberAccessible.lua`, `clickspotAccessible.lua`, `uiToggle.lua`, `consoleAccessible.lua`, `vehicleBindings.lua`, `environmentAccessible.lua`, `routeBeacon.lua`.
- `vehicleInfo.lua` (see @docs/lua-extensions.md) — the stock vehicle selector's details-page specifications, answered on request. Ports 4477/4478.
- `routeBeacon.lua` (see @docs/lua-extensions.md) — the big map's navigation route destination, sent as a POSITION (port 4482, send-only) so Python derives a crow-flies bearing and pulses an HRTF-panned beacon at it (`F9` `Ctrl+W`). **The destination is `routePlanner.path[#path].pos`, never `endWP[1]`**, which is a waypoint-name string as often as a position. The bearing lives in one shared helper (`route_beacon.relative_bearing`, used by the beacon, `F9` `W` and `Ctrl+G` alike) because BeamTel's heading is **180° off a true compass heading** while the bearing's `atan2(-dx, -dy)` carries the cancelling `+180` — correcting either alone mirrors every waypoint and beacon reading front to back.
- @docs/lua-implement-proximity.md — `implementProximity.lua`: what the loader's bucket or forks are about to run into, plus the docking readout, the slam gate, ramp mode, the ramp align teleport and the `RAMPSELF:` line. Ports 4469/4470.
- @docs/lua-binding-learn.md — `bindingLearn.lua`: Learn Bindings Mode. Wraps `core_input_actions.actionToCommands` so a press is announced instead of firing, with the pause/menu category deliberately exempt and a keepalive watchdog that restores the game if beamtel dies. Ports 4479/4480.
- @docs/lua-ui-runtime.md — `bnvdaAutoSpawner.lua` and `ui/ui-vue/mods/bnvda/bnvdaRuntime.js`, the whole UI accessibility layer.

**No teleport in this mod may reset the vehicle.** See @docs/lua-telemetry-protocol.md — `spawn.safeTeleport`'s 8th argument is `resetVehicle` and it **defaults to true**; every call site must pass `nil, nil, nil, false, false`.

### Generated Levels

The Track Builder makes spline ribbons, not worlds. Terrain, water and ground
materials are a **level**, and the World Editor that authors one is ImGui with no
accessibility tree -- so `tools/mapgen/` writes the level from Python instead:
the `.ter` binary directly (the format is decoded and round-trips three shipped
game terrains byte-for-byte), the scene as newline-delimited JSON, the POIs as
the level's own `mainLevel.lua`. See @docs/level-generation.md -- including the
**sound stage**, where the reason a rolling road cannot be terrain is that
`groundmodelName` picks friction, roughness and tyre sound and **never motion**,
so the three rigs are shipped `Type: "Prop"` vehicles SPAWNED by `mainLevel.lua`
(a level scene cannot hold a vehicle at all -- `gridmap_v2` is 7167 `TSStatic` and
zero `BeamNGVehicle`); that the Wheel Roller's rollers are **frictionless troughs**
rather than drums, so it applies essentially **no load** and only the hamster wheel
makes an engine pull; and that all three prop facings differ because every rig is
entered heading north and a prop's `+Y` is not the direction you drive into it --
the tilt ramp's `onramp_*` groups sit at local `+Y`, and the hamster wheel's ramps
are at the ends of its **axle**, so you enter across the drum and turn 90 degrees
inside it; and that **rebuilding the level while the game is running deletes it from the
level list** (`build.py` rmtree's the mod folder, and `core_modmanager.initDB()` plus
`core_levels.onFilesChanged` puts it back without a restart). Also the
three conventions the files cannot reveal -- a SpawnSphere is written facing
**backwards** (`spawn.lua:974` applies a 180 degree flip), scene spawn spheres
are **not** what the UI offers (that list is `info.json`'s `spawnPoints`, and
without it the game silently substitutes one entry called "Default"), and a
terrain berm's steepness is bought with **height**, since the steepest face a
heightmap can express is `height / metres_per_cell` -- and that a lateral
threshold falling mid-cell is resolved by a node on **neither** side of it, which
is how a "one cell" berm ramp came out spanning two nodes and left a climbable
84% outer face behind a wall that looked right in the source. A `WaterBlock` is
likewise a box **centred** on `position` with `scale` as its full extent (so the
surface is `position.z + scale.z / 2`, settled by `getObjBox`/`getWorldBox`
rather than by reading the shipped data, which does not distinguish the cases),
and it renders **nothing** without `rippleTex`/`foamTex`/`depthGradientTex` --
and that a `WaterBlock`'s **waterline is `position.z`**, not the top of
`getWorldBox` (that box is the collision volume and straddles the surface), which
only `obj:inWater` on a parked car can settle. Surfaces follow one rule:
**asphalt is the driving surface and gravel is a margin** laid around what you
should stay inside, so a rumble always means the same thing; the road verge is
computed against the union of the network, or every junction gets a gravel bar
across it. Also the reason
`extensions.reload("core_levels")` must never be used to refresh the level list
(`core_levels.onFilesChanged` is the safe one), and that an in-place
`startFreeroam` reload **does** work (13-16 s, queued behind the unload) -- what
fails is checking whether it did, since a scene probe placed where a periodic
profile is mathematically zero reads identically on both builds and a log tail
from an offset captured after the event reports nothing at all. Also that the two layers of a
`.ter` are **different kinds of grid** -- heights are NODES (inclusive masks),
materials are one byte per CELL (half-open masks) -- so a mask written for one
silently mis-sizes the other, which is how the lane dividers in the suspension
straights came out 2.5 m wide in the middle of the section and 5 m at its edges;
that only **14 of the game's 32 ground models make any tire sound at all**
(`collisiontype` selects it, and SNOW, FOLIAGE and PLASTIC have no branch in
`sounds.lua` -- a surface that drives convincingly and is silent); that
`RUMBLE_STRIP` is a **synthesised** effect rather than geometry, which is the
only reason a flat terrain cell can carry it; and that a heightmap at 2.5 m/cell
**cannot express a washboard at all** (Nyquist is 5 m, four samples per cycle is
10 m), so a wavelength must be an exact multiple of the cell size or it beats
against the node grid, and must not be an integer multiple of the ripple it is
summed with or the two phase-lock and cancel.

### UDP Port Map

| Port | Direction | Purpose |
|------|-----------|---------|
| 4444 | Game→Python | Main telemetry (OutGauge/Extended/MotionSim) |
| 4445 | Game→Python | Vehicle scanner data + AI status |
| 4446 | Python→Game | Diagnostic dump commands |
| 4447 | Game→Python | Diagnostic dump responses |
| 4448 | Python→Game | Scanner commands (ON/OFF/NEXT/PREV/ALIGN/DAMAGE/GEAR) |
| 4449 | Python→Game | AI commands (MODE/AGGR/SPEED/AVOID/LANE/STATUS) |
| 4450 | Game→Python | Camera info data |
| 4451 | Python→Game | Camera info commands (ON/OFF/DIAG) |
| 4452 | Game→Python | Obstacle detector data |
| 4453 | Python→Game | Obstacle detector commands (ON/OFF) |
| 4454 | Game→Python | Node grabber data (node hover, snap coords) |
| 4455 | Python→Game | Node grabber commands (ON/OFF/SCAN_ON/SCAN_OFF/SNAP) |
| 4456 | Game→Python | Clickspot data (trigger list, hover, snap) |
| 4457 | Python→Game | Clickspot commands (ON/OFF/SNAP/EXEC) |
| 4458 | Game→Python | Vehicle slot data (vehicleSlots.lua) |
| 4459 | Python→Game | Vehicle slot commands (vehicleSlots.lua) |
| 4460 | Game→Python | Vehicle spawner data (vehicleSpawnerAccessible.lua) |
| 4461 | Python→Game | Vehicle spawner commands (vehicleSpawnerAccessible.lua) |
| 4462 | Game→Python | Road detector status (ON_ROAD/OFF_ROAD/DORMANT) |
| 4463 | Python→Game | Road detector commands (ON/OFF) |
| 4464 | Python→Game | UI visibility toggle (HIDE/SHOW/TOGGLE) |
| 4465 | Python→Game | Accessible console commands (EXEC/CTXLIST/LOGON/LOGOFF) |
| 4466 | Game→Python | Accessible console responses + log stream |
| 4467 | Game→Python | Vehicle-specific binding list (vehicleBindings.lua) |
| 4468 | Python→Game | Vehicle binding commands (REQUEST/REBUILD/EXEC) |
| 4469 | Game→Python | Implement proximity + docking/ramp alignment events (implementProximity.lua) |
| 4470 | Python→Game | Implement proximity commands (ON/OFF/REBUILD/DOCK_ON/DOCK_OFF/BAND*/RAMPALIGN) |
| 4471 | Game→Python | Terrain scan snapshots (terrainScanner.lua) |
| 4472 | Python→Game | Terrain scan command (SCAN) |
| 4473 | Game→Python | Cannon shot outcomes (cannonShot.lua) |
| 4474 | Game→Python | Environment rows (environmentAccessible.lua) |
| 4475 | Python→Game | Environment commands (REQUEST/SET/RESTORE) |
| 4476 | Game→Python | Trailer articulation angle (trailerAngle.lua) |
| 4477 | Game→Python | Vehicle information rows (vehicleInfo.lua) |
| 4478 | Python→Game | Vehicle information commands (INFO_SELECTOR/INFO:) |
| 4479 | Game→Python | Learn-mode press events (bindingLearn.lua) |
| 4480 | Python→Game | Learn-mode commands (LEARN_ON/LEARN_OFF/KEEPALIVE/DIAG) |
| 4481 | Agent→Python | MCP automation server (HTTP, loopback, off by default) |
| 4482 | Game→Python | Navigation route destination (routeBeacon.lua) |
| 4579 | Game→Python | UI toast messages |
| 8765 | WebSocket | NVDA/UI bridge |

### Data Flow

```
BeamNG.drive  --UDP:4444-->  beamtel.py  ---->  audio.py (sounddevice output)
              --UDP:4579-->  (UI toasts)  ---->  speech.py / Prism (speech)
              --UDP:4445-->  (scanner)
              --UDP:4450-->  (camera)
              --UDP:4452-->  (obstacles)
              --UDP:4454-->  (node grabber)
              --UDP:4456-->  (clickspots)
Browser UI    --WS:8765--->  nvda_ws_speaker.py ---> speech
Python        --UDP:4448-->  vehicleScanner.lua
              --UDP:4449-->  beamtelAI.lua
              --UDP:4451-->  cameraInfo.lua
              --UDP:4453-->  obstacleDetector.lua
              --UDP:4455-->  nodeGrabberAccessible.lua
              --UDP:4457-->  clickspotAccessible.lua
```

### Threading Model

- **Main thread**: UDP telemetry loop (blocking socket with timeout)
- **Audio thread**: `sounddevice.OutputStream` callback (high-priority)
- **Device watcher thread**: Monitors default audio device changes
- **Daemon threads**: UI listener (port 4579), scanner listener (port 4445), keyboard hook, NVDA WebSocket (asyncio event loop)

All shared state is protected by `state_lock` (threading.Lock). The audio callback minimizes lock holding by reading state snapshots.

### Telemetry Protocols

- **OutGauge** (188 bytes, format `<I4sHBBfffffffIIfff16s16si`): Standard speed, RPM, temps, gear (byte: 0=R, 1=N, 2+=1st...), dashboard light bitmask.
- **Extended** (196 bytes, format `<H4sBx9fII36f`): Adds tire/brake temps, clutch temp, G-forces, gear as string ("P","D","R","N","S3","M5"...), and the loader implement block. Field order in `getStructDefinition()` **is** the wire format, and `beamtel.py` unpacks the float tail positionally — a field added on one side and not the other silently shifts everything after it. The previous 164-byte layout (`28f`, pre-implement) is still decoded as `EXT_FORMAT_V1` and padded, because `bng_mod/` is a live junction into the game install: without that, a mod half newer or older than the Python half would fail the length check and take **all** extended telemetry down — no speed, no gear, no warning lights — with no error anywhere. Add new fields at the end, bump the count, and add a new `_V1`-style fallback rather than widening the old one.
- **MotionSim** (magic "BNG1", format `<4s21f`): 3D position and orientation (yaw, pitch, roll).

### Configuration

Config file: `%LOCALAPPDATA%/beamtel/beamtel_config.json` (shared by beamtel.py and configurator.py). Defaults are hardcoded in `DEFAULT_CONFIG` dicts; user values are merged on top. Config corruption triggers auto-backup (.bak) and reset to defaults.

Most settings are consumed on the Python side. `ui_nav_hold_suppression` is the exception: it is enforced in the mod's JS runtime, so beamtel pushes it over the bridge as a `settings` message. The runtime pulls it with a `settings_request` on every transport activation rather than relying on the push alone — at startup the config-change broadcast fires before the Lua bridge has connected and would be dropped.

## Key Patterns

- **Audio callback pattern**: Check triggered events → read state under lock → generate samples → mix all sources → clip to [-0.999, 0.999] → write stereo interleaved output.
- **Virtual browser ownership**: `open_virtual_browser` hooks the keyboard arrows exclusively, and while one is open it also takes the six controller accessibility actions -- `navigate_accessibility_menu` hands them to `_drive_virtual_browser` instead of the Status/Functions/Click spots screens, with next/previous-menu as the way out. Without that a pad could open a browser from the Functions menu and have no way to move through it. It is the rule `on_status_arrow_press` already applies on the keyboard, extended to the pad.
- **UI page text**: `bnvdaRuntime.js` answers a `page_text` request with the current readable screen as lines (today the AngularJS mod repository / automation details pages, whose spec table and description body carry nothing focusable), and pushes a `screen_context` latch so the controller Functions menu can hide the entry elsewhere. See @docs/lua-ui-runtime.md -- nothing is read automatically, and `cleanText` must never touch body text.
- **Keyboard command lifecycle**: F9 → install hooks + 4-second timeout → process key → unhook. Modifier tracking (Ctrl/Shift/Alt) handled in `_on_next_key_press`.
- **Fallback strategy**: configured speech backend → Prism's highest-priority available one, re-acquired on failure; preferred audio device → system default; air pressure controller → electrics scan for vehicles without dedicated tanks.
- **Nuitka build config**: Embedded as `# nuitka-project:` pragmas at the top of `beamtel.py` and `configurator.py`. Both build as `--onefile`. beamtel requires admin (`--windows-uac-admin`) and, like configurator, has **no console** (`--windows-console-mode=disable`) -- so it has no usable standard handles either, which is why `updater.apply_and_restart` hands its detached helper explicit DEVNULL handles rather than letting cmd allocate a console of its own.
- **Cross-VM Lua communication**: Vehicle VM ↔ Game Engine VM via `vehicle:queueLuaCommand()` and `obj:queueGameEngineLua()`. All UDP socket I/O uses LuaSocket.

## Lua Constraints

BeamNG.drive retail uses **LuaJIT (Lua 5.1)**:
- `string.pack`/`string.unpack` are NOT available. Use `string.format` text CSV for UDP packets.
- `castRayStatic(origin, dir, maxDist)` — raycasts against static geometry only. Vehicles use soft-body physics and are invisible to raycasts; use `be:getObject(i)` enumeration instead.
- `core_terrain.getTerrainHeight(pos)` — terrain height queries at arbitrary world positions.
- Lua extensions export a module table `M` with hooks: `onExtensionLoaded()`, `onWorldReadyState(state)`, `onUpdate(dtReal, dtSim, dtRaw)`.

## Dependencies

numpy, sounddevice, wxpython, aiohttp, keyboard, prismatoid (+cffi), zstandard. Prism's native library ships inside the prismatoid wheel (`prism/_native/`), so no DLLs are staged in the project root. Packaging it needs three explicit `--include-data-files` entries (see `prism_nuitka_args()` in `build.py`): `prism.dll` and `_prism_cffi.pyd` at `prism/_native/`, because `--include-data-dir` silently drops binaries and Nuitka cannot follow the `__path__` splicing in `prism/_native.py`; plus `python3.dll`, the stable-ABI forwarder that the abi3 extension links against and that Nuitka does not bundle. Build-time only: h5py (used by `bake_hrtf.py` to extract the SOFA file into `hrtf_kemar_horizontal.npz`; not bundled into the executable).
