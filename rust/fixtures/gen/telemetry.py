"""Golden cases for beam_engine::telemetry (E1): the packet formats and telemetry_loop.

    uv run python rust/fixtures/gen/telemetry.py

Two halves:

* ``decode``: packets packed with ``struct.pack`` for OutGauge, every Extended layout
  (current and each ``EXT_FORMAT_V*`` fallback) and MotionSim, with the tuple the loop
  builds from each (``struct.unpack`` plus the same padding tuples, in the same order).
* ``scenarios``: the REAL ``beamtel.telemetry_loop`` driven packet by packet. The socket,
  the clock, ``say``, the drift announcer, the audio controller and the two UDP pushers
  are replaced with recorders; after every packet the globals the loop writes and every
  call it made are captured. The Rust test replays the same packets through
  ``TelemetryLoop::handle_datagram`` and must produce the same calls and state.
"""

from __future__ import annotations

import copy
import logging
import math
import socket as real_socket
import struct
import types

from _beamtel import beamtel
from _common import fbits, write_fixture

b = beamtel
NAN = math.nan
INF = math.inf


def enc(v):
    """A loop value as JSON: floats exact, ints/bools/str/None as themselves."""
    if v is None or isinstance(v, (bool, str)):
        return v
    if isinstance(v, int):
        return v
    if isinstance(v, float):
        return fbits(v)
    if isinstance(v, bytes):
        return {"bytes": v.hex()}
    raise TypeError(type(v).__name__)


# ---------------------------------------------------------------------------------------
#  Packet builders
# ---------------------------------------------------------------------------------------
EXT_HEAD = ["flags", "gear", "byte2", "speed", "rpm", "rpm_max", "turbo", "turbo_max",
            "engtemp", "fuel", "oil_pressure", "oiltemp", "uint12", "lights"]
EXT_TAIL = [
    "throttle", "brake", "clutch", "steering", "actual_steering", "steering_input",
    "air", "air_max", "clutch_temp", "g_lat", "g_lon",
    "tp_fl", "tp_fr", "tp_rl", "tp_rr", "tt_fl", "tt_fr", "tt_rl", "tt_rr",
    "bt_fl", "bt_fr", "bt_rl", "bt_rr",
    "sig_l", "sig_r", "hazard", "lightbar", "fog",
    "impl_flags", "impl_edge", "impl_clear", "impl_tilt", "impl_tilt_pct", "impl_lift",
    "impl_activity", "artic",
    "tp_f", "tp_r", "tt_f", "tt_r", "bt_f", "bt_r",
]
assert len(EXT_TAIL) == 42
EXT_TAIL8 = ["wmax", "wmin", "power", "torque", "range", "trip", "odo", "life"]

EXT_DEFAULTS = {
    "flags": 0, "gear": b"N", "byte2": 0, "speed": 0.0, "rpm": 800.0, "rpm_max": 6500.0,
    "turbo": 0.0, "turbo_max": 0.0, "engtemp": 90.0, "fuel": 0.5, "oil_pressure": 3.0,
    "oiltemp": 95.0, "uint12": 0, "lights": 0, "presence": 15 | 128,
}
for _i, _n in enumerate(EXT_TAIL):
    EXT_DEFAULTS.setdefault(_n, 0.0)
for _n in EXT_TAIL8:
    EXT_DEFAULTS.setdefault(_n, -1.0)
EXT_DEFAULTS.update({"tp_fl": 32.0, "tp_fr": 32.5, "tp_rl": 31.0, "tp_rr": 31.25,
                     "tt_fl": 40.0, "bt_fl": 120.0, "impl_edge": -1.0, "impl_clear": -1.0})


def ext_packet(version=6, **kw):
    v = dict(EXT_DEFAULTS)
    for k in kw:
        if k not in v:
            raise KeyError(k)
    v.update(kw)
    head = [v[n] for n in EXT_HEAD]
    tail = [v[n] for n in EXT_TAIL]
    tail8 = [v[n] for n in EXT_TAIL8]
    if version == 6:
        return struct.pack(b.EXT_FORMAT, *head, *tail, v["presence"], *tail8)
    if version == 5:
        return struct.pack(b.EXT_FORMAT_V5, *head, *tail, v["presence"], *tail8[:5])
    if version == 4:
        return struct.pack(b.EXT_FORMAT_V4, *head, *tail, v["presence"], *tail8[:2])
    if version == 3:
        return struct.pack(b.EXT_FORMAT_V3, *head, *tail, v["presence"])
    if version == 2:
        return struct.pack(b.EXT_FORMAT_V2, *head, *tail[:36])
    if version == 1:
        return struct.pack(b.EXT_FORMAT_V1, *head, *tail[:28])
    raise ValueError(version)


OG_FIELDS = ["time", "car", "flags", "gear", "plid", "speed", "rpm", "turbo", "engtemp",
             "fuel", "oil_pressure", "oiltemp", "dashlights", "lights", "throttle", "brake",
             "clutch", "display1", "display2", "id"]
