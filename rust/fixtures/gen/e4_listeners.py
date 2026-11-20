"""Golden cases for beam_engine's E4 listeners and browsers (camera, node grabber,
clickspots, vehicle bindings, environment, audio debug, vehicle info, page text, slots,
binding learn, console history, the induction report), from beamtel.py.

    uv run python rust/fixtures/gen/e4_listeners.py

The real listener functions run against a fake socket, exactly as e2_listeners.py does:
each "recv" step's datagrams are handed to ``recvfrom`` in order, then the stop event is
set. The real virtual browser runs with KEYBOARD_OK off, so it speaks and keeps its
state but hooks no keys. Speech is read back from SPEECH_TAP, commands from the fake
socket's ``sendto``, audio state from a real ``audio.AudioController``. Threads that the
Python starts for delayed work (clickspot press/release, the induction report) run
synchronously with ``time.sleep`` a no-op; reply timers are cancelled after each step
and their timeouts are steps of their own.
"""

from __future__ import annotations

import json
import logging
import os
import socket as socket_mod
import tempfile
import threading
import time
import types

from _beamtel import beamtel as b
from _common import deep_fbits, fbits, write_fixture

import audio  # noqa: E402

LOG = logging.getLogger("e4-fixture")


class FakeSock:
    queue: list = []
    sent: list = []
    stop = None

    def __init__(self, *args, **kwargs):
        pass

    def bind(self, addr):
        pass

    def settimeout(self, t):
        pass

    def setsockopt(self, *args):
        pass

    def recvfrom(self, n):
        if FakeSock.queue:
            return FakeSock.queue.pop(0), ("127.0.0.1", 50000)
        if FakeSock.stop is not None:
            FakeSock.stop.set()
        raise socket_mod.timeout()

    def sendto(self, data, addr):
        FakeSock.sent.append([addr[1], data.decode("utf-8")])

    def close(self):
        pass


socket_mod.socket = FakeSock
b.speech.speak = lambda *args, **kwargs: None
b.KEYBOARD_OK = False
b._command_dispatch = lambda fn, *args, **kwargs: fn(*args, **kwargs)

warps = []
b.warp_cursor_to_viewport = lambda x, y, w=0, h=0: warps.append([x, y, w, h])
broadcasts = []
page_replies = []


def fake_broadcast(msg):
    broadcasts.append(msg)
    for r in list(page_replies):
        b._on_page_text(r["lines"], r["title"], r["code"], r["sentence"])


b.broadcast = fake_broadcast


class SyncThread:
    def __init__(self, target=None, daemon=None, args=(), kwargs=None, **_):
        self._t = target
        self._a = args
        self._k = kwargs or {}

    def start(self):
        self._t(*self._a, **self._k)


class _ThreadingShim:
    Thread = SyncThread

    def __getattr__(self, name):
        return getattr(threading, name)


class _TimeShim:
    @staticmethod
    def sleep(_s):
        pass

    def __getattr__(self, name):
        return getattr(time, name)


b.threading = _ThreadingShim()
b.time = _TimeShim()

# The node grabber installs its Space hook with keyboard.hook_key directly.
b.keyboard.hook_key = lambda *a, **k: (lambda: None)
b._unhook_suppressed = lambda records: records.clear() if isinstance(records, list) else None


def reset_speech():
    b.SPEECH_TAP.clear()
    b._speech_protected_until = 0.0
    b._loading_active = False
    b._loading_settling = False
    b._loading_pending_vehicle = ""
    b._command_context = False
    FakeSock.sent.clear()
    FakeSock.queue.clear()
    warps.clear()
    broadcasts.clear()


def tap():
    return [
        {"text": e["text"], "source": e["source"], "interrupt": e["interrupt"],
         "excluded": e["excluded"], "spoken": e["spoken"]}
        for e in b.SPEECH_TAP
    ]


def run_listener(fn, datagrams, *args):
    FakeSock.queue = list(datagrams)
    FakeSock.stop = threading.Event()
    try:
        fn(*args, FakeSock.stop)
        return None
    except Exception as exc:  # noqa: BLE001 -- the thread would die
        return f"{type(exc).__name__}"
    finally:
        FakeSock.stop = None


def enc(d):
    return d.hex()


def raw(d):
    return d if isinstance(d, bytes) else d.encode("utf-8")


def cancel_timers():
    b._aud_cancel_timer()
    if b._env_change_timer is not None:
        b._env_change_timer.cancel()
        b._env_change_timer = None


