"""Capture and summarize road-guidance sessions for live driving diagnosis."""

from __future__ import annotations

import json
import math
import os
import re
import threading
import time
from datetime import datetime, timezone
from pathlib import Path


SCHEMA_VERSION = 1
REPEATED_SETTLED_WINDOW_S = 3.0
STEERING_DEADZONE = 0.08
BEARING_DEADZONE_DEG = 3.0
SLIP_ABS_THRESHOLD_MS = 1.5
SLIP_REL_THRESHOLD = 0.25
SLIP_MIN_GROUND_MS = 2.0
SLIP_SUSTAIN_S = 0.15
_SAFE_LABEL = re.compile(r"[^A-Za-z0-9_-]+")


def _finite(value):
    try:
        value = float(value)
    except (TypeError, ValueError):
        return None
    return value if math.isfinite(value) else None


def _sign(value, deadzone):
    value = _finite(value)
    if value is None or abs(value) <= deadzone:
        return 0
    return 1 if value > 0 else -1


def _round(value, digits=3):
    value = _finite(value)
    return None if value is None else round(value, digits)


def _close_episode(episodes, current, end_t, outcome):
    if current is None:
        return None
    current["end_s"] = round(end_t, 3)
    current["duration_s"] = round(max(0.0, end_t - current["start_s"]), 3)
    current["outcome"] = outcome
    current["phase_transitions"] = max(0, len(current.pop("phases", [])) - 1)
    episodes.append(current)
    return None