OG_DEFAULTS = {
    "time": 1234, "car": b"pick", "flags": 0, "gear": 1, "plid": 0, "speed": 0.0,
    "rpm": 800.0, "turbo": 0.0, "engtemp": 90.0, "fuel": 0.5, "oil_pressure": 3.0,
    "oiltemp": 95.0, "dashlights": 0, "lights": 0, "throttle": 0.0, "brake": 0.0,
    "clutch": 0.0, "display1": b"", "display2": b"", "id": 0,
}


def og_packet(extra=b"", **kw):
    v = dict(OG_DEFAULTS)
    for k in kw:
        if k not in v:
            raise KeyError(k)
    v.update(kw)
    return struct.pack(b.OG_FORMAT, *[v[n] for n in OG_FIELDS]) + extra


def ms_packet(pos=(0.0, 0.0, 0.0), vel=(0.0, 0.0, 0.0), up_z=1.0, roll=0.0, pitch=0.0,
              yaw=0.0, extra=b"", magic=b"BNG1"):
    f = [0.0] * 21
    f[0], f[1], f[2] = pos
    f[3], f[4], f[5] = vel
    f[6], f[7], f[8] = 0.25, -0.5, 9.81  # accelerometer: unused by the loop
    f[9], f[10] = 0.0, 0.0
    f[11] = up_z
    f[12], f[13], f[14] = roll, pitch, yaw
    f[15:21] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
    return struct.pack(b.MS_FORMAT, magic, *f) + extra


# ---------------------------------------------------------------------------------------
#  Decode cases: the exact expressions telemetry_loop evaluates
# ---------------------------------------------------------------------------------------
def loop_unpack(data, mode):
    """telemetry_loop's packet classification, expression for expression."""
    if len(data) >= 4 and data[:4] == b.MS_MAGIC:
        if len(data) >= b.MS_SIZE:
            return "motionsim", struct.unpack(b.MS_FORMAT, data[:b.MS_SIZE])
        return "ignored", None
    if mode == "extended" and len(data) == b.EXT_SIZE:
        return "ext6", struct.unpack(b.EXT_FORMAT, data)
    if mode == "extended" and len(data) == b.EXT_SIZE_V5:
        return "ext5", struct.unpack(b.EXT_FORMAT_V5, data) + b.EXT_V5_DISTANCE_DEFAULTS
    if mode == "extended" and len(data) == b.EXT_SIZE_V4:
        return "ext4", (struct.unpack(b.EXT_FORMAT_V4, data) + b.EXT_V4_ELECTRIC_DEFAULTS
                        + b.EXT_V5_DISTANCE_DEFAULTS)
    if mode == "extended" and len(data) == b.EXT_SIZE_V3:
        return "ext3", (struct.unpack(b.EXT_FORMAT_V3, data) + b.EXT_V3_SLIP_DEFAULTS
                        + b.EXT_V4_ELECTRIC_DEFAULTS + b.EXT_V5_DISTANCE_DEFAULTS)
    if mode == "extended" and len(data) == b.EXT_SIZE_V2:
        return "ext2", (struct.unpack(b.EXT_FORMAT_V2, data) + b.EXT_V2_WHEEL_DEFAULTS
                        + b.EXT_V3_SLIP_DEFAULTS + b.EXT_V4_ELECTRIC_DEFAULTS
                        + b.EXT_V5_DISTANCE_DEFAULTS)
    if mode == "extended" and len(data) == b.EXT_SIZE_V1:
        return "ext1", struct.unpack(b.EXT_FORMAT_V1, data) + (
            b.EXT_V1_IMPLEMENT_DEFAULTS + b.EXT_V2_WHEEL_DEFAULTS + b.EXT_V3_SLIP_DEFAULTS
            + b.EXT_V4_ELECTRIC_DEFAULTS + b.EXT_V5_DISTANCE_DEFAULTS)
    if mode == "outgauge" and len(data) >= b.OG_SIZE:
        return "outgauge", struct.unpack(b.OG_FORMAT, data[:b.OG_SIZE])
    return "ignored", None


ODD = dict(speed=-0.0, rpm=INF, turbo=NAN, fuel=1e-45, engtemp=-INF, oiltemp=0.1,
           throttle=3.4e38, brake=-1.5, gear=b"\xff3\x00\x00", flags=65535, byte2=255,
           uint12=4294967295, lights=0xFFFFFFFF, presence=4294967295, wmax=7.25, wmin=0.5,
           power=-12.5, torque=300.0, range=12345.0, trip=10.0, odo=110.5, life=1e9,
           lightbar=2.0, fog=1.0, impl_flags=7.0, artic=-3.5, tp_f=30.0, bt_r=99.9)

decode_packets = []
for ver in (6, 5, 4, 3, 2, 1):
    decode_packets.append((f"ext_v{ver}_default", ext_packet(ver)))
    decode_packets.append((f"ext_v{ver}_odd", ext_packet(ver, **ODD)))
