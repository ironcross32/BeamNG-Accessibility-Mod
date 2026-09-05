"""Replays the MCP server against real HTTP with stub deps -- no game, no beamtel.

`python diagnostic/mcp_server_sim.py`

Two halves. The first stands the server up on a spare port and drives it over real HTTP,
asserting the things a client actually depends on and that a hand-rolled transport is
exactly where you would get wrong: 202-with-an-empty-body for a notification (get this
wrong and the client hangs on the handshake), 405 on GET, a plain application/json body,
protocol-version echo, and isError rather than a JSON-RPC error when a tool fails. It
also drives the console tap directly, since EXEC correlation rests on ordering plus the
EXECEND sentinel and has no request ids to fall back on -- including the assertion that
LOG records are copied but NOT consumed, because consuming them would silently empty
beamtel's own console log pane.

The second half greps beamtel.py, because the hooks this server needs live there and a
hook that is present but in the wrong place fails silently: the speech tap must record
the FINAL text (after the loading suppression and the interrupt demotion), and the
console tap must sit before the wx.CallAfter it is meant to be able to suppress.
"""

import io
import json
import math
import os
import re
import shutil
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

import mcp_server

PORT = 4487
BASE = "http://127.0.0.1:%d" % PORT

failures = []


def check(label, ok, detail=""):
    print(("OK   " if ok else "FAIL ") + label + (("  -- " + str(detail)) if detail and not ok else ""))
    if not ok:
        failures.append(label)


# ---- stub deps -------------------------------------------------------------------
_spoken = []
_sent = []


def _say(text, interrupt=True):
    _spoken.append(text)


def _speech_log(since_seq=None, last_n=50, source=None, contains=None, spoken_only=False):
    return {"entries": [{"seq": 1, "text": "hello", "source": "test", "spoken": True}], "next_seq": 1, "dropped": False}


def _snapshot(sections=None):
    return {"liveness": {"world_active": False, "seconds_since_telemetry": 999.0}}


def _send_console(msg):
    _sent.append(msg)  # nothing ever answers -> exercises the timeout path


def _send_udp(port, msg):
    _sent.append((port, msg))


_captures = []
_road_diagnostics = []
_FAKE_PNG = b"\x89PNG\r\n\x1a\n" + b"x" * 512


_capture_meta = {"focused_game": True, "game_window_found": True,
                 "region": {"left": 0, "top": 0, "width": 1920, "height": 1080}}


def _capture_png(hide_ui=True, settle_s=None, focus_game=False):
    _captures.append((hide_ui, settle_s, focus_game))
    return _FAKE_PNG, dict(_capture_meta)


mcp_server.init(
    mcp_server.Deps(
        say_fn=_say,
        get_speech_log_fn=_speech_log,
        snapshot_state_fn=_snapshot,
        load_config_fn=lambda: {
            "units": "imperial",
            "mcp_server_enabled": True,
            # A sealed key and an unset one, so scenario 14 can assert both answers.
            "ai_describer_api_key": "dpapi:v1:c2VhbGVk",
            "ai_describer_openai_api_key": "",
        },
        send_console_command_fn=_send_console,
        send_udp_fn=_send_udp,
        press_command_fn=lambda *a, **k: True,
        f9_help={("s", False, False, False): "Speak speed"},
        world_active_fn=lambda: False,
        capture_png_fn=_capture_png,
        road_diagnostic_fn=lambda **kwargs: _road_diagnostics.append(kwargs)
        or {"active": False},
        stop_event=threading.Event(),
        logger=None,
        version="test",
    )
)

thread, stop = mcp_server.start(threading.Event(), port=PORT)
time.sleep(1.2)


def post(payload, raw=False):
    data = json.dumps(payload).encode()
    req = urllib.request.Request(
        BASE + "/mcp",
        data=data,
        headers={"Content-Type": "application/json", "Accept": "application/json, text/event-stream"},
    )
    with urllib.request.urlopen(req, timeout=90) as r:
        body = r.read()
        return r.status, r.headers.get("Content-Type"), r.headers, body


