"""Golden cases for E6: the F9 command handler.

Drives the real ``_invoke_f9_command`` / ``_on_next_key_press`` / ``_mcp_press_command``
with every key and modifier combination over a few named states, plus targeted cases for
each branch's internals. Every function the handler calls in another part of beamtel is
replaced by a recorder (name, arguments, and whether ``_command_context`` was set at the
call), as are the sockets, ``threading.Thread``, ``time.sleep`` and the dump log lines.
Speech goes through the real ``say`` into ``SPEECH_TAP`` with ``speech.speak`` silenced.

The 4447 dumps run synchronously against scripted datagrams when a case supplies them.

    uv run python rust/fixtures/gen/f9_commands.py
"""

from __future__ import annotations

import socket as socket_mod
import time as time_mod
from types import SimpleNamespace

from _beamtel import beamtel as b
from _common import deep_fbits, write_fixture

INF = float("inf")
NAN = float("nan")

# ---- State -------------------------------------------------------------------------------

# Every global the handler reads or writes. Values are taken from beamtel at import, so the
# fixture records the Python's own initial state.
STATE_KEYS = [
    "UNITS_MODE",
    "protocol_mode", "last_speed_ms", "last_rpm", "last_fuel", "last_turbo", "last_engtemp",
    "last_oiltemp", "last_gear_byte", "last_gear_str", "last_pos_x", "last_pos_y",
    "last_pos_z", "last_roll_rad", "last_pitch_rad", "last_heading", "last_rpm_max",
    "last_turbo_max", "last_air_pressure", "last_air_pressure_max", "last_telemetry_presence",
    "heading_guidance_active", "heading_guidance_target", "drift_mode_active",
    "drift_alert_active", "drift_baseline_heading", "last_drift_check_ts",
    "low_speed_mode_active", "slip_mode_active", "coord_guidance_active", "pedal_tones_active",
    "marked_coord_x", "marked_coord_y", "_last_coord_bearing_ts",
    "cam_yaw_deg", "cam_pitch_deg", "cam_agl", "cam_agl_valid", "cam_veh_bearing",
    "cam_veh_distance",
    "scan_mode_active", "coupler_dist_mode", "last_scanner_bearing", "last_scanner_distance",
    "last_scanner_target_name", "last_scanner_approach_deg",
    "road_mode_active",
    "dock_mode_active", "cannon_active", "cannon_kind", "last_dock", "last_dock_fail",
    "last_dock_mode", "last_dock_name", "last_ramp_self", "last_cannon_aim",
    "_implement_word_current",
    "speech_logging_active",
]

for _k in STATE_KEYS:
    assert hasattr(b, _k), _k

BASE = {k: getattr(b, k) for k in STATE_KEYS}
assert BASE["last_dock"] is None and BASE["last_cannon_aim"] is None
assert BASE["last_ramp_self"] is None and BASE["marked_coord_x"] is None
assert b.driving_assistant.active is False

# The globals compared after each case. Structured ones are reduced to what the handler
# itself changes; the drift timestamp is a wall-clock time, so only whether it was set.
VIEW_KEYS = [
    "UNITS_MODE", "heading_guidance_active", "heading_guidance_target", "coupler_dist_mode",
    "drift_mode_active", "drift_alert_active", "drift_baseline_heading", "last_drift_check_ts",
    "low_speed_mode_active", "slip_mode_active", "coord_guidance_active", "pedal_tones_active",
    "marked_coord_x", "marked_coord_y", "_last_coord_bearing_ts", "dock_mode_active",
    "last_dock", "last_dock_fail", "last_dock_mode", "last_dock_name",
    "speech_logging_active", "_input_help_mode", "_capture_mods", "_command_context",
]


def view():
    out = {}
    for k in VIEW_KEYS:
        v = getattr(b, k)
        if k == "last_drift_check_ts":
            v = v > 0.0
        elif k == "last_dock":
            v = None if v is None else [v["name"], v["mode"]]
        elif k == "_capture_mods":
            v = [v["ctrl"], v["shift"], v["alt"]]
        out[k] = deep_fbits(v)
    return out


def dock(name="Flatbed", mode="IMPL", kind="GAP", idx=1, count=3, vertical=0.0):
    return {"name": name, "mode": mode, "kind": kind, "idx": idx, "count": count,
            "vertical": vertical}