decode_packets += [
    ("og_default", og_packet()),
    ("og_odd", og_packet(speed=NAN, rpm=-0.0, gear=0, flags=65535, dashlights=0xFFFFFFFF,
                         lights=0x0FFF, throttle=INF, car=b"\xff\xfe\x00x", id=-7,
                         display1=b"abc", display2=b"\x00" * 16)),
    ("og_extra", og_packet(extra=b"\x01\x02\x03\x04", gear=3, speed=12.5)),
    ("og_short", og_packet()[:-1]),
    ("ms_default", ms_packet()),
    ("ms_odd", ms_packet(pos=(1e6, -2.5, NAN), vel=(3.0, 4.0, -0.0), up_z=-0.9, yaw=-INF)),
    ("ms_extra", ms_packet(extra=b"tail", yaw=1.0)),
    ("ms_short", ms_packet()[:-1]),
    ("ms_magic_only", b"BNG1"),
    ("ms_bad_magic", ms_packet(magic=b"BNG2")),
    ("empty", b""),
    ("three_bytes", b"BNG"),
    ("len_100", bytes(range(100))),
    ("len_255", bytes(255)),
    ("len_257", bytes(257)),
]

decode_cases = []
for name, data in decode_packets:
    for mode in ("extended", "outgauge", "other"):
        kind, tup = loop_unpack(data, mode)
        decode_cases.append({
            "name": name,
            "mode": mode,
            "data": data.hex(),
            "kind": kind,
            "unpacked": None if tup is None else [enc(x) for x in tup],
        })


# ---------------------------------------------------------------------------------------
#  Driving the real telemetry_loop
# ---------------------------------------------------------------------------------------
SNAP = [
    "last_speed_ms", "last_rpm", "last_fuel", "last_turbo", "last_engtemp", "last_oiltemp",
    "last_oil_pressure", "last_rpm_max", "last_turbo_max", "last_protocol_flags",
    "last_throttle", "last_brake", "last_clutch", "last_steering", "last_actual_steering",
    "last_steering_input", "last_air_pressure", "last_air_pressure_max", "last_clutch_temp",
    "last_g_lat", "last_g_lon",
    "last_tire_pressure_fl", "last_tire_pressure_fr", "last_tire_pressure_rl",
    "last_tire_pressure_rr", "last_tire_temp_fl", "last_tire_temp_fr", "last_tire_temp_rl",
    "last_tire_temp_rr", "last_brake_temp_fl", "last_brake_temp_fr", "last_brake_temp_rl",
    "last_brake_temp_rr", "last_tire_pressure_f", "last_tire_pressure_r", "last_tire_temp_f",
    "last_tire_temp_r", "last_brake_temp_f", "last_brake_temp_r", "last_telemetry_presence",
    "last_motor_power_kw", "last_motor_torque_nm", "last_ev_range_m", "last_trip_m",
    "last_odometer_m", "last_lifetime_m",
    "last_signal_left_input", "last_signal_right_input", "last_hazard_enabled",
    "last_lightbar", "last_fog", "last_gear_byte", "last_gear_str", "last_bucket",
    "last_speed_announce_ts", "_last_telemetry_ts",
    "drift_alert_active", "drift_pan_direction", "drift_rate_val",
    "_ls_prev_speed_ms", "_ls_prev_speed_ts", "_ls_steady_start_ts", "last_ground_speed_ms",
    "_ms_last_rx_ts", "_ls_accel_smooth", "_ls_ms_prev_speed_ms", "_ls_ms_prev_ts",
    "_ls_stopped", "_ls_was_moving", "_ls_diag_last_ts", "last_vel_x", "last_vel_y",
    "coord_target_bearing", "_last_coord_bearing_ts", "_ext_version_warned",
    "last_slip_active", "last_slip_kind", "last_slip_mag", "last_tc_active",
    "last_wheel_max_driven", "last_wheel_min",
    "last_implement_flags", "last_implement_edge_height", "last_implement_min_clearance",
    "last_implement_tilt_deg", "last_implement_tilt_percent", "last_implement_lift",
    "last_implement_activity", "last_articulation_deg",
    "last_pos_x", "last_pos_y", "last_pos_z", "last_yaw_rad", "last_roll_rad",
    "last_pitch_rad", "last_up_z", "last_heading", "last_click_heading_deg",
    "compass_click_counter", "last_announced_compass_idx", "last_compass_ts", "inverted",
    "inverted_announced",
]

# Every global a scenario may set, plus everything the loop writes: restored to its
# import-time value before each scenario.
SETTABLE = [
    "announce_turn_signals", "announce_speed", "speed_announce_interval", "oil_chime_enabled",
    "compass_highlight_enabled", "compass_highlight_nth_click", "compass_click_interval_deg",
    "compass_min_interval", "cooldown_sec", "cannon_active", "drift_mode_active",
    "low_speed_mode_active", "slip_mode_active", "pedal_tones_active",
    "heading_guidance_active", "heading_guidance_target", "coord_guidance_active",
    "marked_coord_x", "marked_coord_y", "route_beacon_active", "route_dest_x", "route_dest_y",
    "route_beacon_stamp", "UNITS_MODE", "protocol_mode", "gear_report_settings",
    "last_trailer_id", "_telemetry_baseline_pending", "slip_detector", "last_slip_result",
]
# drift_rate_val exists only once telemetry_loop has run (its first statement sets it).
INITIAL = {name: copy.deepcopy(getattr(b, name)) for name in SNAP + SETTABLE
           if name != "slip_detector" and hasattr(b, name)}

real_say, real_drift, real_push_gear, real_push_obstacle = (
    b.say, b.DRIFT_ANNOUNCER, b._push_gear_direction, b._push_obstacle_state)