def reset_all():
    b.close_virtual_browser(speak_exit=False)
    b.UNITS_MODE = "imperial"
    b.free_cam_active = False
    b.cam_yaw_deg = 0.0
    b.cam_pitch_deg = 0.0
    b.cam_agl = 0.0
    b.cam_agl_valid = True
    b.cam_last_packet_ts = 0.0
    b.cam_veh_bearing = 0.0
    b.cam_veh_distance = -1.0
    b.cam_is_free = False
    b.cam_last_click_heading_deg = 0.0
    b.cam_compass_click_counter = 0
    b.cam_last_announced_compass_idx = -1
    b.cam_last_compass_ts = 0.0
    b.compass_highlight_enabled = True
    b.compass_highlight_nth_click = 6
    b.compass_click_interval_deg = 15.0
    b.compass_min_interval = 0.0
    b._last_vehicle_switch_ts = 0.0
    b.nodegrab_mode_active = False
    b.nodegrab_scanning = False
    b.nodegrab_pad_active = False
    b.nodegrab_last_cid = -1
    b._nodegrab_space_hook = None
    b._nodegrab_space_held = False
    b._clickspot_set_off()
    b.clickspot_last_hover_id = -1
    b.announce_clickspot_actions = False
    b._vehicle_bindings_list = []
    b._vehicle_bindings_vehicle = ""
    b._vehicle_bindings_building = None
    b._vehicle_bindings_building_name = ""
    b._binding_learn_active = False
    b.announce_binding_learn_description = False
    b._env_rows = []
    b._env_level = ""
    b._env_can_change = True
    b._env_building = None
    b._env_unavailable = ""
    b._env_ambient_c = None
    b._env_awaiting_change = False
    b._env_browser_open = False
    b._aud_rows = []
    b._aud_building = None
    b._aud_sfx_rows = []
    b._aud_sfx_building = None
    b._aud_sfx_head = {}
    b._aud_sfx_stack = []
    b._aud_pending = None
    b._aud_pending_start = 0
    b._aud_awaiting = False
    b._aud_browser = None
    b._vinfo_rows = []
    b._vinfo_building = None
    b._vinfo_error = None
    b._vinfo_absent = False
    b._vinfo_event.clear()
    b._ui_screen_context = ""
    b._ui_screen_title = ""
    b._page_text_lines = []
    b._page_text_title = ""
    b._page_text_error = None
    b._page_text_absent = False
    b._page_text_event.clear()
    b._vehicle_slots = {}
    b._selected_slots = set()
    b._target_slot = None
    cancel_timers()
    page_replies.clear()


def opt_f(x):
    return None if x is None else fbits(x)


def snap(area, ac):
    if area == "camera":
        return {
            "free_cam_active": b.free_cam_active, "cam_yaw_deg": fbits(b.cam_yaw_deg),
            "cam_pitch_deg": fbits(b.cam_pitch_deg), "cam_agl": fbits(b.cam_agl),
            "cam_agl_valid": b.cam_agl_valid, "cam_veh_bearing": fbits(b.cam_veh_bearing),
            "cam_veh_distance": fbits(b.cam_veh_distance), "cam_is_free": b.cam_is_free,
            "cam_last_click_heading_deg": fbits(b.cam_last_click_heading_deg),
            "cam_compass_click_counter": b.cam_compass_click_counter,
            "cam_last_announced_compass_idx": b.cam_last_announced_compass_idx,
            "click_pitch": opt_f(getattr(ac, "_cam_click_request_pitch_mult", None)),
            "highlight_pitch": opt_f(getattr(ac, "_cam_highlight_request_pitch_mult", None)),
            "click": ac._cam_click_request.is_set(),
            "highlight": ac._cam_highlight_click_request.is_set(),
        }
    if area == "nodegrab":
        return {
            "mode": b.nodegrab_mode_active, "scanning": b.nodegrab_scanning,
            "pad": b.nodegrab_pad_active, "last_cid": b.nodegrab_last_cid,
            "hooked": b._nodegrab_space_hook is not None,
            "warps": [list(w) for w in warps],
            "beep_freq": fbits(ac._node_beep_freq), "beep_reverse": ac._node_beep_reverse,
            "beep_pos": fbits(ac._node_beep_playback_pos),
        }
    if area == "clickspot":
        building = b._clickspot_trigger_building
        return {
            "mode": b.clickspot_mode_active,
            "list": [list(t) for t in b.clickspot_trigger_list],
            "loading": b.clickspot_list_loading,
            "building": None if building is None else [list(building[k]) for k in sorted(building)],
            "expected": b._clickspot_expected_count,
            "hover": b.clickspot_last_hover_id, "menu_index": b.current_clickspot_index,
            "warps": [list(w) for w in warps],
            "beep_reverse": ac._clickspot_beep_reverse,
            "beep_pos": fbits(ac._clickspot_beep_playback_pos),
        }
    if area == "bindings":
        bl = b._vehicle_bindings_building
        return {
            "list": [list(x) for x in b._vehicle_bindings_list],
            "vehicle": b._vehicle_bindings_vehicle,
            "building": None if bl is None else [list(x) for x in bl],
            "building_name": b._vehicle_bindings_building_name,
            "learn_active": b._binding_learn_active,
        }
    if area == "env":
        return {
            "rows": b._env_rows, "level": b._env_level, "can_change": b._env_can_change,
            "building": b._env_building, "unavailable": b._env_unavailable,
            "ambient": opt_f(b._env_ambient_c), "awaiting": b._env_awaiting_change,
            "browser_open": b._env_browser_open,
        }
    if area == "aud":
        return {
            "rows": b._aud_rows, "building": b._aud_building, "sfx_rows": b._aud_sfx_rows,
            "sfx_building": b._aud_sfx_building, "sfx_head": b._aud_sfx_head,
            "sfx_stack": list(b._aud_sfx_stack), "pending": b._aud_pending,
            "pending_start": b._aud_pending_start, "awaiting": b._aud_awaiting,
            "browser": b._aud_browser,
        }
    if area == "vinfo":
        return {
            "rows": b._vinfo_rows,
            "error": list(b._vinfo_error) if b._vinfo_error else None,
            "absent": b._vinfo_absent, "event": b._vinfo_event.is_set(),
        }
    if area == "page":
        return {
            "context": b._ui_screen_context, "screen_title": b._ui_screen_title,
            "lines": list(b._page_text_lines), "title": b._page_text_title,
            "error": list(b._page_text_error) if b._page_text_error else None,
            "absent": b._page_text_absent, "broadcasts": list(broadcasts),
        }
    if area == "slots":
        return {
            "slots": [[k, v["id"], v["name"]] for k, v in b._vehicle_slots.items()],
            "selected": sorted(b._selected_slots), "target": b._target_slot,
        }
    raise ValueError(area)