# ---- 1. initialize ---------------------------------------------------------------
st, ct, hdrs, body = post(
    {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "0"}},
    }
)
init = json.loads(body)
check("1a initialize returns 200", st == 200, st)
check("1b content-type is application/json", "application/json" in (ct or ""), ct)
check("1c protocolVersion echoed", init["result"]["protocolVersion"] == "2025-06-18", init)
check("1d tools capability advertised", "tools" in init["result"]["capabilities"], init)
check("1e serverInfo present", init["result"]["serverInfo"]["name"] == "beamtel", init)
check("1f no Mcp-Session-Id issued", hdrs.get("Mcp-Session-Id") is None, hdrs.get("Mcp-Session-Id"))
check("1g instructions present", bool(init["result"].get("instructions")))

# unknown protocol version falls back rather than erroring
st, ct, hdrs, body = post(
    {"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}}
)
check("1h unknown version falls back", json.loads(body)["result"]["protocolVersion"] == mcp_server.FALLBACK_PROTOCOL)

# ---- 2. notifications ------------------------------------------------------------
st, ct, hdrs, body = post({"jsonrpc": "2.0", "method": "notifications/initialized"})
check("2a notification returns 202", st == 202, st)
check("2b notification body is empty", body == b"", body)

# ---- 3. tools/list ---------------------------------------------------------------
st, ct, hdrs, body = post({"jsonrpc": "2.0", "id": 3, "method": "tools/list"})
tools = json.loads(body)["result"]["tools"]
check("3a tools/list returns tools", len(tools) >= 10, len(tools))
names = {t["name"] for t in tools}
for expect in (
    "health", "lua_exec", "speech_log", "speak", "press_command", "diag",
    "camera_control", "screenshot", "road_diagnostic",
):
    check("3b tools/list contains " + expect, expect in names)
check("3c every tool has an inputSchema object", all(t["inputSchema"]["type"] == "object" for t in tools))
check("3d every tool has a description", all(t.get("description") for t in tools))

# ---- 4. ping ---------------------------------------------------------------------
st, ct, hdrs, body = post({"jsonrpc": "2.0", "id": 4, "method": "ping"})
check("4a ping returns empty result", json.loads(body)["result"] == {}, body)

# ---- 5. GET / DELETE -------------------------------------------------------------
try:
    urllib.request.urlopen(urllib.request.Request(BASE + "/mcp", method="GET"), timeout=10)
    check("5a GET /mcp returns 405", False, "no error raised")
except urllib.error.HTTPError as e:
    check("5a GET /mcp returns 405", e.code == 405, e.code)

req = urllib.request.Request(BASE + "/mcp", method="DELETE")
with urllib.request.urlopen(req, timeout=10) as r:
    check("5b DELETE /mcp returns 200", r.status == 200, r.status)

with urllib.request.urlopen(BASE + "/health", timeout=10) as r:
    check("5c /health responds", r.status == 200)

# ---- 6. unknown method -----------------------------------------------------------
st, ct, hdrs, body = post({"jsonrpc": "2.0", "id": 6, "method": "resources/list"})
err = json.loads(body)
check("6a unknown method is -32601", err.get("error", {}).get("code") == -32601, err)

# ---- 7. tools that work with no game ---------------------------------------------
def call(name, args=None):
    st, ct, hdrs, body = post(
        {"jsonrpc": "2.0", "id": 99, "method": "tools/call", "params": {"name": name, "arguments": args or {}}}
    )
    return json.loads(body)["result"]

r = call("list_commands")
check("7a list_commands works with no game", not r["isError"], r)
check("7b list_commands returned the F9 entry", "Speak speed" in r["content"][0]["text"], r)

r = call("speak", {"text": "test message"})
check("7c speak works", not r["isError"] and _spoken == ["test message"], _spoken)

r = call("speech_log")
check("7d speech_log works", not r["isError"] and "hello" in r["content"][0]["text"], r)

r = call("get_state")
check("7e get_state works", not r["isError"], r)

r = call("get_config")
check("7f get_config works", not r["isError"], r)

r = call("road_diagnostic", {"action": "status"})
check(
    "7f2 road diagnostic status reaches its injected backend",
    not r["isError"] and _road_diagnostics[-1]["action"] == "status",
    r,
)

r = call("send_command", {"module": "terrainScanner", "payload": "SCAN"})
check("7g send_command routes to the right port", (4472, "SCAN") in _sent, _sent[-3:])

r = call("send_command", {"module": "nope", "payload": "X"})
check("7h unknown module is an isError, not a crash", r["isError"] and "unknown module" in r["content"][0]["text"], r)

# ---- 8. the no-game timeout path -------------------------------------------------
# Nothing ever replies on 4466, so this must fail with a message naming the port --
# promptly, and without wedging the exec lock.
t0 = time.time()
r = call("lua_exec", {"code": "return 1", "timeout_s": 2.0})
elapsed = time.time() - t0
check("8a lua_exec with no game is an isError", r["isError"], r)
check("8b ... names the port and points at health", "4466" in r["content"][0]["text"] and "health" in r["content"][0]["text"], r["content"][0]["text"])
check("8c ... returns promptly rather than hanging", elapsed < 8.0, "%.1fs" % elapsed)

t0 = time.time()
r2 = call("lua_exec", {"code": "return 2", "timeout_s": 2.0})
check("8d exec lock was released (second call behaves the same)", r2["isError"] and "busy" not in r2["content"][0]["text"], r2["content"][0]["text"])
check("8e ... and is still prompt", time.time() - t0 < 8.0)

r = call("health")
check("8f health works with no game", not r["isError"], r)
check("8g health reports console unresponsive", '"console_responsive": false' in r["content"][0]["text"].lower(), r["content"][0]["text"][:400])

# ---- 9. unknown tool -------------------------------------------------------------
st, ct, hdrs, body = post({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "nosuchtool", "arguments": {}}})
check("9a unknown tool is -32602", json.loads(body).get("error", {}).get("code") == -32602, body)

