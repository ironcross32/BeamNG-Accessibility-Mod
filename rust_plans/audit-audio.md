# Audit: audio controller and callback (V1, slice 2)

2026-11-21. Python: `audio.py` (7234 lines, 131 functions), `hrtf.py`, `hrtf_sets.py`,
`wheel_slip.py`, `scanner_hrtf_diag.py`. Rust: `beam-audio` (and `beam-base::hrtf_sets`).
Audited by the orchestrator, which did not write beam-audio.

## Method

1. **Fixtures regenerated against today's Python**: unchanged (see
   `audit-config-ui.md`). That includes the A1-A3 voice goldens, the HRTF fixture
   (every bundled set, several elevations, equalised and not, through the real
   `HRTFSet`), the waveform, DSP, scan and wheel-slip fixtures.
2. **Name inventory**: every one of the 145 Python function names in these five files
   appears in the Rust sources (the port keeps Python names in its doc comments). A weak
   check on its own; it rules out a whole function being skipped.
3. **The Python changes made after the port began** (`git diff` of the working tree):
   `audio.py` +56/-22, `hrtf.py` +277. Each change was traced to the Rust.
4. **Every Python sim has a Rust port**, except `traffic_mode_sim.py`, which tests the
   road listener and belongs to the listeners slice.
5. Hand walk of what no fixture reaches: output device selection, the default-device
   watcher, stream restart, start and stop.

## Results

| Python | Rust | State |
|---|---|---|
| The callback voices (compass, scanner, obstacle, road, implement, dock, coupler, hydro steer, cannon, line-up, slip, gear, pitch/roll, warnings) | `beam-audio/src/callback/*` | equivalent: A1-A3 goldens and the voice sims, all regenerated today |
| `HRTFSet` (several sets, elevation interpolation, ITD split, minimum phase, equalisation, level match) | `beam-audio` hrtf | equivalent: fixture over every set, regenerated today |
| `load_hrtf(path, set_key, elevation, equalise)`: keep the current set on failure, new log text | `controller.rs` `load_hrtf` | equivalent (post-base change, ported) |
| `select_hrtf_set`: no-op when (set, elevation, equalise) is unchanged | `controller.rs` `select_hrtf_set` | equivalent (post-base change, ported); seen working in game |
| Junction target: a `privateEntry` is not a stopping target | `controller.rs:1678` | equivalent (post-base change, ported; `junction_proximity_sim` covers it) |
| `_find_target_device` | `device.rs` `find_target_device` | equivalent rules and log text; indices are cpal WASAPI positions, not PortAudio's |
| `_device_name`, `_restart_audio_stream` (teardown outside the lock), `_device_watcher_loop`, `start`, `stop` | `device.rs` | equivalent |
| `sd.play(buffer, blocking=False)` | `device::play_buffer` | equivalent (stops the previous one; mono, or duplicated if the device refuses one channel) |

## Differences kept

- **Device indices.** PortAudio numbered devices across every host API and fixed the
  list at initialisation; the Rust enumerates WASAPI afresh on each query. The selection
  rules are the same. One consequence, if anything an improvement: a device plugged in
  mid-session is seen by the watcher, where PortAudio's frozen list would not show it.
- **No WASAPI host at all.** Python fell back to PortAudio's default device (often MME);
  the Rust falls back to cpal's default host, which on Windows is WASAPI. Only reachable
  on a machine without WASAPI.

## Performance

The debug build crackles under the heaviest mix (scanner plus a chasing AI car); the
release build does not (user, 2026-11-21). Run release builds.

## Open

None in this slice. `traffic_mode_sim.py` is carried to the listeners audit.
