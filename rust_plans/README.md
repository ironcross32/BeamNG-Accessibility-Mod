# BEAM Rust port: plan index and status

This directory is the complete plan for porting BEAM's Python application to Rust with a
Tauri 2 shell and a semantic-HTML interface. It is written so a different model, or a
fresh session, can pick the work up from the **Status** table at the bottom.

**Start with `HANDOFF.md`**: where the work stopped and what to do next, updated at
every commit.

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

## Batching

Each interrupted agent re-reads its whole transcript when resumed, so wave 2 runs in batches of about five agents rather than all at once. Order: E1, E2, E5 (with A2, A3), then E3, E4, E6, E7, then E8a, E8b, E9, E10, U2.

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
| A1-A3 | beam-audio: callback voices | done | A1: 10 voices, bit-exact without HRTF, within 2.7e-7 with. A2: stereo bit-exact, HRTF within 2.1e-7 (tol 1e-6). No stubs left in beam-audio; every voice sim (hydro, implement, dock, obstacle, road, assistant, junction, route beacon, wheel slip) has a passing Rust test. Not yet reviewed by the orchestrator |
| EA | beam-engine: skeleton, shared state, say(), units, engine start | done | 20 tests; `engine-map.md` covers all 848 top-level statements. beam-audio dependency is commented out in beam-engine/Cargo.toml with stand-ins in `compat.rs` until AH lands waveforms: undo at I1 |
| I0 | Pre-wave-2 integration: re-enable beam-audio in beam-engine, drop audio stand-ins, adapt the four state objects | done | workspace builds; 302 tests pass; `compat.rs` deleted; conversions in `beam_engine::convert`; see engine-map.md section 8 |
| E1 | Telemetry decode and telemetry loop | done | 4 tests (`telemetry_sim`), 18 loop scenarios driven through the real Python loop, every Extended layout V1-V6. Loop body is `TelemetryLoop::handle_datagram` behind a `TelemetryHooks` trait. Python bugs 30-33 in `python-bugs.md` (30 needs the user). Waits on E2's `push_gear_direction`/`push_obstacle_state`. Fixture `telemetry.json` is 3 MB |
| E2 | Listeners A: scanner, obstacle, road, hill climb, fuel | done | 20 tests (`e2_listeners_fixtures`, `obstacle_audio_sim`) against beamtel's real listeners over a fake socket. `ObstaclePacket::Static` now holds a typed `StaticHazard` (inf margin). Untested paths: socket loops, gate/private-road callouts, junction earcons, hill-climb browsers, `AI_STATUS_ALL`. Needs E9 to `catch_unwind` around `on_hill_climb_event`. Bugs 34-40 |
| E3 | Implement, dock, ramp, cannon, terrain scan, line-up, trailer, wheel, beacon | done | Sims `dock_readout` (20), `implement_proximity` (13), `cannon_shot` (7), `route_beacon` (3 + 1 ignored until E6's F9 W readout adds the third `relative_bearing` caller), plus 9 golden tests. `open_camera_lineup` untested (would install real hooks). Bugs 44-47 |
| E4 | Listeners B and their browsers | done | 22 tests: `e4_listeners_fixtures` (24 cases, 258 steps), `environment_row_sim` (8), `vehicle_info_sim` (4). Senders go through E2's `scanner::e2_send`. Page text needs E9's `ui_bridge::broadcast`; cursor warp needs E7's `warp_cursor_to_viewport`. Bugs 48-53 |
| E5 | Keyboard hook, layers, menus, virtual browser | done | `keyboard` package semantics ported (check order, exact-match hotkeys, scan-code matching, its own key-name tables). Tests: `controller_menu_sim`, `menus_golden`, `keyboard_layers`, 12 unit. OS hook installs only once the worker or hotkeys start (accepted). Sim checks owned by E4/E9/B1 listed at the end of `controller_menu_sim.rs`. Bugs 41-43 |
| E6 | F9 command handler and remaining toggles | done | 1081 golden cases (every key x 8 modifier combinations, plus branch internals and dumps) and `F9_HELP` byte-identical. Audit table: `f9-command-table.md` (62 branches). Test seam `set_external_override`. Bugs 54-57 |
| E7 | F10 AI layer, describer, capture, window utilities | done | `e7_ai_golden` 12 tests (F10 dispatch, sends on 4449/4459/4464, 20 validation and 28 describer-worker cases with exact request bytes). HTTP via `ureq`: bundled webpki roots, env proxies only (no registry proxy), different error text for network failures. Win32 paths (GDI grab, window raise, cursor warp) untested live. U2 must call `try_validate_api_key`. Bugs 58-61 |
| E8 | Vehicle spawner | done | `vehicle_spawner/` directory module (screens, arrange, place3d). `vehicle_spawner_golden` 4 tests: scripted packet/key/F11/ticker scenarios against the Python, plus the suppressing hooks through the keyboard listener. The agent's report was lost to a session end, so any Python bugs it found are unrecorded; the V1 spawner audit covers that |
| E9 | UI bridge (WS 8765, TCP 8766) and MCP server | done | `ui_bridge.rs` (`broadcast`, guarded callbacks incl. `challenge_event`), `mcp_server/` (`mod.rs`, `tools.rs`, `camera.rs`; the orchestrator wrote `camera.rs` after the agent died). `mcp_server_sim` ports the Python sim over real HTTP (202 + empty body for a notification, 405 for `GET /mcp`, console-tap correlation, screenshot files, camera argument handling and angle conventions, secret masking) and checks the camera chunks against `mcp_camera.json` (37 cases). `ui_bridge_actions` is the E5 carry-over. The sim's `beamtel.py` greps (sections 13, 19) are left to I1/U2. Known difference: a capture failure reads `screen capture failed: <msg>` without the Python exception type. Agent report lost, so Python bugs unrecorded |
| E10 | Updater, mod installer | done | `e10_updater` 15 tests (check, download, stage, helper bytes and encoding, list2cmdline, install dialogs, startup flow). `beam_base::pyval::json_loads` errors now read as Python's (`line L column C (char P)`). Report lost as for E8 |
| U1 | Frontend: HTML, CSS, JS | done | browser-verified against the mock backend; not yet run in WebView2 or with a screen reader. Contract in `rust/ui/CONTRACT.md` |
| U2 | Tauri backend: commands, events, console, logs menu, dialogs | done, debug build run in game | All 26 commands and 6 events in `CONTRACT.md`. `shell.rs` (state, pre-`frontend_ready` buffering, `UiSink`, close sequence with a 5 s `close_ready` watchdog), `panel.rs` (the 79-key load/save/reset), `describer.rs`, `choices.rs`, `console_view.rs`, `logs_menu.rs` (native Win32 popup), `dialogs.rs` (`MessageBoxW`), `tones.rs`. 24 tests in `src-tauri/tests/` (integration tests, because a Tauri test binary needs the Common Controls v6 manifest: see `build.rs`). Run by the user 2026-11-21 in both modes, un-elevated: panels, saves, F9/F10 layers and suppression work; audio crackled in the debug build only; the release build is clean. See HANDOFF. Deliberate differences: a key validation whose exception would kill the Python worker (leaving the button disabled) reports it as a failure; the gear preview clips a tone that overruns its buffer where numpy raised. No Python fixture yet for `config_load` values: the V1 config/UI audit should add one |
| I1 | Integration: whole workspace builds, engine wired to shell | done | `integration-report.md`. 557 tests pass, no stubs left, wiring checked, user run in game un-elevated. Hands R1 the data files to package and V1 the audits it must widen |
| V1 | Parity audits, module by module | in progress (2 of 8) | Done sequentially by the orchestrator (user's choice). Config and UI: `audit-config-ui.md`, nothing open; fixed spin tie rounding in `util.js`. Audio: `audit-audio.md`, nothing open; the post-base Python changes are all ported. Every fixture regenerates byte-identical from today's Python |
| R1 | Build, package, release pipeline (port of build.py) | not started | |
| D1 | Docs: CLAUDE.md, AGENTS.md, README, docs/ updates | not started | |
| T1 | Developer tooling ports | not started | last |
| UV | User validation in game with a screen reader | not started | needs the user |
