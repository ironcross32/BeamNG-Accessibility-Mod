"""Durable telemetry capture and accessible reports for the Proving Grounds climb."""

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
CHALLENGE_ID = "hill_climb"
CLIMB_START_Y = -780.0
CLIMB_FINISH_Y = 2265.0
ASPHALT_HALF_WIDTH_M = 7.5
RUMBLE_OUTER_M = 10.0
GRAVEL_OUTER_M = 12.5
MAX_SAMPLE_DT_S = 0.25
RESET_DISTANCE_M = 50.0
MOVING_SPEED_MS = 0.5
SLIP_ABS_THRESHOLD_MS = 1.5
SLIP_REL_THRESHOLD = 0.25
SLIP_MIN_GROUND_MS = 2.0
_SAFE_ID = re.compile(r"[^A-Za-z0-9_.-]+")


def _finite(value, default=None):
    try:
        value = float(value)
    except (TypeError, ValueError):
        return default
    return value if math.isfinite(value) else default


def _round(value, digits=3):
    value = _finite(value)
    return None if value is None else round(value, digits)


def _format_time(seconds):
    seconds = max(0.0, _finite(seconds, 0.0))
    minutes, remainder = divmod(seconds, 60.0)
    if minutes:
        return f"{int(minutes)}:{remainder:06.3f}"
    return f"{remainder:.3f} seconds"


def _format_elapsed_words(seconds):
    """Format a split as a sentence-friendly duration for speech output."""
    seconds = max(0.0, _finite(seconds, 0.0))
    minutes, remainder = divmod(seconds, 60.0)
    if not minutes:
        return f"{remainder:.3f} seconds"
    minute_word = "minute" if int(minutes) == 1 else "minutes"
    return f"{int(minutes)} {minute_word}, {remainder:.3f} seconds"


def _format_local_datetime(value):
    """Turn a stored ISO UTC timestamp into readable local wall-clock time."""
    try:
        text = str(value).strip()
        parsed = datetime.fromisoformat(text[:-1] + "+00:00" if text.endswith("Z") else text)
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=timezone.utc)
        local = parsed.astimezone()
    except (TypeError, ValueError, OverflowError):
        return "unknown time"
    hour = local.strftime("%I").lstrip("0") or "12"
    zone = local.tzname()
    suffix = f" {zone}" if zone else ""
    return (
        f"{local.strftime('%B')} {local.day}, {local.year} at "
        f"{hour}:{local.strftime('%M:%S %p')}{suffix}"
    )


def _format_speed(speed_ms, units):
    speed_ms = _finite(speed_ms, 0.0)
    if units == "metric":
        return f"{speed_ms * 3.6:.1f} km/h"
    return f"{speed_ms * 2.2369362920544:.1f} mph"


def _format_distance(metres, units):
    metres = _finite(metres, 0.0)
    if units == "metric":
        return f"{metres:.1f} metres"
    return f"{metres * 3.280839895:.1f} feet"


def _surface_for_position(x, y):
    x = abs(_finite(x, 0.0))
    y = _finite(y, CLIMB_START_Y)
    if y < CLIMB_START_Y or y > CLIMB_FINISH_Y:
        return "outside_course"
    if x <= ASPHALT_HALF_WIDTH_M:
        return "asphalt"
    if x <= RUMBLE_OUTER_M:
        return "rumble"
    if x <= GRAVEL_OUTER_M:
        return "gravel"
    return "outside_corridor"


def _contact_flags(packet):
    diagnostic = (packet or {}).get("diagnostic") or {}
    names = str(diagnostic.get("contactMaterials") or "").upper()
    return {
        "rumble": "RUMBLE_STRIP" in names,
        "gravel": "GRAVEL" in names,
        "materials": names,
    }


