"""Golden cases for beam_engine's E2 listeners (scanner, obstacle, road, fuel, UI toasts,
hill climb), from beamtel.py.

    uv run python rust/fixtures/gen/e2_listeners.py

The real listener functions run against a fake socket: each case's datagrams are handed
to ``recvfrom`` in order, then the stop event is set. Speech is read back from beamtel's
own SPEECH_TAP (so the speech-protection and loading rules are in the golden), commands
from the fake socket's ``sendto``, and audio state from a real ``audio.AudioController``.
"""

from __future__ import annotations

import json
import logging
import math
import socket as socket_mod
import struct
import threading
import time

from _beamtel import beamtel as b
from _common import deep_fbits, fbits, write_fixture

import audio  # noqa: E402

LOG = logging.getLogger("e2-fixture")


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
        # A callable in the queue runs between datagrams (a mode toggle, a setting
        # change), as the command thread would while the listener waits.
        while FakeSock.queue and callable(FakeSock.queue[0]):
            FakeSock.queue.pop(0)()
        if FakeSock.queue:
            return FakeSock.queue.pop(0), ("127.0.0.1", 50000)
        FakeSock.stop.set()
        raise socket_mod.timeout()

    def sendto(self, data, addr):
        FakeSock.sent.append([addr[1], data.decode("utf-8")])

    def close(self):
        pass


socket_mod.socket = FakeSock
b.speech.speak = lambda *args, **kwargs: None

browsers = []
b.open_virtual_browser = lambda lines, **kw: browsers.append(
    {"lines": list(lines), "title": kw.get("title"), "data": list(kw.get("entry_data") or []),
     "on_enter": kw.get("on_enter")}
)
b.close_virtual_browser = lambda speak_exit=True: None
b._command_dispatch = lambda fn, *args, **kwargs: fn(*args, **kwargs)


def reset():
    b.SPEECH_TAP.clear()
    b._speech_protected_until = 0.0
    b._loading_active = False
    b._loading_settling = False
    b._loading_pending_vehicle = ""
    b._command_context = False
    FakeSock.sent.clear()
    FakeSock.queue.clear()
    browsers.clear()


def tap():
    return [
        {
            "text": e["text"],
            "source": e["source"],
            "interrupt": e["interrupt"],
            "excluded": e["excluded"],
            "spoken": e["spoken"],
        }
        for e in b.SPEECH_TAP
    ]


def sent():
    return [list(s) for s in FakeSock.sent]


def run_listener(fn, datagrams, *args):
    FakeSock.queue = list(datagrams)
    FakeSock.stop = threading.Event()
    try:
        fn(*args, FakeSock.stop)
        return None
    except Exception as exc:  # noqa: BLE001 -- the thread would die
        return f"{type(exc).__name__}: {exc}"


def enc(data):
    return data.hex()


def new_audio():
    ac = audio.AudioController(LOG)
    assert ac._is_enabled
    return ac


# ---------------------------------------------------------------------------------------
# Scanner

SCANNER_FIELDS = [
    "last_scanner_target_name", "last_scanner_distance", "last_scanner_approach_deg",
    "last_scanner_bearing", "scanner_ref_reversed", "scan_mode_active", "coupler_dist_mode",
    "coupler_run_active",
]
SCANNER_AUDIO = [
    "_scanner_target_bearing", "_scanner_target_distance", "_scan_mode_active",
    "_coupler_active", "_coupler_bearing", "_coupler_distance", "_coupler_in_range",
]


def scanner_case(name, datagrams, units="imperial", loading=False, **init):
    reset()
    b.UNITS_MODE = units
    b.scan_mode_active = init.get("scan_mode_active", False)
    b.coupler_dist_mode = init.get("coupler_dist_mode", False)
    b.coupler_run_active = init.get("coupler_run_active", False)
    b.last_scanner_target_name = ""
    b.last_scanner_distance = float("inf")
    b.last_scanner_approach_deg = 0.0
    b.last_scanner_bearing = 0.0
    b.scanner_ref_reversed = init.get("scanner_ref_reversed", False)
    b._loading_active = loading
    ac = new_audio()
    raw = [d if isinstance(d, bytes) else d.encode("latin-1") for d in datagrams]
    died = run_listener(b.scanner_listener, raw, ac)
    return {
        "name": name,
        "units": units,
        "loading": loading,
        "init": init,
        "datagrams": [enc(d) for d in raw],
        "died": died,
        "tap": tap(),
        "sent": sent(),
        "state": deep_fbits({k: getattr(b, k) for k in SCANNER_FIELDS}),
        "audio": deep_fbits({k: getattr(ac, k) for k in SCANNER_AUDIO}),
    }


