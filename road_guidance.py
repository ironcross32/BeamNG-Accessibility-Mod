"""Protocol and wording helpers for BeamTel road awareness.

The UDP listener lives in :mod:`beamtel`; this module deliberately has no BeamTel,
audio, or speech imports so its compatibility and phrasing rules can be exercised
without starting the application.
"""

from __future__ import annotations

import json
import math
import threading
import time
from copy import deepcopy


R2_PREFIX = "R2|"
R2_STALE_SECONDS = 1.0
_STATES = {"dormant", "offRoad", "onRoad"}
_PHASES = {"approach", "near"}


def _finite_number(value, field):
    if isinstance(value, bool):
        raise ValueError(f"{field} must be a number")
    try:
        number = float(value)
    except (TypeError, ValueError) as exc:
        raise ValueError(f"{field} must be a number") from exc
    if not math.isfinite(number):
        raise ValueError(f"{field} must be finite")
    return number


def _bearing(value, field):
    value = _finite_number(value, field)
    return ((value + 180.0) % 360.0) - 180.0


def parse_r2_packet(text):
    """Parse and validate one R2 datagram, raising ``ValueError`` on bad input."""
    if not isinstance(text, str) or not text.startswith(R2_PREFIX):
        raise ValueError("not an R2 packet")
    try:
        packet = json.loads(text[len(R2_PREFIX) :])
    except (TypeError, json.JSONDecodeError) as exc:
        raise ValueError("invalid R2 JSON") from exc
    if not isinstance(packet, dict):
        raise ValueError("R2 payload must be an object")

    state = packet.get("state")
    if state not in _STATES:
        raise ValueError("invalid road state")
    one_way = packet.get("oneWay", False)
    if not isinstance(one_way, bool):
        raise ValueError("oneWay must be boolean")

    directions = packet.get("roadDirections", [])
    if not isinstance(directions, list) or len(directions) > 8:
        raise ValueError("roadDirections must be a short array")
    directions = [_bearing(v, "roadDirections") for v in directions]

    off_road = packet.get("offRoad")
    if off_road is not None:
        if not isinstance(off_road, dict):
            raise ValueError("offRoad must be an object or null")
        off_road = {
            "bearing": _bearing(off_road.get("bearing"), "offRoad.bearing"),
            "distance": max(0.0, _finite_number(off_road.get("distance"), "offRoad.distance")),
        }

    correction = packet.get("correction")
    if correction is not None:
        if not isinstance(correction, dict) or not isinstance(correction.get("active"), bool):
            raise ValueError("correction must contain a boolean active field")
        correction = {
            "active": correction["active"],
            "bearing": _bearing(correction.get("bearing", 0.0), "correction.bearing"),
            "severity": min(
                1.0,
                max(0.0, _finite_number(correction.get("severity", 0.0), "correction.severity")),
            ),
        }

    junction = packet.get("junction")
    if junction is not None:
        if not isinstance(junction, dict):
            raise ValueError("junction must be an object or null")
        junction_id = junction.get("id")
        if not isinstance(junction_id, str) or not junction_id.strip() or len(junction_id) > 160:
            raise ValueError("junction.id must be a non-empty string")
        phase = junction.get("phase")
        if phase not in _PHASES:
            raise ValueError("invalid junction phase")
        entered = junction.get("entered", False)
        if not isinstance(entered, bool):
            raise ValueError("junction.entered must be boolean")
        exits = junction.get("exits", [])
        if not isinstance(exits, list) or len(exits) > 16:
            raise ValueError("junction.exits must be a short array")
        kind = junction.get("kind", "intersection")
        if not isinstance(kind, str) or not kind.strip() or len(kind) > 40:
            raise ValueError("junction.kind must be a short string")
        junction = {
            "id": junction_id.strip(),
            "phase": phase,
            "entered": entered,
            "kind": kind.strip(),
            "distance": max(
                0.0, _finite_number(junction.get("distance", 0.0), "junction.distance")
            ),
            "exits": [_bearing(v, "junction.exits") for v in exits],
        }

    return {
        "state": state,
        "oneWay": one_way,
        "roadDirections": directions,
        "offRoad": off_road,
        "correction": correction,
        "junction": junction,
        # Optional extension used to re-arm orientation after a vehicle/world reload
        # even when the wire state remains onRoad throughout.
        "orientation": packet.get("orientation") is True,
    }


