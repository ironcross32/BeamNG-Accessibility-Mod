# Repository Guidelines

## Project Structure & Module Organization

The Python application is intentionally flat. `beamtel.py` is the main telemetry, keyboard, speech, and UI entry point; focused helpers such as `audio.py`, `hrtf.py`, `sral.py`, `ai_describer.py`, and `vehicle_spawner.py` handle individual subsystems. `configurator.py` and `config_ui.py` provide the wxPython configuration interface. The BeamNG mod lives under `bng_mod/`: game-engine and vehicle logic is in `lua/`, the loader is in `scripts/`, UI bridge code is in `ui/`, and sounds are in `art/`. Native/runtime assets (`*.dll`, `*.lib`, and `hrtf_kemar_horizontal.npz`) remain at the repository root. Treat `build/`, Nuitka scratch directories, logs, backups, and local configuration as generated files.

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

## Commit & Pull Request Guidelines

History uses short, sentence-case summaries such as `Add BeamNG mod files...` and `Fixed a ... bug`; Conventional Commit prefixes are not required. Keep each commit to one logical change and state the user-visible outcome. Pull requests should explain affected Python/Lua/UI components, list validation steps, link relevant issues, and include screenshots for visible GUI changes. Call out configuration or protocol changes explicitly. Never commit API keys, `%LOCALAPPDATA%/beamtel` contents, logs, or generated build artifacts.