real_time, real_socket_mod, real_debug, real_ac_ref = (
    b.time, b.socket, b.bnvda_debug_enabled, b.audio_controller_ref)


class Runner:
    def __init__(self):
        self.steps = []
        self.idx = 0
        self.served = None
        self.calls = []
        self.out = []
        self.now = 0.0
        self.mono = 0.0
        self.quiet = False
        self.debug = False


R = Runner()


class FakeStop:
    """``stop_event``: its ``is_set`` runs at the top of every loop pass, BEFORE
    ``now = time.time()``, which is the one place to close a step and open the next."""

    def is_set(self):
        if R.served is not None:
            finish_step(None)
        if R.idx >= len(R.steps):
            return True
        begin_step(R.steps[R.idx])
        R.idx += 1
        return False


class FakeSock:
    def bind(self, addr):
        pass

    def settimeout(self, t):
        pass

    def recvfrom(self, n):
        return R.served["data"], ("127.0.0.1", 50000)

    def close(self):
        pass


class Recorder:
    def trigger_compass_highlight(self, heading, mult):
        R.calls.append(["highlight", enc(heading), enc(mult)])

    def trigger_compass_click(self, heading, mult):
        R.calls.append(["click", enc(heading), enc(mult)])

    def trigger_lowspeed_stopped(self):
        R.calls.append(["lowspeed_stopped"])

    def update_route_beacon(self, available, bearing_deg=0.0, distance_m=0.0):
        R.calls.append(["route_beacon", bool(available), enc(bearing_deg), enc(distance_m)])

    def update_telemetry_state(self, state):
        d = {}
        for k, v in state.items():
            # audio.py applies int() to these two on the way in.
            if k in ("implement_flags", "slip_kind"):
                v = int(v)
            d[k] = enc(v)
        R.calls.append(["telemetry_state", d])

    def trigger_oil_chime(self):
        R.calls.append(["oil_chime"])

    def trigger_gear_tone(self, degree, sport):
        R.calls.append(["gear_tone", degree, bool(sport)])


class LogTap(logging.Handler):
    def emit(self, record):
        if record.name == b.logger.name and R.served is not None:
            R.calls.append(["log", record.levelname, record.getMessage()])


def rec_say(text, interrupt=True, exclude_from_buffer=False, source="python"):
    R.calls.append(["say", text, bool(interrupt), bool(exclude_from_buffer)])


def apply_set(key, value):
    if key in ("gear_mode", "collapse_sport", "number_style"):
        g = dict(b.gear_report_settings)
        g[{"gear_mode": "mode"}.get(key, key)] = value
        b.gear_report_settings = g
    else:
        if key not in SETTABLE:
            raise KeyError(key)
        setattr(b, key, value)


def begin_step(step):
    R.served = step
    R.now = step["now"]
    R.mono = step["mono"]
    R.quiet = step.get("quiet", False)
    R.debug = step.get("debug", False)
    for k, v in step.get("set", {}).items():
        apply_set(k, v)
    if step.get("baseline"):
        b._telemetry_baseline_pending = True


def finish_step(raised):
    step = R.served
    R.served = None
    snap = {name: enc(getattr(b, name)) for name in SNAP}
    R.out.append({
        "now": fbits(step["now"]),
        "mono": fbits(step["mono"]),
        "data": step["data"].hex(),
        "set": {k: enc(v) for k, v in step.get("set", {}).items()},
        "quiet": step.get("quiet", False),
        "debug": step.get("debug", False),
        "baseline": step.get("baseline", False),
        "calls": R.calls,
        "snap": snap,
        "raised": raised,
    })
    R.calls = []


def run_scenario(name, protocol, steps, initial_set=None):
    for k, v in INITIAL.items():
        setattr(b, k, copy.deepcopy(v))
    b.slip_detector = b.wheel_slip.SlipDetector()
    b.protocol_mode = protocol
    initial_set = initial_set or {}
    for k, v in initial_set.items():
        apply_set(k, v)
    R.steps, R.idx, R.served, R.calls, R.out = steps, 0, None, [], []
    raised = None
    try:
        b.telemetry_loop(Recorder(), stop_event=FakeStop())
    except Exception as exc:  # noqa: BLE001 -- recorded, as the engine would see it
        raised = type(exc).__name__
        finish_step(raised)
    out_steps = R.out
    assert len(out_steps) == (len(steps) if raised is None else len(out_steps)), name
    return {
        "name": name,
        "protocol": protocol,
        "initial_set": {k: enc(v) for k, v in initial_set.items()},
        "steps": out_steps,
        "raised": raised,
    }


def install():
    b.say = rec_say
    b.DRIFT_ANNOUNCER = types.SimpleNamespace(quiet=lambda: R.quiet)
    b._push_gear_direction = lambda gear: R.calls.append(["push_gear", gear])
    b._push_obstacle_state = lambda force=False, gear=None: R.calls.append(["push_obstacle"])
    b.time = types.SimpleNamespace(time=lambda: R.now, monotonic=lambda: R.mono,
                                   sleep=lambda s: None)
    b.socket = types.SimpleNamespace(socket=lambda *a, **k: FakeSock(),
                                     AF_INET=real_socket.AF_INET,
                                     SOCK_DGRAM=real_socket.SOCK_DGRAM,
                                     timeout=real_socket.timeout)
    b.bnvda_debug_enabled = lambda: R.debug
    b.audio_controller_ref = Recorder()
    b.logger.setLevel(logging.DEBUG)
    b.logger.addHandler(LogTap(level=logging.DEBUG))


