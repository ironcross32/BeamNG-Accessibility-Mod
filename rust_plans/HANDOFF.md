# Handoff

The short version of where the port is, for a fresh session. Updated at every commit on
`rust-port`. The full status table is in `README.md`; the resume procedure is in
`05-orchestration.md` ("Resuming after a model or session switch").

## Last updated

2026-11-21, with the V1 config/UI audit commit.

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
2026-11-20 before reporting. Their work was on disk, and the orchestrator finished and
committed it on 2026-11-21:

- **E8 (spawner), E9 (bridge, MCP) and E10 (updater, installer) are done.** All
  workspace tests pass and the fixtures regenerate byte-identically. Their Python-bug
  reports were lost, so the V1 audits for the spawner, the bridge/MCP and the updater
  must look for them.
- E9: the orchestrator wrote `mcp_server/camera.rs` (from `mcp_server.py:745-1092`),
  `tests/mcp_server_sim.rs` (the Python sim over real HTTP), the `mcp_camera` fixture,
  and `tests/ui_bridge_actions.rs` (the E5 carry-over). One known difference: the capture
  error carries no Python exception type, so `screen capture failed: <msg>` lacks the
  `<Type>: ` the Python prints. The E2 `challenge_event` guard and E4's `broadcast` are
  in `ui_bridge.rs`.
- Test-writing trap seen here: a `parking_lot` guard taken in one argument of a call
  lives to the end of the statement, so locking the same mutex in another argument
  deadlocks silently (the test hangs at 0 CPU). Clone into a local first.

## Next step

U2 has been run by the user (2026-11-21), un-elevated, debug build:

- `--configurator`: panels filled from the real config, edits saved (gear mode, root,
  scale verified on disk), clean exit 0. Reset to Defaults deliberately not tried (the
  user wants a config backup first).
- Main mode: game launched through Steam, the mod connected (UI bridge, console round
  trip through the MCP server), F10 commands reached the game, and **F9 suppression
  works without admin** (F9 then F read fuel and did not exit the car). Decision: no
  elevation; see `01-scope-and-architecture.md`. Clean close, exit 0, no warnings.
- Audio crackled in the debug build with the scanner on and an AI car chasing. **The
  release build does not** (same test, same session). The unoptimised callback was the
  cause. **Run beamtel from a release build** (`cargo build --release -p beam-app`,
  `target\release\beamtel.exe`); use debug only when debugging.
- With the beamtel window focused, NVDA queued speech instead of interrupting. It
  happened twice and cleared on an NVDA restart; the user judges it NVDA's issue.
  Dropped (2026-11-21).
- Optional, discussed: warn when the game itself runs elevated, the one case an
  un-elevated hook cannot see.

**I1 is done** (`integration-report.md`): builds clean, 557 tests pass, no stubs,
wiring checked. **V1 is under way, one slice at a time in this session** (the user chose
that over eight agents). Done: config and UI (`audit-config-ui.md`). Remaining, in
order: audio, telemetry, listeners, keyboard and F9/F10, spawner, bridge and MCP,
updater. Each writes `audit-<slice>.md`. Then R1 packaging, D1 docs, and the user's
keep-or-fix pass over `python-bugs.md`. Speech is moving to Prismer (user,
2026-11-21): do not plan around `prism.dll`.
Traps found in U2:
- A Tauri test binary dies at load with `STATUS_ENTRYPOINT_NOT_FOUND` unless it embeds
  the Common Controls v6 manifest. `build.rs` adds it to `tests/` targets only
  (`rustc-link-arg-tests`, which Cargo rejects when the crate has no `tests/`), and the
  lib and bin have `test = false`. Put new tests in `src-tauri/tests/`.
- `TrackPopupMenuEx` must run on the window's thread: `logs_menu_popup` posts it with
  `run_on_main_thread` and waits on a channel.
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
