# F9 command table (E6)

The audit artefact for `_on_next_key_press` (`beamtel.py:8292-9352`) and its Rust port,
`beam_engine::f9_commands::on_next_key_press`. Line numbers are the working-tree
`beamtel.py` on `rust-port` when E6 ran. The branch chain is ordered, and several branches
test only some modifiers, so an earlier, looser branch answers combinations a later one
names; the matrix at the bottom resolves every key and modifier combination.

Golden test: `crates/beam-engine/tests/f9_commands.rs` over
`fixtures/data/f9_commands.json` (generator `fixtures/gen/f9_commands.py`):
every key below plus `/ ? f9 enter f1`, modifier names and an upper-case name, in all eight
modifier combinations over two states, and targeted cases for each branch's internals.

## Entry points

| Python | Line | Rust |
|---|---|---|
| `_invoke_f9_command(name, ctrl, shift, alt)` | 8259 | `invoke_f9_command` (Err = `RuntimeError("audio controller is not up yet")`; mods and command context restored by a drop guard, panics included) |
| `_mcp_press_command(...)` | 8287 | `mcp_press_command` |
| `_on_next_key_press(event, audio_controller)` | 8292 | `on_next_key_press` |
| `_F9_HELP` | 5261 | `F9_HELP` (66 entries, Python insertion order) |

## Pre-dispatch (before the state snapshot)

| ID | Key | Modifiers | Action | Line | Rust |
|---|---|---|---|---|---|
| P0 | any | key-up event | ignored | 8295 | `on_next_key_press` |
| P1 | `ctrl` `control` `left ctrl` `right ctrl` / `shift` `left shift` `right shift` / `alt` `left alt` `right alt` | any | set that `_capture_mods` flag, return | 8299-8307 | `on_next_key_press` |
| P2 | `f9` | any | return (layer stays open) | 8309 | `on_next_key_press` |
| P3 | `/` `?` | any | toggle input help; on: "Input help on", cancel timeout; off: "Input help off", clear layer | 8313 | `on_next_key_press` |
| P4 | any | help mode on | speak `_F9_HELP[(key, mods)]`; an unmodified digit says "Speak buffer message N" (0 = 10); else "No command" | 8329 | `on_next_key_press` |
| D | `0`-`9` | none | replay `SPEECH_BUFFER[-n]` (0 = 10) or "No message", clear layer | 8351 | `on_next_key_press` |

Then the state snapshot (`8364-8391`, Rust `snapshot()`), which raises on a NaN speed,
RPM or temperature exactly where `int(round(nan))` does.

## Alt camera block (each clears the layer and returns)

| ID | Key | Modifiers | Action | Line | Rust |
|---|---|---|---|---|---|
| A1 | g | A only | clear, `open_camera_lineup()` | 8394 | `on_next_key_press` -> `lineup::open_camera_lineup` |
| A2 | f | A only | `toggle_free_cam_info(ac)`, clear | 8402 | -> `camera::toggle_free_cam_info` |
| A3 | h | A only | camera heading or `_camera_feed_problem()` | 8410 | `on_next_key_press` |
| A4 | a | A only | camera altitude (feet/meters, "above sea level, ground unknown") | 8422 | `on_next_key_press` |
| A5 | a | S+A, not C | `send_camera_diag()` | 8444 | -> `camera::send_camera_diag` |
| A6 | p | A only | camera pitch up/down | 8455 | `on_next_key_press` |
| A7 | v | A only | vehicle bearing from camera / "No vehicle" | 8471 | `on_next_key_press` |
| A8 | d | A only | vehicle distance from camera / "No vehicle" | 8490 | `on_next_key_press` |

## Main chain (all fall through to `_clear_next_key_hook(False)` at 9352)

