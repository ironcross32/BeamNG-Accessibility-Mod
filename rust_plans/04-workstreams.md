# Workstreams

Each workstream is one agent's job. Line ranges are for `beamtel.py` and `audio.py` as
they stand on `rust-port` at the start of the port; they locate a region, they are not a
boundary to stop reading at. Every agent also follows `02-porting-conventions.md`.

"Sim" means the `diagnostic/` simulator to port as a Rust integration test. "Fixtures"
means golden cases generated from the Python (see the conventions file).

---

## Wave 0

### W0: Scaffold

Creates `rust/` so that every later agent has a place to write and a build that works.

- Cargo workspace with the six library crates, `src-tauri` and `xtask`, shared dependency
  versions in `[workspace.dependencies]`, `rust/.gitignore` (`target/`, `node_modules/`).
- Each crate's `lib.rs` declares every module named in this file, each as an empty file
  with a one-line doc comment, so no later agent has to edit another's `lib.rs`.
- A Tauri 2 application that builds and opens a window showing a placeholder
  `rust/ui/index.html`, static frontend (`frontendDist: "../ui"`, `withGlobalTauri`), no
  bundler. Binary name `beamtel`. Admin manifest behind a cargo feature that is off for
  debug builds. Tauri CLI available (as a dev dependency or `cargo install`), and the
  exact build and run commands written to `rust/README.md`.
- `beam_base::pyfmt`: `py_round`, `py_float_str`, and the printf-style helpers, with
  fixtures generated from Python over a few thousand values including ties, negative
  zero, large and tiny magnitudes.
- Seed ports that prove the fixture pattern end to end: `route_beacon.py` (with
  `diagnostic/route_beacon_sim.py`) and `hrtf_sets.py` in `beam-base`, plus
  `beam_base::paths` (config directory, program directory, the `.npz` lookup that works
  both beside a release exe and from a source checkout).
- `rust/fixtures/gen/_common.py`: puts the repo root on `sys.path`, writes JSON
  deterministically (sorted keys where order is not the point, fixed float repr).
- Verifies: `cargo build --workspace`, `cargo test --workspace`, and the Tauri app
  launches.

---

## Wave 1 (parallel)

### B1: beam-base

| Python | Rust module |
|---|---|
| `bnh_logger.py` | `logger` (same file name, 1 MB, 3 backups, same line format) |
| `secretstore.py` | `secretstore` (DPAPI, `dpapi:v1:` prefix, `None` vs `""`, `SECRET_NAME_PARTS`, `migrate_config`) |
| `gear_report.py` | `gear_report` (phrases, degrees, scales, `render_tone`, `settings_from_config`, `migrate_config`). Sim: `gear_report_sim.py` |
| `ai_describer.py`, registry part only | `ai_registry` (providers, models, extras, `config_keys_for`, defaults, display names, key help) |
| `speech.py`, `migrate_config` only | `speech_config` |
| `configurator.py` config half + `beamtel.py:180-299, 504-644` | `config`: one `DEFAULT_CONFIG`, `load_config` with every coercion and clamp, `_write_config` (atomic, retry on sharing violation), `_read_config_raw`, the corruption path and its two different backup suffixes |

Notes: the Python has **two** `DEFAULT_CONFIG` dicts and two `load_config` functions
(beamtel's and configurator's) that are meant to agree. Diff them. Port one config module
that reproduces both call sites' behaviour, and report every difference found. Config
output bytes are fixture-tested against `json.dump(indent=2, ensure_ascii=False)`.
Secrets round-trip is tested live (seal, unseal) since DPAPI is per user.

### B2: beam-reports

`wheel_slip.py` (sim `wheel_slip_sim.py`, with `diagnostic/data/slip_ice_etkc.csv`),
`wheel_report.py` (`wheel_report_sim.py`), `induction_report.py`
(`induction_report_sim.py`), `camera_lineup.py` (`camera_lineup_sim.py`), `audio_debug.py`
(`audio_debug_row_sim.py`), `drift_speech.py` (`drift_speech_sim.py`), `fuel_stations.py`,
`poi_guidance.py` (`poi_guidance_sim.py`). Fixtures for every public function.