def vbrowser():
    return {
        "active": b._vbrowser_active, "lines": list(b._vbrowser_lines),
        "index": b._vbrowser_index, "data": list(b._vbrowser_entry_data),
        "enter": b._vbrowser_on_enter is not None, "adjust": b._vbrowser_on_adjust is not None,
        "escape": b._vbrowser_on_escape is not None,
    }


LISTENERS = {
    "camera": (b.camera_listener, True),
    "nodegrab": (b.nodegrab_listener, True),
    "clickspot": (b.clickspot_listener, True),
    "bindings": (b.vehicle_bindings_listener, False),
    "env": (b.environment_listener, False),
    "aud": (b.audio_debug_listener, False),
    "vinfo": (b.vehicle_info_listener, False),
    "slots": (b.slot_listener, False),
    "learn": (b.binding_learn_listener, False),
}

vinfo_replies = []


def fake_vinfo_send(command):
    FakeSock.sent.append([b.VEHICLE_INFO_CMD_PORT, command])
    if vinfo_replies:
        q = FakeSock.queue
        run_listener(b.vehicle_info_listener, [raw(d) for d in vinfo_replies])
        FakeSock.queue = q


b._send_vehicle_info_cmd = fake_vinfo_send


def do_step(step, ac):
    kind = step[0]
    result = None
    died = None
    if kind == "recv":
        fn, wants_ac = LISTENERS[step[1]]
        args = (ac,) if wants_ac else ()
        died = run_listener(fn, [raw(d) for d in step[2]], *args)
    elif kind == "call":
        name, args = step[1], step[2]
        fn = getattr(b, name)
        if name in ("toggle_free_cam_info", "toggle_nodegrab_mode", "toggle_clickspot_mode"):
            args = [ac]
        result = fn(*args)
        if name in ("request_vehicle_info", "request_page_text"):
            result = [result[0], list(result[1]) if result[1] else None]
    elif kind == "enter":
        idx = b._vbrowser_index
        b._vbrowser_on_enter(idx, b._vbrowser_lines[idx], b._vbrowser_entry_data[idx])
    elif kind == "adjust":
        idx = b._vbrowser_index
        b._vbrowser_on_adjust(idx, b._vbrowser_lines[idx], b._vbrowser_entry_data[idx], step[1])
    elif kind == "escape":
        if b._vbrowser_on_escape is not None:
            b._vbrowser_on_escape()
        else:
            b.close_virtual_browser(speak_exit=True)
    elif kind == "drive":
        b._drive_virtual_browser(step[1])
    elif kind == "set":
        setattr(b, step[1], step[2])
    elif kind == "set_mono":
        setattr(b, step[1], time.monotonic() + step[2])
    elif kind == "vinfo_replies":
        vinfo_replies[:] = step[1]
    elif kind == "page_replies":
        page_replies[:] = step[1]
    else:
        raise ValueError(kind)
    return died, result


def ser_step(step):
    if step[0] == "recv":
        return [step[0], step[1], [enc(raw(d)) for d in step[2]]]
    out = []
    for x in step:
        if isinstance(x, set):
            x = sorted(x)
        out.append(deep_fbits(x))
    return out


def case(name, area, steps, units="imperial", **init):
    reset_all()
    b.UNITS_MODE = units
    for k, v in init.items():
        setattr(b, k, v)
    ac = audio.AudioController(LOG)
    assert ac._is_enabled
    out = []
    for step in steps:
        reset_speech()
        died, result = do_step(step, ac)
        cancel_timers()
        rec = {
            "step": ser_step(step),
            "died": died, "result": deep_fbits(result) if result is not None else None,
            "tap": tap(), "sent": [list(s) for s in FakeSock.sent],
            "browser": deep_fbits(vbrowser()), "state": deep_fbits(snap(area, ac)),
        }
        out.append(rec)
    vinfo_replies.clear()
    reset_all()
    return {"name": name, "area": area, "units": units, "init": deep_fbits(init),
            "steps": out}


def cam(*fields):
    return ",".join(str(f) for f in fields)


cases = []

# ---- Camera -------------------------------------------------------------------------------
for units in ("imperial", "metric"):
    cases.append(case(f"camera diag {units}", "camera", [
        ("recv", "camera", ["DIAG:12.5,10.0|cam=12.5 ground=10", "DIAG:12.5,nan", "DIAG:ERR|bad",
                            "DIAG:1", "DIAG:3.25,-1.75", b"DIAG:\xff1,2|x", "DIAG:inf,1"]),
        ("call", "send_camera_diag", []),
    ], units=units))
