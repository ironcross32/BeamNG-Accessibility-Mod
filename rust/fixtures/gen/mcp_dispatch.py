"""Golden cases for beam_engine::mcp_server::dispatch against mcp_server._dispatch.

    uv run python rust/fixtures/gen/mcp_dispatch.py

Every JSON-RPC message goes through the real ``_dispatch`` with stub deps. The console
sender answers synchronously through the real ``console_tap`` (the capture is armed
before the command is sent), so the tools that exec Lua return at once. The reply rule,
which the Rust test implements identically:

* ``CTXLIST``: four contexts then ``CTXEND``.
* ``EXEC|i|code``: the camera read / list chunks get fixed JSON; code containing "boom"
  gets a Lua error; anything else gets ``= <len(code)>`` and one printed line.
* anything else (LOGON, LOGOFF): no reply.

Volatile values (``elapsed_ms``, ``mcp_uptime_s``, log-ring ``t``) are zeroed in the
response text on both sides before comparing; the rest of the text is byte-exact.
"""

from __future__ import annotations

import asyncio
import json
import re
import threading

from _beamtel import beamtel as b
from _common import write_fixture

import mcp_server as ms

calls: list = []
world = {"active": False}

CTX_REPLY = ["CTX|0|GE - Lua", "CTX|1|GE - TorqueScript", "CTX|2|CEF/UI - JS",
             "CTX|3|vehicle 7 pickup (current)", "CTX|4|vehicle 9 etk800", "CTXEND"]
CAM_POSE = ('{"mode":"orbit","is_free":false,"pos":[1.5,-2,3.25],"yaw_deg":90,'
            '"pitch_deg":-10,"roll_deg":0,"player_veh_id":7}')
CAM_LIST = ('{"global_cameras":["free","orbit"],"vehicle_cameras":["driver","onboard.hood"],'
            '"player_veh_id":7,"active":"orbit"}')


def reply(cmd):
    if cmd == "CTXLIST":
        return CTX_REPLY
    if cmd.startswith("EXEC|"):
        _, _idx, code = cmd.split("|", 2)
        if code == ms._CAM_READ_LUA:
            return ["RESP|ok", "OUT|= " + CAM_POSE, "EXECEND"]
        if code == ms._CAM_LIST_LUA:
            return ["RESP|ok", "OUT|= " + CAM_LIST, "EXECEND"]
        if "boom" in code:
            return ["RESP|error|boom happened", "EXECEND"]
        return ["RESP|ok", "OUT|= %d" % len(code), "OUT|printed line", "EXECEND"]
    return []


def send_console(cmd):
    calls.append(["console", cmd])
    for rec in reply(cmd):
        ms.console_tap(rec)


ms.init(ms.Deps(
    # What say() would speak: beamtel's own normaliser, as the real say_fn runs it.
    say_fn=lambda text, interrupt=True: (
        (n := b._normalize_speech_value(text, "mcp")) is not None
        and calls.append(["say", n, interrupt])),
    get_speech_log_fn=lambda since_seq=None, last_n=50, source=None, contains=None,
    spoken_only=False: {
        "entries": [{"seq": 1, "t": 0.0, "text": "hello", "source": "test",
                     "interrupt": False, "excluded": False, "spoken": True, "reason": None}],
        "next_seq": 1, "dropped": False},
    snapshot_state_fn=lambda sections=None: {
        "liveness": {"world_active": world["active"]},
        "requested": sorted(sections) if sections else None},
    load_config_fn=lambda: {"units": "imperial", "telemetry_protocol": "extended",
                            "mcp_server_enabled": True, "mcp_server_port": 4481,
                            "ai_describer_api_key": "dpapi:v1:c2VhbGVk",
                            "ai_describer_openai_api_key": ""},
    send_console_command_fn=send_console,
    send_udp_fn=lambda port, msg: calls.append(["udp", port, msg]),
    press_command_fn=lambda name, ctrl=False, shift=False, alt=False: (
        calls.append(["press", name, ctrl, shift, alt]) or True),
    f9_help={("s", False, False, False): "Speak speed",
             ("f", True, False, False): "Fuel, with control"},
    world_active_fn=lambda: world["active"],
    capture_png_fn=None,
    road_diagnostic_fn=lambda **kw: calls.append(["road_diag", kw]) or {"active": False},
    assistant_diagnostic_fn=lambda **kw: calls.append(["assistant_diag", kw])
    or {"recording": False},
    stop_event=threading.Event(),
    logger=None,
    version="test",
))

VOLATILE = re.compile(r'("(?:elapsed_ms|mcp_uptime_s|t)": )-?[0-9.e+-]+')


def normalise(resp):
    if isinstance(resp, dict):
        res = resp.get("result")
        if isinstance(res, dict):
            for c in res.get("content") or []:
                if isinstance(c, dict) and isinstance(c.get("text"), str):
                    c["text"] = VOLATILE.sub(r"\g<1>0", c["text"])
    return resp