def aim(**kw):
    a = {"elevation": 12.34, "bearing": 3.21, "target_angle": 15.0, "line_angle": 4.5,
         "range": 120.0, "speed": 80.0, "reachable": True, "solution_available": True,
         "stamp_age": 0.1}
    a.update(kw)
    return a


SCENARIOS = {
    "idle": {},
    "active": {
        "UNITS_MODE": "metric",
        "protocol_mode": "extended",
        "last_speed_ms": 13.37, "last_rpm": 2345.5, "last_fuel": 0.4251,
        "last_turbo": 0.87, "last_engtemp": 92.5, "last_oiltemp": 104.5,
        "last_gear_str": "D", "last_pos_x": 123.456, "last_pos_y": -78.905,
        "last_pos_z": 12.345, "last_roll_rad": 0.0523, "last_pitch_rad": -0.1047,
        "last_heading": 271.25, "last_rpm_max": 6500.5, "last_turbo_max": 1.25,
        "last_air_pressure": 110.37, "last_air_pressure_max": 120.0,
        "last_telemetry_presence": b.WHEEL_POS_CORNERS | b.TELEMETRY_HAS_ENGINE_TEMP
        | b.TELEMETRY_HAS_AIR,
        "marked_coord_x": 200.0, "marked_coord_y": -50.0,
        "cam_yaw_deg": 45.5, "cam_pitch_deg": -12.5, "cam_agl": 10.25,
        "cam_veh_bearing": -33.3, "cam_veh_distance": 25.5,
        "scan_mode_active": True, "last_scanner_bearing": 45.0,
        "last_scanner_distance": 30.0, "last_scanner_target_name": "Pickup",
        "last_scanner_approach_deg": 100.0,
        "road_mode_active": True,
        "dock_mode_active": True, "last_dock": dock(), "last_dock_fail": None,
        "_implement_word_current": "Forks",
        "heading_guidance_active": True, "drift_mode_active": True, "coord_guidance_active": True,
        "low_speed_mode_active": True, "slip_mode_active": True, "pedal_tones_active": True,
    },
}

# ---- Recorders ---------------------------------------------------------------------------

CALLS = []
RETURNS = {}
DATAGRAMS = []

DEFAULT_RETURNS = {
    "_camera_feed_problem": None,
    "ROAD_GUIDANCE_FEED.speed_limit_phrase": "Speed limit unknown",
    "request_vehicle_info": [[], ["timeout", ""]],
    "_read_mod_details_page": False,
    "_world_is_active": False,
    "_dock_phrase": "DOCK PHRASE",
    "_ramp_self_phrase": "DECK PHRASE",
    "_band_name": "Band",
    "threading.Thread": None,
}


def record(name, args):
    CALLS.append([name, deep_fbits(args), b._command_context])
    return RETURNS.get(name)


class FakeAC:
    def set_dock_mode(self, active):
        record("audio_controller.set_dock_mode", [active])


FAKE_AC = FakeAC()


def plain(name):
    def fn(*args, **kwargs):
        assert not args and not kwargs, (name, args, kwargs)
        return record(name, [])
    return fn


def with_ac(name):
    def fn(ac):
        assert ac is FAKE_AC, name
        return record(name, [])
    return fn


def _request_vehicle_info(model=None, config=None, timeout=0.6):
    assert model is None and config is None and timeout == 0.6
    lines, fail = record("request_vehicle_info", [])
    return list(lines), (tuple(fail) if fail is not None else None)


def _open_virtual_browser(lines, title=None, **kw):
    assert not kw
    record("open_virtual_browser", [list(lines), title])


def _open_fuel_stations(force_list=False):
    record("open_fuel_stations", [force_list])


class FakeFeed:
    def speed_limit_phrase(self, enabled, units, label):
        return record("ROAD_GUIDANCE_FEED.speed_limit_phrase", [enabled, units, label])


