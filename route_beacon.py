"""Protocol and wording helpers for the BeamTel route beacon.

The UDP listener lives in :mod:`beamtel`; this module deliberately has no BeamTel,
audio, or speech imports so its parsing, bearing sign and phrasing rules can be
exercised without starting the application. Same premise as :mod:`road_guidance`.
"""

from __future__ import annotations

import math


ROUTE_PREFIX = "ROUTE:"
ROUTE_CLEAR = "CLEAR"

# Below this the beacon is on top of the destination and the bearing to it becomes
# numerical residue -- the direction to a point you are standing on is not a fact.
AT_DESTINATION_M = 15.0


def parse_route_packet(text):
    """Parse one ROUTE datagram.

    Returns ``None`` for ``ROUTE:CLEAR`` -- which is a *message*, not an absence: "no
    route is set" and "the mod stopped talking" are different facts, and Python must
    not have to infer which from a timeout. Raises ``ValueError`` on anything else
    that does not parse, so a malformed packet is logged rather than silently
    becoming a destination somewhere near the origin.
    """
    if not isinstance(text, str) or not text.startswith(ROUTE_PREFIX):
        raise ValueError("not a ROUTE packet")
    body = text[len(ROUTE_PREFIX) :].strip()
    if body == ROUTE_CLEAR:
        return None

    parts = body.split(",")
    if len(parts) < 4:
        raise ValueError("ROUTE packet needs x, y, z and remaining distance")
    try:
        values = [float(p) for p in parts[:4]]
    except (TypeError, ValueError) as exc:
        raise ValueError("ROUTE fields must be numbers") from exc
    if not all(math.isfinite(v) for v in values):
        raise ValueError("ROUTE fields must be finite")

    x, y, z, remaining = values
    return {"dest": (x, y, z), "route_m": max(0.0, remaining)}


def target_bearing(pos_x, pos_y, dest_x, dest_y):
    """Absolute bearing to a destination, in BeamTel's own heading frame.

    That frame is 180 degrees off a true compass bearing, which is why the ``atan2``
    arguments are negated. It is the *same* 180 that ``yaw_to_heading_deg`` puts into
    every heading BeamTel handles, so the two cancel wherever one is subtracted from
    the other -- see :func:`relative_bearing`. Kept as one function because three
    places in ``beamtel.py`` had written this expression out, and a sign that drifted
    between them would be invisible in all three.
    """
    dx = float(dest_x) - float(pos_x)
    dy = float(dest_y) - float(pos_y)
    return math.degrees(math.atan2(-dx, -dy)) % 360.0


def normalize_bearing(degrees):
    """Fold an angle onto -180..180, positive LEFT."""
    value = float(degrees)
    if value > 180.0:
        value -= 360.0
    elif value < -180.0:
        value += 360.0
    return value


def relative_bearing(pos_x, pos_y, heading_deg, dest_x, dest_y):
    """Return ``(distance_m, signed_bearing_deg)`` from a position to a destination.

    **Positive bearing is the driver's LEFT**, the mod-wide convention, and the whole
    reason this lives in one function: the marked-waypoint readout and the route
    beacon both need exactly this and a sign that disagreed between them would be
    invisible in both. ``audio._render_directional_pulse`` consumes the same
    convention, so the value returned here can be handed to it unchanged.

    Flattened -- z is ignored -- like every other bearing in this mod. A destination
    a kilometre away and two hundred metres up is not two hundred metres of steering.

    ``heading_deg`` is BeamTel's own heading (``yaw_to_heading_deg`` of the MotionSim
    ``yawPos``), which is **180 degrees off a true compass heading**, and the
    ``atan2(-dx, -dy)`` below carries the matching ``+180`` so that the two cancel in
    the subtraction. That is not a redundant pair of negations to be tidied away: the
    result is only the true relative bearing *because* both offsets are exactly 180.
    Correcting one without the other mirrors the answer front-to-back, which on a
    beacon reads as a destination behind you that is in fact ahead. Verified against
    the geometry -- facing north (heading 180 here), a destination due west answers
    +90 and one due east answers -90.
    """
    dx = float(dest_x) - float(pos_x)
    dy = float(dest_y) - float(pos_y)
    distance = math.sqrt(dx * dx + dy * dy)
    err = normalize_bearing(
        float(heading_deg) - target_bearing(pos_x, pos_y, dest_x, dest_y)
    )
    return distance, err


def format_long_distance(metres, units):
    """Phrase a distance that may be kilometres rather than metres.

    ``road_guidance.format_road_distance`` answers in feet or metres, which is right
    for a junction a couple of hundred metres off and useless for a route: "12467
    feet" is not a distance anybody can hold. This switches to the larger unit once
    the smaller one stops being readable.
    """
    value = max(0.0, float(metres))
    if str(units).lower().startswith("imp"):
        feet = value * 3.28084
        if feet < 1000.0:
            return f"{feet:.0f} feet"
        return f"{feet / 5280.0:.1f} miles"
    if value < 1000.0:
        return f"{value:.0f} meters"
    return f"{value / 1000.0:.1f} kilometers"


def route_beacon_phrase(distance_m, bearing_deg, route_m, units):
    """The sentence spoken when the beacon is switched on.

    Names the crow-flies distance *first* because that is what the beacon itself is
    indicating; the route distance follows, and is worth saying because the gap
    between the two is exactly how far the roads are about to take you out of your
    way. Omitted when the mod could not supply it, rather than spoken as zero.
    """
    from road_guidance import direction_label

    if distance_m <= AT_DESTINATION_M:
        head = "Route beacon on, at the destination"
    else:
        where = direction_label(bearing_deg, u_turn=True)
        if where == "straight":
            where = "straight ahead"
        elif where == "U-turn":
            where = "behind you"
        head = (
            f"Route beacon on, destination "
            f"{format_long_distance(distance_m, units)} {where}"
        )
    if route_m and route_m > 0.0:
        return f"{head}, {format_long_distance(route_m, units)} by road"
    return head