scanner = []
for units in ("imperial", "metric"):
    scanner.append(scanner_case(f"coupler start {units}", [
        "COUPLER_START:ball,tow_bar",
        "COUPLER_START:fifthwheel_v2,kingpin,4.6,0.2",
        "COUPLER_START:a,b,2.5,1.26",
        "COUPLER_START:a,b,x,nan",
        "COUPLER_START:onlyone",
        "COUPLER_START:a,b,inf,0.5",
        "COUPLER_START:a,b, 1.0 ,0.51",
        "COUPLER_START:",
    ], units=units))
    scanner.append(scanner_case(f"coupler dist {units}", [
        "COUPLER_DIST:3.5,0.02", "COUPLER_DIST:3.5,-0.3", "COUPLER_DIST:3.5,0.3",
        "COUPLER_DIST:0.0,0.0152", "COUPLER_DIST:0.0,-0.0153", "COUPLER_DIST:x,1",
        "COUPLER_DIST:12.25,nan", "COUPLER_DIST:7", "COUPLER_DIST:-1,0.04999",
    ], units=units))
scanner.append(scanner_case("coupler tracking", [
    "COUPLER_FAIL:No compatible coupler", "COUPLER:12.5,3.2,1", "COUPLER:-40,0.5,0",
    "COUPLER:1,2,1.0", "COUPLER:1,2", "COUPLER:a,2,1", "COUPLER:9,8, 7 ", "COUPLER_LOST",
    "COUPLER_DIST_MODE:ON", "COUPLER_DIST_MODE:OFF", "COUPLER_DIST_MODE:on",
    "COUPLER_DIST_FAIL:Too far away",
]))
for scan, dist in ((True, True), (True, False), (False, True), (False, False)):
    scanner.append(scanner_case(
        f"coupled scan={scan} dist={dist}", ["COUPLER_START:a,b", "COUPLED_DETECTED:"],
        scan_mode_active=scan, coupler_dist_mode=dist,
    ))
scanner.append(scanner_case("misc", [
    "ATTACH_MONITOR:ON", "ATTACH_MONITOR:OFF", "COUPLER_MODE:ON", "COUPLER_MODE:x",
    "ALIGN_OK", "ALIGN_FAIL:Blocked", "AI_OK:fine", "AI_ERR:boom",
    "AI_STATUS:traffic,0.5,off,on,legal", "AI_STATUS:chase,1,60,off,off,extra", "AI_STATUS:a,b",
    "ASSISTANT:not json", "ASSISTANT:[]",
]))
scanner.append(scanner_case("target name protects", ["TARGET_NAME:Ibishu Pessima", "AI_ERR:x"]))
scanner.append(scanner_case("switched", ["SWITCHED:Gavril D-Series", "ALIGN_OK"],
                            scan_mode_active=True))
scanner.append(scanner_case("switched while loading", ["SWITCHED:Covet"], loading=True))
scanner.append(scanner_case("numbers", [
    "12.5,30.2", "-40,-1.5,90", "10,20,30,-1", "10,20,30,1", "10,20,30,0.5", "10,20,30,-0.5",
    "10,20,30,nan", "nan,5", "5,-3", "", "   ", "1,2,3,4.7", "7", "1,2,x",
    "abc,def!", struct.pack("<ff", 15.0, 42.0), b"\xff\x00\x00\x00\x00\x00\x80?",
    struct.pack("<ff", -7.25, 1e30), "x,1", "1.5e3,2_0",
]))
scanner.append(scanner_case("reverse flag survives a bad tail", ["10,20,30,-1", "11,21,31,nan"]))
scanner.append(scanner_case("infinite reference ends the thread", ["10,20,30,-inf", "ALIGN_OK"]))


# Callouts: one pass of the thread with a stop event that lets exactly one iteration run.
class OneShot:
    def __init__(self):
        self.calls = 0

    def is_set(self):
        self.calls += 1
        return self.calls > 2

    def wait(self, timeout=None):
        return False


callouts = []
for units in ("imperial", "metric"):
    for dist, brg, tname, rev in (
        (12.4, 0.0, "Pessima", False), (12.5, 44.9, "", False), (100.0, 45.0, "Van", True),
        (3.0, 135.1, "T-Series", False), (3.0, -179.0, "", True), (0.0, 90.0, "X", False),
        (0.5, -90.0, "", False), (1.5, 135.0, "", False), (2.5, -45.0, "", True),
        (float("inf"), 0.0, "never", False), (float("nan"), float("nan"), "", False),
        (1234.5, -10.0, "far", True),
    ):
        reset()
        b.UNITS_MODE = units
        b.scanner_distance_callout_enabled = True
        b.scan_mode_active = True
        b.scanner_distance_callout_interval = 10
        b.last_scanner_distance = dist
        b.last_scanner_bearing = brg
        b.last_scanner_target_name = tname
        b.scanner_ref_reversed = rev
        b.scanner_callout_thread_fn(OneShot())
        callouts.append({
            "units": units, "dist": fbits(dist), "brg": fbits(brg), "tname": tname,
            "reversed": rev, "tap": tap(),
        })
b.scanner_distance_callout_enabled = False
b.scan_mode_active = False