| ID | Key | Modifiers (as tested) | Action | Line | Rust |
|---|---|---|---|---|---|
| M1 | s | not C | speed + `ROAD_GUIDANCE_FEED.speed_limit_phrase(road_mode_active or assistant, units, "Speed limit")` | 8512 | `on_next_key_press` |
| M2 | r | S, not C | "Redline N RPM" (extended) / "Unavailable" | 8517 | `on_next_key_press` |
| M3 | r | C+S, not A | `speak_road_status()` | 8523 | -> `road::speak_road_status` |
| M4 | r | C only | `toggle_road_mode(ac)` | 8530 | -> `road::toggle_road_mode` |
| M5 | r | none | "N rpm" | 8532 | `on_next_key_press` |
| M6 | h | C | toggle heading guidance (target = heading) | 8534 | `on_next_key_press` |
| M7 | d | C+S, not A | toggle `coupler_dist_mode`, send `COUPLER_DIST` to 4448 (needs scanner) | 8543 | `on_next_key_press` (`scanner_sendto`) |
| M8 | d | C only | toggle drift detection | 8560 | `on_next_key_press` |
| M9 | d | S only | scanner relative bearing | 8580 | `on_next_key_press` |
| M10 | d | none | scanner distance, orientation, side | 8596 | `on_next_key_press` |
| M11 | i | none | cane tap: old cannon aim / cannon inclination / dock phrase / deck phrase / instrument off / reason / nothing | 8638 | `cane_tap`, `old_cannon_readout` |
| M12 | i | S only | cycle reference band (`BANDNEXT`, sleep 0.15, read back) | 8739 | `cycle_reference_band` |
| M13 | i | C only | docking instrument toggle (`ON`, `DOCK_ON/OFF`, `set_dock_mode`, sleep 0.3, `BANDAUTO`) | 8776 | `toggle_docking_instrument` |
| M14 | w | C only | `toggle_route_beacon(ac)` | 8829 | -> `route_beacon::toggle_route_beacon` |
| M15 | w | S only | `trigger_nearest_poi()` | 8835 | -> `terrain_scan::trigger_nearest_poi` |
| M16 | w | none | waypoint distance and bearing (`relative_bearing`) | 8841 | `on_next_key_press` |
| M17 | s | C+S, not A | toggle speech logging (truncates `speech.log`) | 8862 | `on_next_key_press` |
| M18 | l | C+S, not A | toggle low speed detection | 8882 | `on_next_key_press` |
| M19 | k | C only | toggle wheel slip detection | 8896 | `on_next_key_press` |
| M20 | l | C only | "Dumping DOM", broadcast `dom_dump` | 8909 | -> `ui_bridge::broadcast` |
| M21 | g | C | toggle coordinate guidance (refuses without a waypoint) | 8916 | `on_next_key_press` |
| M22 | h | S | hydros dump thread (`HDUMP`, 4447) | 8927 | `start_dump_thread(Hydros)` -> `run_log_dump` |
| M23 | h | any left | speak heading | 8969 | `on_next_key_press` |
| M24 | f | S, not A | `open_fuel_stations(force_list=ctrl)` | 8971 | -> `fuel::open_fuel_stations` |
| M25 | f | any left | "Fuel N percent" | 8978 | `on_next_key_press` |
| M26 | g | any left | gear phrase | 8980 | `on_next_key_press` |
| M27 | t | C only | `speak_ambient_temperature()` | 8982 | -> `browsers::speak_ambient_temperature` |
| M28 | t | A only | `speak_induction_report()` | 8990 | -> `browsers::speak_induction_report` |
| M29 | t | S | max boost / "Unavailable" | 8996 | `on_next_key_press` |
| M30 | t | any left | boost | 8999 | `on_next_key_press` |
| M31 | p | S | powertrain dump thread (`PDUMP`) | 9001 | `start_dump_thread(Powertrain)` |
| M32 | p | not S | air pressure | 9043 | `on_next_key_press` |
| M33 | u | any | flip units, speak the mode | 9056 | `on_next_key_press` |
| M34 | e | S | electrics dump thread (`DUMP`) | 9059 | `start_dump_thread(Electrics)` |
| M35 | e, `engtemp` | not S | engine temperature | 9104 | `on_next_key_press` |
| M36 | m | none | damage report thread (`DAMAGE`) | 9109 | `start_dump_thread(Damage)` -> `run_damage_report` |
| M37 | o | not C | oil temperature | 9155 | `on_next_key_press` |
| M38 | c | C+S+A | `open_clickspot_browser()` | 9157 | -> `browsers::open_clickspot_browser` |
| M39 | c | C+S, not A | `toggle_clickspot_mode(ac)` | 9164 | -> `clickspot::toggle_clickspot_mode` |
| M40 | c | A only | speak marked waypoint | 9171 | `on_next_key_press` |
| M41 | c | S only | mark waypoint at current position | 9183 | `on_next_key_press` |
| M42 | c | none | coordinates | 9191 | `on_next_key_press` |
| M43 | s | C | `toggle_status_mode()` | 9198 | -> `menus::toggle_status_mode` |
| M44 | b | C | `toggle_buffer_mode()` | 9200 | -> `virtual_browser::toggle_buffer_mode` |
| M45 | b | none | `open_vehicle_bindings_browser()` | 9202 | -> `browsers::open_vehicle_bindings_browser` |
| M46 | b | S only | `toggle_binding_learn_mode()` | 9209 | -> `binding_learn::toggle_binding_learn_mode` |
| M47 | n | none | `open_environment_browser()` | 9219 | -> `browsers::open_environment_browser` |
| M48 | j | none | `open_audio_debug_browser()` | 9233 | -> `browsers::open_audio_debug_browser` |
| M49 | c | C only | toggle pedal tones | 9241 | `on_next_key_press` |
| M50 | v | S, not C | ramp align (`RAMPALIGN`) if docking, else `ALIGN` to 4448 (needs scanner) | 9252 | `on_next_key_press` |
| M51 | v | C | `toggle_scan_mode(ac)` | 9276 | -> `scanner::toggle_scan_mode` |
| M52 | o | C | `toggle_obstacle_mode(ac)` | 9278 | -> `obstacle::toggle_obstacle_mode` |
| M53 | n | C only | `toggle_nodegrab_mode(ac)` | 9280 | -> `nodegrab::toggle_nodegrab_mode` |
| M54 | tab | any | scanner `CLOSEST` (C) / `PREV` (S) / `NEXT` to 4448 (needs scanner) | 9286 | `on_next_key_press` |
| M55 | a | any left | roll and pitch | 9302 | `on_next_key_press` |
| M56 | space | none | vehicle info browser / its failure reason / mod details page / terrain scan / `context_action` | 9304 | `space_command` |
| - | anything else | | nothing; layer cleared | 9352 | `on_next_key_press` |

