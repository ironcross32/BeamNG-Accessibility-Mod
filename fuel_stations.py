"""Parsing and speech wording for fuelStations.lua (F9 then Shift+F).

Pure helpers: no sockets, no state. beamtel.py owns the listener and the browser.
"""

import json
import math

from route_beacon import format_long_distance

ROW_PREFIX = "FS:"
EVENT_PREFIX = "FS_EVENT:"

_ENERGY_WORDS = {
    "gasoline": "gasoline",
    "diesel": "diesel",
    "kerosine": "kerosene",
    "n2o": "nitrous",
    "electricEnergy": "EV charging",
    "any": "any fuel",
    "fuel": "fuel",
}


def parse_row(packet):
    """One ``FS:`` station row, validated. Raises ValueError."""
    if not isinstance(packet, str) or not packet.startswith(ROW_PREFIX):
        raise ValueError("not a station row")
    try:
        row = json.loads(packet[len(ROW_PREFIX) :])
    except json.JSONDecodeError as exc:
        raise ValueError("invalid station JSON") from exc
    if not isinstance(row, dict) or not row.get("id"):
        raise ValueError("station row has no id")
    try:
        distance = float(row.get("distance", 0.0))
        bearing = float(row.get("bearing", 0.0))
    except (TypeError, ValueError) as exc:
        raise ValueError("station row has invalid navigation values") from exc
    if not (math.isfinite(distance) and math.isfinite(bearing)):
        raise ValueError("station row has non-finite navigation values")
    energy = row.get("energy")
    return {
        "id": str(row["id"]),
        "name": str(row.get("name") or "Fuel station"),
        "official": bool(row.get("official", True)),
        "energy": [str(e) for e in energy] if isinstance(energy, list) else [],
        "distance": max(0.0, distance),
        "bearing": (bearing + 180.0) % 360.0 - 180.0,
        "pumps": int(row.get("pumps") or 0),
        "compatible": row.get("compatible"),
        "target": bool(row.get("target", False)),
    }


def parse_event(packet):
    """One ``FS_EVENT:`` object. Raises ValueError."""
    if not isinstance(packet, str) or not packet.startswith(EVENT_PREFIX):
        raise ValueError("not a fuel station event")
    try:
        data = json.loads(packet[len(EVENT_PREFIX) :])
    except json.JSONDecodeError as exc:
        raise ValueError("invalid event JSON") from exc
    if not isinstance(data, dict) or not data.get("kind"):
        raise ValueError("event has no kind")
    return data


def energy_phrase(energy):
    """'gasoline and diesel', 'EV charging', 'gasoline, diesel and nitrous'."""
    words = []
    for e in energy:
        word = _ENERGY_WORDS.get(e, e)
        if word and word not in words:
            words.append(word)
    if not words:
        return "fuel type not listed"
    if len(words) == 1:
        return words[0]
    return ", ".join(words[:-1]) + " and " + words[-1]


def direction_phrase(bearing):
    """Eight-way direction relative to the car. Positive bearing is LEFT."""
    b = (float(bearing) + 180.0) % 360.0 - 180.0
    side = "left" if b > 0 else "right"
    a = abs(b)
    if a < 22.5:
        return "ahead"
    if a < 67.5:
        return f"ahead {side}"
    if a < 112.5:
        return f"to the {side}"
    if a < 157.5:
        return f"behind {side}"
    return "behind"


def row_line(row, units):
    """One browser line: name, distance and direction, what it offers, caveats."""
    parts = [row["name"]]
    if row["distance"] < 1.0:
        parts.append("here")
    else:
        parts.append(
            f"{format_long_distance(row['distance'], units)} {direction_phrase(row['bearing'])}"
        )
    parts.append(energy_phrase(row["energy"]))
    if not row["official"]:
        parts.append("unofficial, the game can't refuel here")
    elif row["compatible"] is False:
        parts.append("not for your vehicle")
    if row["target"]:
        parts.append("current destination")
    return ", ".join(parts)


def _pump_phrase(data):
    pump = data.get("pump")
    side = data.get("side")
    where = ""
    if side in ("left", "right"):
        where = f", on your {side}"
    if not data.get("official", True):
        return f"the pumps{where}"
    return f"pump {pump}{where}" if pump else f"a pump{where}"


def event_phrase(data):
    """What to say for an event, or None to stay quiet."""
    kind = data.get("kind")
    station = data.get("station") or "the station"
    if kind == "target":
        if data.get("here"):
            return f"{station}. You are here"
        return f"Route set to {station}. Fuel station assistance on"
    if kind == "in_range":
        return f"Arriving at {station}. Press F9 then Shift F to park at a pump"
    if kind == "parking":
        return f"Parking at {_pump_phrase(data)}. Brake to cancel"
    if kind == "parked":
        if not data.get("official", True):
            return "Stopped by the pumps. The game has no refuelling here"
        action = "recharge" if data.get("electric") else "refuel"
        if data.get("inZone"):
            return (
                f"Stopped at {_pump_phrase(data)}. Put the car in park, "
                f"then press Shift E to {action}"
            )
        return (
            f"Stopped beside {_pump_phrase(data)}, but the game has not registered the car "
            "at the pump. Move a little closer to it"
        )
    if kind in ("park_refused", "park_failed", "error"):
        message = data.get("message") or "Parking failed"
        return message if kind != "park_failed" else f"Parking stopped. {message}"
    if kind == "park_cancelled":
        return "Parking cancelled"
    if kind == "cancelled":
        return "Fuel station assistance off"
    return None