install()

T0 = 1_700_000_000.0
M0 = 5_000.0


class Steps:
    """A step list on a clock: ``add(data, dt=..., **opts)``."""

    def __init__(self, t=T0, m=M0):
        self.t, self.m, self.items = t, m, []

    def add(self, data, dt=0.05, dm=None, **opts):
        self.t += dt
        self.m += dt if dm is None else dm
        self.items.append({"now": self.t, "mono": self.m, "data": data, **opts})
        return self


# ---------------------------------------------------------------------------------------
#  Scenarios
# ---------------------------------------------------------------------------------------
scenarios = []

# 1. Extended v6: gear, speed, signals, lightbar, fog, headlights, oil chime, clamps.
s = Steps()
P = 15 | 128
s.add(ext_packet(gear=b"N"), baseline=True)
s.add(ext_packet(gear=b"1", speed=3.0))
s.add(ext_packet(gear=b"2", speed=6.0, throttle=0.5))
s.add(ext_packet(gear=b"2", speed=6.5, sig_l=1.0))
s.add(ext_packet(gear=b"2", speed=11.5, hazard=1.0))  # bucket change inside cooldown
s.add(ext_packet(gear=b"2", speed=11.5, hazard=0.0), dt=1.2)
s.add(ext_packet(gear=b"3", speed=11.6, sig_r=0.75))
s.add(ext_packet(gear=b"3", speed=11.6, sig_r=0.5))
s.add(ext_packet(gear=b"3", lightbar=1.0))
s.add(ext_packet(gear=b"3", lightbar=2.0))
s.add(ext_packet(gear=b"3", lightbar=3.0))
s.add(ext_packet(gear=b"3", lightbar=0.9))
s.add(ext_packet(gear=b"3", fog=1.0))
s.add(ext_packet(gear=b"3", fog=-0.5))
s.add(ext_packet(gear=b"3", lights=b.DL_LOWBEAM))
s.add(ext_packet(gear=b"3", lights=b.DL_FULLBEAM))
s.add(ext_packet(gear=b"3", lights=b.DL_FULLBEAM | b.DL_LOWBEAM))
s.add(ext_packet(gear=b"3", lights=0))
s.add(ext_packet(gear=b"3", lights=b.DL_OILWARN))
s.add(ext_packet(gear=b"3", lights=b.DL_OILWARN))
s.add(ext_packet(gear=b"3", lights=b.DL_OILWARN), dt=3.0)
s.add(ext_packet(gear=b"3", lights=b.DL_SHIFT | b.DL_TC | b.DL_SIGNAL_L | b.DL_ABS))
s.add(ext_packet(gear=b"R", speed=-2.0))
s.add(ext_packet(gear=b"D", throttle=1.5, brake=-0.3, clutch=NAN))
s.add(ext_packet(gear=b"P", air=0.0, air_max=120.0, presence=P))
s.add(ext_packet(gear=b"S1", speed=20.0), set={"collapse_sport": True})
s.add(ext_packet(gear=b"S2", speed=20.0))
s.add(ext_packet(gear=b"M3", speed=20.0))
s.add(ext_packet(gear=b"\xff4\x00\x00", speed=20.0))
s.add(ext_packet(gear=b"\x00\x005\x00", speed=30.0), set={"UNITS_MODE": "metric"}, dt=1.5)
s.add(ext_packet(gear=b"0", speed=30.0))
s.add(ext_packet(gear=b" n ", speed=30.0, presence=P | b.TELEMETRY_HAS_AIR, air=100.0,
                 air_max=116.0, power=55.5, torque=210.0, range=99000.0, trip=1234.5,
                 odo=99999.0, life=5e6, wmax=12.0, wmin=11.5, flags=b.OG_TURBO,
                 impl_flags=3.0, impl_edge=0.4, impl_clear=0.2, impl_tilt=5.0,
                 impl_tilt_pct=8.7, impl_lift=1.25, impl_activity=0.5, artic=-12.0,
                 clutch_temp=88.0, g_lat=0.3, g_lon=-0.2, steering=0.1,
                 actual_steering=0.2, steering_input=0.3, tp_f=30.0, tp_r=31.0,
                 tt_f=50.0, tt_r=51.0, bt_f=200.0, bt_r=201.0))
scenarios.append(run_scenario("extended_v6_drive", "extended", s.items))

# 2. Each older layout on its own (one warning per scenario), and V3 then V2 (one warning).
for ver in (5, 4, 3, 2, 1):
    s = Steps()
    s.add(ext_packet(ver, gear=b"1", speed=5.0, air=0.0, air_max=0.0, presence=15,
                     wmax=9.0, power=1.0, trip=2.0))
    s.add(ext_packet(ver, gear=b"1", speed=5.0, air=2.0, air_max=0.0, presence=15 | 64))
    s.add(ext_packet(ver, gear=b"1", speed=5.0, air=0.0, air_max=-1.0, presence=0))
    s.add(ext_packet(ver, gear=b"1", speed=5.0, air=-1.0, air_max=3.0, presence=256))
    scenarios.append(run_scenario(f"extended_v{ver}_layout", "extended", s.items))
