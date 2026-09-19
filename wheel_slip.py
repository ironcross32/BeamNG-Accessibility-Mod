"""Wheel slip (spin / lockup) and side-slide detection, and where to place the voice.

No BeamTel, audio or speech imports, so ``beamtel.py``, the road diagnostic and
challenge analysers, and ``diagnostic/wheel_slip_sim.py`` all run the same rule rather
than three copies of it. Same premise as :mod:`route_beacon`.

Three things this module exists to get right, each measured on the Ice Pool:

* **The wheel signal is the raw per-wheel extreme, never ``electrics.wheelspeed``.**
  That electrics value is a smoothed average of *every* wheel: it trailed a 30 ms
  spin-up by about 0.4 s and read 4.9 m/s with the rears at 21 and the fronts at 0.2.
  The vehicle VM now sends the fastest driven wheel and the slowest wheel of any kind.
* **Spin has no ground-speed gate; lockup does.** A wheel doing 10 m/s on a car doing
  0.1 m/s is unambiguous, and gating spin on the car first reaching 2 m/s is what made
  a clutch dump on ice sound about a second late -- after the throttle was already off.
  At a standstill, though, every wheel "looks locked", so lockup keeps the gate.
* **Wheels are compared with LONGITUDINAL travel only.** Against total ground speed a
  sideways slide reads as lockup, because the lateral motion is speed the wheels never
  see. Sideways travel is its own voice (the slide), measured as the angle between
  the direction of travel and the car's own axis.

Bearings follow the mod-wide convention: **positive is the driver's LEFT**.
"""

from __future__ import annotations

import math
from typing import NamedTuple

from route_beacon import normalize_bearing, relative_bearing


# --- Spin / lockup (raw wheel extremes) ---------------------------------------------
SLIP_TAU_S = 0.05  # EMA on the divergence; the raw signal is already clean
SLIP_SUSTAIN_S = 0.08  # divergence must persist this long (shift and packet jitter)
SLIP_ABS_THRESHOLD_MS = 1.5
SLIP_REL_THRESHOLD = 0.25
SLIP_MIN_GROUND_MS = 2.0  # LOCKUP only -- see module docstring

# --- Legacy rule, for a mod half that does not send the wheel extremes -------------
# Exactly what beamtel did before, so an older bng_mod degrades to the old behaviour
# rather than to silence.
LEGACY_TAU_S = 0.10
LEGACY_SUSTAIN_S = 0.15

# --- Side slide -------------------------------------------------------------------
SLIDE_MIN_GROUND_MS = 3.0  # below this the reference node's heading error is noise
SLIDE_ENTER_DEG = 12.0
SLIDE_EXIT_DEG = 8.0  # hysteresis: a drift held near the threshold must not chatter
SLIDE_SUSTAIN_S = 0.10

# --- Voice placement ----------------------------------------------------------------
# Travel within this of the car's own axis is "the direction the wheels are turning",
# and the voice sits dead ahead or dead behind rather than wobbling by a few degrees.
AZIMUTH_SNAP_DEG = 10.0
# Below DIR_MIN the direction of travel is numerical residue, so the voice is placed by
# the gear (ahead, or behind in reverse); the two are blended up to DIR_FULL.
DIR_MIN_MS = 0.5
DIR_FULL_MS = 2.0

KIND_NONE = 0
KIND_SPIN = 1
KIND_LOCK = -1


class SlipResult(NamedTuple):
    active: bool
    kind: int  # KIND_SPIN, KIND_LOCK or KIND_NONE
    magnitude_ms: float
    slide_active: bool
    slide_deg: float
    azimuth_deg: float  # +LEFT, -180..180
    v_long_ms: float
    filtered_ms: float  # signed: + means ground faster than wheels (lockup side)
    legacy: bool


INACTIVE = SlipResult(False, KIND_NONE, 0.0, False, 0.0, 0.0, 0.0, 0.0, False)


def travel_bearing(vel_x, vel_y, heading_deg):
    """Signed bearing of the direction of TRAVEL relative to the car's nose, +LEFT.

    ``heading_deg`` is BeamTel's MotionSim heading, which is 180 degrees off a true
    compass heading; :func:`route_beacon.relative_bearing` carries the matching offset,
    so the velocity vector is handed to it as a destination seen from the origin.
    Do not re-derive this: a lone correction mirrors every answer front to back.
    """
    return relative_bearing(0.0, 0.0, heading_deg, vel_x, vel_y)[1]


def sideslip_deg(bearing_deg):
    """How far travel is off the car's axis, 0..90, either end of the car."""
    b = abs(normalize_bearing(bearing_deg))
    return min(b, 180.0 - b)


