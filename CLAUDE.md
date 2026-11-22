# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

BEAM is a screen reader mod for BeamNG.drive — an accessibility tool that converts real-time vehicle telemetry into audio cues, spatial compass clicks, and speech output for blind and visually impaired players. It runs on Windows 10+. It does not need administrator rights: the keyboard hooks see and suppress keys going to the game as long as the game is not elevated either (a Steam launch is not).

## Build & Run Commands

The application is the Rust workspace in `rust/`; run these from there (PowerShell). `rust/README.md` has the details: prerequisites, the opt-in `uac-admin` feature, fixtures, paths.

```powershell
cargo build --workspace                                  # debug, everything
cargo run -p beam-app                                    # open the app; debug keeps a console
cargo build --release -p beam-app                        # the shipped beamtel.exe (npx tauri build is equivalent)
cargo test --workspace                                   # unit, golden-fixture and sim tests
cargo clippy --workspace --all-targets
cargo fmt --all
```

Run beamtel from a release build; a debug build's audio crackles. There is no release pipeline yet: `rust/xtask` is meant to package the exe and the mod and has no commands.

The Python application this was ported from has been removed (it is in git history before "Remove the Python application"). Code comments and docs that cite a Python name, like `beamtel.py:5066` or `_dock_phrase`, refer to that history; the Rust keeps the name without the leading underscore, and `rust_plans/engine-map.md` maps every old `beamtel.py` name to its module. The only Python left is the level generators in `tools/` (`uv run --project tools python tools/...`).

## Architecture

### Two-Part System

The project has two halves that communicate via UDP:

1. **The app** (`rust/`, binary `beamtel.exe`) — Receives telemetry, generates audio, handles speech output. The docs call this side "beamtel"
2. **Lua mod** (`bng_mod/`) — A directory junction to `%LOCALAPPDATA%/BeamNG/BeamNG.drive/current/mods/unpacked/bng_mod/`. Edits to `bng_mod/` are live in the game. Claude Code resolves the junction before checking permissions, so that real folder must be in `permissions.additionalDirectories` (`.claude/settings.local.json`) or every mod edit prompts.

### Component Docs (read on demand)

The per-file detail, and the reasoning behind every decision that is not obvious from the code, lives in `docs/`. These files are deliberately **not** imported into this one: together they are several times the instruction budget. **Before editing a file in the table, Read its doc first** -- most of what they record is a trap that is invisible in the source and was found from the driver's seat. Keep them out of `@` imports, and when a doc grows past roughly 25k characters, split it by file rather than trimming the reasoning.