class FakeSock:
    def __init__(self, *args):
        pass

    def bind(self, addr):
        assert addr[0] == "127.0.0.1"
        record("socket.bind", [addr[1]])

    def sendto(self, payload, addr):
        assert addr[0] == "127.0.0.1"
        record("udp", [addr[1], payload.decode("utf-8", "replace")])

    def settimeout(self, s):
        record("socket.settimeout", [repr(float(s))])

    def recvfrom(self, n):
        assert n == 4096
        if not DATAGRAMS:
            raise socket_mod.timeout()
        return DATAGRAMS.pop(0), ("127.0.0.1", 4448)

    def close(self):
        pass


class FakeThread:
    def __init__(self, target, daemon):
        assert daemon is True
        self.target = target

    def start(self):
        script = record("threading.Thread", [self.target.__name__])
        if script is not None:
            DATAGRAMS[:] = [bytes(d) for d in script]
            self.target()


class FakeLogger:
    def __init__(self, real):
        self.real = real

    def info(self, msg, *args, **kw):
        if isinstance(msg, str) and msg.startswith("=== "):
            record("logger.info", [msg])
        self.real.info(msg, *args, **kw)

    def __getattr__(self, name):
        return getattr(self.real, name)


def install():
    for name in ["open_camera_lineup", "_camera_feed_problem", "send_camera_diag",
                 "speak_road_status", "trigger_nearest_poi", "speak_ambient_temperature",
                 "speak_induction_report", "open_clickspot_browser", "toggle_status_mode",
                 "toggle_buffer_mode", "open_vehicle_bindings_browser",
                 "toggle_binding_learn_mode", "open_environment_browser",
                 "open_audio_debug_browser", "_read_mod_details_page", "_world_is_active",
                 "trigger_terrain_scan_driving_only"]:
        assert callable(getattr(b, name)), name
        setattr(b, name, plain(name))
    for name in ["toggle_free_cam_info", "toggle_road_mode", "toggle_route_beacon",
                 "toggle_clickspot_mode", "toggle_scan_mode", "toggle_obstacle_mode",
                 "toggle_nodegrab_mode"]:
        assert callable(getattr(b, name)), name
        setattr(b, name, with_ac(name))
    b.request_vehicle_info = _request_vehicle_info
    b.open_virtual_browser = _open_virtual_browser
    b.open_fuel_stations = _open_fuel_stations
    b.broadcast = lambda msg: record("broadcast", [msg])
    b._send_implement_cmd = lambda cmd: record("_send_implement_cmd", [cmd])
    b._dock_phrase = lambda snap: record("_dock_phrase", [snap["name"]])
    b._ramp_self_phrase = lambda deck: record("_ramp_self_phrase", [])
    b._band_name = lambda kind, idx, count: record("_band_name", [kind, idx, count])
    b.ROAD_GUIDANCE_FEED = FakeFeed()
    b.socket = SimpleNamespace(socket=FakeSock, AF_INET=socket_mod.AF_INET,
                               SOCK_DGRAM=socket_mod.SOCK_DGRAM, timeout=socket_mod.timeout)
    b.threading = SimpleNamespace(Thread=FakeThread)
    b.time = SimpleNamespace(time=time_mod.time, monotonic=time_mod.monotonic,
                             sleep=lambda s: record("time.sleep", [repr(float(s))]))
    b.logger = FakeLogger(b.logger)
    b.speech.speak = lambda *a, **k: None


# ---- Running a case ----------------------------------------------------------------------


def apply_state(overrides):
    s = dict(BASE)
    s.update(overrides)
    for k, v in s.items():
        if k == "last_cannon_aim" and v is not None:
            v = dict(v)
            v["stamp"] = time_mod.monotonic() - v.pop("stamp_age")
        elif isinstance(v, dict):
            v = dict(v)
        setattr(b, k, v)