def voice_azimuth(bearing_deg, ground_ms, reverse):
    """Where the slip voices sit: the direction of travel, +LEFT, -180..180."""
    gear_az = 180.0 if reverse else 0.0
    if bearing_deg is None:
        return gear_az
    b = normalize_bearing(bearing_deg)
    if abs(b) < AZIMUTH_SNAP_DEG:
        b = 0.0
    elif abs(b) > 180.0 - AZIMUTH_SNAP_DEG:
        b = 180.0
    w = (ground_ms - DIR_MIN_MS) / (DIR_FULL_MS - DIR_MIN_MS)
    w = max(0.0, min(1.0, w))
    if w >= 1.0:
        return b
    if w <= 0.0:
        return gear_az
    # Blend as unit vectors so ahead-to-behind never sweeps through the side at random.
    x = (1.0 - w) * math.cos(math.radians(gear_az)) + w * math.cos(math.radians(b))
    y = (1.0 - w) * math.sin(math.radians(gear_az)) + w * math.sin(math.radians(b))
    if abs(x) < 1e-9 and abs(y) < 1e-9:
        return b
    return math.degrees(math.atan2(y, x))


def _threshold(v_long):
    return max(SLIP_ABS_THRESHOLD_MS, SLIP_REL_THRESHOLD * v_long)


def _has_wheels(wheel_max, wheel_min):
    return (
        wheel_max is not None
        and wheel_min is not None
        and wheel_max >= 0.0
        and wheel_min >= 0.0
    )


class SlipDetector:
    """Stateful detector, stepped once per telemetry packet."""

    def __init__(self):
        self.reset()

    def reset(self):
        self._prev_ts = 0.0
        self._spin_sm = 0.0
        self._lock_sm = 0.0
        self._spin_since = 0.0
        self._lock_since = 0.0
        self._legacy_sm = 0.0
        self._legacy_since = 0.0
        self._slide_on = False
        self._slide_since = 0.0

    def step(
        self,
        now,
        ground_ms,
        bearing_deg,
        wheel_max_driven,
        wheel_min,
        legacy_wheel_ms,
        reverse=False,
    ):
        """Advance by one packet. ``bearing_deg`` is None when travel has no direction."""
        dt = (now - self._prev_ts) if self._prev_ts > 0.0 else 0.0
        rebase = not (0.0 < dt <= 1.0)
        self._prev_ts = now

        if bearing_deg is not None:
            v_long = abs(ground_ms * math.cos(math.radians(bearing_deg)))
        else:
            v_long = ground_ms

        legacy = not _has_wheels(wheel_max_driven, wheel_min)
        if legacy:
            active, kind, mag, filtered = self._step_legacy(
                now, dt, rebase, ground_ms, legacy_wheel_ms
            )
        else:
            active, kind, mag, filtered = self._step_raw(
                now, dt, rebase, v_long, wheel_max_driven, wheel_min
            )

        slide_active, slide_deg = self._step_slide(now, ground_ms, bearing_deg)
        azimuth = voice_azimuth(bearing_deg, ground_ms, reverse)
        return SlipResult(
            active, kind, mag, slide_active, slide_deg, azimuth, v_long, filtered, legacy
        )

    # -- spin / lockup from the raw wheel extremes --------------------------------------
    def _step_raw(self, now, dt, rebase, v_long, wheel_max, wheel_min):
        spin_raw = wheel_max - v_long
        lock_raw = v_long - wheel_min
        if rebase:
            self._spin_sm, self._lock_sm = spin_raw, lock_raw
        else:
            alpha = 1.0 - math.exp(-dt / SLIP_TAU_S)
            self._spin_sm += alpha * (spin_raw - self._spin_sm)
            self._lock_sm += alpha * (lock_raw - self._lock_sm)
        self._legacy_sm, self._legacy_since = 0.0, 0.0

        thr = _threshold(v_long)
        spin_on = self._sustain("_spin_since", now, self._spin_sm > thr)
        lock_on = self._sustain(
            "_lock_since", now, v_long > SLIP_MIN_GROUND_MS and self._lock_sm > thr
        )
        # Positive = lockup side, matching the legacy filtered value.
        filtered = self._lock_sm if self._lock_sm >= self._spin_sm else -self._spin_sm
        if spin_on and (not lock_on or self._spin_sm >= self._lock_sm):
            return True, KIND_SPIN, self._spin_sm, filtered
        if lock_on:
            return True, KIND_LOCK, self._lock_sm, filtered
        return False, KIND_NONE, 0.0, filtered

    # -- the pre-v4 rule, unchanged ------------------------------------------------------
    def _step_legacy(self, now, dt, rebase, ground_ms, wheel_ms):
        raw = ground_ms - (wheel_ms or 0.0)
        if rebase:
            self._legacy_sm = raw
        else:
            alpha = 1.0 - math.exp(-dt / LEGACY_TAU_S)
            self._legacy_sm += alpha * (raw - self._legacy_sm)
        self._spin_sm = self._lock_sm = 0.0
        self._spin_since = self._lock_since = 0.0

        thr = _threshold(ground_ms)
        diverging = ground_ms > SLIP_MIN_GROUND_MS and abs(self._legacy_sm) > thr
        if diverging:
            if self._legacy_since == 0.0:
                self._legacy_since = now
            if (now - self._legacy_since) >= LEGACY_SUSTAIN_S:
                kind = KIND_LOCK if self._legacy_sm > 0.0 else KIND_SPIN
                return True, kind, abs(self._legacy_sm), self._legacy_sm
        else:
            self._legacy_since = 0.0
        return False, KIND_NONE, 0.0, self._legacy_sm

    # -- sideways travel -----------------------------------------------------------------
    def _step_slide(self, now, ground_ms, bearing_deg):
        if bearing_deg is None or ground_ms <= SLIDE_MIN_GROUND_MS:
            self._slide_on = False
            self._slide_since = 0.0
            return False, 0.0
        beta = sideslip_deg(bearing_deg)
        if self._slide_on:
            if beta < SLIDE_EXIT_DEG:
                self._slide_on = False
                self._slide_since = 0.0
        elif beta > SLIDE_ENTER_DEG:
            if self._slide_since == 0.0:
                self._slide_since = now
            if (now - self._slide_since) >= SLIDE_SUSTAIN_S:
                self._slide_on = True
        else:
            self._slide_since = 0.0
        return self._slide_on, (beta if self._slide_on else 0.0)

    def _sustain(self, attr, now, cond):
        if not cond:
            setattr(self, attr, 0.0)
            return False
        since = getattr(self, attr)
        if since == 0.0:
            setattr(self, attr, now)
            since = now
        return (now - since) >= SLIP_SUSTAIN_S


