"""Golden cases for beam_engine::units, from beamtel.py's units and bearing helpers.

    uv run python rust/fixtures/gen/units.py
"""

from __future__ import annotations

import math

from _beamtel import beamtel
from _common import fbits, write_fixture

b = beamtel


def enc(v):
    """A Python scalar as a tagged value, so the Rust side can check the TYPE as well."""
    if isinstance(v, bool):
        return {"bool": v}
    if isinstance(v, int):
        return {"int": v}
    if isinstance(v, float):
        return {"float": fbits(v)}
    if isinstance(v, str):
        return {"str": v}
    raise TypeError(type(v).__name__)


def pair_outcome(fn, x):
    try:
        val, unit = fn(x)
    except Exception as exc:  # noqa: BLE001
        return {"err": type(exc).__name__}
    return {"val": enc(val), "unit": unit, "text": f"{val} {unit}"}


BASE = [
    0.0, -0.0, 1.0, -1.0, 0.5, 1.5, 2.5, -0.5, -1.5, 10.0, 11.176, 12.5, 26.8224, 33.3333,
    99.95, 99.94, 100.0, 160.9344, 160.93, 999.4, 1609.344, 16093.44, 16093.0, 16092.0,
    1e6, 123456.789, -50.0, 0.04999, 0.05, 0.15, 1 / 3, 2 / 3, 0.1 + 0.2, 1e-9, 3.6e3,
    85.0, 37.5, -40.0, 1234567.0, 9999.0, 10000.0, 160934.4, 1e12, 5e-324,
]
# Inputs that land exactly (or nearly) on a rounding tie after each conversion.
TIES = []
for k in (0.5, 1.5, 2.5, 3.5, 24.5, 25.5, 99.5):
    TIES += [
        k / b.MPH_PER_MS, k / b.KMH_PER_MS, k * b.PSI_PER_BAR, k / b.PSI_PER_BAR,
        k / b.FEET_PER_M, k / b.HP_PER_KW, k / b.LBFT_PER_NM, k / b.MILES_PER_M,
        k * 1000.0, (k - 32.0) * 5.0 / 9.0,
    ]
SPECIAL = [math.nan, math.inf, -math.inf]
INPUTS = BASE + TIES + SPECIAL

FMT = [
    "fmt_speed", "fmt_pressure", "fmt_turbo", "fmt_temp_c_or_f", "fmt_distance",
    "fmt_power_kw", "fmt_torque_nm", "fmt_range", "fmt_trip", "fmt_odometer",
]

cases = []
for mode in ("imperial", "metric"):
    b.UNITS_MODE = mode
    for name in FMT:
        fn = getattr(b, name)
        for x in INPUTS:
            cases.append({"fn": name, "units": mode, "x": fbits(x), "out": pair_outcome(fn, x)})

buckets = []
for mode in ("imperial", "metric"):
    b.UNITS_MODE = mode
    for interval in (25, 10, 1, 0, -5, 7):
        b.speed_announce_interval = interval
        for x in INPUTS:
            try:
                out = {"ok": b.get_speed_bucket(x)}
            except Exception as exc:  # noqa: BLE001
                out = {"err": type(exc).__name__}
            buckets.append({"units": mode, "interval": interval, "x": fbits(x), "out": out})
b.speed_announce_interval = 25

yaws = []
for x in BASE + [math.pi, -math.pi, 2 * math.pi, 7.0, -7.0, 1e-17, -1e-17] + SPECIAL:
    yaws.append({"x": fbits(x), "out": fbits(b.yaw_to_heading_deg(x))})

headings = []
for x in BASE + [359.95, 359.94, 0.05, 0.25, 0.35, 12.25] + SPECIAL:
    b.last_heading = x
    headings.append({"x": fbits(x), "out": b.fmt_heading()})
b.last_heading = 0.0

caps = []
for proto in ("outgauge", "extended", "other"):
    for presence in (0, b.TELEMETRY_HAS_AIR, b.TELEMETRY_HAS_ENGINE_TEMP, b.TELEMETRY_HAS_ELECTRIC,
                     b.WHEEL_POS_CORNERS | b.TELEMETRY_HAS_AIR | b.TELEMETRY_HAS_ELECTRIC, 1023):
        for trip, odo in ((-1.0, -1.0), (0.0, 50.0), (0.0, 100.0), (0.0, 100.5), (10.0, 200.0),
                          (-0.5, 1000.0), (5.0, 105.0)):
            b.protocol_mode = proto
            b.last_telemetry_presence = presence
            b.last_trip_m = trip
            b.last_odometer_m = odo
            caps.append({
                "protocol": proto, "presence": presence, "trip": fbits(trip), "odo": fbits(odo),
                "air": b.has_air_system(), "mileage": b.has_stored_mileage(),
                "engine_temp": b.has_engine_temp(), "electric": b.has_electric_drive(),
            })

b.UNITS_MODE = "imperial"
flips = []
for _ in range(3):
    b.flip_units()
    flips.append(b.UNITS_MODE)
b.UNITS_MODE = "imperial"

write_fixture("units", {
    "fmt": cases,
    "buckets": buckets,
    "yaw": yaws,
    "heading": headings,
    "caps": caps,
    "flips": flips,
    "compass_names": list(b.COMPASS_NAMES),
    "consts": {
        "MPH_PER_MS": fbits(b.MPH_PER_MS), "KMH_PER_MS": fbits(b.KMH_PER_MS),
        "PSI_PER_BAR": fbits(b.PSI_PER_BAR), "FEET_PER_M": fbits(b.FEET_PER_M),
        "HP_PER_KW": fbits(b.HP_PER_KW), "LBFT_PER_NM": fbits(b.LBFT_PER_NM),
        "MILES_PER_M": fbits(b.MILES_PER_M),
    },
})