## Every key and modifier combination

C = Ctrl, S = Shift, A = Alt. Each cell is the branch ID above that answers; `-` means no
branch matches and the layer just closes. With help mode on, every combination speaks its
`_F9_HELP` entry instead (P4).

| Key | none | C | S | A | CS | CA | SA | CSA |
|---|---|---|---|---|---|---|---|---|
| `a` | M55 | M55 | M55 | A4 | M55 | M55 | A5 | M55 |
| `b` | M45 | M44 | M46 | - | M44 | M44 | - | M44 |
| `c` | M42 | M49 | M41 | M40 | M39 | - | - | M38 |
| `d` | M10 | M8 | M9 | A8 | M7 | - | - | - |
| `e` | M35 | M35 | M34 | M35 | M34 | M35 | M34 | M34 |
| `f` | M25 | M25 | M24 | A2 | M24 | M25 | M25 | M25 |
| `g` | M26 | M21 | M26 | A1 | M21 | M21 | M26 | M21 |
| `h` | M23 | M6 | M22 | A3 | M6 | M6 | M22 | M6 |
| `i` | M11 | M13 | M12 | - | - | - | - | - |
| `j` | M48 | - | - | - | - | - | - | - |
| `k` | - | M19 | - | - | - | - | - | - |
| `l` | - | M20 | - | - | M18 | - | - | - |
| `m` | M36 | - | - | - | - | - | - | - |
| `n` | M47 | M53 | - | - | - | - | - | - |
| `o` | M37 | M52 | M37 | M37 | M52 | M52 | M37 | M52 |
| `p` | M32 | M32 | M31 | A6 | M31 | M32 | M31 | M31 |
| `q` | - | - | - | - | - | - | - | - |
| `r` | M5 | M4 | M2 | - | M3 | - | M2 | - |
| `s` | M1 | M43 | M1 | M1 | M17 | M43 | M1 | M43 |
| `t` | M30 | M27 | M29 | M28 | M29 | M30 | M29 | M29 |
| `u` | M33 | M33 | M33 | M33 | M33 | M33 | M33 | M33 |
| `v` | - | M51 | M50 | A7 | M51 | M51 | M50 | M51 |
| `w` | M16 | M14 | M15 | - | - | - | - | - |
| `x` `y` `z` | - | - | - | - | - | - | - | - |
| `0`-`9` | D | - | - | - | - | - | - | - |
| `tab` | M54 | M54 | M54 | M54 | M54 | M54 | M54 | M54 |
| `space` | M56 | - | - | - | - | - | - | - |
| `engtemp` | M35 | M35 | - | M35 | - | M35 | - | - |