# ---- 10. batch -------------------------------------------------------------------
st, ct, hdrs, body = post([{"jsonrpc": "2.0", "id": 10, "method": "ping"}, {"jsonrpc": "2.0", "id": 11, "method": "ping"}])
arr = json.loads(body)
check("10a batch returns an array of responses", isinstance(arr, list) and len(arr) == 2, arr)

# ---- 11. console tap correlation -------------------------------------------------
# Drive the tap directly: arm a capture in a worker, feed it records, and assert both
# the assembled result and that LOG is copied but NOT consumed.
consumed = []


def feed_later():
    time.sleep(0.4)
    consumed.append(("RESP", mcp_server.console_tap("RESP|ok")))
    consumed.append(("OUTVAL", mcp_server.console_tap("OUT|= 42")))
    consumed.append(("OUTLINE", mcp_server.console_tap("OUT|printed line")))
    consumed.append(("LOG", mcp_server.console_tap("LOG|12:00|I|Test|a log line")))
    consumed.append(("EXECEND", mcp_server.console_tap("EXECEND")))


threading.Thread(target=feed_later, daemon=True).start()
res = mcp_server.exec_console("ge", "return 42", timeout_s=5.0)
check("11a exec assembled the return value", res["result"] == "42", res)
check("11b exec assembled the print output", res["output"] == "printed line", res)
check("11c exec reports ok", res["ok"] and not res.get("timed_out"), res)
tags = dict(consumed)
check("11d RESP/OUT/EXECEND are consumed (kept out of the GUI)", tags["RESP"] and tags["OUTVAL"] and tags["EXECEND"], tags)
check("11e LOG is NOT consumed (still reaches the GUI)", tags["LOG"] is False, tags)

# a long value split across datagrams must concatenate, not newline-join
def feed_chunks():
    time.sleep(0.4)
    mcp_server.console_tap("RESP|ok")
    mcp_server.console_tap("OUT|= " + "A" * mcp_server.MAX_DATAGRAM_CONTENT)
    mcp_server.console_tap("OUT|= " + "B" * 10)
    mcp_server.console_tap("EXECEND")


threading.Thread(target=feed_chunks, daemon=True).start()
res = mcp_server.exec_console("ge", "x", timeout_s=5.0)
check("11f chunked value concatenates", res["result"] == "A" * mcp_server.MAX_DATAGRAM_CONTENT + "B" * 10, len(res["result"]))

# an error reply must come back as ok=False with the message
def feed_err():
    time.sleep(0.4)
    mcp_server.console_tap("RESP|error|attempt to index a nil value")
    mcp_server.console_tap("EXECEND")


