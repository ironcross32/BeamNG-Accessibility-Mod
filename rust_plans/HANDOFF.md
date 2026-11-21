# Handoff

The short version of where the port is, for a fresh session. Updated at every commit on
`rust-port`. The full status table is in `README.md`; the resume procedure is in
`05-orchestration.md` ("Resuming after a model or session switch").

## Last updated

2026-11-21, with the E8/E10 commit (E9 code without its sim).

## Done so far

- Wave 0 and wave 1 are complete and committed (W0, B1, B2, B3, S1, AH, AA, EA, U1, I0).
- A1-A3 (the beam-audio callback voices) are complete: beam-audio has no stubs left, and
  every voice sim has a passing Rust test. The last fix was to `audio_voices_a2_golden.rs`:
  `scanner_feeds_the_hrtf_diagnostic` ran 64 ms of audio, shorter than the first beep
  interval, so it recorded nothing. It now runs 0.6 s. Not yet reviewed by the
  orchestrator; the V1 audits cover that.
- E1, E2 and E5 were launched in an earlier session, but none of their work landed. Their
  files were still the EA stubs on 2026-11-19, and they were relaunched from scratch then.
- **E1 (telemetry) is done and committed**: `telemetry.rs`, `tests/telemetry_sim.rs`,
  `fixtures/gen/telemetry.py`, `fixtures/data/telemetry.json`. It found Python bugs 30-33
  (`python-bugs.md`). **Bug 30 is waiting on the user**: the steering globals are never
  written, so the obstacle detector always receives steering 0.

- **E2 (listeners A) is done and committed**: six listener files, `tests/e2_listeners_fixtures.rs`,
  `tests/obstacle_audio_sim.rs`, `fixtures/gen/e2_listeners.py`. Python bugs 34-40.
  Carry-overs for later workstreams: **E9** must `catch_unwind` around
  `hill_climb::on_hill_climb_event` and log `challenge_event callback error: {e}` like the
  Python bridge; optionally a beam-road-typed road telemetry snapshot (see bug 37). The
  fuel test strips the virtual browser's title announcement, which comes from E5's code.

- **E5 (keyboard, menus, virtual browser) is done and committed.** Python bugs 41-43.
  The F9/F10 layers call E6/E7 stubs, so they do nothing until those land.
  `controller_menu_sim.rs` ends with a list of sim checks that belong to E4 (clickspot
  staging, SNAP/EXEC, announcements, packet strings), E9 (the six bridge actions) and B1/U2
  (config defaults, checkbox): those agents must port them.
- E5 follow-up, committed: `keyboard::add_hotkey_suppressed` / `remove_hotkey` for E8's
  F11 (the callback returns `false` to eat the key, as Python's `None`; single-step
  hotkeys only), and `menus.rs` uses E1's exported constants.
- **E3 (instruments) is done and committed**: eight listener files, sims
  `dock_readout`, `implement_proximity`, `cannon_shot`, `route_beacon` (engine half), and
  `e3_instruments_fixtures`. Python bugs 44-47. **Un-ignore** the `route_beacon_sim`
  scenario 3 test once E6 lands (it needs the F9 W waypoint readout).
- **E4 (listeners B, browsers, console) is done and committed.** Python bugs 48-53.
  `request_page_text` sends nothing until E9 ports `ui_bridge::broadcast`.
- **E6 (F9 commands) is done and committed**: `f9_commands.rs`, `tests/f9_commands.rs`
  (1081 golden cases), `rust_plans/f9-command-table.md`. Python bugs 54-57. E3's
  `route_beacon_sim` scenario 3 is un-ignored and passes.
- **E7 (F10 layer, describer, capture, window utilities) is done and committed.** Python
  bugs 58-61. For U2: call `ai_describer::try_validate_api_key` with the selected model
  and `Opts::from_extra_args(ai_registry::extra_args_for(...))`, not the stub-shaped
  `validate_api_key`. For the user's validation pass: F10+Space in game, the MCP
  screenshot with `focus_game`, node-grabber SNAP warping, and API key validation behind
  a corporate/registry proxy (ureq ignores the registry proxy and uses bundled TLS roots).
- Wave 2 was interrupted twice by a usage limit (2026-11-19). The agents were resumed with
  their work intact.

## In flight

Nothing is running. The batch 3 agents (E8, E9, E10) died with the session on
2026-11-20 before reporting; their work was on disk and was finished and committed by
the orchestrator on 2026-11-21:

- **E8 (spawner) and E10 (updater, installer) are done**: all tests pass, fixtures
  regenerate byte-identically. Their Python-bug reports were lost; the V1 audits for the
  spawner and the updater must look for them.
- **E9 is code-complete but untested**: the agent died before writing
  `mcp_server/camera.rs` (written by the orchestrator from `mcp_server.py:745-1092`) and
  before any test. The capture error carries no Python exception type name, so
  `screen capture failed: <msg>` lacks the `<Type>: ` the Python prints.

## Next step

1. Finish E9: port `diagnostic/mcp_server_sim.py` to `tests/mcp_server_sim.rs` (it
   stands the server up and drives it over real HTTP; 202 + empty body for a
   notification and 405 for `GET /mcp` must be tests), plus a fixture for the camera
   chunks (`cam_place_chunk` per addressing mode against the Python's string). Then the
   E5 carry-over checks for the six bridge actions (`controller_menu_sim.rs` lists them).
2. Then U2 (the Tauri backend: `src-tauri/`), which wires the bridge and console into
   the window.
3. Then wave 3: I1 integration, the V1 parity audits, fix-ups.
## Things to know

- **The Python reference has moved under the port.** The working tree has uncommitted
  Python changes made after the port's base: `hrtf.py` (several HRTF sets, `hrtf_sets.py`,
  the new `.npz` files), `audio.py`, `beamtel.py` (road listener, live config),
  `road_guidance.py`, `config_ui.py`, `configurator.py`, and `roadGates.lua` with its
  docs. Line ranges in `04-workstreams.md` can be off by a few lines. Agents port the
  working-tree Python. The crates that were already finished (AH's hrtf, B3's road
  guidance, A1-A3) may predate these changes: **the V1 audits must diff them against the
  current Python**. These Python files are never staged by port commits.
- Port commits stage only `rust/` and `rust_plans/`, by explicit path. Never push, branch
  or merge.
