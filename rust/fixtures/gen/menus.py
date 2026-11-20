"""Golden cases for the E5 menus: status rows, function items, controller navigation
speech, the virtual browser and buffer mode.

Each scenario is a full set of beamtel globals (``BASE`` plus overrides), so the Rust
test can rebuild the same state from ``State::default()``. Speech, F9 command dispatch,
clickspot activation and every keyboard hook are replaced by recorders; nothing reaches
the game or installs a hook.

    uv run python rust/fixtures/gen/menus.py
"""

from __future__ import annotations

import time

from _beamtel import beamtel
from _common import deep_fbits, write_fixture

# Python-side initial values of every global a scenario may set.
BASE = {
    "units": "imperial",
    "protocol_mode": "outgauge",
    "last_heading": 0.0,
    "last_speed_ms": 0.0,
    "last_rpm": 0.0,
    "last_gear_byte": None,
    "last_gear_str": None,
    "last_fuel": 0.0,
    "last_ev_range_m": -1.0,
    "last_motor_power_kw": 0.0,
    "last_motor_torque_nm": 0.0,
    "last_engtemp": 0.0,
    "last_oiltemp": 0.0,
    "last_clutch_temp": 0.0,
    "_env_ambient_c": None,
    "last_turbo": 0.0,
    "last_protocol_flags": 0,
    "last_air_pressure": 0.0,
    "last_air_pressure_max": 0.0,
    "last_trip_m": -1.0,
    "last_odometer_m": -1.0,
    "last_lifetime_m": -1.0,
    "last_telemetry_presence": beamtel.WHEEL_POS_CORNERS,
    "last_implement_flags": 0.0,
    "last_implement_edge_height": -1.0,
    "last_implement_tilt_deg": 0.0,
    "last_articulation_deg": 0.0,
    "last_trailer_deg": 0.0,
    "last_trailer_id": None,
    "last_trailer_name": "",
    "trailer_live": False,
    "wheel_live": False,
    "last_wheel_rows": [],
    "last_tire_pressure_f": 0.0,
    "last_tire_pressure_fl": 0.0,
    "last_tire_pressure_fr": 0.0,
    "last_tire_pressure_r": 0.0,
    "last_tire_pressure_rl": 0.0,
    "last_tire_pressure_rr": 0.0,
    "last_brake_temp_f": 0.0,
    "last_brake_temp_fl": 0.0,
    "last_brake_temp_fr": 0.0,
    "last_brake_temp_r": 0.0,
    "last_brake_temp_rl": 0.0,
    "last_brake_temp_rr": 0.0,
    "scan_mode_active": False,
    "dock_mode_active": False,
    "marked_coord_x": None,
    "free_cam_active": False,
    "world_active": False,
    "route_live": False,
    "_fuel_target_id": None,
    "_fuel_in_range": False,
    "_fuel_parking": False,
    "_ui_screen_context": "",
    "clickspot_mode_active": False,
    "clickspot_list_loading": False,
    "clickspot_trigger_list": [],
}

ICE = {
    "protocol_mode": "extended",
    "last_heading": 123.45,
    "last_speed_ms": 20.0,
    "last_rpm": 2500.5,
    "last_gear_str": "D",
    "last_fuel": 0.456,
    "last_engtemp": 90.4,
    "last_oiltemp": 101.6,
    "last_clutch_temp": 55.5,
    "last_turbo": 1.234,
    "last_protocol_flags": beamtel.OG_TURBO,
    "last_air_pressure": 110.37,
    "last_air_pressure_max": 120.0,
    "last_trip_m": 1234.5,
    "last_odometer_m": 98765.4,
    "last_lifetime_m": 50.0,
    "last_telemetry_presence": beamtel.WHEEL_POS_CORNERS
    | beamtel.TELEMETRY_HAS_ENGINE_TEMP
    | beamtel.TELEMETRY_HAS_CLUTCH
    | beamtel.TELEMETRY_HAS_AIR,
    "last_tire_pressure_fl": 32.15,
    "last_tire_pressure_fr": 31.95,
    "last_tire_pressure_rl": 30.0,
    "last_tire_pressure_rr": 29.05,
    "last_brake_temp_fl": 120.5,
    "last_brake_temp_fr": 119.5,
    "last_brake_temp_rl": 80.25,
    "last_brake_temp_rr": -3.5,
    "_env_ambient_c": 21.5,
}