# Toggle and gear push.
toggles = []
for word, flags in (("", 0.0), ("Bucket", 1.0), ("Forks", 0.0), ("Grapple", 3.0)):
    reset()
    b.scan_mode_active = False
    b.coupler_run_active = False
    b.scanner_ref_reversed = False
    b._implement_word_current = word
    b.last_implement_flags = flags
    ac = new_audio()
    steps = []
    for _ in range(2):
        b.toggle_scan_mode(ac)
        steps.append({
            "state": deep_fbits({k: getattr(b, k) for k in SCANNER_FIELDS}),
            "audio": deep_fbits({k: getattr(ac, k) for k in SCANNER_AUDIO}),
        })
    toggles.append({"word": word, "flags": fbits(flags), "steps": steps, "tap": tap(),
                    "sent": sent()})
b._implement_word_current = ""
b.last_implement_flags = 0.0
reset()
b.coupler_run_active = True
ac = new_audio()
ac.set_coupler_tracking(True)
b.scan_mode_active = True
b.toggle_scan_mode(ac)
toggle_run_kept = {"coupler_active": ac._coupler_active, "tap": tap(), "sent": sent()}
b.coupler_run_active = False

gear_pushes = []
for gear in ("R", "r", " R1", "D", "N", "", "Reverse", "S3", "P"):
    reset()
    b.obstacle_mode_active = False
    b.scanner_ref_reversed = False
    with b.state_lock:
        b._push_gear_direction(gear)
    gear_pushes.append({"gear": gear, "reversed": b.scanner_ref_reversed, "sent": sent()})


# ---------------------------------------------------------------------------------------
# Obstacles

PACKETS = [
    "0", " 0 ", "", "x", "1", "1,1", "1,1,12.5,220,3.4", "1,1,12.5,220,3.4,2,5.0,0.68,1.71",
    "1,1,12.5,220,3.4,3,5.0,0.68,-0.8", "1,1,12.5,220,3.4,4,5.0,0.68,1.71",
    "1,1,12.5,220,3.4,0,5.0,0.68,1.71", "1,1,12.5,2.5,3.4,2,5.0,0.68,1.71",
    "1,2,-30,80,8,15,240,4", "1,2,-30,240,8,15,240,4", "1,3,1,10,1,2,20,2,3,169,3",
    "1,3,1,10,1,2,170,2", "1,1,0,255,0.1", "1,0,1,2,3", "1,-4,1,2,3", "1,5,1,2", "1,2,1,2,3,4",
    "2,10,200,5", "3,-5,100,12", "2,10,200", "2,x,1,1", "2,1,1.5,1", "4,1,2,3,4", "1,1,a,1,1",
    "1,1,1,1,1,2,a,1,1", " 2 , 1 , 2 , 3 ", "1,1,nan,inf,-inf,2,nan,nan,nan",
    "1,1,1,1_0,1,2,1,1,1", "1,99999999999999999999,1,2,3", "1,1,1,1,1,99999999999999999999,1,1,1",
    "+2,1,2,3", "1,1,1e400,1,1,1,1,1,1",
]


def parse_outcome(text):
    try:
        kind, payload = b.parse_obstacle_packet(text)
    except ValueError as exc:
        return {"err": str(exc)}
    if kind == "terrain":
        payload = list(payload)
    return {"kind": kind, "payload": deep_fbits(payload)}


parse_cases = [{"text": t, "out": parse_outcome(t)} for t in PACKETS]

OBSTACLE_AUDIO = ["_obstacle_hazard", "_terrain_type", "_obstacle_emergency_latched"]


def obstacle_case(name, datagrams):
    reset()
    ac = new_audio()
    raw = [d.encode("utf-8") if isinstance(d, str) else d for d in datagrams]
    died = run_listener(b.obstacle_listener, raw, ac)
    return {
        "name": name,
        "datagrams": [enc(d) for d in raw],
        "died": died,
        "tap": tap(),
        "audio": deep_fbits({k: getattr(ac, k) for k in OBSTACLE_AUDIO}),
    }


obstacle = [
    obstacle_case("terrain announcements", ["2,10,200,5", "2,10,200,5", "3,1,1,1", "3,1,1,1",
                                            "2,1,1,1", "0", "2,1,1,1"]),
    obstacle_case("static then clear", ["1,1,12.5,220,3.4,2,5.0,0.68,1.71", "0"]),
    obstacle_case("emergency", ["1,1,10,255,0.4,3,4,0.1,-0.8"]),
    obstacle_case("legacy", ["1,2,-30,80,8,15,240,4"]),
    obstacle_case("malformed are skipped", ["x", "", "  ", "1,1", "2,1,1", "3,1,1,1"]),
    obstacle_case("bad bytes", [b"2,1,\xff1,1", b"\xff\xfe"]),
]

