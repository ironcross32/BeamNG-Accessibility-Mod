"""Golden cases for beam_engine::vehicle_spawner (E8), from vehicle_spawner.py.

    uv run python rust/fixtures/gen/vehicle_spawner.py

Each scenario reloads the real module and drives it with recorders in place of
everything outside it: the 4461 socket (``_cmd_sock``), speech (``say_fn``), the
placement ping (``ping_fn``), the slot getter, the vehicle-information request, the
``keyboard`` package (modifier polls and hook installs only), the clock and
``random.choice``. Steps are datagrams through ``_handle_packet``, key handlers run as
the worker runs them, key releases, F11 toggles, and single passes of the placement
ticker loop. After every step the events it produced (speech, sends, pings, errors) and
a snapshot of the module state (when it changed) are recorded.
"""

from __future__ import annotations

import importlib
import json
import logging
import types

from _common import fbits, write_fixture

import vehicle_spawner as vs0  # noqa: E402

LOG = logging.getLogger("vehicle-spawner-fixture")


class _StopTicker(Exception):
    pass


def _compact(obj):
    # What the Lua's jsonEncode sends: compact, UTF-8 left as is.
    return json.dumps(obj, separators=(",", ":"), ensure_ascii=False)


def cat_item(model, name, brand, typ="Car", configs=None, police=False, **extra):
    d = {
        "i": 0,
        "model": model,
        "name": name,
        "brand": brand,
        "type": typ,
        "bodyStyle": extra.pop("bodyStyle", "Sedan"),
        "drivetrain": extra.pop("drivetrain", "RWD"),
        "propulsion": extra.pop("propulsion", "ICE"),
        "country": extra.pop("country", "United States"),
        "yearMin": 1990,
        "yearMax": 2000,
        "hasPolice": police,
        "configs": configs
        if configs is not None
        else [{"key": "", "name": "(default)", "isDefault": True}],
    }
    d.update(extra)
    return "CATALOG_ITEM:" + _compact(d)


def cfg(key, name, default=False, ctype=""):
    return {"key": key, "name": name, "isDefault": default, "configType": ctype}


CATALOG = [
    "CATALOG_BEGIN:7",
    cat_item(
        "pessima",
        "Pessima",
        "Ibishu",
        configs=[
            cfg("base", "Base", True),
            cfg("gt_ü", "GT über"),
            cfg("police", "Police Interceptor", ctype="Police"),
        ],
        police=True,
        country="Japan",
        drivetrain="FWD",
    ),
    cat_item(
        "etk800",
        "800-Series",
        "ETK",
        configs=[cfg("854t", "854t", True), cfg("856", "856ti")],
        country="Germany",
        bodyStyle="Wagon",
    ),
    # A model name that exercises json.dumps escaping (quote, backslash, tab, astral).
    cat_item("barrier\"\\\t\U0001F697", "Barrier", "", typ="Prop", configs=None),
    cat_item(
        "fullsize",
        "Grand Marshal",
        "Gavril",
        configs=[cfg("v8", "V8", True), cfg("police", "Police", ctype="Police")],
        police=True,
    ),
    cat_item("cone", "Traffic cone", "BeamNG", typ="Prop", propulsion=""),
    cat_item(
        "covet",
        "Covet",
        "Ibishu",
        configs=[cfg("default", "Default", True)],
        country="Japan",
        drivetrain="FWD",
    ),
    cat_item(
        "bx",
        "BX-Series — Zéro",
        "Ibishu",
        configs=[cfg("bx", "BX Coupe", True)],
        country="Japan",
        bodyStyle="Coupe",
    ),
    "CATALOG_END",
]

SLOTS = {"1": {"id": 1001, "name": "Pessima"}, "3": {"id": 1003, "name": "Covet"}}
ACTIVE = "ACTIVE_VEHICLES:1001:pessima;1002:etk800;1003:covet"


def K(*names):
    return [{"op": "key", "name": n} for n in names]


