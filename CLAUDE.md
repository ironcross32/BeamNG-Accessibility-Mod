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
2. **Lua mod** (`bng_mod/`) — A directory junction to `%LOCALAPPDATA%/BeamNG/BeamNG.drive/current/mods/unpacked/bng_mod/`. Edits to `bng_mod/` are live in the game. Claude Code resolves the junction before checking permissions, so that real folder must be in `permissions.additionalDirectories` (`.claude/settings.local.json`) or every mod edit prompts.

### Component Docs (read on demand)

The per-file detail, and the reasoning behind every decision that is not obvious from the code, lives in `docs/`. These files are deliberately **not** imported into this one: together they are several times the instruction budget. **Before editing a file in the table, Read its doc first** -- most of what they record is a trap that is invisible in the source and was found from the driver's seat. Keep them out of `@` imports, and when a doc grows past roughly 25k characters, split it by file rather than trimming the reasoning.

| Doc | Covers |
|---|---|
| `docs/python-components.md` | Every Python module: `beamtel.py`, `audio.py`, `wheel_slip.py`, `gear_report.py`, `hrtf.py`, `configurator.py`, `speech.py`, `ai_describer.py`, `secretstore.py`, `nvda_ws_speaker.py`, `mcp_server.py`, `updater.py`, `bnh_logger.py`, `diagnostic/` |
| `docs/lua-telemetry-protocol.md` | `lua/vehicle/protocols/796F6C6F313035.lua`: the 60 Hz extended struct on 4444, `actualSteering`, the ramp hydraulics push, the loader implement block, and the no-reset teleport rule |
| `docs/lua-vehicle-geometry.md` | `vehicleGeometry.lua` + vehicle `beamtelGeometryWatch.lua`: per-vehicle extents, hull cids, contact bands, occupancy histogram, dropping broken-off parts |
| `docs/lua-ramp-geometry.md` | `rampGeometry.lua`: the drive-in mouth of a ramp-equipped machine, declared mouths, `floorTrusted` |
| `docs/lua-vehicle-scanner.md` | `vehicleScanner.lua`: nearest vehicle, target cycling, coupler matching, the align teleport. Ports 4445/4448 |
| `docs/lua-implement-proximity.md` | `implementProximity.lua`: proximity, docking readout, slam gate, ramp mode, `RAMPALIGN`, `RAMPSELF:`. Ports 4469/4470 |
| `docs/lua-binding-learn.md` | `bindingLearn.lua`: Learn Bindings Mode and its keepalive watchdog. Ports 4479/4480 |
| `docs/lua-ui-runtime.md` | `bnvdaAutoSpawner.lua` and `ui/ui-vue/mods/bnvda/bnvdaRuntime.js`, the whole UI accessibility layer: overview, poll/event model, translation, `cleanText`, text-input focus |
| `docs/lua-ui-runtime-navigation.md` | `bnvdaRuntime.js`: held-navigation coalescing (tap vs hold, the nav burst) |
| `docs/lua-ui-runtime-radial.md` | `bnvdaRuntime.js`: radial menu speech (stick as pointer, release handling, submenu path) |
| `docs/lua-ui-runtime-bindings.md` | `bnvdaRuntime.js`: binding readouts, hints, the edit-binding popup |
| `docs/lua-ui-runtime-tabs.md` | `bnvdaRuntime.js`: tab, sub-tab and breadcrumb announcements |
| `docs/lua-ui-runtime-mod-details.md` | `bnvdaRuntime.js`: `page_text` for the mod repository / automation details pages |
| `docs/lua-ui-runtime-tuning.md` | Tuning slider debounce and apply guard, **parked** in `parked_tuning_hold_workaround.js` (0.40 fixed the stock race; kept for reference) |
| `docs/lua-terrain-scanner.md` | `terrainScanner.lua`: the terrain scan and nearest-POI lookup. Ports 4471/4472 |
| `docs/lua-cannon-shot.md` | `cannonShot.lua`: shot outcomes. Port 4473 |
| `docs/lua-environment.md` | `environmentAccessible.lua`: ambient temperature browser. Ports 4474/4475 |
| `docs/lua-trailer-angle.md` | `trailerAngle.lua`: trailer articulation tone. Port 4476 |
| `docs/lua-wheel-report.md` | `wheelReport.lua` + `wheel_report.py`: every wheel's pressure and brake temperature on the vehicle and anything coupled, labelled from geometry. Port 4485 |
| `docs/lua-route-beacon.md` | `routeBeacon.lua` and `route_beacon.relative_bearing`. Port 4482 |
| `docs/lua-audio-debug.md` | `audioDebugAccessible.lua` + vehicle `beamtelAudioDebug.lua`: the editor's Window > Audio tools (engine sound parameters, sound library). Ports 4483/4484 |
| `docs/lua-vehicle-info.md` | `vehicleInfo.lua`: vehicle selector specifications. Ports 4477/4478 |
| `docs/lua-drift.md` | `driftAccessible.lua` + `drift_speech.py`: speaking the stock drift display, its hooks and contexts, the speed/compass quiet window |
| `docs/lua-extensions.md` | Everything smaller: `accessibilityInput.lua`, `beamtelAI.lua`, `obstacleDetector.lua`, `roadDetector.lua`, `hillClimbChallenge.lua`, `cameraInfo.lua`, `nodeGrabberAccessible.lua`, `clickspotAccessible.lua`, `uiToggle.lua`, `consoleAccessible.lua`, `vehicleBindings.lua` |
| `docs/level-generation.md` | `tools/mapgen/`: the `.ter` format, ground models and which make sound, spawn/POI conventions, level discovery and reload, node-vs-cell grids, `WaterBlock` |
| `docs/level-proving-grounds.md` | The Proving Grounds layout (`mapdef.py`): surfaces, climb, tunnel, sound stage, suspension straights |
| `docs/level-ice-pool.md` | The Ice Pool level (`icepool.py`) |