def _finite(value):
    try:
        value = float(value)
    except (TypeError, ValueError):
        return None
    return value if math.isfinite(value) else None


def classify(sample):
    """Stateless one-shot classification of a recorded telemetry snapshot.

    For the analysers (road diagnostics, challenge results), which look at recorded
    samples rather than a live stream. Uses the raw wheel extremes and the travel
    bearing when the sample carries them, and the legacy total-speed rule otherwise, so
    recordings made before those fields existed still analyse the same way they did.
    Returns ``{"active", "kind", "magnitude_mps", "raw_mps", "slide", "slide_deg"}``;
    ``kind`` is ``"wheelspin"``, ``"lockup"`` or ``"none"``, and ``raw_mps`` is signed
    with positive meaning the ground is faster than the wheels.
    """
    ground = _finite(sample.get("ground_speed_ms")) or 0.0
    bearing = _finite(sample.get("travel_bearing_deg"))
    wmax = _finite(sample.get("wheel_max_driven_ms"))
    wmin = _finite(sample.get("wheel_min_ms"))

    slide_deg = sideslip_deg(bearing) if bearing is not None else 0.0
    slide = (
        bearing is not None and ground > SLIDE_MIN_GROUND_MS and slide_deg > SLIDE_ENTER_DEG
    )

    if _has_wheels(wmax, wmin):
        v_long = abs(ground * math.cos(math.radians(bearing))) if bearing is not None else ground
        spin = wmax - v_long
        lock = v_long - wmin
        thr = _threshold(v_long)
        spin_on = spin > thr
        lock_on = v_long > SLIP_MIN_GROUND_MS and lock > thr
        if spin_on and (not lock_on or spin >= lock):
            kind, mag, raw = "wheelspin", spin, -spin
        elif lock_on:
            kind, mag, raw = "lockup", lock, lock
        else:
            kind = "none"
            raw = lock if lock >= spin else -spin
            mag = abs(raw)
    else:
        wheel = _finite(sample.get("wheel_speed_ms")) or 0.0
        raw = ground - wheel
        active = ground > SLIP_MIN_GROUND_MS and abs(raw) > _threshold(ground)
        kind = ("lockup" if raw > 0 else "wheelspin") if active else "none"
        mag = abs(raw)

    return {
        "active": kind != "none",
        "kind": kind,
        "magnitude_mps": round(mag, 4),
        "raw_mps": round(raw, 4),
        "slide": slide,
        "slide_deg": round(slide_deg if slide else 0.0, 2),
    }