def P(*texts):
    return [{"op": "packet", "text": t} for t in texts]


def MODS(**m):
    return [{"op": "mods", "mods": m}]


def CLOCK(t):
    return [{"op": "clock", "t": t}]


TICK = [{"op": "tick"}]
F11 = [{"op": "f11"}]


def REL(*names):
    return [{"op": "release", "name": n} for n in names]


def open_with_catalog():
    return P(*CATALOG) + F11


SCENARIOS = []


def scenario(name, steps):
    SCENARIOS.append({"name": name, "steps": steps})


scenario(
    "not_ready_and_unfocused",
    [{"op": "focus", "v": False}]
    + F11
    + [{"op": "focus", "v": True}]
    + F11
    + P("CATALOG_BEGIN:1", cat_item("x", "X", "Y"))
    + F11
    + P("CATALOG_END")
    + F11
    + F11,
)

scenario(
    "main_navigation",
    open_with_catalog()
    + K("down", "down", "up", "end", "home", "page down", "page up", "down", "down")
    + K("right", "down", "down", "end", "home", "left", "right", "esc")
    + K("tab")
    + MODS(shift=True)
    + K("tab")
    + MODS()
    + K("tab")
    + P(ACTIVE)
    + K("down", "down", "down", "up", "tab", "esc")
    + F11
    + K("tab", "tab")
    + F11
    + P(ACTIVE)
    + [{"op": "close"}],
)

scenario(
    "queue_and_replace",
    open_with_catalog()
    + K("enter", "down", "enter", "enter")
    + MODS(shift=True)
    + K("enter")
    + MODS()
    + [{"op": "slots", "slots": SLOTS}]
    + MODS(shift=True)
    + K("enter")
    + MODS()
    + K("down", "down", "end", "home", "up", "esc")
    + MODS(shift=True)
    + K("enter")
    + MODS()
    + K("down", "enter", "left", "down", "right", "enter")
    + K("tab", "down", "up", "end", "home", "x")
    + K("down", "enter", "x", "x", "esc", "down", "w", "up", "delete", "delete")
    + K("delete", "delete", "delete", "w", "x"),
)

scenario(
    "filters",
    open_with_catalog()
    + K("f", "down", "down", "end", "home", "page down", "page up", "space")
    + K("down", "space", "space", "enter", "down", "right", "down", "esc")
    + K("c", "f")
    + K("down", "down", "down", "space", "esc")
    + K("f", "down", "down", "down", "down", "down", "space", "down", "space", "enter")
    + K("down", "down", "r")
    + K("tab", "f", "c", "tab", "tab", "f", "c"),
)

scenario(
    "place3d_spawn",
    # main -> configs ETK, queue 854t and 856ti, back to main, Tab to the queue.
    open_with_catalog()
    + K("enter", "enter", "down", "enter", "left", "tab")
    # Anchor list for item 0: current, mark, next. Escape cancels; then "current".
    + K("w", "down", "down", "up", "esc", "w", "end", "home", "enter")
    + CLOCK(1000.0)
    + K("up")
    + REL("up")
    + K("2", "right")
    + REL("right")
    + K("4", "page up")
    + REL("page up")
    + K("3", "page down", "page down")
    + REL("page down")
    + K("1", "left")
    + REL("left")
    + K("q")
    + REL("q")
    + MODS(ctrl=True)
    + K("e")
    + MODS(ctrl=True, shift=True)
    + K("w")
    + REL("e", "w")
    + MODS(shift=True)
    + K("s", "d")
    + MODS()
    + K("a")
    + REL("s", "d", "a")
    + K("space", "x")
    + CLOCK(1001.0)
    + TICK
    + CLOCK(1001.3)
    + TICK
    + TICK
    + K("down")
    + CLOCK(1001.31)
    + TICK
    + CLOCK(1001.6)
    + TICK
    + CLOCK(1001.75)
    + TICK
    + CLOCK(1001.76)
    + TICK
    + CLOCK(1001.9)
    + TICK
    + K("right")
    + CLOCK(1002.3)
    + TICK
    + CLOCK(1003.0)
    + TICK
    + REL("down", "right")
    + CLOCK(1003.5)
    + TICK
    + CLOCK(1004.3)
    + TICK
    + TICK
    + K("4", "down", "down")
    + REL("down")
    + K("down")
    + REL("down")
    + K("3")
    + [{"op": "key", "name": "down"}] * 3
    + REL("down")
    + K("r")
    + CLOCK(1005.0)
    + TICK
    + K("up")
    + REL("up")
    + K("enter")
    # Item 1: current, mark, previous -> previous.
    + K("down", "w", "down", "down", "enter", "up")
    + REL("up")
    + K("enter")
    # Mark a vehicle: the list is empty until ACTIVE_VEHICLES arrives.
    + K("w", "down", "enter", "down")
    + P(ACTIVE)
    + K("down", "end", "enter", "left")
    + REL("left")
    + K("enter")
    + K("w", "down", "enter", "esc")
    + K("w", "esc", "up", "space"),
)