threading.Thread(target=feed_err, daemon=True).start()
res = mcp_server.exec_console("ge", "bad", timeout_s=5.0)
check("11g a Lua error surfaces as ok=false with the message", (not res["ok"]) and "nil value" in res["result"], res)

# ---- 12. context resolution ------------------------------------------------------
check("12a ge resolves to 0", mcp_server._resolve_context("ge") == 0)
check("12b ts resolves to 1", mcp_server._resolve_context("ts") == 1)
check("12c ui resolves to 2", mcp_server._resolve_context("ui") == 2)
try:
    mcp_server._resolve_context("garbage")
    check("12d an unknown context is a ToolError", False)
except mcp_server.ToolError:
    check("12d an unknown context is a ToolError", True)

# ---- 14. screenshot --------------------------------------------------------------
# The point of this tool is a FILE the agent can read, so what is asserted is the file:
# that it exists, that its byte count matches what was reported, and that the path came
# back absolute. Nothing is sent anywhere, which is the whole distinction from the AI
# Describer sharing the same capture path.
shot_dir = tempfile.mkdtemp(prefix="beam_shot_")
try:
    r = call("screenshot", {"dir": shot_dir})
    check("14a screenshot works with no game", not r["isError"], r)
    payload = json.loads(r["content"][0]["text"])
    check("14b ... returns an absolute path", os.path.isabs(payload["path"]), payload)
    check("14c ... wrote the file", os.path.isfile(payload["path"]), payload)
    check(
        "14d ... and the reported byte count is the file's",
        payload["bytes"] == os.path.getsize(payload["path"]) == len(_FAKE_PNG),
        payload,
    )
    check("14e ... hides the UI by default", _captures[-1][0] is True, _captures[-1])
    check("14e2 ... and raises the game window by default", _captures[-1][2] is True, _captures[-1])

    r = call("screenshot", {"dir": shot_dir, "hide_ui": False})
    check("14f hide_ui=false is honoured", _captures[-1][0] is False, _captures[-1])

    # Two shots in the same second must not collide, which is what the millisecond
    # stamp is for -- a before/after pair is the main thing this tool gets used for.
    a = json.loads(call("screenshot", {"dir": shot_dir})["content"][0]["text"])["path"]
    b = json.loads(call("screenshot", {"dir": shot_dir})["content"][0]["text"])["path"]
    check("14g back-to-back shots land at different paths", a != b, (a, b))

    r = call("screenshot", {"dir": shot_dir, "filename": "named.png"})
    check("14h an explicit filename is used", os.path.basename(
        json.loads(r["content"][0]["text"])["path"]) == "named.png", r)

    r = call("screenshot", {"dir": shot_dir, "filename": "../escape.png"})
    check("14i a filename containing a path is refused", r["isError"], r)

    # A grab of the desktop, because the game would not come up, looks exactly like a
    # real screenshot to everything except the meta -- so the meta has to ride back.
    _capture_meta.update({"focused_game": False, "game_window_found": False, "region": None})
    r = call("screenshot", {"dir": shot_dir})
    _p = json.loads(r["content"][0]["text"])
    check("14k a failed focus is reported, not silently believed", "warning" in _p, _p)
    check("14l ... and says the game window was not found", _p.get("game_window_found") is False, _p)
    _capture_meta.update({"focused_game": True, "game_window_found": True,
                          "region": {"left": 0, "top": 0, "width": 1920, "height": 1080}})

    r = call("screenshot", {"dir": shot_dir, "focus_game": False})
    check("14m focus_game=false is honoured", _captures[-1][2] is False, _captures[-1])

    r = call("screenshot", {"dir": os.path.join(shot_dir, "sub"), "filename": "deep.png"})
    check("14j a missing directory is created", not r["isError"], r)
finally:
    shutil.rmtree(shot_dir, ignore_errors=True)

# ---- 15. camera_control: argument handling ---------------------------------------
# world_active_fn is False in this harness, so every action must stop at _require_world
# rather than reaching the console -- that guard was previously defined and called by
# nothing, and this tool is its first caller.
r = call("camera_control", {"action": "get"})
check("15a camera_control with no world is an isError", r["isError"], r)
check("15b ... and says so rather than timing out on the console",
      "no live world" in r["content"][0]["text"], r["content"][0]["text"])