def run_case(case):
    apply_state({**SCENARIOS[case.get("scenario", "idle")], **case.get("state", {})})
    b.SPEECH_TAP.clear()
    b.SPEECH_BUFFER.clear()
    b.SPEECH_BUFFER.extend(case.get("buffer", []))
    b._input_help_mode = case.get("help_mode", False)
    b._capture_mods.update({"ctrl": False, "shift": False, "alt": False})
    pre = case.get("preset_mods", [False, False, False])
    b._capture_mods.update({"ctrl": pre[0], "shift": pre[1], "alt": pre[2]})
    b._command_context = case.get("preset_ctx", False)
    b.next_key_timer = None
    b.audio_controller_ref = None if case.get("no_ac") else FAKE_AC
    RETURNS.clear()
    RETURNS.update(DEFAULT_RETURNS)
    RETURNS.update(case.get("returns", {}))
    CALLS.clear()
    DATAGRAMS.clear()
    before = view()
    err = None
    key = case["key"]
    c, s, a = case.get("mods", [False, False, False])
    entry = case.get("entry", "invoke")
    try:
        if entry == "invoke":
            assert b._invoke_f9_command(key, ctrl=c, shift=s, alt=a) is True
        elif entry == "mcp":
            assert b._mcp_press_command(key, ctrl=c, shift=s, alt=a) is True
        else:
            evt = SimpleNamespace(event_type=case.get("event_type", "down"), name=key)
            b._on_next_key_press(evt, FAKE_AC)
    except Exception as exc:  # noqa: BLE001
        err = type(exc).__name__
    after = view()
    case["err"] = err
    case["spoken"] = [
        [e["text"].replace(b.SPEECH_LOG_PATH, "<SPEECH_LOG_PATH>"), e["interrupt"], e["excluded"]]
        for e in b.SPEECH_TAP
    ]
    case["calls"] = [list(c) for c in CALLS]
    case["changes"] = {k: after[k] for k in VIEW_KEYS if after[k] != before[k]}
    case["state"] = deep_fbits(case.get("state", {}))
    return case


# ---- The cases ---------------------------------------------------------------------------

MOD_COMBOS = [[c, s, a] for c in (False, True) for s in (False, True) for a in (False, True)]
SWEEP_KEYS = (
    list("abcdefghijklmnopqrstuvwxyz") + list("0123456789")
    + ["tab", "space", "/", "?", "f9", "engtemp", "enter", "f1", "ctrl", "left shift", "alt", "S"]
)

CASES = []


def case(name, key, **kw):
    CASES.append({"name": name, "key": key, **kw})


for scen in ("idle", "active"):
    for key in SWEEP_KEYS:
        for mods in MOD_COMBOS:
            case(f"sweep/{scen}/{key}/{''.join('1' if m else '0' for m in mods)}", key,
                 scenario=scen, mods=mods)

ALT = [False, False, True]
CTRL = [True, False, False]
SHIFT = [False, True, False]
CS = [True, True, False]
AS = [False, True, True]

# Camera readouts.
for key in "hapvd":
    case(f"camera/{key}/problem", key, mods=ALT, returns={"_camera_feed_problem": "Camera info is off"})
    case(f"camera/{key}/imperial", key, mods=ALT, scenario="active", state={"UNITS_MODE": "imperial"})
case("camera/a/no_ground", "a", mods=ALT, state={"cam_agl_valid": False, "cam_agl": 512.25})
case("camera/a/no_ground_metric", "a", mods=ALT,
     state={"cam_agl_valid": False, "cam_agl": 512.25, "UNITS_MODE": "metric"})
case("camera/p/up", "p", mods=ALT, state={"cam_pitch_deg": 0.0})
case("camera/p/half", "p", mods=ALT, state={"cam_pitch_deg": -2.5})
case("camera/v/left", "v", mods=ALT, state={"cam_veh_bearing": 12.5, "cam_veh_distance": 0.0})
case("camera/v/zero", "v", mods=ALT, state={"cam_veh_bearing": 0.0, "cam_veh_distance": 3.0})
case("camera/v/none", "v", mods=ALT, state={"cam_veh_distance": -1.0})
case("camera/d/none", "d", mods=ALT, state={"cam_veh_distance": -0.5})
case("camera/d/metric", "d", mods=ALT, state={"cam_veh_distance": 2.5, "UNITS_MODE": "metric"})
case("camera/as/diag", "a", mods=AS)