scenario(
    "place3d_holds_two_keys",
    open_with_catalog()
    + K("enter", "enter", "left", "tab", "w", "enter")
    + CLOCK(2000.0)
    + K("up")
    + CLOCK(2000.05)
    + K("right")
    + MODS(ctrl=True)
    + K("q")
    + MODS()
    + K("e")
    + CLOCK(2000.4)
    + TICK
    + CLOCK(2000.43)
    + TICK
    + CLOCK(2000.6)
    + TICK
    + K("up")
    + REL("q", "e")
    + CLOCK(2000.9)
    + TICK
    + REL("up", "right")
    + CLOCK(2002.5)
    + TICK
    + K("enter")
    + K("space"),
)

scenario(
    "teleport",
    open_with_catalog()
    + K("tab", "tab")
    + P(ACTIVE)
    + K("down", "enter", "enter", "enter", "down", "enter")
    + K("a")
    + MODS(ctrl=True)
    + K("a")
    + MODS()
    + K("w", "down", "down", "down", "up", "enter")
    + K("x", "up", "space")
    + REL("up")
    + K("x", "x", "right")
    + REL("right")
    + K("enter")
    + K("w", "enter", "space", "esc")
    + K("w", "end", "enter")
    + P("ACTIVE_VEHICLES:1001:pessima;1002:etk800")
    + K("down", "down", "esc")
    + K("w", "end", "enter", "down", "enter", "page up")
    + REL("page up")
    + K("enter")
    + K("delete", "r", "v", "home")
    + MODS(ctrl=True)
    + K("a")
    + MODS()
    + K("r")
    + P("RELOAD_DONE:3,0")
    + K("delete")
    + P("REMOVE_DONE:2,1", "ACTIVE_VEHICLES:1002:etk800")
    + K("enter", "down", "w", "enter", "x", "enter")
    + K("v")
    + P("ACTIVE_VEHICLES:")
    + K("w", "down", "up"),
)

scenario(
    "queue_ops",
    open_with_catalog()
    + K("enter", "enter", "down", "enter", "left", "down", "enter", "enter", "left", "tab")
    + K("x")
    + [{"op": "slots", "slots": SLOTS}]
    + K("x", "down", "enter", "w", "x", "x", "esc")
    + K("down", "x", "enter", "end", "page up", "page down", "home")
    + K("end", "delete", "delete", "delete", "delete", "w", "x", "space"),
)