| Doc | Covers |
|---|---|
| `docs/app-engine.md` | `beam-engine`: telemetry, listeners, F9/F10 layers, status menu, hill-climb and road diagnostics capture; plus the small `beam-reports` modules (wheel report, camera line-up, audio debug, drift speech, fuel stations) |
| `docs/app-audio.md` | `beam-audio`: the synthesis engine and every voice (compass, scanner, dock, implement, ramp, terrain scan, obstacle, junction) |
| `docs/app-hrtf.md` | `beam-audio` `hrtf.rs` + `beam-base` `hrtf_sets.rs` (datasets, elevation, equalisation), `gear_report.rs`, `wheel_slip.rs` |
| `docs/app-services.md` | Settings panel, `beam-speech`, AI describer, secret store, UI bridge (8765/8766), updater, logger |
| `docs/app-mcp.md` | `beam-engine` `mcp_server/`: the MCP automation server on 4481 |
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
| `docs/lua-wheel-report.md` | `wheelReport.lua` + `beam-reports` `wheel_report.rs`: every wheel's pressure and brake temperature on the vehicle and anything coupled, labelled from geometry. Port 4485 |
| `docs/road-privacy-and-gates.md` | `roadDetector.lua`'s private roads and `roadGates.lua`: what the game's "gated" flag really means (not a gate), gate meshes, the private-entry cue, relative legality |
| `docs/lua-route-beacon.md` | `routeBeacon.lua` and `beam_base::route_beacon::relative_bearing`. Port 4482 |
| `docs/lua-fuel-stations.md` | `fuelStations.lua` + vehicle `beamtelFuelPark.lua` + `beam-reports` `fuel_stations.rs`: the map's pumps and chargers, routing to one, measured pump placements, the parking controller. Ports 4486/4488 |
| `docs/driving-assistant-route.md` | `drivingAssistant.lua` following a big-map route, `beam-road` `assistant_route.rs`: what the game clears on arrival, the forward-only search, callouts, off-road destinations |
| `docs/camera-lineup.md` | `cameraLineup.lua` + `beam-reports` `camera_lineup.rs`: the free-camera line-up guide (F9 Alt+G), head-on and crossing targets, the FM tone. Port 4489 |
| `docs/lua-audio-debug.md` | `audioDebugAccessible.lua` + vehicle `beamtelAudioDebug.lua`: the editor's Window > Audio tools (engine sound parameters, sound library). Ports 4483/4484 |
| `docs/lua-vehicle-info.md` | `vehicleInfo.lua`: vehicle selector specifications. Ports 4477/4478 |
| `docs/lua-drift.md` | `driftAccessible.lua` + `beam-reports` `drift_speech.rs`: speaking the stock drift display, its hooks and contexts, the speed/compass quiet window |
| `docs/lua-extensions.md` | Everything smaller: `accessibilityInput.lua`, `beamtelAI.lua`, `obstacleDetector.lua`, `roadDetector.lua`, `hillClimbChallenge.lua`, `cameraInfo.lua`, `nodeGrabberAccessible.lua`, `clickspotAccessible.lua`, `uiToggle.lua`, `consoleAccessible.lua`, `vehicleBindings.lua` |
| `docs/level-generation.md` | `tools/mapgen/`: the `.ter` format, ground models and which make sound, spawn/POI conventions, level discovery and reload, node-vs-cell grids, `WaterBlock` |
| `docs/level-proving-grounds.md` | The Proving Grounds layout (`mapdef.py`): surfaces, climb, tunnel, sound stage, suspension straights |
| `docs/level-ice-pool.md` | The Ice Pool level (`icepool.py`) |

The Lua mod's entry point is `scripts/bng_screenreader_mod/modScript.lua`, which loads all GE extensions.

### Rules that apply everywhere

These cut across files, so they are stated here rather than left to a doc that may not have been opened.