def direction_label(bearing, u_turn=False):
    """Return the plan's travel-relative direction band for a signed bearing."""
    bearing = _bearing(bearing, "bearing")
    magnitude = abs(bearing)
    if magnitude <= 15.0:
        return "straight"
    side = "left" if bearing > 0 else "right"
    if magnitude <= 45.0:
        return f"slight {side}"
    if magnitude <= 120.0:
        return side
    if magnitude <= 150.0:
        return f"sharp {side}"
    return "U-turn" if u_turn else "behind"


def _joined(items, conjunction="and"):
    items = list(items)
    if not items:
        return ""
    if len(items) == 1:
        return items[0]
    if len(items) == 2:
        return f"{items[0]} {conjunction} {items[1]}"
    return f"{', '.join(items[:-1])}, {conjunction} {items[-1]}"


def direction_list(bearings, conjunction="and"):
    labels = []
    for bearing in sorted(bearings, reverse=True):  # driver's left through right
        label = direction_label(bearing, u_turn=True)
        if label != "U-turn" and label not in labels:
            labels.append(label)
    return _joined(labels, conjunction)


def road_direction_list(bearings, conjunction="and"):
    labels = []
    for bearing in sorted(bearings, reverse=True):
        label = direction_label(bearing)
        if label not in labels:
            labels.append(label)
    return _joined(labels, conjunction)


def format_road_distance(metres, units):
    value = max(0.0, float(metres))
    if str(units).lower().startswith("imp"):
        return f"{value * 3.28084:.0f} feet"
    return f"{value:.0f} meters"


def junction_phrase(junction, units):
    kind = junction.get("kind", "intersection")
    distance = format_road_distance(junction.get("distance", 0.0), units)
    exits = direction_list(junction.get("exits", []), conjunction="or" if kind in {"fork", "tJunction"} else "and")
    if kind == "deadEnd":
        return f"Road ends in {distance}."
    names = {
        "fork": "Road forks",
        "tJunction": "T-junction",
        "crossroads": "Crossroads",
        "complex": "Complex intersection",
        "intersection": "Intersection",
    }
    lead = names.get(kind, "Intersection")
    if exits:
        return f"{lead} in {distance}: {exits}."
    return f"{lead} in {distance}."


