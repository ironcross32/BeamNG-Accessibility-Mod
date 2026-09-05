"""Exercise durable road-guidance capture, analysis, and Lua/Python wiring."""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from road_diagnostics import RoadDiagnosticRecorder


class Clock:
    value = 100.0

    def __call__(self):
        return self.value


def packet(
    active=True,
    settled=False,
    candidate=False,
    error=1.0,
    lateral_speed=0.5,
    steering_input=0.2,
    applied_steering=None,
    contact_materials="10:ASPHALT,30:RUBBER",
):
    if applied_steering is None:
        applied_steering = steering_input * 0.9
    return {
        "state": "onRoad",
        "oneWay": False,
        "roadDirections": [0, -180],
        "offRoad": None,
        "junction": None,
        "correction": {
            "active": active,
            "bearing": -12 if active else 0,
            "severity": 0.6 if active else 0,
            "phase": "correct" if active else "idle",
            "settled": settled,
        },
        "diagnostic": {
            "edgeId": "hill-a|hill-b",
            "edgeT": 0.5,
            "roadRadius": 7.5,
            "activeBefore": active or settled,
            "targetSide": 1,
            "targetError": error,
            "targetTolerance": 1.125,
            "settledHeadingTolerance": 6.0,
            "settledLateralSpeedTolerance": 0.6,
            "lateralSpeed": lateral_speed,
            "headingError": 2.0,
            "correctionBearing": -12.0,
            "withinLateral": abs(error) <= 1.125,
            "withinHeading": True,
            "withinLateralSpeed": abs(lateral_speed) <= 0.6,
            "settledCandidate": candidate,
            "steeringInput": steering_input,
            "steering": applied_steering,
            "contactMaterials": contact_materials,
        },
    }


def telemetry(x, steering, ground=10.0, wheel=10.0):
    return {
        "position": {"x": x, "y": 0, "z": 50},
        "ground_speed_ms": ground,
        "wheel_speed_ms": wheel,
        "steering_input": steering,
        "pitch_rad": 0.4,
    }


clock = Clock()
with tempfile.TemporaryDirectory() as temp_dir:
    recorder = RoadDiagnosticRecorder(temp_dir, clock=clock, wall_clock=clock)
    started = recorder.start("hill / climb")
    assert started["active"] and "hill-climb" in started["path"]
    recorder.mark("starting uphill")

    recorder.record(packet(error=1.2), telemetry(0, 0.3))
    clock.value += 0.05
    recorder.record(packet(candidate=True, error=0.2, lateral_speed=0.2), telemetry(1, -0.3))
    clock.value += 0.05
    recorder.record(
        packet(active=False, settled=True, candidate=True, error=0.1, lateral_speed=0.1),
        telemetry(2, 0.0),
        {"settled_triggered": True},
    )

    # A second correction and settlement inside three seconds reproduces the repeated-tone
    # signature. Wheel speed running ahead of ground speed overlaps this correction long
    # enough to become a sustained wheelspin episode in the review.
    clock.value += 1.0
    recorder.record(
        packet(error=-1.4, applied_steering=0.8),
        telemetry(3, 0.4, ground=10, wheel=15),
    )
    clock.value += 0.10
    recorder.record(
        packet(
            error=-0.7,
            applied_steering=-0.8,
            contact_materials="11:DIRT,30:RUBBER",
        ),
        telemetry(4, -0.4, ground=10, wheel=15),
    )
    clock.value += 0.10
    recorder.record(
        packet(error=-0.3, applied_steering=0.8),
        telemetry(5, 0.4, ground=10, wheel=15),
    )
    clock.value += 0.05
    recorder.record(
        packet(active=False, settled=True, candidate=True, error=0.1, lateral_speed=0.1),
        telemetry(6, 0.0),
        {"settled_triggered": True},
    )
    clock.value += 0.05
    recorder.record(packet(active=False), telemetry(7, 0.0))

    summary = recorder.stop()
    assert summary["samples"] == 8
    assert summary["correction"]["settled_audio_triggers_s"] == [0.1, 1.35]
    assert summary["correction"]["repeated_settled"][0]["interval_s"] == 1.25
    assert len(summary["correction"]["episodes"]) == 2
    assert summary["correction"]["longest_settled_candidate_run_samples"] >= 2
    assert summary["correction"]["settled_band"] == {
        "min_lateral_tolerance_m": 1.125,
        "max_lateral_tolerance_m": 1.125,
        "heading_tolerance_deg": 6.0,
        "min_lateral_speed_tolerance_mps": 0.6,
        "max_lateral_speed_tolerance_mps": 0.6,
    }
    assert summary["correction"]["steering_source_samples"]["lua_vehicle"] == 8
    assert summary["correction"]["steering_reversals"] == 2
    assert summary["max_absolute"]["applied_steering"] == 0.8
    assert summary["contact_materials"]["sample_sets"]["11:DIRT,30:RUBBER"] == 1
    assert len(summary["contact_materials"]["changes"]) == 2
    assert summary["slip"]["episodes"][0]["kind"] == "wheelspin"
    assert summary["slip"]["episodes"][0]["during_correction"]
    assert Path(summary["summary_path"]).is_file()
    reviewed = recorder.review()
    assert reviewed["samples"] == summary["samples"]
    assert recorder.list_sessions()[0]["summary"]

    rows = [json.loads(line) for line in Path(started["path"]).read_text().splitlines()]
    assert rows[0]["kind"] == "meta" and rows[-1]["kind"] == "end"
    assert any(row.get("kind") == "marker" for row in rows)

root = Path(__file__).resolve().parent.parent
lua_path = root / "bng_mod" / "lua" / "ge" / "extensions" / "roadDetector.lua"
lua = lua_path.read_text(encoding="utf-8")
for token in (
    'upper == "DIAG_ON"',
    'upper == "DIAG_OFF"',
    "diagnostic.activeBefore",
    "diagnostic.targetTolerance",
    "diagnostic.settledHeadingTolerance",
    "diagnostic.settledLateralSpeedTolerance",
    "diagnostic.settledCandidate",
    "diagnostic.clearTicksAfter",
    "diagnostic.lateralDistance",
    "diagnostic.correctionArmed",
    "diagnostic.steeringInput",
    "diagnostic.contactMaterials",
):
    assert token in lua, token

print("road_diagnostic_sim: all diagnostics passed")
