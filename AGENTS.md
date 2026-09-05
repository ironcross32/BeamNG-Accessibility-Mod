# Repository Guidelines

## Project Structure & Module Organization

The Python application is intentionally flat. `beamtel.py` is the main telemetry, keyboard, speech, and UI entry point; focused helpers such as `audio.py`, `hrtf.py`, `speech.py`, `ai_describer.py`, and `vehicle_spawner.py` handle individual subsystems. `configurator.py` and `config_ui.py` provide the wxPython configuration interface. The BeamNG mod lives under `bng_mod/`: game-engine and vehicle logic is in `lua/`, the loader is in `scripts/`, UI bridge code is in `ui/`, and sounds are in `art/`. `hrtf_kemar_horizontal.npz` remains at the repository root; the speech library's native payload comes from the installed `prismatoid` wheel rather than the repo. Treat `build/`, Nuitka scratch directories, logs, backups, and local configuration as generated files.

## Build, Test, and Development Commands

- `uv sync` installs Python 3.11+ dependencies from `uv.lock`.
- `uv run python beamtel.py` starts the main receiver; keyboard suppression requires an elevated Windows terminal.
- `uv run python configurator.py` opens the configuration GUI.
- `uv run python build.py` builds `beamtel.exe`, packages the mod, and creates a release ZIP in `build/`.
- `uv run python build.py --mod` packages only the mod; `--exe` builds only the executable.

## Coding Style & Naming Conventions

Use four spaces in Python, `snake_case` for functions and variables, `PascalCase` for classes, and uppercase names for constants. Follow the surrounding two-space style in Lua and JavaScript, where module filenames commonly use descriptive camel case such as `vehicleScanner.lua`. No formatter or linter is configured, so keep diffs focused and preserve existing protocol names, UDP ports, and fallback behavior. Add comments for cross-VM communication, threading, or non-obvious audio timing—not for routine control flow.

## Testing Guidelines

There is currently no automated test suite or coverage target. At minimum, syntax-check edited Python files with `uv run python -m compileall -q <files>` and manually exercise the affected entry point. Changes spanning Python and `bng_mod/` must be verified in BeamNG.drive, including speech/audio output and both sides of any UDP or WebSocket exchange. Describe manual checks in the pull request.

## Map Generator Knowledge

The reproducible BeamNG level generator is under `tools/mapgen/`; read `docs/level-generation.md` before changing it. `mapdef.py` synthesizes the 2048 x 2048 node-aligned heightmap and per-cell material layer, `terfile.py` reads and writes BeamNG `.ter` versions 7 and 9, `textures.py` creates terrain/depth/preview PNGs without Pillow, `build.py` assembles the unpacked `beam_proving_grounds` mod, and `verify.lua` performs checks that require a running game. The local BeamNG installation used for format comparisons is `D:\steam\steamapps\common\BeamNG.drive`; shipped levels are ZIP archives under `content\levels`, and `gameengine.zip` plus the loose `lua\` tree contain shared data and engine Lua. Extract only needed files into a temporary directory rather than modifying the installation.

Validated on 2026-09-03: the parser re-emitted the shipped `smallgrid` (v7, 256) and `small_island` (v9, 1024) terrains byte-for-byte; a temporary Proving Grounds build completed, all generated JSON parsed, and its generated 2048 terrain also round-tripped identically. Current BeamNG Lua confirms level discovery through `main/`, `info.json` spawn-point enumeration, the SpawnSphere 180-degree correction, level-local `mainLevel.lua` loading, contact material IDs, and rumble-strip audio. The previous live `road_hill_climb` loaded with 33 scene control points and produced 25 optimized navigation nodes / 24 edges; `map.findClosestRoad()` resolved it at the base, middle, and summit. The generated replacement is a 3000 m climb from 4 to 54 percent grade with a later-steepening `P = 2` profile, 63 scene control points, a flat z=682.15 m summit after a 45 m cubic vertical curve, and a 2100 m terrain encoding range; it still requires fresh in-game verification. The one-kilometre tunnel is generated at x=-2000 from y=500 to 1500: 16 globally mounted Utah 64 m collision modules are uniformly shortened to 62.5 m and sunk 0.20 m so their measured pavement is flush with the z=50 terrain, an `SFXSpace` spans exactly 1000 m using the verified global `Level_Tunnel_Closed_Dynamic` ambience, and a separate invisible DecalRoad exposes it to AI and BeamTel. File validation does not replace `verify.lua`: water wetness, final headings, surface behavior, hill-climb navigation/terrain, tunnel collision/reverb, and runtime-spawned sound-stage rigs still require BeamNG.

BeamNG does not execute an issued level/Lua reload until the game has focus. Before treating any console reload as completed, focus the BeamNG window (or stop and tell the user to do so) and confirm a fresh load in `beamng.log`; a successful command response alone is not evidence that the reload ran.

Known follow-ups: `docs/level-generation.md` still says the Tilt Ramp faces south although `mapdef.py` and `verify.lua` currently expect north, and the same document says its POI approach is 90 m although `STAGE_APPROACH_M` is 45 m. Do not silently resolve these inconsistencies while doing unrelated work.

## Commit & Pull Request Guidelines

History uses short, sentence-case summaries such as `Add BeamNG mod files...` and `Fixed a ... bug`; Conventional Commit prefixes are not required. Keep each commit to one logical change and state the user-visible outcome. Pull requests should explain affected Python/Lua/UI components, list validation steps, link relevant issues, and include screenshots for visible GUI changes. Call out configuration or protocol changes explicitly. Never commit API keys, `%LOCALAPPDATA%/beamtel` contents, logs, or generated build artifacts.