pushes = []
for proto, gear_str, gear_byte in (("extended", "R", 1), ("extended", None, 0),
                                   ("outgauge", "D", 0), ("outgauge", "R", 2)):
    for force, gear in ((False, None), (True, None), (True, "R"), (True, " r2 "), (False, "")):
        for last, recent in ((None, False), (("F", 0.0, 0.0, 0.0), True),
                             (("F", 0.0, 0.0, 0.0), False), (("R", 0.5, 0.2, 0.0), True)):
            for steer, thr, brk in ((0.0, 0.0, 0.0), (0.02, 0.04, 0.049), (0.03, 0.05, 0.05),
                                    (-2.0, 3.0, -1.0), (float("nan"), 0.123456, 0.9999)):
                reset()
                b.obstacle_mode_active = True
                b.protocol_mode = proto
                b.last_gear_str = gear_str
                b.last_gear_byte = gear_byte
                b.last_steering_input = steer
                b.last_throttle = thr
                b.last_brake = brk
                b._obstacle_state_last = last
                b._obstacle_state_last_sent = time.monotonic() + 1000.0 if recent else 0.0
                result = b._push_obstacle_state(force=force, gear=gear)
                pushes.append({
                    "proto": proto, "gear_str": gear_str, "gear_byte": gear_byte,
                    "force": force, "gear": gear,
                    "last": None if last is None else deep_fbits(list(last)),
                    "recent": recent, "steer": fbits(steer), "thr": fbits(thr),
                    "brk": fbits(brk), "result": result, "sent": sent(),
                    "after": None if b._obstacle_state_last is None
                    else deep_fbits(list(b._obstacle_state_last)),
                })
reset()
b.obstacle_mode_active = False
b._obstacle_state_last = None
b._obstacle_state_last_sent = 0.0
pushes_off = b._push_obstacle_state(force=True)
b.protocol_mode = "outgauge"
b.last_gear_str = None
b.last_gear_byte = None
b.last_steering_input = 0.0
b.last_throttle = 0.0
b.last_brake = 0.0

obstacle_toggles = []
for sens in ("normal", "early", "LATE", "", "weird", "Early"):
    reset()
    b.obstacle_mode_active = False
    b.obstacle_warning_sensitivity = sens
    ac = new_audio()
    b.toggle_obstacle_mode(ac)
    on_audio = ac._obstacle_mode_active
    b.toggle_obstacle_mode(ac)
    obstacle_toggles.append({
        "sensitivity": sens, "tap": tap(), "sent": sent(), "on_audio": on_audio,
        "off_audio": ac._obstacle_mode_active, "last": b._obstacle_state_last,
    })
b.obstacle_warning_sensitivity = "normal"


# ---------------------------------------------------------------------------------------
# Road

def r2(**fields):
    return "R2|" + json.dumps(fields)


ROAD_FLAGS = [
    "road_mode_active", "road_follow_guidance_enabled", "road_junction_speech_enabled",
    "road_stop_sign_speech_enabled", "road_traffic_light_speech_enabled",
    "road_speed_limit_speech_enabled", "road_surface_speech_enabled",
    "road_junction_earcon_enabled",
]


def road_case(name, texts, units="imperial", loading=False, **flags):
    reset()
    b.UNITS_MODE = units
    values = {k: True for k in ROAD_FLAGS}
    values["road_mode_active"] = False
    values.update(flags)
    for k, v in values.items():
        setattr(b, k, v)
    b.ROAD_GUIDANCE_FEED.reset()
    b._loading_active = loading
    ac = new_audio()
    ac.set_road_mode(values["road_mode_active"])
    died = run_listener(b.road_listener, [t.encode("utf-8") for t in texts], ac)
    return {
        "name": name, "units": units, "loading": loading, "flags": values, "texts": texts,
        "died": died, "tap": tap(), "sent": sent(),
        "audio": {"_road_on_road": ac._road_on_road},
        "feed_mode": b.ROAD_GUIDANCE_FEED.snapshot()["mode"],
    }


JUNCTION = {"id": "j1", "phase": "approach", "kind": "tJunction", "distance": 120,
            "exits": [90, -90]}
STOP = {"id": "s1", "kind": "stopSign", "state": "stop", "phase": "approach", "distance": 80}
LIGHT = {"id": "l1", "kind": "trafficLight", "state": "red", "phase": "approach",
         "distance": 150}

road = []
for units in ("imperial", "metric"):
    road.append(road_case(f"surface {units}", [
        "SURFACE|Dirt|1", "SURFACE|Grass|0", "SURFACE|bad", "SURFACE|Dirt road|1",
        "SURFACE|Asphalt|1|extra",
    ], units=units))
road.append(road_case("surface off", ["SURFACE|Dirt|1"], road_surface_speech_enabled=False))
road.append(road_case("surface while loading", ["SURFACE|Dirt|1"], loading=True))
road.append(road_case("dormant road mode", [r2(state="dormant"), r2(state="dormant"),
                                            r2(state="onRoad"), r2(state="dormant")],
                      road_mode_active=True))