# Speed, RPM, redline.
case("s/limit_empty", "s", returns={"ROAD_GUIDANCE_FEED.speed_limit_phrase": ""})
case("s/road_on", "s", state={"road_mode_active": True, "last_speed_ms": 31.29})
case("s/metric_half", "s", state={"UNITS_MODE": "metric", "last_speed_ms": 0.69444444})
case("r/nan", "r", state={"last_rpm": NAN})
case("r/half_even", "r", state={"last_rpm": 2.5})
case("r/redline_outgauge", "r", mods=SHIFT, state={"last_rpm_max": 7000.0})
case("r/redline_ext", "r", mods=SHIFT, state={"protocol_mode": "extended", "last_rpm_max": 6999.5})
case("r/road_status", "r", mods=CS)
case("r/toggle_road", "r", mods=CTRL)
case("f/nan", "f", state={"last_fuel": NAN})
case("f/over", "f", state={"last_fuel": 1.7})
case("f/under", "f", state={"last_fuel": -0.2})
case("f/half", "f", state={"last_fuel": 0.125})
case("speed/nan", "s", state={"last_speed_ms": NAN})

# Heading guidance, drift, coupler.
case("ctrl_h/on", "h", mods=CTRL, state={"last_heading": 12.5})
case("ctrl_h/off", "h", mods=CTRL, state={"heading_guidance_active": True})
case("ctrl_d/on", "d", mods=CTRL, state={"last_heading": 99.0, "drift_alert_active": True})
case("ctrl_d/off", "d", mods=CTRL, state={"drift_mode_active": True, "drift_alert_active": True})
case("cs_d/scanner_off", "d", mods=CS)
case("cs_d/on", "d", mods=CS, state={"scan_mode_active": True})
case("cs_d/off", "d", mods=CS, state={"scan_mode_active": True, "coupler_dist_mode": True})

# Scanner bearing and distance.
case("shift_d/off", "d", mods=SHIFT)
case("shift_d/inf", "d", mods=SHIFT, state={"scan_mode_active": True})
case("shift_d/left", "d", mods=SHIFT, state={"scan_mode_active": True, "last_scanner_distance": 5.0,
                                              "last_scanner_bearing": 0.0})
case("shift_d/right", "d", mods=SHIFT, state={"scan_mode_active": True, "last_scanner_distance": 5.0,
                                               "last_scanner_bearing": -12.5})
case("d/off", "d")
case("d/inf", "d", state={"scan_mode_active": True})
for brg in (0.0, 29.9, 30.0, -59.9, 60.0, 119.9, -120.0, 149.9, 150.0, 180.0, NAN):
    for ap in (44.9, 45.0, -45.0, 90.0, 135.0, 135.1, -170.0, NAN):
        case(f"d/orient/{brg}/{ap}", "d",
             state={"scan_mode_active": True, "last_scanner_distance": 12.5,
                    "last_scanner_bearing": brg, "last_scanner_approach_deg": ap,
                    "last_scanner_target_name": "" if ap == 90.0 else "Hauler"})
case("d/metric", "d", state={"scan_mode_active": True, "last_scanner_distance": 2.5,
                             "UNITS_MODE": "metric"})

# The cane tap.
case("i/old/none", "i", state={"cannon_kind": "OLD"})
case("i/old/stale", "i", state={"cannon_kind": "OLD", "last_cannon_aim": aim(stamp_age=5.0)})
case("i/old/no_target", "i", state={"cannon_kind": "OLD", "last_cannon_aim": aim(range=-1.0)})
case("i/old/aligned_bearing", "i", state={"cannon_kind": "OLD", "last_cannon_aim": aim(bearing=-0.5)})
case("i/old/right", "i", state={"cannon_kind": "OLD", "last_cannon_aim": aim(bearing=-7.25)})
case("i/old/no_solution", "i", state={"cannon_kind": "OLD",
                                      "last_cannon_aim": aim(solution_available=False)})
case("i/old/unreachable", "i", state={"cannon_kind": "OLD", "last_cannon_aim": aim(reachable=False)})
case("i/old/elev_aligned", "i", state={"cannon_kind": "OLD",
                                       "last_cannon_aim": aim(target_angle=12.84)})
case("i/old/lower", "i", state={"cannon_kind": "OLD", "last_cannon_aim": aim(target_angle=2.0),
                                "UNITS_MODE": "metric"})
