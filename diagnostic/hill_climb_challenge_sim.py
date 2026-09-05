"""Deterministic lifecycle and aggregation checks for hill-climb reports."""

from __future__ import annotations

import json
import os
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from challenge_results import HillClimbChallengeRecorder, completion_speech


class Clock:
    value = 100.0

    def __call__(self):
        return self.value


clock = Clock()
epoch = datetime(2026, 9, 4, tzinfo=timezone.utc)


def utc_now():
    return epoch + timedelta(seconds=clock.value - 100.0)


def packet(state="onRoad", materials="10:ASPHALT"):
    return {
        "state": state,
        "diagnostic": {"contactMaterials": materials},
        "correction": {"active": False},
    }


def telemetry(x, y, speed=10.0, wheel=10.0, throttle=0.5, brake=0.0, tc=False):
    return {
        "position": {"x": x, "y": y, "z": 50 + (y + 780) * 0.04},
        "speed_ms": speed,
        "ground_speed_ms": speed,
        "wheel_speed_ms": wheel,
        "throttle": throttle,
        "brake": brake,
        "steering": 0.1,
        "traction_control_active": tc,
    }


completed = []
with tempfile.TemporaryDirectory() as temp_dir:
    recorder = HillClimbChallengeRecorder(
        temp_dir,
        clock=clock,
        now_utc=utc_now,
        finalize_callback=lambda summary, auto: completed.append((summary, auto)),
    )
    action = recorder.handle_event({
        "type": "challenge_event",
        "challenge_id": "hill_climb",
        "attempt_id": "sim/one",
        "event": "started",
    })
    assert action == {"capture": True}

    recorder.record(packet(), telemetry(0, -780, speed=8, wheel=8))
    clock.value += 0.05
    recorder.record(packet(), telemetry(0, -779.5, speed=10, wheel=15))
    clock.value += 0.05
    recorder.record(
        packet(materials="20:RUMBLE_STRIP"),
        telemetry(8, -779, speed=12, wheel=17, tc=True),
    )
    clock.value += 0.05
    recorder.record(
        packet(materials="30:GRAVEL"),
        telemetry(11, -778.5, speed=10, wheel=5, throttle=0, brake=0.8),
    )
    clock.value += 0.50  # deliberately rejected packet gap
    recorder.record(packet(state="offRoad"), telemetry(15, -778, speed=0, wheel=0))

    recorder.handle_event({
        "type": "challenge_event",
        "challenge_id": "hill_climb",
        "attempt_id": "sim/one",
        "event": "checkpoint",
        "checkpoint_index": 1,
        "checkpoint_name": "Checkpoint 1",
        "race_time_s": 0.65,
    }, telemetry=telemetry(15, -778, speed=14))
    assert recorder.handle_event({
        "type": "challenge_event",
        "challenge_id": "hill_climb",
        "attempt_id": "sim/one",
        "event": "race_complete",
        "race_time_s": 1.0,
        "recovery_count": 1,
    }) == {"capture": False}
    recorder.handle_event({
        "type": "challenge_event",
        "challenge_id": "hill_climb",
        "attempt_id": "sim/one",
        "event": "attempt_aggregated",
        "official_time_s": 6.0,
        "raw_time_s": 1.0,
        "penalty_s": 5.0,
        "recovery_count": 1,
        "new_best": True,
    })

    assert len(completed) == 1 and completed[0][1] is True
    summary = completed[0][0]
    stats = summary["statistics"]
    assert summary["status"] == "completed"
    assert stats["official_time_s"] == 6.0 and stats["penalty_s"] == 5.0
    assert stats["samples"] == 5
    assert stats["packet_gaps"]["count"] == 1
    assert stats["durations"]["off_asphalt_s"] == 0.1
    assert stats["durations"]["rumble_contact_s"] == 0.05
    assert stats["durations"]["gravel_contact_s"] == 0.05
    assert stats["durations"]["wheelspin_s"] == 0.1
    assert stats["durations"]["lockup_s"] == 0.05
    assert stats["departures"] == {"left": 0, "right": 1, "total": 1}
    assert stats["checkpoints"][0]["speed_mps"] == 14.0
    assert stats["recoveries"] == 1
    assert stats["official_confirmed"]
    assert summary["personal_best"]["new_best"]
    assert "New personal best" in completion_speech(summary)
    report_lines = recorder.report_lines(summary)
    assert any(line.startswith("Started, ") and " at " in line for line in report_lines)
    assert any(line.startswith("Finished, ") and " at " in line for line in report_lines)
    assert any(line.startswith("Average speed") for line in report_lines)
    assert any(line.startswith("Time on asphalt") for line in report_lines)
    assert any(line.startswith("Navigation on-road time") for line in report_lines)
    assert not any(line.startswith("Data quality") for line in report_lines)
    assert any(
        line.startswith("Checkpoint 1 reached at 0.650 seconds into the run")
        for line in report_lines
    )
    history_line = recorder.history_line(summary)
    assert "sim/one" not in history_line and "2026-09-04T" not in history_line

    raw_rows = [
        json.loads(line)
        for line in Path(summary["raw_path"]).read_text(encoding="utf-8").splitlines()
    ]
    sample_rows = [row for row in raw_rows if row.get("kind") == "sample"]
    assert len(sample_rows) == 5
    assert all("ground_speed_ms" in row["telemetry"] for row in sample_rows)
    assert raw_rows[0]["kind"] == "meta" and raw_rows[-1]["kind"] == "end"
    assert Path(summary["summary_path"]).is_file()
    assert recorder.list_reports()[0]["attempt_id"] == "sim/one"

    # BeamNG starts the race once while staging and again when the countdown ends.
    # The second event replaces that short trace without polluting report history.
    completed_count = len(completed)
    recorder.handle_event({
        "type": "challenge_event", "challenge_id": "hill_climb",
        "attempt_id": "sim-staging", "event": "started",
    })
    recorder.record(packet(), telemetry(0, -796, speed=0, throttle=0, brake=1))
    staging_path = recorder._attempt["raw_path"]
    recorder.handle_event({
        "type": "challenge_event", "challenge_id": "hill_climb",
        "attempt_id": "sim-two", "event": "started", "replace_active": True,
    })
    assert len(completed) == completed_count
    assert not staging_path.exists()

    # An actual abort retains the timed trace but does not auto-open it.
    recorder.record(packet(), telemetry(0, -780))
    recorder.handle_event({
        "type": "challenge_event", "challenge_id": "hill_climb",
        "attempt_id": "sim-two", "event": "aborted",
    })
    assert completed[-1][0]["status"] == "aborted" and completed[-1][1] is False

print("hill climb challenge simulation passed")