road.append(road_case("dormant road off", [r2(state="dormant")]))
for units in ("imperial", "metric"):
    road.append(road_case(f"one way {units}", [
        r2(state="onRoad", oneWay=True, roadDirections=[-170, 25.4]),
    ], units=units, road_mode_active=True))
    road.append(road_case(f"one way ahead {units}", [
        r2(state="onRoad", oneWay=True, roadDirections=[5, -100]),
        r2(state="onRoad", oneWay=True, roadDirections=[5, -100]),
        r2(state="offRoad", offRoad={"bearing": 10, "distance": 4}),
        r2(state="onRoad", oneWay=True, roadDirections=[-12.5], orientation=True),
    ], units=units, road_mode_active=True))
    road.append(road_case(f"junction {units}", [
        r2(state="onRoad", junction=JUNCTION),
        r2(state="onRoad", junction=JUNCTION),
        r2(state="onRoad", junction={**JUNCTION, "id": "j2", "kind": "fork", "exits": [30, -20]}),
        r2(state="onRoad", junction={**JUNCTION, "id": "j3", "kind": "deadEnd", "exits": []}),
        r2(state="onRoad", junction={**JUNCTION, "id": "j4", "kind": "gate"}),
        r2(state="onRoad", junction={**JUNCTION, "id": "j5", "kind": "private"}),
        r2(state="onRoad", junction={**JUNCTION, "id": "j6", "kind": "crossroads",
                                     "exits": [80, 0, -80, 179]}),
        r2(state="onRoad", junction={**JUNCTION, "id": "j7", "phase": "near"}),
        r2(state="onRoad", junction={**JUNCTION, "id": "j7", "phase": "near", "entered": True}),
    ], units=units, road_mode_active=True))
    road.append(road_case(f"signals {units}", [
        r2(state="onRoad", signalStatus="available", signal=STOP, signalContext="m:1"),
        r2(state="onRoad", signalStatus="available", signal=STOP, signalContext="m:1"),
        r2(state="onRoad", signalStatus="available", signal=LIGHT, junction=JUNCTION,
           signalContext="m:1"),
        r2(state="onRoad", signalStatus="available", signal={**LIGHT, "state": "green"},
           signalContext="m:1"),
        r2(state="onRoad", signalStatus="available",
           signal={**LIGHT, "id": "l2", "groupId": "g2", "state": "unknown", "preview": True},
           signalContext="m:1"),
        r2(state="onRoad", signalStatus="available",
           signal={**STOP, "id": "s9", "groupId": "g9", "phase": "near"},
           signalContext="m:1"),
    ], units=units, road_mode_active=True))
road.append(road_case("signals without road mode", [
    r2(state="onRoad", signalStatus="available", signal=STOP),
    r2(state="onRoad", junction=JUNCTION),
]))
road.append(road_case("stop sign speech off", [
    r2(state="onRoad", signalStatus="available", signal=STOP),
    r2(state="onRoad", junction=JUNCTION),
], road_mode_active=True, road_stop_sign_speech_enabled=False))
road.append(road_case("junction speech off", [r2(state="onRoad", junction=JUNCTION)],
                      road_mode_active=True, road_junction_speech_enabled=False))
road.append(road_case("loading silences awareness", [
    r2(state="onRoad", signalStatus="available", signal=STOP),
], road_mode_active=True, loading=True))
road.append(road_case("gate and private (assistant off)", [
    r2(state="onRoad", proximity={"id": "g1", "distance": 40, "speed": 0, "action": "stop",
                                  "source": "stopPoint", "kind": "gate", "privateEntry": "p1"}),
], road_mode_active=True))
road.append(road_case("malformed", [
    'R2|{"state":"flying"}', "R2|not json", 'R2|{"state":"onRoad","roadDirections":[NaN]}',
    "R2|", "garbage", "ON_ROADX", "OFF_ROAD,x,1", "OFF_ROAD,1",
], road_mode_active=True))
road.append(road_case("legacy", [
    "DORMANT", "DORMANT", "ON_ROAD,10,-170", "ON_ROAD", "ON_ROAD,x,1", "OFF_ROAD,45,12.5",
    "OFF_ROAD,-30,-4", "DORMANT",
], road_mode_active=True))
road.append(road_case("legacy masked by R2", [r2(state="onRoad"), "DORMANT", "OFF_ROAD,1,2"],
                      road_mode_active=True))
road.append(road_case("big int junction distance overflows", [
    'R2|{"state":"onRoad","junction":{"id":"x","phase":"approach","distance":1' + "0" * 400
    + "}}", "DORMANT",
], road_mode_active=True))

road_status = []
for mode_active in (False, True):
    b.road_mode_active = mode_active
    b.ROAD_GUIDANCE_FEED.reset()
    reset()
    b.speak_road_status()
    road_status.append({"road_mode_active": mode_active, "tap": tap()})
b.road_mode_active = False

