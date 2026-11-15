"""Golden buffers for the machine-instrument voices of the audio callback: the articulation
(hydraulic steering) tone and its centre click, the loader implement tones, the docking
instrument and the dock earcons, plus their couplings (the articulation ducked one block
late by the implement and dock gates; the dock voice inside the obstacle duck bus).

Drives the real ``audio.AudioController._audio_callback`` through scripted sequences with
the fake clock of ``audio_callback_harness`` and writes four fixtures::

    uv run python rust/fixtures/gen/audio_callback_a3.py

Steps (a superset of ``audio_callback_harness.run_script``'s):

``{"telemetry": {...}}``          ``update_telemetry_state(dict)``
``{"call": m, "args": [...]}``    ``controller.m(*args)``
``{"hrtf": "<file.npz>"}``        ``load_hrtf(<repo root>/<file>)``
``{"advance": dt}``               advance both fake clocks
``{"blocks": n, "frames": f, "tick": bool, "record": bool}``
    run ``n`` callbacks; ``tick`` advances the clock by ``frames / samplerate`` after each
    one (the clock the dock's stale guard reads), ``record`` keeps the output.

Every number in a script has at most six decimals, so any JSON parser reads it exactly.
Recorded blocks are stored as hex of the little-endian float32 samples (compact, exact).
"""

from __future__ import annotations

import os
from unittest import mock

import numpy as np

from _common import REPO_ROOT, write_fixture
from audio_callback_harness import FakeClock, make_controller

import audio  # noqa: E402

FR = 512


def r6(x):
    return round(float(x), 6)


def run(script, config=None):
    clock = FakeClock()
    blocks = []
    with mock.patch.object(audio.time, "monotonic", clock), mock.patch.object(
        audio.time, "perf_counter", clock
    ):
        ctrl = make_controller(config)
        for step in script:
            if "telemetry" in step:
                ctrl.update_telemetry_state(dict(step["telemetry"]))
            elif "call" in step:
                getattr(ctrl, step["call"])(*step.get("args", []))
            elif "hrtf" in step:
                ok = ctrl.load_hrtf(os.path.join(REPO_ROOT, step["hrtf"]))
                assert ok, "HRTF set failed to load"
            elif "advance" in step:
                clock.t += float(step["advance"])
            elif "blocks" in step:
                frames = int(step["frames"])
                for _ in range(int(step["blocks"])):
                    out = np.zeros((frames, 2), dtype=np.float32)
                    ctrl._audio_callback(out, frames, None, None)
                    if step.get("tick", False):
                        clock.t += frames / ctrl.samplerate
                    if step.get("record", True):
                        blocks.append(
                            {
                                "left": out[:, 0].astype("<f4").tobytes().hex(),
                                "right": out[:, 1].astype("<f4").tobytes().hex(),
                            }
                        )
            else:
                raise ValueError(f"unknown step {step!r}")
    return blocks


def blk(n=1, frames=FR, tick=True, record=True):
    return {"blocks": n, "frames": frames, "tick": tick, "record": record}


def tel(**kw):
    return {"telemetry": {k: (r6(v) if isinstance(v, float) else v) for k, v in kw.items()}}


# -----------------------------------------------------------------------------------------
# Articulation
# -----------------------------------------------------------------------------------------


def hydro_sweep(frames_cycle=(FR,)):
    # A frame bend of exactly 0.0 hands the channel to the trailer source (and so disarms
    # the centre click), so the crossings land on small non-zero values to click; the
    # final exact zeros show the hand-off.
    vals = (
        [0.0, 0.0]
        + list(np.linspace(0.0, 1.0, 12))
        + [1.0, 1.0]
        + list(np.linspace(1.0, -1.04, 18))
        + [-1.0, -1.0]
        + list(np.linspace(-1.0, 0.01, 8))
        + [0.01] * 4
        + [0.0] * 3
    )
    script = []
    for i, x in enumerate(vals):
        script.append(tel(last_actual_steering=float(x)))
        script.append(blk(frames=frames_cycle[i % len(frames_cycle)]))
    return script


