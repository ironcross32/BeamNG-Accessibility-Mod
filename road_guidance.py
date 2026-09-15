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
_CORRECTION_PHASES = {"idle", "correct", "unwind"}
_SIGNAL_STATES = {
    "stop", "red", "yellow", "green", "redYellow", "flashingRed",
    "flashingYellow", "flashingGreen", "off", "unknown",
}


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


def _short_array(value, field, max_items):
    """Accept BeamNG's empty-table JSON spelling while keeping arrays strict.

    BeamNG's Lua ``jsonEncode`` cannot distinguish an empty array from an empty
    object, so protocol fields sent as ``{}`` on the wire mean ``[]``.  A
    non-empty object is still malformed and must not be silently accepted.
    """
    if value == {}:
        value = []
    if not isinstance(value, list) or len(value) > max_items:
        raise ValueError(f"{field} must be a short array")
    return value


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

    directions = _short_array(packet.get("roadDirections", []), "roadDirections", 8)
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
        correction_data = correction
        correction_phase = correction.get(
            "phase", "correct" if correction["active"] else "idle"
        )
        if correction_phase not in _CORRECTION_PHASES:
            raise ValueError("invalid correction phase")
        settled = correction.get("settled", False)
        if not isinstance(settled, bool):
            raise ValueError("correction.settled must be boolean")
        correction = {
            "active": correction["active"],
            "bearing": _bearing(correction.get("bearing", 0.0), "correction.bearing"),
            "severity": min(
                1.0,
                max(0.0, _finite_number(correction.get("severity", 0.0), "correction.severity")),
            ),
            "phase": correction_phase,
            "settled": settled,
        }
        for field in ("lateralRatio", "headingError", "timeToEdge"):
            value = correction_data.get(field)
            if value is not None:
                correction[field] = _finite_number(value, f"correction.{field}")
        if "lateralRatio" in correction:
            correction["lateralRatio"] = max(0.0, correction["lateralRatio"])
        if "headingError" in correction:
            correction["headingError"] = _bearing(
                correction["headingError"], "correction.headingError"
            )
        if "timeToEdge" in correction:
            correction["timeToEdge"] = max(0.0, correction["timeToEdge"])

    diagnostic = packet.get("diagnostic")
    if diagnostic is not None:
        if not isinstance(diagnostic, dict):
            raise ValueError("diagnostic must be an object or null")
        parsed_diagnostic = {}
        for field in (
            "edgeT",
            "roadRadius",
            "signedLateral",
            "lateralDistance",
            "lateralRatio",
            "lateralSpeed",
            "predictedLateral",
            "predictedRatio",
            "predictionSeconds",
            "targetSide",
            "targetOffset",
            "targetError",
            "targetTolerance",
            "settledHeadingTolerance",
            "settledLateralSpeedTolerance",
            "headingError",
            "correctionBearing",
            "secondsToTarget",
            "outwardSpeed",
            "timeToEdge",
            "speed",
            "clearTicksBefore",
            "clearTicksAfter",
            "rearmTicks",
            "steeringInput",
            "steering",
        ):
            if field in diagnostic:
                parsed_diagnostic[field] = _finite_number(
                    diagnostic[field], f"diagnostic.{field}"
                )
        for field in (
            "activeBefore",
            "shouldEnter",
            "withinLateral",
            "withinHeading",
            "withinLateralSpeed",
            "settledCandidate",
            "inDecisionZone",
            "correctionArmed",
        ):
            if field in diagnostic:
                value = diagnostic[field]
                if not isinstance(value, bool):
                    raise ValueError(f"diagnostic.{field} must be boolean")
                parsed_diagnostic[field] = value
        edge_id = diagnostic.get("edgeId")
        if edge_id is not None:
            if not isinstance(edge_id, str) or len(edge_id) > 200:
                raise ValueError("diagnostic.edgeId must be a short string")
            parsed_diagnostic["edgeId"] = edge_id
        contact_materials = diagnostic.get("contactMaterials")
        if contact_materials is not None:
            if not isinstance(contact_materials, str) or len(contact_materials) > 500:
                raise ValueError("diagnostic.contactMaterials must be a short string")
            parsed_diagnostic["contactMaterials"] = contact_materials
        diagnostic = parsed_diagnostic

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
        exits = _short_array(junction.get("exits", []), "junction.exits", 16)
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

    signal = packet.get("signal")
    signal_status = packet.get("signalStatus", "unsupported")
    if not isinstance(signal_status, str) or signal_status not in {"unsupported", "inactive", "none", "available"}:
        raise ValueError("invalid signalStatus")
    signal_context = packet.get("signalContext", "")
    if not isinstance(signal_context, str) or len(signal_context) > 200:
        raise ValueError("invalid signalContext")
    if signal is not None:
        if not isinstance(signal, dict):
            raise ValueError("signal must be an object or null")
        signal_id = signal.get("id")
        if not isinstance(signal_id, str) or not signal_id or len(signal_id) > 160:
            raise ValueError("invalid signal.id")
        kind = signal.get("kind")
        signal_state = signal.get("state")
        if (not isinstance(kind, str) or kind not in {"stopSign", "trafficLight"}
            or not isinstance(signal_state, str) or signal_state not in _SIGNAL_STATES):
            raise ValueError("invalid signal kind or state")
        if (kind == "stopSign") != (signal_state == "stop"):
            raise ValueError("signal state does not match kind")
        if not isinstance(signal.get("phase"), str) or signal["phase"] not in _PHASES:
            raise ValueError("invalid signal.phase")
        distance = _finite_number(signal.get("distance"), "signal.distance")
        if distance < 0 or distance > 1000:
            raise ValueError("invalid signal.distance")
        if signal_status != "available" or state != "onRoad":
            raise ValueError("signal requires an available on-road feed")
        group_id = signal.get("groupId", signal_id)
        if not isinstance(group_id, str) or not group_id or len(group_id) > 160:
            raise ValueError("invalid signal.groupId")
        preview = signal.get("preview", False)
        if not isinstance(preview, bool):
            raise ValueError("invalid signal.preview")
        if preview and kind == "trafficLight" and signal_state != "unknown":
            raise ValueError("preview cannot claim an approach-specific light state")
        signal = {"id": signal_id, "kind": kind, "state": signal_state,
                  "phase": signal["phase"], "distance": distance,
                  "groupId": group_id, "preview": preview}
    elif signal_status == "available":
        raise ValueError("available signal is missing")

    proximity = packet.get("proximity")
    if proximity is not None:
        if not isinstance(proximity, dict) or state != "onRoad":
            raise ValueError("proximity requires an on-road object")
        target_id = proximity.get("id")
        if not isinstance(target_id, str) or not target_id or len(target_id) > 200:
            raise ValueError("invalid proximity.id")
        action, source = proximity.get("action"), proximity.get("source")
        if action not in ("stop", "approach") or source not in ("stopPoint", "junctionBoundary"):
            raise ValueError("invalid proximity action or source")
        if source == "junctionBoundary" and action == "stop":
            raise ValueError("junction geometry cannot establish a stop requirement")
        distance = _finite_number(proximity.get("distance"), "proximity.distance")
        speed = _finite_number(proximity.get("speed"), "proximity.speed")
        if not 0 <= distance <= 1000 or speed < 0:
            raise ValueError("invalid proximity distance or speed")
        proximity = {"id": target_id, "distance": distance, "speed": speed,
                     "action": action, "source": source}

    speed_limit = packet.get("speedLimit")
    if speed_limit is not None:
        speed_limit = _finite_number(speed_limit, "speedLimit")
        if speed_limit <= 0 or state != "onRoad":
            raise ValueError("speedLimit requires a positive on-road value")

    return {
        "state": state,
        "speedLimit": speed_limit,
        "proximity": proximity,
        "oneWay": one_way,
        "roadDirections": directions,
        "offRoad": off_road,
        "correction": correction,
        "diagnostic": diagnostic,
        "junction": junction,
        "signal": signal,
        "signalStatus": signal_status,
        "signalContext": signal_context,
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


def one_way_phrase(bearing):
    if abs(bearing) <= 10:
        return "One-way road. Legal travel is ahead"
    side = "left" if bearing > 0 else "right"
    return f"One-way road. Legal travel is {abs(bearing):.0f} degrees to your {side}"


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


def signal_phrase(signal, units, change=False):
    if signal.get("preview"):
        control = "Stop sign" if signal["kind"] == "stopSign" else "Traffic lights"
        return f"{control} ahead, about {format_road_distance(signal['distance'], units)}."
    if signal["kind"] == "stopSign":
        return f"Stop sign in {format_road_distance(signal['distance'], units)}."
    names = {
        "red": "red", "yellow": "yellow", "green": "green",
        "redYellow": "red and yellow", "flashingRed": "flashing red",
        "flashingYellow": "flashing yellow", "flashingGreen": "flashing green",
        "off": "off", "unknown": "state unknown",
    }
    state = names.get(signal["state"], "state unknown")
    if change:
        return f"Traffic light {state}."
    return f"Traffic light {state}, in {format_road_distance(signal['distance'], units)}."


def map_speed_limit_phrase(speed_ms, units):
    if speed_ms is None:
        return "Map speed limit unavailable."
    value = round(speed_ms * (2.2369362920544 if units == "imperial" else 3.6))
    unit = "mph" if units == "imperial" else "kilometers per hour"
    return f"Map speed limit {value} {unit}."


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
            self._signal_context = None
            self._signal_history = {}
            self._signal_options = None
            self._speed_context = None
            self._speed_candidate = None
            self._speed_since = None
            self._speed_announced = None

    def arm_orientation(self):
        """Reintroduce manual road guidance without forgetting shared alerts."""
        with self._lock:
            self._orientation_armed = True
            self._last_state = None

    def _speed_limit_event(self, packet, now, enabled):
        context = (packet.get("signalContext"), enabled)
        if (context != self._speed_context or self.last_r2_time is None
            or now - self.last_r2_time > R2_STALE_SECONDS):
            self._speed_candidate = self._speed_announced = None
            self._speed_since = None
        self._speed_context = context
        if not enabled:
            return None
        value = packet.get("speedLimit") if packet["state"] == "onRoad" else None
        value = round(value, 3) if value is not None else None
        if value != self._speed_candidate or self._speed_since is None:
            self._speed_candidate, self._speed_since = value, now
        # Edge selection can flicker at junctions. A brief missing sample must
        # neither repeat the old limit nor announce that it disappeared.
        dwell = 0.75 if value is not None else 3.0
        if now - self._speed_since < dwell or value == self._speed_announced:
            return None
        self._speed_announced = value
        if packet["state"] != "onRoad":
            return None
        return {"value": value}

    def accept_r2(self, packet, now=None, *, stop_signs=True, traffic_lights=True, speed_limits=True):
        now = self._clock() if now is None else float(now)
        with self._lock:
            speed_event = self._speed_limit_event(packet, now, speed_limits)
            context = packet.get("signalContext")
            options = (stop_signs, traffic_lights)
            if (context != self._signal_context or options != self._signal_options
                or self.last_r2_time is None or now - self.last_r2_time > R2_STALE_SECONDS):
                self._signal_history.clear()
            self._signal_context, self._signal_options = context, options
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

            signal_event = None
            # Brief edge-selection gaps must not repeat an approach announcement.
            self._signal_history = {k: v for k, v in self._signal_history.items()
                                    if now - v[2] < 5.0}
            signal = packet.get("signal")
            if signal and (stop_signs if signal["kind"] == "stopSign" else traffic_lights):
                key = (signal["kind"], signal.get("groupId", signal["id"]))
                previous = self._signal_history.get(key)
                near = signal["phase"] == "near"
                changed = previous is not None and previous[0] != signal["state"]
                if previous is None or changed or (near and not previous[1]):
                    signal_event = dict(signal, change=changed)
                self._signal_history[key] = (signal["state"], near or bool(previous and previous[1]), now)

            return {"orientation": orientation, "junction": junction_event,
                    "signal": signal_event, "speedLimit": speed_event}

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
                self._signal_history.clear()
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

    def speed_limit_phrase(self, enabled, units):
        with self._lock:
            recent = (self.last_r2_time is not None
                      and self._clock() - self.last_r2_time <= R2_STALE_SECONDS)
            packet = self.packet or {}
            value = packet.get("speedLimit") if enabled and recent and packet.get("state") == "onRoad" else None
        return map_speed_limit_phrase(value, units)

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
        parts.append(self.speed_limit_phrase(enabled, units))
        directions = road_direction_list(
            packet.get("roadDirections", []), conjunction="or"
        )
        if packet.get("oneWay"):
            parts.append("One-way road.")
        if directions:
            parts.append(f"Legal direction{'s' if not packet.get('oneWay') else ''}: {directions}.")
        correction = packet.get("correction")
        if correction and correction.get("active"):
            if correction.get("phase") == "unwind":
                parts.append("Straighten steering now.")
            else:
                bearing = correction.get("bearing", 0.0)
                side = "left" if bearing > 0 else "right"
                strength = "strong" if correction.get("severity", 0.0) >= 0.67 else (
                    "moderate" if correction.get("severity", 0.0) >= 0.34 else "small"
                )
                parts.append(f"Apply a {strength} correction {side}.")
        else:
            parts.append("No correction needed.")
        junction = snap["junction"]
        signal = packet.get("signal")
        if signal:
            parts.append(signal_phrase(signal, units))
        elif packet.get("signalStatus", "unsupported") == "unsupported":
            parts.append("Traffic control data unavailable.")
        elif packet.get("signalStatus") == "inactive":
            parts.append("Traffic signal system inactive.")
        else:
            parts.append("No stop sign or traffic light reported ahead.")
        if junction:
            parts.append("Next, " + junction_phrase(junction, units))
        else:
            parts.append("No intersection reported ahead.")
        return " ".join(parts)
