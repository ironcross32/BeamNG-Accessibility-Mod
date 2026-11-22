# beamtel.py -> beam-engine: the engine map

Written by EA. Every top-level statement of `beamtel.py` (12,677 lines on `rust-port`) is in
the table at the bottom: its line, kind, Python name, the Rust module it goes to, and the
wave-2 workstream that owns it. The table was produced from Python's own `ast` over the file,
so nothing top-level is missing; nested helpers (closures inside listeners, the F9 dump
closures) travel with their enclosing function.

Read with `02-porting-conventions.md`. This file adds the engine-specific rules.

## 1. How shared state works (binding on every wave-2 agent)

### The idiom

```rust
use crate::state::{state, State};

// Python: `with state_lock: last_scanner_distance = d`
{
    let mut st = state();                       // parking_lot::MutexGuard<'static, State>
    st.scanner.last_scanner_distance = d;
}

// Python reads a global WITHOUT the lock (most toggles, flags, `_route_is_set`): Rust takes
// the lock for that read. One short guard per read site, never held across I/O.
let active = state().scanner.scan_mode_active;
```

* Every module-level mutable global that the Python guards with `state_lock`, or reads and
  writes bare, is a field of `state::State`, grouped in sub-structs named after the feature
  (`st.tel`, `st.cfg`, `st.scanner`, `st.dock`, `st.camera`, `st.env`, `st.vbrowser`, ...).
  **Field names are the Python names with the leading underscore dropped** (the same rule
  as functions: `_env_rows` -> `st.env.env_rows`). Initial values are the Python ones.
  `rg -n "fn default\(\)" src/state.rs` shows them all in one place.
* Functions the Python calls *while holding* `state_lock` and that read globals take the
  state as a parameter instead of locking: `has_air_system(&st)`, `fmt_heading(&st)`,
  `get_speed_bucket(&st, speed)`, `report_gear_change(&st, ...)`,
  `push_gear_direction(&mut st, gear)`, `push_obstacle_state(&mut st, force, gear)`,
  the status-row getters. The Python lock is non-reentrant too, so the Python never
  re-enters it; the Rust only has to avoid locking where the Python *read bare inside a
  locked region*. `parking_lot::Mutex` is not reentrant: **calling `state()` while a guard
  is alive on the same thread deadlocks.** When in doubt, take the guard once at the top of
  the function and pass `&mut State` down.
* A Python exception that its caller does not catch is a Rust `panic!` with the Python
  message (see `units::py_int_round`). `parking_lot` does not poison, so a panic inside a
  guard releases it exactly as `with state_lock:` does. The command worker and the engine
  `catch_unwind` where the Python has `except Exception` / `finally`.

### What is NOT in `State`, and why