def hydro_trailer():
    s = []
    # Trailer is the source (frame bend exactly 0), going forward: ducked by the glide.
    s.append(tel(last_actual_steering=0.0, trailer_artic=0.5, trailer_reverse=False))
    s.append(blk(6))
    # Reverse: the duck glides back out.
    s.append(tel(trailer_reverse=True))
    s.append(blk(6))
    # Uncouple at an angle: straight to 0.0 is a source-preserving crossing -> click.
    s.append(tel(trailer_artic=0.0))
    s.append(blk(4))
    # Arm on the trailer, then the frame takes over: a source change disarms the click.
    s.append(tel(trailer_artic=0.4))
    s.append(blk(3))
    s.append(tel(last_actual_steering=0.02))
    s.append(blk(6))
    return s


HYDRO_CASES = [
    {"name": "hydro_pan_sweep", "config": None, "script": hydro_sweep()},
    {
        "name": "hydro_hrtf_sweep",
        "config": None,
        "script": [{"hrtf": "hrtf_mit_kemar.npz"}] + hydro_sweep((FR, 480, 256)),
    },
    {"name": "hydro_trailer", "config": None, "script": hydro_trailer()},
]


# -----------------------------------------------------------------------------------------
# Implement
# -----------------------------------------------------------------------------------------

# 20 Hz telemetry against ~94 Hz blocks: one update, then five blocks (close enough; the
# estimator measures against its own elapsed time, whatever the ratio).
def impl_approach():
    s = [tel(implement_flags=5, implement_min_clearance=2.5, implement_activity=0.0,
             implement_tilt_deg=0.0, implement_lift=0.0)]
    s.append(blk(4))
    # Drive into a pile with the bucket still: closing alone opens the gate.
    for i in range(24):
        s.append(tel(implement_min_clearance=r6(2.0 - 0.08 * i)))
        s.append(blk(5, record=(i % 3 == 0) or i > 18))
    # Contact, then jack the machine up.
    for i in range(12):
        s.append(tel(implement_min_clearance=0.0, implement_lift=r6(0.035 * i),
                     implement_activity=0.5))
        s.append(blk(2))
    # Release: the gate holds through a short gap, then fades for about a second.
    s.append(tel(implement_activity=0.35))
    s.append(blk(10, record=False))
    s.append(blk(2))
    s.append(tel(implement_activity=0.0))
    s.append(blk(4))
    s.append(blk(60, record=False))
    s.append(blk(4))
    # Ground lost (flag 4 off) with the implement still worked: tilt alone.
    s.append(tel(implement_flags=1, implement_activity=0.5))
    s.append(blk(3))
    return s


def impl_tilt():
    s = [tel(implement_flags=1, implement_min_clearance=-1.0, implement_activity=0.5,
             implement_tilt_deg=0.0)]
    s.append(blk(3))
    # First stroke up to a real stop at 28 deg (fallback span), back to level (stop
    # confirmed, span swapped at level), up again on the learned scale.
    up = [28.0 * i / 20.0 for i in range(21)]
    for i, deg in enumerate(up + list(reversed(up)) + up[:12]):
        s.append(tel(implement_tilt_deg=r6(deg)))
        s.append(blk(1, record=(i % 2 == 0)))
    # Down side, with frames that are not a power of two.
    for deg in [-2.0 * i for i in range(10)]:
        s.append(tel(implement_tilt_deg=r6(deg)))
        s.append(blk(1, frames=441))
    # Release while tilted: the step freezes through the fade even as the bucket settles.
    s.append(tel(implement_activity=0.0))
    for deg in (-18.0, -17.0, -15.0, -12.0, -10.0):
        s.append(tel(implement_tilt_deg=deg))
        s.append(blk(2))
    s.append(blk(80, record=False))
    s.append(blk(3))
    # Disabled by config mid-gesture is covered by the not-present branch:
    s.append(tel(implement_flags=0, implement_activity=0.5))
    s.append(blk(2))
    return s