def analyze_records(records, path=None):
    """Return a compact, evidence-oriented summary of recorded sample dictionaries."""
    samples = [row for row in records if row.get("kind") == "sample"]
    markers = [
        {"t": _round(row.get("t")), "note": str(row.get("note", ""))}
        for row in records
        if row.get("kind") == "marker"
    ]
    if not samples:
        return {
            "schema": SCHEMA_VERSION,
            "path": str(path) if path else None,
            "samples": 0,
            "duration_s": 0.0,
            "assessment_flags": ["no road-guidance samples were recorded"],
        }

    duration = max(0.0, float(samples[-1].get("t", 0.0)))
    distance_m = 0.0
    previous_pos = None
    state_counts = {}
    edge_ids = set()
    missing_diagnostic = 0
    leading_missing_diagnostic = 0
    saw_diagnostic = False
    eligible = 0
    within_lateral = 0
    within_heading = 0
    within_lateral_speed = 0
    settled_candidates = 0
    longest_candidate_run = 0
    candidate_run = 0
    settled_times = []
    audio_settled_times = []
    steering_reversals = 0
    steering_source_samples = {"lua_vehicle": 0, "telemetry": 0}
    bearing_reversals = 0
    target_crossings = 0
    last_steer_sign = 0
    last_bearing_sign = 0
    last_target_sign = 0
    correction_samples = 0
    max_abs = {
        "target_error_m": 0.0,
        "lateral_speed_mps": 0.0,
        "heading_error_deg": 0.0,
        "correction_bearing_deg": 0.0,
        "steering_input": 0.0,
        "applied_steering": 0.0,
        "raw_slip_mps": 0.0,
        "vehicle_pitch_deg": 0.0,
    }
    drive = {
        "start_z_m": None,
        "end_z_m": None,
        "min_ground_speed_mps": None,
        "max_ground_speed_mps": 0.0,
        "max_throttle": 0.0,
        "max_brake": 0.0,
        "traction_control_samples": 0,
    }
    settled_band = {
        "min_lateral_tolerance_m": None,
        "max_lateral_tolerance_m": None,
        "heading_tolerance_deg": None,
        "min_lateral_speed_tolerance_mps": None,
        "max_lateral_speed_tolerance_mps": None,
    }
    contact_material_sets = {}
    contact_material_changes = []
    last_contact_materials = None

    episodes = []
    current_episode = None
    slip_episodes = []
    current_slip = None
    slip_overlap_samples = 0

    for row in samples:
        t = float(row.get("t", 0.0))
        road = row.get("road") or {}
        correction = road.get("correction") or {}
        diagnostic = road.get("diagnostic")
        telemetry = row.get("telemetry") or {}
        state = road.get("state", "unknown")
        state_counts[state] = state_counts.get(state, 0) + 1

        pos = telemetry.get("position") or {}
        xy = (_finite(pos.get("x")), _finite(pos.get("y")))
        if None not in xy:
            if previous_pos is not None:
                step = math.hypot(xy[0] - previous_pos[0], xy[1] - previous_pos[1])
                if step < 50.0:  # vehicle resets are boundaries, not distance travelled
                    distance_m += step
            previous_pos = xy
        z = _finite(pos.get("z"))
        if z is not None:
            if drive["start_z_m"] is None:
                drive["start_z_m"] = z
            drive["end_z_m"] = z
        ground_speed = _finite(telemetry.get("ground_speed_ms"))
        if ground_speed is not None:
            if drive["min_ground_speed_mps"] is None:
                drive["min_ground_speed_mps"] = ground_speed
            drive["min_ground_speed_mps"] = min(drive["min_ground_speed_mps"], ground_speed)
            drive["max_ground_speed_mps"] = max(drive["max_ground_speed_mps"], ground_speed)
        drive["max_throttle"] = max(
            drive["max_throttle"], _finite(telemetry.get("throttle")) or 0.0
        )
        drive["max_brake"] = max(
            drive["max_brake"], _finite(telemetry.get("brake")) or 0.0
        )
        drive["traction_control_samples"] += int(
            bool(telemetry.get("traction_control_active"))
        )
        pitch = _finite(telemetry.get("pitch_rad"))
        if pitch is not None:
            max_abs["vehicle_pitch_deg"] = max(
                max_abs["vehicle_pitch_deg"], abs(math.degrees(pitch))
            )

        active = bool(correction.get("active"))
        settled = bool(correction.get("settled"))
        if settled:
            settled_times.append(round(t, 3))
        if (row.get("audio") or {}).get("settled_triggered"):
            audio_settled_times.append(round(t, 3))

        if diagnostic is None:
            if state == "onRoad":
                if saw_diagnostic:
                    missing_diagnostic += 1
                else:
                    leading_missing_diagnostic += 1
        else:
            saw_diagnostic = True
            edge_id = diagnostic.get("edgeId")
            if edge_id:
                edge_ids.add(str(edge_id))
            was_active = bool(diagnostic.get("activeBefore")) or active or settled
            if was_active:
                eligible += 1
                within_lateral += int(bool(diagnostic.get("withinLateral")))
                within_heading += int(bool(diagnostic.get("withinHeading")))
                within_lateral_speed += int(bool(diagnostic.get("withinLateralSpeed")))
                candidate = bool(diagnostic.get("settledCandidate"))
                settled_candidates += int(candidate)
                candidate_run = candidate_run + 1 if candidate else 0
                longest_candidate_run = max(longest_candidate_run, candidate_run)
                tolerance = _finite(diagnostic.get("targetTolerance"))
                if tolerance is not None:
                    current_min = settled_band["min_lateral_tolerance_m"]
                    current_max = settled_band["max_lateral_tolerance_m"]
                    settled_band["min_lateral_tolerance_m"] = (
                        tolerance if current_min is None else min(current_min, tolerance)
                    )
                    settled_band["max_lateral_tolerance_m"] = (
                        tolerance if current_max is None else max(current_max, tolerance)
                    )
                heading_tolerance = _finite(
                    diagnostic.get("settledHeadingTolerance")
                )
                lateral_speed_tolerance = _finite(
                    diagnostic.get("settledLateralSpeedTolerance")
                )
                if heading_tolerance is not None:
                    settled_band["heading_tolerance_deg"] = heading_tolerance
                if lateral_speed_tolerance is not None:
                    current_min = settled_band["min_lateral_speed_tolerance_mps"]
                    current_max = settled_band["max_lateral_speed_tolerance_mps"]
                    settled_band["min_lateral_speed_tolerance_mps"] = (
                        lateral_speed_tolerance
                        if current_min is None
                        else min(current_min, lateral_speed_tolerance)
                    )
                    settled_band["max_lateral_speed_tolerance_mps"] = (
                        lateral_speed_tolerance
                        if current_max is None
                        else max(current_max, lateral_speed_tolerance)
                    )

            contact_materials = str(diagnostic.get("contactMaterials") or "").strip()
            if contact_materials:
                contact_material_sets[contact_materials] = (
                    contact_material_sets.get(contact_materials, 0) + 1
                )
                if last_contact_materials and contact_materials != last_contact_materials:
                    contact_material_changes.append(
                        {
                            "t": round(t, 3),
                            "from": last_contact_materials,
                            "to": contact_materials,
                            "z_m": _round(z, 2),
                            "road_state": state,
                        }
                    )
                last_contact_materials = contact_materials

            for key, field in (
                ("target_error_m", "targetError"),
                ("lateral_speed_mps", "lateralSpeed"),
                ("heading_error_deg", "headingError"),
                ("correction_bearing_deg", "correctionBearing"),
            ):
                value = _finite(diagnostic.get(field))
                if value is not None:
                    max_abs[key] = max(max_abs[key], abs(value))

        raw_steering = _finite(diagnostic.get("steeringInput")) if diagnostic else None
        if raw_steering is None:
            raw_steering = _finite(telemetry.get("steering_input"))
        applied_steering = _finite(diagnostic.get("steering")) if diagnostic else None
        if applied_steering is not None:
            steering_source_samples["lua_vehicle"] += 1
        else:
            applied_steering = _finite(telemetry.get("steering"))
            if applied_steering is not None:
                steering_source_samples["telemetry"] += 1
        # Older diagnostic files predate the applied channel. Raw input is still
        # preferable to reporting no steering at all when reviewing those sessions.
        steering = applied_steering if applied_steering is not None else raw_steering
        steering = steering or 0.0
        max_abs["steering_input"] = max(
            max_abs["steering_input"], abs(raw_steering or 0.0)
        )
        max_abs["applied_steering"] = max(
            max_abs["applied_steering"], abs(steering)
        )
        if active:
            correction_samples += 1
            steer_sign = _sign(steering, STEERING_DEADZONE)
            if steer_sign and last_steer_sign and steer_sign != last_steer_sign:
                steering_reversals += 1
            if steer_sign:
                last_steer_sign = steer_sign

            bearing_sign = _sign(
                diagnostic.get("correctionBearing") if diagnostic else correction.get("bearing"),
                BEARING_DEADZONE_DEG,
            )
            if bearing_sign and last_bearing_sign and bearing_sign != last_bearing_sign:
                bearing_reversals += 1
            if bearing_sign:
                last_bearing_sign = bearing_sign

            target_sign = _sign(diagnostic.get("targetError") if diagnostic else None, 0.05)
            if target_sign and last_target_sign and target_sign != last_target_sign:
                target_crossings += 1
            if target_sign:
                last_target_sign = target_sign

            if current_episode is None:
                current_episode = {
                    "start_s": round(t, 3),
                    "side": diagnostic.get("targetSide") if diagnostic else None,
                    "phases": [],
                    "max_severity": 0.0,
                }
            phase = str(correction.get("phase", "idle"))
            if not current_episode["phases"] or current_episode["phases"][-1] != phase:
                current_episode["phases"].append(phase)
            current_episode["max_severity"] = round(
                max(current_episode["max_severity"], _finite(correction.get("severity")) or 0.0),
                3,
            )
        elif current_episode is not None:
            current_episode = _close_episode(
                episodes, current_episode, t, "settled" if settled else "cleared_without_settling"
            )
            last_steer_sign = last_bearing_sign = last_target_sign = 0

        ground = _finite(telemetry.get("ground_speed_ms"))
        wheel = _finite(telemetry.get("wheel_speed_ms"))
        raw_slip = (ground - wheel) if ground is not None and wheel is not None else 0.0
        max_abs["raw_slip_mps"] = max(max_abs["raw_slip_mps"], abs(raw_slip))
        threshold = max(SLIP_ABS_THRESHOLD_MS, SLIP_REL_THRESHOLD * max(0.0, ground or 0.0))
        slipping = (ground or 0.0) > SLIP_MIN_GROUND_MS and abs(raw_slip) > threshold
        if slipping:
            if current_slip is None:
                current_slip = {
                    "start_s": round(t, 3),
                    "kind": "wheelspin" if raw_slip < 0 else "lockup",
                    "max_magnitude_mps": 0.0,
                    "samples": 0,
                    "during_correction": False,
                }
            current_slip["samples"] += 1
            current_slip["max_magnitude_mps"] = round(
                max(current_slip["max_magnitude_mps"], abs(raw_slip)), 3
            )
            current_slip["during_correction"] |= active
            slip_overlap_samples += int(active)
        elif current_slip is not None:
            current_slip["end_s"] = round(t, 3)
            current_slip["duration_s"] = round(t - current_slip["start_s"], 3)
            if current_slip["duration_s"] >= SLIP_SUSTAIN_S:
                slip_episodes.append(current_slip)
            current_slip = None

    end_t = float(samples[-1].get("t", 0.0))
    if current_episode is not None:
        _close_episode(episodes, current_episode, end_t, "recording_ended")
    if current_slip is not None:
        current_slip["end_s"] = round(end_t, 3)
        current_slip["duration_s"] = round(end_t - current_slip["start_s"], 3)
        if current_slip["duration_s"] >= SLIP_SUSTAIN_S:
            slip_episodes.append(current_slip)

    repeated = []
    for previous, current in zip(audio_settled_times, audio_settled_times[1:]):
        interval = current - previous
        if interval <= REPEATED_SETTLED_WINDOW_S:
            repeated.append(
                {"first_s": previous, "second_s": current, "interval_s": round(interval, 3)}
            )

    def ratio(count):
        return round(count / eligible, 3) if eligible else None

    drive["start_z_m"] = _round(drive["start_z_m"], 2)
    drive["end_z_m"] = _round(drive["end_z_m"], 2)
    drive["elevation_change_m"] = (
        round(drive["end_z_m"] - drive["start_z_m"], 2)
        if drive["start_z_m"] is not None and drive["end_z_m"] is not None
        else None
    )
    for key in ("min_ground_speed_mps", "max_ground_speed_mps", "max_throttle", "max_brake"):
        drive[key] = _round(drive[key])
    for key, value in settled_band.items():
        settled_band[key] = _round(value)

    flags = []
    if not saw_diagnostic and leading_missing_diagnostic:
        missing_diagnostic += leading_missing_diagnostic
    if repeated:
        flags.append(f"{len(repeated)} settled-tone retrigger(s) within {REPEATED_SETTLED_WINDOW_S:.0f} seconds")
    unresolved = sum(episode["outcome"] != "settled" for episode in episodes)
    if unresolved:
        flags.append(f"{unresolved} correction episode(s) did not settle")
    if steering_reversals >= 4:
        flags.append(f"{steering_reversals} steering reversals occurred during correction")
    if target_crossings >= 2:
        flags.append(f"the vehicle crossed the lane target {target_crossings} times")
    overlap = sum(episode.get("during_correction", False) for episode in slip_episodes)
    if overlap:
        flags.append(f"{overlap} sustained slip episode(s) overlapped lane correction")
    if eligible and settled_candidates / eligible < 0.05:
        flags.append("the full settled condition held in fewer than 5 percent of correction samples")
    if correction_samples and not steering_source_samples["lua_vehicle"]:
        flags.append("vehicle-VM steering input was unavailable during correction")
    if saw_diagnostic and not contact_material_sets:
        flags.append("wheel contact-material data was unavailable")
    if missing_diagnostic:
        flags.append(f"{missing_diagnostic} sample(s) lacked Lua diagnostic fields")
    if not flags:
        flags.append("no automatic anomaly threshold was crossed")

    return {
        "schema": SCHEMA_VERSION,
        "path": str(path) if path else None,
        "samples": len(samples),
        "duration_s": round(duration, 3),
        "distance_travelled_m": round(distance_m, 2),
        "drive": drive,
        "road_state_samples": state_counts,
        "markers": markers,
        "navigation_edges": sorted(edge_ids),
        "diagnostic_startup_samples_skipped": (
            leading_missing_diagnostic if saw_diagnostic else 0
        ),
        "correction": {
            "samples": correction_samples,
            "episodes": episodes,
            "settled_packets_s": settled_times,
            "settled_audio_triggers_s": audio_settled_times,
            "repeated_settled": repeated,
            "eligible_samples": eligible,
            "within_lateral_ratio": ratio(within_lateral),
            "within_heading_ratio": ratio(within_heading),
            "within_lateral_speed_ratio": ratio(within_lateral_speed),
            "all_settled_conditions_ratio": ratio(settled_candidates),
            "longest_settled_candidate_run_samples": longest_candidate_run,
            "steering_reversals": steering_reversals,
            "steering_source_samples": steering_source_samples,
            "correction_bearing_reversals": bearing_reversals,
            "target_crossings": target_crossings,
            "settled_band": settled_band,
        },
        "slip": {
            "episodes": slip_episodes,
            "correction_overlap_samples": slip_overlap_samples,
        },
        "contact_materials": {
            "sample_sets": dict(sorted(contact_material_sets.items())),
            "changes": contact_material_changes,
        },
        "max_absolute": {key: round(value, 3) for key, value in max_abs.items()},
        "assessment_flags": flags,
    }