road_toggles = []
for include in (False, True):
    reset()
    b.road_mode_active = False
    b.road_include_private = include
    ac = new_audio()
    b.toggle_road_mode(ac)
    on_audio = ac._road_mode_active
    b.toggle_road_mode(ac)
    road_toggles.append({"include_private": include, "tap": tap(), "sent": sent(),
                         "on_audio": on_audio, "off_audio": ac._road_mode_active})
b.road_include_private = False


# Scripted runs: the real road_listener with mode toggles, assistant handoffs, setting
# changes and loading between packets. Steps are {"packet": text}, {"assistant": bool},
# {"toggle": true}, {"set": [flag, bool]} or {"loading": bool}.

def road_script(name, steps, **flags):
    reset()
    b.UNITS_MODE = "imperial"
    values = {k: True for k in ROAD_FLAGS}
    values["road_mode_active"] = False
    values.update(flags)
    for k, v in values.items():
        setattr(b, k, v)
    b.ROAD_GUIDANCE_FEED.reset()
    b.driving_assistant.active = False
    ac = new_audio()
    ac.set_road_mode(values["road_mode_active"])

    def action(step):
        if "assistant" in step:
            return lambda: setattr(b.driving_assistant, "active", step["assistant"])
        if "toggle" in step:
            return lambda: b.toggle_road_mode(ac)
        if "set" in step:
            return lambda: setattr(b, step["set"][0], step["set"][1])
        if "loading" in step:
            return lambda: setattr(b, "_loading_active", step["loading"])
        raise ValueError(step)

    queue = [s["packet"].encode("utf-8") if "packet" in s else action(s) for s in steps]
    died = run_listener(b.road_listener, queue, ac)
    b.driving_assistant.active = False
    b._loading_active = False
    return {"name": name, "flags": values, "steps": steps, "died": died,
            "tap": tap(), "sent": sent()}


def traffic(state="stop", identity="sign", distance=35):
    # diagnostic/traffic_mode_sim.py's packet.
    return {"packet": r2(state="onRoad", signalContext="test:1", signalStatus="available",
                         signal={"id": identity,
                                 "kind": "stopSign" if state == "stop" else "trafficLight",
                                 "state": state, "distance": distance,
                                 "phase": "near" if distance <= 12 else "approach"})}


def near(target_id, kind="", private=""):
    return {"packet": r2(state="onRoad", proximity={
        "id": target_id, "action": "stop", "source": "stopPoint", "distance": 40,
        "speed": 10, "kind": kind, "privateEntry": private})}


ON, OFF, TOGGLE = {"assistant": True}, {"assistant": False}, {"toggle": True}
road_scripts = [
    # diagnostic/traffic_mode_sim.py, step for step: signal speech follows whichever of
    # road mode and the assistant is on, survives handoffs without repeating a sign,
    # honours both preferences, and is held during loading.
    road_script("traffic mode handoffs", [
        traffic(), ON, traffic(), TOGGLE, traffic(), TOGGLE, traffic(),
        traffic(distance=8), traffic("red", "light"), TOGGLE, traffic("red", "light"),
        OFF, traffic("green", "light"), TOGGLE, traffic("yellow", "light"),
        ON, traffic("yellow", "light"),
        {"set": ["road_stop_sign_speech_enabled", False]}, traffic(identity="other"),
        traffic("green", "light"),
        {"set": ["road_traffic_light_speech_enabled", False]}, traffic("red", "light"),
        {"set": ["road_stop_sign_speech_enabled", True]}, traffic(identity="other"),
        {"loading": True}, traffic(identity="loading"),
    ], road_speed_limit_speech_enabled=False, road_follow_guidance_enabled=False,
        road_junction_speech_enabled=False, road_junction_earcon_enabled=False),
    # The assistant's gate and private-road words: once per id, only while the
    # assistant drives, and an id seen while it was off is not announced later.
    road_script("gate and private entry", [
        ON, near("g1", "gate"), near("g1", "gate"),
        near("j1", private="private:p1"), near("j1", private="private:p1"),
        near("j2", private="private:p1"),
        OFF, near("g2", "gate"), near("j3", private="private:p2"),
        ON, near("g2", "gate"), near("g3", "gate"), near("j4", private="private:p2"),
        near("j5", private="private:p3"),
        OFF, TOGGLE, near("g4", "gate"),
        ON, {"loading": True}, near("g5", "gate"), {"loading": False}, near("g5", "gate"),
    ], road_follow_guidance_enabled=False, road_junction_speech_enabled=False,
        road_junction_earcon_enabled=False),
]


# ---------------------------------------------------------------------------------------
# Fuel

ROWS = [
    {"id": "s1", "name": "Gas One", "official": True, "energy": ["gasoline", "diesel"],
     "distance": 420.5, "bearing": 30, "pumps": 4, "compatible": True},
    {"id": 7, "name": "", "official": False, "energy": ["electricEnergy"], "distance": 0.4,
     "bearing": -170, "compatible": None},
    {"id": "s3", "name": "Far Away", "energy": "bad", "distance": 25000, "bearing": 200,
     "compatible": False, "target": True},
]