scenario(
    "arrange",
    open_with_catalog()
    + K("g")
    + K("down", "down", "down", "down", "down", "up", "end", "home")
    + K("right", "right", "right", "right", "right", "left", "left")
    + K("down", "right", "right", "right", "left", "up")
    + K("down", "down", "enter")
    + K("up", "up", "left", "up", "right", "down", "down", "end", "up", "home", "down")
    + K("esc", "enter")
    + K("down", "enter", "down", "enter", "esc")
    # Queue two, then arrange from the queue page.
    + K("enter", "enter", "down", "enter", "left", "tab", "g")
    + P(ACTIVE)
    + K("right", "right", "right", "right", "down", "down", "down", "down", "enter")
    + K("up", "enter", "esc")
    + P("ACTIVE_VEHICLES:1:a;2:b;3:c;4:d;5:e")
    + K("g", "end")
    + P("PLAYER_VEH_ID:3")
    + K("up", "down", "enter")
    + F11
    + K("g", "home", "left", "down", "right", "down", "down", "enter")
    + F11
    + P("PLAYER_VEH_ID:-1")
    + K("g", "home", "left", "left", "end", "enter")
    + F11
    + K("tab", "g", "esc", "g", "home", "right", "end")
    + P("PLAYER_VEH_ID:7")
    + K("enter"),
)

scenario(
    "info",
    open_with_catalog()
    + [{"op": "info", "lines": ["Name: Pessima", "Brand: Ibishu", "Power: 100 hp"]}]
    + K("i", "down", "down", "down", "up", "home", "end", "esc")
    + K("right", "down", "i", "esc")
    + [{"op": "info", "lines": [], "failure": ["no_model", "That vehicle has no data."]}]
    + K("i")
    + [{"op": "info", "lines": [], "failure": ["timeout", ""]}]
    + K("i", "enter", "left", "tab")
    + [{"op": "info", "lines": ["Config: 856ti"]}]
    + K("i", "down", "esc", "tab", "i", "tab", "tab"),
)

scenario(
    "random_and_digits",
    open_with_catalog()
    + [{"op": "picks", "picks": [3, 1, 0, 5, 2]}]
    + K("r", "r", "right", "r", "r", "esc")
    + K("1", "2", "s", "d", "q", "e", "x", "v", "a", "delete")
    + K("tab", "1", "s", "r", "v", "space"),
)

scenario(
    "packets",
    P(
        "",
        "CATALOG_ERR:not ready",
        "CATALOG_ITEM:{bad json",
        "RELOAD_DONE:2,1",
        "RELOAD_DONE:2",
        "REMOVE_DONE:0,0",
        "REMOVE_DONE:a,b,c",
        "IGNITION_OFF_DONE:1",
        "IGNITION_OFF_DONE:5",
        "IGNITION_OFF_DONE:x",
        "IGNITION_OFF_DONE: 12 ",
        "SPAWN_OK:1",
        "SPAWN_OK:3",
        "SPAWN_OK:",
        "SPAWN_PARTIAL:2,1,some, message",
        "SPAWN_PARTIAL:2",
        "SPAWN_ERR:bad model",
        "TELEPORT_OK",
        "TELEPORT_OK:extra",
        "TELEPORT_LAUNCHED:42.5,2.25",
        "TELEPORT_LAUNCHED:42.5,2.25,1",
        "TELEPORT_LAUNCHED:42.5,2.25,0",
        "TELEPORT_LAUNCHED:0.5,0.05",
        "TELEPORT_LAUNCHED:1.5,0.25,1",
        "TELEPORT_LAUNCHED:fast",
        "TELEPORT_LAUNCHED:12",
        "TELEPORT_LAUNCHED:nan,inf",
        "TELEPORT_LANDED:1.5,1,1",
        "TELEPORT_LANDED:10,8,-2",
        "TELEPORT_LANDED:10,-8,2",
        "TELEPORT_LANDED:10,0.5,0.5",
        "TELEPORT_LANDED:3,0.9,0.9",
        "TELEPORT_LANDED:10,-5,-6,extra",
        "TELEPORT_LANDED:10,5",
        "TELEPORT_LANDED:x,1,1",
        "TELEPORT_LANDED:nan,1,1",
        "TELEPORT_ERR:no vehicle",
        "ARRANGE_OK:1",
        "ARRANGE_OK:4",
        "ARRANGE_OK:-",
        "ARRANGE_ERR:too few",
        "PLAYER_VEH_ID:12",
        "PLAYER_VEH_ID:zz",
        "UNKNOWN:thing",
        "CATALOG_BEGIN:zz",
        "CATALOG_END",
    )
    + F11,
)