cases.append(case("camera feed off ignores data", "camera", [
    ("recv", "camera", [cam(10, 0, 1, 0, -1, 1)]),
    ("call", "_camera_feed_problem", []),
]))
cases.append(case("camera feed", "camera", [
    ("call", "toggle_free_cam_info", []),
    ("recv", "camera", [
        cam(0, -5.5, 12.25, 90, 33.5, 1), cam(16, 0, 1, 0, -1, 0, 0), cam(44.9, 0, 1, 0, -1, 1, 1),
        cam(46, 0, 1, 0, -1, 1), cam(90, 0, 1, 0, -1, 1), cam(181, 0, 1, 0, -1, 1),
        cam(359, 0, 1, 0, -1, 1), cam(2, 0, 1, 0, -1, 1), cam(30, 0, 1, 0, -1, 1),
        cam(60, 0, 1, 0, -1, 1), cam(75, 0, 1, 0, -1, 1), cam(120, 0, 1, 0, -1, 1),
        cam(135, 0, 1, 0, -1, 1), "nan,0,1,0,-1,1", "1,2,3", "x,0,1,0,-1,1",
        b"\xff0,0,1,0,-1,1", "100,0,1,0,-1,1.0", "200,0,1,0,-1,1,x", " 225 , 1 , 2 , 3 , 4 , 1 ",
        "270,0,1,0,-1,1_0", "inf,0,1,0,-1,1", "315,0,1,0,-1,1",
    ]),
    ("call", "_camera_feed_problem", []),
    ("call", "toggle_free_cam_info", []),
]))
cases.append(case("camera highlight every second click", "camera", [
    ("set", "free_cam_active", True),
    ("recv", "camera", [cam(h, 0, 1, 0, -1, 1) for h in (20, 40, 60, 80, 100, 120)]),
], compass_highlight_nth_click=2))
cases.append(case("camera highlight off", "camera", [
    ("set", "free_cam_active", True),
    ("recv", "camera", [cam(h, 0, 1, 0, -1, 1) for h in (20, 40, 60, 80, 100, 120, 140, 160)]),
], compass_highlight_enabled=False))
cases.append(case("camera compass rate limit", "camera", [
    ("set", "free_cam_active", True),
    ("recv", "camera", [cam(h, 0, 1, 0, -1, 1) for h in (0, 45, 90, 135)]),
], compass_min_interval=1000.0))
cases.append(case("camera zero interval ends the thread", "camera", [
    ("set", "free_cam_active", True),
    ("recv", "camera", [cam(20, 0, 1, 0, -1, 1), cam(90, 0, 1, 0, -1, 1)]),
], compass_click_interval_deg=0.0))
cases.append(case("camera vehicle switch quiet", "camera", [
    ("set", "free_cam_active", True),
    ("set_mono", "_last_vehicle_switch_ts", 1000.0),
    ("recv", "camera", [cam(0, 0, 1, 0, -1, 1), cam(45, 0, 1, 0, -1, 1)]),
]))

# ---- Node grabber ---------------------------------------------------------------------------
cases.append(case("nodegrab messages", "nodegrab", [
    ("recv", "nodegrab", ["PADMODE:1", "PADSYNC:0", "PADMODE:0", "GRAB:1", "GRAB:0", "PINNED",
                          "UNPINNED:3", "UNPIN_FAILED", "NODE_CLEAR", "", "   "]),
    ("recv", "nodegrab", ["NODE:12,ref,front left,bumper,0.75"]),
    ("set", "nodegrab_scanning", True),
    ("recv", "nodegrab", ["NODE:12,ref,front left,bumper,0.75", "NODE:13,b,rear,,0.25",
                          "NODE:1,2,3", "NODE_CLEAR", "NODE_CLEAR"]),
    ("recv", "nodegrab", ["NODE:14,c,top,,2.5"]),
    ("recv", "nodegrab", ["SNAP:100,200,1920,1080", "SNAP:1,2,3,4,77,name,loc,grp,0.1",
                          "SNAP:1,2,3,4,77,name,loc,,-1", "SNAP:1,2", "SNAP:a,2,3,4"]),
    ("recv", "nodegrab", ["NODE:x,a,b,c,0.5", "PINNED"]),
    ("recv", "nodegrab", ["NODE:5,a,b,c,zz"]),
    ("set", "nodegrab_scanning", False),
    ("recv", "nodegrab", ["PADMODE:1", "NODE:9,pad,centre,g,0.5", "PADMODE:0"]),
]))
cases.append(case("nodegrab toggle", "nodegrab", [
    ("call", "toggle_nodegrab_mode", []),
    ("call", "toggle_nodegrab_mode", []),
]))

# ---- Clickspots ---------------------------------------------------------------------------
for announce in (False, True):
    cases.append(case(f"clickspot list announce={announce}", "clickspot", [
        ("call", "open_clickspot_browser", []),
        ("call", "toggle_clickspot_mode", []),
        ("call", "open_clickspot_browser", []),
        ("recv", "clickspot", ["TRIGGER:0,5,stray", "TRIGGER_LIST:3", "TRIGGER:2,30,Horn, loud",
                               "TRIGGER:0,10,Lights", "TRIGGER:5,1,out", "TRIGGER:-1,1,neg",
                               "TRIGGER:x,1,bad", "TRIGGER:1,2", "TRIGGER:0,11,Lights again"]),
        ("call", "open_clickspot_browser", []),
        ("recv", "clickspot", ["TRIGGER:1,20,Wipers"]),
        ("call", "open_clickspot_browser", []),
        ("drive", "status_down"),
        ("enter",),
        ("recv", "clickspot", ["HOVER:30,Horn", "HOVER:31,Other", "HOVER_CLEAR", "HOVER_CLEAR",
                               "SNAP_OK:10,20,800,600,30,Horn, loud", "SNAP_OK:1,2,3",
                               "SNAP_FAIL:off screen", "SNAP_FAIL:"]),
        ("recv", "clickspot", ["HOVER:x,bad", "HOVER:1,after"]),
        ("recv", "clickspot", ["SNAP_OK:a,2,3,4,5,six"]),
        ("recv", "clickspot", ["TRIGGER_LIST:1", "TRIGGER_LIST:0", "TRIGGER_LIST:-2",
                               "TRIGGER_LIST:x", "TRIGGER_LIST: 2 "]),
        ("recv", "clickspot", ["TRIGGER_LIST_PENDING"]),
        ("recv", "clickspot", ["TRIGGER_LIST_OFF"]),
        ("call", "open_clickspot_browser", []),
        ("call", "toggle_clickspot_mode", []),
        ("call", "toggle_clickspot_mode", []),
    ], announce_clickspot_actions=announce))