case("i/old/in_cannon_too", "i", state={"cannon_kind": "OLD", "cannon_active": True})
case("i/cannon/strength", "i", state={"cannon_active": True, "last_rpm": 455.0, "last_gear_str": " 75% "})
case("i/cannon/unknown", "i", state={"cannon_active": True, "last_rpm": 1500.0, "last_gear_str": "D"})
case("i/cannon/none_gear", "i", state={"cannon_active": True, "last_rpm": -3.0, "last_gear_str": None})
case("i/cannon/nan", "i", state={"cannon_active": True, "last_rpm": NAN, "last_gear_str": "100%"})
case("i/dock_reading", "i", state={"dock_mode_active": True, "last_dock": dock(name="Trailer")})
case("i/dock_reading_over_deck", "i", state={"dock_mode_active": True, "last_dock": dock(),
                                              "last_ramp_self": {"pitch": 3.0, "lip": None, "groups": []}})
case("i/deck", "i", state={"last_ramp_self": {"pitch": 3.0, "lip": 0.5, "groups": []}})
case("i/deck_dock_on", "i", state={"dock_mode_active": True,
                                   "last_ramp_self": {"pitch": None, "lip": None, "groups": []}})
case("i/off", "i")
case("i/off_with_reading", "i", state={"last_dock": dock()})
case("i/why", "i", state={"dock_mode_active": True, "last_dock_fail": "No implement fitted"})
case("i/why_empty", "i", state={"dock_mode_active": True, "last_dock_fail": ""})
case("i/nothing", "i", state={"dock_mode_active": True})

# Reference bands.
case("shift_i/ramp", "i", mods=SHIFT, state={"last_dock": dock(mode="RAMP")})
case("shift_i/no_word", "i", mods=SHIFT)
case("shift_i/nothing", "i", mods=SHIFT, state={"_implement_word_current": "Forks"})
for vert in (0.0, 0.0499, 0.05, -0.05, 0.3, -1.234):
    for units in ("imperial", "metric"):
        case(f"shift_i/{vert}/{units}", "i", mods=SHIFT,
             state={"_implement_word_current": "Bucket", "UNITS_MODE": units,
                    "last_dock": dock(kind="FACE", idx=2, count=4, vertical=vert)})

# The docking instrument toggle.
case("ctrl_i/off", "i", mods=CTRL, state={"dock_mode_active": True, "last_dock": dock(),
                                          "last_dock_fail": "x", "last_dock_mode": "IMPL",
                                          "last_dock_name": "Flatbed"})
case("ctrl_i/on_none", "i", mods=CTRL)
case("ctrl_i/on_why", "i", mods=CTRL, state={"last_dock_fail": "Not a loader"})
case("ctrl_i/on_ramp", "i", mods=CTRL, state={"last_dock": dock(mode="RAMP", name="Hauler")})
case("ctrl_i/on_ramp_noname", "i", mods=CTRL, state={"last_dock": dock(mode="RAMP", name="")})
case("ctrl_i/on_impl_why", "i", mods=CTRL, state={"last_dock": dock(mode="IMPL"),
                                                  "last_dock_fail": "Raise the forks"})
case("ctrl_i/on_other", "i", mods=CTRL, state={"last_dock": dock(mode="SOMETHING")})

# Waypoints.
case("w/none", "w")
case("w/at", "w", state={"marked_coord_x": 0.3, "marked_coord_y": 0.3})
for heading in (0.0, 90.0, 180.0, 271.25):
    for units in ("imperial", "metric"):
        case(f"w/{heading}/{units}", "w", state={"marked_coord_x": -40.0, "marked_coord_y": 25.0,
                                                 "last_pos_x": 1.5, "last_pos_y": -2.5,
                                                 "last_heading": heading, "UNITS_MODE": units})
case("w/route_beacon", "w", mods=CTRL)
case("w/poi", "w", mods=SHIFT)
case("alt_c/none", "c", mods=ALT)
case("alt_c/marked", "c", mods=ALT, state={"marked_coord_x": 1.25, "marked_coord_y": -0.05})
case("shift_c/mark", "c", mods=SHIFT, state={"last_pos_x": 10.25, "last_pos_y": -3.35,
                                              "_last_coord_bearing_ts": 99.0})
case("c/coords", "c", state={"last_pos_x": 10.125, "last_pos_y": -3.335, "last_pos_z": 0.005})
case("ctrl_g/no_waypoint", "g", mods=CTRL)
case("ctrl_g/on", "g", mods=CTRL, state={"marked_coord_x": 1.0, "marked_coord_y": 2.0})
case("ctrl_g/off", "g", mods=CTRL, state={"coord_guidance_active": True})

