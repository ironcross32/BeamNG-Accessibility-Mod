"""Offline simulation for the controller Accessibility menu.

Run with::

    uv run python diagnostic/controller_menu_sim.py

It imports the real catalogs and navigator, replaces speech and command execution with
recorders, and requires no controller, audio device, elevated keyboard hook, or game.
"""

import os
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

import beamtel  # noqa: E402
import configurator  # noqa: E402
import nvda_ws_speaker  # noqa: E402


failures = []


def check(label, condition, detail=""):
    print(("OK   " if condition else "FAIL ") + label)
    if not condition:
        failures.append(f"{label}: {detail}")


real_status = beamtel.STATUS_METRICS
real_functions = beamtel.FUNCTION_ITEMS
real_say = beamtel.say
real_invoke = beamtel._invoke_f9_command
real_activate_clickspot = beamtel._activate_clickspot
real_send_clickspot = beamtel._send_clickspot_cmd
real_thread_class = beamtel.threading.Thread
real_sleep = beamtel.time.sleep
real_announce_clickspot_actions = beamtel.announce_clickspot_actions

spoken = []
commands = []
clickspot_activations = []
available = {"optional": True}


def metric(label, value, category=None):
    row = {
        "label": label,
        "getValue": lambda: (value, "units"),
        "isAvailable": lambda: True,
    }
    if category:
        row["category"] = category
    return row


def item(label, category, command, is_available=lambda: True):
    return {
        "label": label,
        "category": category,
        "command": (command, False, False, False),
        "isAvailable": is_available,
    }