# ---- Vehicle bindings -------------------------------------------------------------------------
cases.append(case("vehicle bindings", "bindings", [
    ("call", "open_vehicle_bindings_browser", []),
    ("recv", "bindings", ["BINDING:0,orphan", "BINDINGS_END", "BINDINGS_BEGIN:3, Gavril D-Series ",
                          "BINDING:4,Toggle 4WD, low range", "BINDING:x,bad", "BINDING:7",
                          "BINDING: 9 ,Horn"]),
    ("call", "open_vehicle_bindings_browser", []),
    ("recv", "bindings", ["BINDINGS_END", "BINDINGS_END"]),
    ("call", "open_vehicle_bindings_browser", []),
    ("enter",),
    ("drive", "status_down"),
    ("enter",),
    ("recv", "bindings", ["BINDINGS_BEGIN:1", "BINDING:2,Only", "BINDINGS_END"]),
    ("call", "open_vehicle_bindings_browser", []),
    ("recv", "bindings", ["BINDINGS_BEGIN:", "BINDINGS_END"]),
    ("call", "open_vehicle_bindings_browser", []),
]))

# ---- Binding learn ------------------------------------------------------------------------
LEARN_ROWS = [
    {"control": "btn_a", "items": [{"title": "Accept", "suppressed": 1},
                                   {"title": "Shift up", "desc": "Next gear", "suppressed": 0},
                                   {"title": "  ", "suppressed": True}, "junk"]},
    {"control": " d-pad right ", "kind": "axis", "title": "Steer", "desc": "Steer right"},
    {"control": "", "kind": "axis", "action": "steer_right"},
    {"control": "btn_x", "unbound": True},
    {"unbound": 1},
    {"control": None, "items": []},
    {"control": "lt", "items": [{"title": "Brake", "desc": "Brake", "suppressed": 1}]},
    {"control": "lt", "items": [{"title": "Brake", "desc": " Slows down ", "suppressed": None}]},
    {"control": "x", "suppressed": None},
    {"control": "x", "items": "notalist", "title": 0, "action": "Fallback"},
    {},
]
for describe in (False, True):
    cases.append(case(f"binding learn describe={describe}", "bindings", [
        ("call", "toggle_binding_learn_mode", []),
        ("recv", "learn", ["LEARN:" + json.dumps(r) for r in LEARN_ROWS]),
        ("recv", "learn", ["LEARN:{bad", "LEARN:[1,2]", "LEARN:null", "LEARNMODE:ON;started",
                           "LEARNMODE: off ", "LEARNMODE:on", "LEARNFAIL:nowrap;Could not wrap",
                           "LEARNFAIL:norestore", "OTHER"]),
        ("recv", "learn", ['LEARN:{"control": 5}', "LEARNMODE:ON"]),
        ("recv", "learn", ['LEARN:{"control": "a", "items": [{"title": ["x"]}]}']),
        ("call", "toggle_binding_learn_mode", []),
        ("call", "toggle_binding_learn_mode", []),
    ], announce_binding_learn_description=describe))

# ---- Environment ----------------------------------------------------------------------------
TEMP = ("key=temperature;kind=numberC;value=15.00;min=-60.0;max=60.0;step=1;label=Temperature;"
        "curveLo=5.00;curveHi=25.00;editable=1;future=x")
RESTORE = "key=restore;kind=action;label=Restore level default;curveLo=15.00;curveHi=15.00;editable=1"
for units in ("imperial", "metric"):
    cases.append(case(f"environment {units}", "env", [
        ("call", "open_environment_browser", []),
        ("call", "speak_ambient_temperature", []),
        ("recv", "env", ["ENV:" + TEMP, "ENV_END", "ENV_BEGIN:2;level=smallgrid;canChange=0",
                         "ENV:" + TEMP, "ENV:label=no key", "ENV:key=;label=empty", "ENV:" + RESTORE,
                         "AMBIENT:valueC=7.25", "ENV_END"]),
        ("call", "speak_ambient_temperature", []),
        ("call", "open_environment_browser", []),
        ("adjust", 1),
        ("recv", "env", ["ENV_BEGIN:2;level=smallgrid", "ENV:" + TEMP.replace("15.00", "15.56"),
                         "ENV:" + RESTORE, "ENV_END"]),
        ("adjust", -10),
        ("call", "_env_change_timed_out", []),
        ("call", "_env_change_timed_out", []),
        ("enter",),
        ("drive", "status_down"),
        ("enter",),
        ("recv", "env", ["ENV_BEGIN:1", "ENV:" + TEMP, "ENV_END"]),
        ("recv", "env", ["ENV_ERROR:Locked by the scenario", "ENV_ERROR:", "AMBIENT:",
                         "AMBIENT:valueC=x"]),
        ("call", "speak_ambient_temperature", []),
        ("recv", "env", ["ENV_UNAVAILABLE: no level loaded "]),
        ("call", "speak_ambient_temperature", []),
        ("call", "open_environment_browser", []),
    ], units=units))
cases.append(case("environment locked and limits", "env", [
    ("recv", "env", ["ENV_BEGIN:3", "ENV:key=temperature;kind=numberC;value=60.00;min=-60;max=60;"
                     "label=Temperature;editable=1",
                     "ENV:key=other;kind=numberC;value=-60;min=-60;max=60;label=Other",
                     "ENV:key=locked;kind=numberC;value=1;label=Locked one;editable=0",
                     "ENV:key=restore;kind=action;label=Restore;editable=0",
                     "ENV:key=noval;kind=numberC;label=No value",
                     "ENV:key=info;kind=info;label=Info",
                     "ENV:key=nolabel;kind=action", "ENV_END"]),
    ("call", "open_environment_browser", []),
    ("adjust", 10),
    ("drive", "status_down"),
    ("adjust", -10),
    ("drive", "status_down"),
    ("adjust", 1),
    ("enter",),
    ("drive", "status_down"),
    ("enter",),
    ("adjust", 1),
    ("drive", "status_down"),
    ("adjust", 1),
    ("enter",),
    ("escape",),
]))