| Python | Rust | Why |
|---|---|---|
| `UNITS_MODE` | `units::units_mode()` / `set_units_mode()` (own `RwLock`) | read by every `fmt_*`, which is called both under and outside `state_lock` |
| `_command_context`, `_speech_protected_until`, `speech_logging_active`, `SPEECH_BUFFER`, `SPEECH_TAP`, the `_loading_*` block, `_telemetry_baseline_pending` | `speech_out` (atomics, `_loading_lock`, `_speech_tap_lock`, the buffer's own lock) | `say()` reads them and `say()` is called while `state_lock` is held (`_report_gear_change`, `on_buffer_nav_press`, the F9 digit branch, the telemetry loop). `say()` must never take `state_lock` |
| `audio_controller_ref` | `state::audio_controller_ref()` (`RwLock<Option<Arc<AudioController>>>`) | used from everywhere, often under the lock |
| `driving_assistant`, `ROAD_GUIDANCE_FEED`, `HILL_CLIMB_CHALLENGE`, `ROAD_DIAGNOSTICS`, `ASSISTANT_DIAGNOSTICS`, `DRIFT_ANNOUNCER` | `state::driving_assistant()`, `state::road_guidance_feed()`, `state::hill_climb_challenge()`, `state::road_diagnostics()`, `state::assistant_diagnostics()`, `state::drift_announcer()` | objects with their own internal locks, called under `state_lock` by the Python |
| `_vehicle_slots`, `_selected_slots`, `_target_slot` + `_slots_lock` | `state::slots()` (`Mutex<Slots>`) | Python's separate `_slots_lock` |
| `_capture_lock` | `state::CAPTURE_LOCK` | separate lock in the Python |
| `_describer_lock` + `_describer_busy` | `state::DESCRIBER_BUSY` (`Mutex<bool>`) | separate lock in the Python |
| `_vinfo_event`, `_page_text_event` | `state::VINFO_EVENT`, `state::PAGE_TEXT_EVENT` (`timer::Event`) | `threading.Event`s, waited on outside any lock |
| `STOP`, `LAUNCH_GATE`, `LAUNCH_ALLOWED`, `LAUNCH_SUPPRESSED_BY_SHIFT` | `timer::STOP`, `engine::LAUNCH_GATE`, `engine::LAUNCH_ALLOWED`, `engine::LAUNCH_SUPPRESSED_BY_SHIFT` | events/flags |
| `_kb_layer`, `_live_mods`, `_kb_layer_hook`, `_kb_layer_release_hook`, `_command_queue`, `_command_worker_thread`, `KEYBOARD_OK` | `keyboard` module statics (E5) | read inside the low-level hook, which must take no lock (`beamtel.py:5066`) |
| `nodegrab_mode_active`, `_nodegrab_space_held`, `_nodegrab_space_hook` | `state::NODEGRAB_MODE_ACTIVE`, `state::NODEGRAB_SPACE_HELD` (atomics), hook record in `listeners::nodegrab` | `_nodegrab_on_space` reads them inside the hook |
| `_mcp_console_tap`, `console_frame` | `console::register_console_tap`, `UiSink::console_message` | callbacks |
| `_vehicle_spawner` | `vehicle_spawner` module itself (`vehicle_spawner::is_modal_open()` / `close_modal()`) | Python held a module reference |
| constants (`*_PORT`, `IMPL_*`, `RAMP_*`, `LS_*`, `DRIFT_*`, ...) | `pub const` in the module that uses them; ports all in `net` | constants are not state |

### Lock ordering observed in the Python (keep it)

1. `state_lock` is outermost. While holding it the Python calls: `say()` (-> `_loading_lock`,
   `_speech_tap_lock`, the speech library), `AudioController` methods (its own lock),
   `DRIFT_ANNOUNCER.quiet()`, `ROAD_GUIDANCE_FEED.speed_limit()`, `driving_assistant.active`,
   `HILL_CLIMB_CHALLENGE.list_reports()` (function-item availability under the navigator's
   lock), and UDP sends (`_push_gear_direction` and `_push_obstacle_state` from the telemetry
   loop). All of those are leaves: none of them ever takes `state_lock`.
2. `_slots_lock` is never nested with `state_lock` (`_mcp_snapshot_state` releases
   `state_lock` before taking it). `say()` is called while holding `_slots_lock`
   (`_toggle_slot_selection`, `_set_target_slot`). If you ever need both: state, then slots.
3. `_loading_lock`, `_speech_tap_lock` and the speech buffer lock are leaves. Never call
   `say()` while holding one (the Python never does).
4. `_capture_lock` is held across HIDE -> sleep -> grab -> SHOW and takes no other engine lock.
5. Nothing may be held while sleeping, waiting on an `Event`, blocking on a socket, or
   installing/removing a keyboard hook, except where the Python does exactly that
   (`_capture_lock`, the 4447 dump receivers which hold nothing).
6. The keyboard hook callbacks take **no** lock at all; they classify and enqueue
   (`keyboard::command_dispatch`).

## 2. Shared helpers (EA, done)

* `speech_out`: `say`, `say_value`, `stop_speech`, the replay buffer, the speech tap and
  `get_speech_log`, the loading lifecycle (`on_loading_state_changed`,
  `finish_loading_settle`), the speech log file, and the accessors other modules need for
  the speech-side flags (`command_context`, `protect_speech`, `queue_loading_pending_vehicle`,
  `loading_or_settling`, `take_telemetry_baseline_pending`, ...).
* `units`: every `fmt_*`, `has_*`, `fmt_heading`, `flip_units`, `get_speed_bucket`,
  `yaw_to_heading_deg`, `COMPASS_NAMES`, and the two 8-way compass index helpers lifted out
  of `camera_listener` and `telemetry_loop` (`compass_zone_index`, `compass_snap_match`).
* `net`: `UdpListener`, `Recv`, `decode`/`Decode`, `send_udp`, `send_cmd`, `send_quiet`,
  every port constant.
* `timer`: `Event`, `Timer`, `STOP`, `spawn_daemon`, `sleep`, `time_time`, `monotonic`.
* `ui_sink`: the `UiSink` trait the Tauri crate implements, plus `NullSink` and
  `RecordingSink` for tests.
* `engine`: `run_engine`, `apply_live_config`, `config_watcher`, `broadcast_ui_settings`,
  `LaunchGate`, the Shift check, the game launch, the shutdown sequence, signals.
* `compat`: TEMPORARY shims, only for the items whose real crate could not be used when EA
  finished: `AudioController` (beam-audio did not compile; the shim copies the real
  signatures, and **beam-engine's dependency on beam-audio is commented out in its
  Cargo.toml until integration**), and `DrivingAssistant`, `RoadGuidanceFeed`,
  `HillClimbChallengeRecorder`, `DriftAnnouncer` (real types exist; their constructors and
  typed snapshots need adapters in `state.rs`). Everything else (config, gear_report,
  secretstore, ai_registry, road_guidance functions, wheel_report, wheel_slip, camera_lineup,
  audio_debug, fuel_stations, induction_report, poi_guidance, assistant_route, the diagnostic
  recorders, beam-speech) is called from the real crates directly. Wave-2 agents: call real
  crates, and use `crate::compat::beam_audio::controller::AudioController` for audio until
  integration swaps the path. `state::pyjson_to_json` converts beam-road's Python-shaped JSON.
* `engine::after_main_loop` calls `beam_speech::speech::atexit()` once on exit.

## 3. The command senders

All are one throwaway UDP socket to `127.0.0.1:<port>` and `net::send_cmd(port, cmd, what)`
logs `Failed to send {what} via UDP: {e}`. Keep each Python name as a thin wrapper.

| Python | Port | `what` in the log | Returns | Rust (owner) |
|---|---|---|---|---|
| `_send_implement_cmd` | 4470 | `implement proximity command` | None | `listeners::implement::send_implement_cmd` (E3) |
| `_send_scan_cmd` | 4472 | `terrain scan command` | None | `listeners::terrain_scan::send_scan_cmd` (E3) |
| `_send_scanner_cmd` | 4448 | `scanner command` | None | `listeners::scanner::send_scanner_cmd` (E2) |
| `toggle_scan_mode` inline ON/OFF | 4448 | `scanner command` | - | E2 |
| `scanner_listener` COUPLED_DETECTED inline COUPLER_DIST/OFF | 4448 | silent (`except: pass`) -> `net::send_quiet` | - | E2 |
| `_send_fuel_cmd` | 4488 | `fuel station command` | None | `listeners::fuel::send_fuel_cmd` (E2) |
| `_send_obstacle_command` | 4453 | `obstacle detector command` | bool | `listeners::obstacle::send_obstacle_command` (E2) |
| `_send_road_command` | 4463 | `road detector command` | bool | `listeners::road::send_road_command` (E2) |
| `send_camera_command` | 4451 | `camera command {command}` | bool | `listeners::camera::send_camera_command` (E4) |
| `_send_nodegrab_cmd` | 4455 | `node grabber command` | None | `listeners::nodegrab::send_nodegrab_cmd` (E4) |
| `_send_clickspot_cmd` | 4457 | `clickspot command` | None | `listeners::clickspot::send_clickspot_cmd` (E4) |
| `_send_vehicle_bindings_cmd` | 4468 | `vehicle bindings command` | None | `listeners::vehicle_bindings::send_vehicle_bindings_cmd` (E4) |
| `_send_binding_learn_cmd` | 4480 | `binding learn command` | None | `listeners::binding_learn::send_binding_learn_cmd` (E4) |
| `_send_vehicle_info_cmd` | 4478 | `vehicle info command` | None | `listeners::vehicle_info::send_vehicle_info_cmd` (E4) |
| `_send_env_cmd` | 4475 | `environment command` | None | `browsers::send_env_cmd` (E4) |
| `_send_aud_cmd` | 4484 | `audio debug command` | None | `browsers::send_aud_cmd` (E4) |
| `send_console_command` | 4465 | `console command` | None | `console::send_console_command` (E4) |
| `_send_ai_command` | 4449 | `AI command` | None | `ai_layer::send_ai_command` (E7) |
| `_send_slot_command` | 4459 | `slot command` | None | `ai_layer::send_slot_command` (E7) |
| `_send_ui_command` | 4464 | `UI toggle command` | None | `capture::send_ui_command` (E7) |
| F9 Shift+Ctrl+D COUPLER_DIST | 4448 | message is `Failed to send COUPLER_DIST command via UDP: {e}` | - | E6 |
| F9 Shift+V ALIGN | 4448 | `Failed to send ALIGN command via UDP: {e}` | - | E6 |
| F9 Tab NEXT/PREV/CLOSEST | 4448 | `Failed to send b'NEXT' command: {e}` (Python formats the bytes repr, no "via UDP") | - | E6 |
| F9 dumps (DUMP/PDUMP/HDUMP/DAMAGE) and F9 Alt+T INDUCTION | send 4448, receive on a fresh bind of 127.0.0.1:4447 | errors are not caught except timeout | - | E6 / E4 (`speak_induction_report` is in `browsers`) |
| `mcp_server.make_udp_sender` | any | raises `ToolError` | - | E9 |

## 4. Listener specifics (bind address, timeout, buffer, decode)

All bind `127.0.0.1` except telemetry (`0.0.0.0:4444`). None sets `SO_REUSEADDR`.

| Listener | Port | Timeout s | Buffer | Decode | On `OSError` |
|---|---|---|---|---|---|
| `ui_listener` | 4579 | 0.2 | 8192 | utf-8 ignore | **break** (thread ends) |
| `scanner_listener` | 4445 | 0.2 | 65535 | ascii strict, then binary `<ff` fallback | log `Scanner listener socket error.` |
| `obstacle_listener` | 4452 | 0.2 | 1024 | utf-8 ignore | log |
| `road_listener` | 4462 | 0.2 | 65535 | utf-8 ignore | log; timeout runs `check_timeout` |
| `cannon_shot_listener` | 4473 | 0.2 | 2048 | utf-8 ignore | log |
| `camera_lineup_listener` | 4489 | 0.2 | 1024 | utf-8 ignore | log, continue; bind error logged |
| `trailer_angle_listener` | 4476 | 0.2 | 1024 | utf-8 ignore | log |
| `wheel_report_listener` | 4485 | 0.2 | 65535 | utf-8 ignore | log |
| `route_beacon_listener` | 4482 | 0.2 | 1024 | utf-8 ignore | log |
| `terrain_scan_listener` | 4471 | 0.2 | 65535 | utf-8 ignore | log |
| `implement_listener` | 4469 | 0.2 | 2048 | utf-8 ignore | log; timeout re-probes `ON` |
| `camera_listener` | 4450 | 0.2 | 1024 | `DIAG:` ascii replace; else ascii strict | log |
| `nodegrab_listener` | 4454 | **0.05** | 2048 | utf-8 ignore | log; polls GetAsyncKeyState(VK_CONTROL) every pass |
| `clickspot_listener` | 4456 | 0.2 | 2048 | utf-8 ignore | log |
| `vehicle_bindings_listener` | 4467 | 0.2 | 2048 | utf-8 ignore | log |
| `environment_listener` | 4474 | 0.2 | 2048 | utf-8 ignore | log |
| `audio_debug_listener` | 4483 | 0.2 | 4096 | utf-8 ignore | log |
| `vehicle_info_listener` | 4477 | 0.2 | 8192 | utf-8 ignore | log |
| `slot_listener` | 4458 | 0.2 | 4096 | **utf-8 strict, uncaught** (see bugs) | log |
| `binding_learn_listener` | 4479 | 0.2 | 4096 | utf-8 ignore | log |
| `fuel_station_listener` | 4486 | 0.2 | 4096 | utf-8 ignore | log |
| `console_listener` | 4466 | 0.2 | 8192 | utf-8 replace; SO_RCVBUF 1 MiB | **break** |
| `telemetry_loop` | 0.0.0.0:4444 | 0.25 | 2048 | binary | break if stopping, else **re-raise** (kills the engine) |

## 5. Ownership: files per wave-2 workstream

Line counts are the Python lines each workstream ports out of `beamtel.py` (from the ranges
in the table), plus other Python files where named.

| WS | Rust files it owns (in `crates/beam-engine/`) | Python lines |
|---|---|---|
| E1 | `src/telemetry.rs` (+ `tests/` it adds) | ~1,070 (`647-735`, `4306-4316`, `10526-11481`) |
| E2 | `src/listeners/{ui_toast,scanner,obstacle,road,fuel,hill_climb}.rs` | ~1,250 |
| E3 | `src/listeners/{dock_phrases,cannon,lineup,trailer,wheel_report,route_beacon,terrain_scan,implement}.rs` | ~1,780 |
| E4 | `src/listeners/{camera,nodegrab,clickspot,vehicle_bindings,environment,audio_debug,vehicle_info,slots,binding_learn}.rs`, `src/browsers.rs`, `src/console.rs` | ~2,090 |
| E5 | `src/keyboard/` (whole directory), `src/menus.rs`, `src/virtual_browser.rs` | ~1,330 |
| E6 | `src/f9_commands.rs` (+ `rust_plans/f9-command-table.md`) | ~1,190 |
| E7 | `src/ai_layer.rs`, `src/capture.rs`, `src/ai_describer.rs`, `src/win_util.rs` | ~1,150 + `ai_describer.py` non-registry (~520) |
| E8 | `src/vehicle_spawner.rs` (may become `src/vehicle_spawner/` ) | `vehicle_spawner.py` ~2,930 |
| E9 | `src/ui_bridge.rs`, `src/mcp_server.rs` | `nvda_ws_speaker.py` ~450 + `mcp_server.py` ~1,670 |
| E10 | `src/updater.rs`, `src/mod_install.rs` | `updater.py` ~780 + `config_ui.py:41-335` ~300 |
| U2 | `src-tauri/` | `beamtel.py:11489-12128` ~640 + `configurator.py:main` |
| EA (done) | `Cargo.toml`, `src/lib.rs`, `src/listeners/mod.rs`, `src/state.rs`, `src/speech_out.rs`, `src/units.rs`, `src/net.rs`, `src/timer.rs`, `src/ui_sink.rs`, `src/engine.rs`, `src/compat.rs` (temporary) | ~1,900 |

Rules: a wave-2 agent replaces the bodies in its own files and adds private items freely.
It must not change a `pub` signature another workstream calls without telling the
orchestrator. If it needs a new `State` field it asks the orchestrator (EA's file); the
fields that exist are every Python global, so this should not happen. Tests go in the
crate's `tests/` directory named after the sim (`tests/<sim>_sim.rs`), or as `#[cfg(test)]`
modules in the agent's own files.

### Boundary decisions EA made

* **`ui_listener` (port 4579) -> E2** (`listeners/ui_toast.rs`). It is the only listener that
  depends on the driving assistant's toast suppression (`suppressed_toast`), which is E2's
  area alongside the scanner's `ASSISTANT:` routing.
* **`_report_gear_change` -> E1.** It lives in the units section of the Python but only the
  telemetry loop calls it, under the lock.
* **`_clear_next_key_hook` and `_start_next_key_capture` -> E5** (layer lifecycle);
  `_on_next_key_press` and `_invoke_f9_command`/`_mcp_press_command` -> E6; `_F9_HELP` -> E6
  (`f9_commands::F9_HELP`), `_F10_HELP` -> E7 (`ai_layer::F10_HELP`).
* **The browsers built on listener data** (`open_vehicle_bindings_browser`,
  `request_vehicle_info`, page text, the environment browser and its edit/timeout logic,
  `speak_induction_report`, `speak_ambient_temperature`, the audio-debug browsers,
  `open_clickspot_browser`) -> E4 in `browsers.rs`; the listeners' own row parsing stays in
  the listener files (`_parse_env_fields`, `_env_float` in `listeners/environment.rs`).
* **Hill-climb report browsers** (`open_hill_climb_report`, `open_hill_climb_history`) -> E2
  with the rest of the hill-climb glue, not E5.
* **Buffer mode** (`on_buffer_nav_press`, `toggle_buffer_mode`) -> E5 in `virtual_browser.rs`.
* **`toggle_route_beacon` -> E3** (with the route beacon listener), not E2.
* **AI slots** state lives in `state::slots()`; the listener is E4's (`listeners/slots.rs`), the
  readers are E7's.
* **Constructing `driving_assistant`** is EA's (`state::driving_assistant()`); its callbacks
  `_assistant_cue` and `_assistant_arrival` are E7's (`ai_layer::assistant_cue`,
  `ai_layer::assistant_arrival`).
* **`capture_scene_png`** -> E7 (`capture.rs`); MCP's screenshot tool calls it.
* **`console_frame` / `wx.CallAfter(frame.on_console_message, text)`** -> `UiSink::console_message`;
  parsing the record (`BeamTelFrame.on_console_message`, CTX/RESP/OUT/EXECEND/LOG, the
  single-line speak on EXECEND) is U2's, because it mutates window state. U2 must call
  `speech_out::say` for the EXECEND line exactly as the Python does.
* **`_load_console_history` / `_save_console_history`** -> E4 (`console.rs`), called by U2.
* **Updater/mod-installer prompts** go through `UiSink::message_box` and `UiSink::progress_*`.

### Balance and a proposed re-split

E4 (~2,090) and E3 (~1,780) are the largest beamtel.py slices; E8 (`vehicle_spawner.py`,
~2,930 lines) and E9 (~2,120) are larger still. Proposal, if the orchestrator wants
balance:

1. Move `browsers.rs`'s **audio-debug half** (`_aud_*`, `beamtel.py:8006-8240`, ~235 lines)
   and `listeners/audio_debug.rs` (~60) from E4 to **E2** -> E4 ~1,790, E2 ~1,545.
2. Split **E8** in two: the place3d editor (`vehicle_spawner.py:1933-2460`, ~530 lines plus
   its key handlers) as a separate agent, the catalog/modal/screens as the other. Both edit
   one Python module, so this needs `vehicle_spawner` to become a directory module
   (`vehicle_spawner/{mod,place3d}.rs`); EA left it a single file.
3. E9: MCP alone is ~1,670 lines. Acceptable as is (it has its own sim), but if split, the
   tool implementations (`_t_*`, `mcp_server.py:425-1098`) are separable from the JSON-RPC
   server.

## 6. Python bugs and oddities found while mapping (ported as they are)

1. `slot_listener` decodes with strict UTF-8 and catches only `socket.timeout`/`OSError`, so
   a single non-UTF-8 `SLOTS:` datagram raises `UnicodeDecodeError` out of the loop and the
   slot listener dies for the session (silently, in the windowed build).
2. `ui_listener` and `console_listener` end on **any** `OSError` from `recvfrom`
   (`except OSError: break`), including a datagram larger than the buffer (WSAEMSGSIZE) or
   WSAECONNRESET. Toasts / the console then stop for the session.
3. The loading "ready" cue is demoted by its own protection window: `_finish_loading_settle` sets `_speech_protected_until` and then calls `say(..., source="loading_lifecycle")`, which the protection check turns into `interrupt=False`. Harmless (it queues), but probably not intended. (`beamng_renderer` is read with a `"d3d"` default and is absent from beamtel's `DEFAULT_CONFIG`; the configurator writes it, so that is not a bug.)
4. `speed_announce_interval` and `scanner_distance_callout_interval` are taken from the config
   uncoerced. A non-number there raises `TypeError` in `get_speed_bucket` inside the
   telemetry loop (killing the engine thread, and with it the app's telemetry) or in the
   callout thread (killing callouts).
5. The F9 Tab error log reads `Failed to send b'NEXT' command: ...` (bytes repr).
6. `telemetry_loop` re-raises a non-stop `OSError` from `recvfrom`, which ends `_run_engine`
   (its `finally` sets `STOP` and stops audio): one socket error on 4444 shuts the whole
   engine down while the window stays open.
7. `_last_telemetry_ts` is stamped before the packet is validated, so any datagram on 4444
   (a malformed one, a MotionSim-only stream) makes `_world_is_active()` true.
8. `_vinfo_error` is set to `""` on `INFO_BEGIN`/`INFO_END` and to `None` elsewhere; both are
   falsy and only tested for truth, so the Rust uses `Option` (`None` for both).
9. `_apply_live_config` and `_run_engine` pass `cfg.get("drift_speech_enabled", True)`
   uncoerced to `DRIFT_ANNOUNCER.set_enabled`, unlike every other boolean (`bool(...)`).
10. Many globals are written by one thread and read by another without the lock
   (`scan_mode_active`, `dock_mode_active`, the `_env_*`/`_aud_*`/`_fuel_*` blocks, ...). The
   Rust takes the lock for each such access; this is stricter, not different, but it means
   a read can now wait behind the telemetry loop's critical section (sub-millisecond).

## 7. The map

Kinds: `def`, `class`, `global` (module-level assignment; one row per name bound), `stmt`
(other top-level statement), `import`.

| Line | Kind | Python | Rust module | Owner |
|---|---|---|---|---|
| 1 | import | `import math` | lib.rs (imports) | EA |
| 2 | import | `import socket` | lib.rs (imports) | EA |
| 3 | import | `import struct` | lib.rs (imports) | EA |
| 4 | import | `import time` | lib.rs (imports) | EA |
| 5 | import | `import threading` | lib.rs (imports) | EA |
| 6 | import | `import queue` | lib.rs (imports) | EA |
| 7 | import | `import logging` | lib.rs (imports) | EA |
| 8 | import | `import re` | lib.rs (imports) | EA |
| 9-21 | import | `from nvda_ws_speaker import (` | lib.rs (imports) | EA |
| 22 | import | `import signal` | lib.rs (imports) | EA |
| 23 | import | `import os` | lib.rs (imports) | EA |
| 24 | import | `import json` | lib.rs (imports) | EA |
| 25 | import | `import sys` | lib.rs (imports) | EA |
| 26 | import | `import ctypes` | lib.rs (imports) | EA |
| 27 | import | `import ctypes.wintypes` | lib.rs (imports) | EA |
| 28 | import | `from collections import deque` | lib.rs (imports) | EA |
| 29 | import | `from types import SimpleNamespace` | lib.rs (imports) | EA |
| 31 | import | `import wx` | lib.rs (imports) | EA |
| 33 | import | `from bnh_logger import get_logger, LOG_FILENAME` | lib.rs (imports) | EA |
| 34 | import | `from audio import AudioController, DOCK_RAMP_MAX_RANGE_M, SC` | lib.rs (imports) | EA |
| 35-36 | import | `from road_guidance import (RoadGuidanceFeed, junction_phrase` | lib.rs (imports) | EA |
| 37 | import | `from road_diagnostics import RoadDiagnosticRecorder` | lib.rs (imports) | EA |
| 38 | import | `from assistant_route import suppressed_toast` | lib.rs (imports) | EA |
| 39 | import | `from driving_assistant import DrivingAssistant` | lib.rs (imports) | EA |
| 40 | import | `from assistant_diagnostics import AssistantDiagnosticRecorde` | lib.rs (imports) | EA |
| 41-44 | import | `from challenge_results import (` | lib.rs (imports) | EA |
| 45 | import | `from drift_speech import DriftAnnouncer` | lib.rs (imports) | EA |
| 46-53 | import | `from route_beacon import (` | lib.rs (imports) | EA |
| 54 | import | `from poi_guidance import parse_poi_packet, poi_phrase` | lib.rs (imports) | EA |
| 55 | import | `import wheel_slip` | lib.rs (imports) | EA |
| 56 | import | `import wheel_report` | lib.rs (imports) | EA |
| 57 | import | `import audio_debug` | lib.rs (imports) | EA |
| 58 | import | `import fuel_stations` | lib.rs (imports) | EA |
| 59 | import | `import camera_lineup` | lib.rs (imports) | EA |
| 60 | import | `import gear_report` | lib.rs (imports) | EA |
| 61 | import | `import induction_report` | lib.rs (imports) | EA |
| 62 | import | `from gear_report import gear_to_phrase, extended_gear_to_phr` | lib.rs (imports) | EA |
| 64 | global | `logger` | log facade (beam_base::logger) | EA |
| 66 | global | `FROZEN` | engine (paths: beam_base::paths) | EA |
| 67-71 | global | `BASE_DIR` | engine (paths: beam_base::paths) | EA |
| 72 | global | `HERE` | engine (paths: beam_base::paths) | EA |
| 76-82 | def | `_get_config_dir` | engine (beam_base::paths::config_dir/config_path) | EA |
| 85 | global | `CONFIG_DIR` | engine (beam_base::paths::config_dir/config_path) | EA |
| 86 | stmt | `os.makedirs(CONFIG_DIR, exist_ok=True)` | engine (beam_base::paths::config_dir/config_path) | EA |
| 87 | global | `CONFIG_PATH` | engine (beam_base::paths::config_dir/config_path) | EA |
| 88 | global | `ASSISTANT_DIAGNOSTICS` | state (recorder/feed statics) | EA |
| 89-91 | global | `ROAD_DIAGNOSTICS` | state (recorder/feed statics) | EA |
| 92-94 | global | `HILL_CLIMB_CHALLENGE` | state (recorder/feed statics) | EA |
| 97 | global | `DRIFT_ANNOUNCER` | state (recorder/feed statics) | EA |
| 100 | global | `STOP` | timer::STOP | EA |
| 109 | global | `LAUNCH_GATE` | engine | EA |
| 110 | global | `LAUNCH_ALLOWED` | engine | EA |
| 119 | global | `LAUNCH_SUPPRESSED_BY_SHIFT` | engine | EA |
| 121 | global | `VK_LSHIFT` | engine | EA |
| 122 | global | `VK_RSHIFT` | engine | EA |
| 125-137 | def | `_shift_held_at_startup` | engine | EA |
| 140-151 | class | `LaunchGate` | engine | EA |
| 154 | global | `DOM_DUMP_PATH` | engine | EA |
| 157 | global | `SPEECH_LOG_PATH` | speech_out | EA |
| 158 | global | `speech_logging_active` | speech_out | EA |
| 161-169 | def | `_on_dom_dump_received` | engine | EA |
| 172-173 | def | `_handle_sigint` | engine | EA |
| 176 | stmt | `signal.signal(signal.SIGINT, _handle_sigint)` | engine (signals) | EA |
| 177-178 | stmt | `if hasattr(signal, "SIGBREAK"):` | engine (signals) | EA |
| 183-297 | global | `DEFAULT_CONFIG` | beam_base::config (DEFAULT_CONFIG) | B1 |
| 302 | import | `import speech` | speech_out | EA |
| 303 | import | `import secretstore` | speech_out | EA |
| 305 | global | `SPEECH_BUFFER` | speech_out | EA |
| 314 | global | `SPEECH_TAP` | speech_out | EA |
| 315 | global | `_speech_tap_lock` | speech_out | EA |
| 316 | global | `_speech_tap_seq` | speech_out | EA |
| 319-337 | def | `_speech_tap_record` | speech_out | EA |
| 340-358 | def | `get_speech_log` | speech_out | EA |
| 361 | global | `_loading_active` | speech_out | EA |
| 362 | global | `_loading_settling` | speech_out | EA |
| 363 | global | `_loading_focus_text` | speech_out | EA |
| 364 | global | `_loading_pending_vehicle` | speech_out | EA |
| 365 | global | `_loading_generation` | speech_out | EA |
| 366 | global | `_telemetry_baseline_pending` | speech_out | EA |
| 367 | global | `_loading_lock` | speech_out | EA |
| 370-402 | def | `_normalize_speech_value` | speech_out | EA |
| 405-449 | def | `say` | speech_out | EA |
| 452-453 | def | `stop_speech` | speech_out | EA |
| 456-468 | def | `_finish_loading_settle` | speech_out | EA |
| 471-501 | def | `_on_loading_state_changed` | speech_out | EA |
| 504-545 | def | `_write_config` | beam_base::config | B1 |
| 548-563 | def | `_read_config_raw` | beam_base::config | B1 |
| 566-641 | def | `load_config` | beam_base::config | B1 |
| 647 | global | `OG_FORMAT` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 648 | global | `OG_SIZE` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 649 | global | `OG_TURBO` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 657 | global | `EXT_FORMAT_V1` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 658 | global | `EXT_SIZE_V1` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 660 | global | `EXT_FORMAT_V2` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 661 | global | `EXT_SIZE_V2` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 665 | global | `EXT_FORMAT_V3` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 666 | global | `EXT_SIZE_V3` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 669 | global | `EXT_FORMAT_V4` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 670 | global | `EXT_SIZE_V4` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 674 | global | `EXT_FORMAT_V5` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 675 | global | `EXT_SIZE_V5` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 679 | global | `EXT_FORMAT` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 680 | global | `EXT_SIZE` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 684 | global | `EXT_V3_SLIP_DEFAULTS` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 688 | global | `EXT_V4_ELECTRIC_DEFAULTS` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 691 | global | `EXT_V5_DISTANCE_DEFAULTS` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 695 | global | `EXT_V1_IMPLEMENT_DEFAULTS` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 697 | global | `WHEEL_POS_FL` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 698 | global | `WHEEL_POS_FR` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 699 | global | `WHEEL_POS_RL` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 700 | global | `WHEEL_POS_RR` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 701 | global | `WHEEL_POS_F` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 702 | global | `WHEEL_POS_R` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 703 | global | `TELEMETRY_HAS_CLUTCH` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 707 | global | `TELEMETRY_HAS_ENGINE_TEMP` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 708 | global | `TELEMETRY_HAS_ELECTRIC` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 712 | global | `TELEMETRY_HAS_AIR` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 713 | global | `WHEEL_POS_CORNERS` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 717 | global | `EXT_V2_WHEEL_DEFAULTS` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 718 | global | `_ext_version_warned` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 720 | global | `MS_MAGIC` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 721 | global | `MS_FORMAT` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 722 | global | `MS_SIZE` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 725 | global | `DL_SHIFT` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 726 | global | `DL_FULLBEAM` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 727 | global | `DL_HANDBRAKE` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 728 | global | `DL_TC` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 729 | global | `DL_SIGNAL_L` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 730 | global | `DL_SIGNAL_R` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 731 | global | `DL_CHECK` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 732 | global | `DL_OILWARN` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 733 | global | `DL_BATTERY` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 734 | global | `DL_ABS` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 735 | global | `DL_LOWBEAM` | telemetry (formats, DL_*, WHEEL_POS_*, TELEMETRY_HAS_*) | E1 |
| 738 | global | `UI_PORT` | net (ports) | EA |
| 739 | global | `SCANNER_LISTEN_PORT` | net (ports) | EA |
| 742-793 | def | `ui_listener` | listeners/ui_toast | E2 |
| 796-1075 | def | `scanner_listener` | listeners/scanner | E2 |
| 1078-1119 | def | `scanner_callout_thread_fn` | listeners/scanner | E2 |
| 1122-1175 | def | `parse_obstacle_packet` | listeners/obstacle | E2 |
| 1178-1255 | def | `obstacle_listener` | listeners/obstacle | E2 |
| 1258-1477 | def | `road_listener` | listeners/road | E2 |
| 1485 | global | `IMPL_PROX_ENTER_M` | listeners/dock_phrases | E3 |
| 1486 | global | `IMPL_PROX_LEAVE_M` | listeners/dock_phrases | E3 |
| 1487 | global | `IMPL_PROX_RELATION_HOLD` | listeners/dock_phrases | E3 |
| 1488 | global | `IMPL_PROX_INSIDE_HOLD` | listeners/dock_phrases | E3 |
| 1489 | global | `IMPL_PROX_LEAVE_HOLD` | listeners/dock_phrases | E3 |
| 1495 | global | `IMPL_DOCK_LEVEL_M` | listeners/dock_phrases | E3 |
| 1496 | global | `IMPL_DOCK_YAW_DEG` | listeners/dock_phrases | E3 |
| 1502 | global | `IMPL_DOCK_ENTRY_MIN_M` | listeners/dock_phrases | E3 |
| 1503 | global | `IMPL_DOCK_ENTRY_EXIT_M` | listeners/dock_phrases | E3 |
| 1504 | global | `IMPL_ANNOUNCE_PROBE_S` | listeners/dock_phrases | E3 |
| 1510 | global | `RAMP_DOCK_CENTRE_M` | listeners/dock_phrases | E3 |
| 1511 | global | `RAMP_DOCK_SQUARE_DEG` | listeners/dock_phrases | E3 |
| 1512 | global | `RAMP_DOCK_HEADING_ZERO_DEG` | listeners/dock_phrases | E3 |
| 1513 | global | `RAMP_DOCK_TIGHT_M` | listeners/dock_phrases | E3 |
| 1514 | global | `RAMP_DOCK_PITCH_DEG` | listeners/dock_phrases | E3 |
| 1517 | global | `RAMP_SELF_LEVEL_DEG` | listeners/dock_phrases | E3 |
| 1524 | global | `RAMP_SELF_DISTANCE_STROKE_M` | listeners/dock_phrases | E3 |
| 1529 | global | `RAMP_ALIGN_TIGHT_M` | listeners/dock_phrases | E3 |
| 1530 | global | `RAMP_ALIGN_SNUG_M` | listeners/dock_phrases | E3 |
| 1536 | global | `RAMP_ALIGN_LIP_SAY_M` | listeners/dock_phrases | E3 |
| 1537 | global | `CANNON_READOUT_STALE_S` | listeners/dock_phrases | E3 |
| 1553 | global | `RAMP_CORRIDOR_ENTER_M` | listeners/dock_phrases | E3 |
| 1554 | global | `RAMP_CORRIDOR_EXIT_M` | listeners/dock_phrases | E3 |
| 1558 | global | `RAMP_CORRIDOR_MIN_RANGE_M` | listeners/dock_phrases | E3 |
| 1563 | global | `CANNON_SHOT_CENTRE_M` | listeners/dock_phrases | E3 |
| 1566 | global | `CANNON_SHOT_APEX_SAY_M` | listeners/dock_phrases | E3 |
| 1569 | global | `CANNON_SHOT_COMPARE_M` | listeners/dock_phrases | E3 |
| 1574-1578 | global | `_SLAM_CUES` | listeners/dock_phrases | E3 |
| 1580-1584 | global | `_IMPL_RELATION_PHRASE` | listeners/dock_phrases | E3 |
| 1587-1594 | def | `_implement_word` | listeners/dock_phrases | E3 |
| 1597-1613 | def | `_band_name` | listeners/dock_phrases | E3 |
| 1616-1631 | def | `_ramp_bearing_deg` | listeners/dock_phrases | E3 |
| 1634-1645 | def | `_ramp_acquire` | listeners/dock_phrases | E3 |
| 1648-1702 | def | `_ramp_align_phrase` | listeners/dock_phrases | E3 |
| 1705-1715 | def | `_cannon_shot_long_distance` | listeners/dock_phrases | E3 |
| 1718-1774 | def | `_cannon_shot_phrase` | listeners/dock_phrases | E3 |
| 1777-1787 | def | `_dock_phrase` | listeners/dock_phrases | E3 |
| 1790-1906 | def | `_dock_phrase_ramp` | listeners/dock_phrases | E3 |
| 1909-1957 | def | `_parse_ramp_self` | listeners/dock_phrases | E3 |
| 1960-1977 | def | `_ramp_hyd_words` | listeners/dock_phrases | E3 |
| 1980-2002 | def | `_ramp_hyd_strip_namespace` | listeners/dock_phrases | E3 |
| 2005-2060 | def | `_ramp_self_phrase` | listeners/dock_phrases | E3 |
| 2063-2114 | def | `_dock_phrase_impl` | listeners/dock_phrases | E3 |
| 2117-2197 | def | `cannon_shot_listener` | listeners/cannon | E3 |
| 2206 | global | `_lineup_mode` | listeners/lineup | E3 |
| 2207 | global | `_lineup_offset_m` | listeners/lineup | E3 |
| 2208 | global | `_lineup_depth_m` | listeners/lineup | E3 |
| 2209 | global | `_lineup_enabled` | listeners/lineup | E3 |
| 2210 | global | `_lineup_muted` | listeners/lineup | E3 |
| 2211 | global | `_lineup_geom` | listeners/lineup | E3 |
| 2212 | global | `_lineup_stamp` | listeners/lineup | E3 |
| 2213 | global | `_lineup_hooks` | listeners/lineup | E3 |
| 2214 | global | `_lineup_layer_open` | listeners/lineup | E3 |
| 2215 | global | `LINEUP_STALE_S` | listeners/lineup | E3 |
| 2218-2219 | def | `_lineup_in_free_cam` | listeners/lineup | E3 |
| 2222-2235 | def | `_lineup_push` | listeners/lineup | E3 |
| 2238-2275 | def | `camera_lineup_listener` | listeners/lineup | E3 |
| 2278-2284 | def | `_lineup_state_suffix` | listeners/lineup | E3 |
| 2287-2330 | def | `_lineup_on_key` | listeners/lineup | E3 |
| 2333-2340 | def | `_lineup_close_layer` | listeners/lineup | E3 |
| 2343-2371 | def | `open_camera_lineup` | listeners/lineup | E3 |
| 2374-2441 | def | `trailer_angle_listener` | listeners/trailer | E3 |
| 2444-2475 | def | `wheel_report_listener` | listeners/wheel_report | E3 |
| 2478-2539 | def | `route_beacon_listener` | listeners/route_beacon | E3 |
| 2542-2551 | def | `_route_is_set` | listeners/route_beacon | E3 |
| 2554-2572 | def | `_trailer_artic_norm` | listeners/trailer | E3 |
| 2575-2697 | def | `terrain_scan_listener` | listeners/terrain_scan | E3 |
| 2700-3145 | def | `implement_listener` | listeners/implement | E3 |
| 3148-3154 | def | `_send_implement_cmd` | listeners/implement | E3 |
| 3157-3166 | def | `_watch_scan_reply` | listeners/terrain_scan | E3 |
| 3169-3175 | def | `_send_scan_cmd` | listeners/terrain_scan | E3 |
| 3178-3192 | def | `trigger_terrain_scan_driving_only` | listeners/terrain_scan | E3 |
| 3195-3205 | def | `_watch_poi_reply` | listeners/terrain_scan | E3 |
| 3208-3220 | def | `trigger_nearest_poi` | listeners/terrain_scan | E3 |
| 3223-3229 | def | `_send_scanner_cmd` | listeners/scanner | E2 |
| 3232-3253 | def | `_push_gear_direction` | listeners/scanner | E2 |
| 3256-3416 | def | `camera_listener` | listeners/camera | E4 |
| 3419-3571 | def | `nodegrab_listener` | listeners/nodegrab | E4 |
| 3574-3585 | def | `_clickspot_set_pending` | listeners/clickspot | E4 |
| 3588-3599 | def | `_clickspot_set_off` | listeners/clickspot | E4 |
| 3602-3617 | def | `_clickspot_begin_list` | listeners/clickspot | E4 |
| 3620-3649 | def | `_clickspot_append_row` | listeners/clickspot | E4 |
| 3652-3659 | def | `_announce_clickspot_action` | listeners/clickspot | E4 |
| 3662-3784 | def | `clickspot_listener` | listeners/clickspot | E4 |
| 3787-3857 | def | `vehicle_bindings_listener` | listeners/vehicle_bindings | E4 |
| 3860-3948 | def | `environment_listener` | listeners/environment | E4 |
| 3951-4009 | def | `audio_debug_listener` | listeners/audio_debug | E4 |
| 4012-4098 | def | `vehicle_info_listener` | listeners/vehicle_info | E4 |
| 4101-4145 | def | `slot_listener` | listeners/slots | E4 |
| 4151 | global | `UNITS_MODE` | units | EA |
| 4152 | global | `oil_chime_enabled` | state (settings) / units (UNITS_MODE) | EA |
| 4153 | global | `announce_turn_signals` | state (settings) / units (UNITS_MODE) | EA |
| 4154 | global | `announce_speed` | state (settings) / units (UNITS_MODE) | EA |
| 4155 | global | `speed_announce_interval` | state (settings) / units (UNITS_MODE) | EA |
| 4157 | global | `gear_report_settings` | state (settings) / units (UNITS_MODE) | EA |
| 4160 | global | `ui_nav_hold_suppression` | state (settings) / units (UNITS_MODE) | EA |
| 4161 | global | `ai_describer_provider` | state (settings) / units (UNITS_MODE) | EA |
| 4165 | global | `ai_describer_settings` | state (settings) / units (UNITS_MODE) | EA |
| 4166 | global | `ai_describer_disable_ui_toggle` | state (settings) / units (UNITS_MODE) | EA |
| 4167 | global | `MPH_PER_MS` | units | EA |
| 4168 | global | `KMH_PER_MS` | units | EA |
| 4169 | global | `PSI_PER_BAR` | units | EA |
| 4170 | global | `FEET_PER_M` | units | EA |
| 4173-4177 | def | `fmt_speed` | units | EA |
| 4180-4184 | def | `fmt_pressure` | units | EA |
| 4187-4191 | def | `fmt_turbo` | units | EA |
| 4194-4199 | def | `fmt_temp_c_or_f` | units | EA |
| 4202-4214 | def | `fmt_distance` | units | EA |
| 4217 | global | `HP_PER_KW` | units | EA |
| 4218 | global | `LBFT_PER_NM` | units | EA |
| 4219 | global | `MILES_PER_M` | units | EA |
| 4222-4225 | def | `fmt_power_kw` | units | EA |
| 4228-4231 | def | `fmt_torque_nm` | units | EA |
| 4234-4237 | def | `fmt_range` | units | EA |
| 4240-4249 | def | `fmt_trip` | units | EA |
| 4252-4258 | def | `fmt_odometer` | units | EA |
| 4261-4264 | def | `has_air_system` | units | EA |
| 4267-4273 | def | `has_stored_mileage` | units | EA |
| 4276-4280 | def | `has_engine_temp` | units | EA |
| 4283-4286 | def | `has_electric_drive` | units | EA |
| 4289-4290 | def | `fmt_heading` | units | EA |
| 4293-4298 | def | `flip_units` | units | EA |
| 4306-4315 | def | `_report_gear_change` | telemetry | E1 |
| 4318-4327 | def | `get_speed_bucket` | units | EA |
| 4333-4342 | global | `COMPASS_NAMES` | units | EA |
| 4345-4348 | def | `yaw_to_heading_deg` | units | EA |
| 4354 | global | `state_lock` | state::state() | EA |
| 4356 | global | `last_speed_ms` | state | EA |
| 4357 | global | `last_rpm` | state | EA |
| 4358 | global | `last_fuel` | state | EA |
| 4359 | global | `last_turbo` | state | EA |
| 4360 | global | `last_protocol_flags` | state | EA |
| 4361 | global | `last_engtemp` | state | EA |
| 4362 | global | `last_oiltemp` | state | EA |
| 4363 | global | `last_motor_power_kw` | state | EA |
| 4364 | global | `last_motor_torque_nm` | state | EA |
| 4365 | global | `last_ev_range_m` | state | EA |
| 4366 | global | `last_trip_m` | state | EA |
| 4367 | global | `last_odometer_m` | state | EA |
| 4368 | global | `last_lifetime_m` | state | EA |
| 4369 | global | `last_throttle` | state | EA |
| 4370 | global | `last_brake` | state | EA |
| 4371 | global | `last_clutch` | state | EA |
| 4372 | global | `last_steering` | state | EA |
| 4373 | global | `last_actual_steering` | state | EA |
| 4374 | global | `last_steering_input` | state | EA |
| 4375 | global | `last_rpm_max` | state | EA |
| 4376 | global | `last_turbo_max` | state | EA |
| 4377 | global | `last_heading` | state | EA |
| 4378 | global | `last_oil_pressure` | state | EA |
| 4379 | global | `protocol_mode` | state | EA |
| 4380 | global | `compass_highlight_enabled` | state | EA |
| 4381 | global | `compass_highlight_nth_click` | state | EA |
| 4382 | global | `compass_click_interval_deg` | state | EA |
| 4385 | global | `last_air_pressure` | state | EA |
| 4386 | global | `last_air_pressure_max` | state | EA |
| 4391 | global | `IMPL_FLAG_PRESENT` | state | EA |
| 4392 | global | `IMPL_FLAG_ARTIC_VALID` | state | EA |
| 4393 | global | `IMPL_FLAG_GROUND_VALID` | state | EA |
| 4394 | global | `IMPL_FLAG_JACKING` | state | EA |
| 4395 | global | `IMPL_FLAG_DETACHED` | state | EA |
| 4396 | global | `last_implement_flags` | state | EA |
| 4397 | global | `last_implement_edge_height` | state | EA |
| 4398 | global | `last_implement_min_clearance` | state | EA |
| 4399 | global | `last_implement_tilt_deg` | state | EA |
| 4400 | global | `last_implement_tilt_percent` | state | EA |
| 4401 | global | `last_implement_lift` | state | EA |
| 4402 | global | `last_implement_activity` | state | EA |
| 4403 | global | `last_articulation_deg` | state | EA |
| 4414 | global | `last_trailer_deg` | state | EA |
| 4415 | global | `last_trailer_id` | state | EA |
| 4416 | global | `last_trailer_name` | state | EA |
| 4417 | global | `last_trailer_stamp` | state | EA |
| 4423 | global | `TRAILER_FULL_DEG` | state | EA |
| 4424 | global | `TRAILER_STALE_SEC` | state | EA |
| 4430 | global | `last_wheel_rows` | state | EA |
| 4431 | global | `last_wheel_stamp` | state | EA |
| 4432 | global | `WHEEL_REPORT_STALE_SEC` | state | EA |
| 4439 | global | `route_beacon_active` | state | EA |
| 4440 | global | `route_dest_x` | state | EA |
| 4441 | global | `route_dest_y` | state | EA |
| 4446 | global | `route_remaining_m` | state | EA |
| 4447 | global | `route_beacon_stamp` | state | EA |
| 4453 | global | `ROUTE_STALE_SEC` | state | EA |
| 4456 | global | `last_clutch_temp` | state | EA |
| 4457 | global | `last_g_lat` | state | EA |
| 4458 | global | `last_g_lon` | state | EA |
| 4459-4464 | global | `last_tire_pressure_fl` | state | EA |
| 4459-4464 | global | `last_tire_pressure_fr` | state | EA |
| 4459-4464 | global | `last_tire_pressure_rl` | state | EA |
| 4459-4464 | global | `last_tire_pressure_rr` | state | EA |
| 4465-4470 | global | `last_tire_temp_fl` | state | EA |
| 4465-4470 | global | `last_tire_temp_fr` | state | EA |
| 4465-4470 | global | `last_tire_temp_rl` | state | EA |
| 4465-4470 | global | `last_tire_temp_rr` | state | EA |
| 4471-4476 | global | `last_brake_temp_fl` | state | EA |
| 4471-4476 | global | `last_brake_temp_fr` | state | EA |
| 4471-4476 | global | `last_brake_temp_rl` | state | EA |
| 4471-4476 | global | `last_brake_temp_rr` | state | EA |
| 4477 | global | `last_tire_pressure_f` | state | EA |
| 4477 | global | `last_tire_pressure_r` | state | EA |
| 4478 | global | `last_tire_temp_f` | state | EA |
| 4478 | global | `last_tire_temp_r` | state | EA |
| 4479 | global | `last_brake_temp_f` | state | EA |
| 4479 | global | `last_brake_temp_r` | state | EA |
| 4480 | global | `last_telemetry_presence` | state | EA |
| 4481-4485 | global | `last_signal_left_input` | state | EA |
| 4481-4485 | global | `last_signal_right_input` | state | EA |
| 4481-4485 | global | `last_hazard_enabled` | state | EA |
| 4486 | global | `last_lightbar` | state | EA |
| 4487 | global | `last_fog` | state | EA |
| 4490 | global | `status_keyboard_mode_active` | state | EA |
| 4491 | global | `current_status_metric_index` | state | EA |
| 4492 | global | `current_functions_item_index` | state | EA |
| 4493 | global | `current_clickspot_index` | state | EA |
| 4494 | global | `current_accessibility_screen_index` | state | EA |
| 4495 | global | `status_arrow_hooks` | state | EA |
| 4498 | global | `buffer_mode_active` | state | EA |
| 4499 | global | `current_buffer_index` | state | EA |
| 4500 | global | `buffer_key_hooks` | state | EA |
| 4503 | global | `last_click_heading_deg` | state | EA |
| 4504 | global | `compass_click_counter` | state | EA |
| 4507 | global | `last_pos_x` | state | EA |
| 4507 | global | `last_pos_y` | state | EA |
| 4507 | global | `last_pos_z` | state | EA |
| 4507 | global | `last_yaw_rad` | state | EA |
| 4508 | global | `last_compass_ts` | state | EA |
| 4509 | global | `last_announced_compass_idx` | state | EA |
| 4510 | global | `compass_min_interval` | state | EA |
| 4513 | global | `last_roll_rad` | state | EA |
| 4513 | global | `last_pitch_rad` | state | EA |
| 4513 | global | `last_up_z` | state | EA |
| 4514 | global | `inverted` | state | EA |
| 4514 | global | `inverted_announced` | state | EA |
| 4516 | global | `last_bucket` | state | EA |
| 4516 | global | `last_speed_announce_ts` | state | EA |
| 4516 | global | `cooldown_sec` | state | EA |
| 4518 | global | `REVERSE` | state | EA |
| 4519 | global | `NEUTRAL` | state | EA |
| 4520 | global | `last_gear_byte` | state | EA |
| 4520 | global | `last_gear_str` | state | EA |
| 4523 | global | `pedal_tones_active` | state | EA |
| 4524 | global | `scan_mode_active` | state | EA |
| 4525 | global | `coupler_dist_mode` | state | EA |
| 4530 | global | `coupler_run_active` | state | EA |
| 4531 | global | `obstacle_mode_active` | state | EA |
| 4532 | global | `obstacle_warning_sensitivity` | state | EA |
| 4533 | global | `_obstacle_state_last` | state | EA |
| 4534 | global | `_obstacle_state_last_sent` | state | EA |
| 4535 | global | `OBSTACLE_STATE_HEARTBEAT_S` | state | EA |
| 4536 | global | `OBSTACLE_STEERING_DELTA` | state | EA |
| 4537 | global | `OBSTACLE_PEDAL_DELTA` | state | EA |
| 4538 | global | `road_mode_active` | state | EA |
| 4539 | global | `road_follow_guidance_enabled` | state | EA |
| 4540 | global | `road_junction_speech_enabled` | state | EA |
| 4541 | global | `road_stop_sign_speech_enabled` | state | EA |
| 4542 | global | `road_traffic_light_speech_enabled` | state | EA |
| 4543 | global | `road_speed_limit_speech_enabled` | state | EA |
| 4544 | global | `road_surface_speech_enabled` | state | EA |
| 4545 | global | `road_junction_earcon_enabled` | state | EA |
| 4546 | global | `road_include_private` | state | EA |
| 4547 | global | `ROAD_GUIDANCE_FEED` | state::road_guidance_feed() | EA |
| 4548 | global | `_command_context` | state | EA |
| 4551 | global | `heading_guidance_active` | state | EA |
| 4552 | global | `heading_guidance_target` | state | EA |
| 4555 | global | `drift_mode_active` | state | EA |
| 4556 | global | `last_drift_check_ts` | state | EA |
| 4557 | global | `drift_baseline_heading` | state | EA |
| 4558 | global | `drift_alert_active` | state | EA |
| 4559 | global | `drift_pan_direction` | state | EA |
| 4564 | global | `DRIFT_SMOOTH_TAU` | state | EA |
| 4565 | global | `DRIFT_ON_RATE` | state | EA |
| 4566 | global | `DRIFT_OFF_RATE` | state | EA |
| 4567 | global | `DRIFT_RELEASE_HOLD` | state | EA |
| 4568 | global | `DRIFT_PAN_MIN_RATE` | state | EA |
| 4571 | global | `low_speed_mode_active` | state | EA |
| 4574 | global | `slip_mode_active` | state | EA |
| 4575 | global | `last_slip_active` | state | EA |
| 4576 | global | `last_slip_kind` | state | EA |
| 4577 | global | `last_slip_mag` | state | EA |
| 4578 | global | `last_tc_active` | state | EA |
| 4580 | global | `slip_detector` | state (State::tel.slip_detector) | EA |
| 4581 | global | `last_slip_result` | state | EA |
| 4583 | global | `last_wheel_max_driven` | state | EA |
| 4584 | global | `last_wheel_min` | state | EA |
| 4586 | global | `last_vel_x` | state | EA |
| 4587 | global | `last_vel_y` | state | EA |
| 4590 | global | `coord_guidance_active` | state | EA |
| 4591 | global | `marked_coord_x` | state | EA |
| 4592 | global | `marked_coord_y` | state | EA |
| 4593 | global | `coord_target_bearing` | state | EA |
| 4594 | global | `_last_coord_bearing_ts` | state | EA |
| 4595 | global | `_ls_prev_speed_ms` | state | EA |
| 4596 | global | `_ls_prev_speed_ts` | state | EA |
| 4597 | global | `_ls_steady_start_ts` | state | EA |
| 4603 | global | `last_ground_speed_ms` | state | EA |
| 4604 | global | `_ms_last_rx_ts` | state | EA |
| 4605 | global | `_ls_accel_smooth` | state | EA |
| 4606 | global | `_ls_ms_prev_speed_ms` | state | EA |
| 4607 | global | `_ls_ms_prev_ts` | state | EA |
| 4608 | global | `_ls_stopped` | state | EA |
| 4609 | global | `_ls_was_moving` | state | EA |
| 4612 | global | `LS_ACCEL_TAU_S` | state | EA |
| 4613 | global | `LS_ACCEL_MIN_DT` | state | EA |
| 4614 | global | `LS_ACCEL_MAX_DT` | state | EA |
| 4615 | global | `LS_ACCEL_DEADBAND` | state | EA |
| 4616 | global | `LS_STEADY_HOLD_S` | state | EA |
| 4617 | global | `LS_STOP_ENTER_MS` | state | EA |
| 4618 | global | `LS_STOP_EXIT_MS` | state | EA |
| 4619 | global | `LS_STOP_TONE_MIN_MS` | state | EA |
| 4620 | global | `LS_MS_STALE_S` | state | EA |
| 4628 | global | `LOWSPEED_DIAG_INTERVAL_S` | state | EA |
| 4629 | global | `_ls_diag_last_ts` | state | EA |
| 4632 | global | `last_scanner_target_name` | state | EA |
| 4633 | global | `last_scanner_distance` | state | EA |
| 4634 | global | `last_scanner_approach_deg` | state | EA |
| 4635 | global | `last_scanner_bearing` | state | EA |
| 4647 | global | `scanner_ref_reversed` | state | EA |
| 4650 | global | `scanner_distance_callout_enabled` | state | EA |
| 4651 | global | `scanner_distance_callout_interval` | state | EA |
| 4655 | global | `announce_implement_proximity` | state | EA |
| 4656 | global | `announce_cannon_shot` | state | EA |
| 4657 | global | `announce_binding_learn_description` | state | EA |
| 4660 | global | `_implement_word_current` | state | EA |
| 4664 | global | `_last_scan_reply_ts` | state | EA |
| 4667 | global | `_last_poi_reply_ts` | state | EA |
| 4675 | global | `_last_telemetry_ts` | state | EA |
| 4676 | global | `WORLD_ACTIVE_TELEMETRY_S` | state | EA |
| 4679-4681 | def | `_world_is_active` | state | EA |
| 4684-4829 | def | `_mcp_snapshot_state` | state | EA |
| 4832-4840 | def | `_travel_bearing_or_none` | state | EA |
| 4843-4876 | def | `_road_diagnostic_telemetry_snapshot` | state | EA |
| 4881 | global | `dock_mode_active` | state | EA |
| 4882 | global | `last_dock` | state | EA |
| 4885 | global | `last_dock_fail` | state | EA |
| 4889 | global | `cannon_active` | state | EA |
| 4890 | global | `cannon_kind` | state | EA |
| 4894 | global | `last_ramp_self` | state | EA |
| 4897 | global | `last_cannon_aim` | state | EA |
| 4903 | global | `last_dock_mode` | state | EA |
| 4908 | global | `last_dock_name` | state | EA |
| 4915 | global | `last_cannon_shot` | state | EA |
| 4916 | global | `cannon_shot_session` | state | EA |
| 4922 | global | `_last_vehicle_switch_ts` | state | EA |
| 4923 | global | `VEHICLE_SWITCH_QUIET_S` | state | EA |
| 4933 | global | `_speech_protected_until` | state | EA |
| 4934 | global | `SPEECH_PROTECT_S` | state | EA |
| 4937 | global | `free_cam_active` | state | EA |
| 4938 | global | `cam_yaw_deg` | state | EA |
| 4939 | global | `cam_pitch_deg` | state | EA |
| 4940 | global | `cam_agl` | state | EA |
| 4943 | global | `cam_agl_valid` | state | EA |
| 4946 | global | `cam_last_packet_ts` | state | EA |
| 4949 | global | `CAMERA_STALE_SEC` | state | EA |
| 4950 | global | `cam_veh_bearing` | state | EA |
| 4951 | global | `cam_veh_distance` | state | EA |
| 4952 | global | `cam_is_free` | state | EA |
| 4953 | global | `cam_last_click_heading_deg` | state | EA |
| 4954 | global | `cam_compass_click_counter` | state | EA |
| 4955 | global | `cam_last_announced_compass_idx` | state | EA |
| 4956 | global | `cam_last_compass_ts` | state | EA |
| 4959 | global | `nodegrab_mode_active` | state | EA |
| 4960 | global | `nodegrab_scanning` | state | EA |
| 4961 | global | `nodegrab_pad_active` | state | EA |
| 4962 | global | `nodegrab_last_cid` | state | EA |
| 4963 | global | `_nodegrab_space_hook` | state | EA |
| 4964 | global | `_nodegrab_space_held` | state | EA |
| 4967 | global | `clickspot_mode_active` | state | EA |
| 4968 | global | `announce_clickspot_actions` | state | EA |
| 4969 | global | `clickspot_trigger_list` | state | EA |
| 4970 | global | `clickspot_list_loading` | state | EA |
| 4971 | global | `_clickspot_trigger_building` | state | EA |
| 4972 | global | `_clickspot_expected_count` | state | EA |
| 4973 | global | `clickspot_last_hover_id` | state | EA |
| 4981 | global | `_env_rows` | state | EA |
| 4982 | global | `_env_level` | state | EA |
| 4983 | global | `_env_can_change` | state | EA |
| 4984 | global | `_env_building` | state | EA |
| 4985 | global | `_env_unavailable` | state | EA |
| 4989 | global | `_env_ambient_c` | state | EA |
| 4994 | global | `_vinfo_rows` | state | EA |
| 4995 | global | `_vinfo_building` | state | EA |
| 4996 | global | `_vinfo_error` | state | EA |
| 4997 | global | `_vinfo_event` | state | EA |
| 5003 | global | `_vinfo_absent` | state | EA |
| 5011 | global | `_ui_screen_context` | state | EA |
| 5012 | global | `_ui_screen_title` | state | EA |
| 5013 | global | `_page_text_lines` | state | EA |
| 5014 | global | `_page_text_title` | state | EA |
| 5015 | global | `_page_text_error` | state | EA |
| 5016 | global | `_page_text_event` | state | EA |
| 5020 | global | `_page_text_absent` | state | EA |
| 5022 | global | `_vehicle_bindings_list` | state | EA |
| 5023 | global | `_vehicle_bindings_vehicle` | state | EA |
| 5024 | global | `_vehicle_bindings_building` | state | EA |
| 5025 | global | `_vehicle_bindings_building_name` | state | EA |
| 5029 | global | `_binding_learn_active` | state | EA |
| 5032 | global | `_vbrowser_lines` | state | EA |
| 5033 | global | `_vbrowser_index` | state | EA |
| 5034 | global | `_vbrowser_hooks` | state | EA |
| 5035 | global | `_vbrowser_active` | state | EA |
| 5036 | global | `_vbrowser_on_enter` | state | EA |
| 5037 | global | `_vbrowser_on_adjust` | state | EA |
| 5038 | global | `_vbrowser_on_escape` | state | EA |
| 5039 | global | `_vbrowser_entry_data` | state | EA |
| 5041 | global | `audio_controller_ref` | state | EA |
| 5044 | global | `_vehicle_spawner` | state | EA |
| 5049-5057 | stmt | `try: import keyboard (KEYBOARD_OK)` | keyboard/mod.rs (KEYBOARD_OK, command worker) | E5 |
| 5059 | global | `next_key_timer` | state (State::kb) | E5 |
| 5060 | global | `command_timeout_sec` | keyboard/mod.rs (KEYBOARD_OK, command worker) | E5 |
| 5061 | global | `_capture_mods` | state (State::kb) | E5/E6 |
| 5063 | global | `_input_help_mode` | state (State::kb) | E5/E6/E7 |
| 5086 | global | `_command_queue` | keyboard/mod.rs (KEYBOARD_OK, command worker) | E5 |
| 5087 | global | `_command_worker_thread` | keyboard/mod.rs (KEYBOARD_OK, command worker) | E5 |
| 5090-5101 | class | `_SynthKeyEvent` | keyboard/mod.rs (KEYBOARD_OK, command worker) | E5 |
| 5104-5109 | def | `_command_dispatch` | keyboard/mod.rs (KEYBOARD_OK, command worker) | E5 |
| 5112-5123 | def | `_kb_enqueue` | keyboard/mod.rs (KEYBOARD_OK, command worker) | E5 |
| 5126-5136 | def | `_command_worker_loop` | keyboard/mod.rs (KEYBOARD_OK, command worker) | E5 |
| 5139-5146 | def | `_start_command_worker` | keyboard/mod.rs (KEYBOARD_OK, command worker) | E5 |
| 5162 | global | `_kb_layer` | keyboard/mod.rs (layer hook) | E5 |
| 5163 | global | `_kb_layer_hook` | keyboard/mod.rs (layer hook) | E5 |
| 5164 | global | `_kb_layer_release_hook` | keyboard/mod.rs (layer hook) | E5 |
| 5169 | global | `_live_mods` | keyboard/mod.rs (layer hook) | E5 |
| 5171-5182 | global | `_MOD_ALIASES` | keyboard/mod.rs (layer hook) | E5 |
| 5185-5204 | def | `_kb_layer_press` | keyboard/mod.rs (layer hook) | E5 |
| 5207-5214 | def | `_kb_layer_release` | keyboard/mod.rs (layer hook) | E5 |
| 5217-5226 | def | `_kb_run_layer_command` | keyboard/mod.rs (layer hook) | E5 |
| 5229-5241 | def | `_kb_open_layer` | keyboard/mod.rs (layer hook) | E5 |
| 5244-5257 | def | `_kb_close_layer` | keyboard/mod.rs (layer hook) | E5 |
| 5261-5341 | global | `_F9_HELP` | f9_commands (_F9_HELP) | E6 |
| 5344-5362 | global | `_F10_HELP` | ai_layer (_F10_HELP) | E7 |
| 5364-5375 | stmt | `for-loop filling _F10_HELP` | ai_layer (_F10_HELP) | E7 |
| 5378-5381 | stmt | `for-loop filling _F10_HELP` | ai_layer (_F10_HELP) | E7 |
| 5384 | global | `_F10_HELP` | ai_layer (_F10_HELP) | E7 |
| 5385 | global | `_F10_HELP` | ai_layer (_F10_HELP) | E7 |
| 5386 | global | `_F10_HELP` | ai_layer (_F10_HELP) | E7 |
| 5387-5389 | global | `_F10_HELP` | ai_layer (_F10_HELP) | E7 |
| 5390 | global | `_F10_HELP` | ai_layer (_F10_HELP) | E7 |
| 5393-5407 | def | `_clear_next_key_hook` | keyboard/mod.rs | E5 |
| 5411-5418 | def | `_wheel_status_metric` | menus | E5 |
| 5421-5424 | def | `_wheel_report_live` | menus | E5 |
| 5427-5456 | def | `_wheel_rows` | menus | E5 |
| 5459-5464 | def | `_status_rows` | menus | E5 |
| 5467-5504 | global | `_TIRE_PRESSURE_SLOT_ROWS` | menus | E5 |
| 5506-5543 | global | `_BRAKE_TEMP_SLOT_ROWS` | menus | E5 |
| 5546-5547 | def | `_current_speed_limit` | menus | E5 |
| 5550-5555 | def | `_air_pressure_status` | menus | E5 |
| 5558-5737 | global | `STATUS_METRICS` | menus | E5 |
| 5740-5759 | def | `_function_item` | menus | E5 |
| 5765-5962 | global | `FUNCTION_ITEMS` | menus | E5 |
| 5964 | global | `ACCESSIBILITY_SCREENS` | menus | E5 |
| 5965-5974 | global | `ACCESSIBILITY_ACTIONS` | menus | E5 |
| 5978-5994 | def | `_hook_suppressed` | keyboard/mod.rs (suppressed hooks, release_arrow_owners) | E5 |
| 5997-6014 | def | `_unhook_suppressed` | keyboard/mod.rs (suppressed hooks, release_arrow_owners) | E5 |
| 6017-6033 | def | `release_arrow_owners` | keyboard/mod.rs (suppressed hooks, release_arrow_owners) | E5 |
| 6036-6042 | def | `_execute_function_item` | menus | E5 |
| 6045-6063 | def | `_drive_virtual_browser` | menus | E5 |
| 6066-6210 | def | `navigate_accessibility_menu` | menus | E5 |
| 6213-6215 | def | `navigate_status` | menus | E5 |
| 6218-6221 | def | `_on_accessibility_action` | menus | E5 |
| 6224-6240 | def | `on_status_arrow_press` | menus | E5 |
| 6243-6259 | def | `toggle_status_mode` | menus | E5 |
| 6262-6291 | def | `on_buffer_nav_press` | virtual_browser (buffer mode) | E5 |
| 6294-6309 | def | `toggle_buffer_mode` | virtual_browser (buffer mode) | E5 |
| 6312-6329 | def | `_on_vbrowser_nav` | virtual_browser | E5 |
| 6332-6352 | def | `_on_vbrowser_adjust` | virtual_browser | E5 |
| 6355-6362 | def | `_on_vbrowser_escape` | virtual_browser | E5 |
| 6365-6374 | def | `_on_vbrowser_enter` | virtual_browser | E5 |
| 6377-6440 | def | `open_virtual_browser` | virtual_browser | E5 |
| 6443-6459 | def | `close_virtual_browser` | virtual_browser | E5 |
| 6462-6473 | def | `open_hill_climb_report` | listeners/hill_climb | E2 |
| 6476-6494 | def | `open_hill_climb_history` | listeners/hill_climb | E2 |
| 6501 | global | `_fuel_rows` | listeners/fuel | E2 |
| 6502 | global | `_fuel_building` | listeners/fuel | E2 |
| 6503 | global | `_fuel_pending` | listeners/fuel | E2 |
| 6504 | global | `_fuel_timer` | listeners/fuel | E2 |
| 6505 | global | `_fuel_target_id` | listeners/fuel | E2 |
| 6506 | global | `_fuel_target_name` | listeners/fuel | E2 |
| 6507 | global | `_fuel_in_range` | listeners/fuel | E2 |
| 6508 | global | `_fuel_parking` | listeners/fuel | E2 |
| 6509 | global | `FUEL_REPLY_TIMEOUT_S` | listeners/fuel | E2 |
| 6512-6518 | def | `_send_fuel_cmd` | listeners/fuel | E2 |
| 6521-6528 | def | `_fuel_cancel_timer` | listeners/fuel | E2 |
| 6531-6536 | def | `_fuel_arm_timer` | listeners/fuel | E2 |
| 6539-6544 | def | `_fuel_timed_out` | listeners/fuel | E2 |
| 6547-6551 | def | `_fuel_request_list` | listeners/fuel | E2 |
| 6554-6585 | def | `_fuel_open_browser` | listeners/fuel | E2 |
| 6588-6600 | def | `open_fuel_stations` | listeners/fuel | E2 |
| 6603-6629 | def | `_fuel_on_event` | listeners/fuel | E2 |
| 6632-6690 | def | `fuel_station_listener` | listeners/fuel | E2 |
| 6693-6701 | def | `_present_hill_climb_result` | listeners/hill_climb | E2 |
| 6704-6715 | def | `_hill_climb_finalized` | listeners/hill_climb | E2 |
| 6718-6725 | def | `_on_hill_climb_event` | listeners/hill_climb | E2 |
| 6728 | global | `SCANNER_CMD_PORT` | net (ports) | EA |
| 6729 | global | `AI_CMD_PORT` | net (ports) | EA |
| 6730 | global | `SLOT_LISTEN_PORT` | net (ports) | EA |
| 6731-6733 | global | `SLOT_CMD_PORT_OUT` | net (ports) | EA |
| 6734 | global | `CAMERA_LISTEN_PORT` | net (ports) | EA |
| 6735 | global | `CAMERA_CMD_PORT` | net (ports) | EA |
| 6736-6738 | global | `OBSTACLE_LISTEN_PORT` | net (ports) | EA |
| 6739 | global | `OBSTACLE_CMD_PORT` | net (ports) | EA |
| 6740-6742 | global | `NODEGRAB_LISTEN_PORT` | net (ports) | EA |
| 6743 | global | `NODEGRAB_CMD_PORT` | net (ports) | EA |
| 6744-6746 | global | `CLICKSPOT_LISTEN_PORT` | net (ports) | EA |
| 6747 | global | `CLICKSPOT_CMD_PORT` | net (ports) | EA |
| 6748 | global | `ROAD_LISTEN_PORT` | net (ports) | EA |
| 6749 | global | `ROAD_CMD_PORT` | net (ports) | EA |
| 6750 | global | `UI_TOGGLE_CMD_PORT` | net (ports) | EA |
| 6751 | global | `CONSOLE_CMD_PORT` | net (ports) | EA |
| 6752-6754 | global | `CONSOLE_RESP_PORT` | net (ports) | EA |
| 6755-6757 | global | `VEHICLE_BINDINGS_LISTEN_PORT` | net (ports) | EA |
| 6758 | global | `VEHICLE_BINDINGS_CMD_PORT` | net (ports) | EA |
| 6759-6761 | global | `BINDING_LEARN_LISTEN_PORT` | net (ports) | EA |
| 6762 | global | `BINDING_LEARN_CMD_PORT` | net (ports) | EA |
| 6767 | global | `BINDING_LEARN_KEEPALIVE_S` | net (ports) | EA |
| 6768 | global | `ENV_LISTEN_PORT` | net (ports) | EA |
| 6769 | global | `ENV_CMD_PORT` | net (ports) | EA |
| 6770 | global | `AUDIO_DEBUG_LISTEN_PORT` | net (ports) | EA |
| 6771 | global | `AUDIO_DEBUG_CMD_PORT` | net (ports) | EA |
| 6772 | global | `VEHICLE_INFO_LISTEN_PORT` | net (ports) | EA |
| 6773 | global | `VEHICLE_INFO_CMD_PORT` | net (ports) | EA |
| 6774-6776 | global | `IMPLEMENT_LISTEN_PORT` | net (ports) | EA |
| 6777 | global | `IMPLEMENT_CMD_PORT` | net (ports) | EA |
| 6778-6780 | global | `TERRAIN_SCAN_LISTEN_PORT` | net (ports) | EA |
| 6781 | global | `TERRAIN_SCAN_CMD_PORT` | net (ports) | EA |
| 6782-6784 | global | `CANNON_SHOT_LISTEN_PORT` | net (ports) | EA |
| 6787-6789 | global | `TRAILER_ANGLE_LISTEN_PORT` | net (ports) | EA |
| 6790-6792 | global | `ROUTE_BEACON_LISTEN_PORT` | net (ports) | EA |
| 6793-6795 | global | `WHEEL_REPORT_LISTEN_PORT` | net (ports) | EA |
| 6796 | global | `FUEL_LISTEN_PORT` | net (ports) | EA |
| 6797 | global | `FUEL_CMD_PORT` | net (ports) | EA |
| 6798 | global | `CAMERA_LINEUP_LISTEN_PORT` | net (ports) | EA |
| 6806 | global | `CONSOLE_HISTORY_PATH` | console | E4 |
| 6807 | global | `CONSOLE_HISTORY_MAX` | console | E4 |
| 6811 | global | `console_frame` | ui_sink (UiSink::console_message) | EA |
| 6814-6823 | def | `_load_console_history` | console | E4 |
| 6826-6832 | def | `_save_console_history` | console | E4 |
| 6837 | global | `_mcp_console_tap` | console | E4 |
| 6840-6843 | def | `register_console_tap` | console | E4 |
| 6846-6853 | def | `send_console_command` | console | E4 |
| 6856-6898 | def | `console_listener` | console | E4 |
| 6901 | global | `ai_speed_limit_ms` | state (AI, slots, _slots_lock) | EA |
| 6902 | global | `ai_avoid_mode` | state (AI, slots, _slots_lock) | EA |
| 6903 | global | `ai_lane_driving` | state (AI, slots, _slots_lock) | EA |
| 6906 | global | `_vehicle_slots` | state (AI, slots, _slots_lock) | EA |
| 6907 | global | `_selected_slots` | state (AI, slots, _slots_lock) | EA |
| 6908 | global | `_target_slot` | state (AI, slots, _slots_lock) | EA |
| 6909 | global | `_pending_target_confirm` | state (AI, slots, _slots_lock) | EA |
| 6910 | global | `_slots_lock` | state (AI, slots, _slots_lock) | EA |
| 6915 | global | `_ai_vehicle_commands` | state (AI, slots, _slots_lock) | EA |
| 6918-6963 | def | `toggle_scan_mode` | listeners/scanner | E2 |
| 6966-6974 | def | `_send_obstacle_command` | listeners/obstacle | E2 |
| 6977-7007 | def | `_push_obstacle_state` | listeners/obstacle | E2 |
| 7010-7014 | def | `_send_obstacle_configuration` | listeners/obstacle | E2 |
| 7017-7035 | def | `toggle_obstacle_mode` | listeners/obstacle | E2 |
| 7038-7046 | def | `_send_road_command` | listeners/road | E2 |
| 7049-7050 | def | `_send_road_configuration` | listeners/road | E2 |
| 7053-7061 | def | `assistant_diagnostic_control` | listeners/road | E2 |
| 7064-7094 | def | `road_diagnostic_control` | listeners/road | E2 |
| 7097-7102 | def | `speak_road_status` | listeners/road | E2 |
| 7105-7122 | def | `toggle_road_mode` | listeners/road | E2 |
| 7125-7156 | def | `toggle_route_beacon` | listeners/route_beacon | E3 |
| 7159-7177 | def | `_format_camera_diag` | listeners/camera | E4 |
| 7180-7189 | def | `send_camera_command` | listeners/camera | E4 |
| 7192-7207 | def | `_camera_feed_problem` | listeners/camera | E4 |
| 7210-7219 | def | `send_camera_diag` | listeners/camera | E4 |
| 7222-7236 | def | `toggle_free_cam_info` | listeners/camera | E4 |
| 7239-7245 | def | `_send_nodegrab_cmd` | listeners/nodegrab | E4 |
| 7248-7265 | def | `_nodegrab_on_space` | listeners/nodegrab | E4 |
| 7268-7284 | def | `_install_nodegrab_hooks` | listeners/nodegrab | E4 |
| 7287-7292 | def | `_remove_nodegrab_hooks` | listeners/nodegrab | E4 |
| 7295-7311 | def | `toggle_nodegrab_mode` | listeners/nodegrab | E4 |
| 7314-7320 | def | `_send_clickspot_cmd` | listeners/clickspot | E4 |
| 7323-7333 | def | `_activate_clickspot` | listeners/clickspot | E4 |
| 7336-7339 | def | `_clickspot_browser_on_enter` | listeners/clickspot | E4 |
| 7342-7356 | def | `toggle_clickspot_mode` | listeners/clickspot | E4 |
| 7359-7367 | def | `_send_vehicle_bindings_cmd` | listeners/vehicle_bindings | E4 |
| 7370-7376 | def | `_send_binding_learn_cmd` | listeners/binding_learn | E4 |
| 7379-7390 | def | `toggle_binding_learn_mode` | listeners/binding_learn | E4 |
| 7393-7405 | def | `binding_learn_keepalive_thread_fn` | listeners/binding_learn | E4 |
| 7408-7468 | def | `_binding_learn_phrase` | listeners/binding_learn | E4 |
| 7471-7542 | def | `binding_learn_listener` | listeners/binding_learn | E4 |
| 7545-7551 | def | `_vehicle_bindings_on_enter` | browsers | E4 |
| 7554-7570 | def | `open_vehicle_bindings_browser` | browsers | E4 |
| 7573-7591 | def | `_parse_env_fields` | listeners/environment | E4 |
| 7594-7600 | def | `_send_vehicle_info_cmd` | listeners/vehicle_info | E4 |
| 7603-7623 | def | `_vinfo_render` | browsers | E4 |
| 7626-7661 | def | `request_vehicle_info` | browsers | E4 |
| 7664-7668 | def | `_on_screen_context` | browsers | E4 |
| 7671-7682 | def | `_on_page_text` | browsers | E4 |
| 7685-7708 | def | `request_page_text` | browsers | E4 |
| 7711-7736 | def | `_read_mod_details_page` | browsers | E4 |
| 7739-7760 | def | `open_page_text_browser` | browsers | E4 |
| 7763-7769 | def | `_send_env_cmd` | browsers | E4 |
| 7772-7776 | def | `_env_float` | browsers | E4 |
| 7779-7783 | def | `_env_temp_from_display` | browsers | E4 |
| 7786-7820 | def | `_env_row_line` | browsers | E4 |
| 7823-7858 | def | `speak_induction_report` | browsers | E4 |
| 7861-7872 | def | `speak_ambient_temperature` | browsers | E4 |
| 7877 | global | `_env_awaiting_change` | browsers | E4 |
| 7878 | global | `_env_change_timer` | browsers | E4 |
| 7879 | global | `_env_browser_open` | browsers | E4 |
| 7882-7889 | def | `_env_change_timed_out` | browsers | E4 |
| 7892-7912 | def | `_env_notify_refresh` | browsers | E4 |
| 7915-7925 | def | `_env_arm_change` | browsers | E4 |
| 7928-7952 | def | `_env_on_adjust` | browsers | E4 |
| 7955-7972 | def | `_env_on_enter` | browsers | E4 |
| 7975-8003 | def | `open_environment_browser` | browsers | E4 |
| 8010 | global | `_aud_rows` | browsers | E4 |
| 8011 | global | `_aud_building` | browsers | E4 |
| 8012 | global | `_aud_sfx_rows` | browsers | E4 |
| 8013 | global | `_aud_sfx_building` | browsers | E4 |
| 8014 | global | `_aud_sfx_head` | browsers | E4 |
| 8015 | global | `_aud_sfx_stack` | browsers | E4 |
| 8016 | global | `_aud_pending` | browsers | E4 |
| 8017 | global | `_aud_pending_start` | browsers | E4 |
| 8018 | global | `_aud_awaiting` | browsers | E4 |
| 8019 | global | `_aud_timer` | browsers | E4 |
| 8020 | global | `_aud_browser` | browsers | E4 |
| 8023-8029 | def | `_send_aud_cmd` | browsers | E4 |
| 8032-8039 | def | `_aud_cancel_timer` | browsers | E4 |
| 8042-8049 | def | `_aud_timed_out` | browsers | E4 |
| 8052-8064 | def | `_aud_send` | browsers | E4 |
| 8067-8076 | def | `_aud_on_message` | browsers | E4 |
| 8079-8080 | def | `_aud_engine_lines` | browsers | E4 |
| 8083-8096 | def | `_aud_open_engine_browser` | browsers | E4 |
| 8099-8122 | def | `_aud_on_rows` | browsers | E4 |
| 8125-8144 | def | `_aud_engine_adjust` | browsers | E4 |
| 8147-8172 | def | `_aud_engine_enter` | browsers | E4 |
| 8175-8177 | def | `_aud_open_sfx` | browsers | E4 |
| 8180-8204 | def | `_aud_on_sfx_rows` | browsers | E4 |
| 8207-8219 | def | `_aud_sfx_enter` | browsers | E4 |
| 8222-8233 | def | `_aud_sfx_escape` | browsers | E4 |
| 8236-8238 | def | `open_audio_debug_browser` | browsers | E4 |
| 8241-8256 | def | `open_clickspot_browser` | browsers | E4 |
| 8259-8284 | def | `_invoke_f9_command` | f9_commands | E6 |
| 8287-8289 | def | `_mcp_press_command` | f9_commands | E6 |
| 8292-9352 | def | `_on_next_key_press` | f9_commands | E6 |
| 9355-9363 | def | `_start_next_key_capture` | keyboard/mod.rs | E5 |
| 9369-9379 | global | `_AGGRESSION_MAP` | ai_layer | E7 |
| 9380 | global | `_AVOID_CYCLE` | ai_layer | E7 |
| 9382 | global | `ai_timer` | ai_layer | E7 |
| 9383 | global | `_ai_mods` | ai_layer | E7 |
| 9386-9392 | def | `_send_ai_command` | ai_layer | E7 |
| 9395-9405 | def | `_assistant_cue` | ai_layer | E7 |
| 9408-9414 | def | `_assistant_arrival` | ai_layer | E7 |
| 9417-9424 | global | `driving_assistant` | state::driving_assistant() | EA |
| 9427-9433 | def | `_send_slot_command` | ai_layer | E7 |
| 9436-9443 | def | `_send_ui_command` | capture | E7 |
| 9451 | global | `UI_HIDE_SETTLE_S` | capture | E7 |
| 9459 | global | `_capture_lock` | capture | E7 |
| 9466 | global | `GAME_FOCUS_SETTLE_S` | capture | E7 |
| 9468 | global | `_GAME_WINDOW_HINT` | capture | E7 |
| 9471-9503 | def | `_find_game_window` | capture | E7 |
| 9506-9535 | def | `_raise_game_window` | capture | E7 |
| 9538-9550 | def | `_window_region` | capture | E7 |
| 9553-9601 | def | `capture_scene_png` | capture | E7 |
| 9607 | global | `_describer_lock` | ai_describer | E7 |
| 9608 | global | `_describer_busy` | ai_describer | E7 |
| 9611-9683 | def | `_ai_describe_worker` | ai_describer | E7 |
| 9686-9701 | def | `_trigger_ai_describe` | ai_describer | E7 |
| 9704-9706 | def | `_record_ai_command` | ai_layer | E7 |
| 9711 | global | `_police_lights_active` | ai_layer | E7 |
| 9714-9719 | def | `_set_lightbars` | ai_layer | E7 |
| 9722-9728 | def | `_clear_police_lights` | ai_layer | E7 |
| 9731-9764 | def | `_speak_status_all_response` | ai_layer | E7 |
| 9767-9809 | def | `_speak_ai_status` | ai_layer | E7 |
| 9812-9824 | def | `_reset_ai_timer` | ai_layer | E7 |
| 9827-9829 | def | `_slot_key_label` | ai_layer | E7 |
| 9832-9853 | def | `_toggle_slot_selection` | ai_layer | E7 |
| 9856-9866 | def | `_select_all_slots` | ai_layer | E7 |
| 9869-9903 | def | `_set_target_slot` | ai_layer | E7 |
| 9906-9948 | def | `_dispatch_ai_mode` | ai_layer | E7 |
| 9951 | global | `_PRESET_DELAY_SEC` | ai_layer | E7 |
| 9954-9963 | def | `_run_staggered` | ai_layer | E7 |
| 9966-9978 | def | `_preset_motorcade` | ai_layer | E7 |
| 9981-10000 | def | `_preset_gangup` | ai_layer | E7 |
| 10003-10015 | def | `_preset_tod` | ai_layer | E7 |
| 10018-10040 | def | `_preset_tod_spectator` | ai_layer | E7 |
| 10043-10071 | def | `_preset_police` | ai_layer | E7 |
| 10074-10093 | def | `_trigger_preset` | ai_layer | E7 |
| 10096-10111 | def | `_clear_ai_hook` | ai_layer | E7 |
| 10114-10307 | def | `_on_ai_key_press` | ai_layer | E7 |
| 10310-10314 | def | `_start_ai_capture` | ai_layer | E7 |
| 10325 | global | `_user32` | win_util | E7 |
| 10327 | global | `_DPI_CTX_PER_MONITOR_V2` | win_util | E7 |
| 10328-10330 | global | `_WNDENUMPROC` | win_util | E7 |
| 10333-10350 | def | `_set_thread_dpi_aware` | win_util | E7 |
| 10353-10359 | def | `_restore_thread_dpi` | win_util | E7 |
| 10362 | global | `_GAME_WINDOW_CLASS` | win_util | E7 |
| 10363 | global | `_PROCESS_QUERY_LIMITED_INFORMATION` | win_util | E7 |
| 10366-10390 | def | `_window_process_name` | win_util | E7 |
| 10393-10436 | def | `_find_beamng_window` | win_util | E7 |
| 10439-10467 | def | `warp_cursor_to_viewport` | win_util | E7 |
| 10470-10478 | def | `_is_beamng_focused` | win_util | E7 |
| 10481-10520 | def | `install_hotkeys` | keyboard/mod.rs (install_hotkeys) | E5 |
| 10526-11481 | def | `telemetry_loop` | telemetry | E1 |
| 11489-11507 | class | `WxLogHandler` | src-tauri (log sink; ui_sink::UiSink::append_log) | U2 |
| 11515-12128 | class | `BeamTelFrame` | src-tauri (window, console pane, logs menu, close) | U2 |
| 12136-12147 | def | `_broadcast_ui_settings` | engine | EA |
| 12150-12237 | def | `_apply_live_config` | engine | EA |
| 12240-12253 | def | `_config_watcher` | engine | EA |
| 12256-12616 | def | `_run_engine` | engine | EA |
| 12624-12638 | def | `_run_update_flow` | engine::run_update_flow (+ updater) | EA/E10 |
| 12641-12671 | def | `main` | engine::main_sequence (+ src-tauri main) | EA/U2 |
| 12674-12675 | stmt | `if __name__ == "__main__":` | engine (+ src-tauri for the wx parts) | EA |


## 8. Value types and conversions (I0)

Added by I0 when beam-engine was wired to the real crates. **`compat.rs` is gone**: section 2's
paragraph about it is history. Every item it stood in for is now the real one:
`beam_audio::controller::AudioController` (and `beam_audio::scan::*`,
`beam_audio::controller::{TelemetryState, HazardIn, JunctionProximitySnapshot,
RoadGuidanceResult, JsonMap}`), `beam_road::driving_assistant::DrivingAssistant`,
`beam_road::road_guidance::RoadGuidanceFeed`,
`beam_road::challenge_results::HillClimbChallengeRecorder` and
`beam_reports::drift_speech::DriftAnnouncer`. `ConfigMap` moved to `crate::convert`.

### Reaching the module-level objects

| Python | Rust | Notes |
|---|---|---|
| `audio_controller_ref` | `state::audio_controller_ref()` -> `Option<Arc<beam_audio::AudioController>>` | listeners get their own `Arc` from `run_engine`. Methods take `&self` except `apply_config`, `start`, `restart_audio_stream` (`self: &Arc<Self>`) |
| `driving_assistant` | `state::driving_assistant()` -> `&'static DrivingAssistant` | `.active()`, `.snapshot().following_route`, `.seconds_since_arrival()`, `.receive(payload)?`, `.toggle()`, `.status()`. Built with `ai_layer::send_ai_command`, `speech_out::say` (interrupt default True, source "python"), `ai_layer::assistant_cue`, `ASSISTANT_DIAGNOSTICS`, `units::units_mode`, `ai_layer::assistant_arrival` |
| `ROAD_GUIDANCE_FEED` | `state::road_guidance_feed()` -> `&'static RoadGuidanceFeed` | typed: `accept_r2(&R2Packet, ..)`, `speed_limit(enabled)`, `snapshot()` (a `FeedSnapshot`; `.to_value()` is the Python dict) |
| `HILL_CLIMB_CHALLENGE` | `state::hill_climb_challenge()` -> `&'static HillClimbChallengeRecorder` | everything in and out is `beam_road::pyjson::Value`; the finalize callback is `listeners::hill_climb::hill_climb_finalized(&PyJson, bool)`. Methods that raise in Python return `Result<_, beam_road::pyjson::PyErr>` |
| `DRIFT_ANNOUNCER` | `state::drift_announcer()` -> `&'static DriftAnnouncer` | `.quiet()`, `.set_enabled(bool)`, `.handle(&beam_reports::pyops::PyValue) -> Result` |
| `ASSISTANT_DIAGNOSTICS` / `ROAD_DIAGNOSTICS` | `state::assistant_diagnostics()` / `state::road_diagnostics()` | `.status()` is a `beam_road::pyjson::Value` |

### Which value type each dependency API takes

| Type (alias in `crate::convert`) | Crate | APIs that take or return it |
|---|---|---|
| `PyVal` = `beam_base::pyval::PyValue` (dicts: `PyDict`) | beam-base | the config loaders, `AudioController::apply_config(&PyDict)`, `gear_report::settings_from_config`, `secretstore`, `speech_config` |
| `PyOps` = `beam_reports::pyops::PyValue` | beam-reports | `DriftAnnouncer::handle`, `drift_phrase`, and the report modules that take a decoded dict (`json_loads` there returns it) |
| `PyJson` = `beam_road::pyjson::Value` | beam-road | the driving assistant (vehicle, arrival, snapshots), road guidance `to_value()`s, the hill-climb recorder (events, telemetry rows, summaries, `list_reports`, `report_lines`), both diagnostic recorders |
| `serde_json::Value` | - | the engine's own JSON: UI bridge payloads and callbacks (`ui_bridge::JsonCallback`), the MCP snapshot, `ConfigMap`, `beam_speech::speech::configure/init`, beam-audio's `JsonMap` targets |

### The conversions (all in `crate::convert`, all tested there)

| From -> to | Function |
|---|---|
| `PyVal` <-> JSON | `pyval_to_json`, `json_to_pyval`; dicts: `pydict_to_config_map`, `config_map_to_pydict` |
| `PyJson` <-> JSON | `pyjson_to_json` (also re-exported as `state::pyjson_to_json`), `json_to_pyjson` |
| `PyOps` <-> JSON | `pyops_to_json`, `json_to_pyops` |
| `PyVal` -> `PyJson` / `PyOps` (a config value handed to beam-road / beam-reports) | `pyval_to_pyjson`, `pyval_to_pyops` |
| `PyJson` <-> `PyOps` | `pyjson_to_pyops`, `pyops_to_pyjson` |
| typed snapshots -> JSON | `junction_proximity_snapshot_json` (audio.py's dict: `target` None when empty), `assistant_snapshot_json`, `feed_snapshot_json` |
| a float -> JSON | `json_float` (NaN/inf -> null) |

Rules: conversions between the three Python types are lossless (big ints, NaN, inf, key
order). Conversions to JSON lose only what JSON cannot hold (NaN/inf -> `null`, ints beyond
`u64` -> nearest float). When the original is text off the wire, decode it with the target
crate's own loader (`beam_road::pyjson::loads`, `beam_reports::pyops::json_loads`,
`beam_base::pyval::json_loads`) rather than going through `serde_json`. Do not write another
converter in a wave-2 file; add it here with a test.