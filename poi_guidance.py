"""Parsing and speech wording for the nearest BeamNG point of interest."""

import html
import json
import math
import re


POI_PACKET_PREFIX = "POI:"
POI_PARKED_RADIUS_M = 8.0
_TAG_RE = re.compile(r"<[^>]*>")
_SPACE_RE = re.compile(r"\s+")


def _spoken_text(value):
    """Turn the small amount of HTML used by POI descriptions into plain speech."""
    if not isinstance(value, str):
        return ""
    value = re.sub(r"<\s*br\s*/?\s*>", ". ", value, flags=re.IGNORECASE)
    value = _TAG_RE.sub(" ", value)
    return _SPACE_RE.sub(" ", html.unescape(value)).strip(" .")


def parse_poi_packet(packet):
    """Parse and validate one ``POI:`` JSON datagram from terrainScanner.lua."""
    if not isinstance(packet, str) or not packet.startswith(POI_PACKET_PREFIX):
        raise ValueError("not a POI packet")
    try:
        row = json.loads(packet[len(POI_PACKET_PREFIX) :])
    except json.JSONDecodeError as exc:
        raise ValueError("invalid POI JSON") from exc
    if not isinstance(row, dict):
        raise ValueError("POI payload is not an object")

    name = _spoken_text(row.get("name")) or "Point of interest"
    description = _spoken_text(row.get("description"))
    try:
        distance = float(row["distance"])
        bearing = float(row["bearing"])
        radius = float(row.get("radius", 0.0))
    except (KeyError, TypeError, ValueError) as exc:
        raise ValueError("POI payload has invalid navigation values") from exc
    if not all(math.isfinite(v) for v in (distance, bearing, radius)):
        raise ValueError("POI payload has non-finite navigation values")
    if distance < 0:
        raise ValueError("POI distance is negative")

    return {
        "name": name,
        "description": description,
        "distance": distance,
        "bearing": (bearing + 180.0) % 360.0 - 180.0,
        "radius": max(0.0, radius),
    }


def poi_phrase(poi, units_mode="imperial"):
    """Describe a POI, ending immediately after the parked status when applicable."""
    parts = [poi["name"]]
    if poi.get("description"):
        parts.append(poi["description"])

    centre_distance = max(0.0, float(poi["distance"]))
    radius = max(POI_PARKED_RADIUS_M, float(poi.get("radius", 0.0)))
    if centre_distance <= radius:
        parts.append("Parked on it")
        return ". ".join(parts) + "."

    distance = centre_distance - radius
    if units_mode == "imperial":
        distance_text = f"{distance * 3.28084:.0f} feet"
    else:
        distance_text = f"{distance:.0f} meters"

    bearing = float(poi["bearing"])
    if abs(bearing) < 0.5:
        bearing_text = "bearing 0 degrees, straight ahead"
    else:
        direction = "left" if bearing > 0 else "right"
        bearing_text = f"bearing {abs(bearing):.0f} degrees {direction}"
    parts.append(f"{distance_text}, {bearing_text}")
    return ". ".join(parts) + "."
