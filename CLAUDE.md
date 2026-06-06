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

- **beamtel.py** — Main entry point. UDP telemetry listener, keyboard command system (F9 hotkey + modifier combos), drives audio and speech.
- **audio.py** — Procedural audio synthesis engine. Real-time stereo 48kHz float32 via `sounddevice` callback. Compass clicks, shift/TC/steering tones, warning buzzers, drift alerts, heading guidance, scanner beeps, obstacle detection tones.
- **hrtf.py** — HRTF binaural panning. Loads MIT KEMAR SOFA files via h5py, interpolates impulse responses by azimuth, FFT-resamples to 48kHz. Used by audio.py for spatial compass clicks.
- **configurator.py** — wxPython GUI for `beamtel_config.json`. SAPI voice enumeration via comtypes, real-time tone testing.
- **sral.py** — Wrapper around native SRAL.dll for speech synthesis. Falls back to SAPI if unavailable.
- **ai_describer.py** — AI Describer pipeline. Captures the primary monitor (`mss`), sends the image to Google Gemini's `generateContent` REST endpoint with a blind-friendly system prompt, returns the spoken description. Validates API keys via the free ListModels endpoint. Logs all descriptions and API errors to `%LOCALAPPDATA%/beamtel/ai_descriptions.log`. Invoked in-game by F10 then Space.
- **nvda_ws_speaker.py** — aiohttp WebSocket server on port 8765. Bridges UI events to speech output.
- **bnh_logger.py** — Rotating file logger (`bnvdahook.log`, 1MB max, 3 backups).
- **diagnostic/** — Standalone UDP listeners for debugging telemetry protocols.

### Lua Mod Components (`bng_mod/`)

Entry point: `scripts/bng_screenreader_mod/modScript.lua` — loads all GE extensions.

- **`lua/vehicle/protocols/796F6C6F313035.lua`** — Custom extended telemetry protocol (filename is hex for "yolo1035"). Sends binary struct at 60Hz on port 4444 with steering, temps, G-forces, tire pressures, air pressure. Also handles diagnostic dump commands (DUMP/PDUMP/HDUMP/DAMAGE) on port 4446→4447.
- **`lua/ge/extensions/vehicleScanner.lua`** — Nearest vehicle detection, target cycling, coupler compatibility matching, alignment teleport. Sends CSV on port 4445, receives commands on port 4448.
- **`lua/ge/extensions/beamtelAI.lua`** — AI mode control (chase, follow, flee, stop). Receives commands on port 4449.
- **`lua/ge/extensions/obstacleDetector.lua`** — 12-ray fan obstacle detection + terrain drop-off/hill sampling. Speed-scaled ranges. 4-quadrant slots prevent audio cacophony. Port 4452→4453.
- **`lua/ge/extensions/cameraInfo.lua`** — Camera spatial data (yaw, pitch, AGL). Port 4450→4451.
- **`lua/ge/extensions/nodeGrabberAccessible.lua`** — Accessible node grabber. Ray-sphere node detection under mouse cursor, cross-VM node metadata caching, SNAP cursor warp. Port 4454→4455.
- **`lua/ge/extensions/clickspotAccessible.lua`** — Accessible clickspot detection. Enumerates vehicle interior trigger volumes, detects hover via `be:triggerRaycastClosest()`, executes trigger actions directly. Port 4456→4457.
- **`lua/ge/extensions/uiToggle.lua`** — Hides/shows the game UI on command (`ui_visibility.set`/`toggle`, the same thing ALT+U toggles). Used by the AI Describer to remove HUD elements before a screenshot. Port 4464.
- **`lua/ge/extensions/bnvdaAutoSpawner.lua`** — Deferred UI app spawning (currently commented out).
- **`ui/modules/apps/bnvda_hook/app.js`** — Invisible Angular directive (1x1px). WebSocket client bridging BeamNG UI events to Python on ws://127.0.0.1:8765. DOM observer with debouncing and controller dominance detection.

### UDP Port Map

| Port | Direction | Purpose |
|------|-----------|---------|
| 4444 | Game→Python | Main telemetry (OutGauge/Extended/MotionSim) |
| 4445 | Game→Python | Vehicle scanner data + AI status |
| 4446 | Python→Game | Diagnostic dump commands |
| 4447 | Game→Python | Diagnostic dump responses |
| 4448 | Python→Game | Scanner commands (ON/OFF/NEXT/PREV/ALIGN/DAMAGE) |
| 4449 | Python→Game | AI commands (MODE/AGGR/SPEED/AVOID/LANE/STATUS) |
| 4450 | Game→Python | Camera info data |
| 4451 | Python→Game | Camera info commands (ON/OFF) |
| 4452 | Game→Python | Obstacle detector data |
| 4453 | Python→Game | Obstacle detector commands (ON/OFF) |
| 4454 | Game→Python | Node grabber data (node hover, snap coords) |
| 4455 | Python→Game | Node grabber commands (ON/OFF/SCAN_ON/SCAN_OFF/SNAP) |
| 4456 | Game→Python | Clickspot data (trigger list, hover, snap) |
| 4457 | Python→Game | Clickspot commands (ON/OFF/SNAP/EXEC) |
| 4462 | Game→Python | Road detector status (ON_ROAD/OFF_ROAD/DORMANT) |
| 4463 | Python→Game | Road detector commands (ON/OFF) |
| 4464 | Python→Game | UI visibility toggle (HIDE/SHOW/TOGGLE) |
| 4579 | Game→Python | UI toast messages |
| 8765 | WebSocket | NVDA/UI bridge |

### Data Flow

```
BeamNG.drive  --UDP:4444-->  beamtel.py  ---->  audio.py (sounddevice output)
              --UDP:4579-->  (UI toasts)  ---->  sral.py / SAPI (speech)
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
- **Extended** (132 bytes, format `<H4sBx9fII22f`): Adds tire/brake temps, clutch temp, G-forces, gear as string ("P","D","R","N","S3","M5"...).
- **MotionSim** (magic "BNG1", format `<4s21f`): 3D position and orientation (yaw, pitch, roll).

### Configuration

Config file: `%LOCALAPPDATA%/beamtel/beamtel_config.json` (shared by beamtel.py and configurator.py). Defaults are hardcoded in `DEFAULT_CONFIG` dicts; user values are merged on top. Config corruption triggers auto-backup (.bak) and reset to defaults.

## Key Patterns

- **Audio callback pattern**: Check triggered events → read state under lock → generate samples → mix all sources → clip to [-0.999, 0.999] → write stereo interleaved output.
- **Keyboard command lifecycle**: F9 → install hooks + 4-second timeout → process key → unhook. Modifier tracking (Ctrl/Shift/Alt) handled in `_on_next_key_press`.
- **Fallback strategy**: SRAL → SAPI for speech; preferred audio device → system default; air pressure controller → electrics scan for vehicles without dedicated tanks.
- **Nuitka build config**: Embedded as `# nuitka-project:` pragmas at the top of `beamtel.py` and `configurator.py`. Both build as `--onefile`. beamtel requires admin (`--windows-uac-admin`) and a console; configurator disables the console.
- **Cross-VM Lua communication**: Vehicle VM ↔ Game Engine VM via `vehicle:queueLuaCommand()` and `obj:queueGameEngineLua()`. All UDP socket I/O uses LuaSocket.

## Lua Constraints

BeamNG.drive retail uses **LuaJIT (Lua 5.1)**:
- `string.pack`/`string.unpack` are NOT available. Use `string.format` text CSV for UDP packets.
- `castRayStatic(origin, dir, maxDist)` — raycasts against static geometry only. Vehicles use soft-body physics and are invisible to raycasts; use `be:getObject(i)` enumeration instead.
- `core_terrain.getTerrainHeight(pos)` — terrain height queries at arbitrary world positions.
- Lua extensions export a module table `M` with hooks: `onExtensionLoaded()`, `onWorldReadyState(state)`, `onUpdate(dtReal, dtSim, dtRaw)`.

## Dependencies

numpy, sounddevice, wxpython, aiohttp, h5py, keyboard, comtypes, zstandard. Native DLLs: SRAL.dll, nvdaControllerClient.dll.