try:
    beamtel.say = lambda text, **kwargs: spoken.append(text)
    beamtel._invoke_f9_command = (
        lambda name, ctrl=False, shift=False, alt=False: commands.append(
            (name, ctrl, shift, alt)
        )
    )
    beamtel._activate_clickspot = clickspot_activations.append
    beamtel.STATUS_METRICS = [
        metric("Status one", "one"),
        metric("Status two", "two"),
        metric("Status three", "three"),
    ]
    beamtel.FUNCTION_ITEMS = [
        item("Alpha one", "Alpha", "a"),
        item("Alpha two", "Alpha", "b"),
        item("Beta one", "Beta", "c"),
        item("Beta two", "Beta", "d"),
        item(
            "Optional one",
            "Optional",
            "e",
            is_available=lambda: available["optional"],
        ),
    ]
    beamtel.current_accessibility_screen_index = 0
    beamtel.current_status_metric_index = 0
    beamtel.current_functions_item_index = 0
    beamtel.current_clickspot_index = 0
    beamtel._clickspot_set_off()

    check(
        "Status is the startup screen",
        beamtel.ACCESSIBILITY_SCREENS[
            beamtel.current_accessibility_screen_index
        ]
        == "Status",
    )

    beamtel.navigate_accessibility_menu("status_down")
    beamtel.navigate_accessibility_menu("next_menu")
    check(
        "Next screen selects Functions and announces category",
        beamtel.current_accessibility_screen_index == 1
        and spoken[-1] == "Functions: Alpha: Alpha one",
        spoken[-1],
    )
    beamtel.navigate_accessibility_menu("status_down")
    beamtel.navigate_accessibility_menu("next_menu")
    check(
        "Next screen selects Click spots after Functions",
        beamtel.current_accessibility_screen_index == 2
        and spoken[-1] == "Click spots: Turn on clickspot detection",
        spoken[-1],
    )
    beamtel.navigate_accessibility_menu("activate")
    check(
        "Click spots enable item dispatches the existing toggle",
        commands[-1] == ("c", True, True, False),
        commands[-1],
    )
    beamtel.navigate_accessibility_menu("next_menu")
    check(
        "Three screens wrap and preserve the Status cursor",
        beamtel.current_accessibility_screen_index == 0
        and beamtel.current_status_metric_index == 1
        and spoken[-1] == "Status: Status two, two units",
        spoken[-1],
    )
    beamtel.navigate_accessibility_menu("previous_menu")
    check(
        "Previous screen wraps from Status to Click spots",
        beamtel.current_accessibility_screen_index == 2
        and spoken[-1] == "Click spots: Turn on clickspot detection",
        spoken[-1],
    )
    beamtel.navigate_accessibility_menu("previous_menu")
    check(
        "Previous screen preserves the Functions cursor",
        beamtel.current_accessibility_screen_index == 1
        and beamtel.current_functions_item_index == 1
        and spoken[-1] == "Functions: Alpha: Alpha two",
        spoken[-1],
    )

    beamtel.current_functions_item_index = 0
    beamtel.navigate_accessibility_menu("status_up")
    check(
        "Functions wraps upward",
        beamtel.current_functions_item_index == 4
        and spoken[-1] == "Optional: Optional one",
        spoken[-1],
    )
    beamtel.navigate_accessibility_menu("status_down")
    check(
        "Functions wraps downward",
        beamtel.current_functions_item_index == 0
        and spoken[-1] == "Alpha: Alpha one",
        spoken[-1],
    )

    beamtel.current_functions_item_index = 1
    beamtel.navigate_accessibility_menu("status_down")
    forward_boundary = spoken[-1]
    beamtel.navigate_accessibility_menu("status_up")
    check(
        "Category boundaries announce in both directions",
        forward_boundary == "Beta: Beta one" and spoken[-1] == "Alpha: Alpha two",
        f"{forward_boundary!r}, {spoken[-1]!r}",
    )
    cursor_before = beamtel.current_functions_item_index
    beamtel.navigate_accessibility_menu("status_repeat")
    check(
        "Functions Repeat reads without moving",
        beamtel.current_functions_item_index == cursor_before
        and spoken[-1] == "Alpha two",
        spoken[-1],
    )
    beamtel.navigate_accessibility_menu("activate")
    check(
        "Functions Activate dispatches the selected command",
        commands[-1] == ("b", False, False, False)
        and beamtel.current_functions_item_index == cursor_before,
        commands[-1],
    )

    beamtel.navigate_accessibility_menu("previous_menu")
    beamtel.current_status_metric_index = 0
    beamtel.navigate_accessibility_menu("status_up")
    check(
        "Status wraps upward",
        beamtel.current_status_metric_index == 2
        and spoken[-1] == "Status three, three units",
        spoken[-1],
    )
    beamtel.navigate_accessibility_menu("status_down")
    check(
        "Status wraps downward",
        beamtel.current_status_metric_index == 0
        and spoken[-1] == "Status one, one units",
        spoken[-1],
    )
    beamtel.navigate_accessibility_menu("status_repeat")
    check("Status Repeat reads only the value", spoken[-1] == "one units", spoken[-1])
    beamtel.navigate_accessibility_menu("activate")
    check("Status Activate repeats only the value", spoken[-1] == "one units", spoken[-1])

    beamtel.navigate_accessibility_menu("next_menu")
    available["optional"] = False
    beamtel.current_functions_item_index = 999
    beamtel.navigate_accessibility_menu("status_repeat")
    check(
        "Availability changes safely normalize the cursor",
        0 <= beamtel.current_functions_item_index < 4,
        beamtel.current_functions_item_index,
    )

    beamtel.navigate_accessibility_menu("next_menu")
    beamtel._clickspot_set_pending()
    beamtel.navigate_accessibility_menu("status_repeat")
    command_count = len(commands)
    activation_count = len(clickspot_activations)
    beamtel.navigate_accessibility_menu("activate")
    check(
        "Loading placeholder is announced and non-mutating",
        spoken[-1] == "Detecting click spots"
        and len(commands) == command_count
        and len(clickspot_activations) == activation_count,
        spoken[-1],
    )

    beamtel._clickspot_begin_list(2)
    beamtel._clickspot_append_row(1, 102, "Window switch")
    check(
        "Partial clickspot transfers are not published",
        beamtel.clickspot_list_loading and not beamtel.clickspot_trigger_list,
        beamtel.clickspot_trigger_list,
    )
    beamtel._clickspot_append_row(0, 101, "Door latch")
    check(
        "Completed clickspot transfers publish atomically in cache order",
        not beamtel.clickspot_list_loading
        and beamtel.clickspot_trigger_list
        == [(0, 101, "Door latch"), (1, 102, "Window switch")],
        beamtel.clickspot_trigger_list,
    )
    beamtel.navigate_accessibility_menu("status_repeat")
    check(
        "Click spots Repeat reads without moving",
        beamtel.current_clickspot_index == 0 and spoken[-1] == "Door latch",
        spoken[-1],
    )
    beamtel.navigate_accessibility_menu("status_down")
    beamtel.navigate_accessibility_menu("activate")
    check(
        "Click spots Activate dispatches the selected cache index",
        clickspot_activations[-1] == 1 and beamtel.current_clickspot_index == 1,
        clickspot_activations,
    )
    beamtel.navigate_accessibility_menu("status_down")
    downward_wrap = spoken[-1]
    beamtel.navigate_accessibility_menu("status_up")
    check(
        "Click spots wrap in both directions",
        downward_wrap == "Door latch"
        and beamtel.current_clickspot_index == 1
        and spoken[-1] == "Window switch",
        f"{downward_wrap!r}, {spoken[-1]!r}",
    )

    beamtel.navigate_accessibility_menu("next_menu")
    beamtel.navigate_accessibility_menu("next_menu")
    beamtel.navigate_accessibility_menu("next_menu")
    check(
        "Click spots preserve their cursor across screen changes",
        beamtel.current_accessibility_screen_index == 2
        and beamtel.current_clickspot_index == 1
        and spoken[-1] == "Click spots: Window switch",
        spoken[-1],
    )

    beamtel._clickspot_set_pending()
    check(
        "A vehicle recache immediately clears stale rows and resets the cursor",
        beamtel.clickspot_list_loading
        and not beamtel.clickspot_trigger_list
        and beamtel.current_clickspot_index == 0,
        beamtel.clickspot_trigger_list,
    )
    beamtel._clickspot_begin_list(0)
    beamtel.navigate_accessibility_menu("status_repeat")
    activation_count = len(clickspot_activations)
    beamtel.navigate_accessibility_menu("activate")
    check(
        "Empty placeholder is announced and non-mutating",
        spoken[-1] == "No click spots found"
        and not beamtel.clickspot_list_loading
        and len(clickspot_activations) == activation_count,
        spoken[-1],
    )

    # Validate the production catalog after the navigation state machine checks.
    beamtel.FUNCTION_ITEMS = real_functions
    expected_commands = [
        ("Toggle vehicle scanner", ("v", True, False, False)),
        ("Next vehicle scanner target", ("tab", False, False, False)),
        ("Previous vehicle scanner target", ("tab", False, True, False)),
        ("Lock onto nearest vehicle", ("tab", True, False, False)),
        ("Distance and orientation", ("d", False, False, False)),
        ("Relative bearing", ("d", False, True, False)),
        ("Trailer or ramp alignment", ("v", False, True, False)),
        ("Coupler distance callouts", ("d", True, True, False)),
        ("Toggle alignment instrument", ("i", True, False, False)),
        ("Alignment or cannon readout", ("i", False, False, False)),
        ("Cycle reference band", ("i", False, True, False)),
        ("Pedal tones", ("c", True, False, False)),
        ("Heading guidance", ("h", True, False, False)),
        ("Coordinate guidance", ("g", True, False, False)),
        ("Drift detection", ("d", True, False, False)),
        ("Low-speed detection", ("l", True, True, False)),
        ("Wheel-slip detection", ("k", True, False, False)),
        ("Obstacle detection", ("o", True, False, False)),
        ("Road detection", ("r", True, False, False)),
        ("Road-status readout", ("r", True, True, False)),
        ("Mark waypoint", ("c", False, True, False)),
        ("Speak marked coordinates", ("c", False, False, True)),
        ("Distance and bearing", ("w", False, False, False)),
        ("Redline RPM", ("r", False, True, False)),
        ("Maximum turbo pressure", ("t", False, True, False)),
        ("Air pressure", ("p", False, False, False)),
        ("Attitude", ("a", False, False, False)),
        ("Coordinates", ("c", False, False, False)),
        ("Damage report", ("m", False, False, False)),
        ("Toggle camera information", ("f", False, False, True)),
        ("Camera heading", ("h", False, False, True)),
        ("Camera altitude", ("a", False, False, True)),
        ("Camera pitch", ("p", False, False, True)),
        ("Vehicle bearing", ("v", False, False, True)),
        ("Vehicle distance", ("d", False, False, True)),
        ("Accessible node grabber", ("n", True, False, False)),
        ("Clickspot detection", ("c", True, True, False)),
        ("Switch units", ("u", False, False, False)),
    ]
    actual_commands = [
        (row["label"], row["command"])
        for row in real_functions
        if "command" in row
    ]
    check(
        "Functions catalog maps every item to the intended F9 command",
        actual_commands == expected_commands,
        actual_commands,
    )
    commands.clear()
    for row in real_functions:
        if "command" in row:
            beamtel._execute_function_item(row)
    check(
        "Every Functions command dispatches to its existing F9 handler",
        commands == [command for _, command in expected_commands],
        commands,
    )
    terrain = next(row for row in real_functions if row["label"] == "Terrain scan")
    check(
        "Terrain scan uses the direct driving-only handler",
        terrain.get("handler") is beamtel.trigger_terrain_scan_driving_only
        and "command" not in terrain,
    )

    beamtel.scan_mode_active = False
    beamtel.free_cam_active = False
    beamtel.marked_coord_x = None
    beamtel.protocol_mode = "outgauge"
    beamtel.last_protocol_flags = 0
    beamtel.last_air_pressure_max = 0.0
    beamtel._last_telemetry_ts = 0.0
    hidden = {row["label"] for row in real_functions if not row["isAvailable"]()}
    check(
        "Dependent functions hide while unavailable",
        {
            "Distance and orientation",
            "Coordinate guidance",
            "Maximum turbo pressure",
            "Air pressure",
            "Camera heading",
            "Terrain scan",
        }.issubset(hidden),
        hidden,
    )
    beamtel.scan_mode_active = True
    beamtel.free_cam_active = True
    beamtel.marked_coord_x = 1.0
    beamtel.protocol_mode = "extended"
    beamtel.last_protocol_flags = beamtel.OG_TURBO
    beamtel.last_air_pressure_max = 10.0
    beamtel._last_telemetry_ts = time.time()
    shown = {row["label"] for row in real_functions if row["isAvailable"]()}
    check(
        "Dependent functions reappear from live state",
        {
            "Distance and orientation",
            "Coordinate guidance",
            "Maximum turbo pressure",
            "Air pressure",
            "Camera heading",
            "Terrain scan",
        }.issubset(shown),
        shown,
    )

    sent_clickspot_commands = []

    class ImmediateThread:
        def __init__(self, target, args=(), kwargs=None, **_options):
            self.target = target
            self.args = args
            self.kwargs = kwargs or {}

        def start(self):
            self.target(*self.args, **self.kwargs)

    beamtel._send_clickspot_cmd = sent_clickspot_commands.append
    beamtel.threading.Thread = ImmediateThread
    beamtel.time.sleep = lambda _seconds: None
    beamtel._activate_clickspot = real_activate_clickspot
    beamtel._activate_clickspot(7)
    check(
        "Controller clickspot activation uses SNAP then press and release",
        sent_clickspot_commands
        == ["SNAP:7", "EXEC:7,1", "EXEC:7,0"],
        sent_clickspot_commands,
    )
    sent_clickspot_commands.clear()
    beamtel._clickspot_browser_on_enter(0, "ignored", 8)
    check(
        "Keyboard Enter uses the same clickspot activation helper",
        sent_clickspot_commands
        == ["SNAP:8", "EXEC:8,1", "EXEC:8,0"],
        sent_clickspot_commands,
    )

    check(
        "Clickspot action announcements default off in both config backends",
        beamtel.DEFAULT_CONFIG["announce_clickspot_actions"] is False
        and configurator.DEFAULT_CONFIG["announce_clickspot_actions"] is False,
    )
    beamtel.announce_clickspot_actions = False
    speech_count = len(spoken)
    beamtel._announce_clickspot_action("Door latch")
    check(
        "Successful clickspot activation is silent when announcements are off",
        len(spoken) == speech_count,
        spoken[speech_count:],
    )
    beamtel._announce_clickspot_action(None, failure_reason="behind camera")
    check(
        "Failed clickspot activation is silent when announcements are off",
        len(spoken) == speech_count,
        spoken[speech_count:],
    )
    beamtel.announce_clickspot_actions = True
    beamtel._announce_clickspot_action("Door latch")
    check(
        "Successful clickspot activation is announced when opted in",
        spoken[-1] == "Jumped to Door latch",
        spoken[-1],
    )
    beamtel._announce_clickspot_action(None, failure_reason="behind camera")
    check(
        "Failed clickspot activation is announced when opted in",
        spoken[-1] == "Cannot jump, behind camera",
        spoken[-1],
    )

    with open(os.path.join(ROOT, "config_ui.py"), "r", encoding="utf-8") as ui_file:
        ui_source = ui_file.read()
    check(
        "Configuration UI loads and saves the clickspot announcement checkbox",
        'label="Announce clickspot actions"' in ui_source
        and 'cfg.get("announce_clickspot_actions", False)' in ui_source
        and 'cfg["announce_clickspot_actions"] =' in ui_source,
    )
    beamtel._send_clickspot_cmd = real_send_clickspot
    beamtel.threading.Thread = real_thread_class
    beamtel.time.sleep = real_sleep

    lua_path = os.path.join(
        ROOT, "bng_mod", "lua", "ge", "extensions", "clickspotAccessible.lua"
    )
    with open(lua_path, "r", encoding="utf-8") as lua_file:
        lua_source = lua_file.read()
    check(
        "Lua invalidates stale rows across every cache lifecycle",
        lua_source.count('sendListState("TRIGGER_LIST_PENDING")') >= 4
        and lua_source.count('sendListState("TRIGGER_LIST_OFF")') >= 3,
    )
    with open(os.path.join(ROOT, "beamtel.py"), "r", encoding="utf-8") as py_file:
        python_source = py_file.read()
    check(
        "Python listener accepts Lua pending and off lifecycle packets",
        'if text == "TRIGGER_LIST_PENDING"' in python_source
        and 'if text == "TRIGGER_LIST_OFF"' in python_source,
    )

    received_actions = []
    nvda_ws_speaker.register_accessibility_action_callback(received_actions.append)
    for action in beamtel.ACCESSIBILITY_ACTIONS:
        nvda_ws_speaker.handle_ws_message(
            {"type": "accessibility_action", "action": action}
        )
    nvda_ws_speaker.handle_ws_message(
        {"type": "accessibility_action", "action": "not_a_command"}
    )
    check(
        "Python bridge allows exactly the six controller actions",
        set(received_actions) == set(beamtel.ACCESSIBILITY_ACTIONS)
        and len(received_actions) == 6,
        received_actions,
    )
finally:
    beamtel.STATUS_METRICS = real_status
    beamtel.FUNCTION_ITEMS = real_functions
    beamtel.say = real_say
    beamtel._invoke_f9_command = real_invoke
    beamtel._activate_clickspot = real_activate_clickspot
    beamtel._send_clickspot_cmd = real_send_clickspot
    beamtel.threading.Thread = real_thread_class
    beamtel.time.sleep = real_sleep
    beamtel.announce_clickspot_actions = real_announce_clickspot_actions


if failures:
    print("\n" + "\n".join(failures))
    raise SystemExit(1)
print("\nController menu simulation passed.")
