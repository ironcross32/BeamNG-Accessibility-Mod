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

### Core Components

- **beamtel.py** — Main entry point. Listens for UDP telemetry on port 4444 (OutGauge or Extended protocol), UI messages on port 4579, and vehicle scanner data on port 4445. Implements a keyboard command system (F9 hotkey) with status mode, buffer mode, heading guidance, drift detection, and vehicle scanner. Drives both audio and speech output based on telemetry state.

- **audio.py** — Procedural audio synthesis engine. Generates real-time stereo audio (48kHz, float32) via `sounddevice` callback: compass clicks with stereo panning, shift/TC/steering tones, warning buzzers, drift alerts, heading guidance, and scanner proximity beeps. All levels in dBFS. Manages audio device selection and auto-follows system default device.

- **configurator.py** — wxPython GUI for editing `beamtel_config.json`. Groups settings for speech, units, compass, shift tone, warnings, and audio device. Includes real-time tone testing and SAPI voice enumeration via `comtypes`.

- **sral.py** — Wrapper around native SRAL.dll for cross-platform speech synthesis. Falls back to SAPI if unavailable.

- **nvda_ws_speaker.py** — aiohttp WebSocket server on port 8765 bridging UI events to speech output. Handles speak, log, hover, and hover_cancel message types.

- **bnh_logger.py** — Rotating file logger (`bnvdahook.log`, 1MB max, 3 backups).

### Data Flow

```
BeamNG.drive  --UDP:4444-->  beamtel.py  ---->  audio.py (sounddevice output)
              --UDP:4579-->  (UI toasts)  ---->  sral.py / SAPI (speech)
              --UDP:4445-->  (scanner)
Browser/App   --WS:8765--->  nvda_ws_speaker.py ---> speech
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
- **Fallback strategy**: SRAL → SAPI for speech; preferred audio device → system default.
- **Nuitka build config**: Embedded as `# nuitka-project:` pragmas at the top of `beamtel.py` and `configurator.py`. Both build as `--onefile`. beamtel requires admin (`--windows-uac-admin`) and a console; configurator disables the console.

## Dependencies

numpy, sounddevice, wxpython, aiohttp, keyboard, comtypes. Native DLLs: SRAL.dll, nvdaControllerClient.dll.
