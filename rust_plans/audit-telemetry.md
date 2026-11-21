# Audit: telemetry loop (V1, slice 3)

2026-11-21. Python: `beamtel.py` `telemetry_loop` (lines 10526-11482) and the packet
formats. Rust: `beam-engine/src/telemetry.rs`. Audited by the orchestrator, which did
not write E1.

## Method

1. **The fixture drives the real loop.** `fixtures/gen/telemetry.py` runs
   `beamtel.telemetry_loop` packet by packet, with the socket, clock, speech, audio and
   UDP pushers replaced by recorders. It captures every global the loop writes and every
   call it makes, over 17 scenarios plus a decode table for every packet layout. The
   Rust replays the same packets through `TelemetryLoop::handle_datagram`. It
   regenerated unchanged against today's Python.
2. **Coverage of the real loop, measured.** The generator was run under `coverage`:

   ```
   uv run --with coverage python -m coverage run --include=beamtel.py rust/fixtures/gen/telemetry.py
   uv run --with coverage python -m coverage json -o tcov.json
   ```

   then the executed and missing lines inside the function were counted. **358 of the
   loop's 392 statements (91%) run under the fixture.** The 34 that do not were walked
   by hand. The same method applies to any slice whose fixture drives real Python.
3. `git diff` of the Python working tree: nothing changed in the loop since the port
   began.

## The 9% the fixture does not reach

| Python lines | What | Rust | State |
|---|---|---|---|
| 10656-10661 | recv timeout continues; `OSError` breaks if stopping, else re-raises | `telemetry_loop`: `Recv::Timeout` / `Recv::Error` | equivalent |
| 10683, 10839-10928 | `except Exception: continue` around each `struct.unpack` | none needed | unreachable: each unpack is behind an exact length check |
| 10737-10740 | heading delta wrap to ±180 for the drift rate | `telemetry.rs:736-741` | equivalent |
| 11367-11378 | route beacon bearing from position and heading; silent at the destination; off when no route | `telemetry.rs:1329-1344` | equivalent (and `route_beacon_sim` checks the shared helper) |
| 11389-11404 | drift alert release hold, and the resets on activation and when drift mode is off | `telemetry.rs:1351-1369` | equivalent |
| 10633, 11480 | `stop_event = STOP` default, `sock.close()` failure | `telemetry_loop` signature, drop | equivalent |

## Python bugs

E1 recorded bugs 30-33 in `python-bugs.md`. Bug 30: the steering globals are never
written, so the obstacle detector always got steering 0. (An earlier version of this
audit said the drift alert's steering condition was always met too. It is not: the loop
assigns locals, and drift reads the locals.) **Fixed in the Rust** at the user's decision
(2026-11-21); see `python-bugs.md` 30.

## Open

None.
