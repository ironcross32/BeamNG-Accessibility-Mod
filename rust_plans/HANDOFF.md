# Handoff

The short version of where the port is, for a fresh session. Updated at every commit on
`rust-port`. The full status table is in `README.md`; the resume procedure is in
`05-orchestration.md` ("Resuming after a model or session switch").

## Last updated

2026-11-20, with the commit that fixes `keyboard_layers::f9_layer_eats_keys_until_cleared`.
It now checks what the F9 handler receives through
`keyboard::set_layer_body_override_for_tests`, rather than state left behind by E6's old stub.

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
- Wave 2 was interrupted once by a usage limit (2026-11-19). The agents were resumed with
  their work intact.

## In flight

- E3 (instruments) and E4 (listeners B and browsers), running as background Opus agents.
- E6 (F9 commands) and E7 (F10 layer, describer, capture), started 2026-11-20.
- E8 (vehicle spawner, one agent, may convert `vehicle_spawner.rs` to a directory
  module), started 2026-11-20.
- Then E9, E10, U2. If the session died, check their files for `not_ported!` and relaunch
  whichever is unfinished, telling it to continue from what is on disk.

## Next step

1. When the batch 1 agents report: review each report's stub list, run
   `cargo build --workspace` and `cargo test --workspace` in `rust/`, update `README.md`,
   commit by explicit path (only `rust/` and `rust_plans/`), and update this file.
2. Then batch 2: E3, E4, E6, E7. Then batch 3: E8 (spawner), E9, E10, U2.
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