def fuel_state():
    return {
        "pending": b._fuel_pending,
        "target_id": b._fuel_target_id,
        "target_name": b._fuel_target_name,
        # How the name is used: `_fuel_target_name or 'the station'`, inside an f-string.
        "target_name_str": str(b._fuel_target_name) if b._fuel_target_name else None,
        "in_range": b._fuel_in_range,
        "parking": b._fuel_parking,
        "rows": deep_fbits(b._fuel_rows),
        "building": None if b._fuel_building is None else deep_fbits(b._fuel_building),
    }


def fuel_reset():
    b._fuel_cancel_timer()
    b._fuel_rows = []
    b._fuel_building = None
    b._fuel_pending = False
    b._fuel_target_id = None
    b._fuel_target_name = None
    b._fuel_in_range = False
    b._fuel_parking = False


def fuel_step(kind, arg=None):
    """One action: a datagram through the listener, or a key press."""
    if kind == "recv":
        run_listener(b.fuel_station_listener, [arg.encode("utf-8")])
    elif kind == "key":
        b.open_fuel_stations(force_list=arg)
    elif kind == "timeout":
        b._fuel_timed_out()
    elif kind == "choose":
        # Enter on line `arg` of the browser the previous step opened.
        browser = fuel_last_browser[0]
        browser["on_enter"](arg, browser["lines"][arg], browser["data"][arg])
    if browsers:
        fuel_last_browser[0] = browsers[-1]
    b._fuel_cancel_timer()


fuel_last_browser = [None]


def fuel_case(name, steps, world=True, units="imperial"):
    reset()
    fuel_reset()
    fuel_last_browser[0] = None
    b.UNITS_MODE = units
    b._last_telemetry_ts = time.time() + 1000.0 if world else 0.0
    out = []
    for kind, arg in steps:
        reset()
        fuel_step(kind, arg)
        out.append({
            "kind": kind, "arg": arg, "tap": tap(), "sent": sent(),
            "browsers": [{"lines": bw["lines"], "title": bw["title"],
                          "data": deep_fbits(bw["data"])} for bw in browsers],
            "state": fuel_state(),
        })
    fuel_reset()
    b._last_telemetry_ts = 0.0
    return {"name": name, "world": world, "units": units, "steps": out}


def row(r):
    return "FS:" + json.dumps(r)


def ev(**fields):
    return "FS_EVENT:" + json.dumps(fields)


fuel = [
    fuel_case("no world", [("key", False), ("key", True)], world=False),
    fuel_case("list and choose", [
        ("key", False), ("recv", "FS_WAIT"), ("recv", "FS_BEGIN:3"), ("recv", row(ROWS[0])),
        ("recv", "FS:{bad"), ("recv", 'FS:{"name":"no id"}'), ("recv", row(ROWS[1])),
        ("recv", row(ROWS[2])), ("recv", "FS_END"), ("choose", 0),
        ("recv", ev(kind="target", id="s1", station="Gas One", here=False)),
        ("key", False), ("recv", ev(kind="in_range", station="Gas One")), ("key", False),
        ("recv", ev(kind="parking", pump=3, side="left", official=True)), ("key", False),
        ("recv", ev(kind="parked", inZone=True, electric=False, pump=3, side="right")),
        ("recv", ev(kind="left_range")), ("key", False),
        ("recv", "FS_BEGIN:1"), ("recv", row(ROWS[0])), ("recv", "FS_END"), ("choose", 0),
        ("recv", ev(kind="cancelled")),
    ], units="imperial"),
    fuel_case("metric rows", [
        ("key", True), ("recv", "FS_BEGIN:3"), ("recv", row(ROWS[0])), ("recv", row(ROWS[1])),
        ("recv", row(ROWS[2])), ("recv", "FS_END"), ("choose", 1),
    ], units="metric"),
    fuel_case("empty list", [("key", False), ("recv", "FS_BEGIN:0"), ("recv", "FS_END")]),
    fuel_case("empty list with a target", [
        ("recv", ev(kind="target", id=5, station="", here=True)), ("key", True),
        ("recv", "FS_BEGIN:0"), ("recv", "FS_END"), ("choose", 0),
    ]),
    fuel_case("rows without a request", [
        ("recv", "FS_BEGIN:1"), ("recv", row(ROWS[0])), ("recv", "FS_END"), ("recv", "FS_END"),
        ("recv", row(ROWS[0])), ("recv", "FS_WAIT"),
    ]),
    fuel_case("timeout", [("key", False), ("timeout", None), ("timeout", None)]),
    fuel_case("events", [
        ("recv", ev(kind="target", id=None, station="X")),
        ("recv", ev(kind="target", id=0, station=["a"], here=1)),
        ("recv", ev(kind="parking", official=False, side="up")),
        ("recv", ev(kind="parked", official=False)),
        ("recv", ev(kind="parked", electric=True)),
        ("recv", ev(kind="park_failed", message="Blocked")),
        ("recv", ev(kind="park_failed")),
        ("recv", ev(kind="park_refused", message=["one"])),
        ("recv", ev(kind="park_refused", message=["one", "two"])),
        ("recv", ev(kind="park_refused", message=0)),
        ("recv", ev(kind="error", message=12)),
        ("recv", ev(kind="park_cancelled")),
        ("recv", ev(kind="scan_done")),
        ("recv", ev(kind="status", x=1)),
        ("recv", ev(kind=5)),
        ("recv", "FS_EVENT:{bad"),
        ("recv", 'FS_EVENT:{"kind":""}'),
        ("recv", "FS_EVENT:[]"),
    ]),
]