s = Steps()
s.add(ext_packet(3, gear=b"2"))
s.add(ext_packet(2, gear=b"3"))
s.add(ext_packet(6, gear=b"4"))
s.add(ext_packet(1, gear=b"5"))
scenarios.append(run_scenario("extended_mixed_layouts_warn_once", "extended", s.items))

# 3. Packets the extended decoder ignores (but which still stamp _last_telemetry_ts).
s = Steps()
s.add(bytes(range(100)))
s.add(og_packet())
s.add(ms_packet()[:50])
s.add(b"BNG1")
s.add(b"")
s.add(bytes(255))
s.add(ext_packet(gear=b"N")[:-1])
s.add(ext_packet(gear=b"N") + b"\x00")
s.add(ext_packet(gear=b"N"))
scenarios.append(run_scenario("extended_ignored", "extended", s.items))

# 4. OutGauge: gear bytes, reverse, extra bytes, a 256-byte packet, a short one, audio mode.
s = Steps()
s.add(og_packet(gear=1))
s.add(og_packet(gear=2, speed=4.0, throttle=0.4))
s.add(og_packet(gear=3, speed=12.0, lights=b.DL_LOWBEAM | b.DL_SHIFT, flags=b.OG_TURBO))
s.add(og_packet(gear=0, speed=1.0, lights=b.DL_LOWBEAM))
s.add(og_packet(gear=1, extra=b"\x01\x02\x03\x04"))
s.add(ext_packet(gear=b"2"))
s.add(og_packet()[:-1])
s.add(og_packet(gear=4, throttle=-1.0, brake=2.0, clutch=0.25), set={"gear_mode": "audio"})
s.add(og_packet(gear=0))
s.add(og_packet(gear=200))
s.add(og_packet(gear=1, lights=b.DL_OILWARN))
s.add(og_packet(gear=1, lights=b.DL_OILWARN), set={"oil_chime_enabled": False}, dt=4.0)
s.add(og_packet(gear=1, lights=b.DL_TC))
s.add(og_packet(gear=2), set={"gear_mode": "off"})
scenarios.append(run_scenario("outgauge_drive", "outgauge", s.items))

# 5. MotionSim compass clicks, highlights, names, inversion, ground speed and accel.
s = Steps()
for k in range(14):
    s.add(ms_packet(yaw=math.radians(16.0 * k), vel=(3.0, 4.0 + 0.1 * k, 9.0)))
s.add(ms_packet(yaw=math.radians(16.0 * 14), up_z=-0.7), dt=0.3)
s.add(ms_packet(yaw=math.radians(16.0 * 14), up_z=-0.5))
s.add(ms_packet(yaw=math.radians(16.0 * 15), up_z=-0.61), quiet=True)
s.add(ms_packet(yaw=-math.radians(45.0), vel=(1.0, 1.0, 0.0)), dt=2.0)
s.add(ms_packet(yaw=-math.radians(90.0)), set={"compass_highlight_enabled": False})
s.add(ms_packet(yaw=math.radians(180.0)), set={"compass_click_interval_deg": 30.0})
s.add(ms_packet(yaw=math.radians(225.0), extra=b"xyz"),
      set={"compass_highlight_nth_click": 1, "compass_highlight_enabled": True})
s.add(ms_packet(yaw=NAN))
s.add(ms_packet(yaw=math.radians(7.5)), set={"compass_click_interval_deg": 7.5})
s.add(ms_packet(yaw=math.radians(22.5)))
s.add(ms_packet(yaw=math.radians(37.5)))
scenarios.append(run_scenario("motionsim_compass", "extended", s.items))

# 6. Drift: yaw-rate filter on MotionSim, the alert gate on telemetry packets.
s = Steps()
s.add(ext_packet(gear=b"2", speed=10.0), set={"drift_mode_active": True})
yaw = 0.0
for k in range(12):
    yaw += math.radians(0.4 if k < 8 else -0.03)
    s.add(ms_packet(yaw=yaw, vel=(0.0, 10.0, 0.0)), dt=0.1)
    s.add(ext_packet(gear=b"2", speed=10.0, steering=1.0 if k == 3 else 0.0), dt=0.002)
s.add(ms_packet(yaw=yaw), dt=0.002)
for k in range(6):
    s.add(ext_packet(gear=b"2", speed=10.0), dt=0.12)
s.add(ms_packet(yaw=yaw + 0.1), set={"drift_mode_active": False})
s.add(ext_packet(gear=b"2", speed=10.0))
s.add(ms_packet(yaw=yaw + 0.2), set={"drift_mode_active": True})
s.add(ms_packet(yaw=yaw - 3.0), dt=0.003)
s.add(ms_packet(yaw=yaw - 3.0), dt=0.2)
s.add(ext_packet(gear=b"2", speed=10.0, steering=-6.0))
scenarios.append(run_scenario("drift", "extended", s.items))

