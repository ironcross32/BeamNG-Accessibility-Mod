# BEAM Rust port: plan index and status

This directory is the complete plan for porting BEAM's Python application to Rust with a
Tauri 2 shell and a semantic-HTML interface. It is written so a different model, or a
fresh session, can pick the work up from the **Status** table at the bottom.

Read in this order:

1. `01-scope-and-architecture.md`: what is ported, what is not, crate layout, runtime design.
2. `02-porting-conventions.md`: the rules every porting agent follows, and the Python-to-Rust
   parity traps (rounding, float formatting, modulo, numpy dtypes).
3. `03-ui-spec.md`: the interface, tab by tab and group by group, and the accessibility rules.
4. `04-workstreams.md`: every unit of work, its Python source, the Rust files it owns, and
   how it is verified.
5. `05-orchestration.md`: the waves, how agents are briefed, commit policy, and what needs
   the user (in-game validation, cutover).

## Hard constraints from the user

- 1:1 feature parity with the Python mod. Nothing dropped, nothing reworded.
- Screen reader accessibility comes first. Every grouping on every tab of the Python
  interface exists in the new one **as a heading**. Semantic HTML first; ARIA only where
  HTML has no element for the job (the tab strip is the one known case).
- All work stays on the `rust-port` branch. **No new branches, no merges, no pushes, no
  pull requests.** Staged commits on `rust-port` are fine. Agents never run a git command
  that changes anything; the orchestrator commits.
- Coding is done by a team of **Opus** subagents; the orchestrator plans, briefs, reviews
  and integrates.
- This machine is Windows 11: shell commands are PowerShell syntax.

## Standing decisions (made by the orchestrator, change if the user objects)

- The Rust code lives in `rust/`. The Python sources stay in place and untouched until the
  user has validated the port in game and approves a cutover. They are the reference
  implementation and the source of golden test fixtures.
- The Lua/JS mod (`bng_mod/`) does not change. Every UDP, WebSocket and TCP wire format
  stays byte-compatible, so either application can run against the same mod.
- The config file, its path, its keys and its on-disk format do not change. A user can
  switch between the Python and Rust builds with the same `beamtel_config.json`.
- The release asset name (`BeamNG_accessibility_mod.zip`) and the executable name
  (`beamtel.exe`) do not change, so an installed Python build can auto-update into the
  Rust build through its existing updater.
- The frontend is plain HTML, CSS and ES modules. No framework and no bundler: the DOM the
  screen reader sees is the DOM that was written.
- Developer tooling (`tools/mapgen`, `tools/dynorig`, `tools/checkerboard`, `bake_hrtf.py`,
  `diagnostic/map_survey`) is the last phase, after the application reaches parity.

## Status

Update this table as work lands. One line per workstream from `04-workstreams.md`.

| ID | Workstream | State | Notes |
|----|-----------|-------|-------|
| W0 | Scaffold workspace, Tauri shell, fixture harness, seed modules | done | builds, 22 tests pass, window opens; admin-elevated launch not yet checked |
| B1 | beam-base: config, logger, secretstore, gear_report, AI registry | done | 51 tests; both Python loaders reproduced (`load_config_engine`, `load_config_configurator`). Config is held as `pyval::PyDict`. Configurator audio-test helpers belong to U2 |
| B2 | beam-reports: pure report modules | done | 87 tests, bit-exact fixtures. Has its own `pyops` (PyValue, json_loads); B1 is writing a similar `pyval` in beam-base: reconcile at I1. wheel_slip_sim scenario 7 belongs to A1 |
| B3 | beam-road: road guidance, assistant, recorders | done | 63 tests; file bytes match Python. challenge_results fixture is time-zone dependent (Eastern). traffic_mode_sim belongs to E2; stop-bar audio block to A2 |
| S1 | beam-speech: Prism FFI and speech | done | 29 tests; matches Python backend/voice listing on this machine; app must call `speech::atexit()` on exit; ship `prism.dll` beside the exe |
| AH | beam-audio: hrtf, terrain scan render, waveform generators | done | 34 tests; numpy default_rng reproduced exactly in `dsp`, so all 76 waveforms and scan jitter are bit-identical to Python. beam-audio now builds |
| AA | beam-audio: controller state, API, device, callback skeleton | done | split in `audio-callback-split.md`; crate builds once AH lands `waveforms.rs`; scanner diag hookup and device run untested |
| A1-A3 | beam-audio: callback voices | not started | split decided by AA |
| EA | beam-engine: skeleton, shared state, say(), units, engine start | done | 20 tests; `engine-map.md` covers all 848 top-level statements. beam-audio dependency is commented out in beam-engine/Cargo.toml with stand-ins in `compat.rs` until AH lands waveforms: undo at I1 |
| I0 | Pre-wave-2 integration: re-enable beam-audio in beam-engine, drop audio stand-ins, adapt the four state objects | in progress | |
| E1 | Telemetry decode and telemetry loop | not started | |
| E2 | Listeners A: scanner, obstacle, road, hill climb, fuel | not started | |
| E3 | Implement, dock, ramp, cannon, terrain scan, line-up, trailer, wheel, beacon | not started | |
| E4 | Listeners B and their browsers | not started | |
| E5 | Keyboard hook, layers, menus, virtual browser | not started | |
| E6 | F9 command handler and remaining toggles | not started | |
| E7 | F10 AI layer, describer, capture, window utilities | not started | |
| E8 | Vehicle spawner | not started | |
| E9 | UI bridge (WS 8765, TCP 8766) and MCP server | not started | |
| E10 | Updater, mod installer | not started | |
| U1 | Frontend: HTML, CSS, JS | done | browser-verified against the mock backend; not yet run in WebView2 or with a screen reader. Contract in `rust/ui/CONTRACT.md` |
| U2 | Tauri backend: commands, events, console, logs menu, dialogs | not started | |
| I1 | Integration: whole workspace builds, engine wired to shell | not started | |
| V1 | Parity audits, module by module | not started | |
| R1 | Build, package, release pipeline (port of build.py) | not started | |
| D1 | Docs: CLAUDE.md, AGENTS.md, README, docs/ updates | not started | |
| T1 | Developer tooling ports | not started | last |
| UV | User validation in game with a screen reader | not started | needs the user |