# ---------------------------------------------------------------------------------------
# UI toasts

def ui_case(datagrams):
    reset()
    raw = [d.encode("utf-8") if isinstance(d, str) else d for d in datagrams]
    died = run_listener(b.ui_listener, raw)
    return {"datagrams": [enc(d) for d in raw], "died": died, "tap": tap()}


def ui(kind, data):
    return "BEAMTEL_UI " + json.dumps({"kind": kind, "data": data})


ui_cases = [
    ui_case([ui("toastr", {"title": "Saved", "msg": "Your game was saved"})]),
    ui_case([ui("toastr", {"title": "", "msg": "Only message"})]),
    ui_case([ui("toastr", {"title": "  Only title  ", "msg": None})]),
    ui_case([ui("toastr", {"title": 0, "msg": False})]),
    ui_case([ui("toastr", {"title": ["One"], "msg": {"a": "Two"}})]),
    ui_case([ui("toastr", {"title": ["One", "Two"], "msg": 3.0})]),
    ui_case([ui("toastr", {"title": "table: 0x0001ab", "msg": "x"})]),
    ui_case([ui("message", {"msg": "  A message  "})]),
    ui_case([ui("message", {"msg": 42})]),
    ui_case([ui("message", {"msg": True})]),
    ui_case([ui("other", {"msg": "ignored"})]),
    ui_case([ui("message", "not a dict")]),
    ui_case([ui("message", None)]),
    ui_case([ui("message", [])]),
    ui_case(["BEAMTEL_UI [1, 2]", "BEAMTEL_UI {bad", "beamtel_ui {}", " BEAMTEL_UI {}",
             "BEAMTEL_UI {}", 'BEAMTEL_UI {"kind": "message"}']),
    ui_case([b"BEAMTEL_UI " + json.dumps({"kind": "message", "data": {"msg": "café"}})
             .encode("utf-8") + b"\xff"]),
    ui_case([ui("toastr", {"title": "Navigation", "msg": "Please make a U-turn when possible"})]),
    ui_case([ui("message", {"msg": "You have reached your destination."})]),
    ui_case(['BEAMTEL_UI {"kind": "message", "data": {"msg": 1e400}}']),
    ui_case(['BEAMTEL_UI {"kind": "message", "data": {"msg": [100000000000000000000]}}']),
    ui_case(['BEAMTEL_UI {"kind": "message", "data": {"msg": -0.0}}']),
]


# ---------------------------------------------------------------------------------------
# Hill climb

class Capture(logging.Handler):
    def __init__(self):
        super().__init__()
        self.lines = []

    def emit(self, record):
        self.lines.append(record.getMessage())


capture = Capture()
b.logger.addHandler(capture)
quality = []
for summary in (
    {"attempt_id": "a1", "status": "completed",
     "statistics": {"samples": 120, "sample_rate_hz": 19.8, "packet_gaps": {"count": 2}}},
    {},
    {"attempt_id": None, "statistics": {"packet_gaps": None}},
    {"statistics": None, "status": 3},
    {"statistics": {"samples": [1], "packet_gaps": {}}},
    {"attempt_id": "x", "statistics": [1]},
    {"statistics": {"packet_gaps": [2]}},
):
    capture.lines.clear()
    try:
        b._hill_climb_finalized(summary, False)
        out = {"line": [m for m in capture.lines if m.startswith("Hill-climb data")][0]}
    except Exception as exc:  # noqa: BLE001
        out = {"err": type(exc).__name__}
    quality.append({"summary": deep_fbits(summary), "out": out})
b.logger.removeHandler(capture)


write_fixture("e2_listeners", {
    "scanner": scanner,
    "callouts": callouts,
    "toggles": toggles,
    "toggle_run_kept": toggle_run_kept,
    "gear_pushes": gear_pushes,
    "obstacle_parse": parse_cases,
    "obstacle": obstacle,
    "pushes": pushes,
    "pushes_off": pushes_off,
    "obstacle_toggles": obstacle_toggles,
    "road": road,
    "road_status": road_status,
    "road_toggles": road_toggles,
    "road_scripts": road_scripts,
    "fuel": fuel,
    "ui": ui_cases,
    "hill_quality": quality,
    "hill_titles": [{"n": n, "title": f"Hill-climb report history, {n} attempts. Press Enter for details"}
                    for n in (1, 2, 100)],
})