# ---- Audio debug ----------------------------------------------------------------------------
AUD = [
    "AUD_BEGIN:5;soundCount=2",
    "AUD:0;key=sound;kind=select;label=Sound;index=0;count=2;ref=engine;device=mainEngine;blend=V8",
    "AUD:1;key=mainGain;kind=number;label=Main gain;unit=dB;value=-3.5;min=-30;max=10;step=0.5",
    "AUD:2;key=lowShelfFreq;kind=number;label=Low shelf frequency;unit=Hz;value=500;hz=1044;min=0;max=1000;step=10",
    "AUD:3;key=copy;kind=action;label=Copy as jbeam",
    "AUD:4;key=restore;kind=action;label=Restore vehicle defaults;editable=0",
    "AUD:5;key=library;kind=action;label=Sound library",
    "AUD_END",
]
SFX_ROOT = ["AUD_SFX_BEGIN:2;path=;total=3", "AUD_SFX:0;kind=dir;name=UI;count=71",
            "AUD_SFX:1;kind=event;name=Horn;id=event:>Vehicle>Horn", "AUD_SFX:2;kind=event;name=Silent;id=",
            "AUD_SFX_END"]
SFX_UI = ["AUD_SFX_BEGIN:1;path=UI;total=1", "AUD_SFX:0;kind=event;name=Click;id=event:>UI>Click",
          "AUD_SFX_END"]
cases.append(case("audio debug", "aud", [
    ("recv", "aud", ["AUD:0;key=x", "AUD_END", "AUD_SFX:0;kind=dir", "AUD_SFX_END"]),
    ("call", "open_audio_debug_browser", []),
    ("call", "_aud_timed_out", []),
    ("call", "_aud_timed_out", []),
    ("call", "open_audio_debug_browser", []),
    ("recv", "aud", AUD),
    ("adjust", 1),
    ("recv", "aud", [AUD[0]] + [AUD[1].replace("index=0", "index=1") + ";modified=1"] + AUD[2:]),
    ("drive", "status_down"),
    ("adjust", 1),
    ("recv", "aud", [AUD[0], AUD[1], AUD[2].replace("-3.5", "-3"), *AUD[3:]]),
    ("adjust", 100),
    ("recv", "aud", [AUD[0], AUD[1], AUD[2].replace("-3.5", "10"), *AUD[3:]]),
    ("adjust", 1),
    ("enter",),
    ("drive", "status_down"),
    ("enter",),
    ("drive", "status_down"),
    ("enter",),
    ("recv", "aud", ["AUD_DONE:Copied to the clipboard", "AUD_DONE:", "AUD_ERROR:", "AUD_ERROR:Bad thing"]),
    ("drive", "status_down"),
    ("enter",),
    ("drive", "status_down"),
    ("enter",),
    ("recv", "aud", SFX_ROOT),
    ("enter",),
    ("drive", "status_down"),
    ("enter",),
    ("recv", "aud", SFX_UI),
    ("drive", "status_down"),
    ("enter",),
    ("drive", "status_up"),
    ("enter",),
    ("escape",),
    ("recv", "aud", SFX_ROOT),
    ("drive", "status_down"),
    ("drive", "status_down"),
    ("enter",),
    ("escape",),
    ("recv", "aud", ["AUD_BEGIN:2", AUD[1], AUD[3], "AUD_END"]),
    ("recv", "aud", ["AUD_BEGIN:2", AUD[2], AUD[3], "AUD_END"]),
    ("adjust", 1),
    ("recv", "aud", ["AUD_ERROR:The vehicle did not answer", "AUD_BEGIN:2", AUD[2], AUD[3], "AUD_END"]),
    ("escape",),
]))
cases.append(case("audio debug single sound and empty escape", "aud", [
    ("call", "open_audio_debug_browser", []),
    ("recv", "aud", ["AUD_BEGIN:1", "AUD:0;key=sound;kind=select;label=Sound;index=0;count=1;ref=motor",
                     "AUD:1;key=info;kind=info;label=Electric motor", "AUD:2;key=restore;kind=action;label=Restore;editable=1",
                     "AUD_END"]),
    ("adjust", 1),
    ("drive", "status_down"),
    ("adjust", 1),
    ("enter",),
    ("drive", "status_down"),
    ("enter",),
    ("set", "_aud_rows", []),
    ("set", "_aud_pending", "sfx"),
    ("recv", "aud", ["AUD_SFX_BEGIN:0", "AUD_SFX_END"]),
    ("escape",),
]))