r = call("camera_control", {"action": "nonsense"})
check("15c an unknown action is an isError naming the valid ones",
      r["isError"] and "place" in r["content"][0]["text"], r["content"][0]["text"])

# ---- 16. camera_control: the composed Lua ----------------------------------------
# The chunk builders are pure, so they are driven directly. Two things can only be
# caught here: a chunk that overflows one datagram (consoleAccessible's EXEC arrives as
# a single UDP packet), and a percent sign, which this file's own %-formatting would
# mangle on any string that ever routes through it.
cam_cases = {
    "absolute": {"x": 10, "y": 20, "z": 5, "yaw_deg": 90, "pitch_deg": -10},
    "relative": {"rel_fwd_m": -8, "rel_up_m": 3, "look_at": 1234},
    "relative+veh": {"veh_id": 7, "rel_fwd_m": 5, "rel_right_m": 2},
    "orbit": {"orbit_distance_m": 12, "orbit_azimuth_deg": 135, "orbit_elevation_deg": 25},
    "abs+lookat_point": {"x": 1, "y": 2, "z": 3, "look_at": [4, 5, 6]},
}
for _name, _args in cam_cases.items():
    _chunk = mcp_server._cam_place_chunk(_args)
    check("16a %s chunk fits one datagram" % _name,
          len(_chunk) <= mcp_server.MAX_DATAGRAM_CONTENT, len(_chunk))
    check("16b %s chunk carries no percent sign" % _name, "%" not in _chunk)
    # setPosition and setFreeCameraYawPitchRollDeg BOTH early-return unless the free
    # camera is already active (core/camera.lua:1088, :1573), so a place that skipped
    # this would report success and move nothing.
    check("16c %s chunk enters the free camera first" % _name, "isFreeCamera" in _chunk)
for _lua in (mcp_server._CAM_READ_LUA, mcp_server._CAM_LIST_LUA):
    check("16d the fixed chunks fit one datagram", len(_lua) <= mcp_server.MAX_DATAGRAM_CONTENT, len(_lua))
    check("16e the fixed chunks carry no percent sign", "%" not in _lua)

try:
    mcp_server._cam_place_chunk({"x": 1, "y": 2, "z": 3, "rel_fwd_m": 4})
    check("16f mixing two position forms is refused", False, "no error raised")
except mcp_server.ToolError as _e:
    check("16f mixing two position forms is refused", "exactly one position form" in str(_e))
try:
    mcp_server._cam_place_chunk({})
    check("16g a place with no position at all is refused", False, "no error raised")
except mcp_server.ToolError:
    check("16g a place with no position at all is refused", True)

# ---- 17. camera_control: the angle conventions -----------------------------------
# The half of this tool that cannot be seen from the seat. core/camera.lua:1105 builds
# the free camera's forward as (sin y * cos p, cos y * cos p, -sin p) -- yaw measured as
# atan2(f.x, f.y), pitch positive-DOWN -- while cameraInfo.lua:253/258 reports yaw as
# atan2(-f.x, -f.y) and pitch positive-UP. The tool speaks the MOD's convention in both
# directions; a sign error there tilts the camera the wrong way while every readout
# still agrees with itself, so each check below also asserts what the un-converted form
# would have answered.
def _game_forward(yaw_set_deg, pitch_down_deg):
    _y, _p = math.radians(yaw_set_deg), math.radians(pitch_down_deg)
    _cp = math.cos(_p)
    return (math.sin(_y) * _cp, math.cos(_y) * _cp, -math.sin(_p))


def _mod_readback(f):
    _l = math.sqrt(f[0] * f[0] + f[1] * f[1] + f[2] * f[2])
    return math.degrees(math.atan2(-f[0], -f[1])) % 360, math.degrees(math.asin(f[2] / _l))


_worst_y = _worst_p = 0.0
for _wy in (0, 45, 90, 137.5, 180, 270, 359):
    for _wp in (-80, -30, 0, 15, 60):
        _gy, _gp = _mod_readback(_game_forward((_wy - mcp_server.CAM_YAW_OFFSET_DEG) % 360, -_wp))
        _worst_y = max(_worst_y, abs((_gy - _wy + 180) % 360 - 180))
        _worst_p = max(_worst_p, abs(_gp - _wp))