class RoadGuidanceFeed:
    """Thread-safe R2/legacy feed state and once-per-junction event tracking."""

    def __init__(self, clock=None):
        self._clock = clock or time.monotonic
        self._lock = threading.Lock()
        self.reset()

    def reset(self):
        with getattr(self, "_lock", threading.Lock()):
            self.mode = "unavailable"
            self.packet = None
            self.last_r2_time = None
            self.last_legacy_time = None
            self.legacy_state = None
            self._last_state = None
            self._orientation_armed = True
            self._junction_id = None
            self._junction_missing = 0
            self._announced_phases = set()
            self.last_known_junction = None

    def accept_r2(self, packet, now=None):
        now = self._clock() if now is None else float(now)
        with self._lock:
            previous_state = self._last_state
            self.mode = "r2"
            self.last_r2_time = now
            self.packet = deepcopy(packet)
            self._last_state = packet["state"]

            orientation = False
            if packet["state"] == "onRoad":
                orientation = bool(packet.get("orientation")) or (
                    self._orientation_armed and previous_state != "onRoad"
                )
                if orientation:
                    self._orientation_armed = False
            elif packet["state"] == "offRoad":
                self._orientation_armed = True

            junction_event = None
            junction = packet.get("junction")
            if junction is None:
                self._junction_missing += 1
                if self._junction_missing >= 3:
                    self._junction_id = None
                    self._announced_phases.clear()
                    self.last_known_junction = None
            else:
                self._junction_missing = 0
                if junction["id"] != self._junction_id:
                    self._junction_id = junction["id"]
                    self._announced_phases.clear()
                self.last_known_junction = deepcopy(junction)
                event_phase = "entered" if junction.get("entered") else junction["phase"]
                phase_key = (junction["id"], event_phase)
                if phase_key not in self._announced_phases:
                    self._announced_phases.add(phase_key)
                    junction_event = deepcopy(junction)
                    junction_event["phase"] = event_phase

            return {"orientation": orientation, "junction": junction_event}

    def r2_recent(self, now=None):
        now = self._clock() if now is None else float(now)
        with self._lock:
            return self.last_r2_time is not None and now - self.last_r2_time <= R2_STALE_SECONDS

    def accept_legacy(self, state, bearing=0.0, distance=0.0, directions=None, now=None):
        now = self._clock() if now is None else float(now)
        with self._lock:
            self.last_legacy_time = now
            self.legacy_state = {
                "state": state,
                "bearing": float(bearing),
                "distance": max(0.0, float(distance)),
                "directions": list(directions or []),
            }
            if self.last_r2_time is None or now - self.last_r2_time > R2_STALE_SECONDS:
                self.mode = "legacy"
                return True
            return False

    def check_timeout(self, now=None):
        """Update stale/fallback state. Return True only on a new R2 timeout."""
        now = self._clock() if now is None else float(now)
        with self._lock:
            if self.last_r2_time is None or now - self.last_r2_time <= R2_STALE_SECONDS:
                return False
            newly_stale = self.mode == "r2"
            legacy_recent = (
                self.last_legacy_time is not None
                and now - self.last_legacy_time <= R2_STALE_SECONDS
            )
            self.mode = "legacy" if legacy_recent else "stale"
            if newly_stale:
                self.packet = None
                self._last_state = None
                self._orientation_armed = True
                self.last_known_junction = None
            return newly_stale

    def snapshot(self):
        with self._lock:
            return {
                "mode": self.mode,
                "packet": deepcopy(self.packet),
                "legacy": deepcopy(self.legacy_state),
                "junction": deepcopy(self.last_known_junction),
            }

    def status_phrase(self, enabled, units):
        if not enabled:
            return "Road guidance is off."
        snap = self.snapshot()
        mode = snap["mode"]
        if mode == "unavailable":
            return "Road guidance feed is unavailable."
        if mode == "stale":
            return "Road guidance feed is stale."
        if mode == "legacy":
            legacy = snap["legacy"]
            if not legacy:
                return "Legacy road detector only; enhanced guidance is unavailable."
            if legacy["state"] == "DORMANT":
                return "No roads detected on this map. Legacy road detector only."
            if legacy["state"] == "OFF_ROAD":
                side = direction_label(legacy["bearing"])
                dist = format_road_distance(legacy["distance"], units)
                return f"Off road. Road {side}, {dist} away. Legacy guidance only."
            return "On road. Legacy road detector only; enhanced guidance is unavailable."

        packet = snap["packet"] or {}
        state = packet.get("state")
        if state == "dormant":
            return "No roads detected on this map."
        if state == "offRoad":
            off_road = packet.get("offRoad")
            if off_road is None:
                return "Off road. No vertically compatible road found within search range."
            side = direction_label(off_road.get("bearing", 0.0))
            dist = format_road_distance(off_road.get("distance", 0.0), units)
            return f"Off road. Road {side}, {dist} away."

        parts = ["On road."]
        directions = road_direction_list(
            packet.get("roadDirections", []), conjunction="or"
        )
        if packet.get("oneWay"):
            parts.append("One-way road.")
        if directions:
            parts.append(f"Legal direction{'s' if not packet.get('oneWay') else ''}: {directions}.")
        correction = packet.get("correction")
        if correction and correction.get("active"):
            parts.append(f"Correction needed: {direction_label(correction.get('bearing', 0.0))}.")
        else:
            parts.append("No correction needed.")
        junction = snap["junction"]
        if junction:
            parts.append("Next, " + junction_phrase(junction, units))
        else:
            parts.append("No intersection reported ahead.")
        return " ".join(parts)