IMPL_CASES = [
    {"name": "impl_approach", "config": None, "script": impl_approach()},
    {
        "name": "impl_tilt",
        "config": {"implement_tilt_tone_dbfs": -14.0, "implement_ground_tone_dbfs": -24.0},
        "script": impl_tilt(),
    },
]


# -----------------------------------------------------------------------------------------
# Docking
# -----------------------------------------------------------------------------------------

PACKET_BLOCKS = 9  # 10 Hz packets against 512-frame blocks at 48 kHz (0.096 s)


def dock_packets(s, n, range_fn, lateral_fn, null_fn, mode="IMPL", acquire=False,
                 bearing_fn=None, record_every=1):
    for i in range(n):
        args = [r6(range_fn(i)), r6(lateral_fn(i)), r6(null_fn(i)), mode, acquire,
                r6(bearing_fn(i)) if bearing_fn else 0.0]
        s.append({"call": "update_dock_target", "args": args})
        # The first three blocks after each packet are kept; the rest only carry state.
        if i % record_every == 0:
            s.append(blk(3))
            s.append(blk(PACKET_BLOCKS - 3, record=False))
        else:
            s.append(blk(PACKET_BLOCKS, record=False))


def dock_impl(hrtf=False):
    s = [{"hrtf": "hrtf_mit_kemar.npz"}] if hrtf else []
    s.append({"call": "set_dock_mode", "args": [True]})
    # Out of range: silent.
    dock_packets(s, 2, lambda i: 7.0, lambda i: 0.0, lambda i: 0.4)
    # Pulse only; lateral swings across.
    dock_packets(s, 4, lambda i: 5.0 - 0.3 * i, lambda i: 0.6 - 0.4 * i, lambda i: 0.4)
    # Pair fades in: well above the band (armed), settles onto it (lock chime).
    dock_packets(s, 4, lambda i: 3.0 - 0.2 * i, lambda i: -0.3, lambda i: 0.4 - 0.1 * i)
    dock_packets(s, 3, lambda i: 2.0, lambda i: 0.0, lambda i: 0.0)
    # Below the band (other tremolo rate), back to the null: second chime.
    dock_packets(s, 3, lambda i: 1.5, lambda i: 0.1, lambda i: -0.3)
    dock_packets(s, 3, lambda i: 1.2, lambda i: 0.0, lambda i: 0.0)
    # Feed stops: stale fade, then the tail drains.
    s.append(blk(14))
    return s


def dock_ramp():
    s = [{"call": "set_dock_mode", "args": [True]}]
    # Implement mode, armed at 0.4 m.
    dock_packets(s, 3, lambda i: 2.0, lambda i: 0.0, lambda i: 0.4)
    # Switch to ramp inside its lock window: no chime (latch and beat env reset).
    dock_packets(s, 3, lambda i: 2.0, lambda i: 0.0, lambda i: 0.2, mode="RAMP")
    # Hunting from behind the mouth: beacon at the true bearing, no pair.
    dock_packets(s, 4, lambda i: -12.0 + 3.0 * i, lambda i: 3.0, lambda i: 0.0,
                 mode="RAMP", acquire=True, bearing_fn=lambda i: 140.0 - 20.0 * i)
    # Into the corridor: handoff pip, the pair comes in, lateral pan with deadzone.
    s.append({"call": "trigger_dock_approach_cue"})
    dock_packets(s, 3, lambda i: 12.0 - i, lambda i: 1.0 - 0.3 * i, lambda i: 9.0,
                 mode="RAMP")
    dock_packets(s, 3, lambda i: 6.0, lambda i: 0.1, lambda i: -6.0, mode="RAMP")
    dock_packets(s, 3, lambda i: 5.0, lambda i: 0.0, lambda i: 0.0, mode="RAMP")
    # Re-arm on 8 degrees and lock again.
    dock_packets(s, 2, lambda i: 4.0, lambda i: 0.0, lambda i: 8.0, mode="RAMP")
    dock_packets(s, 3, lambda i: 4.0, lambda i: 0.0, lambda i: 0.0, mode="RAMP")
    # Instrument switched off: everything fades and the tail drains.
    s.append({"call": "set_dock_mode", "args": [False]})
    s.append(blk(10))
    return s


