# Python bugs found during the port

Collected from the wave 1 agents' reports. The rule so far is "port the bug, report it".
This file exists so the user can decide, per item, whether the Rust should keep
reproducing it or be hardened. **Rust now** says what the Rust does today.

Legend: R = reproduced as in Python; H = already hardened in Rust (a departure).

Decided by the user: 30, 66 and 67 fixed in the Rust (2026-11-21). Every bug that loses
configuration data is fixed in the Rust (user, 2026-11-21): 1, 2, 23, 24, 25.

Keep-or-fix pass (user, 2026-11-21), to fix in the Rust:
- Every listener/thread killer: 3-9, 33, 34, 37, 39, 44, 48-51, 53, 63, 64 (catch, log,
  carry on).
- Every wrong readout: 29, 31, 32, 41, 46, 52, 56, 57, 58.
- Input: 42 and 55. **54 stays** (loose F9 modifier checks: people may rely on them).
- Minor: 14, 15, 19, 20, 62.
- Everything else not already H stays as Python: 16, 17, 18, 21, 27, 28, 35, 36, 38, 40,
  43, 45, 47, 54, 59, 60, 65. The Python is sunsetted:
no fixes go there. The rest are R or H as
listed, pending the user's keep-or-fix pass.

## Would lose user data

| # | Where | What happens | Rust now |
|---|---|---|---|
| 1 | both `load_config` | A value that overflows a number conversion (`1e400`, `Infinity`, a huge int in an int or volume key, `scanner_pitch_offset_oct` NaN/inf) is not caught by the per-key fallback; the whole file is treated as corrupt, renamed to the backup and replaced with defaults | **H** (user, 2026-11-21): that key falls back to its default; only unparseable JSON or a non-object root is corruption. The engine loader also defaults an overflowing gear octave/level, which every later `settings_from_config` raised on. `config_fixtures` lists the departing cases (`is_overflow_departure`) |
| 2 | both `load_config` | A failed migration write-back (locked file after retries) takes the same corruption path and renames a good config | **H** (user, 2026-11-21): logged; the migrated config is used in memory and the next load retries the write. `config_fixtures` `failed_migration_write_keeps_the_config` |

## One bad packet or socket error silences a feature until restart

