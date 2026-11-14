# Python bugs found during the port

Collected from the wave 1 agents' reports. The rule so far is "port the bug, report it".
This file exists so the user can decide, per item, whether the Rust should keep
reproducing it or be hardened. **Rust now** says what the Rust does today.

Legend: R = reproduced as in Python; H = already hardened in Rust (a departure).

## Would lose user data

| # | Where | What happens | Rust now |
|---|---|---|---|
| 1 | both `load_config` | A value that overflows a number conversion (`1e400`, `Infinity`, a huge int in an int or volume key, `scanner_pitch_offset_oct` NaN/inf) is not caught by the per-key fallback; the whole file is treated as corrupt, renamed to the backup and replaced with defaults | R |
| 2 | both `load_config` | A failed migration write-back (locked file after retries) takes the same corruption path and renames a good config | R |

## One bad packet or socket error silences a feature until restart

| # | Where | What happens | Rust now |
|---|---|---|---|
| 3 | `telemetry_loop` | One socket error on 4444 ends the whole engine; the window stays open | R |
| 4 | `slot_listener` | A non-UTF-8 `SLOTS` datagram kills the listener | R |
| 5 | `ui_listener`, `console_listener` | Any socket error stops the listener for the session | R |
| 6 | `road_listener` | `parse_r2_packet` can raise `TypeError`/`OverflowError`; only `ValueError` is caught. `ROAD_DIAGNOSTICS.record` raises on any NaN, and `HILL_CLIMB_CHALLENGE.record` can raise `AttributeError`, both outside any try | R |
| 7 | induction report thread | Any malformed field (non-number, `"0"` as a string for max turbo RPM, non-dict `turbo`) raises and the report is never spoken | R |
| 8 | drift, fuel, POI parsers | `OverflowError` (infinite or huge numbers) escapes the callers' `except ValueError` | R |
| 9 | `DrivingAssistant.receive` | Unhashable `kind`, non-string `turn`, or a string arrival `bearing` raise past the handler; the last one after state is cleared, so the "off" cue never plays | R |
| 10 | audio callback | An exception aborts the stream for good; the watcher restarts only on a device change | H: caught, logged, silence until restart |
| 11 | telemetry / callout threads | A non-numeric `speed_announce_interval` or `scanner_distance_callout_interval` crashes the thread later | H: previous value kept, warning logged |
| 12 | `speech.speak` | An exception reading the backend name inside a log call escapes a function that promises never to raise | H: logs "unknown" |

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
| 23 | Configuration panel | With no WASAPI devices, "(no WASAPI devices found)" is saved as the preferred device name | R |
| 24 | AI Describer panel | Switching provider while a key validates saves the key under the new provider | R |
| 25 | Reset to Defaults | The next save rewrites keys the panel does not own from defaults | R |
| 26 | `speech` test backend | Context can be shut down while the test backend is alive | H: context kept alive until last backend drops |
| 27 | hill climb | A second `race_complete` replaces the timer without cancelling; the stale one fires. `list_reports(0)` returns one. A summary that is valid JSON but not an object aborts the finish | R |
| 28 | recorders | `RoadDiagnosticRecorder.stop` can leave a truncated summary; `AssistantDiagnosticRecorder.review` raises on a non-object line | R (Rust summary writes are all-or-nothing) |
| 29 | F9 Tab error log | Prints `b'NEXT'` | R |

## Recommendation

Harden 1-9 in the Rust (catch at the listener or per key, log, carry on), keep the rest as
they are. None of 1-9 changes anything a user hears in normal operation; they only stop a
malformed value from costing a feature or the whole config.