def _slip(telemetry):
    ground = _finite(telemetry.get("ground_speed_ms"), 0.0)
    wheel = _finite(telemetry.get("wheel_speed_ms"), 0.0)
    raw = ground - wheel
    threshold = max(SLIP_ABS_THRESHOLD_MS, SLIP_REL_THRESHOLD * max(0.0, ground))
    active = ground > SLIP_MIN_GROUND_MS and abs(raw) > threshold
    return {
        "active": active,
        "kind": "lockup" if active and raw > 0 else ("wheelspin" if active else "none"),
        "magnitude_mps": round(abs(raw), 4),
        "raw_mps": round(raw, 4),
    }


def analyze_attempt(samples, checkpoints, event):
    """Build time-weighted statistics from raw challenge samples."""
    totals = {
        "sampled_s": 0.0,
        "moving_s": 0.0,
        "stopped_s": 0.0,
        "throttle_s": 0.0,
        "brake_s": 0.0,
        "traction_control_s": 0.0,
        "asphalt_s": 0.0,
        "off_asphalt_s": 0.0,
        "rumble_contact_s": 0.0,
        "gravel_contact_s": 0.0,
        "nav_on_road_s": 0.0,
        "nav_off_road_s": 0.0,
        "wheelspin_s": 0.0,
        "lockup_s": 0.0,
    }
    moving_speed_integral = 0.0
    distance_m = 0.0
    max_speed = finish_speed = max_slip = 0.0
    start_z = end_z = None
    previous = None
    gaps = []
    departures = {"left": 0, "right": 0}
    was_asphalt = True
    slip_episodes = {"wheelspin": 0, "lockup": 0}
    previous_slip = "none"

    for sample in samples:
        telemetry = sample.get("telemetry") or {}
        position = telemetry.get("position") or {}
        speed = max(0.0, _finite(telemetry.get("ground_speed_ms"), 0.0))
        max_speed = max(max_speed, speed)
        finish_speed = speed
        z = _finite(position.get("z"))
        if z is not None:
            if start_z is None:
                start_z = z
            end_z = z

        if previous is not None:
            raw_dt = max(0.0, sample["t"] - previous["t"])
            if raw_dt > MAX_SAMPLE_DT_S:
                gaps.append(round(raw_dt, 3))
                dt = 0.0
            else:
                dt = raw_dt
            totals["sampled_s"] += dt
            if speed > MOVING_SPEED_MS:
                totals["moving_s"] += dt
                moving_speed_integral += speed * dt
            else:
                totals["stopped_s"] += dt
            if _finite(telemetry.get("throttle"), 0.0) > 0.1:
                totals["throttle_s"] += dt
            if _finite(telemetry.get("brake"), 0.0) > 0.1:
                totals["brake_s"] += dt
            if telemetry.get("traction_control_active"):
                totals["traction_control_s"] += dt

            surface = sample.get("surface") or "outside_course"
            on_asphalt = surface == "asphalt"
            totals["asphalt_s" if on_asphalt else "off_asphalt_s"] += dt
            if was_asphalt and not on_asphalt:
                side = "left" if _finite(position.get("x"), 0.0) < 0 else "right"
                departures[side] += 1
            was_asphalt = on_asphalt

            contacts = sample.get("contacts") or {}
            if contacts.get("rumble"):
                totals["rumble_contact_s"] += dt
            if contacts.get("gravel"):
                totals["gravel_contact_s"] += dt
            state = (sample.get("road") or {}).get("state")
            if state == "onRoad":
                totals["nav_on_road_s"] += dt
            elif state == "offRoad":
                totals["nav_off_road_s"] += dt

            slip = sample.get("slip") or {}
            kind = slip.get("kind", "none") if slip.get("active") else "none"
            if kind in ("wheelspin", "lockup"):
                totals[kind + "_s"] += dt
                max_slip = max(max_slip, _finite(slip.get("magnitude_mps"), 0.0))
                if kind != previous_slip:
                    slip_episodes[kind] += 1
            previous_slip = kind

            previous_pos = (previous.get("telemetry") or {}).get("position") or {}
            x0, y0 = _finite(previous_pos.get("x")), _finite(previous_pos.get("y"))
            x1, y1 = _finite(position.get("x")), _finite(position.get("y"))
            if None not in (x0, y0, x1, y1):
                step = math.hypot(x1 - x0, y1 - y0)
                if step < RESET_DISTANCE_M:
                    distance_m += step
        previous = sample

    sampled = totals["sampled_s"]
    moving = totals["moving_s"]
    official = _finite(event.get("official_time_s"))
    penalty = max(0.0, _finite(event.get("penalty_s"), 0.0))
    raw_time = _finite(event.get("raw_time_s"))
    if raw_time is None and official is not None:
        raw_time = max(0.0, official - penalty)
    if raw_time is None:
        raw_time = max(0.0, _finite(event.get("race_time_s"), sampled))
    if official is None:
        official = raw_time + penalty

    rounded_totals = {key: round(value, 3) for key, value in totals.items()}
    sample_span = (
        max(0.0, samples[-1]["t"] - samples[0]["t"]) if len(samples) > 1 else 0.0
    )
    max_lateral = max(
        (abs(_finite(((row.get("telemetry") or {}).get("position") or {}).get("x"), 0.0)) for row in samples),
        default=0.0,
    )
    return {
        "official_time_s": round(official, 3),
        "raw_time_s": round(raw_time, 3),
        "penalty_s": round(penalty, 3),
        "samples": len(samples),
        "sample_rate_hz": round((len(samples) - 1) / sample_span, 2) if sample_span else 0.0,
        "packet_gaps": {"count": len(gaps), "durations_s": gaps},
        "distance_m": round(distance_m, 2),
        "elevation_gain_m": round(max(0.0, (end_z or 0.0) - (start_z or 0.0)), 2),
        "average_speed_mps": round(distance_m / raw_time, 3) if raw_time else 0.0,
        "moving_average_speed_mps": round(moving_speed_integral / moving, 3) if moving else 0.0,
        "max_speed_mps": round(max_speed, 3),
        "finish_speed_mps": round(finish_speed, 3),
        "durations": rounded_totals,
        "departures": {**departures, "total": departures["left"] + departures["right"]},
        "max_lateral_excursion_m": round(max_lateral, 2),
        "slip": {
            "wheelspin_episodes": slip_episodes["wheelspin"],
            "lockup_episodes": slip_episodes["lockup"],
            "max_magnitude_mps": round(max_slip, 3),
        },
        "checkpoints": checkpoints,
        "recoveries": int(_finite(event.get("recovery_count"), 0.0)),
        "official_confirmed": bool(event.get("official_confirmed", False)),
    }


