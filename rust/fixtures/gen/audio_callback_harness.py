"""Drive the real ``audio.AudioController._audio_callback`` offline through a scripted
state sequence and record the stereo output, for golden-buffer tests of the Rust port.

Used two ways:

* ``uv run python rust/fixtures/gen/audio_callback_harness.py`` writes
  ``rust/fixtures/data/audio_callback_frame.json`` (the frame's own cases).
* Wave 2 generators import it: ``from audio_callback_harness import run_script`` and
  write their own fixture from the returned blocks.

A script is a list of steps, run in order on one controller:

``{"call": "<method>", "args": [...], "kwargs": {...}}``
    call ``controller.<method>(*args, **kwargs)`` (any method, private ones included)
``{"set": "<attr>", "value": v}``
    ``setattr(controller, attr, v)`` -- what the sims do to reach internal state
``{"clock": t}`` / ``{"advance": dt}``
    set or advance the fake clock that replaces BOTH ``time.monotonic`` and
    ``time.perf_counter`` inside ``audio``
``{"blocks": n, "frames": f}``
    run ``n`` callbacks of ``f`` frames; each block's ``(left, right)`` is recorded

The controller is built with no stream, its pedal detunes zeroed (Python draws them from
an unseeded RNG; the Rust harness zeroes them too), ``_regenerate_waveforms()`` run, and
then ``apply_config(config)`` if a config is given. The clock starts at 100.0.
"""

from __future__ import annotations

import logging
from unittest import mock

from _common import write_fixture

import audio  # noqa: E402  (the repository root is on sys.path via _common)

START_CLOCK = 100.0


class FakeClock:
    def __init__(self, t=START_CLOCK):
        self.t = float(t)

    def __call__(self):
        return self.t


def make_controller(config=None):
    """A controller with no stream, zero detune, waveforms generated, config applied."""
    ctrl = audio.AudioController(logging.getLogger("audio_callback_harness"))
    ctrl._detune_c = ctrl._detune_b = ctrl._detune_t = 0.0
    ctrl._regenerate_waveforms()
    if config is not None:
        ctrl.apply_config(config)
    return ctrl


def run_script(script, config=None):
    """Run ``script`` (see the module docstring); return the recorded blocks as a list of
    ``{"left": [...], "right": [...]}`` with plain Python floats (exact float32 values)."""
    import numpy as np

    clock = FakeClock()
    blocks = []
    with mock.patch.object(audio.time, "monotonic", clock), mock.patch.object(
        audio.time, "perf_counter", clock
    ):
        ctrl = make_controller(config)
        for step in script:
            if "call" in step:
                getattr(ctrl, step["call"])(*step.get("args", []), **step.get("kwargs", {}))
            elif "set" in step:
                setattr(ctrl, step["set"], step["value"])
            elif "clock" in step:
                clock.t = float(step["clock"])
            elif "advance" in step:
                clock.t += float(step["advance"])
            elif "blocks" in step:
                frames = int(step["frames"])
                for _ in range(int(step["blocks"])):
                    out = np.zeros((frames, 2), dtype=np.float32)
                    ctrl._audio_callback(out, frames, None, None)
                    blocks.append(
                        {
                            "left": [float(x) for x in out[:, 0]],
                            "right": [float(x) for x in out[:, 1]],
                        }
                    )
            else:
                raise ValueError(f"unknown step {step!r}")
    return blocks


# The frame's own cases: silence in, silence out; and the simplest one-shot.
CASES = [
    {
        "name": "silence",
        "config": None,
        "script": [{"blocks": 4, "frames": 480}],
    },
    {
        "name": "check_engine_buzzer",
        "config": {"check_engine_buzzer_level_dbfs": -12.0},
        "script": [
            {"call": "trigger_check_engine_buzzer"},
            {"blocks": 3, "frames": 480},
        ],
    },
    {
        "name": "shift_tone",
        "config": None,
        "script": [
            {"call": "update_telemetry_state", "args": [{"shift_active": True}]},
            {"blocks": 2, "frames": 512},
        ],
    },
]


def main():
    out = []
    for case in CASES:
        out.append(
            {
                "name": case["name"],
                "config": case["config"],
                "script": case["script"],
                "blocks": run_script(case["script"], case["config"]),
            }
        )
    write_fixture("audio_callback_frame", {"cases": out})


if __name__ == "__main__":
    main()
