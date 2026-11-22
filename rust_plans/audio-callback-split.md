# Audio callback split

`AudioController._audio_callback` (`audio.py:4138-7023`) divided into voice modules, the
order they run in, every coupling between them, the locking pattern, and the assignment of
the voice bodies to the wave 2 agents A1, A2 and A3. Written by AA. Line numbers are
`audio.py` on `rust-port` at the start of the port.

Rust layout (all in `crates/beam-audio/src/`):

| File | What |
|---|---|
| `controller.rs` | `AudioController`, `Shared` (Python's `self.lock` state), `Voice` (state the callback mutates outside the lock), `Waveforms`, `Params`, every public method |
| `callback/mod.rs` | the frame: request intake, snapshot, buffers, voice order, duck buses, final mix, clip, interleave, panic containment |
| `callback/block.rs` | `Block`, the context every voice receives; `Snapshot` (the Python locals copied under the lock) |
| `callback/common.rs` | helpers every voice shares: overlap-add HRTF convolution, tail drain, resampled one-shot playback, straight one-shot playback, delayed-pulse queues, `pan_gains`, `render_directional_pulse`, numpy `linspace(endpoint=False)` |
| `callback/clock.rs` | `Clock` trait (`monotonic`, `perf_counter`), `SystemClock`, `FakeClock` |
| `callback/<voice>.rs` | one per voice below |

## Locking and snapshot pattern

Python has one lock, `self.lock`, and relies on the GIL for everything else: the callback
mutates its synthesis state (phases, envelopes, overlap tails, playback cursors) **outside**
the lock, and several API methods write those same attributes, some under the lock, some
with no lock at all. Rust needs every shared field behind a lock, and the callback must not
hold the state lock while synthesising, so the state is split in two:

- `AudioController::lock: Mutex<Shared>` is `self.lock`. It holds everything the API and
  telemetry threads write and the callback snapshots: telemetry inputs, mode toggles,
  targets, the request slots of the five HRTF one-shots, the queues Python mixes **under**
  the lock (road chime, road junction, assistant cues, gear tones, junction proximity / stop
  bar), config-derived `Params`, and the HRTF set (`Option<Arc<HrtfSet>>`).
- `AudioController::voice: Mutex<Voice>` is everything the callback writes outside the
  lock. The callback locks it for the **whole block**. API methods that write a `Voice`
  field (Python wrote it with or without `self.lock`) lock `voice` for that write.
- **Lock order is always `voice` then `lock`.** The callback takes `voice`, then `lock`
  briefly for intake and snapshot (and again where Python takes it mid-callback: the road
  chime/junction queues, `_mix_junction_proximity`, `_mix_assistant_cues`,
  `_mix_gear_tones`). An API method that needs both takes `voice` first. Never the reverse.
  Holding `voice` costs an API thread at most one block's compute (well under 1 ms); the
  callback only ever waits on `lock`, which nobody holds for long except the HRTF one-shot
  triggers, which convolve under it exactly as Python does.
- `AudioController::waves: Mutex<Arc<Waveforms>>`: every `*_WAVEFORM` attribute. Python
  reassigns them from the config thread with no lock; Rust builds a new `Waveforms`
  (fields are `Option<Arc<[f32]>>`, so the clone is cheap) and swaps the `Arc`. The callback
  clones the `Arc` once per block.
- `samplerate` is an `f64` in `Shared::samplerate` and copied into the snapshot; it only
  changes in `restart_audio_stream` while no stream exists.

`Snapshot` is exactly the block of locals Python copies at `4182-4308`, plus the things
Python reads from `self` outside the lock that the API writes (`_drift_alert_active`,
`_drift_rate`, `_drift_pan`, `_scan_tones_enabled`, every config param, `_hrtf`, the five
one-shot conv buffers and `use_hrtf` flags): reading them once per block instead of live
can only move a change by one block, which Python's own races already allow.

The `trailer_duck_env` glide is computed **inside** the snapshot section (Python does it
under the lock at 4259), so `_trailer_duck_env` lives in `Shared`.

## The frame (callback/mod.rs), Python order

| # | Step | Python | Notes |
|---|---|---|---|
| F1 | Request intake: click, ping, highlight, cam click, cam highlight | 4139-4180 | `threading.Event`s become `bool`s in `Shared`, consumed under `lock`; cursor/pitch/pan land in `Voice` |
| F2 | Snapshot under the lock, trailer duck glide, `obstacle_duck = obstacle_duck_gain(state)` | 4182-4308 | `obstacle_duck_gain` is AH's (`consts.rs`, with the other module-level pure functions) |
| F3 | `bufL`/`bufR` zeroed, `frames` long, `f32` | 4310-4313 | preallocated scratch in `Voice`; grown, never shrunk |
| F4 | voices 1-29 below, with the duck buses | 4315-7020 | |
| F5 | stack, `np.clip(-0.999, 0.999)` in `f32`, write interleaved | 7021-7023 | NaN passes through clip in both |

**Errors.** Python wraps nothing: an exception escapes into sounddevice's cffi callback,
which is declared `error=paAbort`, so the traceback is printed and the stream aborts (and
stays dead: the watcher only restarts on a device change, a real Python bug, reported).
The Rust frame runs under `catch_unwind`; on a panic it logs once, writes silence, and
sets `Voice::callback_aborted`, after which every later block on that stream is silence
(the closest cpal equivalent of `paAbort`; a restart clears it).

## Voice modules, in run order

"State" lists the Python attributes; R = read, W = written. Shared fields reach a voice
through `Snapshot`; `Voice` fields through `&mut Voice`. All voices write `bufL`/`bufR`.

| # | Module (`callback/`) | Python lines | Produces | State | Config | Sim |
|---|---|---|---|---|---|---|
| 1 | `oneshots.rs` **(ported by AA)** | 4315-4332 | TC tone, check-engine buzzer, oil chime, describe-error buzz, low-speed stop tone, centred | W `_tc/_buzzer/_chime/_describe_error/_ls_stop_playback_pos` | buzzer/chime levels via the waveforms | - |
| 2 | `placement_ping.rs` | 4334-4369 | placement ping, resampled, HRTF conv or pan | R snapshot ping conv/use_hrtf, `PLACEMENT_PING_WAVEFORMS`; W `_ping_playback_pos` | `placement_ping_volume_db` | - |
| 3 | `compass_clicks.rs` | 4371-4627 | compass click, highlight, cam click, cam highlight; resampled, conv or pan | R snapshot conv/use_hrtf x4; W four `*_playback_pos` | `compass_click_level_dbfs` | - |
| 4 | `shift_tone.rs` **(ported by AA)** | 4629-4635 | shift triangle | R `shift_on`, `shift_amp`, `_phase_inc_shift`; W `_phase_shift` | `shift_tone_*` | - |
| 5 | `pedals_steer.rs` | 4637-4690 | pedal tones (3 detuned sines), steering square | R `pt_on`, clutch/brake/throttle, `v_steer` (= `_hydro_steer_input`), `_detune_*`; W `_sm_c/b/t`, `_phase_c/b/t`, `_phase_steer` | - | - |
| 6 | `attitude.rs` | 4692-4855 | roll pulse + pitch triangle, HRTF overlap-add | R `inv`, roll/pitch rad, att dB params; W `_sm_roll_int`, `_sm_pitch_int`, fade mults, phases, roll/pitch overlaps | `pitch_roll_*` | - |
| 7 | `drift.rs` | 4857-4924 | drift tone, glided pitch/pan/level | R drift active/pan/rate; W `_sm_drift_rate`, `_sm_drift_pan`, `_drift_gain`, `_drift_inc`, `_phase_drift` | - | - |
| 8 | `scanner.rs` | 4926-5177 | bearing EMA, beep train (bright when aligned), steering-locked steady tone; HRTF; diag capture | R scan active/bearing/dist/closing/base_freq/offset/steer flag, `v_steer`, `coupler_active`, **`_dock_gate_env` (previous block)**; W `_sm_scan_bearing`, `_scan_tone_level`, `_scanner_beep_timer`, `_scanner_playback_pos`, overlaps, `_scan_tone_phase` | `scanner_*` | - |
| 9 | `cannon_aim.rs` | 5183-5241 | centred elevation pulse | R scan_active, coupler_active, **`_dock_gate_env` (previous block)**, cannon snapshot, `perf_counter`; W pulse/carrier phase | - | - |
| 10 | `node_clickspot.rs` | 5243-5287 | node hover beep (resampled), clickspot beep | W both cursors; R freq/reverse | - | - |
| 11 | `coupler.rs` | 5289-5369 | coupler tone / in-range beeps, HRTF | R coupler snapshot; W phase, beep timer, overlaps | - | - |
| 12 | `obstacle.rs` | 5371-5514 | doublet / cadence pulses, steady emergency spectrum, terrain sweep | R hazard snapshot, `_obstacle_amp`, `_obstacle_harmonic_peak`; W render bearing, cadence clock, pulse queue, doublet flag, steady env/phase/overlaps, `_terrain_playback_pos` | `obstacle_buzz_volume_db` | `obstacle_audio_sim.py` |
| 13 | `road_beacon.rs` | 5516-5575 | off-road beacon pulse | R road snapshot; W beep timer, cursor, pulse L/R | `road_beep_volume_db` | `road_audio_sim.py` |
| 14 | `route_beacon.rs` | 5577-5622 | route beacon pulse | R route snapshot; W timer, cursor, pulse L/R | `route_beacon_volume_db` | `route_beacon_sim.py` (audio half) |
| 15 | `road_correction.rs` | 5624-5654 + `_render_road_correction_block` 2515-2620 + `_stop_road_correction_voice` 2508-2513 | correction pips / unwind doublet, settled sweep | R correction snapshot; W correction env/phases/overlaps/render bearing, `_road_correction_settled_pos` | `road_correction_volume_db` | `road_audio_sim.py` |
| 16 | `road_queues.rs` | 5656-5714 | orientation chime queue, junction pip queue (mixed **under `lock`**) | RW `_road_chime_queue`, `_road_junction_queue` | `road_junction_volume_db` | `road_audio_sim.py` |
| 17 | `lowspeed.rs` | 5720-5843 | low-speed clicks, HRTF conv at 0 deg | R ls snapshot; W fade, timer, cursor, pitch, conv L/R, use_hrtf | `lowspeed_*` | - |
| 18 | `slip.rs` | 5845-5964 + `_slip_voice_tables` 3408-3449 | spin/lock wavetable voice, slide noise, HRTF | R slip snapshot, `_slip_amp_db`; W fades, ratio smoother, pitch kind, phases, noise pos, az vector, overlaps, table cache | `slip_tone_level_dbfs` | `wheel_slip_sim.py` (callback half) |
| 19 | `heading_guidance.rs` | 5966-6036 | heading tone | R guide active/error; W `_sm_guidance_error`, `_phase_guidance` | - | - |
| 20 | `hydro_steer.rs` | 6038-6221 | articulation FM tone, centre-crossing click | R `hydro_actual`, `hydro_from_trailer`, `trailer_duck_env`, **`_impl_gate_env` and `_dock_gate_env` (previous block)**; W env, phases, ratio/index/inc, sm mag, overlaps, centre armed/pos, `_hydro_was_from_trailer` | - | `hydro_steer_sim.py` |
| 21 | `implement.rs` | 6223-6457 | ground FM tone, tilt quarter-tone sine | R impl snapshot, `_impl_ground_db`, `_impl_tilt_db`; W closing estimator, gate (**`_impl_gate_env`, read next block by #20**), ground/tilt phases, tilt stop learning | `implement_*` | `implement_tone_sim.py` |
| 22 | `dock.rs` | 6459-6812 | panned pulse, centred beat pair, lock chime | R dock snapshot, `perf_counter`; W **`_dock_gate_env` (read next block by #8, #9, #20)**, beat env, phases, rates, overlaps, lock latch/cursor | `dock_tone_*` | `dock_tone_sim.py` |
| 23 | `dock_earcons.rs` | 6818-6867 | slam cue, entry cue, approach cue (centred) | W the three cursors, R `_slam_cue_kind` | - | - |
| 24 | `coord_guidance.rs` | 6869-6968 | coordinate FM tone, HRTF | R coord snapshot; W smoother, phases, overlaps | - | - |
| 25 | `lineup.rs` | 6974-6977 + `_render_lineup` 3311-3399 | line-up FM tone, HRIR crossfade | R `lineup_live`, target; W smoothers, phases, `_lineup_ir`, overlaps | - | `camera_lineup_sim.py` (render half, if any) |
| 26 | `scan_playback.rs` | 6979-7016 | pre-rendered terrain scan + crossfade of the replaced one | R `_scan_tones_enabled`; W `_scan_buf`, `_scan_pos`, `_scan_old_*` | `scan_tone*` | `terrain_scan_sim.py` is AH's (render) |
| 27 | `junction_proximity.rs` | 7018 + `_mix_junction_proximity` 2413-2460 + `_stop_bar_render_state` 2381-2411 | stop-bar tone (under `lock`) | RW `_junction_proximity*`, `_stop_bar_*` | `road_junction_*` | `junction_proximity_sim.py` |
| 28 | `assistant_cues.rs` **(ported by AA)** | 7019 + `_mix_assistant_cues` 2490-2506 | queued assistant cues (under `lock`) | RW `_assistant_cue_queue` | - | `assistant_audio_sim.py` |
| 29 | `gear_tones.rs` **(ported by AA)** | 7020 + `_mix_gear_tones` 2477-2488 | in-flight gear tones (under `lock`) | RW `_gear_tones` | `gear_tone_*` | `gear_report_sim.py` (render is B1's) |

### The duck buses (obstacle duck)

`obstacle_duck` is `obstacle_duck_gain(hazard.state)` from the snapshot: 1.0 (none or
advisory), -6 dB (urgent), -12 dB (emergency). The frame copies the buffers before a group
and afterwards replaces them with `mix + (buf - mix) * duck`, all in `f32`, **only when
`duck < 1.0`**. Five groups, exactly these and nothing else:

| Group | Copy taken | Duck applied | Voices inside |
|---|---|---|---|
| scanner | 4949 (after the bearing EMA) | 5179 | 8 (beeps and steady tone) |
| road | 5516 | 5716 | 13, 14, 15, 16 |
| heading | 5966 | 6034 | 19 |
| dock | 6460 | 6814 | 22 (pulse, pair, lock chime) |
| coordinate | 6869 | 6970 | 24 |

Never ducked: one-shots, clicks, pedals, attitude, drift, **cannon aim** (runs after the
scanner duck), node/clickspot, coupler, obstacle itself and the terrain sweep, low speed,
slip, articulation and its click, implement, dock earcons (slam/entry/approach), line-up,
terrain scan, stop bar, assistant cues, gear tones. `docs/obstacle-warning.md` says "scanner,
road, docking, coordinate, and heading guidance"; the code agrees.

Because the duck is applied to the **difference**, a voice inside a group that drains an
HRTF tail is ducked too, and anything the voice adds that was rendered before the group
copy (none today) is not.

## Cross-voice couplings

1. **Dock gate, previous block.** `_dock_gate_env` is written at the end of voice 22. Voice
   8 (scanner) is gated on `_dock_gate_env <= 0.0`, voice 9 (cannon aim) likewise, and voice
   20 (articulation) adds `DOCK_HYDRO_DUCK_DB * _dock_gate_env` to its level. All three run
   earlier in the block, so they see the previous block's value. Rust: the field lives in
   `Voice`, so the earlier voices read `v._dock_gate_env` before voice 22 updates it. Do not
   move it into the snapshot.
2. **Implement gate, previous block.** Voice 20 adds `IMPL_HYDRO_DUCK_DB * _impl_gate_env`;
   voice 21 updates it later the same block. Same rule.
3. **Coupler suppresses scanner and cannon.** `coupler_active` (snapshot) gates voices 8 and
   9; voice 8's else-branch then clears the scanner tails and the morph level.
4. **Steering input feeds two voices.** `v_steer` is `_hydro_steer_input` (not
   `last_steering`), used by voice 5's steering square and voice 8's steady-tone morph.
5. **Articulation source selection and trailer duck** happen in the snapshot (F2): the
   hinge is the frame bend unless `_hydro_actual_steer == 0.0` exactly, then the trailer;
   the forward-trailer duck env glides there under the lock. Voice 20 also disarms the
   centre click when the source flips.
6. **Obstacle duck** (above) is computed in F2 from the snapshotted hazard state; the
   obstacle voice itself does not consult it.
7. **Obstacle steady voice uses `_obstacle_harmonic_peak`**, which Python sets as a side
   effect of `_generate_obstacle_buzz` (AH's generator returns it; the controller stores it
   in `Waveforms::_obstacle_harmonic_peak`, default 1.0 via `getattr`).
8. **Road correction is stopped** (`_stop_road_correction_voice`) by voice 15 when not
   (`road_active and road_on_road`), and by `set_road_mode(False)`, `clear_road_audio`,
   `update_road_guidance` (new instruction, settled, off-road): all `Voice` writes.
9. **Road chime and junction queues** are mixed under `lock` (Python does), and the
   triggers replace the whole queue under `lock`.
10. **Low speed**: the standstill tone is a one-shot (voice 1, `trigger_lowspeed_stopped`);
    voice 17 cuts the click fade short (`LS_STOP_FADE_S`) and starts no new click while
    `ls_stopped`.
11. **Scan playback reads `_scan_tones_enabled` live** (snapshot); `apply_config` turning it
    off also calls `stop_scan`.
12. **Shared HRTF.** Every spatial voice uses `hrtf` and `_hrtf_user_enabled` from the
    snapshot; `_hrtf_front_emphasis_db` / `_hrtf_distance_gain` only by the one-shot
    convolutions (triggers, low speed, slip).
13. **Scanner diagnostic.** Voice 8 calls `ScannerHRTFDiagnostic::record_beep` (AH's
    `scanner_hrtf_diag`) when enabled.
14. **Lock chime and dock earcons.** The lock chime (in the dock duck group) and the slam,
    entry and approach cues (after it, unducked) play to completion whatever the gate does.

## dtype notes for voice authors

`bufL`/`bufR` are `float32`. numpy is 2.3 (NEP 50: Python scalars are weak, so
`f32_array * python_float` stays `f32`; `np.float64` scalars are strong). Most voices build
a `float64` expression and `+=` it into the `f32` buffer (one rounding per sample at the
add); some `.astype(np.float32)` first and then multiply by a pan gain in `f32`. Follow
each line. `get_hrir` returns `float64` IRs except on an exact grid hit (`float32` copy), so
`np.convolve(f32, ir)` is `float64` and the overlap tails are `float64`; the helper in
`callback/common.rs` computes in `f64`. `np.linspace(a, b, n, endpoint=False)` is
`dsp`-style `a + i*(b-a)/n`; `common::linspace_open` reproduces numpy's formula.

## Assignment

Roughly 950-1,100 Python callback lines each, grouped so coupled voices share an agent.

**A1: vehicle, attitude and one-shot voices**
- Files: `placement_ping.rs`, `compass_clicks.rs`, `pedals_steer.rs`, `attitude.rs`,
  `drift.rs`, `node_clickspot.rs`, `lowspeed.rs`, `slip.rs`, `heading_guidance.rs`,
  `lineup.rs`. (`assistant_cues.rs` and `gear_tones.rs` were small enough that AA ported them;
  A1 owns their sims.)
- Sims: `wheel_slip_sim.py` (callback half), `assistant_audio_sim.py`, `camera_lineup_sim.py`
  (any render checks), and golden buffers for clicks, pedals, attitude, drift, low speed,
  slip (noise voice by property), heading, line-up.

**A2: guidance and warning voices (scanner, obstacle, road, coordinate)**
- Files: `scanner.rs`, `cannon_aim.rs`, `coupler.rs`, `obstacle.rs`, `road_beacon.rs`,
  `route_beacon.rs`, `road_correction.rs`, `road_queues.rs`, `coord_guidance.rs`,
  `scan_playback.rs`, `junction_proximity.rs`.
- Sims: `obstacle_audio_sim.py`, `road_audio_sim.py`, `route_beacon_sim.py` (audio half),
  `junction_proximity_sim.py`, and golden buffers for the scanner (with and without the
  dock gate), cannon aim, coupler, road, coordinate guidance, scan playback crossfade.

**A3: machine instruments (articulation, implement, docking)**
- Files: `hydro_steer.rs`, `implement.rs`, `dock.rs`, `dock_earcons.rs`.
- Sims: `hydro_steer_sim.py`, `implement_tone_sim.py`, `dock_tone_sim.py`, plus golden
  buffers that exercise couplings 1 and 2 (articulation ducked by the implement and dock
  gates one block late) and the dock duck bus.
- A3 has fewer files but the three largest state machines and the three longest sims.

Each agent fills only its own `callback/<voice>.rs` bodies (and may add private helpers in
those files, or new files `callback/<voice>_*.rs` declared from its own module) plus its
own `tests/*`. `callback/mod.rs`, `block.rs`, `common.rs` and `controller.rs` are AA's; a
missing helper or field goes back to the orchestrator rather than being edited in.

## Interfaces the wave 2 agents fill (as stubbed by AA)

- Every voice: `pub fn render(b: &mut Block<'_>, v: &mut Voice)`. `b.snap` is the
  snapshot, `b.snap.hrtf()` the HRTF set when the user has it enabled, `b.waves` the
  waveforms, `b.clock` the clocks, `b.shared` the lock (only for the voices Python
  mixes under it), `b.buf_l`/`b.buf_r` the `f32` block. Never lock `voice` inside a
  voice; the frame already holds it.
- Extra entry points with fixed signatures, because the Python sims call the private
  methods directly and `AudioController` exposes wrappers for them:
  `scanner::smooth_bearing` (runs before the scanner duck copy),
  `road_correction::render_road_correction_block`, `slip::slip_voice_tables`,
  `lineup::render_lineup`, `junction_proximity::{mix_junction_proximity,
  stop_bar_render_state}`, `assistant_cues::mix_assistant_cues`,
  `gear_tones::mix_gear_tones`.
- Helpers in `callback/common.rs`: `clamp`/`clamp01`, `pan_gains`,
  `linspace_open_at`/`linspace_open_into`, `convolve_full_into`/`convolve_f32_into`,
  `add_tail`/`keep_tail`/`add_conv_f32`/`drain_tails` (overlap-add), `play_centred`,
  `play_resampled`, `PulseEntry` + `mix_pulse_queue`, `render_directional_pulse`,
  `db_to_amp`. AH's `dsp` also has numpy's `default_rng` (PCG64), so the slip noise
  tables and the centre click can be compared sample for sample.
- Tests: `tests/audio_harness/mod.rs` (`mod audio_harness;`) builds a controller with no
  stream and a `FakeClock`, zero detune, waveforms and config; `run_blocks(n, frames)`.
  `fixtures/gen/audio_callback_harness.py`'s `run_script(script, config)` produces
  the matching Python blocks (steps: `call`, `set`, `clock`, `advance`, `blocks`).
  `tests/common` belongs to AH.