# 7. Low speed clicks, standstill tone, steady suppression, gear gates, debug log, slip.
s = Steps()
on = {"low_speed_mode_active": True, "slip_mode_active": True}
speeds = [5.0, 5.0, 4.0, 3.0, 2.0, 1.0, 0.6, 0.3, 0.1, 0.0, 0.0, 0.6, 0.6, 0.6]
for k, v in enumerate(speeds):
    s.add(ms_packet(vel=(0.0, v, 0.0), yaw=0.0), dt=0.06, debug=(k % 3 == 0))
    s.add(ext_packet(gear=b"D", speed=v, wmax=v * 1.3, wmin=v * 0.9),
          dt=0.01, debug=True, set=on if k == 0 else {})
for k in range(30):
    s.add(ms_packet(vel=(0.0, 4.0, 0.0), yaw=0.0), dt=0.06)
    s.add(ext_packet(gear=b"N" if k == 10 else (b"R" if k > 20 else b"2"), speed=4.0,
                     wmax=6.0 if k < 15 else 4.0, wmin=1.0 if k < 15 else 4.0),
          dt=0.01, debug=(k % 7 == 0))
# MotionSim goes stale: the wheel-speed fallback with its own accel EMA.
for k, v in enumerate([4.0, 3.5, 3.0, 2.0, 0.1, 0.0, 0.0]):
    s.add(ext_packet(gear=b"2", speed=v), dt=0.4 if k == 0 else 0.06, debug=True)
s.add(ext_packet(gear=b"2", speed=2.0), dt=1.5, debug=True)
s.add(ext_packet(gear=b"P", speed=2.0), debug=True)
scenarios.append(run_scenario("lowspeed_and_slip", "extended", s.items))

# 8. Low speed on OutGauge (neutral is gear byte 1).
s = Steps()
for k, gb in enumerate([2, 2, 1, 1, 0, 0]):
    s.add(og_packet(gear=gb, speed=3.0 - 0.4 * k), dt=0.07, debug=True,
          set={"low_speed_mode_active": True, "slip_mode_active": True} if k == 0 else {})
s.add(ms_packet(vel=(0.0, 2.0, 0.0)), dt=0.07)
s.add(og_packet(gear=0, speed=2.0), dt=0.07)
s.add(og_packet(gear=2, speed=2.0), dt=0.07)
scenarios.append(run_scenario("lowspeed_outgauge", "outgauge", s.items))

# 9. Heading, waypoint and route guidance.
s = Steps()
s.add(ms_packet(pos=(10.0, 20.0, 0.0), yaw=math.radians(100.0)),
      set={"heading_guidance_active": True, "heading_guidance_target": 300.0})
s.add(ext_packet())
s.add(ext_packet(), set={"heading_guidance_target": -100.0})
s.add(ext_packet(), set={"heading_guidance_target": 100.0})
s.add(ext_packet(), set={"coord_guidance_active": True})
s.add(ext_packet(), set={"marked_coord_x": 110.0, "marked_coord_y": -30.0})
s.add(ms_packet(pos=(60.0, 20.0, 0.0), yaw=math.radians(10.0)))
s.add(ext_packet())
s.add(ext_packet(), dt=1.0)
s.add(ext_packet(), set={"marked_coord_x": 60.2, "marked_coord_y": 20.2}, dt=1.0)
s.add(ext_packet(), set={"route_beacon_active": True})
s.add(ext_packet(), set={"route_dest_x": 500.0, "route_dest_y": 400.0,
                         "route_beacon_stamp": M0 + 1.0})
s.add(ext_packet(), set={"route_dest_x": 70.0, "route_dest_y": 25.0})
s.add(ext_packet(), dm=2.0)
s.add(ext_packet(), set={"route_beacon_active": False})
scenarios.append(run_scenario("guidance", "extended", s.items))

# 10. Suppression: cannon, drift quiet, baseline, the announce toggles, pedal tones.
s = Steps()
s.add(ext_packet(gear=b"1", speed=0.0))
s.add(ext_packet(gear=b"2", speed=0.0), set={"cannon_active": True})
s.add(ext_packet(gear=b"80%", speed=0.0))
s.add(ext_packet(gear=b"3", speed=15.0), set={"cannon_active": False}, quiet=True)
s.add(ext_packet(gear=b"4", speed=25.0, sig_l=1.0, lightbar=1.0, fog=1.0,
                 lights=b.DL_LOWBEAM), baseline=True, dt=2.0)
s.add(ext_packet(gear=b"5", speed=35.0, sig_l=0.0),
      set={"announce_speed": False, "announce_turn_signals": False}, dt=2.0)
s.add(ext_packet(gear=b"5", speed=45.0, sig_r=1.0),
      set={"announce_speed": True, "speed_announce_interval": 0, "cooldown_sec": 0.0,
           "pedal_tones_active": True}, dt=2.0)
