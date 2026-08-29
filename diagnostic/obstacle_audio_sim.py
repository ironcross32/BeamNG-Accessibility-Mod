"""Deterministic checks for the predictive obstacle protocol and audio state."""

from __future__ import annotations

import logging
import math
import pathlib
import sys
import time

import numpy as np

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

import audio
import beamtel


checks = 0


def check(condition, message):
    global checks
    if not condition:
        raise AssertionError(message)
    checks += 1


check(audio.OBSTACLE_CUE_FREQ_HZ == 413.7, "fundamental must be exactly 413.7 Hz")
check(
    audio.OBSTACLE_CUE_HARMONICS
    == ((1, 1.0), (2, 0.5), (4, 0.25), (6, 1.0 / 6.0), (8, 0.125), (10, 0.1)),
    "spectrum must contain the fundamental and exact even-harmonic 1/n ratios",
)

controller = audio.AudioController(logging.getLogger("obstacle-audio-sim"))
check(controller._is_enabled, "audio dependencies are required for this diagnostic")
controller._obstacle_amp = 1.0
pulse = controller._generate_obstacle_buzz()
check(len(pulse) == int(controller.samplerate * 0.080), "pulse must last 80 ms")
check(pulse[0] == 0.0 and pulse[-1] == 0.0, "raised-cosine edges must land on zero")
check(float(np.max(np.abs(pulse))) <= 1.000001, "normalised pulse must not exceed configured level")
check(float(np.max(np.abs(pulse))) > 0.98, "normalised pulse must use the available peak")

check(math.isclose(audio.obstacle_cue_rate_hz(1, 0), 0.7), "advisory floor must be 0.7 Hz")
check(math.isclose(audio.obstacle_cue_rate_hz(1, 255), 1.5), "advisory ceiling must be 1.5 Hz")
check(math.isclose(audio.obstacle_cue_rate_hz(2, 170), 1.5), "urgent floor must be 1.5 Hz")
check(math.isclose(audio.obstacle_cue_rate_hz(2, 255), 4.0), "urgent ceiling must be 4 Hz")
check(audio.obstacle_cue_rate_hz(3, 255) <= 4.0, "pulse cadence must never exceed 4 Hz")

check(not hasattr(controller, "_obstacle_bearings"), "renderer must not retain per-obstacle arrays")
check(hasattr(controller, "_obstacle_hazard"), "renderer must expose one selected source")
controller._hrtf_user_enabled = False
left_l, left_r = controller._render_directional_pulse(pulse, 35.0)
right_l, right_r = controller._render_directional_pulse(pulse, -35.0)
check(np.sum(left_l * left_l) > np.sum(left_r * left_r), "positive bearing must pan left")
check(np.sum(right_r * right_r) > np.sum(right_l * right_l), "negative bearing must pan right")

hazard = {
    "bearing": 10,
    "urgency": 255,
    "gap": 0.4,
    "state": 3,
    "closing": 4,
    "ttc": 0.1,
    "stopping_margin": -0.8,
}
controller.update_selected_hazard(hazard)
check(controller._obstacle_emergency_latched, "emergency must latch")
check(controller._obstacle_doublet_requested, "emergency transition must request a doublet")
controller._obstacle_doublet_requested = False
controller.update_selected_hazard(hazard)
check(not controller._obstacle_doublet_requested, "latched emergency must not retrigger its doublet")
controller.update_selected_hazard({**hazard, "state": 1, "urgency": 50})
controller._obstacle_below_urgent_since = time.monotonic() - 1.01
controller.update_selected_hazard({**hazard, "state": 1, "urgency": 50})
check(not controller._obstacle_emergency_latched, "one second below urgent must re-arm emergency")
controller.update_selected_hazard(None)
check(controller._obstacle_hazard is None, "clear must remove the selected source")
check(not controller._obstacle_emergency_latched, "clear must immediately re-arm emergency")

check(math.isclose(audio.obstacle_duck_gain(1), 1.0), "advisory must not duck guidance")
check(math.isclose(audio.obstacle_duck_gain(2), 10 ** (-6 / 20)), "urgent duck must be 6 dB")
check(math.isclose(audio.obstacle_duck_gain(3), 10 ** (-12 / 20)), "emergency duck must be 12 dB")
release_start = 1.0
release_end = release_start * math.exp(-0.150 / audio.OBSTACLE_STEADY_RELEASE_S)
check(0.36 < release_end < 0.38, "steady voice must use a 150 ms release time constant")

kind, parsed = beamtel.parse_obstacle_packet("1,1,12.5,220,3.4,2,5.0,0.68,1.71")
check(kind == "static" and parsed["state"] == 2 and not parsed["legacy"],
      "extended packet tail must be accepted")
kind, parsed = beamtel.parse_obstacle_packet("1,2,-30,80,8,15,240,4")
check(parsed["bearing"] == 15 and parsed["state"] == 2 and parsed["legacy"],
      "legacy multi-obstacle packet must select its highest urgency")
kind, parsed = beamtel.parse_obstacle_packet("1,1,0,255,0.1")
check(parsed["state"] == 2, "legacy centre distance must never infer emergency")
kind, parsed = beamtel.parse_obstacle_packet("0")
check(kind == "clear" and parsed is None, "clear packet must remain compatible")

# Exercise the real callback state machine: immediate doublet, one scheduled source, then
# steady emergency attack and a non-abrupt clear release.
render = audio.AudioController(logging.getLogger("obstacle-render-sim"))
render._regenerate_waveforms()
render.set_obstacle_mode(True)
render.update_selected_hazard(hazard)
block = np.zeros((480, 2), dtype=np.float32)
render._audio_callback(block, len(block), None, None)
check(np.max(np.abs(block)) > 0, "emergency transition must render immediately")
check(len(render._obstacle_pulse_queue) == 2, "doublet must be the only scheduled source")
check(not render._obstacle_doublet_requested, "callback must consume the doublet request once")
render.update_selected_hazard(hazard)
check(len(render._obstacle_pulse_queue) == 2, "packet refresh must not restart the doublet")
for _ in range(40):
    block.fill(0)
    render._audio_callback(block, len(block), None, None)
check(not render._obstacle_pulse_queue, "doublet queue must drain")
block.fill(0)
render._audio_callback(block, len(block), None, None)
attack_env = render._obstacle_steady_env
check(attack_env > 0 and np.max(np.abs(block)) > 0, "steady emergency spectrum must attack")
render.update_selected_hazard(None)
block.fill(0)
render._audio_callback(block, len(block), None, None)
check(0 < render._obstacle_steady_env < attack_env, "clear must release rather than hard-cut steady tone")

print(f"obstacle audio simulation: {checks} checks passed")