# ---- Vehicle info -------------------------------------------------------------------------
VROWS = [
    {"i": 0, "kind": "head", "label": "", "value": "Pessima Track (M)"},
    {"i": 1, "kind": "head", "label": "Brand", "value": " Ibishu "},
    {"i": 2, "kind": "group", "label": "Summary", "value": ""},
    {"i": 3, "kind": "spec", "label": "Weight", "value": "3075 lb"},
    {"i": 4, "label": "Empty", "value": ""},
    {"i": 5, "kind": "spec", "label": "", "value": "Drivetrain: All Wheel Drive"},
    {"i": 6, "label": None, "value": None},
]
VREPLY = ["INFO_BEGIN:7"] + ["INFO_ROW:" + json.dumps(r) for r in VROWS] + ["INFO_ROW:{bad", "INFO_END"]
cases.append(case("vehicle info", "vinfo", [
    ("recv", "vinfo", ["INFO_ROW:{}", "INFO_END"]),
    ("vinfo_replies", VREPLY),
    ("call", "request_vehicle_info", []),
    ("call", "request_vehicle_info", ["pessima", "race"]),
    ("call", "request_vehicle_info", ["etk800", None]),
    ("call", "request_vehicle_info", ["", "x"]),
    ("vinfo_replies", ["INFOFAIL:notselector;not on a vehicle selector screen"]),
    ("call", "request_vehicle_info", []),
    ("vinfo_replies", ["INFOFAIL: nomodel ; a, b; and c "]),
    ("call", "request_vehicle_info", ["x", ""]),
    ("vinfo_replies", ["INFOFAIL:no such vehicle"]),
    ("call", "request_vehicle_info", []),
    ("vinfo_replies", ["INFO_BEGIN:0", "INFOFAIL:;", "INFO_END"]),
    ("call", "request_vehicle_info", []),
    ("vinfo_replies", []),
    ("call", "request_vehicle_info", [None, None, 0.05]),
    ("call", "request_vehicle_info", []),
    ("vinfo_replies", ["INFO_BEGIN:1", "INFO_ROW:" + json.dumps({"kind": "group", "label": "G"}), "INFO_END"]),
    ("call", "request_vehicle_info", []),
]))

# ---- Page text ------------------------------------------------------------------------------
def pr(lines=(), title="", code="", sentence=""):
    return {"lines": list(lines), "title": title, "code": code, "sentence": sentence}


cases.append(case("page text", "page", [
    ("call", "_read_mod_details_page", []),
    ("call", "_on_screen_context", ["mod_details", "Mods"]),
    ("page_replies", [pr(["Name: Cool mod", "Author: someone"], "Cool mod")]),
    ("call", "_read_mod_details_page", []),
    ("page_replies", [pr(["a"], "")]),
    ("call", "_read_mod_details_page", []),
    ("call", "open_page_text_browser", []),
    ("page_replies", [pr(code="notdetails", sentence="Not a details page")]),
    ("call", "_read_mod_details_page", []),
    ("call", "open_page_text_browser", []),
    ("page_replies", [pr(code="empty", sentence="")]),
    ("call", "_read_mod_details_page", []),
    ("call", "open_page_text_browser", []),
    ("page_replies", [pr(code="busy", sentence="Still loading")]),
    ("call", "_read_mod_details_page", []),
    ("page_replies", [pr([], "T")]),
    ("call", "open_page_text_browser", []),
    ("call", "request_page_text", []),
    ("page_replies", []),
    ("call", "request_page_text", [0.05]),
    ("call", "_read_mod_details_page", []),
    ("call", "open_page_text_browser", []),
    ("page_replies", [pr(["x", "y", "z"], "Back")]),
    ("call", "open_page_text_browser", []),
    ("call", "_on_screen_context", ["", ""]),
    ("call", "_read_mod_details_page", []),
]))

# ---- Slots ----------------------------------------------------------------------------------
cases.append(case("slots", "slots", [
    ("recv", "slots", ["SLOTS:1,100,Pessima|2,200,Covet, red|x,1,bad|3,300|4,400,Van|1,101,Again"]),
    ("set", "_selected_slots", {1, 2, 4}),
    ("set", "_target_slot", 4),
    ("recv", "slots", ["SLOTS:2,200,Covet| 4 , 401 , Van ", "NOTSLOTS:1,1,x", "  SLOTS:  "]),
    ("recv", "slots", ["SLOTS:", "SLOTS:1,1,x"]),
    ("set", "_selected_slots", {1, 3}),
    ("set", "_target_slot", 3),
    ("recv", "slots", ["SLOTS:1,5,a|2,6,b"]),
    ("recv", "slots", [b"SLOTS:3,7,\xff", "SLOTS:9,9,never"]),
]))

# ---- Pure phrase tables ---------------------------------------------------------------------
diag = []
for units in ("imperial", "metric"):
    b.UNITS_MODE = units
    for h in ("12.5,10.0", "12.5,nan", "ERR", "1", "", "3.25,-1.75", "nan,1", "1,2,3", "inf,1",
              " 4 , 2 ", "1e400,0", "-0.0,0"):
        diag.append({"units": units, "heights": h, "out": b._format_camera_diag(h)})


def env_line(row):
    try:
        return {"ok": b._env_row_line(row)}
    except Exception as exc:  # noqa: BLE001
        return {"err": type(exc).__name__}


ENV_ROWS = [
    {"key": "temperature", "kind": "numberC", "value": "15.00", "label": "Temperature",
     "curveLo": "15.00", "curveHi": "15.00", "editable": "1"},
    {"key": "temperature", "kind": "numberC", "value": "15.00", "label": "Temperature",
     "curveLo": "5.00", "curveHi": "25.00"},
    {"key": "temperature", "kind": "numberC", "value": "15.00", "curveLo": "15.00", "curveHi": "15.50"},
    {"key": "temperature", "kind": "numberC", "value": "15.00", "curveLo": "15.00", "curveHi": "16.50"},
    {"key": "temperature", "kind": "numberC", "value": "15.00", "curveLo": "15.00", "curveHi": "15.49"},
    {"kind": "numberC", "value": "-3.6", "editable": "0"},
    {"kind": "numberC", "value": "", "label": "T"},
    {"kind": "numberC", "value": "nan", "label": "T"},
    {"kind": "numberC", "value": "inf", "label": "T"},
    {"kind": "numberC", "value": "2", "label": "T", "curveLo": "nan", "curveHi": "1"},
    {"kind": "numberC", "value": "2", "label": "T", "curveLo": "x", "curveHi": "1"},
    {"kind": "action", "key": "restore", "label": "Restore", "curveLo": "15.00", "curveHi": "15.00"},
    {"kind": "action", "key": "restore", "label": "Restore", "curveLo": "5.00", "curveHi": "25.00"},
    {"kind": "action", "key": "restore", "label": "Restore", "curveLo": "5.00"},
    {"kind": "action", "key": "restore", "label": "Restore", "curveHi": "5.00"},
    {"kind": "action", "label": "R", "curveLo": "2.5", "curveHi": "inf"},
    {"kind": "info", "label": "Plain"},
    {"key": "k"},
    {},
    {"kind": "numberC", "value": "  21.5  ", "label": "Spaced"},
    {"kind": "numberC", "value": "-17.5", "label": "Half"},
    {"kind": "numberC", "value": "0.5", "label": "Half"},
    {"kind": "numberC", "value": "2.5", "label": "Half"},
]
env_lines = []
for units in ("imperial", "metric"):
    b.UNITS_MODE = units
    for row in ENV_ROWS:
        env_lines.append({"units": units, "row": row, "out": env_line(row)})