EV = {
    "protocol_mode": "extended",
    "last_heading": 359.96,
    "last_speed_ms": 3.3,
    "last_gear_str": "P",
    "last_fuel": 0.995,
    "last_ev_range_m": -1.0,
    "last_motor_power_kw": 150.5,
    "last_motor_torque_nm": -20.4,
    "last_telemetry_presence": beamtel.WHEEL_POS_F
    | beamtel.WHEEL_POS_RL
    | beamtel.WHEEL_POS_RR
    | beamtel.TELEMETRY_HAS_ELECTRIC,
    "last_tire_pressure_f": 36.6,
    "last_brake_temp_f": 40.0,
    "last_trip_m": 50.0,
}

LOADER = {
    "protocol_mode": "extended",
    "last_gear_str": "M3",
    "last_implement_flags": float(
        beamtel.IMPL_FLAG_PRESENT | beamtel.IMPL_FLAG_GROUND_VALID | beamtel.IMPL_FLAG_ARTIC_VALID
    ),
    "last_implement_edge_height": -0.5,
    "last_implement_tilt_deg": -3.4,
    "last_articulation_deg": 12.6,
    "trailer_live": True,
    "last_trailer_id": 5,
    "last_trailer_name": "Flatbed",
    "last_trailer_deg": -0.4,
    "wheel_live": True,
    "last_wheel_rows": [
        {"label": "front left", "psi": 30.25, "brake_c": 60.0, "broken": False},
        {"label": "front right", "psi": None, "brake_c": 61.5, "broken": False},
        {"label": "trailer rear left", "psi": 80.0, "brake_c": None, "broken": False},
        {"label": "trailer rear right", "psi": 0.0, "brake_c": 0.0, "broken": True},
    ],
}


def scenario(*overrides):
    s = dict(BASE)
    for o in overrides:
        s.update(o)
    return s


STATUS_SCENARIOS = {
    "initial_outgauge": scenario(),
    "outgauge_third_gear": scenario({"last_gear_byte": 3, "last_rpm": 1500.5, "last_speed_ms": 13.4}),
    "ice_imperial": scenario(ICE),
    "ice_metric": scenario(ICE, {"units": "metric"}),
    "ev_imperial": scenario(EV),
    "ev_metric_range": scenario(EV, {"units": "metric", "last_ev_range_m": 123456.0}),
    "loader_imperial": scenario(LOADER),
    "loader_metric": scenario(LOADER, {"units": "metric", "last_implement_edge_height": 1.234,
                                        "last_implement_tilt_deg": 7.6,
                                        "last_articulation_deg": -0.2,
                                        "last_trailer_deg": 12.5}),
    "air_without_max": scenario(ICE, {"last_air_pressure_max": 1.0}),
    "hybrid": scenario(ICE, {"last_telemetry_presence": beamtel.WHEEL_POS_CORNERS
                             | beamtel.TELEMETRY_HAS_ENGINE_TEMP
                             | beamtel.TELEMETRY_HAS_ELECTRIC,
                             "last_ev_range_m": 4000.0}),
}

FUNCTION_SCENARIOS = {
    "nothing": scenario(),
    "everything": scenario(ICE, {
        "scan_mode_active": True,
        "marked_coord_x": 1.0,
        "free_cam_active": True,
        "world_active": True,
        "route_live": True,
        "_fuel_target_id": 3,
        "_fuel_in_range": True,
        "_ui_screen_context": "mod_details",
    }),
    "dock_only_parking": scenario({
        "dock_mode_active": True,
        "world_active": True,
        "_fuel_target_id": 3,
        "_fuel_in_range": True,
        "_fuel_parking": True,
    }),
}

NAV_ACTIONS_STATUS = [
    "status_down", "status_down", "status_up", "status_repeat", "activate", "status_down",
    "status_down", "status_down", "status_down", "status_down", "status_down", "status_down",
    "status_down", "status_down", "status_down", "status_down", "status_down", "status_down",
    "status_up", "status_up", "next_menu", "status_down", "status_down", "status_repeat",
    "status_up", "status_up", "status_up", "activate", "status_down", "next_menu",
    "status_repeat", "activate", "status_down", "activate", "previous_menu", "previous_menu",
    "status_down", "bogus", "next_menu",
]

