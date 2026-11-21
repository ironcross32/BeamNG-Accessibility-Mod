# Audit: listeners (V1, slice 4)

2026-11-21. Python: every `*_listener` in `beamtel.py` and what they call. Rust:
`beam-engine/src/listeners/`, `console.rs`. Audited by the orchestrator, which did not
write E2-E4.

## Method

1. **Coverage of the real listeners under every fixture generator.** All generators
   were run under `coverage` together, and the executed lines were attributed per
   function (`ast` over `beamtel.py`). This shows how much of each real Python listener
   the fixtures drive, all of which the Rust tests replay. The scripts used:

   ```
   uv run --with coverage python -m coverage run --append --include=beamtel.py <each generator>
   uv run --with coverage python -m coverage json -o allcov.json
   ```

2. Every missed line that is not error handling or shutdown was walked against the Rust.
3. The Python change made after the port began (`road_listener`'s gate and private
   road words) was traced, and given a fixture.
4. `diagnostic/traffic_mode_sim.py`, the one Python sim never ported, is now ported.

## Coverage of the real Python listeners (before this audit's additions)

| Listener | Statements run | Listener | Statements run |
|---|---|---|---|
| ui_listener | 89% | camera_listener | 91% |
| scanner_listener | 95% | nodegrab_listener | 85% |
| obstacle_listener | 89% | clickspot_listener | 93% |
| road_listener | 89% (now higher) | vehicle_bindings_listener | 84% |
| cannon_shot_listener | 83% | environment_listener | 89% |
| camera_lineup_listener | 61% | audio_debug_listener | 85% |
| trailer_angle_listener | 86% | vehicle_info_listener | 87% |
| wheel_report_listener | 70% | slot_listener | 88% |
| route_beacon_listener | 84% | fuel_station_listener | 86% |
| terrain_scan_listener | 90% | binding_learn_listener | 87% |
| implement_listener | 97% | console_listener | 3% (driven in Rust by `mcp_server_sim`) |

The low figures are almost all socket timeout, socket error and shutdown handling.

## Added

- **`road_scripts`** in `fixtures/gen/e2_listeners.py` and `e2_listeners_fixtures.rs`.
  The fake socket now runs a callable queued between datagrams, so a script can toggle
  road mode, switch the assistant, change a preference or start loading between packets,
  through the real Python listener. Two scripts:
  - **traffic mode handoffs**: `traffic_mode_sim.py` step for step. Signal speech
    follows whichever of road mode and the assistant is on, is not repeated across a
    handoff, honours both preferences, and is held during loading.
  - **gate and private entry**: "Gate ahead. Stop at the bar" and "Private road ahead"
    once per id, only while the assistant drives (the post-base Python change).
- `DrivingAssistant::set_active_for_tests`, the Rust counterpart of the fixtures
  assigning `driving_assistant.active`.

## Walked by hand (no fixture reaches them)

| Python | Rust | State |
|---|---|---|
| `scanner_listener` `AI_STATUS_ALL:` | `scanner.rs:413` -> `ai_layer` | equivalent |
| `road_listener` two-way orientation chime, junction speech without a signal, junction earcon when proximity is off, entry earcon, speed limit, feed timeout | `road.rs` `r2_packet`, `road_timeout` | equivalent (conditions, interrupt flags and sources) |
| `camera_listener` heading delta wrap | `camera.rs:108` | equivalent |
| `nodegrab_listener` Ctrl polling: SCAN_ON + SNAP on press, SCAN_OFF on release, reset when the mode is off | `nodegrab.rs` `poll_ctrl` | equivalent; Python's `snap_sent_this_press` is written but never read, so the Rust drops it |
| `console_listener` (1 MB receive buffer, 127.0.0.1, 8192 bytes, tap then window) | `console.rs` | equivalent |
| Timeouts, socket errors, `close()` in every listener | `net::UdpListener` | equivalent (shared helper, 127.0.0.1 except telemetry's 0.0.0.0) |

## Python bugs

- **62 (new)**: the road listener records the last gate and private-entry ids on every
  packet, announced or not. So one first seen while the assistant is off, or during
  loading, is never announced once the assistant takes over. Reproduced (R); in
  `python-bugs.md` for the user's decision.

## Open

None besides bug 62's decision.