def dock_ramp_hrtf():
    s = [{"hrtf": "hrtf_sadie_kemar.npz"}, {"call": "set_dock_mode", "args": [True]}]
    dock_packets(s, 3, lambda i: -6.0 + 2.0 * i, lambda i: -2.0, lambda i: 0.0,
                 mode="RAMP", acquire=True, bearing_fn=lambda i: -150.0 + 30.0 * i)
    dock_packets(s, 3, lambda i: 7.0 - i, lambda i: -2.5 + i, lambda i: -3.0, mode="RAMP")
    s.append({"call": "clear_dock_target"})
    s.append(blk(8))
    return s


def earcons():
    s = [{"call": "trigger_slam_cue", "args": ["clear"]}, blk(4, tick=False)]
    s += [{"call": "trigger_slam_cue", "args": ["over"]}, blk(3, frames=400, tick=False)]
    s += [{"call": "trigger_slam_cue", "args": ["committed"]},
          {"call": "trigger_entry_cue"},
          {"call": "trigger_dock_approach_cue"},
          blk(6, tick=False)]
    s += [{"call": "trigger_slam_cue", "args": ["bogus"]}, blk(2, tick=False)]
    return s


DOCK_CASES = [
    {"name": "dock_impl_pan", "config": None, "script": dock_impl()},
    {"name": "dock_impl_hrtf", "config": None, "script": dock_impl(hrtf=True)},
    {"name": "dock_ramp_pan", "config": {"dock_tone_dbfs": -12.0}, "script": dock_ramp()},
    {"name": "dock_ramp_hrtf", "config": None, "script": dock_ramp_hrtf()},
    {"name": "dock_earcons", "config": None, "script": earcons()},
]


# -----------------------------------------------------------------------------------------
# Couplings 1 and 2, and the dock duck bus
# -----------------------------------------------------------------------------------------


def coupling():
    s = [tel(last_actual_steering=0.4)]
    s.append(blk(3))
    # Implement gate opens: the articulation drops IMPL_HYDRO_DUCK_DB one block late.
    s.append(tel(implement_flags=1, implement_activity=0.5, implement_tilt_deg=5.0))
    s.append(blk(5))
    # The dock gate opens on top: DOCK_HYDRO_DUCK_DB, again one block late.
    s.append({"call": "set_dock_mode", "args": [True]})
    dock_packets(s, 2, lambda i: 2.0, lambda i: 0.5, lambda i: 0.2)
    # An urgent hazard (obstacle mode itself off) ducks the dock voice by 6 dB, and only
    # the dock voice: the articulation and implement stay where they were.
    s.append({"call": "update_selected_hazard", "args": [{"state": 2, "bearing": 10.0}]})
    dock_packets(s, 2, lambda i: 2.0, lambda i: 0.5, lambda i: 0.0)
    s.append({"call": "update_selected_hazard", "args": [{"state": 3, "bearing": 10.0}]})
    dock_packets(s, 1, lambda i: 1.8, lambda i: 0.2, lambda i: 0.3)
    s.append({"call": "update_selected_hazard", "args": [None]})
    # Implement released and dock switched off: both ducks glide away.
    s.append(tel(implement_activity=0.0))
    s.append({"call": "set_dock_mode", "args": [False]})
    s.append(blk(12))
    return s


COUPLING_CASES = [{"name": "coupling", "config": None, "script": coupling()}]


def build(cases):
    out = []
    for case in cases:
        blocks = run(case["script"], case["config"])
        out.append(
            {
                "name": case["name"],
                "config": case["config"],
                "script": case["script"],
                "blocks": blocks,
            }
        )
    return {"cases": out}


def main():
    write_fixture("audio_a3_hydro", build(HYDRO_CASES))
    write_fixture("audio_a3_implement", build(IMPL_CASES))
    write_fixture("audio_a3_dock", build(DOCK_CASES[:2]))
    write_fixture("audio_a3_dock_ramp", build(DOCK_CASES[2:]))
    write_fixture("audio_a3_coupling", build(COUPLING_CASES))


if __name__ == "__main__":
    main()