### B3: beam-road

`road_guidance.py` (sims `road_guidance_sim.py`, `speed_limit_sim.py`,
`traffic_control_sim.py`, `traffic_mode_sim.py`), `assistant_route.py`
(`assistant_route_sim.py`), `driving_assistant.py` (`driving_assistant_sim.py`),
`road_diagnostics.py` (`road_diagnostic_sim.py`), `challenge_results.py`
(`hill_climb_challenge_sim.py`), `assistant_diagnostics.py`. The NDJSON and summary files
these write must be byte-compatible, since reports written by one build are reopened by
the other. Some of these sims also touch `audio` or `beamtel`; port the parts that
exercise this crate and list the rest for the owner of the other half.

### S1: beam-speech

`speech.py` over `prism.dll`. Derive the FFI surface from the installed wheel
(`.venv/Lib/site-packages/prism/core.py`, `common.py`, `_dispatch.py`) and the upstream
Prism C header; load the DLL with `libloading` from the program directory (and from the
venv's `prism/_native` in a source checkout). Port backend selection by priority, the
re-acquire on failure and on a screen reader starting or exiting, capability bits,
`list_backends`, `list_voices`, `test_speak`, braille, `describe_capabilities`,
`configure`. Verified by a small example binary that lists backends and speaks a line,
run on this machine.

### AH: beam-audio, pure parts

- `hrtf.py` -> `hrtf` (npz reader, `process_ring`, ITD by low-passed cross-correlation,
  mirror symmetry, equalisation, loudness match, FFT resample, `get_hrir`). Fixtures:
  the processed ring for each bundled set at several elevations, compared within a stated
  tolerance; `band_energy`; the ITD curve.
- `audio.py:1-1449` module-level helpers and constants, and `render_scan` with its grain
  functions -> `consts`, `scan`. Sim: `terrain_scan_sim.py`.
- `audio.py:3475-4138`, every `_generate_*` waveform -> `waveforms`. Fixtures: each
  deterministic waveform sample for sample within tolerance at the default config and one
  non-default; noise-based ones by their documented properties.
- `scanner_hrtf_diag.py` -> `scanner_hrtf_diag`.
- A `dsp` module for the numpy idioms used more than once (`linspace`, convolve modes,
  one-pole, interpolation, window functions), each fixture-tested.

### AA: beam-audio, controller

Reads **all** of `audio.py`. Owns `controller` (the `AudioController` state from
`__init__`, every public method at `audio.py:1941-3475`, `apply_config`,
`update_telemetry_state`, HRTF selection), `device` (host API lookup, target device,
stream restart, the watcher thread, on `cpal`), and `callback/mod.rs` (the callback's
frame: event intake, state snapshot under the lock, voice order, final mix, clip to
`[-0.999, 0.999]`, interleave).

Its first deliverable, before any voice is ported, is `rust_plans/audio-callback-split.md`:
the callback (`audio.py:4138-7027`) divided into voice modules with line ranges, the state
each one reads and writes, the cross-voice couplings that the doc calls out (the dock gate
and implement gate read from the *previous* callback, the obstacle duck bus, the order the
blocks run in), and an assignment of those modules to three agents of roughly equal size.
It then stubs each voice module with its function signature so wave 2 can fill them
without touching `callback/mod.rs`.

### EA: beam-engine, skeleton

Reads **all** of `beamtel.py`. Owns:

- `state`: the shared-state section (`beamtel.py:4351-5046`) as one struct behind the
  `state_lock` equivalent, with the same field names; the snapshot functions.
- `speech_out`: `say`, the speech buffer, the speech tap, `stop_speech`, the loading-state
  settle logic (`beamtel.py:299-504`).
- `units`: all `fmt_*`, `has_*`, `flip_units`, bearing helpers (`beamtel.py:4148-4351`).
- `net`: the listener helper (bind, timeout, stop flag, decode) and the command senders'
  shared code, so two dozen listeners do not each reinvent it.
- `timer`: the cancellable `threading.Timer` equivalent.
- `ui_sink`: the trait the Tauri crate implements.
- `engine`: `_run_engine`, `_apply_live_config`, `_config_watcher`, `_broadcast_ui_settings`,
  the launch gate and game launch (`beamtel.py:125-180, 12136-12618`), with each
  subsystem's start call in the Python's order.
- A stub for every other module in the crate with the function signatures the engine
  calls, so wave 2 agents fill bodies rather than negotiate interfaces.

It also writes `rust_plans/engine-map.md`: every top-level function in `beamtel.py`, the
Rust module it belongs to, and the wave 2 workstream that owns it. Nothing in `beamtel.py`
may be left off that map.

### U1: Frontend

Owns `rust/ui/`. Follows `03-ui-spec.md`. Reads `config_ui.py`, `BeamTelFrame`,
`configurator.py:main`, `updater.py` in full. Delivers `index.html`, `styles.css`,
`js/*.js`, `CONTRACT.md` (the exact command and event list), and
`rust/ui/CONTROL-INVENTORY.md`: a table of every wx control (source line, label, name,
tooltip, range, default, enable and show rules) against its HTML counterpart. Includes a
mock backend (`js/mock-backend.js`, used only when `window.__TAURI__` is absent) so the
page can be opened in a browser and driven with a screen reader before the Rust exists.

---

## Wave 2 (parallel, after wave 1 is integrated)

### A1, A2, A3: callback voices

As assigned by `audio-callback-split.md`. Each ports its voices and the sims that cover
them: `hydro_steer_sim.py`, `implement_tone_sim.py`, `dock_tone_sim.py`,
`obstacle_audio_sim.py`, `road_audio_sim.py`, `assistant_audio_sim.py`,
`junction_proximity_sim.py`. In addition, golden-buffer tests: a Python harness drives
`AudioController._audio_callback` offline through a scripted state sequence and saves the
stereo output; the Rust test replays the same sequence and compares within tolerance
(deterministic voices) or by property (noise voices).

### E1: Telemetry

`beamtel.py:644-742` (OutGauge, Extended current and `EXT_FORMAT_V1`, MotionSim: the
struct formats in the root `CLAUDE.md` are the wire contract, and the positional float
tail is where a mistake is silent) and `telemetry_loop` (`beamtel.py:10526-11484`).
Fixtures: packed packets from Python's `struct` with known values, decoded by both.

### E2: Listeners A

Scanner and callouts (`796-1122`), obstacle (`1122-1258`), road (`1258-1587`), fuel
stations (`6512-6693`), hill climb (`6693-6814`), and the toggles and command senders
that belong to them (`6918-7160`). Sims: `controller_menu_sim.py` parts,
`speed_limit_sim.py`, `obstacle_audio_sim.py` (listener half).

### E3: Instruments

Implement, dock, ramp and cannon phrasing (`1587-2117`), the cannon, line-up, trailer,
wheel-report, route-beacon and terrain-scan listeners (`2117-2700`), the implement
listener and its senders (`2700-3256`). Sims: `dock_readout_sim.py`,
`implement_proximity_sim.py`, `cannon_shot_sim.py`, `route_beacon_sim.py` (listener
half). Docs: `lua-implement-proximity.md`, `lua-ramp-geometry.md`, `lua-cannon-shot.md`,
`camera-lineup.md`, `lua-trailer-angle.md`.

### E4: Listeners B and browsers

Camera, node grabber, clickspot, vehicle bindings, environment, audio debug, vehicle
info, slots (`3256-4148`), console listener and history (`6814-6918`), their toggles
(`7159-7400`), binding learn (`7359-7545`), and every browser built on them
(`7545-8259`). Sims: `environment_row_sim.py`, `vehicle_info_sim.py`,
`audio_debug_row_sim.py` (glue half).

### E5: Keyboard and menus

The low-level hook, the shared F9/F10 layer hook, the command-dispatch worker
(`5046-5411`), status metrics and rows (`5411-5740`), function items (`5740-5978`),
suppressed-key hook records and arrow ownership (`5978-6036`), the accessibility menu
navigator (`6036-6262`), buffer mode and the virtual browser (`6262-6512`),
`install_hotkeys` (`10481-10523`). Sim: `controller_menu_sim.py`. The hook callback must
do nothing but classify and enqueue; the reason is at `beamtel.py:5066`.

### E6: F9 commands

`_invoke_f9_command`, `_mcp_press_command`, `_on_next_key_press` (`8259-9366`) and
`_F9_HELP`. Every branch, every modifier combination, in the Python's order. Produces
`rust_plans/f9-command-table.md`: key, modifiers, action, the Python line, the Rust
function. That table is the audit artefact for this handler.

### E7: F10 layer, describer, capture

AI control (`9366-9446`, `9704-10333`), screen capture and the describer worker
(`9446-9704`), window and DPI utilities (`10333-10481`), and the non-registry half of
`ai_describer.py` (capture, the two provider request builders, validation, logging).

### E8: Vehicle spawner

`vehicle_spawner.py`, all of it, including its own key handling and UDP pair.

### E9: Bridge and MCP

`nvda_ws_speaker.py` (WebSocket 8765, TCP relay 8766, every `register_*_callback`, the
`accessibility_action` validation) and `mcp_server.py`. Sim: `mcp_server_sim.py`, which
stands the server up and drives it over real HTTP; the two protocol details the doc calls
load-bearing (202 with an empty body for a notification, 405 for `GET /mcp`) are tests.

### E10: Updater and mod installer

`updater.py` (check, download, validate, stage, the helper `.cmd` and its verification,
the pending flag and its version check, `NullGate`) and `config_ui.py:41-335`
(`install_mod_interactive`, layout cleanup, stamped version comparison). The logic lives
in `beam-engine`; prompts go through `UiSink`. `APP_VERSION` moves to one constant read
by the build as well.

### U2: Tauri backend

Owns `src-tauri/`. Implements `rust/ui/CONTRACT.md`: every command and event; the log
sink that feeds `log-line`; the console record parser and history; the native View Logs
menu; `MessageBoxW` dialogs; window title; the close sequence (flush saves, stop the
engine, join with a timeout); `--configurator` mode; the startup update flow scheduled
after the window is up, releasing the launch gate exactly once.

---

## Wave 3

### I1: Integration

One agent at a time. Makes the whole workspace compile, replaces remaining stubs or lists
them, wires `engine::run` into the Tauri `setup`, and runs the full test suite. Produces
`rust_plans/integration-report.md`: what builds, what runs, what is stubbed.

### V1: Parity audits

Several read-mostly agents, each given a slice of the Python and its Rust, none of them
the author. Each walks the Python top to bottom and records, per function: ported and
equivalent / ported with a difference / missing. They fix what is small and unambiguous
and list the rest. Slices: config and UI inventory; audio controller and callback;
telemetry loop; listeners; keyboard and F9/F10; spawner; bridge and MCP; updater.
Output: `rust_plans/audit-<slice>.md`. The port is not at parity until every audit's
"missing" and "difference" lists are empty or signed off by the user.

## Wave 4

### R1: Build and release

`xtask` replaces `build.py`: build the release exe, package the mod zip with the stamped
`bnvda_mod_version.json`, gather `prism.dll`, the `hrtf_*.npz` files, `README.md` and
`HRTF_DATA_LICENSES.md`, write `BeamNG_accessibility_mod.zip`. Update
`scripts/release.ps1` and `.github/workflows/release.yml` (the tag-equals-version check
stays). **The workflow file is edited but nothing is pushed.**

### D1: Documentation

`CLAUDE.md`, `AGENTS.md`, `README.md` and `docs/python-components.md` gain the Rust
layout, build commands and module map. The reasoning in `docs/` is language-independent
and stays.

### T1: Developer tooling

`tools/mapgen`, `tools/dynorig`, `tools/checkerboard`, `bake_hrtf.py`,
`diagnostic/map_survey`, the standalone listeners. Ported as `xtask` subcommands or small
binaries. Generated levels are compared byte for byte against the Python's output.

### UV: User validation

See `05-orchestration.md`.