check("17a yaw/pitch round-trip over the whole sphere", _worst_y < 1e-6 and _worst_p < 1e-6,
      (_worst_y, _worst_p))
_naive, _ = _mod_readback(_game_forward(90.0, 0.0))
check("17b ... and the un-offset form is wrong by 180", abs(_naive - 90.0) > 179.0, _naive)
check("17c the yaw offset is the derived 180", mcp_server.CAM_YAW_OFFSET_DEG == 180.0,
      mcp_server.CAM_YAW_OFFSET_DEG)
check("17d pitch_deg positive looks UP",
      _game_forward((0 - mcp_server.CAM_YAW_OFFSET_DEG) % 360, -45.0)[2] > 0.7)
check("17e ... and the un-negated form would look DOWN",
      _game_forward((0 - mcp_server.CAM_YAW_OFFSET_DEG) % 360, 45.0)[2] < -0.7)

# The readback is derived from getForward() exactly the way cameraInfo.lua derives it,
# rather than from core_camera.getYawPitchRoll(), which speaks the engine's convention.
# If these ever diverge, `get` and the Alt+H readout disagree about the same camera.
check("17f the readback uses cameraInfo's own yaw formula",
      "math.atan2(-f.x,-f.y)" in mcp_server._CAM_READ_LUA)
CAMINFO = io.open(os.path.join(ROOT, "bng_mod", "lua", "ge", "extensions", "cameraInfo.lua"), encoding="utf-8").read()
check("17g ... and cameraInfo.lua still uses it too",
      "atan2(-fwd.x, -fwd.y)" in CAMINFO or "atan2(-fwd.x,-fwd.y)" in CAMINFO, "cameraInfo yaw changed")

# ---- 18. get_config must not hand an agent a credential ---------------------------
# Its output lands in agent transcripts and logs, so a key readable through it is a key
# that has left the machine. Masked even now that the stored form is a DPAPI blob: that
# blob is still the credential, in a form that travels.
raw = call("get_config")["content"][0]["text"]
cfg = json.loads(raw)
check("18a a set secret reads back redacted", cfg["ai_describer_api_key"] == "<redacted>", cfg)
check("18b ... and never leaks the stored value",
      "dpapi:v1:c2VhbGVk" not in raw, raw)
# Whether a key is SET is the fact an agent legitimately needs -- it is why the describer
# refuses to run -- and it gives nothing away, so an unset key must NOT read as redacted.
check("18c an unset secret stays empty", cfg["ai_describer_openai_api_key"] == "", cfg)
check("18d ordinary settings are untouched", cfg["units"] == "imperial", cfg)
check("18e the tool description states the contract", "<redacted>" in
      [t for t in tools if t["name"] == "get_config"][0]["description"])

stop()

# ===================================================================================
#  13. Cross-file: the beamtel.py hooks
# ===================================================================================
NL = chr(10)
BEAMTEL = io.open(os.path.join(ROOT, "beamtel.py"), encoding="utf-8").read()

check("13a beamtel defines the speech tap", "SPEECH_TAP = deque(" in BEAMTEL)
check("13b beamtel exposes get_speech_log", "def get_speech_log(" in BEAMTEL)
check("13c beamtel exposes register_console_tap", "def register_console_tap(" in BEAMTEL)
check("13d beamtel exposes the state snapshot", "def _mcp_snapshot_state(" in BEAMTEL)
check("13e beamtel exposes the command injector", "def _mcp_press_command(" in BEAMTEL)

# The tap must record what was actually SAID -- i.e. after the loading-suppression
# return and after the _speech_protected_until interrupt demotion. Recording earlier
# would log text the user never heard, with the wrong interrupt flag.
say_body = BEAMTEL[BEAMTEL.index("def say("):]
say_body = say_body[: say_body.index(NL + "def stop_speech")]
tap_at = say_body.index("_speech_tap_record(t, source, interrupt, exclude_from_buffer, True)")
speak_at = say_body.index("speech.speak(t, bool(interrupt))")
demote_at = say_body.index("_speech_protected_until")
check("13f the spoken tap sits immediately before speech.speak", 0 < speak_at - tap_at < 200, speak_at - tap_at)
check("13g ... and after the interrupt demotion", demote_at < tap_at, (demote_at, tap_at))
check(
    "13h suppressed speech is recorded too, flagged",
    'loading_suppressed' in say_body and 'False, "loading_suppressed"' in say_body,
)

