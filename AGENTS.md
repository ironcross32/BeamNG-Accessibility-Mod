# Repository Guidelines

## Project Structure & Module Organization

BEAM's application is the Rust workspace at the repository root: `crates/beam-engine` (telemetry, UDP listeners, keyboard layers, menus, MCP server, updater), `crates/beam-audio` (synthesis, HRTF, the output callback), `crates/beam-speech`, `crates/beam-road` (road guidance and the driving assistant), `crates/beam-reports` (pure report and phrasing modules), `crates/beam-base` (config, paths, logger, HRTF set list), and `src-tauri` (the window; binary `beamtel.exe`) with its frontend in `ui/`. `BUILDING.md` has the full layout. The BeamNG mod lives under `bng_mod/`: game-engine and vehicle logic is in `lua/`, the loader is in `scripts/`, UI bridge code is in `ui/`, and sounds are in `art/`. The HRTF sets (`hrtf_*.npz`, listed in `crates/beam-base/src/hrtf_sets.rs`) remain at the repository root. Treat `target/`, logs, backups, and local configuration as generated files.

The only Python left is the level generators in `tools/`, which have their own `tools/pyproject.toml`.

## Build, Test, and Development Commands

Run from the repository root:

- `cargo build --workspace` builds everything; `cargo run -p beam-app` opens the app (debug builds keep a console).
- `cargo build --release -p beam-app` builds the shipped `beamtel.exe`. It needs no elevation: keyboard suppression works while the game is un-elevated. `--features uac-admin` is opt-in, only for someone who runs the game as administrator.
- `cargo test --workspace` runs the unit, fixture and sim tests; `cargo clippy --workspace --all-targets` and `cargo fmt --all` before committing.

`cargo xtask dist` builds the release bundle (`target\dist\BeamNG_accessibility_mod.zip`: the exe, the stamped mod zip, the HRTF sets and their licence notice, the README). The version is `[workspace.package] version` in `Cargo.toml` and nowhere else; `git release` (`scripts/release.ps1`) bumps it, commits, tags and pushes. CI (`.github/workflows/ci.yml`) runs fmt, clippy with `-D warnings` and the tests on Windows; `release.yml` builds and publishes on a `v*` tag. Details: `BUILDING.md`, "Releases".

## Coding Style & Naming Conventions

Rust follows `cargo fmt` and `rust_plans/02-porting-conventions.md`. Follow the surrounding two-space style in Lua and JavaScript, where module filenames commonly use descriptive camel case such as `vehicleScanner.lua`. Keep diffs focused and preserve existing protocol names, UDP ports, and fallback behavior. Add comments for cross-VM communication, threading, or non-obvious audio timing, not for routine control flow.

## Testing Guidelines

`cargo test --workspace` must pass. The golden fixtures in `fixtures/data/` were recorded from the old Python application and are frozen; never edit one by hand. Lua and JavaScript sims live in `diagnostic/` (`*_sim.lua`, `*_sim.js`); run the one matching the area you touched. Changes spanning the app and `bng_mod/` must be verified in BeamNG.drive, including speech/audio output and both sides of any UDP or WebSocket exchange. Describe manual checks in the pull request.

## Map Generator Knowledge

The reproducible BeamNG level generator is under `tools/mapgen/`; read `docs/level-generation.md` (shared engine and format facts) and `docs/level-proving-grounds.md` (the layout) before changing it. Run it with `uv run --project tools python tools/mapgen/build.py`. `mapdef.py` synthesizes the 2048 x 2048 node-aligned heightmap and per-cell material layer, `terfile.py` reads and writes BeamNG `.ter` versions 7 and 9, `textures.py` creates terrain/depth/preview PNGs without Pillow, `build.py` assembles the unpacked `beam_proving_grounds` mod, and `verify.lua` performs checks that require a running game. The local BeamNG installation used for format comparisons is `D:\steam\steamapps\common\BeamNG.drive`; shipped levels are ZIP archives under `content\levels`, and `gameengine.zip` plus the loose `lua\` tree contain shared data and engine Lua. Extract only needed files into a temporary directory rather than modifying the installation.

Validated on 2026-09-03: the parser re-emitted the shipped `smallgrid` (v7, 256) and `small_island` (v9, 1024) terrains byte-for-byte; a temporary Proving Grounds build completed, all generated JSON parsed, and its generated 2048 terrain also round-tripped identically. Current BeamNG Lua confirms level discovery through `main/`, `info.json` spawn-point enumeration, the SpawnSphere 180-degree correction, level-local `mainLevel.lua` loading, contact material IDs, and rumble-strip audio. The previous live `road_hill_climb` loaded with 33 scene control points and produced 25 optimized navigation nodes / 24 edges; `map.findClosestRoad()` resolved it at the base, middle, and summit. The generated replacement is a 3000 m climb from 4 to 54 percent grade with a later-steepening `P = 2` profile, 63 scene control points, a flat z=682.15 m summit after a 45 m cubic vertical curve, and a 2100 m terrain encoding range; it still requires fresh in-game verification. The one-kilometre tunnel is generated at x=-2000 from y=500 to 1500: 16 globally mounted Utah 64 m collision modules are uniformly shortened to 62.5 m and sunk 0.20 m so their measured pavement is flush with the z=50 terrain, an `SFXSpace` spans exactly 1000 m using the verified global `Level_Tunnel_Closed_Dynamic` ambience, and a separate invisible DecalRoad exposes it to AI and BeamTel. File validation does not replace `verify.lua`: water wetness, final headings, surface behavior, hill-climb navigation/terrain, tunnel collision/reverb, and runtime-spawned sound-stage rigs still require BeamNG.

BeamNG does not execute an issued level/Lua reload until the game has focus. Before treating any console reload as completed, focus the BeamNG window (or stop and tell the user to do so) and confirm a fresh load in `beamng.log`; a successful command response alone is not evidence that the reload ran.

Known follow-ups: `docs/level-proving-grounds.md` still says the Tilt Ramp faces south although `mapdef.py` and `verify.lua` currently expect north, and the same document says its POI approach is 90 m although `STAGE_APPROACH_M` is 45 m. Do not silently resolve these inconsistencies while doing unrelated work.

## Commit & Pull Request Guidelines

History uses short, sentence-case summaries such as `Add BeamNG mod files...` and `Fixed a ... bug`; Conventional Commit prefixes are not required. Keep each commit to one logical change and state the user-visible outcome. Pull requests should explain affected Rust crates, Lua and UI components, list validation steps, link relevant issues, and include screenshots for visible GUI changes. Call out configuration or protocol changes explicitly. Never commit API keys, `%LOCALAPPDATA%/beamtel` contents, logs, or generated build artifacts.