s.add(ext_packet(gear=b"5", speed=45.5), set={"speed_announce_interval": 7.5})
s.add(ext_packet(gear=b"S", speed=46.0), set={"gear_mode": "audio"}, quiet=True)
s.add(ext_packet(gear=b"S3", speed=46.0))
s.add(ext_packet(gear=b"S4", speed=46.0), set={"collapse_sport": True})
s.add(ext_packet(gear=b"N", speed=46.0), set={"number_style": "number", "gear_mode": "speech"})
s.add(ext_packet(gear=b"6", speed=46.0))
s.add(ext_packet(gear=b"x", speed=46.0))
s.add(ext_packet(gear=b"", speed=46.0))
s.add(ext_packet(gear=b"-1", speed=46.0))
scenarios.append(run_scenario("suppression", "extended", s.items))

# 11. Uncaught exceptions end the loop exactly where Python's do.
s = Steps()
s.add(ext_packet(gear=b"1", sig_l=1.0))
s.add(ext_packet(gear=b"1", lightbar=NAN))
s.add(ext_packet(gear=b"1"))
scenarios.append(run_scenario("raises_nan_lightbar", "extended", s.items))
s = Steps()
s.add(og_packet(gear=2, speed=1.0))
s.add(og_packet(gear=2, speed=NAN))
scenarios.append(run_scenario("raises_nan_speed", "outgauge", s.items))
s = Steps()
s.add(ext_packet(gear=b"1", fog=INF))
scenarios.append(run_scenario("raises_inf_fog", "extended", s.items))
s = Steps()
s.add(ext_packet(gear=b"1", impl_flags=NAN))
scenarios.append(run_scenario("raises_nan_implement_flags", "extended", s.items))
s = Steps()
s.add(ms_packet(yaw=1.0), set={"compass_click_interval_deg": 0.0})
scenarios.append(run_scenario("raises_zero_click_interval", "extended", s.items))

# 12. Protocol mode "other": only MotionSim is decoded.
s = Steps()
s.add(ext_packet(gear=b"1"))
s.add(og_packet(gear=2))
s.add(ms_packet(yaw=1.0))
scenarios.append(run_scenario("protocol_other", "other", s.items))

for sc in scenarios:
    assert sc["steps"], sc["name"]


# Restore the module for anything that imports this generator afterwards.
def uninstall():
    b.say, b.DRIFT_ANNOUNCER = real_say, real_drift
    b._push_gear_direction, b._push_obstacle_state = real_push_gear, real_push_obstacle
    b.time, b.socket, b.bnvda_debug_enabled = real_time, real_socket_mod, real_debug
    b.audio_controller_ref = real_ac_ref


write_fixture("telemetry", {
    "consts": {
        "OG_FORMAT": b.OG_FORMAT, "OG_SIZE": b.OG_SIZE, "OG_TURBO": b.OG_TURBO,
        "EXT_FORMAT_V1": b.EXT_FORMAT_V1, "EXT_SIZE_V1": b.EXT_SIZE_V1,
        "EXT_FORMAT_V2": b.EXT_FORMAT_V2, "EXT_SIZE_V2": b.EXT_SIZE_V2,
        "EXT_FORMAT_V3": b.EXT_FORMAT_V3, "EXT_SIZE_V3": b.EXT_SIZE_V3,
        "EXT_FORMAT_V4": b.EXT_FORMAT_V4, "EXT_SIZE_V4": b.EXT_SIZE_V4,
        "EXT_FORMAT_V5": b.EXT_FORMAT_V5, "EXT_SIZE_V5": b.EXT_SIZE_V5,
        "EXT_FORMAT": b.EXT_FORMAT, "EXT_SIZE": b.EXT_SIZE,
        "MS_FORMAT": b.MS_FORMAT, "MS_SIZE": b.MS_SIZE, "MS_MAGIC": b.MS_MAGIC.hex(),
        "EXT_V3_SLIP_DEFAULTS": [enc(x) for x in b.EXT_V3_SLIP_DEFAULTS],
        "EXT_V4_ELECTRIC_DEFAULTS": [enc(x) for x in b.EXT_V4_ELECTRIC_DEFAULTS],
        "EXT_V5_DISTANCE_DEFAULTS": [enc(x) for x in b.EXT_V5_DISTANCE_DEFAULTS],
        "EXT_V1_IMPLEMENT_DEFAULTS": [enc(x) for x in b.EXT_V1_IMPLEMENT_DEFAULTS],
        "EXT_V2_WHEEL_DEFAULTS": [enc(x) for x in b.EXT_V2_WHEEL_DEFAULTS],
        **{n: getattr(b, n) for n in (
            "WHEEL_POS_FL", "WHEEL_POS_FR", "WHEEL_POS_RL", "WHEEL_POS_RR", "WHEEL_POS_F",
            "WHEEL_POS_R", "WHEEL_POS_CORNERS", "TELEMETRY_HAS_CLUTCH",
            "TELEMETRY_HAS_ENGINE_TEMP", "TELEMETRY_HAS_ELECTRIC", "TELEMETRY_HAS_AIR",
            "DL_SHIFT", "DL_FULLBEAM", "DL_HANDBRAKE", "DL_TC", "DL_SIGNAL_L", "DL_SIGNAL_R",
            "DL_CHECK", "DL_OILWARN", "DL_BATTERY", "DL_ABS", "DL_LOWBEAM")},
    },
    "decode": decode_cases,
    "scenarios": scenarios,
})
uninstall()