- **No teleport in this mod may reset the vehicle.** `spawn.safeTeleport`'s 8th argument is `resetVehicle` and it **defaults to true**; every call site must pass `nil, nil, nil, false, false`. See `docs/lua-telemetry-protocol.md`.
- **Positive is LEFT, and the lateral vector is `up:cross(fwd)`** in every file that builds one. It must be built from the un-negated forward, including in reverse. Sims grep for it.
- **BeamTel's heading is 180 degrees off a true compass heading**, and `beam_base::route_beacon::relative_bearing`'s `atan2(-dx, -dy)` carries the cancelling offset. Use that helper; correcting either half alone mirrors every bearing front to back.
- **A cross-VM chunk (`queueLuaCommand` long string) may contain no percent sign and no doubled close-square-bracket**, in code or comments. A syntax error there fails silently in another VM.
- **Every listening extension checks `setsockname`'s return, has `M.onExtensionUnloaded`, and calls `retryCmdBind(dtReal)` first in `onUpdate`.** `vehicle_geometry_sim.lua` scenario 12 enforces it.
- **Wire tails are optional and appended, never inserted.** `bng_mod/` is a live junction, so the two halves genuinely go out of step.
- **"Unknown" is never sent as `0`** where zero is a real reading (level ramp, level ground, touching both walls). Use the documented sentinel.
- **Run the matching sim after touching an area**; each doc names its sim. Lua and JS sims are `diagnostic/*_sim.lua` / `*_sim.js` (the Lua ones run under the game's `Bin64\console.x64.exe`); the app's sims are Rust tests of the same name, `cargo test -p <crate> --test <name>_sim`.

### Generated Levels

The World Editor is ImGui with no accessibility tree, so `tools/mapgen/` writes levels from Python (the one Python left in the repo): the `.ter` binary directly, the scene as newline-delimited JSON, the POIs as the level's own `mainLevel.lua`. Read `docs/level-generation.md` before touching it. The traps most likely to bite without warning:

- Rebuilding while the game is running **removes the level from the list**; `core_modmanager.initDB()` plus `core_levels.onFilesChanged` restores it. Never `extensions.reload("core_levels")` or `extensions.reload("mainLevel")`.
- Heights are a **node** grid (inclusive masks); materials are one byte per **cell** (half-open masks).
- A SpawnSphere is written facing **backwards**, and the UI's spawn list is `info.json`'s `spawnPoints`, not the scene.
- A `WaterBlock`'s waterline is `position.z`, and it renders nothing without its three textures.
- A level scene cannot hold a vehicle; the sound-stage rigs are props spawned by `mainLevel.lua`.

### UDP Port Map

| Port | Direction | Purpose |
|------|-----------|---------|
| 4444 | Game→beamtel | Main telemetry (OutGauge/Extended/MotionSim) |
| 4445 | Game→beamtel | Vehicle scanner data + AI status |
| 4446 | beamtel→Game | Diagnostic dump commands |
| 4447 | Game→beamtel | Diagnostic dump responses |
| 4448 | beamtel→Game | Scanner commands (ON/OFF/NEXT/PREV/ALIGN/DAMAGE/INDUCTION/GEAR) |
| 4449 | beamtel→Game | AI commands (MODE/AGGR/SPEED/AVOID/LANE/STATUS) |
| 4450 | Game→beamtel | Camera info data |
| 4451 | beamtel→Game | Camera info commands (ON/OFF/DIAG) |
| 4452 | Game→beamtel | Obstacle detector data |
| 4453 | beamtel→Game | Obstacle detector commands (ON/OFF) |
| 4454 | Game→beamtel | Node grabber data (node hover, snap coords) |
| 4455 | beamtel→Game | Node grabber commands (ON/OFF/SCAN_ON/SCAN_OFF/SNAP) |
| 4456 | Game→beamtel | Clickspot data (trigger list, hover, snap) |
| 4457 | beamtel→Game | Clickspot commands (ON/OFF/SNAP/EXEC) |
| 4458 | Game→beamtel | Vehicle slot data (vehicleSlots.lua) |
| 4459 | beamtel→Game | Vehicle slot commands (vehicleSlots.lua) |
| 4460 | Game→beamtel | Vehicle spawner data (vehicleSpawnerAccessible.lua) |
| 4461 | beamtel→Game | Vehicle spawner commands (vehicleSpawnerAccessible.lua) |
| 4462 | Game→beamtel | Road detector status (ON_ROAD/OFF_ROAD/DORMANT) |
| 4463 | beamtel→Game | Road detector commands (ON/OFF) |
| 4464 | beamtel→Game | UI visibility toggle (HIDE/SHOW/TOGGLE) |
| 4465 | beamtel→Game | Accessible console commands (EXEC/CTXLIST/LOGON/LOGOFF) |
| 4466 | Game→beamtel | Accessible console responses + log stream |
| 4467 | Game→beamtel | Vehicle-specific binding list (vehicleBindings.lua) |
| 4468 | beamtel→Game | Vehicle binding commands (REQUEST/REBUILD/EXEC) |
| 4469 | Game→beamtel | Implement proximity + docking/ramp alignment events (implementProximity.lua) |
| 4470 | beamtel→Game | Implement proximity commands (ON/OFF/REBUILD/DOCK_ON/DOCK_OFF/BAND*/RAMPALIGN) |
| 4471 | Game→beamtel | Terrain scan snapshots (terrainScanner.lua) |
| 4472 | beamtel→Game | Terrain scan command (SCAN) |
| 4473 | Game→beamtel | Cannon shot outcomes (cannonShot.lua) |
| 4474 | Game→beamtel | Environment rows (environmentAccessible.lua) |
| 4475 | beamtel→Game | Environment commands (REQUEST/SET/RESTORE) |
| 4476 | Game→beamtel | Trailer articulation angle (trailerAngle.lua) |
| 4477 | Game→beamtel | Vehicle information rows (vehicleInfo.lua) |
| 4478 | beamtel→Game | Vehicle information commands (INFO_SELECTOR/INFO:) |
| 4479 | Game→beamtel | Learn-mode press events (bindingLearn.lua) |
| 4480 | beamtel→Game | Learn-mode commands (LEARN_ON/LEARN_OFF/KEEPALIVE/DIAG) |
| 4481 | Agent→beamtel | MCP automation server (HTTP, loopback, off by default) |
| 4482 | Game→beamtel | Navigation route destination (routeBeacon.lua) |
| 4483 | Game→beamtel | Audio debug rows: engine sounds, sound library (audioDebugAccessible.lua) |
| 4484 | beamtel→Game | Audio debug commands (REQUEST/SELECT/SET/RESTORE/COPY/SFX_*) |
| 4485 | Game→beamtel | Every wheel's position, pressure and brake temperature, vehicle and trailers (wheelReport.lua) |
| 4486 | Game→beamtel | Fuel station rows and parking events (fuelStations.lua) |
| 4488 | beamtel→Game | Fuel station commands (LIST/TARGET/PARK/PARK_CANCEL/CANCEL/STATUS) |
| 4489 | Game→beamtel | Free-camera line-up geometry (cameraLineup.lua) |
| 4579 | Game→beamtel | UI toast messages |
| 8765 | WebSocket | NVDA/UI bridge |

### Data Flow

```
BeamNG.drive  --UDP:4444-->  beam-engine telemetry.rs  ---->  beam-audio (cpal output)
              --UDP:4579-->  (UI toasts)               ---->  beam-speech (Prism)
              --UDP:4445-->  listeners/scanner.rs
              --UDP:4450-->  listeners/camera.rs
              --UDP:4452-->  listeners/obstacle.rs
              --UDP:4454-->  listeners/nodegrab.rs
              --UDP:4456-->  listeners/clickspot.rs
Browser UI    --WS:8765--->  beam-engine ui_bridge.rs ---> speech
beamtel       --UDP:4448-->  vehicleScanner.lua
              --UDP:4449-->  beamtelAI.lua
              --UDP:4451-->  cameraInfo.lua
              --UDP:4453-->  obstacleDetector.lua
              --UDP:4455-->  nodeGrabberAccessible.lua
              --UDP:4457-->  clickspotAccessible.lua
```

Every listener lives in `rust/crates/beam-engine/src/listeners/`; ports are constants in `net.rs`.

### Threading Model

- **Telemetry thread**: the UDP loop in `telemetry.rs` (blocking socket with timeout).
- **Audio**: the `cpal` output stream's callback (`beam-audio` `callback/`), high priority; a device watcher thread in `device.rs` follows default-device changes.
- **Listener and worker threads**: one per UDP listener, the UI bridge (tokio/axum), started through `timer::spawn_supervised`, which logs a panic or unexpected exit and restarts the loop after a backoff.
- **Keyboard**: a hotkey thread owns F9/F10/F11 (`RegisterHotKey`, only while the game is foreground). The low-level hook gets a thread of its own only while a layer or mode has keys hooked, and must never stay installed otherwise: a resident hook collides with NVDA's (`docs/app-engine.md`).
- **The window**: Tauri's main thread (`src-tauri`).

Shared engine state is `state::state()`, a `parking_lot` mutex guard over `state::State` (grouped by feature: `st.tel`, `st.scanner`, ...). It is **not reentrant**: calling `state()` while a guard is alive on the same thread deadlocks, so take it once and pass `&mut State` down. `say()` never takes it. `rust_plans/engine-map.md` section 1 has the lock order and what lives outside `State`. The audio callback reads state snapshots and holds the controller's lock as briefly as possible.

### Telemetry Protocols

- **OutGauge** (188 bytes, format `<I4sHBBfffffffIIfff16s16si`): Standard speed, RPM, temps, gear (byte: 0=R, 1=N, 2+=1st...), dashboard light bitmask.
- **Extended** (256 bytes, format `<H4sBx9fII42fI8f`): Adds tire/brake temps, clutch temp, G-forces, gear as string ("P","D","R","N","S3","M5"...), the loader implement block, centred wheels and the presence mask, wheel extremes, electric drive power/torque/range, and trip/odometer/model-lifetime distance. Field order in `getStructDefinition()` **is** the wire format, and `beam-engine`'s `telemetry.rs` unpacks the float tail positionally — a field added on one side and not the other silently shifts everything after it. Every earlier layout is still decoded (`EXT_FORMAT_V1`..`V6`) and padded, because `bng_mod/` is a live junction into the game install: without that, a mod half newer or older than the app would fail the length check and take **all** extended telemetry down — no speed, no gear, no warning lights — with no error anywhere. Add new fields at the end, bump the count, and add a new `_V1`-style fallback rather than widening the old one.
- **MotionSim** (magic "BNG1", format `<4s21f`): 3D position and orientation (yaw, pitch, roll).

### Configuration

Config file: `%LOCALAPPDATA%/beamtel/beamtel_config.json`. Defaults, coercion and clamping are `beam-base` `config.rs` (`DEFAULT_CONFIG`); user values are merged on top, and the settings panel (`src-tauri` `panel.rs` + `rust/ui/`) edits the same file. Config corruption triggers auto-backup (.bak) and reset to defaults.

Most settings are consumed in the app. `ui_nav_hold_suppression` is the exception: it is enforced in the mod's JS runtime, so beamtel pushes it over the bridge as a `settings` message. The runtime pulls it with a `settings_request` on every transport activation rather than relying on the push alone — at startup the config-change broadcast fires before the Lua bridge has connected and would be dropped.

## Key Patterns

- **Audio callback pattern**: Check triggered events → read state under lock → generate samples → mix all sources → clip to [-0.999, 0.999] → write stereo interleaved output.
- **Virtual browser ownership**: `open_virtual_browser` (`virtual_browser.rs`) hooks the keyboard arrows exclusively, and while one is open it also takes the six controller accessibility actions -- `menus::navigate_accessibility_menu` hands them to `drive_virtual_browser` instead of the Status/Functions/Click spots screens, with next/previous-menu as the way out. Without that a pad could open a browser from the Functions menu and have no way to move through it. It is the rule `on_status_arrow_press` already applies on the keyboard, extended to the pad.
- **UI page text**: `bnvdaRuntime.js` answers a `page_text` request with the current readable screen as lines (today the AngularJS mod repository / automation details pages, whose spec table and description body carry nothing focusable), and pushes a `screen_context` latch so the controller Functions menu can hide the entry elsewhere. See `docs/lua-ui-runtime-mod-details.md` -- nothing is read automatically, and `cleanText` must never touch body text.
- **Keyboard command lifecycle**: F9 → install hooks + 4-second timeout → process key → unhook. Modifier tracking (Ctrl/Shift/Alt) handled in `_on_next_key_press`.
- **Fallback strategy**: configured speech backend → Prism's highest-priority available one, re-acquired on failure; preferred audio device → system default; air pressure controller → electrics scan for vehicles without dedicated tanks.
- **Elevation and no console**: the shipped build is not elevated. A `WH_KEYBOARD_LL` hook sees and suppresses keys bound for a window at the same integrity level, and the game runs un-elevated (verified in game 2026-11-21). The opt-in `--features uac-admin` builds a `requireAdministrator` exe only for someone who runs Steam or BeamNG as administrator, whose keystrokes Windows hides from a normal process. Release builds have no console window. With no usable standard handles, `updater::apply_and_restart` gives its detached helper explicit null handles rather than letting cmd allocate a console of its own.
- **Cross-VM Lua communication**: Vehicle VM ↔ Game Engine VM via `vehicle:queueLuaCommand()` and `obj:queueGameEngineLua()`. All UDP socket I/O uses LuaSocket.

## Lua Constraints

BeamNG.drive retail uses **LuaJIT (Lua 5.1)**:
- `string.pack`/`string.unpack` are NOT available. Use `string.format` text CSV for UDP packets.
- `castRayStatic(origin, dir, maxDist)` — raycasts against static geometry only. Vehicles use soft-body physics and are invisible to raycasts; use `be:getObject(i)` enumeration instead.
- `core_terrain.getTerrainHeight(pos)` — terrain height queries at arbitrary world positions.
- Lua extensions export a module table `M` with hooks: `onExtensionLoaded()`, `onWorldReadyState(state)`, `onUpdate(dtReal, dtSim, dtRaw)`.

## Dependencies

Rust (`rust/Cargo.toml`, `[workspace.dependencies]`): Tauri 2 for the window, `cpal` for audio output, `rustfft`/`realfft` for the HRTF processing, `tokio` + `axum` for the WebSocket bridge and the MCP server, `ureq` for the updater and AI describer, `windows` for the keyboard hooks and window utilities, `parking_lot`, `serde_json` (with `preserve_order` and `float_roundtrip`), `zip`.

Speech is Prism through the `prismer` crate with its `static` feature (`beam-speech` `prism.rs` adapts it to the speech layer): Prism is compiled from source by the build script and linked into the exe, so there is no DLL to ship, and building needs CMake 3.24+ and a C++23 MSVC. The HRTF sets are read straight from the committed `hrtf_*.npz` files (`beam-audio` `npy.rs`); the `.sofa` sources and the baker went with the Python, so adding a set means baking it outside the repo.