scenario(
    "catalog_errors",
    P("CATALOG_BEGIN:2", "CATALOG_ITEM:[1,2]")
    + P(cat_item("a", "A", "B"), "CATALOG_ITEM:{\"model\":\"m\",\"name\":5}")
    + P("CATALOG_END")
    + P("CATALOG_BEGIN:1", cat_item("a", "A", "B"), "CATALOG_END")
    + F11
    + K("right", "esc", "down")
    + P("CATALOG_BEGIN:1", "CATALOG_ITEM:{\"model\":\"solo\"}", "CATALOG_END")
    + K("right", "down", "esc", "r", "i", "home", "right", "enter", "space"),
)

scenario(
    "json_escapes",
    open_with_catalog()
    + K("down", "down", "down", "down", "right", "down", "enter", "left", "down", "right")
    + K("enter", "left", "i", "space"),
)

scenario(
    "drill_index_error",
    open_with_catalog()
    + K("end")
    + P("CATALOG_BEGIN:1", cat_item("a", "A", "B"), "CATALOG_END")
    + K("right", "enter", "down", "right"),
)


def run_scenario(spec):
    vs = importlib.reload(vs0)
    events = []
    st = {
        "mods": {},
        "clock": 1000.0,
        "picks": [],
        "slots": {},
        "focused": True,
        "info": ([], None),
    }

    class FakeSock:
        def sendto(self, data, addr):
            assert addr == ("127.0.0.1", vs.CMD_PORT)
            events.append(["send", data.decode("utf-8")])

        def close(self):
            pass

    class FakeKeyboard:
        KEY_DOWN = "down"
        KEY_UP = "up"

        def is_pressed(self, name):
            return bool(st["mods"].get(name, False))

        def hook_key(self, key, cb, suppress=False):
            return ("hook", key)

        def unhook(self, handle):
            pass

    class FakeRandom:
        def choice(self, seq):
            k = st["picks"].pop(0) if st["picks"] else 0
            return seq[k % len(seq)]

    class FakeEvent:
        def __init__(self):
            self.flag = False
            self.waits = 0

        def set(self):
            self.flag = True

        def clear(self):
            self.flag = False

        def is_set(self):
            return self.flag

        def wait(self, timeout=None):
            if self.waits >= 1:
                raise _StopTicker()
            self.waits += 1
            return True

    vs._cmd_sock = FakeSock()
    vs.keyboard = FakeKeyboard()
    vs._KEYBOARD_OK = True
    vs.time = types.SimpleNamespace(monotonic=lambda: st["clock"], sleep=lambda s: None)
    vs.random = FakeRandom()
    vs._start_place_ticker = lambda: None
    vs._place_active = FakeEvent()

    def fake_info(model, config):
        events.append(["info_req", str(model), config])
        return st["info"]

    vs.init(
        say_fn=lambda t, interrupt=True, exclude_from_buffer=False: events.append(
            ["say", t]
        ),
        is_focused_fn=lambda: st["focused"],
        logger=LOG,
        get_slots_fn=lambda: {
            int(k): dict(v) for k, v in st["slots"].items()
        },
        close_others_fn=lambda: events.append(["close_others"]),
        ping_fn=lambda r, a, p: events.append(["ping", r, fbits(a), fbits(p)]),
        request_info_fn=fake_info,
    )
    handlers = dict(vs._MODAL_KEYS)

    def item_snap(it):
        return {
            "model": it.get("model"),
            "config": it.get("config"),
            "displayName": it.get("displayName"),
            "refMode": it.get("refMode"),
            "refVehId": it.get("refVehId"),
            "replaceSlot": it.get("replaceSlot"),
            "replaceVehId": it.get("replaceVehId"),
            "replaceVehName": it.get("replaceVehName"),
            "vals": [it.get(f) for f in vs.PLACE_DEFAULTS],
        }

    def snap():
        return {
            "screen": vs._screen,
            "modal": vs._modal_open,
            "queue": [item_snap(it) for it in vs._to_spawn],
            "place": [vs._place_values[f] for f in vs.PLACE_DEFAULTS],
            "unit_step": vs._place_unit_step,
            "launch": vs._place_launch_mode,
            "place_mode": vs._place_mode,
            "spacing": vs._spacing_field.value(),
            "readout_pending": vs._place_readout_pending,
            "selected": sorted(vs._manage_selected),
            "place_active": vs._place_active.is_set(),
            "catalog_len": len(vs._catalog),
            "hooks": len(vs._hook_handles),
        }

    out = []
    last = None
    for step in spec["steps"]:
        events.clear()
        op = step["op"]
        try:
            if op == "packet":
                vs._handle_packet(step["text"])
            elif op == "key":
                handlers[step["name"]](
                    types.SimpleNamespace(name=step["name"], event_type="down")
                )
            elif op == "release":
                vs._on_place_release(
                    types.SimpleNamespace(name=step["name"], event_type="up")
                )
            elif op == "f11":
                vs._toggle_modal()
            elif op == "close":
                vs.close_modal()
            elif op == "mods":
                st["mods"] = dict(step["mods"])
            elif op == "clock":
                st["clock"] = step["t"]
            elif op == "tick":
                vs._place_active.waits = 0
                try:
                    vs._place_ticker_loop()
                except _StopTicker:
                    pass
            elif op == "picks":
                st["picks"] = list(step["picks"])
            elif op == "slots":
                st["slots"] = dict(step["slots"])
            elif op == "focus":
                st["focused"] = step["v"]
            elif op == "info":
                fail = step.get("failure")
                st["info"] = (list(step["lines"]), tuple(fail) if fail else None)
            else:
                raise AssertionError(op)
        except Exception:  # noqa: BLE001 -- the worker logs and carries on
            events.append(["error"])
        # The state is recorded only when it changed, to keep the fixture small.
        now = snap()
        out.append(
            {"step": step, "events": list(events), "state": now if now != last else None}
        )
        last = now
    return {"name": spec["name"], "steps": out}