The Lua mod's entry point is `scripts/bng_screenreader_mod/modScript.lua`, which loads all GE extensions.

### Rules that apply everywhere

These cut across files, so they are stated here rather than left to a doc that may not have been opened.

- **No teleport in this mod may reset the vehicle.** `spawn.safeTeleport`'s 8th argument is `resetVehicle` and it **defaults to true**; every call site must pass `nil, nil, nil, false, false`. See `docs/lua-telemetry-protocol.md`.
- **Positive is LEFT, and the lateral vector is `up:cross(fwd)`** in every file that builds one. It must be built from the un-negated forward, including in reverse. Sims grep for it.
- **BeamTel's heading is 180 degrees off a true compass heading**, and `route_beacon.relative_bearing`'s `atan2(-dx, -dy)` carries the cancelling offset. Use that helper; correcting either half alone mirrors every bearing front to back.
- **A cross-VM chunk (`queueLuaCommand` long string) may contain no percent sign and no doubled close-square-bracket**, in code or comments. A syntax error there fails silently in another VM.
- **Every listening extension checks `setsockname`'s return, has `M.onExtensionUnloaded`, and calls `retryCmdBind(dtReal)` first in `onUpdate`.** `vehicle_geometry_sim.lua` scenario 12 enforces it.
- **Wire tails are optional and appended, never inserted.** `bng_mod/` is a live junction, so the two halves genuinely go out of step.
- **"Unknown" is never sent as `0`** where zero is a real reading (level ramp, level ground, touching both walls). Use the documented sentinel.
- **Run the matching `diagnostic/*_sim.*` after touching an area**; each doc names its sim.

### Generated Levels

The World Editor is ImGui with no accessibility tree, so `tools/mapgen/` writes levels from Python: the `.ter` binary directly, the scene as newline-delimited JSON, the POIs as the level's own `mainLevel.lua`. Read `docs/level-generation.md` before touching it. The traps most likely to bite without warning:

- Rebuilding while the game is running **removes the level from the list**; `core_modmanager.initDB()` plus `core_levels.onFilesChanged` restores it. Never `extensions.reload("core_levels")` or `extensions.reload("mainLevel")`.
- Heights are a **node** grid (inclusive masks); materials are one byte per **cell** (half-open masks).
- A SpawnSphere is written facing **backwards**, and the UI's spawn list is `info.json`'s `spawnPoints`, not the scene.
- A `WaterBlock`'s waterline is `position.z`, and it renders nothing without its three textures.
- A level scene cannot hold a vehicle; the sound-stage rigs are props spawned by `mainLevel.lua`.

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
| 4483 | Game→Python | Audio debug rows: engine sounds, sound library (audioDebugAccessible.lua) |
| 4484 | Python→Game | Audio debug commands (REQUEST/SELECT/SET/RESTORE/COPY/SFX_*) |
| 4485 | Game→Python | Every wheel's position, pressure and brake temperature, vehicle and trailers (wheelReport.lua) |
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
- **Extended** (256 bytes, format `<H4sBx9fII42fI8f`): Adds tire/brake temps, clutch temp, G-forces, gear as string ("P","D","R","N","S3","M5"...), the loader implement block, centred wheels and the presence mask, wheel extremes, electric drive power/torque/range, and trip/odometer/model-lifetime distance. Field order in `getStructDefinition()` **is** the wire format, and `beamtel.py` unpacks the float tail positionally — a field added on one side and not the other silently shifts everything after it. The previous 164-byte layout (`28f`, pre-implement) is still decoded as `EXT_FORMAT_V1` and padded, because `bng_mod/` is a live junction into the game install: without that, a mod half newer or older than the Python half would fail the length check and take **all** extended telemetry down — no speed, no gear, no warning lights — with no error anywhere. Add new fields at the end, bump the count, and add a new `_V1`-style fallback rather than widening the old one.
- **MotionSim** (magic "BNG1", format `<4s21f`): 3D position and orientation (yaw, pitch, roll).

### Configuration

Config file: `%LOCALAPPDATA%/beamtel/beamtel_config.json` (shared by beamtel.py and configurator.py). Defaults are hardcoded in `DEFAULT_CONFIG` dicts; user values are merged on top. Config corruption triggers auto-backup (.bak) and reset to defaults.

Most settings are consumed on the Python side. `ui_nav_hold_suppression` is the exception: it is enforced in the mod's JS runtime, so beamtel pushes it over the bridge as a `settings` message. The runtime pulls it with a `settings_request` on every transport activation rather than relying on the push alone — at startup the config-change broadcast fires before the Lua bridge has connected and would be dropped.

## Key Patterns

- **Audio callback pattern**: Check triggered events → read state under lock → generate samples → mix all sources → clip to [-0.999, 0.999] → write stereo interleaved output.
- **Virtual browser ownership**: `open_virtual_browser` hooks the keyboard arrows exclusively, and while one is open it also takes the six controller accessibility actions -- `navigate_accessibility_menu` hands them to `_drive_virtual_browser` instead of the Status/Functions/Click spots screens, with next/previous-menu as the way out. Without that a pad could open a browser from the Functions menu and have no way to move through it. It is the rule `on_status_arrow_press` already applies on the keyboard, extended to the pad.
- **UI page text**: `bnvdaRuntime.js` answers a `page_text` request with the current readable screen as lines (today the AngularJS mod repository / automation details pages, whose spec table and description body carry nothing focusable), and pushes a `screen_context` latch so the controller Functions menu can hide the entry elsewhere. See `docs/lua-ui-runtime-mod-details.md` -- nothing is read automatically, and `cleanText` must never touch body text.
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