# Toggles.
case("cs_s/on", "s", mods=CS)
case("cs_s/off", "s", mods=CS, state={"speech_logging_active": True})
case("cs_l/on", "l", mods=CS)
case("cs_l/off", "l", mods=CS, state={"low_speed_mode_active": True})
case("ctrl_k/on", "k", mods=CTRL)
case("ctrl_k/off", "k", mods=CTRL, state={"slip_mode_active": True})
case("ctrl_c/on", "c", mods=CTRL)
case("ctrl_c/off", "c", mods=CTRL, state={"pedal_tones_active": True})

# Readouts.
case("h/heading", "h", state={"last_heading": 12.345})
case("g/outgauge_none", "g")
case("g/outgauge_reverse", "g", state={"last_gear_byte": 0})
case("g/outgauge_third", "g", state={"last_gear_byte": 4})
case("g/extended_none", "g", state={"protocol_mode": "extended"})
case("g/extended_m5", "g", state={"protocol_mode": "extended", "last_gear_str": "M5"})
case("t/outgauge_max", "t", mods=SHIFT, state={"last_turbo_max": 1.5})
case("t/extended_max", "t", mods=SHIFT, state={"protocol_mode": "extended", "last_turbo_max": 1.5})
case("t/extended_max_metric", "t", mods=SHIFT,
     state={"protocol_mode": "extended", "last_turbo_max": 1.255, "UNITS_MODE": "metric"})
case("t/boost", "t", state={"last_turbo": 0.123})
case("t/boost_metric", "t", state={"last_turbo": 0.125, "UNITS_MODE": "metric"})
case("t/ambient", "t", mods=CTRL)
case("t/induction", "t", mods=ALT)
case("p/outgauge", "p")
case("p/no_air", "p", state={"protocol_mode": "extended", "last_telemetry_presence": 0})
case("p/air_flag", "p", state={"protocol_mode": "extended",
                               "last_telemetry_presence": b.TELEMETRY_HAS_AIR})
case("p/pressure_only", "p", state={"protocol_mode": "extended", "last_telemetry_presence": 0,
                                    "last_air_pressure": 55.55, "last_air_pressure_max": 1.0})
case("p/with_max", "p", state={"protocol_mode": "extended", "last_telemetry_presence": 0,
                               "last_air_pressure": 55.55, "last_air_pressure_max": 120.0})
case("p/with_max_metric", "p", state={"protocol_mode": "extended", "last_telemetry_presence": 0,
                                      "last_air_pressure": 55.55, "last_air_pressure_max": 120.0,
                                      "UNITS_MODE": "metric"})
case("u/to_metric", "u")
case("u/to_imperial", "u", state={"UNITS_MODE": "metric"})
case("u/odd", "u", state={"UNITS_MODE": "nautical"})
case("e/outgauge", "e", state={"last_engtemp": 90.5})
case("e/no_temp", "e", state={"protocol_mode": "extended", "last_telemetry_presence": 0})
case("e/metric", "engtemp", state={"UNITS_MODE": "metric", "last_engtemp": 90.5})
case("e/nan", "e", state={"last_engtemp": NAN})
case("o/oil", "o", state={"last_oiltemp": 101.5})
case("a/attitude", "a", state={"last_roll_rad": 0.7853981633974483, "last_pitch_rad": -0.00087})

# Alignment.
case("shift_v/ramp", "v", mods=SHIFT, state={"dock_mode_active": True})
case("shift_v/no_scanner", "v", mods=SHIFT)
case("shift_v/trailer", "v", mods=SHIFT, state={"scan_mode_active": True})
case("shift_v/alt", "v", mods=AS, state={"scan_mode_active": True})

# Tab.
case("tab/off", "tab")
for mods in MOD_COMBOS:
    case(f"tab/{mods}", "tab", mods=mods, state={"scan_mode_active": True})

