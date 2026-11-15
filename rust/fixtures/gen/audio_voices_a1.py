"""Golden buffers for the A1 callback voices: placement ping, compass clicks, pedals and
steering, attitude, drift, node/clickspot beeps, low speed, wheel slip, heading guidance,
the line-up guide, gear tones and assistant cues.

Each case drives the real ``AudioController._audio_callback`` through a scripted state
sequence via ``audio_callback_harness.run_script`` and stores the clipped stereo output
as base64 little-endian float32. One fixture file per voice keeps each under ~1.5 MB::

    uv run python rust/fixtures/gen/audio_voices_a1.py

Steps are the harness's, plus ``{"hrtf": "<file>"}``, which loads that repository-root
HRTF set (``load_hrtf``) -- rewritten to an absolute-path ``call`` for Python, kept as is
in the fixture so the Rust test resolves the same file from its own repository root.
"""

from __future__ import annotations

import base64
import os

import numpy as np

from _common import REPO_ROOT, write_fixture
from audio_callback_harness import run_script

F = 512
HRTF = {"hrtf": "hrtf_mit_kemar.npz"}


def tel(**state):
    return {"call": "update_telemetry_state", "args": [state]}


def call(name, *args):
    return {"call": name, "args": list(args)}


def blocks(n, frames=F):
    return {"blocks": n, "frames": frames}


def _python_script(script):
    out = []
    for step in script:
        if "hrtf" in step:
            out.append({"call": "load_hrtf", "args": [os.path.join(REPO_ROOT, step["hrtf"])]})
        else:
            out.append(step)
    return out


def _b64(values):
    return base64.b64encode(np.asarray(values, dtype="<f4").tobytes()).decode("ascii")


def ping_cases():
    s1 = [call("trigger_placement_ping", 2, 45.0, 1.0), blocks(3),
          call("trigger_placement_ping", 0, -90.0, 1.5), blocks(3),
          call("trigger_placement_ping", 4, 180.0, 0.75), blocks(4)]
    return [
        {"name": "ping_pan", "config": None, "script": s1},
        {"name": "ping_hrtf", "config": None, "script": [HRTF] + s1},
        {"name": "ping_level", "config": {"placement_ping_volume_db": -4.0},
         "script": [call("trigger_placement_ping", 1, 30.0, 1.25), blocks(4)]},
    ]


def click_cases():
    s1 = [call("trigger_compass_click", 30.0, 1.0), blocks(2),
          call("trigger_compass_highlight", -60.0, 1.2), blocks(2),
          call("trigger_cam_compass_click", 135.0, 0.9), blocks(2),
          call("trigger_cam_compass_highlight", -150.0, 1.1),
          call("trigger_compass_click", 0.0, 0.8), blocks(3)]
    return [
        {"name": "clicks_pan", "config": None, "script": s1},
        {"name": "clicks_hrtf", "config": None, "script": [HRTF] + s1},
        {"name": "clicks_level", "config": {"compass_click_level_dbfs": -3.0},
         "script": [call("trigger_compass_highlight", 90.0, 1.0),
                    call("trigger_cam_compass_click", -45.0, 1.0), blocks(3)]},
    ]


def pedal_cases():
    s = [tel(pedal_tones_active=True, last_clutch=0.25, last_brake=0.75, last_throttle=1.0,
             last_steering_input=0.5), blocks(4),
         tel(last_clutch=0.0, last_brake=0.0, last_throttle=0.5, last_steering_input=-0.8),
         blocks(4),
         tel(last_steering_input=0.0), blocks(2),
         tel(pedal_tones_active=False), blocks(1)]
    return [{"name": "pedals_steer", "config": None, "script": s}]


def attitude_cases():
    s = [tel(last_roll_rad=0.3, last_pitch_rad=-0.2), blocks(4),
         tel(last_roll_rad=-0.5, last_pitch_rad=0.4), blocks(4),
         tel(last_roll_rad=0.0, last_pitch_rad=0.0), blocks(4),
         tel(inverted=True, last_roll_rad=0.4), blocks(1)]
    return [
        {"name": "attitude_pan", "config": None, "script": s},
        {"name": "attitude_hrtf", "config": None, "script": [HRTF] + s},
        {"name": "attitude_levels",
         "config": {"pitch_roll_max_dbfs": -12.0, "pitch_roll_min_dbfs": -30.0},
         "script": [tel(last_roll_rad=0.6, last_pitch_rad=0.25), blocks(4)]},
        {"name": "attitude_disabled", "config": {"pitch_roll_tones_enabled": False},
         "script": [tel(last_roll_rad=0.6, last_pitch_rad=0.25), blocks(2)]},
    ]


def drift_cases():
    s = [tel(drift_alert_active=True, drift_rate=3.0, drift_pan=-0.5), blocks(4),
         tel(drift_rate=8.0, drift_pan=0.75), blocks(4),
         tel(drift_alert_active=False), blocks(8)]
    return [{"name": "drift", "config": None, "script": s}]