def run(msg, world_active=False):
    world["active"] = world_active
    calls.clear()
    out = {"message": msg, "world": world_active}
    try:
        out["response"] = normalise(asyncio.run(ms._dispatch(msg)))
    except Exception as exc:  # noqa: BLE001
        out["raised"] = [type(exc).__name__, str(exc)]
    out["calls"] = json.loads(json.dumps(calls, default=str))
    return out


def call(name, args=None, world_active=False, id_=7):
    msg = {"jsonrpc": "2.0", "id": id_, "method": "tools/call",
           "params": {"name": name, "arguments": args if args is not None else {}}}
    return run(msg, world_active)


cases = [
    run({"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "2025-03-26"}}),
    run({"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "1999-01-01"}}),
    run({"jsonrpc": "2.0", "id": "a", "method": "initialize"}),
    run({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
    run({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
    run({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    run({"jsonrpc": "2.0", "id": 4, "method": "resources/list"}),
    run({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "nope"}}),
    run({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": ["x"]}}),
    run({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": "bad"}),
    run({"jsonrpc": "2.0", "id": 6, "method": "tools/call",
         "params": {"name": "speak", "arguments": "bad"}}),
    run({"jsonrpc": "2.0", "id": None, "method": "ping"}),
    run(["not", "a", "dict"]),
    call("list_commands"),
    call("speak", {"text": "hello there"}),
    call("speak", {"text": "queued", "interrupt": False}),
    call("speak", {}),
    call("speak", {"text": 5}),
    call("speak", {"text": 2.5}),
    call("speak", {"text": True}),
    call("speak", {"text": ["only"]}),
    call("speak", {"text": ["a", "b"]}),
    call("speak", {"text": {"k": "v"}}),
    call("speech_log"),
    call("speech_log", {"since_seq": 3, "last_n": 2, "source": "x", "contains": "h",
                        "spoken_only": True}),
    call("get_state"),
    call("get_state", {"sections": ["liveness", "telemetry"]}),
    call("get_config"),
    call("send_command", {"module": "terrainScanner", "payload": "SCAN"}),
    call("send_command", {"module": "nope", "payload": "X"}),
    call("send_command", {"payload": "X"}),
    call("send_command", {"port": 4500, "payload": "RAW"}),
    call("send_command", {"module": "vehicleScanner"}),
    call("press_command", {"name": "s"}),
    call("press_command", {"name": "f", "ctrl": True}),
    call("press_command", {}),
    call("lua_exec", {"code": "return 1"}),
    call("lua_exec", {"code": "boom()", "context": "ge"}),
    call("lua_exec", {"code": "x", "context": "veh:current"}),
    call("lua_exec", {"code": "x", "context": "veh:etk800"}),
    call("lua_exec", {"code": "x", "context": "veh:nothing"}),
    call("lua_exec", {"code": "x", "context": "mars"}),
    call("lua_exec", {"code": "x", "context": "ui", "verbose": True}),
    call("lua_exec", {}),
    call("lua_contexts"),
    call("health"),
    call("diag", {"name": "player_id"}),
    call("diag", {"name": "nope"}),
    call("console_log", {"action": "on"}),
    call("console_log", {"action": "read"}),
    call("console_log", {"action": "bogus"}),
    call("reset_test_state"),
    call("vehicle_control", {"action": "list_active"}),
    call("vehicle_control", {"action": "bogus"}),
    call("vehicle_control", {}),
    call("road_diagnostic", {"action": "status"}),
    call("road_diagnostic", {"action": "start", "label": "lap"}, world_active=True),
    call("road_diagnostic", {"action": "start"}),
    call("assistant_diagnostic", {"action": "read", "since_seq": 12, "limit": 5}),
    call("assistant_diagnostic", {"action": "start"}),
    call("camera_control", {"action": "nonsense"}),
    call("camera_control", {"action": "get"}),
    call("camera_control", {"action": "get"}, world_active=True),
    call("camera_control", {"action": "list_modes"}, world_active=True),
    call("camera_control", {"action": "set_mode", "mode": "orbit", "settle_ms": 0},
         world_active=True),
    call("camera_control", {"action": "set_mode", "mode": "bad'name"}, world_active=True),
    call("camera_control", {"action": "cycle", "offset": -1, "settle_ms": 0},
         world_active=True),
    call("camera_control", {"action": "place", "orbit_distance_m": 10, "settle_ms": 0},
         world_active=True),
    # screenshot is left out: Python answers "injected no capture function" when the dep
    # is None, a state the Rust Deps (a required capture fn) cannot express.
    # mcp_server_sim covers the real screenshot path.
]

write_fixture("mcp_dispatch", {"cases": cases})