class HillClimbChallengeRecorder:
    """Thread-safe lifecycle recorder fed by the Lua bridge and road listener."""

    def __init__(self, directory, clock=time.monotonic, now_utc=None, finalize_callback=None):
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)
        self.clock = clock
        self.now_utc = now_utc or (lambda: datetime.now(timezone.utc))
        self.finalize_callback = finalize_callback
        self._lock = threading.RLock()
        self._attempt = None
        self._timer = None

    def set_finalize_callback(self, callback):
        self.finalize_callback = callback

    def is_capturing(self):
        with self._lock:
            return bool(self._attempt and self._attempt.get("capturing"))

    def _attempt_name(self, attempt_id):
        safe = _SAFE_ID.sub("-", str(attempt_id or "attempt")).strip("-.") or "attempt"
        stamp = self.now_utc().strftime("%Y%m%dT%H%M%S.%fZ")
        return f"{stamp}-{safe}"

    def _write_row(self, attempt, row, flush=False):
        attempt["file"].write(json.dumps(row, separators=(",", ":"), ensure_ascii=False) + "\n")
        attempt["writes"] += 1
        if flush or attempt["writes"] % 20 == 0:
            attempt["file"].flush()

    def _start_locked(self, event):
        name = self._attempt_name(event.get("attempt_id"))
        raw_path = self.directory / f"{name}.ndjson"
        handle = raw_path.open("w", encoding="utf-8", newline="\n")
        started_mono = self.clock()
        attempt = {
            "attempt_id": str(event.get("attempt_id") or name),
            "name": name,
            "raw_path": raw_path,
            "file": handle,
            "writes": 0,
            "started_mono": started_mono,
            "started_utc": self.now_utc().isoformat(),
            "samples": [],
            "checkpoints": [],
            "capturing": True,
            "pending_complete": False,
            "race_complete_event": {},
        }
        self._write_row(attempt, {
            "kind": "meta",
            "schema": SCHEMA_VERSION,
            "challenge_id": CHALLENGE_ID,
            "attempt_id": attempt["attempt_id"],
            "started_utc": attempt["started_utc"],
            "event": event,
        }, flush=True)
        self._attempt = attempt

    def _discard_locked(self):
        """Drop a superseded pre-start trace without adding it to report history."""
        attempt = self._attempt
        if not attempt:
            return
        if self._timer:
            self._timer.cancel()
            self._timer = None
        attempt["capturing"] = False
        try:
            attempt["file"].close()
        finally:
            try:
                attempt["raw_path"].unlink()
            except FileNotFoundError:
                pass
            self._attempt = None

    def record(self, packet, telemetry):
        with self._lock:
            attempt = self._attempt
            if not attempt or not attempt.get("capturing"):
                return False
            position = (telemetry or {}).get("position") or {}
            now = self.clock()
            row = {
                "kind": "sample",
                "t": round(max(0.0, now - attempt["started_mono"]), 4),
                "utc": self.now_utc().isoformat(),
                "road": packet or {},
                "telemetry": telemetry or {},
                "course_progress_m": round(max(0.0, min(
                    CLIMB_FINISH_Y - CLIMB_START_Y,
                    _finite(position.get("y"), CLIMB_START_Y) - CLIMB_START_Y,
                )), 3),
                "lateral_offset_m": _round(position.get("x"), 3),
                "surface": _surface_for_position(position.get("x"), position.get("y")),
                "contacts": _contact_flags(packet),
                "slip": _slip(telemetry or {}),
            }
            attempt["samples"].append(row)
            self._write_row(attempt, row)
            return True

    def _prior_best_locked(self, excluding_name):
        best = None
        for path in self.directory.glob("*.summary.json"):
            if path.name.startswith(excluding_name):
                continue
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
                if data.get("status") != "completed":
                    continue
                value = _finite((data.get("statistics") or {}).get("official_time_s"))
                if value is not None and (best is None or value < best):
                    best = value
            except (OSError, ValueError, TypeError):
                continue
        return best

    def _finish_locked(self, status, event, reason=""):
        attempt = self._attempt
        if not attempt:
            return None
        if self._timer:
            self._timer.cancel()
            self._timer = None
        attempt["capturing"] = False
        merged = dict(attempt.get("race_complete_event") or {})
        merged.update(event or {})
        statistics = analyze_attempt(attempt["samples"], attempt["checkpoints"], merged)
        prior_best = self._prior_best_locked(attempt["name"])
        official = statistics["official_time_s"]
        explicit_best = merged.get("new_best")
        new_best = bool(explicit_best) if explicit_best is not None else (
            status == "completed" and (prior_best is None or official < prior_best)
        )
        summary = {
            "schema": SCHEMA_VERSION,
            "challenge_id": CHALLENGE_ID,
            "attempt_id": attempt["attempt_id"],
            "status": status,
            "reason": reason,
            "started_utc": attempt["started_utc"],
            "ended_utc": self.now_utc().isoformat(),
            "raw_path": str(attempt["raw_path"]),
            "statistics": statistics,
            "personal_best": {
                "new_best": new_best,
                "previous_best_s": _round(prior_best),
                "delta_s": _round(official - prior_best) if prior_best is not None else None,
            },
        }
        summary_path = self.directory / f"{attempt['name']}.summary.json"
        summary["summary_path"] = str(summary_path)
        self._write_row(attempt, {
            "kind": "end",
            "status": status,
            "reason": reason,
            "statistics": statistics,
        }, flush=True)
        attempt["file"].close()
        temp_path = summary_path.with_suffix(summary_path.suffix + ".tmp")
        temp_path.write_text(json.dumps(summary, indent=2), encoding="utf-8")
        os.replace(temp_path, summary_path)
        self._attempt = None
        return summary

    def _notify(self, summary, auto_open):
        if summary and self.finalize_callback:
            try:
                self.finalize_callback(summary, auto_open)
            except Exception:
                pass

    def _timeout_complete(self):
        summary = None
        with self._lock:
            if self._attempt and self._attempt.get("pending_complete"):
                event = dict(self._attempt.get("race_complete_event") or {})
                event["official_confirmed"] = False
                summary = self._finish_locked("completed", event, "aggregation_timeout")
        self._notify(summary, True)

    def handle_event(self, event, telemetry=None):
        """Apply one Lua lifecycle event and return the requested capture state."""
        if not isinstance(event, dict) or event.get("challenge_id") != CHALLENGE_ID:
            return None
        kind = str(event.get("event") or "")
        notifications = []
        capture = None
        with self._lock:
            if kind == "started":
                if self._attempt:
                    if event.get("replace_active"):
                        self._discard_locked()
                    else:
                        notifications.append((self._finish_locked("aborted", {}, "restarted"), False))
                self._start_locked(event)
                capture = True
            elif kind == "checkpoint" and self._attempt:
                snapshot = telemetry or {}
                split = {
                    "index": int(_finite(event.get("checkpoint_index"), len(self._attempt["checkpoints"]) + 1)),
                    "name": str(event.get("checkpoint_name") or "Checkpoint"),
                    "time_s": _round(event.get("race_time_s")),
                    "speed_mps": _round(snapshot.get("ground_speed_ms")),
                }
                self._attempt["checkpoints"].append(split)
                self._write_row(self._attempt, {"kind": "checkpoint", **split}, flush=True)
            elif kind == "race_complete" and self._attempt:
                self._attempt["capturing"] = False
                self._attempt["pending_complete"] = True
                self._attempt["race_complete_event"] = dict(event)
                self._write_row(self._attempt, {"kind": "race_complete", "event": event}, flush=True)
                capture = False
                self._timer = threading.Timer(3.0, self._timeout_complete)
                self._timer.daemon = True
                self._timer.start()
            elif kind == "attempt_aggregated":
                if not self._attempt:
                    self._start_locked(event)
                    self._attempt["capturing"] = False
                event = dict(event)
                event["official_confirmed"] = True
                notifications.append((self._finish_locked("completed", event), True))
                capture = False
            elif kind in ("aborted", "mission_stopped") and self._attempt:
                notifications.append((self._finish_locked("aborted", event, kind), False))
                capture = False
        for summary, auto_open in notifications:
            self._notify(summary, auto_open)
        return {"capture": capture} if capture is not None else {}

    def shutdown(self):
        summary = None
        with self._lock:
            if self._attempt:
                summary = self._finish_locked("aborted", {}, "beamtel_shutdown")
        self._notify(summary, False)

    def list_reports(self, limit=100):
        reports = []
        for path in sorted(self.directory.glob("*.summary.json"), reverse=True):
            try:
                report = json.loads(path.read_text(encoding="utf-8"))
                report["summary_path"] = str(path)
                reports.append(report)
            except (OSError, ValueError, TypeError):
                continue
            if len(reports) >= limit:
                break
        return reports

    @staticmethod
    def history_line(summary, units="imperial"):
        stats = summary.get("statistics") or {}
        stamp = _format_local_datetime(summary.get("started_utc"))
        status = summary.get("status", "unknown")
        time_text = _format_time(stats.get("official_time_s")) if status == "completed" else status
        best = ", new personal best" if (summary.get("personal_best") or {}).get("new_best") else ""
        return f"{stamp}, {time_text}{best}"

    @staticmethod
    def report_lines(summary, units="imperial"):
        stats = summary.get("statistics") or {}
        durations = stats.get("durations") or {}
        best = summary.get("personal_best") or {}
        lines = [
            f"Status, {summary.get('status', 'unknown')}",
            f"Started, {_format_local_datetime(summary.get('started_utc'))}",
            f"Finished, {_format_local_datetime(summary.get('ended_utc'))}",
            f"Official time, {_format_time(stats.get('official_time_s'))}",
            f"Driving time, {_format_time(stats.get('raw_time_s'))}",
            f"Penalty time, {_format_time(stats.get('penalty_s'))}",
        ]
        if best.get("new_best"):
            lines.append("Personal best, new best time")
        elif best.get("delta_s") is not None:
            delta = _finite(best.get("delta_s"), 0.0)
            lines.append(f"Personal best comparison, {abs(delta):.3f} seconds {'slower' if delta >= 0 else 'faster'}")
        lines.extend([
            f"Distance travelled, {_format_distance(stats.get('distance_m'), units)}",
            f"Elevation gained, {_format_distance(stats.get('elevation_gain_m'), units)}",
            f"Average speed, {_format_speed(stats.get('average_speed_mps'), units)}",
            f"Moving average speed, {_format_speed(stats.get('moving_average_speed_mps'), units)}",
            f"Maximum speed, {_format_speed(stats.get('max_speed_mps'), units)}",
            f"Finish speed, {_format_speed(stats.get('finish_speed_mps'), units)}",
            f"Stopped time, {_format_time(durations.get('stopped_s'))}",
            f"Time on asphalt, {_format_time(durations.get('asphalt_s'))}",
            f"Time off asphalt, {_format_time(durations.get('off_asphalt_s'))}",
            f"Rumble strip contact, {_format_time(durations.get('rumble_contact_s'))}",
            f"Gravel contact, {_format_time(durations.get('gravel_contact_s'))}",
            f"Navigation on-road time, {_format_time(durations.get('nav_on_road_s'))}",
            f"Navigation off-road time, {_format_time(durations.get('nav_off_road_s'))}",
            f"Departures, {stats.get('departures', {}).get('total', 0)} total, {stats.get('departures', {}).get('left', 0)} left, {stats.get('departures', {}).get('right', 0)} right",
            f"Maximum lateral excursion, {_format_distance(stats.get('max_lateral_excursion_m'), units)}",
            f"Wheelspin, {_format_time(durations.get('wheelspin_s'))}, {stats.get('slip', {}).get('wheelspin_episodes', 0)} episodes",
            f"Wheel lockup, {_format_time(durations.get('lockup_s'))}, {stats.get('slip', {}).get('lockup_episodes', 0)} episodes",
            f"Maximum wheel slip, {_format_speed(stats.get('slip', {}).get('max_magnitude_mps'), units)} difference",
            f"Traction control active, {_format_time(durations.get('traction_control_s'))}",
            f"Throttle applied, {_format_time(durations.get('throttle_s'))}",
            f"Brake applied, {_format_time(durations.get('brake_s'))}",
            f"Recoveries, {stats.get('recoveries', 0)}",
        ])
        for checkpoint in stats.get("checkpoints") or []:
            lines.append(
                f"Checkpoint {checkpoint.get('index')} reached at "
                f"{_format_elapsed_words(checkpoint.get('time_s'))} into the run, "
                f"traveling {_format_speed(checkpoint.get('speed_mps'), units)}"
            )
        return lines


def completion_speech(summary):
    stats = summary.get("statistics") or {}
    phrase = f"Hill climb complete in {_format_time(stats.get('official_time_s'))}"
    penalty = _finite(stats.get("penalty_s"), 0.0)
    if penalty > 0:
        phrase += f", including {penalty:g} seconds of penalties"
    if (summary.get("personal_best") or {}).get("new_best"):
        phrase += ". New personal best"
    return phrase