# Space.
case("space/info", "space", returns={"request_vehicle_info": [["Engine: V8", "Power: 300 hp"], None]})
case("space/info_and_fail", "space", returns={"request_vehicle_info": [["Only"], ["x", "y"]]})
case("space/fail_reason", "space", returns={"request_vehicle_info": [[], ["nodetails", "No details page"]]})
case("space/fail_blank", "space", returns={"request_vehicle_info": [[], ["error", ""]]})
case("space/no_fail", "space", returns={"request_vehicle_info": [[], None]})
case("space/notselector_mod", "space", returns={"request_vehicle_info": [[], ["notselector", ""]],
                                                "_read_mod_details_page": True})
case("space/world", "space", returns={"_world_is_active": True})
case("space/menu", "space")

# Replay buffer.
case("digit/empty", "1")
for key in "1234567890":
    case(f"digit/three/{key}", key, buffer=["first", "second", "third"])
case("digit/ten", "0", buffer=[f"line {i}" for i in range(12)])
case("digit/shift", "3", mods=SHIFT, buffer=["a", "b", "c"])

# Input help.
case("help/on", "?", entry="press")
case("help/off", "/", entry="press", help_mode=True)
for key, mods in [("s", [False] * 3), ("s", CS), ("c", [True, True, True]), ("5", [False] * 3),
                  ("0", [False] * 3), ("5", CTRL), ("x", [False] * 3), ("tab", SHIFT),
                  ("space", [False] * 3), ("f9", [False] * 3), ("ctrl", [False] * 3)]:
    case(f"help/{key}/{mods}", key, entry="press", help_mode=True, preset_mods=mods)

# The key event itself.
case("press/up_ignored", "s", entry="press", event_type="up")
for key in ["ctrl", "control", "left ctrl", "right ctrl", "shift", "left shift", "right shift",
            "alt", "left alt", "right alt", "F9", "Left Ctrl"]:
    case(f"press/{key}", key, entry="press", preset_mods=[False, True, False])
case("press/mods_read_live", "s", entry="press", preset_mods=CTRL, preset_ctx=True)
case("press/ctrl_s", "s", entry="press", preset_mods=CTRL)

# The invoke wrapper.
case("invoke/no_ac", "s", no_ac=True)
case("invoke/restores", "r", mods=CTRL, preset_mods=[False, True, True], preset_ctx=True)
case("invoke/restores_after_raise", "r", preset_mods=[True, False, False], state={"last_rpm": NAN})
case("invoke/upper", "R", mods=SHIFT)
case("mcp/press", "g", entry="mcp", state={"last_gear_byte": 2})

# The dumps, run against scripted datagrams.
HD = [b"zeta = 1", b"  alpha = 2  ", b"mid\xff = 3", b"DONE", b"after"]
case("dump/hydros", "h", mods=SHIFT, returns={"threading.Thread": [list(d) for d in HD]})
case("dump/hydros_timeout", "h", mods=SHIFT, returns={"threading.Thread": []})
case("dump/powertrain_no_done", "p", mods=SHIFT,
     returns={"threading.Thread": [list(b"b = 2"), list(b"a = 1")]})
case("dump/electrics", "e", mods=CS, returns={"threading.Thread": [list(b" DONE ")]})
case("dump/electrics_lines", "e", mods=SHIFT,
     returns={"threading.Thread": [list(b"x"), list(b""), list(b"DONE")]})
for n, script in [
    ("none", []),
    ("only_name", [b"NAME:Pickup", b"NONE", b"DONE"]),
    ("one", [b"NONE", b"engine", b"DONE"]),
    ("two", [b"NAME:Van", b"engine", b"radiator", b"DONE"]),
    ("three", [b"NAME:", b"engine", b"radiator", b"left door\xe2\x82", b"DONE"]),
]:
    case(f"dump/damage/{n}", "m", returns={"threading.Thread": [list(d) for d in script]})


def main():
    install()
    help_rows = [[k[0], k[1], k[2], k[3], v] for k, v in b._F9_HELP.items()]
    cases = [run_case(c) for c in CASES]
    fixture = {
        "help": help_rows,
        "base": deep_fbits(BASE),
        "scenarios": {n: deep_fbits(s) for n, s in SCENARIOS.items()},
        "view_keys": VIEW_KEYS,
        "default_returns": DEFAULT_RETURNS,
        "cases": cases,
    }
    write_fixture("f9_commands", fixture)


if __name__ == "__main__":
    main()