| # | Where | What happens | Rust now |
|---|---|---|---|
| 3 | `telemetry_loop` | One socket error on 4444 ends the whole engine; the window stays open | **H** (user, 2026-11-21): the telemetry loop runs under `timer::supervise` and restarts until STOP |
| 4 | `slot_listener` | A non-UTF-8 `SLOTS` datagram kills the listener | **H** (user, 2026-11-21): the thread runs supervised (`timer::spawn_supervised`): logged and restarted after a 1-30 s backoff; `tests/supervise.rs` |
| 5 | `ui_listener`, `console_listener` | Any socket error stops the listener for the session | **H** (user, 2026-11-21): the thread runs supervised (`timer::spawn_supervised`): logged and restarted after a 1-30 s backoff; `tests/supervise.rs` |
| 6 | `road_listener` | `parse_r2_packet` can raise `TypeError`/`OverflowError`; only `ValueError` is caught. `ROAD_DIAGNOSTICS.record` raises on any NaN, and `HILL_CLIMB_CHALLENGE.record` can raise `AttributeError`, both outside any try | **H** (user, 2026-11-21): the road listener logs an unexpected error and reads on; a panic restarts it (supervised) |
| 7 | induction report thread | Any malformed field (non-number, `"0"` as a string for max turbo RPM, non-dict `turbo`) raises and the report is never spoken | **H** (user, 2026-11-21): an unparseable reply is skipped; a report that cannot be phrased says "Forced induction report could not be read". `e4_listeners_fixtures` `induction_report_matches_python` |
| 8 | drift, fuel, POI parsers | `OverflowError` (infinite or huge numbers) escapes the callers' `except ValueError` | **H** (user, 2026-11-21): the thread runs supervised (`timer::spawn_supervised`): logged and restarted after a 1-30 s backoff; `tests/supervise.rs` |
| 9 | `DrivingAssistant.receive` | Unhashable `kind`, non-string `turn`, or a string arrival `bearing` raise past the handler; the last one after state is cleared, so the "off" cue never plays | **H** (user, 2026-11-21): the scanner listener survives (logged; a panic restarts it). The "off" cue after a bad arrival bearing is still lost |
| 10 | audio callback | An exception aborts the stream for good; the watcher restarts only on a device change | H: caught, logged, silence until restart |
| 11 | telemetry / callout threads | A non-numeric `speed_announce_interval` or `scanner_distance_callout_interval` crashes the thread later | H: previous value kept, warning logged |
| 12 | `speech.speak` | An exception reading the backend name inside a log call escapes a function that promises never to raise | H: logs "unknown" |
| 63 | `vehicle_spawner._handle_packet` | A `CATALOG_ITEM:` whose JSON parses but is not an object (`[1,2]`, `5`, `"x"`) passes the `json.loads` guard and raises AttributeError on `obj.get("hasPolice")`, outside any `try`. The exception ends the spawner listener thread, so F11 has no catalog updates until restart (E8 fixture `catalog_errors`; recorded by V1, as E8's report was lost) | **H** (user, 2026-11-21): the spawner's listener and worker run supervised |
| 64 | `nvda_ws_speaker.tcp_handler` | The relay catches only decode and JSON errors. A line that parses but is not an object (`[1, 2]`), an `accessibility_action` whose action is unhashable (`["x"]`), or a `log` message whose level names a non-callable logger attribute (`handlers`, `level`) raises out of `handle_ws_message` and ends the Lua TCP relay connection, dropping every later UI message until the mod reconnects. The WebSocket path catches everything. The mod never sends these today (dormant). Found by E9; confirmed by V1 (`bridge` fixture) | **H** (user, 2026-11-21): logged as an invalid relay message; the connection stays up. `bridge_fixture` `tcp_lines_match_python` |

## Wrong behaviour, not fatal

| # | Where | What happens | Rust now |
|---|---|---|---|
| 13 | audio device watcher | PortAudio caches its device list, so "Follow Default Audio Device" probably never sees a change | H: cpal enumerates live, so it follows |
| 14 | `AudioController.__init__` | `buzzer_amp`/`chime_amp` start as `1.0 ** (-12/20)` = 1.0 (0 dB, not -12 dB) until the first `apply_config` | R |
| 15 | `apply_config` | Never regenerates the TC, scanner, node, clickspot or describe waveforms | R |
| 16 | `_world_is_active` | Any datagram on 4444, valid or not, marks the world active | R |
| 17 | loading "Ready" cue | Demoted by the protection window it has just opened itself | R |
| 18 | `_apply_live_config` | `drift_speech_enabled` passed without `bool()` | R |
| 19 | config loaders | Road toggles: engine accepts only a real bool, configurator uses truthiness (`"false"` is true). Configurator migrates legacy speech/gear keys in memory but only writes back when a secret was sealed | R (both loaders reproduced) |
| 20 | config loaders | `mcp_server_port` never range-checked | R |
| 21 | `extra_args_for` | A truthy non-string OpenAI base URL crashes the describer | R |
| 22 | Configuration panel | A gear root such as "Bb" or "c" passes validation and then crashes the wx panel on load | H: HTML selects the first entry |
| 23 | Configuration panel | With no WASAPI devices, "(no WASAPI devices found)" is saved as the preferred device name. Also (same cause): a saved device that is unplugged when the panel opens is replaced by the first device on the next save | **H** (user, 2026-11-21): the device's value is saved, not the label; a saved device that is absent stays listed as "<name> (not connected)" and selected; the placeholder saves `""`. `config-panel.js` `fillDevices` |
| 24 | AI Describer panel | Switching provider while a key validates saves the key under the new provider | **H** (user, 2026-11-21): the provider choice is disabled while a key validates |
| 25 | Reset to Defaults | The next save rewrites keys the panel does not own from defaults | **H** (user, 2026-11-21): every key the panel does not own is re-read from disk on each save; CONTRACT.md `config_save_panel` step 4. `tests/panel.rs` `save_after_reset_keeps_keys_the_panel_does_not_own` |
| 26 | `speech` test backend | Context can be shut down while the test backend is alive | H: context kept alive until last backend drops |
| 27 | hill climb | A second `race_complete` replaces the timer without cancelling; the stale one fires. `list_reports(0)` returns one. A summary that is valid JSON but not an object aborts the finish | R |
| 28 | recorders | `RoadDiagnosticRecorder.stop` can leave a truncated summary; `AssistantDiagnosticRecorder.review` raises on a non-object line | R (Rust summary writes are all-or-nothing) |
| 29 | F9 Tab error log | Prints `b'NEXT'` | **H** (user, 2026-11-21): logs the command text (`NEXT`) |
| 30 | `telemetry_loop` (~`beamtel.py:11118`) | No `global` for `last_steering`, `last_actual_steering`, `last_steering_input`, so the module globals stay 0.0 all session: `_push_obstacle_state` always sends steering 0.000 to obstacleDetector, and the MCP and road-diagnostic snapshots report steering 0. The loop's own readers use the locals and are correct. **Fixing it changes what the obstacle detector receives, so it is the user's call, and it should be fixed in Python too** | **H** (user decision 2026-11-21): the loop writes the shared state too, and the push sends the path's bend in the detector's convention: BeamNG steering is positive RIGHT and the fan turns LEFT for a positive value, so -steering forward and +steering in reverse (the Python, sending 0, never exercised this sign). NaN steering is no bend (the Python clamp made it full right lock). `e2_listeners` `steering_is_sent_as_the_bend_of_the_path`, `nan_steering_is_no_bend`. **Needs an in-game check** |
| 31 | `telemetry_loop` | The first non-baseline Extended packet speaks "Lightbar off" and "Fog lights off" (`last_lightbar`/`last_fog` start at -1) | **H** (user, 2026-11-21): the first lightbar and fog readings are a baseline. `telemetry_sim` `python_calls_after_departures` |
| 32 | `telemetry_loop` | Low and high beam bits both set: the `else` branch says "Headlights off" | **H** (user, 2026-11-21): both bits set says "High beams on" |
| 33 | `telemetry_loop` | A NaN in the lightbar, fog or implement-flags float, or in speed (via the speed bucket), raises and kills the telemetry thread | **H** (user, 2026-11-21): a non-finite lightbar or fog reading is not announced, a non-finite speed has no bucket, non-finite implement flags are 0. `telemetry_sim` (`raises_*` scenarios) |
| 34 | `scanner_listener` | A direction field of `inf` raises an uncaught OverflowError; the scanner listener stops for the session | **H** (user, 2026-11-21): the scanner listener logs an unexpected error and reads on; a panic restarts it (supervised) |
| 35 | `scanner_listener` | An unparseable text packet that is exactly 8 bytes (`abc,def!`) is read as binary `<ff` and feeds made-up numbers to the scanner tone | R |
| 36 | `scanner_listener` | A NaN direction updates scanner state but never the audio, so the two disagree | R |
| 37 | `road_listener` | A huge int in an R2 packet (OverflowError) or a NaN reaching `ROAD_DIAGNOSTICS.record` (ValueError) stops the road listener | **H** (user, 2026-11-21): as 6 (the NaN never raised in the Rust: the snapshot is serde_json, so it becomes null) |
| 38 | `road_listener` | A disk OSError from either recorder is logged as "Road listener socket error." | R |
| 39 | `fuel_listener` | Fuel JSON nested deeply enough to raise RecursionError stops the listener | **H** (user, 2026-11-21): the thread runs supervised (`timer::spawn_supervised`): logged and restarted after a 1-30 s backoff; `tests/supervise.rs` |
| 41 | Status menu Gear row | Before any telemetry (protocol "outgauge", no gear byte), `gear_to_phrase(None)` raises TypeError; navigating onto Gear kills that command and the worker logs it | **H** (user, 2026-11-21): the Gear row reads "unknown" before any gear byte. `menus_golden` |
| 42 | status mode + browser | With status mode on, a browser opened on top gets no arrows: status mode's hook eats them and `on_status_arrow_press` returns because a browser is open | R |
| 43 | `keyboard` package hook thread | `while not GetMessage(...)` is inverted; the thread would exit if any message were posted (none is) | R |
| 44 | `terrain_scan_listener` | `int(inf)` in `render_scan` (an object with inf range) raises OverflowError; only ValueError/IndexError are caught, so the listener dies. A POI OverflowError or RecursionError does the same | **H** (user, 2026-11-21): the thread runs supervised (`timer::spawn_supervised`): logged and restarted after a 1-30 s backoff; `tests/supervise.rs` |
| 45 | `terrain_scan_listener` | A malformed `SCAN` packet stamps `_last_scan_reply_ts` before parsing, so the watchdog counts garbage as a reply | R |
| 46 | `_cannon_shot_long_distance` | Tests `UNITS_MODE == "imperial"` while `fmt_distance` tests `== "metric"`; any other config value mixes units in one utterance | **H** (user, 2026-11-21): the cannon readout treats everything but "metric" as imperial, like `fmt_distance` |
| 47 | `_dock_phrase_ramp` | A NaN yaw on approach reaches `int(math.floor(nan))` and raises into the F9 caller | R |
| 48 | node grabber, clickspot listeners | Catch only timeout/OSError: one bad int/float in `SNAP:`/`NODE:`/`HOVER:`/`SNAP_OK:` ends the listener for the session | **H** (user, 2026-11-21): the thread runs supervised (`timer::spawn_supervised`): logged and restarted after a 1-30 s backoff; `tests/supervise.rs` |
| 49 | camera listener | `compass_click_interval_deg == 0` raises ZeroDivisionError and the thread dies; an infinite yaw bumps the click counter before the packet is dropped | **H** (user, 2026-11-21): the engine holds `compass_click_interval` to 1-90 (non-finite: 15), which also covers the vehicle compass. The inf-yaw counter bump is kept |
| 50 | environment listener | `ENV_END` refreshes the browser inside the listener; a NaN/inf temperature makes `fmt_temp_c_or_f` raise and the listener dies | **H** (user, 2026-11-21): the thread runs supervised (`timer::spawn_supervised`): logged and restarted after a 1-30 s backoff; `tests/supervise.rs` |
| 51 | binding learn listener | A non-string `control`/`title`/`desc` raises AttributeError and ends the listener | **H** (user, 2026-11-21): the thread runs supervised (`timer::spawn_supervised`): logged and restarted after a 1-30 s backoff; `tests/supervise.rs` |
| 52 | clickspot listener | `TRIGGER_LIST:-2` clears the list but says "-2 clickspots found" | **H** (user, 2026-11-21): a count of zero or less says "No clickspots found on this vehicle". `e4_listeners_fixtures` `python_speech_after_departures` |
| 53 | induction report | A non-numeric `boost` raises in the report thread; nothing is spoken, not even "No response" | **H** (user, 2026-11-21): as 7 |
| 54 | F9 layer | Loose modifier checks swallow combinations: Shift+Alt+F speaks fuel (not fuel stations), Ctrl+Alt+S toggles status mode, Ctrl+Shift+Alt+E runs the electrics dump, Ctrl+Alt+T speaks boost, Tab with any modifiers cycles targets. Full matrix in `f9-command-table.md` | R |
| 55 | `_invoke_f9_command` | Ends with `_clear_next_key_hook`, so an MCP or controller command run while a real F9 layer is open closes that layer | R |
| 56 | F9 readouts | NaN RPM speaks "Inclination 100 percent"; a NaN fuel level speaks "Fuel 100 percent" (NaN through `min`/`max`) | **H** (user, 2026-11-21): a non-finite fuel level says "Fuel level unknown", a non-finite cannon rpm "Inclination unknown". `f9_commands` golden |
| 57 | F9 snapshot | A NaN speed, RPM, engine or oil temperature raises ValueError for every F9 key; the layer stays open until its timeout | **H** (user, 2026-11-21): each non-finite reading is spoken as unknown ("RPM unknown", "Speed unknown", "Engine temperature unknown"); the other keys work. `f9_commands` golden |
| 58 | F10 layer | `name in "12345"` / `base_name in "1234567890"` are substring tests: an empty name with a modifier raises `int('')`; "12" with Shift speaks "Slot 12 is empty" | **H** (user, 2026-11-21): only a single digit is a slot or preset key. `e7_ai_golden` |
| 59 | describer, `config_ui` validation | An OpenAI base URL with no scheme makes `urllib.request.Request` raise ValueError outside every try: the describer says "Description failed."; the config dialog's validation thread dies and the dialog stays on "Validating API key..." with the button disabled for good | R |
| 60 | describer | A non-string API key, model or extra in the config raises AttributeError ("Unexpected error: ..." then "Description failed.") | R |
| 61 | `capture_scene_png` (mss) | mss sets process DPI awareness on first use, after the window rect was read, so the first `focus_game` capture on a scaled display mixes logical and physical coordinates | H: the Rust process is DPI-aware from the start |
| 62 | `road_listener` gate and private-entry words | The last gate id and private-entry id are recorded on every packet, announced or not. So a gate or private road first seen while the assistant is off, or during loading, is never announced once the assistant takes over approaching the same one (found by V1, `e2_listeners` `road_scripts`) | R |
| 65 | `nvda_ws_speaker.hover_on` | `hover_on` sets `_HOVER_TOKEN`, then calls `hover_cancel()`, which clears it to None; the delayed task compares its own token against None, so a hover is never spoken. Dormant: `bnvdaRuntime.js` only ever sends `hover_cancel`. Found by E9 (noted in `ui_bridge.rs`), recorded by V1 | R |
| 66 | `mcp_server._origin_ok` (**security**) | The DNS-rebinding guard is a prefix match: an `Origin` of `http://localhost.example.com` or `http://127.0.0.1.example.com` starts with an allowed prefix and passes. A page served from such a name, rebound to 127.0.0.1, can then call any tool, `lua_exec` included, whenever the MCP server is enabled. Fix: compare the parsed host exactly (`127.0.0.1`, `localhost`, optional port). Off by default and loopback-bound, which limits it to users who enabled it. Found by V1 | **H** (user decision 2026-11-21): the host must be exactly `127.0.0.1` or `localhost` with an optional numeric port; `tests/mcp_origin.rs`, `mcp_server_sim` 10b/c |
| 67 | `updater.write_helper` / `apply_and_restart` | The helper `.cmd` is written with `encoding="ascii"`, and its paths include the update folder under `%LOCALAPPDATA%`, inside the Windows user profile. Any non-ASCII character in the user name or install path (José, Zoë, Cyrillic, CJK) raises UnicodeEncodeError, which `apply_and_restart` does not catch (it catches only OSError). By then the pending flag is written and the game launch denied, so the user is left with beamtel running, no game, no message (a log line only), and an update that never applies. Found by E10 (`helper_encoding_matches_python`), recorded by V1 | **H** (user decision 2026-11-21): the paths reach the helper through its environment, so the file stays ASCII and any path works (checked with a real `cmd` on a `Zoë 漢字 50%` folder); a write failure is reported like the other apply failures. `e10_updater` `non_ascii_paths_reach_the_helper` |
| 40 | `_hill_climb_finalized` | A non-dict `statistics` raises AttributeError; the recorder swallows it and the data-quality log line is lost | R |
## Recommendation

Harden 1-9 in the Rust (catch at the listener or per key, log, carry on), keep the rest as
they are. None of 1-9 changes anything a user hears in normal operation; they only stop a
malformed value from costing a feature or the whole config.