# The console tap has to sit before the wx.CallAfter, or it cannot suppress anything --
# and on_console_message SPEAKS a single-line result, so a missed suppression means
# every agent exec talks over the user.
lst = BEAMTEL[BEAMTEL.index("def console_listener("):]
lst = lst[: lst.index(NL + "# AI Control State")] if (NL + "# AI Control State") in lst else lst[:4000]
check("13i the console tap is installed in the listener", "tap = _mcp_console_tap" in lst)
check(
    "13j ... before the wx.CallAfter it exists to suppress",
    lst.index("tap = _mcp_console_tap") < lst.index("wx.CallAfter(frame.on_console_message"),
)
check("13k ... and consuming skips the forward", "if tap(text):" in lst and "continue" in lst)
check(
    "13l on_console_message still speaks single-line results (the reason for the tap)",
    "if len(lines) == 1:" in BEAMTEL and "say(lines[0].strip())" in BEAMTEL,
)

# Both DEFAULT_CONFIG dicts must carry the keys. configurator.py's copy drops any key it
# does not know about on save -- the trap that file documents in its own comments.
CONFIGURATOR = io.open(os.path.join(ROOT, "configurator.py"), encoding="utf-8").read()
for key in ("mcp_server_enabled", "mcp_server_port"):
    check("13m beamtel DEFAULT_CONFIG has " + key, '"%s"' % key in BEAMTEL)
    check("13n configurator DEFAULT_CONFIG has " + key, '"%s"' % key in CONFIGURATOR)
check("13o configurator coerces the port to int", '_coerce("mcp_server_port", int' in CONFIGURATOR)

# The server must be off unless asked, and must not be able to stop beamtel starting.
check("13p the server is gated on the config flag", 'cfg.get("mcp_server_enabled", False)' in BEAMTEL)
check("13q a start failure only logs", "Failed to start MCP server" in BEAMTEL)
check("13r the tap is dropped on shutdown", "register_console_tap(None)" in BEAMTEL)

# The screenshot tool and the AI Describer share one capture path, and the lock around
# it is NOT the describer's busy flag: two overlapping hide/grab/show sequences race, so
# the first one's SHOW lands mid-grab of the second and the "clean" shot has the HUD in
# it. _describer_busy exists to make a double-press of F10+Space buzz rather than queue
# a second AI request, which is a different job and must not refuse an agent screenshot.
check("13t beamtel exposes the shared capture helper", "def capture_scene_png(" in BEAMTEL)
check("13u ... injected into the MCP deps", "capture_png_fn=capture_scene_png" in BEAMTEL)
check("13v ... guarded by its own lock, not the describer's busy flag",
      "_capture_lock = threading.Lock()" in BEAMTEL
      and "with _capture_lock:" in BEAMTEL)
_cap = BEAMTEL[BEAMTEL.index("def capture_scene_png("):]
_cap = _cap[: _cap.index(NL + "# =========================")]
check("13w ... and the UI is restored in a finally", "finally:" in _cap and '_send_ui_command("SHOW")' in _cap)
check("13x the describer goes through the same helper, keeping one copy of the toggle",
      "png, _cap_meta = capture_scene_png(hide_ui=toggle_ui)" in BEAMTEL)

# A minimized BeamNG parks its window off-screen and renders nothing, so an agent grab
# returns the desktop -- silently, and looking entirely like a real screenshot. The
# describer must NOT default to raising the window (it fires mid-drive, where a z-order
# change is a real cost and the game is already in front); the agent tool must.
check("13y beamtel can find the game window", "def _find_game_window(" in BEAMTEL)
check("13z ... raise it", "def _raise_game_window(" in BEAMTEL)
check("13aa ... and measure it, rejecting the -32000 minimized park",
      "def _window_region(" in BEAMTEL and "-30000" in BEAMTEL)
