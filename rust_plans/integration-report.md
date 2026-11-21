# Integration report (I1)

2026-11-21, after U2. What builds, what runs, what is stubbed, and what the next
workstreams inherit.

## Builds

- `cargo build --workspace` and `cargo build --release -p beam-app` are clean.
- Clippy reports nothing in `src-tauri/`. The audio callback crates carry deliberate
  warnings (negated comparisons that keep Python's NaN behaviour, indexed loops in the
  voice mixers); leave them.
- The release exe is `rust/target/release/beamtel.exe`, about 21 MB. **Run release
  builds**: the debug build's audio callback is too slow and crackles (see below).

## Tests

- `cargo test --workspace --no-fail-fast`: **557 passed, 0 failed** over 107 targets.
  The 2 ignored are `beam-engine` doc examples, ignored by design.
- The last ignored test, `beam-base`'s route beacon scenario 3, duplicated the
  engine-side check E6 had already enabled. It is replaced by a pointer to
  `beam-engine/tests/route_beacon_sim.rs`.
- `src-tauri`'s tests are integration tests in `src-tauri/tests/`: a Tauri test binary
  needs the Common Controls v6 manifest, which `build.rs` can only embed into those.

## Stubs

None. No `not_ported!`, `todo!`, `unimplemented!` or TODO remains in `rust/`.

## Wiring checked

The Python sims checked these by reading `beamtel.py`; the Rust equivalents were read.

| Check | Where | State |
|---|---|---|
| Engine started by the window | `src-tauri/src/lib.rs` setup: `set_ui_sink`, log sink, `start_engine_thread` | done |
| Startup update flow after the page is ready, releasing the launch gate once | `Shell::frontend_ready` -> `engine::run_update_flow` | done |
| Close: flush, `STOP`, 2 s join, `LEARN_OFF`, speech atexit | `Shell::begin_close` / `finish_close`, `RunEvent::Exit` -> `after_main_loop` | done |
| Speech tap records the final text after the loading suppression and interrupt demotion, immediately before speaking | `speech_out::say_normalized` | done |
| Console tap before the window, LOG copied but not consumed | `console::handle_console_datagram` | done (and `mcp_server_sim` 11d/e) |
| MCP off unless `mcp_server_enabled`; a start failure only logs; tap dropped before the server stops | `engine::run_engine` | done |
| Screenshot and describer share one capture path under their own lock; the describer does not raise the window | `capture::capture_scene_png`, `CAPTURE_LOCK` | done |
| Page text request reaches the UI | `browsers::request_page_text` -> `ui_bridge::broadcast` | done (E4's carry-over closed) |
| Hill climb callback guarded like the Python bridge | `ui_bridge` `guarded("challenge_event", ..)` | done (E2's carry-over closed) |
| Six controller actions through the bridge | `tests/ui_bridge_actions.rs` | done (E5's carry-over closed) |

## Runs (user, 2026-11-21, un-elevated, NVDA)

- `--configurator`: panels filled from the real config; gear mode, root and scale edits
  saved and verified on disk; exit 0.
- Main mode, debug then release: game launched through Steam (or skipped when already
  running), every listener bound, UI bridge and console round trip live, MCP server
  answered over real HTTP. F9 and F10 layers worked; **key suppression works without
  admin**; AI commands reached the game; clean close, no warnings or errors logged.
- Audio crackled in the debug build with the scanner and a chasing AI car; **the release
  build is clean** under the same load.
- NVDA queued instead of interrupting while the beamtel window had focus, twice; it
  cleared on an NVDA restart. Treated as NVDA's issue, not pursued.

## Decisions made here

- **No administrator requirement.** The hook needs none while the game is un-elevated,
  which a Steam launch gives. `uac-admin` stays as an opt-in feature for anyone who runs
  the game elevated. See `01-scope-and-architecture.md`.
- **Release builds are the standard way to run beamtel.**

## Inherited by later workstreams

- **R1 (packaging):** in development the HRTF `.npz` sets, `HRTF_DATA_LICENSES.md` and
  `prism.dll` were found through the repository root. A shipped exe needs them beside
  it, as `build.py` arranges for the Python build. Also `bng_screenreader_mod.zip`
  next to the exe for the mod installer. The release manifest is `asInvoker`.
- **V1 (parity audits):**
  - The crates finished before the Python working tree moved (AH's HRTF, B3's road
    guidance, A1-A3) must be diffed against the current Python (`HANDOFF.md`, "Things to
    know").
  - E8, E9 and E10's agents died before reporting, so any Python bugs they found are
    unrecorded: the spawner, bridge/MCP and updater audits must look for them.
  - The config/UI audit should add a Python fixture for `config_load`'s values and
    `config_save_panel`'s output.
- **The user:** `python-bugs.md` holds 61 Python bugs the port reproduced as is. Each
  needs a keep-or-fix decision; bug 30 (steering never reaches the obstacle detector) was
  already flagged as waiting.
- **D1 (docs):** `CLAUDE.md`, `README.md` and the `docs/` files still describe the
  Python build, including "requires administrator privileges".