env_parse = [{"payload": p, "out": b._parse_env_fields(p)} for p in (
    "", ";", "2;level=smallgrid;canChange=1", "a=1;a=2", " k = v = w ;bare;bare2", "x=;=y",
    "key=temperature;kind=numberC;value=15.00", "count=3;7", "7;count=3",
)]
env_disp = []
for units in ("imperial", "metric"):
    b.UNITS_MODE = units
    for v in (59, 60, -40, 0, 212, 1):
        env_disp.append({"units": units, "v": v, "out": fbits(b._env_temp_from_display(v))})

vinfo_render = []
for rows in ([], VROWS, [{"kind": "group"}], [{"kind": "spec", "label": "L", "value": 0}],
             [{"kind": "spec", "label": "L", "value": 5}], [5], [{"label": [], "value": "v"}],
             [{"kind": None, "label": "a", "value": "b"}], [{"kind": "group", "label": "  "}]):
    try:
        vinfo_render.append({"rows": rows, "out": b._vinfo_render(rows)})
    except Exception as exc:  # noqa: BLE001
        vinfo_render.append({"rows": rows, "err": type(exc).__name__})

learn_phrases = []
for describe in (False, True):
    b.announce_binding_learn_description = describe
    for row in LEARN_ROWS + [{"control": 1}, {"control": "a", "items": [{"title": 2}]},
                             {"control": "a", "items": [{"title": "t", "desc": 3}]},
                             {"control": "a", "items": [{"title": "", "desc": "d"}]},
                             {"control": "a", "items": [{"title": "t", "suppressed": 0.0}]},
                             {"control": "a", "items": [{"title": "t", "suppressed": "x"}], "kind": "AXIS"}]:
        try:
            learn_phrases.append({"describe": describe, "row": row, "out": b._binding_learn_phrase(row)})
        except Exception as exc:  # noqa: BLE001
            learn_phrases.append({"describe": describe, "row": row, "err": type(exc).__name__})
b.announce_binding_learn_description = False

# ---- Console history ------------------------------------------------------------------------
tmpdir = tempfile.mkdtemp(prefix="beam_e4_console_")
b.CONSOLE_HISTORY_PATH = os.path.join(tmpdir, "console_history.json")
history_saves = []
for hist in ([], ["dump(be)"], ["a", 'quote " and \\ back', "tab\tnew\nline", "cafÃƒÂ© Ã¢â€šÂ¬ \U0001F600",
                                "\x00\x1f\x7f"], [f"cmd {i}" for i in range(60)]):
    b._save_console_history(hist)
    with open(b.CONSOLE_HISTORY_PATH, "rb") as fh:
        history_saves.append({"history": hist, "bytes": fh.read().decode("utf-8")})
history_loads = []
for content in (b"", b"[]", b'["a", "b"]', b'[1, 2.5, true, null, "x", [1, "a"], {"k": 1}]',
                b'{"a": 1}', b"not json", b'\xef\xbb\xbf["bom"]', b'["\xff"]',
                json.dumps([f"c{i}" for i in range(55)]).encode()):
    with open(b.CONSOLE_HISTORY_PATH, "wb") as fh:
        fh.write(content)
    history_loads.append({"content": content.hex(), "out": b._load_console_history()})
os.remove(b.CONSOLE_HISTORY_PATH)
history_loads.append({"content": None, "out": b._load_console_history()})

# ---- Induction report -----------------------------------------------------------------------
induction = []
for units in ("imperial", "metric"):
    b.UNITS_MODE = units
    for replies in ([], ["DONE"], ["ERROR:no vehicle", "DONE"],
                    ['FI:{"turbo": {"boost": 14.5, "maxBoost": 20}}', "DONE"],
                    ['FI:{"turbo": {"boost": 14.5}}', "FI:{}", "FI:[1]", "garbage", "DONE", "FI:{\"damaged\": true}"],
                    ['FI:{"supercharger": {"type": "roots", "boost": 7}, "twoStep": false}'],
                    ['FI:{"turbo": {"boost": "x"}}']):
        reset_speech()
        FakeSock.queue = [r.encode("utf-8") for r in replies]
        try:
            b.speak_induction_report()
            died = None
        except Exception as exc:  # noqa: BLE001
            died = type(exc).__name__
        induction.append({"units": units, "replies": replies, "tap": tap(), "died": died,
                          "sent": [list(s) for s in FakeSock.sent]})
b.UNITS_MODE = "imperial"

write_fixture("e4_listeners", {
    "cases": cases,
    "camera_diag": diag,
    "env_lines": env_lines,
    "env_parse": env_parse,
    "env_display": env_disp,
    "vinfo_render": vinfo_render,
    "learn_phrases": learn_phrases,
    "history_saves": history_saves,
    "history_loads": history_loads,
    "induction": induction,
})