NAV_SCENARIOS = {
    "ice_tour": (scenario(ICE), NAV_ACTIONS_STATUS),
    "loader_tour": (scenario(LOADER), NAV_ACTIONS_STATUS),
    "clickspots_listed": (
        scenario(ICE, {
            "clickspot_mode_active": True,
            "clickspot_trigger_list": [[0, 101, "Door latch"], [1, 102, "Window switch"],
                                       [2, 103, "Horn"]],
        }),
        ["previous_menu", "status_down", "status_down", "status_down", "status_up",
         "activate", "status_repeat", "next_menu", "next_menu"],
    ),
    "clickspots_loading": (
        scenario({"clickspot_mode_active": True, "clickspot_list_loading": True}),
        ["previous_menu", "activate", "status_down", "status_repeat"],
    ),
    "clickspots_empty": (
        scenario({"clickspot_mode_active": True}),
        ["previous_menu", "activate", "status_up"],
    ),
}


def apply(s):
    b = beamtel
    b.UNITS_MODE = s["units"]
    now_mono = time.monotonic()
    for key, value in s.items():
        if key in ("units", "trailer_live", "wheel_live", "world_active", "route_live"):
            continue
        if key == "clickspot_trigger_list":
            value = [tuple(r) for r in value]
        if key == "last_wheel_rows":
            value = [dict(r) for r in value]
        setattr(b, key, value)
    b.last_trailer_stamp = now_mono if s["trailer_live"] else 0.0
    b.last_wheel_stamp = now_mono if s["wheel_live"] else 0.0
    b._last_telemetry_ts = time.time() if s["world_active"] else 0.0
    if s["route_live"]:
        b.route_dest_x, b.route_dest_y, b.route_beacon_stamp = 1.0, 2.0, now_mono
    else:
        b.route_dest_x, b.route_dest_y, b.route_beacon_stamp = None, None, 0.0
    b.road_mode_active = False
    b.current_accessibility_screen_index = 0
    b.current_status_metric_index = 0
    b.current_functions_item_index = 0
    b.current_clickspot_index = 0
    b._vbrowser_active = False


def status_rows(s):
    apply(s)
    out = []
    for m in beamtel._status_rows():
        row = {"label": m["label"], "category": m.get("category"), "available": bool(m["isAvailable"]())}
        if row["available"]:
            try:
                value, unit = m["getValue"]()
                row["value_text"] = f"{value} {unit}".strip()
            except Exception as exc:  # noqa: BLE001
                row["err"] = type(exc).__name__
        out.append(row)
    return out


def function_labels(s):
    apply(s)
    return [i["label"] for i in beamtel.FUNCTION_ITEMS
            if i["label"] != "Hill-climb reports" and i["isAvailable"]()]


spoken = []
commands = []
activations = []


def navigate(s, actions):
    apply(s)
    spoken.clear()
    commands.clear()
    activations.clear()
    steps = []
    for a in actions:
        before = len(spoken), len(commands), len(activations)
        err = None
        try:
            beamtel.navigate_accessibility_menu(a)
        except Exception as exc:  # noqa: BLE001
            err = type(exc).__name__
        steps.append({
            "action": a,
            "spoken": spoken[before[0]:],
            "commands": [list(c) for c in commands[before[1]:]],
            "activations": activations[before[2]:],
            "err": err,
            "screen": beamtel.current_accessibility_screen_index,
            "status_index": beamtel.current_status_metric_index,
            "functions_index": beamtel.current_functions_item_index,
            "clickspot_index": beamtel.current_clickspot_index,
        })
    return steps


callbacks = []