def analyze_file(path):
    records = []
    with open(path, "r", encoding="utf-8") as source:
        for line in source:
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(row, dict):
                records.append(row)
    return analyze_records(records, path=path)


class RoadDiagnosticRecorder:
    """Thread-safe NDJSON session recorder with reviewable summary files."""

    def __init__(self, directory, clock=None, wall_clock=None):
        self.directory = Path(directory)
        self._clock = clock or time.monotonic
        self._wall_clock = wall_clock or time.time
        self._lock = threading.RLock()
        self._file = None
        self._path = None
        self._started = None
        self._samples = 0
        self._markers = 0

    def start(self, label="hill-climb"):
        with self._lock:
            if self._file is not None:
                raise RuntimeError("a road diagnostic recording is already active")
            self.directory.mkdir(parents=True, exist_ok=True)
            safe = _SAFE_LABEL.sub("-", str(label or "hill-climb")).strip("-")[:48]
            safe = safe or "hill-climb"
            stamp = datetime.now().strftime("%Y%m%d_%H%M%S")
            path = self.directory / f"road_guidance_{stamp}_{safe}.ndjson"
            suffix = 2
            while path.exists():
                path = self.directory / f"road_guidance_{stamp}_{safe}_{suffix}.ndjson"
                suffix += 1
            self._file = open(path, "x", encoding="utf-8", buffering=1)
            self._path = path
            self._started = self._clock()
            self._samples = 0
            self._markers = 0
            self._write_locked(
                {
                    "kind": "meta",
                    "schema": SCHEMA_VERSION,
                    "started_utc": datetime.now(timezone.utc).isoformat(),
                    "label": safe,
                }
            )
            return self.status()

    def _write_locked(self, row):
        self._file.write(json.dumps(row, separators=(",", ":"), allow_nan=False) + "\n")
        self._file.flush()

    def record(self, packet, telemetry, audio_event=None):
        with self._lock:
            if self._file is None:
                return False
            self._write_locked(
                {
                    "kind": "sample",
                    "t": round(self._clock() - self._started, 6),
                    "wall_time": self._wall_clock(),
                    "road": packet,
                    "telemetry": telemetry,
                    "audio": audio_event or {},
                }
            )
            self._samples += 1
            return True

    def mark(self, note):
        note = str(note or "").strip()
        if not note:
            raise ValueError("marker note is required")
        with self._lock:
            if self._file is None:
                raise RuntimeError("no road diagnostic recording is active")
            self._write_locked(
                {
                    "kind": "marker",
                    "t": round(self._clock() - self._started, 6),
                    "wall_time": self._wall_clock(),
                    "note": note[:500],
                }
            )
            self._markers += 1
            return self.status()

    def stop(self):
        with self._lock:
            if self._file is None:
                raise RuntimeError("no road diagnostic recording is active")
            path = self._path
            self._write_locked(
                {
                    "kind": "end",
                    "t": round(self._clock() - self._started, 6),
                    "wall_time": self._wall_clock(),
                }
            )
            self._file.close()
            self._file = None
            self._started = None
        summary = analyze_file(path)
        summary_path = path.with_suffix(".summary.json")
        with open(summary_path, "w", encoding="utf-8") as output:
            json.dump(summary, output, indent=2, allow_nan=False)
            output.write("\n")
        summary["summary_path"] = str(summary_path)
        return summary

    def status(self):
        with self._lock:
            active = self._file is not None
            return {
                "active": active,
                "path": str(self._path) if self._path else None,
                "elapsed_s": round(self._clock() - self._started, 1) if active else 0.0,
                "samples": self._samples,
                "markers": self._markers,
            }

    def review(self, session=None):
        with self._lock:
            if self._file is not None:
                self._file.flush()
            if session:
                name = Path(str(session)).name
                if name != str(session) or not name.endswith(".ndjson"):
                    raise ValueError("session must be a bare .ndjson file name")
                path = self.directory / name
            else:
                candidates = sorted(self.directory.glob("road_guidance_*.ndjson"))
                path = self._path if self._path is not None else (candidates[-1] if candidates else None)
        if path is None or not path.is_file():
            raise RuntimeError("no road diagnostic session is available")
        return analyze_file(path)

    def list_sessions(self, limit=20):
        self.directory.mkdir(parents=True, exist_ok=True)
        paths = sorted(self.directory.glob("road_guidance_*.ndjson"), reverse=True)
        return [
            {
                "session": path.name,
                "bytes": path.stat().st_size,
                "summary": path.with_suffix(".summary.json").is_file(),
            }
            for path in paths[: max(1, min(int(limit), 100))]
        ]