def node_cases():
    s = [call("trigger_node_hover_beep", 0.75, False), blocks(2),
         call("trigger_clickspot_beep", False), blocks(2),
         call("trigger_node_hover_beep", 0.0, True),
         call("trigger_clickspot_beep", True), blocks(4)]
    return [{"name": "node_clickspot", "config": None, "script": s}]


def lowspeed_cases():
    s = [tel(ls_clicks_active=True, ls_speed_mph=10.0, ls_accel=2.0), blocks(30),
         tel(ls_speed_mph=4.0, ls_accel=-6.0), blocks(20),
         tel(ls_clicks_active=False, ls_stopped=True, ls_speed_mph=0.0, ls_accel=0.0),
         call("trigger_lowspeed_stopped"), blocks(12)]
    return [
        {"name": "lowspeed_pan", "config": None, "script": s},
        {"name": "lowspeed_hrtf", "config": None, "script": [HRTF] + s},
        {"name": "lowspeed_levels",
         "config": {"lowspeed_click_level_dbfs": -8.0, "hrtf_front_emphasis_db": -3.0},
         "script": [HRTF, tel(ls_clicks_active=True, ls_speed_mph=20.0, ls_accel=3.0),
                    blocks(24)]},
    ]


def slip_cases():
    base = dict(slip_active=True, slip_kind=1, slip_mag=3.0, slip_ratio=0.4,
                slide_active=False, slide_deg=0.0, slip_azimuth_deg=60.0)
    s = [tel(**base), blocks(6),
         tel(slip_ratio=0.9), blocks(4),
         tel(slip_kind=-1, slip_ratio=0.5, slip_azimuth_deg=-120.0), blocks(4),
         tel(slip_active=False, slide_active=True, slide_deg=30.0, slip_azimuth_deg=-30.0),
         blocks(6),
         tel(slide_active=False), blocks(10)]
    return [
        {"name": "slip_pan", "config": None, "script": s},
        {"name": "slip_hrtf", "config": None, "script": [HRTF] + s},
        {"name": "slip_level",
         "config": {"slip_tone_level_dbfs": -10.0, "hrtf_distance_gain_db": -4.0},
         "script": [HRTF, tel(**dict(base, slide_active=True, slide_deg=40.0)), blocks(6)]},
    ]


def heading_cases():
    s = [tel(guidance_active=True, guidance_error_deg=3.0), blocks(4),
         tel(guidance_error_deg=-8.0), blocks(4),
         tel(guidance_error_deg=0.25), blocks(6),
         tel(guidance_active=False), blocks(1)]
    return [{"name": "heading", "config": None, "script": s}]


def lineup_cases():
    s = [call("set_lineup", True, 0.75, 2.0, 1.5, 30.0), blocks(4),
         call("set_lineup", True, 0.5, 1.0, 1.25, -60.0), blocks(4),
         {"advance": 1.0}, blocks(3),
         call("set_lineup", True, 0.25, 3.0, 2.0, 170.0), blocks(3),
         call("set_lineup", False), blocks(12)]
    return [
        {"name": "lineup_pan", "config": None, "script": s},
        {"name": "lineup_hrtf", "config": None, "script": [HRTF] + s},
    ]


def gear_assistant_cases():
    return [
        {"name": "gear_tones", "config": None,
         "script": [call("trigger_gear_tone", 1), blocks(3),
                    call("trigger_gear_tone", 3, True), blocks(2),
                    call("trigger_gear_tone", -1), blocks(16)]},
        {"name": "gear_tones_custom",
         "config": {"gear_tone_level_dbfs": -9.0, "gear_tone_octave": 5},
         "script": [call("trigger_gear_tone", 2), blocks(12)]},
        {"name": "assistant_cues_hrtf", "config": {"hrtf_enabled": True},
         "script": [HRTF, call("trigger_assistant_cue", "junction", ["left", "straight", "right"]),
                    blocks(60)]},
        {"name": "assistant_cues_pan", "config": {"hrtf_enabled": False},
         "script": [call("trigger_assistant_cue", "junction", ["left", "right"]), blocks(10),
                    call("trigger_assistant_cue", "off"), blocks(40)]},
    ]


GROUPS = {
    "audio_a1_ping": ping_cases,
    "audio_a1_clicks": click_cases,
    "audio_a1_pedals": pedal_cases,
    "audio_a1_attitude": attitude_cases,
    "audio_a1_drift": drift_cases,
    "audio_a1_node": node_cases,
    "audio_a1_lowspeed": lowspeed_cases,
    "audio_a1_slip": slip_cases,
    "audio_a1_heading": heading_cases,
    "audio_a1_lineup": lineup_cases,
    "audio_a1_gear_assistant": gear_assistant_cases,
}


def main():
    for name, make in GROUPS.items():
        out = []
        for case in make():
            recorded = run_script(_python_script(case["script"]), case["config"])
            out.append({
                "name": case["name"],
                "config": case["config"],
                "script": case["script"],
                "blocks": [{"left": _b64(b["left"]), "right": _b64(b["right"])} for b in recorded],
            })
        write_fixture(name, {"cases": out})


if __name__ == "__main__":
    main()