def pure_cases():
    vs = importlib.reload(vs0)
    offsets = []
    for typ, _, variants in vs.ARRANGE_TYPES:
        for vkey, _ in variants + [("bogus", "")]:
            for n in (-2, 0, 1, 2, 4, 5, 9, 13):
                for s in (4.572, 0.3048 * 33) if vkey == "middle" else (4.572,):
                    res = vs._compute_arrangement_offsets(typ, vkey, n, s)
                    offsets.append(
                        {
                            "type": typ,
                            "variant": vkey,
                            "n": n,
                            "s": fbits(s),
                            "out": [[fbits(a), fbits(b)] for a, b in res],
                        }
                    )
    offsets.append(
        {
            "type": "nope",
            "variant": "x",
            "n": 3,
            "s": fbits(1.0),
            "out": [],
        }
    )
    repeat = [
        {"held": fbits(h), "out": fbits(vs._place_repeat_interval(h))}
        for h in (0.0, 0.3, 0.31, 0.5, 1.0, 1.05, 1.8, 2.5, -1.0)
    ]
    cross = []
    for old, new in [
        (0, 1),
        (9, 10),
        (10, 9),
        (99, 100),
        (-99, -100),
        (-1, 1),
        (995, 1005),
        (999, 1000),
        (9999, 10000),
        (12345, 22345),
        (5, -5),
        (-10, 0),
        (150, 160),
    ]:
        cross.append({"old": old, "new": new, "out": vs._place_cross_rung(old, new)})
    return {"offsets": offsets, "repeat": repeat, "cross": cross}


def main():
    fixture = {
        "scenarios": [run_scenario(s) for s in SCENARIOS],
        "pure": pure_cases(),
    }
    write_fixture("vehicle_spawner", fixture)


if __name__ == "__main__":
    main()