def vbrowser_cases():
    cases = []

    def run(name, fn):
        spoken.clear()
        callbacks.clear()
        fn()
        cases.append({"name": name, "spoken": list(spoken), "callbacks": list(callbacks),
                      "active": beamtel._vbrowser_active,
                      "index": beamtel._vbrowser_index,
                      "lines": list(beamtel._vbrowser_lines)})

    def nav(n):
        beamtel._on_vbrowser_nav(beamtel.SimpleNamespace(name=n))

    lines = ["Alpha", "Beta", "Gamma"]

    def read_only():
        beamtel.open_virtual_browser(lines, title="List")
        nav("up"); nav("down"); nav("down"); nav("down"); nav("up"); nav("left")
        beamtel._drive_virtual_browser("status_repeat")
        beamtel._drive_virtual_browser("activate")
        beamtel._drive_virtual_browser("status_down")
        beamtel._drive_virtual_browser("status_up")
        beamtel._drive_virtual_browser("next_menu")

    def with_enter():
        beamtel.open_virtual_browser(
            lines, on_enter=lambda i, line, d: callbacks.append(["enter", i, line, d]),
            entry_data=[10, {"k": "v"}], start_index=7, announce_interrupt=False)
        beamtel._drive_virtual_browser("activate")
        nav("up")
        beamtel._drive_virtual_browser("activate")
        nav("up")
        beamtel._on_vbrowser_enter(None)
        beamtel._on_vbrowser_escape(None)

    def with_adjust_escape():
        beamtel.open_virtual_browser(
            lines, title="",
            on_adjust=lambda i, line, d, delta: callbacks.append(["adjust", i, line, d, delta]),
            on_escape=lambda: callbacks.append(["escape"]), start_index=-4)
        beamtel._on_vbrowser_adjust(beamtel.SimpleNamespace(name="right"))
        beamtel._on_vbrowser_adjust(beamtel.SimpleNamespace(name="left"))
        beamtel._drive_virtual_browser("previous_menu")
        beamtel.close_virtual_browser(speak_exit=True)

    def empty():
        beamtel.open_virtual_browser([], title="Nothing")
        beamtel._drive_virtual_browser("status_repeat")

    def closed_nav():
        beamtel.close_virtual_browser(speak_exit=False)
        nav("down")
        beamtel._on_vbrowser_enter(None)

    run("read_only", read_only)
    run("with_enter", with_enter)
    run("with_adjust_escape", with_adjust_escape)
    run("empty", empty)
    run("closed_nav", closed_nav)
    return cases


def buffer_cases():
    cases = []

    def run(name, buffer, keys):
        spoken.clear()
        beamtel.SPEECH_BUFFER.clear()
        beamtel.SPEECH_BUFFER.extend(buffer)
        beamtel.buffer_mode_active = False
        beamtel.toggle_buffer_mode()
        start = beamtel.current_buffer_index
        for k in keys:
            beamtel.on_buffer_nav_press(beamtel.SimpleNamespace(name=k))
        end = beamtel.current_buffer_index
        beamtel.toggle_buffer_mode()
        after = beamtel.on_buffer_nav_press(beamtel.SimpleNamespace(name="["))
        del after
        cases.append({"name": name, "buffer": buffer, "keys": keys, "spoken": list(spoken),
                      "start_index": start, "end_index": end})

    run("three", ["one", "two", "three"], ["[", "[", "[", "[", "]", "]", "]", "x"])
    run("empty", [], ["[", "]"])
    run("single", ["only"], ["]", "[", "["])
    return cases


def main():
    beamtel.say = lambda text, interrupt=True, exclude_from_buffer=False, **_k: spoken.append(
        [text, interrupt, exclude_from_buffer])
    beamtel._invoke_f9_command = lambda name, ctrl=False, shift=False, alt=False: commands.append(
        (name, ctrl, shift, alt))
    beamtel._activate_clickspot = activations.append
    beamtel._hook_suppressed = lambda key, handler: {"key": key}
    beamtel._unhook_suppressed = lambda records: records.clear()
    beamtel.keyboard.is_pressed = lambda name: False
    beamtel.HILL_CLIMB_CHALLENGE.list_reports = lambda limit=None: []

    catalog = []
    for i in beamtel.FUNCTION_ITEMS:
        entry = {"label": i["label"], "category": i["category"]}
        if "command" in i:
            entry["command"] = list(i["command"])
        else:
            entry["handler"] = True
        catalog.append(entry)

    fixture = {
        "base": deep_fbits(BASE),
        "status": [
            {"name": n, "state": deep_fbits(s), "rows": status_rows(s)}
            for n, s in STATUS_SCENARIOS.items()
        ],
        "function_catalog": catalog,
        "functions": [
            {"name": n, "state": deep_fbits(s), "available": function_labels(s)}
            for n, s in FUNCTION_SCENARIOS.items()
        ],
        "navigation": [
            {"name": n, "state": deep_fbits(s), "steps": navigate(s, actions)}
            for n, (s, actions) in NAV_SCENARIOS.items()
        ],
        "vbrowser": vbrowser_cases(),
        "buffer": buffer_cases(),
    }
    write_fixture("menus", fixture)


if __name__ == "__main__":
    main()
