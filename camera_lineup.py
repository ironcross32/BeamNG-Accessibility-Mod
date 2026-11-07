"""Free-camera line-up guide: geometry, tone mapping and wording.

Pure functions only, so diagnostic/camera_lineup_sim.py can import it. The sockets, the key
layer and the state live in beamtel.py (the `_lineup_*` block); the synthesis is audio.py's
`_lineup_*` block. See docs/camera-lineup.md.

The one test both modes share: does the ray from the front centre of the car, along its
heading, pass through a target point P?

  head-on   P = camera + camera_left * offset   (offset positive = car passes the camera's LEFT)
  crossing  P = camera + camera_forward * depth (the car's path crosses the view at that depth)

The error is the signed sideways miss of the car's path at P, in the CAR's frame, positive
LEFT (the mod-wide convention: lateral is up x forward, built from the un-negated forward).
A path missing to the left means the car is left of the line: the FM ratio dips below 1.0 and
the sound sits on the left. A target behind the nose has no miss at all -- the path never
reaches it -- and reads as full error from behind.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Optional

HEAD_ON = "head_on"
CROSSING = "crossing"

DEFAULT_OFFSET_M = 3.0  # head-on: pass 3 m to the camera's left
DEFAULT_DEPTH_M = 10.0  # crossing: cross the view 10 m out
STEP_M = 1.0
OFFSET_LIMIT_M = 50.0
DEPTH_MIN_M = 1.0
DEPTH_MAX_M = 200.0

DEADZONE_M = 0.5  # silent within this miss
FULL_M = 8.0  # full volume and grit at this miss
INDEX_MIN = 1.0
INDEX_MAX = 1.3
RATIO_DRIFT = 0.08  # ratio 0.92 at full left, 1.08 at full right
CARRIER_HZ = 440.0
SIDE_AZIMUTH_DEG = 90.0  # how far round the head the sound swings at full error
BEHIND_AZIMUTH_DEG = 180.0

FEET_PER_M = 3.28084


@dataclass(frozen=True)
class Geometry:
    cam_x: float
    cam_y: float
    cam_fx: float
    cam_fy: float
    nose_x: float
    nose_y: float
    veh_fx: float
    veh_fy: float


@dataclass(frozen=True)
class Tone:
    """Audio parameters. amp is 0..1 of the guide's peak level; az is HRTF degrees
    (0 front, 90 left, counter-clockwise)."""

    amp: float
    index: float
    ratio: float
    az: float


SILENT = Tone(0.0, INDEX_MIN, 1.0, 0.0)


def parse_packet(text: str):
    """'OFF' for LINEUP:OFF, a Geometry for a live packet, None for anything else."""
    text = (text or "").strip()
    if not text.startswith("LINEUP:"):
        return None
    body = text[7:]
    if body == "OFF":
        return "OFF"
    parts = body.split(",")
    if len(parts) < 8:
        return None
    try:
        vals = [float(p) for p in parts[:8]]
    except ValueError:
        return None
    if not all(math.isfinite(v) for v in vals):
        return None
    return Geometry(*vals)


def _unit(x, y):
    n = math.hypot(x, y)
    if n < 1e-6:
        return None
    return x / n, y / n


def target_point(geom: Geometry, mode: str, offset_m: float, depth_m: float):
    cf = _unit(geom.cam_fx, geom.cam_fy)
    if cf is None:
        return None
    if mode == CROSSING:
        return geom.cam_x + cf[0] * depth_m, geom.cam_y + cf[1] * depth_m
    left = (-cf[1], cf[0])  # up x forward, positive LEFT
    return geom.cam_x + left[0] * offset_m, geom.cam_y + left[1] * offset_m


def solve(geom: Geometry, mode: str, offset_m: float, depth_m: float):
    """(status, miss, bearing).

    status   'ok', 'behind' (target not ahead of the nose) or 'unknown' (degenerate frame)
    miss     signed perpendicular miss in metres, positive = path passes LEFT of P; None
             unless 'ok'
    bearing  signed angle from the car's heading to P in degrees, positive = P to the LEFT,
             in (-180, 180]; None only when 'unknown'
    """
    p = target_point(geom, mode, offset_m, depth_m)
    vf = _unit(geom.veh_fx, geom.veh_fy)
    if p is None or vf is None:
        return "unknown", None, None
    vl = (-vf[1], vf[0])
    dx, dy = p[0] - geom.nose_x, p[1] - geom.nose_y
    along = dx * vf[0] + dy * vf[1]
    lat = dx * vl[0] + dy * vl[1]  # how far LEFT of the car's path the target is
    bearing = math.degrees(math.atan2(lat, along))
    if along <= 0.0:
        return "behind", None, bearing
    # Target to the car's left means the path passes to the target's RIGHT.
    return "ok", -lat, bearing


def miss_left_m(geom: Geometry, mode: str, offset_m: float, depth_m: float):
    """(status, miss) -- solve() without the bearing."""
    status, miss, _bearing = solve(geom, mode, offset_m, depth_m)
    return status, miss


def tone_for(status: str, miss: Optional[float], bearing: Optional[float] = None) -> Tone:
    """Loudness and grit come from the miss in metres; position comes from the bearing.

    Position is the bearing to the target MIRRORED onto the side of the error: a target
    20 degrees to the right means the path misses left, so the sound sits 20 degrees left.
    That keeps it continuous all the way round -- a target abeam sounds at the side, one
    behind sounds behind -- and it moves with the steering angle directly, where a position
    taken from the metres jumps (far out, a tiny heading change is metres of miss).
    """
    if status == "behind":
        if bearing is None:
            return Tone(1.0, INDEX_MAX, 1.0, BEHIND_AZIMUTH_DEG)
        side = -1.0 if bearing > 0 else 1.0  # error side, +1 left
        return Tone(1.0, INDEX_MAX, _ratio_for(side, bearing), (-bearing) % 360.0)
    if status != "ok" or miss is None:
        return SILENT
    t = (abs(miss) - DEADZONE_M) / (FULL_M - DEADZONE_M)
    t = max(0.0, min(1.0, t))
    if t <= 0.0:
        return SILENT
    side = 1.0 if miss > 0 else -1.0  # +1 left
    if bearing is None:
        az = (side * SIDE_AZIMUTH_DEG * t) % 360.0
        ratio = 1.0 - RATIO_DRIFT * t * side
    else:
        az = (-bearing) % 360.0
        ratio = _ratio_for(side, bearing)
    return Tone(
        amp=t,
        index=INDEX_MIN + (INDEX_MAX - INDEX_MIN) * t,
        ratio=ratio,
        az=az,
    )


def _ratio_for(side: float, bearing: float) -> float:
    """The ratio drifts with the ANGLE, not the metres: 1.0 lined up, the full drift only at
    180 degrees (target straight behind). Tied to the metres it saturated at FULL_M, which
    is a few degrees of turn far out, so most of the turn carried no change at all."""
    return 1.0 - RATIO_DRIFT * side * min(1.0, abs(bearing) / 180.0)


def step_value(mode: str, offset_m: float, depth_m: float, delta: int):
    """Returns the new (offset_m, depth_m) after `delta` steps."""
    if mode == CROSSING:
        depth_m = max(DEPTH_MIN_M, min(DEPTH_MAX_M, depth_m + delta * STEP_M))
    else:
        offset_m = max(-OFFSET_LIMIT_M, min(OFFSET_LIMIT_M, offset_m + delta * STEP_M))
    return offset_m, depth_m


def _dist(m: float, units: str) -> str:
    if units == "imperial":
        v = round(abs(m) * FEET_PER_M)
        return f"{v} {'foot' if v == 1 else 'feet'}"
    v = round(abs(m))
    return f"{v} {'meter' if v == 1 else 'meters'}"


def value_phrase(mode: str, offset_m: float, depth_m: float, units: str) -> str:
    if mode == CROSSING:
        return f"{_dist(depth_m, units)} out"
    if abs(offset_m) < 0.05:
        return "through the camera"
    return f"{_dist(offset_m, units)} {'left' if offset_m > 0 else 'right'}"


def mode_name(mode: str) -> str:
    return "Crossing" if mode == CROSSING else "Head-on"


def target_phrase(mode: str, offset_m: float, depth_m: float, units: str) -> str:
    v = value_phrase(mode, offset_m, depth_m, units)
    if mode == HEAD_ON and abs(offset_m) >= 0.05:
        v += " of the camera"
    return f"{mode_name(mode)}, {v}"
