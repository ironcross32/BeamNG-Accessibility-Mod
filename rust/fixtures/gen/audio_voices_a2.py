"""Golden buffers for the A2 callback voices: the vehicle scanner (with and without the
dock gate and the coupler), the Old Cannon elevation pulse, coupler tracking, the
predictive obstacle cue and the obstacle duck buses, the road beacon / correction /
settled sweep / chime and junction queues, the route beacon, coordinate guidance, the
terrain scan playback crossfade, and the junction stop bar.

Each case drives the real ``AudioController._audio_callback`` through a scripted state
sequence via ``audio_callback_harness.run_script`` and stores the clipped stereo output
as base64 little-endian float32, one fixture file per group (each under ~1.5 MB)::

    uv run python rust/fixtures/gen/audio_voices_a2.py

Steps are the harness's, plus two that are rewritten before Python runs them and kept as
written in the fixture so the Rust test can rebuild them:

``{"hrtf": "<file>"}``
    ``load_hrtf`` on that repository-root HRTF set
``{"scan": [n, seed]}``
    ``play_scan`` of a deterministic ``(n, 2)`` float32 buffer (see ``scan_buffer``)
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


def advance(dt):
    return {"advance": dt}


def scan_buffer(n, seed):
    """Integer arithmetic, one float64 division, one float32 rounding: reproducible."""
    k = np.arange(n, dtype=np.int64)
    left = (((k * 37 + seed) % 200) - 100) / 400.0
    right = (((k * 53 + seed * 3) % 160) - 80) / 320.0
    return np.stack([left, right], axis=1).astype(np.float32)


def _python_script(script):
    out = []
    for step in script:
        if "hrtf" in step:
            out.append({"call": "load_hrtf", "args": [os.path.join(REPO_ROOT, step["hrtf"])]})
        elif "scan" in step:
            n, seed = step["scan"]
            out.append({"call": "play_scan", "args": [scan_buffer(n, seed)]})
        else:
            out.append(step)
    return out


def _b64(values):
    return base64.b64encode(np.asarray(values, dtype="<f4").tobytes()).decode("ascii")


# --- Scanner, cannon aim, coupler --------------------------------------------------------


def scanner_script():
    return [
        call("set_scan_mode", True),
        call("update_scanner_target", 20.0, 15.0), blocks(6),
        advance(0.05), call("update_scanner_target", 24.0, 13.0), blocks(6),
        # Aligned: the bright waveform, and the EMA walking in from +24.
        advance(0.05), call("update_scanner_target", -1.0, 5.0), blocks(12),
        # Steering-locked steady tone fades in.
        tel(last_steering_input=0.6), blocks(10),
        # Across the rear seam: +179 then -179 is a two-degree move.
        advance(0.05), call("update_scanner_target", 179.0, 30.0), blocks(4),
        advance(0.05), call("update_scanner_target", -179.0, 31.0), blocks(4),
        tel(last_steering_input=0.0), blocks(6),
        call("set_scan_mode", False), blocks(3),
    ]


def scanner_cases():
    s = scanner_script()
    return [
        {"name": "scanner_pan", "config": None, "script": s},
        {"name": "scanner_hrtf", "config": None, "script": [HRTF] + s},
        {"name": "scanner_config",
         "config": {"scanner_base_freq_hz": 700.0, "scanner_pitch_offset_oct": 1.5,
                    "scanner_steer_tone_enabled": False},
         "script": [HRTF] + s[:9]},
    ]


def scanner_coupling_cases():
    # The docking instrument (previous block's gate) and coupler tracking both take the
    # scanner's target; the scanner falls silent and drops its tails, then returns.
    dock = [
        call("set_scan_mode", True),
        call("update_scanner_target", 10.0, 3.0), blocks(6),
        call("set_dock_mode", True),
        call("update_dock_target", 2.0, 0.3, 0.05), blocks(8),
        call("set_dock_mode", False), blocks(10),
    ]
    coupler = [
        call("set_scan_mode", True),
        call("update_scanner_target", -15.0, 4.0), tel(last_steering_input=0.7), blocks(8),
        call("set_coupler_tracking", True),
        call("update_coupler_target", 30.0, 3.0, False), blocks(6),
        call("set_coupler_tracking", False), blocks(6),
    ]
    return [
        {"name": "scanner_dock_gate", "config": None, "script": [HRTF] + dock},
        {"name": "scanner_dock_gate_pan", "config": None, "script": dock},
        {"name": "scanner_coupler", "config": None, "script": [HRTF] + coupler},
    ]


def cannon_cases():
    s = [
        call("set_scan_mode", True),
        call("update_scanner_target", 2.0, 40.0),
        call("update_cannon_aim", 5.0, 2.0, 12.0, True, True), blocks(10),
        # Below the target: the other pitch, a slower pulse.
        call("update_cannon_aim", 9.0, 2.0, 6.5, True, True), blocks(10),
        # Inside the deadzone: silent, pulse phase reset.
        call("update_cannon_aim", 6.3, 2.0, 6.5, True, True), blocks(3),
        call("update_cannon_aim", 1.0, 2.0, 20.0, True, True), blocks(6),
        # Stale solution: withheld.
        advance(0.7), blocks(3),
        # Unreachable: withheld.
        call("update_cannon_aim", 1.0, 2.0, 20.0, True, False), blocks(2),
        call("update_cannon_aim", 1.0, 2.0, 20.0, True, True),
        call("set_coupler_tracking", True), blocks(2),
    ]
    return [{"name": "cannon_aim", "config": None, "script": s}]


def coupler_cases():
    s = [
        call("set_coupler_tracking", True),
        call("update_coupler_target", 40.0, 12.0, False), blocks(6),
        call("update_coupler_target", 10.0, 4.0, False), blocks(6),
        call("update_coupler_target", -20.0, 1.0, True), blocks(10),
        call("update_coupler_target", -5.0, 0.2, True), blocks(10),
        call("set_coupler_tracking", False), blocks(2),
    ]
    return [
        {"name": "coupler_pan", "config": None, "script": s},
        {"name": "coupler_hrtf", "config": None, "script": [HRTF] + s},
    ]


# --- Obstacle -----------------------------------------------------------------------------

OF = 2048


def hazard(state, urgency, bearing, closing=0.0):
    return {"bearing": bearing, "urgency": urgency, "gap": 3.0, "state": state,
            "closing": closing, "ttc": 1.0, "stopping_margin": 1.0}


def obstacle_script():
    return [
        call("set_obstacle_mode", True),
        call("update_selected_hazard", hazard(1, 255, 30.0)), blocks(18, OF),
        call("update_selected_hazard", hazard(2, 220, -40.0, 2.0)), blocks(8, OF),
        call("update_selected_hazard", hazard(3, 255, -10.0, 4.0)), blocks(12, OF),
        call("update_obstacle", 2, 0.0, 0.0, 0.0),
        call("update_selected_hazard", None), blocks(4, OF),
        call("update_obstacle", 3, 0.0, 0.0, 0.0), blocks(3, OF),
    ]


def obstacle_pan_cases():
    return [
        {"name": "obstacle_pan", "config": None, "script": obstacle_script()},
        {"name": "obstacle_level", "config": {"obstacle_buzz_volume_db": -9.0},
         "script": [call("set_obstacle_mode", True),
                    call("update_selected_hazard", hazard(3, 255, 50.0, 3.0)),
                    blocks(10, OF)]},
    ]


def obstacle_hrtf_cases():
    return [{"name": "obstacle_hrtf", "config": None, "script": [HRTF] + obstacle_script()}]


def obstacle_duck_cases():
    # Urgent then emergency hazards duck the scanner, road (beacon + route beacon) and
    # coordinate buses; the obstacle cue itself is never ducked.
    s = [
        call("set_scan_mode", True), call("update_scanner_target", 30.0, 6.0),
        call("set_road_mode", True),
        call("update_road_guidance", "offRoad", {"bearing": -30.0, "distance": 4.0}, None),
        call("set_route_beacon_mode", True), call("update_route_beacon", True, 70.0, 40.0),
        tel(coord_guidance_active=True, coord_guidance_error_deg=-80.0),
        blocks(6),
        call("set_obstacle_mode", True),
        call("update_selected_hazard", hazard(2, 200, 0.0, 1.0)), blocks(8),
        call("update_selected_hazard", hazard(3, 255, 5.0, 3.0)), blocks(8),
        call("update_selected_hazard", None), blocks(4),
    ]
    return [{"name": "obstacle_ducks", "config": None, "script": s}]


# --- Road, route beacon ---------------------------------------------------------------------


def road_script():
    return [
        call("set_road_mode", True),
        call("update_road_guidance", "offRoad", {"bearing": 40.0, "distance": 10.0}, None),
        blocks(30),
        call("update_road_guidance", "offRoad", {"bearing": -120.0, "distance": 2.0}, None),
        blocks(30),
        # Back on the road: the beacon is cut, a correction starts.
        call("update_road_guidance", "onRoad", None,
             {"active": True, "bearing": 15.0, "severity": 0.5, "phase": "correct"}),
        blocks(30),
        call("update_road_guidance", "onRoad", None,
             {"active": True, "bearing": -50.0, "severity": 0.9, "phase": "correct"}),
        blocks(20),
        call("update_road_guidance", "onRoad", None,
             {"active": True, "bearing": -5.0, "severity": 0.4, "phase": "unwind"}),
        blocks(40),
        call("update_road_guidance", "onRoad", None,
             {"active": False, "bearing": 0, "severity": 0, "phase": "idle", "settled": True}),
        blocks(10),
        call("trigger_road_orientation_chime", 20.0, -160.0), blocks(30),
        call("trigger_road_junction_earcon"), blocks(12),
        call("trigger_road_junction_entry_earcon"), blocks(14),
        # Route beacon alongside, then cleared mid-pulse.
        call("set_route_beacon_mode", True),
        call("update_route_beacon", True, -75.0, 200.0), blocks(30),
        call("update_route_beacon", True, 120.0, 20.0), blocks(20),
        call("update_route_beacon", False), blocks(2),
        call("set_road_mode", False), blocks(2),
    ]


def road_pan_cases():
    return [{"name": "road_pan", "config": None, "script": road_script()}]


def road_level_cases():
    return [
        {"name": "road_levels",
         "config": {"road_beep_volume_db": -8.0, "road_correction_volume_db": -16.0,
                    "road_junction_volume_db": -10.0, "route_beacon_volume_db": -12.0},
         "script": [call("set_road_mode", True),
                    call("update_road_guidance", "offRoad",
                         {"bearing": 10.0, "distance": 1.0}, None), blocks(12),
                    call("update_road_guidance", "onRoad", None,
                         {"active": True, "bearing": 30.0, "severity": 0.2}), blocks(12),
                    call("set_route_beacon_mode", True),
                    call("update_route_beacon", True, 45.0, 0.0), blocks(12)]},
    ]


def road_hrtf_cases():
    return [{"name": "road_hrtf", "config": None, "script": [HRTF] + road_script()}]


# --- Coordinate guidance, scan playback ----------------------------------------------------


def coord_cases():
    s = [
        tel(coord_guidance_active=True, coord_guidance_error_deg=60.0), blocks(8),
        tel(coord_guidance_error_deg=-120.0), blocks(8),
        tel(coord_guidance_error_deg=179.0), blocks(4),
        tel(coord_guidance_error_deg=2.0), blocks(14),
        tel(coord_guidance_error_deg=-30.0), blocks(6),
        tel(coord_guidance_active=False), blocks(2),
    ]
    return [
        {"name": "coord_pan", "config": None, "script": s},
        {"name": "coord_hrtf", "config": None, "script": [HRTF] + s},
    ]


def scan_cases():
    s = [
        {"scan": [3000, 1]}, blocks(3),
        # A scan landing mid-playback crossfades the old one out over 15 ms.
        {"scan": [4000, 7]}, blocks(4),
        {"scan": [900, 11]}, blocks(6, 300),
        {"scan": [2000, 3]}, blocks(2, 100), blocks(3),
    ]
    return [
        {"name": "scan_crossfade", "config": None, "script": s},
        {"name": "scan_disabled", "config": {"scan_tones_enabled": False},
         "script": [{"scan": [3000, 1]}, blocks(2)]},
    ]


# --- Junction stop bar -------------------------------------------------------------------

JF = 1024


def jtarget(signed, speed, tid="junction:T"):
    return {"id": tid, "distance": max(0.0, signed), "signedDistance": signed,
            "speed": speed, "action": "approach", "source": "junctionBoundary"}


def junction_cases():
    s = [
        call("update_junction_proximity", jtarget(120.0, 20.0)), blocks(12, JF),
        call("update_junction_proximity", jtarget(12.0, 20.0)), blocks(10, JF),
        call("update_junction_proximity", jtarget(0.0, 5.0)), blocks(8, JF),
        call("update_junction_proximity", jtarget(-0.5, 0.0)), blocks(12, JF),
        call("update_junction_proximity", jtarget(0.3, 3.0)), blocks(4, JF),
        call("update_junction_proximity", jtarget(-0.2, 3.0)), blocks(3, JF),
        call("update_junction_proximity", jtarget(10.0, 0.0, "junction:W")), blocks(20, JF),
        # A dead feed: stale after 0.75 s, released.
        advance(1.0), blocks(3, JF),
    ]
    return [
        {"name": "stop_bar", "config": None, "script": s},
        {"name": "stop_bar_level", "config": {"road_junction_volume_db": -6.0},
         "script": [call("update_junction_proximity", jtarget(30.0, 8.0)), blocks(12, JF)]},
        {"name": "stop_bar_disabled", "config": {"road_junction_proximity_enabled": False},
         "script": [call("update_junction_proximity", jtarget(30.0, 8.0)), blocks(2, JF)]},
    ]


GROUPS = {
    "audio_a2_scanner": scanner_cases,
    "audio_a2_scanner_coupling": scanner_coupling_cases,
    "audio_a2_cannon": cannon_cases,
    "audio_a2_coupler": coupler_cases,
    "audio_a2_obstacle_pan": obstacle_pan_cases,
    "audio_a2_obstacle_hrtf": obstacle_hrtf_cases,
    "audio_a2_obstacle_ducks": obstacle_duck_cases,
    "audio_a2_road_pan": road_pan_cases,
    "audio_a2_road_levels": road_level_cases,
    "audio_a2_road_hrtf": road_hrtf_cases,
    "audio_a2_coord": coord_cases,
    "audio_a2_scan_playback": scan_cases,
    "audio_a2_junction": junction_cases,
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