check("13ab the describer does not raise the window", "def capture_scene_png(hide_ui=True, settle_s=UI_HIDE_SETTLE_S, focus_game=False)" in BEAMTEL)
check("13ac the previous foreground window is put back", "SetForegroundWindow(prev)" in BEAMTEL)
DESCRIBER = io.open(os.path.join(ROOT, "ai_describer.py"), encoding="utf-8").read()
check("13ad ai_describer can grab a region", "def capture_region(" in DESCRIBER)
check("13ae ... and the whole-monitor form still exists for the describer",
      "def capture_primary_monitor(" in DESCRIBER)

# ---- 19. the config's secrets are sealed at rest ----------------------------------
import secretstore

MCP_SRC = io.open(os.path.join(ROOT, "mcp_server.py"), encoding="utf-8").read()
# One list decides both what is sealed on disk and what is masked by get_config. A second
# copy would drift on the first new provider, and it would drift in the direction that leaks.
check("19a the mask reads secretstore's list, not a copy of it",
      "secretstore.is_secret_setting(" in MCP_SRC and "_SECRET_NAME_PARTS" not in MCP_SRC)

sealed = secretstore.protect("AIzaSy-not-a-real-key")
check("19b protect marks what it produces", secretstore.is_protected(sealed), sealed[:20])
check("19c ... and does not contain the plaintext", "AIzaSy-not-a-real-key" not in sealed)
check("19d round trip", secretstore.unprotect(sealed) == "AIzaSy-not-a-real-key")
check("19e protect is idempotent", secretstore.protect(sealed) == sealed)
# A blob this Windows account cannot open is NOT "no key configured": the fix is to enter
# it again, and "no API key set" would send the user hunting for a key plainly in the file.
# So the failure answer is None, distinct from the "" an empty setting gives.
check("19f a foreign blob is None, not empty",
      secretstore.unprotect(secretstore.PREFIX + "bm90YWJsb2I=") is None)
check("19g an empty setting is empty, not None", secretstore.unprotect("") == "")
# What carries every config written before any of this existed.
check("19h an unmarked value passes through", secretstore.unprotect("legacy-plaintext") == "legacy-plaintext")

# Migration runs on LOAD in BOTH halves, or an install whose owner never re-enters a key
# stays in the clear forever -- and both must write the result back.
check("19i beamtel migrates on load", "secretstore.migrate_config(user)" in BEAMTEL)
check("19j configurator migrates on load", "secretstore.migrate_config(user)" in CONFIGURATOR)
check("19k configurator writes the sealed form back",
      "if secretstore.migrate_config(user):" in CONFIGURATOR
      and "_write_config(CONFIG_PATH, user)" in CONFIGURATOR)
_mig = {"ai_describer_api_key": "plain", "ai_describer_openai_api_key": "", "units": "imperial"}
check("19l migrate seals a plaintext key", secretstore.migrate_config(_mig)
      and secretstore.is_protected(_mig["ai_describer_api_key"]))
check("19m ... leaves an unset one alone", _mig["ai_describer_openai_api_key"] == "")
check("19n ... leaves ordinary settings alone", _mig["units"] == "imperial")
check("19o ... and a second pass is a no-op", secretstore.migrate_config(_mig) is False)
# The UI must seal before storing, and the describer must unseal only at the point of use
# -- a plaintext key left in ai_describer_settings is one a state dump would carry.
CONFIG_UI = io.open(os.path.join(ROOT, "config_ui.py"), encoding="utf-8").read()
check("19p the config UI stores the sealed form",
      "self.cur_cfg[key_cfg] = secretstore.protect(key)" in CONFIG_UI)
check("19q the describer unseals at the point of use",
      "secretstore.unprotect(stored_key)" in BEAMTEL)
check("19r ... and tells the user which failure it hit", "could not be decrypted" in BEAMTEL)

# The default port must not collide with anything the mod already binds.
mod_ports = set(int(m) for m in re.findall(r"_PORT = (\d{4})", BEAMTEL))
check(
    "13s the default MCP port is clear of every mod port",
    mcp_server.DEFAULT_PORT not in mod_ports and mcp_server.DEFAULT_PORT not in (8765, 8766),
    sorted(mod_ports),
)

print()
if failures:
    print("%d FAILURES: %s" % (len(failures), ", ".join(failures)))
    sys.exit(1)
print("all checks passed")
