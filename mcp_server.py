"""MCP server for beamtel -- lets an AI agent drive and test the mod directly.

Almost every feature in this mod is verified one of two ways: an offline simulator in
`diagnostic/` that replays a state machine against a fake clock, or the user physically
driving a machine into position and reporting what they heard. The second half is slow,
and it is the half that catches what the simulators cannot -- wrong node resolves,
plausible-but-wrong numbers, readouts that only misbehave in a real world. This module
is the other end of that: an agent can execute Lua in the game, place vehicles into
known geometry, read live state, trigger any F9 command, and assert on exactly what the
mod announced.

Off by default (`mcp_server_enabled`); binds loopback only.

It **imports nothing from beamtel**. Everything it needs arrives through `init()`, the
same dependency-injection shape `vehicle_spawner.init()` uses, because beamtel imports
this module and the reverse would be a cycle.

Transport is streamable-HTTP MCP (JSON-RPC 2.0 over POST). The `mcp` SDK is deliberately
not a dependency: the Nuitka onefile build fights a 25 MB release limit (h5py was already
dropped for it), aiohttp is already here and already runs a server on a daemon thread
(`nvda_ws_speaker.py`), and the protocol surface this needs is five methods.
"""

import asyncio
import collections
import json
import os
import socket
import threading
import time
import traceback

from aiohttp import web

import secretstore

HOST = "127.0.0.1"
DEFAULT_PORT = 4481  # clear of the 4444-4473 mod block, leaving it room to grow

# The five methods implemented here are identical across all three revisions, so the
# client's own is echoed back rather than forcing one.
SUPPORTED_PROTOCOLS = ("2024-11-05", "2025-03-26", "2025-06-18")
FALLBACK_PROTOCOL = "2025-06-18"
SERVER_NAME = "beamtel"

# consoleAccessible.lua's own datagram cap, mirrored here because reassembly needs it to
# tell a continuation chunk from a fresh print line (see _assemble).
MAX_DATAGRAM_CONTENT = 1400
MAX_RESULT_CHARS = 100000

# GE Lua is synchronous; a vehicle context streams its result back cross-VM in 700-byte
# pieces and its EXECEND lands one or more frames later.
EXEC_TIMEOUT_GE = 5.0
EXEC_TIMEOUT_VEH = 15.0

# `send_command` takes a module name, not a bare port, so a typo is an error rather than
# a datagram into the void.
MODULE_PORTS = {
    "vehicleScanner": 4448,
    "beamtelAI": 4449,
    "cameraInfo": 4451,
    "obstacleDetector": 4453,
    "nodeGrabber": 4455,
    "clickspot": 4457,
    "vehicleSlots": 4459,
    "vehicleSpawner": 4461,
    "roadDetector": 4463,
    "uiToggle": 4464,
    "console": 4465,
    "vehicleBindings": 4468,
    "implementProximity": 4470,
    "terrainScanner": 4472,
}

# Curated shortcuts over the Lua diagnostic entry points. Each value is a Lua expression
# evaluated in the GE context; `{id}` is substituted with the `veh_id` argument.
DIAGNOSTICS = {
    "dock": "extensions.implementProximity.dockTruth()",
    "ramp": "extensions.implementProximity.rampTruth()",
    "ramp_states": "extensions.rampGeometry.diag()",
    "ramp_state_of": "extensions.rampGeometry.stateOf({id})",
    "ramp_retry": "extensions.rampGeometry.retry({id})",
    "mouth_frame": "dumps(extensions.rampGeometry.mouthFrame({id}))",
    "cannon": "extensions.cannonShot.diag()",
    "trailer": "extensions.trailerAngle.diag()",
    "implement_frame": "dumps(extensions.implementProximity.getImplementFrame())",
    "implement_points": "dumps(extensions.implementProximity.getImplementPoints())",
    "scanner_target": "extensions.vehicleScanner.getCurrentTargetID()",
    "player_id": "be:getPlayerVehicleID(0)",
    # terrainScanner.diag() logs via tsLog and returns nil, so this one is wrapped in a
    # LOGON window by _t_diag rather than read from the return value.
    "terrain": "extensions.terrainScanner.diag()",
}

# --- camera conventions -------------------------------------------------------------
# Two conventions collide here, and both are DERIVED from the game source rather than
# measured, because each is the exact algebraic inverse of the other.
#
#   core_camera.setFreeCameraYawPitchRollDeg(yawDeg, pitchDownDeg, rollDeg) builds its
#   forward as (sin(yaw)cos(p), cos(yaw)cos(p), -sin(p))  --  core/camera.lua:1105.
#   So its yaw is atan2(f.x, f.y) and its pitch is positive-DOWN.
#
#   cameraInfo.lua:253 reports yaw as atan2(-f.x, -f.y) normalised to 0..360 (the
#   MotionSim heading convention the whole mod uses) and pitch positive-UP (:258).
#
# atan2(-x, -y) == atan2(x, y) +/- 180, so the two yaws differ by exactly 180 degrees
# with the same sign, and the two pitches differ by a sign alone.
#
# This tool speaks the MOD's convention in both directions, so `get` can never disagree
# with `get_state`, the live 4450 feed, or the Alt+H / Alt+A readouts -- and the readback
# is derived from getForward() the same way cameraInfo.lua derives it, for that reason
# rather than from core_camera.getYawPitchRoll(), which speaks the engine's.
CAM_YAW_OFFSET_DEG = 180.0

# Let the camera settle before reading the pose back, so a set is self-verifying.
CAM_SETTLE_MS_DEFAULT = 150
CAM_SETTLE_MS_MAX = 3000

CAMERA_ACTIONS = ("get", "list_modes", "set_mode", "cycle", "reset", "place")

# Where `screenshot` writes when the caller names no directory -- beside the AI
# Describer's own log, which is the only other thing this mod writes per-run.
SCREENSHOT_DIR_DEFAULT = os.path.join(
    os.getenv("LOCALAPPDATA") or os.path.expanduser("~"), "beamtel", "screenshots"
)
SHOT_SETTLE_MS_MAX = 5000

_LOG_RING_MAX = 2000


class ToolError(Exception):
    """A tool failed in a way the model should see and reason about.

    Surfaces as `isError: true` with the message, never as a JSON-RPC error -- protocol
    errors are reserved for protocol faults.
    """


class Deps:
    """Callables and values injected by beamtel at startup."""

    def __init__(self, **kw):
        self.__dict__.update(kw)


_deps = None
_logger = None
_started_at = 0.0

_RUNNER = None
_LOOP = None

# --- console tap state -------------------------------------------------------------
_exec_lock = threading.Lock()
_tap_lock = threading.Lock()
_capture = None
_log_ring = collections.deque(maxlen=_LOG_RING_MAX)
_log_seq = 0
_last_record_ts = 0.0

_ctx_cache = None  # (monotonic_ts, [(index, label), ...])
_CTX_CACHE_S = 10.0

_input_lock = threading.Lock()


def _log(level, msg):
    if _logger is not None:
        try:
            getattr(_logger, level)("mcp_server: " + msg)
        except Exception:
            pass


# ===================================================================================
#  Console tap / EXEC correlation
# ===================================================================================
#
# consoleAccessible.lua has no request IDs: correlation is ordering plus the EXECEND
# sentinel. Port 4466 is exclusively bound by beamtel's console_listener, so a second
# process could never listen -- hence an in-process tap, and hence `_exec_lock`, which
# keeps exactly one EXEC outstanding so ordering is a sound basis for correlation.


class _Capture:
    __slots__ = ("records", "done", "suppress")

    def __init__(self, suppress):
        self.records = []
        self.done = threading.Event()
        self.suppress = suppress


def console_tap(record):
    """Runs on beamtel's console listener thread. Returns True to consume the record.

    Consuming matters beyond tidiness: `on_console_message` speaks any single-line exec
    result aloud, so without suppression every agent exec would talk over the user.

    LOG records are copied but never consumed -- they arrive unsolicited and belong to
    the GUI's log pane.
    """
    global _log_seq, _last_record_ts
    try:
        tag = record.split("|", 1)[0]
        with _tap_lock:
            _last_record_ts = time.monotonic()
            if tag == "LOG":
                _log_seq += 1
                _log_ring.append({"seq": _log_seq, "t": time.time(), "raw": record})
                if _capture is not None:
                    _capture.records.append(record)
                return False
            cap = _capture
            if cap is None:
                return False
            cap.records.append(record)
            if tag in ("EXECEND", "CTXEND"):
                cap.done.set()
            return cap.suppress
    except Exception:
        return False  # a tap fault must never kill the listener


def _run_console(cmd, timeout, suppress=True):
    """Send one console command and collect its records up to the terminating sentinel.

    Returns (records, timed_out). Always releases the lock and disarms the capture.
    """
    global _capture
    if not _exec_lock.acquire(timeout=2.0):
        raise ToolError("console busy -- another exec is still in flight")
    cap = _Capture(suppress)
    try:
        with _tap_lock:
            _capture = cap
        _deps.send_console_command_fn(cmd)
        deadline = time.monotonic() + timeout
        timed_out = False
        while not cap.done.wait(0.2):
            if _deps.stop_event is not None and _deps.stop_event.is_set():
                raise ToolError("beamtel is shutting down")
            if time.monotonic() > deadline:
                timed_out = True
                break
        return list(cap.records), timed_out
    finally:
        with _tap_lock:
            _capture = None
        _exec_lock.release()


def _assemble(records):
    """Reassemble RESP/OUT records into a result value and print output.

    emitChunks() repeats its prefix on *every* chunk, so a long value arrives as several
    `OUT|= ` records that must be concatenated, not newline-joined. Bare `OUT|` records
    are ambiguous -- a continuation of one long print line looks exactly like the next
    print line -- so a chunk is treated as a continuation when the previous one filled
    the datagram or already ended in a newline.
    """
    ok = True
    result_parts = []
    out_lines = []
    prev_out = None
    saw_resp = False

    for rec in records:
        parts = rec.split("|")
        tag = parts[0]
        if tag == "RESP":
            saw_resp = True
            status = parts[1] if len(parts) > 1 else ""
            body = "|".join(parts[2:]) if len(parts) > 2 else ""
            if status == "error":
                ok = False
                result_parts.append(body or "error")
            elif status == "queued":
                result_parts.append(
                    "queued (fire-and-forget context; no value is returned)"
                )
            elif body:
                result_parts.append(body)
        elif tag == "OUT":
            content = "|".join(parts[1:])
            if content.startswith("= "):
                result_parts.append(content[2:])
                prev_out = None
            else:
                cont = prev_out is not None and (
                    len(prev_out) >= MAX_DATAGRAM_CONTENT or prev_out.endswith("\n")
                )
                if cont and out_lines:
                    out_lines[-1] += content
                else:
                    out_lines.append(content)
                prev_out = content

    return {
        "ok": ok,
        "saw_response": saw_resp,
        "result": "".join(result_parts),
        "output": "\n".join(out_lines),
    }


def _truncate(payload):
    for key in ("result", "output"):
        val = payload.get(key) or ""
        if len(val) > MAX_RESULT_CHARS:
            payload[key] = val[:MAX_RESULT_CHARS]
            payload["truncated"] = True
    return payload


def _console_unreachable(timeout):
    return ToolError(
        "no response from consoleAccessible.lua on 127.0.0.1:4466 within "
        "%.1fs -- is BeamNG running with the mod loaded? Call `health`." % timeout
    )


def _fetch_contexts(timeout=5.0):
    records, timed_out = _run_console("CTXLIST", timeout, suppress=True)
    if timed_out and not records:
        raise _console_unreachable(timeout)
    out = []
    for rec in records:
        parts = rec.split("|")
        if parts[0] == "CTX" and len(parts) >= 3:
            try:
                out.append((int(parts[1]), "|".join(parts[2:])))
            except ValueError:
                pass
    return out


def _contexts(force=False):
    global _ctx_cache
    now = time.monotonic()
    if not force and _ctx_cache is not None and now - _ctx_cache[0] < _CTX_CACHE_S:
        return _ctx_cache[1]
    ctxs = _fetch_contexts()
    _ctx_cache = (now, ctxs)
    return ctxs


def _match_vehicle_context(want, force):
    for idx, label in _contexts(force=force):
        if idx < 3:
            continue
        low = label.lower()
        if want in ("current", "player"):
            if "(current)" in low:
                return idx
        elif want in low:
            return idx
    return None


def _resolve_context(spec):
    """Map a friendly context name onto consoleAccessible's index."""
    s = (spec or "ge").strip().lower()
    if s in ("ge", "lua", "ge-lua", "0"):
        return 0
    if s in ("ts", "torque", "torquescript", "1"):
        return 1
    if s in ("ui", "js", "cef", "2"):
        return 2
    if s.isdigit():
        return int(s)
    if s.startswith("veh"):
        want = s.split(":", 1)[1].strip() if ":" in s else "current"
        for force in (False, True):
            idx = _match_vehicle_context(want, force)
            if idx is not None:
                return idx
        raise ToolError(
            "no vehicle context matching %r; call `lua_contexts` to see what is loaded"
            % want
        )
    raise ToolError(
        "unknown context %r (use ge / ts / ui / veh:<id> / veh:current)" % spec
    )


def exec_console(context, code, timeout_s=None, verbose=False, suppress_gui=True):
    idx = _resolve_context(context)
    if timeout_s is None:
        timeout_s = EXEC_TIMEOUT_VEH if idx >= 3 else EXEC_TIMEOUT_GE
    started = time.monotonic()
    records, timed_out = _run_console(
        "EXEC|%d|%s" % (idx, code), timeout_s, suppress=suppress_gui
    )
    payload = _assemble(records)
    payload["context"] = context
    payload["context_index"] = idx
    payload["elapsed_ms"] = int((time.monotonic() - started) * 1000)
    if timed_out:
        payload["timed_out"] = True
        if not records:
            raise _console_unreachable(timeout_s)
        payload["ok"] = False
        payload["note"] = (
            "no EXECEND within %.1fs; the records below are partial" % timeout_s
        )
    if verbose:
        payload["records"] = records
    return _truncate(payload)


# ===================================================================================
#  Tool implementations
# ===================================================================================


def _require_world():
    if _deps.world_active_fn is not None and not _deps.world_active_fn():
        raise ToolError(
            "no live world: telemetry on 4444 has gone quiet. The game may be closed, "
            "at a menu, or paused. Call `health`."
        )


def _t_health(args):
    console_ok = None
    console_detail = ""
    try:
        res = exec_console("ge", "return 1", timeout_s=2.0)
        console_ok = bool(res.get("ok")) and not res.get("timed_out")
        console_detail = res.get("result", "")
    except Exception as e:
        console_ok = False
        console_detail = str(e)
    cfg = {}
    try:
        full = _deps.load_config_fn() or {}
        cfg = {
            k: full.get(k)
            for k in ("units", "telemetry_protocol", "mcp_server_enabled", "mcp_server_port")
        }
    except Exception:
        pass
    state = {}
    try:
        state = _deps.snapshot_state_fn(["liveness"]) or {}
    except Exception:
        pass
    return {
        "world_active": bool(_deps.world_active_fn()) if _deps.world_active_fn else None,
        "liveness": state.get("liveness", {}),
        "console_responsive": console_ok,
        "console_detail": console_detail,
        "mcp_uptime_s": round(time.monotonic() - _started_at, 1),
        "config": cfg,
        "version": getattr(_deps, "version", "unknown"),
    }


def _t_lua_exec(args):
    code = args.get("code")
    if not code:
        raise ToolError("`code` is required")
    return exec_console(
        args.get("context", "ge"),
        code,
        timeout_s=args.get("timeout_s"),
        verbose=bool(args.get("verbose")),
        suppress_gui=bool(args.get("suppress_gui", True)),
    )


def _t_lua_contexts(args):
    ctxs = _contexts(force=bool(args.get("refresh", True)))
    return {"contexts": [{"index": i, "label": lbl} for i, lbl in ctxs]}


def _t_get_state(args):
    sections = args.get("sections")
    return _deps.snapshot_state_fn(sections)


def _t_road_diagnostic(args):
    callback = getattr(_deps, "road_diagnostic_fn", None)
    if callback is None:
        raise ToolError("this beamtel build has no road diagnostic recorder")
    action = str(args.get("action", "status")).strip().lower()
    if action == "start":
        _require_world()
    return callback(
        action=action,
        label=args.get("label"),
        session=args.get("session"),
        note=args.get("note"),
        limit=args.get("limit", 20),
    )


# Which settings are secrets is `secretstore`'s list, not a second copy of it:
# the same names decide what gets sealed on disk and what gets masked here, and
# two lists would drift the day a provider is added. `get_config` is served to an
# agent over loopback HTTP and its output ends up in transcripts and logs, so a
# key readable through it is a key that has left the user's machine -- masked
# even though it is now stored sealed, because a DPAPI blob is still the
# credential in transportable form.
_REDACTED = "<redacted>"


def _mask_secrets(cfg):
    """Copy of `cfg` with secret-named values replaced.

    An empty value is left empty rather than redacted: whether a key is SET is
    exactly what an agent legitimately needs to know (it is why the describer
    refuses to run), and it gives away nothing.
    """
    if not isinstance(cfg, dict):
        return cfg
    out = {}
    for k, v in cfg.items():
        if secretstore.is_secret_setting(k) and v not in (None, "", [], {}):
            out[k] = _REDACTED
        else:
            out[k] = v
    return out


def _t_get_config(args):
    return _mask_secrets(_deps.load_config_fn())


def _t_speech_log(args):
    return _deps.get_speech_log_fn(
        since_seq=args.get("since_seq"),
        last_n=args.get("last_n", 50),
        source=args.get("source"),
        contains=args.get("contains"),
        spoken_only=bool(args.get("spoken_only", False)),
    )


def _t_speak(args):
    text = args.get("text")
    if not text:
        raise ToolError("`text` is required")
    _deps.say_fn(text, bool(args.get("interrupt", True)))
    return {"spoken": text}


def _t_list_commands(args):
    out = []
    for key, desc in (_deps.f9_help or {}).items():
        name, ctrl, shift, alt = key
        out.append(
            {
                "name": name,
                "ctrl": bool(ctrl),
                "shift": bool(shift),
                "alt": bool(alt),
                "description": desc,
            }
        )
    out.sort(key=lambda d: (d["name"], d["ctrl"], d["shift"], d["alt"]))
    return {"commands": out}


def _t_press_command(args):
    name = args.get("name")
    if not name:
        raise ToolError("`name` is required (the key pressed after F9)")
    settle = float(args.get("settle_ms", 300)) / 1000.0
    with _input_lock:
        before = _deps.get_speech_log_fn(last_n=1)
        cursor = before.get("next_seq", 0)
        _deps.press_command_fn(
            name,
            bool(args.get("ctrl")),
            bool(args.get("shift")),
            bool(args.get("alt")),
        )
        time.sleep(max(0.0, min(settle, 5.0)))
        after = _deps.get_speech_log_fn(since_seq=cursor, last_n=50)
    return {"pressed": name, "speech": after.get("entries", []), "next_seq": after.get("next_seq")}


def _t_send_command(args):
    payload = args.get("payload")
    if not payload:
        raise ToolError("`payload` is required")
    module = args.get("module")
    port = args.get("port")
    if module:
        if module not in MODULE_PORTS:
            raise ToolError(
                "unknown module %r; known: %s" % (module, ", ".join(sorted(MODULE_PORTS)))
            )
        port = MODULE_PORTS[module]
    if not port:
        raise ToolError("one of `module` or `port` is required")
    _deps.send_udp_fn(int(port), payload)
    return {
        "sent": payload,
        "port": int(port),
        "note": "fire-and-forget; verify the effect with get_state / speech_log / diag",
    }


def _t_vehicle_control(args):
    action = (args.get("action") or "").strip()
    if action == "list_active":
        _deps.send_udp_fn(4461, "REQUEST_ACTIVE_VEHICLES")
        return {"sent": "REQUEST_ACTIVE_VEHICLES", "note": "read the result with lua_exec or diag player_id"}
    if action == "player_id":
        return exec_console("ge", "return be:getPlayerVehicleID(0)")
    if action == "ignition_off":
        _deps.send_udp_fn(4461, "IGNITION_OFF")
        return {"sent": "IGNITION_OFF"}
    if action in ("remove", "reload"):
        ids = args.get("vehicle_ids")
        if not ids:
            raise ToolError("`vehicle_ids` is required for %s" % action)
        verb = "REMOVE" if action == "remove" else "RELOAD"
        msg = "%s:%s" % (verb, ",".join(str(i) for i in ids))
        _deps.send_udp_fn(4461, msg)
        return {"sent": msg}
    if action == "spawn":
        items = args.get("items")
        if not items:
            raise ToolError("`items` is required for spawn")
        body = {"items": items}
        if args.get("arrangement"):
            body["arrangement"] = args["arrangement"]
        msg = "SPAWN:" + json.dumps(body)
        _deps.send_udp_fn(4461, msg)
        return {"sent": msg}
    if action == "teleport":
        body = {
            k: args[k]
            for k in (
                "vehId",
                "refVehId",
                "refMode",
                "offFwdFt",
                "offRightFt",
                "offUpFt",
                "rotYawDeg",
                "rotPitchDeg",
                "rotRollDeg",
                "mode",
            )
            if k in args
        }
        if "vehId" not in body:
            raise ToolError("`vehId` is required for teleport")
        msg = "TELEPORT_PLACE:" + json.dumps(body)
        _deps.send_udp_fn(4461, msg)
        return {"sent": msg, "note": "the settle report lands on 4460 and is consumed by the spawner"}
    raise ToolError(
        "unknown action %r (list_active, player_id, spawn, teleport, remove, reload, ignition_off)"
        % action
    )


def _t_diag(args):
    name = (args.get("name") or "").strip()
    if name not in DIAGNOSTICS:
        raise ToolError(
            "unknown diagnostic %r; known: %s" % (name, ", ".join(sorted(DIAGNOSTICS)))
        )
    expr = DIAGNOSTICS[name]
    if "{id}" in expr:
        veh = args.get("veh_id")
        expr = expr.replace("{id}", "nil" if veh in (None, "") else str(veh))
    if name == "terrain":
        # diag() logs rather than returning, so read it out of a LOGON window.
        _deps.send_udp_fn(4465, "LOGON")
        try:
            before_seq = _log_seq
            exec_console("ge", expr)
            time.sleep(0.5)
            lines = [e for e in list(_log_ring) if e["seq"] > before_seq]
        finally:
            _deps.send_udp_fn(4465, "LOGOFF")
        return {"diagnostic": name, "log": [e["raw"] for e in lines]}
    res = exec_console("ge", "return " + expr)
    res["diagnostic"] = name
    return res


def _t_console_log(args):
    action = (args.get("action") or "read").strip()
    if action == "on":
        _deps.send_udp_fn(4465, "LOGON")
        return {"log_streaming": True}
    if action == "off":
        _deps.send_udp_fn(4465, "LOGOFF")
        return {"log_streaming": False}
    if action == "read":
        since = args.get("since_seq")
        with _tap_lock:
            entries = list(_log_ring)
        if since is not None:
            entries = [e for e in entries if e["seq"] > since]
        limit = int(args.get("last_n", 200))
        entries = entries[-limit:]
        return {
            "entries": entries,
            "next_seq": entries[-1]["seq"] if entries else since or 0,
        }
    raise ToolError("unknown action %r (on / off / read)" % action)


def _t_reset_test_state(args):
    _deps.send_udp_fn(4470, "REBUILD")
    out = {"sent": ["REBUILD -> implementProximity"]}
    try:
        res = exec_console("ge", "return extensions.rampGeometry.retry(nil)")
        out["ramp_retry"] = res.get("result", "")
    except Exception as e:
        out["ramp_retry_error"] = str(e)
    return out


# --- camera control -----------------------------------------------------------------
#
# Composed GE Lua run through exec_console rather than a new verb on cameraInfo.lua's
# 4451, for three reasons. Readback is the point of the tool and exec_console is the only
# channel that returns a value synchronously -- 4451 is fire-and-forget with no reply
# path, and giving 4450 a reply format for agent-only queries would be new wire protocol
# on a live junction. A new *listener* would additionally owe the setsockname-return /
# onExtensionUnloaded / retryCmdBind triple that vehicle_geometry_sim.lua scenario 12
# polices across every listening extension. And this is exactly what _t_diag already is:
# a curated wrapper over Lua the agent could have typed, leaving lua_exec the universal
# surface. Nothing in bng_mod/ changes.
#
# NOT ONE PERCENT SIGN appears in any chunk below -- these strings go through Python's
# own % formatting elsewhere in this file, and the modulo operator is the one Lua
# construct that would make a chunk unsafe to route that way. Hence n360().

# Read the pose back the way cameraInfo.lua reads it, so the two can never disagree.
# n360 is applied AFTER the rounding as well as before it: a yaw of 0 arrives from
# atan2 as 359.9999997, which r3 rounds to a clean 360.0 -- a value that does not
# exist in a 0..360 convention, and one an agent testing `yaw_deg == 0` would miss.
_CAM_READ_LUA = (
    "local p=core_camera.getPosition() local f=core_camera.getForward()"
    " local function n360(a) a=math.fmod(a,360) if a<0 then a=a+360 end return a end"
    " local function r3(v) return math.floor(v*1000+0.5)/1000 end"
    " local y,pi=0,0 local l=math.sqrt(f.x*f.x+f.y*f.y+f.z*f.z)"
    " if l>1e-9 then y=n360(math.deg(math.atan2(-f.x,-f.y))) pi=math.deg(math.asin(f.z/l)) end"
    " local ro=0 local ok,t=pcall(core_camera.getYawPitchRoll)"
    " if ok and type(t)=='table' and t.rollDeg then ro=t.rollDeg end"
    " return jsonEncode({mode=(core_camera.getActiveCamName()) or 'unknown',"
    "is_free=(commands.isFreeCamera() and true or false),"
    "pos={r3(p.x),r3(p.y),r3(p.z)},yaw_deg=n360(r3(y)),pitch_deg=r3(pi),roll_deg=r3(ro),"
    "player_veh_id=be:getPlayerVehicleID(0)})"
)

_CAM_LIST_LUA = (
    "local g,v={},{} local ok,gc=pcall(core_camera.getGlobalCameras)"
    " if ok and type(gc)=='table' then for k,_ in pairs(gc) do g[#g+1]=k end end"
    " local vid=be:getPlayerVehicleID(0)"
    " local ok2,vc=pcall(core_camera.getCameraDataById,vid)"
    " if ok2 and type(vc)=='table' then for k,_ in pairs(vc) do v[#v+1]=k end end"
    " table.sort(g) table.sort(v)"
    " return jsonEncode({global_cameras=g,vehicle_cameras=v,player_veh_id=vid,"
    "active=(core_camera.getActiveCamName()) or 'unknown'})"
)

# Entering the free camera goes through commands.setFreeCamera() rather than a bare
# setByName(0,'free'): it seeds the free cam at the camera's current pos/rot
# (commands.lua:38), so there is no visible jump before the placement lands.
_CAM_ENSURE_FREE = "if not commands.isFreeCamera() then commands.setFreeCamera() end "

_N360_LUA = "local function n360(a) a=math.fmod(a,360) if a<0 then a=a+360 end return a end "


def _cam_num(args, key, default=None, required=False):
    val = args.get(key, default)
    if val is None:
        if required:
            raise ToolError("`%s` is required" % key)
        return None
    try:
        val = float(val)
    except (TypeError, ValueError):
        raise ToolError("`%s` must be a number, got %r" % (key, args.get(key)))
    if val != val or val in (float("inf"), float("-inf")):
        raise ToolError("`%s` must be a finite number" % key)
    return val


def _cam_lua_num(val):
    return repr(float(val))


def _cam_veh_expr(args):
    """Lua binding __v to the vehicle a relative/orbit placement is measured against."""
    vid = args.get("veh_id")
    if vid in (None, ""):
        return "local __v=be:getPlayerVehicle(0)"
    try:
        vid = int(vid)
    except (TypeError, ValueError):
        raise ToolError("`veh_id` must be an integer vehicle id, got %r" % args.get("veh_id"))
    return "local __v=be:getObjectByID(" + str(vid) + ")"


def _cam_read(settle_ms=None):
    """Run the readback chunk and return it as a dict."""
    if settle_ms:
        time.sleep(max(0.0, min(float(settle_ms), CAM_SETTLE_MS_MAX)) / 1000.0)
    res = exec_console("ge", _CAM_READ_LUA)
    if res.get("timed_out"):
        raise _console_unreachable(EXEC_TIMEOUT_GE)
    if not res.get("ok"):
        raise ToolError("camera readback failed: " + (res.get("result") or "unknown error"))
    try:
        return json.loads(res.get("result") or "")
    except ValueError:
        raise ToolError("camera readback was not JSON: " + repr(res.get("result"))[:400])


def _cam_run(chunk):
    res = exec_console("ge", chunk)
    if res.get("timed_out"):
        raise _console_unreachable(EXEC_TIMEOUT_GE)
    if not res.get("ok"):
        raise ToolError("camera command failed: " + (res.get("result") or "unknown error"))
    return res


def _cam_place_chunk(args):
    """Build the free-camera placement chunk for whichever addressing mode was given."""
    has_abs = any(k in args for k in ("x", "y", "z"))
    has_rel = any(k in args for k in ("rel_fwd_m", "rel_right_m", "rel_up_m"))
    has_orbit = any(
        k in args for k in ("orbit_azimuth_deg", "orbit_elevation_deg", "orbit_distance_m")
    )
    if sum((has_abs, has_rel, has_orbit)) > 1:
        raise ToolError(
            "give exactly one position form: absolute (x/y/z), relative "
            "(rel_fwd_m/rel_right_m/rel_up_m) or orbit (orbit_*)"
        )

    tgt_default = ""
    if has_abs:
        x = _cam_num(args, "x", required=True)
        y = _cam_num(args, "y", required=True)
        z = _cam_num(args, "z", required=True)
        pos_lua = (
            "local __p=vec3(" + _cam_lua_num(x) + "," + _cam_lua_num(y) + ","
            + _cam_lua_num(z) + ")"
        )
    elif has_rel:
        f = _cam_num(args, "rel_fwd_m", 0.0)
        r = _cam_num(args, "rel_right_m", 0.0)
        u = _cam_num(args, "rel_up_m", 0.0)
        # __r is the vehicle's RIGHT: fwd:cross(up), deliberately the mirror of the
        # mod-wide up:cross(fwd) positive-is-LEFT rule. That rule governs bearings
        # reported to a driver; this is a placement parameter literally named "right".
        pos_lua = (
            _cam_veh_expr(args)
            + " if not __v then error('no such vehicle for a relative placement',0) end"
            " local __vp=__v:getPosition() local __f=__v:getDirectionVector()"
            " local __u=__v:getDirectionVectorUp() local __r=__f:cross(__u)"
            " local __p=vec3(__vp.x,__vp.y,__vp.z)+__f*" + _cam_lua_num(f)
            + "+__r*" + _cam_lua_num(r) + "+__u*" + _cam_lua_num(u)
        )
    elif has_orbit:
        az = _cam_num(args, "orbit_azimuth_deg", 0.0)
        el = _cam_num(args, "orbit_elevation_deg", 20.0)
        dist = _cam_num(args, "orbit_distance_m", required=True)
        if dist <= 0:
            raise ToolError("`orbit_distance_m` must be greater than zero")
        # Elevation is measured off the WORLD horizontal and the ring is built on the
        # vehicle's FLATTENED heading, so "30 degrees up at 10 metres" means the same
        # thing on a machine parked nose-down a bank as on one standing level.
        pos_lua = (
            _cam_veh_expr(args)
            + " if not __v then error('no such vehicle to orbit',0) end"
            " local __vp=__v:getPosition() local __vf=__v:getDirectionVector()"
            " local __h=vec3(__vf.x,__vf.y,0)"
            " if __h:length()<1e-6 then __h=vec3(0,1,0) end __h:normalize()"
            " local __rt=vec3(__h.y,-__h.x,0)"
            " local __az=math.rad(" + _cam_lua_num(az) + ") local __el=math.rad("
            + _cam_lua_num(el) + ") local __d=" + _cam_lua_num(dist)
            + " local __ch=math.cos(__el)*__d"
            " local __p=vec3(__vp.x,__vp.y,__vp.z)+__h*(math.cos(__az)*__ch)"
            "+__rt*(math.sin(__az)*__ch)+vec3(0,0,math.sin(__el)*__d)"
        )
        tgt_default = " local __t=vec3(__vp.x,__vp.y,__vp.z)"
    else:
        raise ToolError(
            "`place` needs a position: absolute (x/y/z), relative "
            "(rel_fwd_m/rel_right_m/rel_up_m) or orbit (orbit_distance_m + orbit_*)"
        )

    tgt_lua = tgt_default
    look = args.get("look_at")
    if look is not None:
        if isinstance(look, (list, tuple)):
            if len(look) != 3:
                raise ToolError("`look_at` as a point must be [x, y, z]")
            pt = [_cam_num({"v": v}, "v", required=True) for v in look]
            tgt_lua = (
                " local __t=vec3(" + _cam_lua_num(pt[0]) + "," + _cam_lua_num(pt[1])
                + "," + _cam_lua_num(pt[2]) + ")"
            )
        else:
            try:
                lid = int(look)
            except (TypeError, ValueError):
                raise ToolError("`look_at` must be [x, y, z] or a vehicle id, got %r" % (look,))
            tgt_lua = (
                " local __lv=be:getObjectByID(" + str(lid) + ")"
                " if not __lv then error('no such look_at vehicle',0) end"
                " local __lp=__lv:getPosition() local __t=vec3(__lp.x,__lp.y,__lp.z)"
            )

    yaw = _cam_num(args, "yaw_deg", 0.0)
    pitch = _cam_num(args, "pitch_deg", 0.0)
    roll = _cam_num(args, "roll_deg", 0.0)

    # A resolved look-at target overrides the explicit angles. Both yaw and pitch are
    # derived here in the MOD's convention and converted once, at the call itself.
    aim = (
        " local __y,__pi=" + _cam_lua_num(yaw) + "," + _cam_lua_num(pitch)
        + " if __t then local __d=__t-__p local __l=__d:length()"
        " if __l>1e-6 then __y=n360(math.deg(math.atan2(-__d.x,-__d.y)))"
        " __pi=math.deg(math.asin(__d.z/__l)) end end"
    )

    return (
        _CAM_ENSURE_FREE
        + _N360_LUA
        + pos_lua
        + tgt_lua
        + aim
        + " core_camera.setPosition(0,__p)"
        " core_camera.setFreeCameraYawPitchRollDeg(n360(__y-"
        + _cam_lua_num(CAM_YAW_OFFSET_DEG)
        + "),-__pi," + _cam_lua_num(roll) + ") return 'placed'"
    )


def _t_camera_control(args):
    # Argument validation comes BEFORE the world check: a typo'd action is a caller
    # error and must name the valid ones whatever the game is doing. Told "the game may
    # be closed" instead, the agent goes and debugs the game.
    action = (args.get("action") or "get").strip()
    if action not in CAMERA_ACTIONS:
        raise ToolError("unknown action %r (%s)" % (action, " | ".join(CAMERA_ACTIONS)))
    _require_world()

    settle = args.get("settle_ms", CAM_SETTLE_MS_DEFAULT)

    if action == "list_modes":
        res = _cam_run(_CAM_LIST_LUA)
        try:
            out = json.loads(res.get("result") or "")
        except ValueError:
            raise ToolError("list_modes was not JSON: " + repr(res.get("result"))[:400])
        out["action"] = "list_modes"
        return out

    if action == "get":
        out = _cam_read()
        out["action"] = "get"
        return out

    if action == "set_mode":
        mode = (args.get("mode") or "").strip()
        if not mode:
            raise ToolError("`mode` is required for set_mode; call list_modes for the names")
        # Validated rather than escaped: a camera name is an identifier, and refusing
        # anything else keeps quotes out of the composed chunk entirely.
        if not all(c.isalnum() or c in "._-" for c in mode):
            raise ToolError(
                "`mode` must be a camera name (letters, digits, . _ -), got %r" % mode
            )
        chunk = (
            "local n='" + mode + "' if n=='free' then " + _CAM_ENSURE_FREE
            + "else core_camera.setByName(0,n) end return 'mode set'"
        )
    elif action == "cycle":
        offset = int(_cam_num(args, "offset", 1))
        if offset == 0:
            raise ToolError("`offset` must be non-zero (+1 next camera, -1 previous)")
        chunk = "core_camera.setVehicleCameraByIndexOffset(0," + str(offset) + ") return 'cycled'"
    elif action == "reset":
        chunk = "core_camera.resetCamera(0) return 'reset'"
    else:  # place
        chunk = _cam_place_chunk(args)

    if len(chunk) > MAX_DATAGRAM_CONTENT:
        raise ToolError(
            "composed camera chunk is %d bytes, over the %d-byte datagram limit"
            % (len(chunk), MAX_DATAGRAM_CONTENT)
        )
    _cam_run(chunk)
    out = _cam_read(settle)
    out["action"] = action
    # Every action answers with the readback, so a set is self-verifying in one round
    # trip -- and `place` in particular can only work from the free camera, since
    # setPosition and setFreeCameraYawPitchRollDeg both early-return otherwise
    # (core/camera.lua:1088). `is_free` here is therefore proof that it landed rather
    # than an inference from the absence of an error.
    return out


def _t_screenshot(args):
    """Write a PNG of the game to disk. Deliberately does NOT send it anywhere."""
    capture = getattr(_deps, "capture_png_fn", None)
    if capture is None:
        raise ToolError(
            "this beamtel build injected no capture function -- screenshot is unavailable"
        )

    hide_ui = bool(args.get("hide_ui", True))
    # Default TRUE, unlike the AI Describer, because the situations differ: that one
    # fires from a keypress while the user is playing, so the game is already in front.
    # An agent shoots while the user is looking at a terminal, and BeamNG is then
    # usually MINIMIZED -- it parks off-screen and renames itself "- background", so a
    # plain monitor grab returns the desktop. That is a silent, entirely plausible
    # wrong answer, which is the failure this whole server exists to catch.
    focus_game = bool(args.get("focus_game", True))
    settle_ms = args.get("settle_ms")
    settle_s = None
    if settle_ms is not None:
        settle_s = max(0.0, min(float(settle_ms), SHOT_SETTLE_MS_MAX)) / 1000.0

    directory = args.get("dir") or SCREENSHOT_DIR_DEFAULT
    directory = os.path.abspath(os.path.expanduser(str(directory)))

    filename = args.get("filename")
    if filename:
        filename = str(filename)
        if os.path.basename(filename) != filename or filename in (".", ".."):
            raise ToolError("`filename` must be a bare file name, not a path: %r" % filename)
        if not filename.lower().endswith(".png"):
            filename += ".png"
    else:
        # Timestamped to the millisecond: a before/after pair is the main thing this
        # tool gets used for, and two shots in the same second must not collide.
        now = time.time()
        filename = time.strftime("beam_%Y%m%d_%H%M%S", time.localtime(now)) + (
            "_%03d.png" % int((now - int(now)) * 1000)
        )

    path = os.path.join(directory, filename)

    try:
        if settle_s is not None:
            got = capture(hide_ui, settle_s, focus_game)
        else:
            got = capture(hide_ui, focus_game=focus_game)
    except Exception as e:
        raise ToolError("screen capture failed: %s: %s" % (type(e).__name__, e))
    png, meta = got if isinstance(got, tuple) else (got, {})
    if not png:
        raise ToolError("screen capture returned no data")

    try:
        os.makedirs(directory, exist_ok=True)
        with open(path, "wb") as fh:
            fh.write(png)
    except Exception as e:
        raise ToolError("could not write %s: %s: %s" % (path, type(e).__name__, e))

    out = {
        "path": path,
        "bytes": len(png),
        "hide_ui": hide_ui,
        "note": "written to disk only -- not sent to any model. Read the file to view it.",
    }
    # What was actually captured rides back with it. A grab of the desktop because the
    # game would not come up looks exactly like a real screenshot to everything except
    # this field.
    out.update(meta or {})
    if focus_game and not out.get("game_window_found"):
        out["warning"] = "no BeamNG window found -- this is a grab of the primary monitor"
    return out


# ===================================================================================
#  Tool registry
# ===================================================================================

TOOLS = [
    {
        "name": "health",
        "description": (
            "Whether the game and the mod are reachable: live world, telemetry age, "
            "console round-trip, uptime. Call this first in any session; every other "
            "tool's failure message points back here."
        ),
        "inputSchema": {"type": "object", "properties": {}},
        "handler": _t_health,
    },
    {
        "name": "lua_exec",
        "description": (
            "Execute Lua (or TorqueScript / UI JavaScript) inside BeamNG and return the "
            "value plus captured print output. context: 'ge' (Game Engine Lua, the only "
            "one that returns a value synchronously), 'ts', 'ui', 'veh:current' or "
            "'veh:<id>' for a vehicle VM. This is the universal surface -- it reaches "
            "every mod diagnostic, including those with no UDP command."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "Lua source. Use 'return ...' to get a value."},
                "context": {"type": "string", "description": "ge | ts | ui | veh:current | veh:<id>", "default": "ge"},
                "timeout_s": {"type": "number", "description": "Default 5s for ge, 15s for a vehicle context."},
                "verbose": {"type": "boolean", "description": "Include the raw wire records."},
                "suppress_gui": {"type": "boolean", "description": "Keep the result out of beamtel's console pane and out of the user's ears. Default true.", "default": True},
            },
            "required": ["code"],
        },
        "handler": _t_lua_exec,
    },
    {
        "name": "lua_contexts",
        "description": "List the execution contexts consoleAccessible offers, including one per loaded vehicle.",
        "inputSchema": {
            "type": "object",
            "properties": {"refresh": {"type": "boolean", "default": True}},
        },
        "handler": _t_lua_contexts,
    },
    {
        "name": "get_state",
        "description": (
            "Snapshot of beamtel's live state. Sections: telemetry, position, implement, "
            "trailer, dock, cannon, scanner, modes, slots, liveness. Omit `sections` for all."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "sections": {"type": "array", "items": {"type": "string"}}
            },
        },
        "handler": _t_get_state,
    },
    {
        "name": "get_config",
        "description": (
            "The user's current beamtel configuration. Read-only by design -- the agent "
            "must not silently rewrite the user's settings. API keys and other secrets "
            "read back as `<redacted>` when set and empty when unset."
        ),
        "inputSchema": {"type": "object", "properties": {}},
        "handler": _t_get_config,
    },
    {
        "name": "road_diagnostic",
        "description": (
            "Record and review a road-guidance driving attempt. `start` opens a durable "
            "NDJSON session and enables detailed Lua lane-correction fields; `stop` closes "
            "it and returns an analysis of settled-band occupancy, correction episodes, "
            "rapid settled-tone retriggers, steering/target oscillation, and wheel slip. "
            "`status`, `list`, and `review` are read-only; `mark` adds a timestamped note."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "start | stop | status | mark | review | list",
                    "default": "status",
                },
                "label": {"type": "string", "description": "start: short session label"},
                "session": {"type": "string", "description": "review: bare .ndjson session file name; omit for latest"},
                "note": {"type": "string", "description": "mark: note about an observed event"},
                "limit": {"type": "integer", "description": "list: maximum sessions", "default": 20},
            },
        },
        "handler": _t_road_diagnostic,
    },
    {
        "name": "speech_log",
        "description": (
            "What the mod actually announced, as a sequence-numbered ring buffer. This is "
            "the assertion primitive: it records every say() call including the ones "
            "excluded from the user's replay buffer, and records calls that were "
            "suppressed during loading (spoken=false). Use `since_seq` as a cursor."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "since_seq": {"type": "integer", "description": "Return only entries after this sequence number."},
                "last_n": {"type": "integer", "default": 50},
                "source": {"type": "string", "description": "Filter by the say() source tag."},
                "contains": {"type": "string", "description": "Case-insensitive substring filter."},
                "spoken_only": {"type": "boolean", "default": False},
            },
        },
        "handler": _t_speech_log,
    },
    {
        "name": "speak",
        "description": "Say something to the user through the mod's own speech output -- how to give instructions mid-test.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "text": {"type": "string"},
                "interrupt": {"type": "boolean", "default": True},
            },
            "required": ["text"],
        },
        "handler": _t_speak,
    },
    {
        "name": "list_commands",
        "description": "Every F9 command the mod offers, with its modifiers and description. Works with no game running.",
        "inputSchema": {"type": "object", "properties": {}},
        "handler": _t_list_commands,
    },
    {
        "name": "press_command",
        "description": (
            "Trigger an F9 command in-process (no OS key injection, no window focus "
            "needed) and return the speech it produced. One round trip is act plus "
            "assert. Note that some commands open modal UI."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The key pressed after F9, e.g. 's' for speed."},
                "ctrl": {"type": "boolean"},
                "shift": {"type": "boolean"},
                "alt": {"type": "boolean"},
                "settle_ms": {"type": "integer", "default": 300, "description": "How long to wait for speech before reading it back."},
            },
            "required": ["name"],
        },
        "handler": _t_press_command,
    },
    {
        "name": "send_command",
        "description": (
            "Send a raw UDP command to one of the mod's Lua extensions. Fire-and-forget: "
            "verify the effect with get_state / speech_log / diag. Modules: "
            + ", ".join(sorted(MODULE_PORTS))
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "module": {"type": "string", "description": "Extension name from the list above."},
                "port": {"type": "integer", "description": "Escape hatch for a port not in the map."},
                "payload": {"type": "string", "description": "The command verb, e.g. 'DOCK_ON', 'SCAN', 'MODE:chase'."},
            },
            "required": ["payload"],
        },
        "handler": _t_send_command,
    },
    {
        "name": "vehicle_control",
        "description": (
            "Spawn, teleport, remove or reload vehicles -- how to build known test "
            "geometry. Teleport offsets are in FEET, rotations in degrees."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "description": "list_active | player_id | spawn | teleport | remove | reload | ignition_off"},
                "items": {"type": "array", "items": {"type": "object"}, "description": "spawn: [{model, config, refMode, offFwdFt, ...}]"},
                "arrangement": {"type": "object"},
                "vehicle_ids": {"type": "array", "items": {"type": "integer"}},
                "vehId": {"type": "integer"},
                "refVehId": {"type": "integer"},
                "refMode": {"type": "string"},
                "offFwdFt": {"type": "number"},
                "offRightFt": {"type": "number"},
                "offUpFt": {"type": "number"},
                "rotYawDeg": {"type": "number"},
                "rotPitchDeg": {"type": "number"},
                "rotRollDeg": {"type": "number"},
                "mode": {"type": "string"},
            },
            "required": ["action"],
        },
        "handler": _t_vehicle_control,
    },
    {
        "name": "diag",
        "description": (
            "Run one of the mod's Lua ground-truth diagnostics. Names: "
            + ", ".join(sorted(DIAGNOSTICS))
            + ". These print what the resolver actually chose, which is the only way to "
            "catch a confident wrong number."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "veh_id": {"type": "integer", "description": "For the diagnostics that take a vehicle id."},
            },
            "required": ["name"],
        },
        "handler": _t_diag,
    },
    {
        "name": "console_log",
        "description": "Turn the game's Lua log stream on or off, and read what it captured. Log records still reach beamtel's own console pane.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "description": "on | off | read", "default": "read"},
                "since_seq": {"type": "integer"},
                "last_n": {"type": "integer", "default": 200},
            },
        },
        "handler": _t_console_log,
    },
    {
        "name": "camera_control",
        "description": (
            "Move the eye. Switches camera mode (including free camera) and places the "
            "free camera anywhere, then returns the resulting camera state -- so a set "
            "is self-verifying in one round trip and pairs directly with `screenshot`. "
            "actions: get (default) | list_modes | set_mode | cycle | reset | place. "
            "`place` takes exactly one position form -- absolute (x/y/z), relative to a "
            "vehicle's own frame (rel_fwd_m/rel_right_m/rel_up_m), or orbit around one "
            "(orbit_distance_m + orbit_azimuth_deg/orbit_elevation_deg) -- plus an "
            "optional `look_at` that derives the orientation for you. Angles are in the "
            "mod's own convention: yaw 0-360 matching the 4450 feed and the Alt+H "
            "readout, pitch positive-UP. This never moves the vehicle."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "get | list_modes | set_mode | cycle | reset | place",
                    "default": "get",
                },
                "mode": {"type": "string", "description": "set_mode: a camera name from list_modes, e.g. 'free', 'orbit', 'onboard.driver'."},
                "offset": {"type": "integer", "description": "cycle: +1 for the next vehicle camera, -1 for the previous.", "default": 1},
                "veh_id": {"type": "integer", "description": "place: the vehicle a relative or orbit placement is measured against. Defaults to the player's."},
                "x": {"type": "number", "description": "place, absolute: world X."},
                "y": {"type": "number", "description": "place, absolute: world Y."},
                "z": {"type": "number", "description": "place, absolute: world Z."},
                "rel_fwd_m": {"type": "number", "description": "place, relative: metres along the vehicle's forward. Negative is behind it."},
                "rel_right_m": {"type": "number", "description": "place, relative: metres to the vehicle's RIGHT. Note this is a placement offset, so it is the mirror of the mod's positive-is-LEFT bearing convention."},
                "rel_up_m": {"type": "number", "description": "place, relative: metres along the vehicle's up."},
                "orbit_distance_m": {"type": "number", "description": "place, orbit: radius from the vehicle. Implies looking at it."},
                "orbit_azimuth_deg": {"type": "number", "description": "place, orbit: 0 puts the camera directly ahead of the vehicle, positive swings to its right.", "default": 0},
                "orbit_elevation_deg": {"type": "number", "description": "place, orbit: degrees above the world horizontal.", "default": 20},
                "look_at": {"description": "place: a [x, y, z] point or a vehicle id to aim at. Overrides yaw_deg/pitch_deg."},
                "yaw_deg": {"type": "number", "description": "place: heading, 0-360, same convention as the 4450 camera feed.", "default": 0},
                "pitch_deg": {"type": "number", "description": "place: positive is looking UP, matching cameraInfo.lua.", "default": 0},
                "roll_deg": {"type": "number", "description": "place: camera roll.", "default": 0},
                "settle_ms": {"type": "integer", "description": "Wait before reading the pose back.", "default": 150},
            },
        },
        "handler": _t_camera_control,
    },
    {
        "name": "screenshot",
        "description": (
            "Capture the game and WRITE THE PNG TO A FILE for you to read -- the same "
            "capture path the AI Describer uses, with nothing sent to any model. Use it "
            "when a picture settles a question the numbers cannot: whether a car really "
            "is where a resolver says it is, what a mouth or an implement is actually "
            "pointing at, or what the mod's own UI is showing. Pass `dir` to put the "
            "file in your own working directory, then read that path. Pairs with "
            "`camera_control`: place the eye, then shoot."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "dir": {"type": "string", "description": "Directory to write into. Defaults to %LOCALAPPDATA%/beamtel/screenshots."},
                "filename": {"type": "string", "description": "Bare file name, no path. Defaults to a millisecond-stamped name so successive shots never collide."},
                "hide_ui": {"type": "boolean", "description": "Hide the game HUD around the grab for a clean view of the world. Pass false to inspect the UI itself.", "default": True},
                "focus_game": {"type": "boolean", "description": "Raise the BeamNG window before the grab and put the previous window back afterwards. Needed whenever the game is minimized, which it usually is when an agent is driving -- a minimized BeamNG renders nothing and a plain grab returns the desktop. The result reports what was actually captured.", "default": True},
                "settle_ms": {"type": "integer", "description": "How long to let the game render a UI-free frame after hiding.", "default": 200},
            },
        },
        "handler": _t_screenshot,
    },
    {
        "name": "reset_test_state",
        "description": "Drop the mod's cached geometry (implement rebuild + ramp resolve re-arm) so the next test starts clean.",
        "inputSchema": {"type": "object", "properties": {}},
        "handler": _t_reset_test_state,
    },
]

_TOOLS_BY_NAME = {t["name"]: t for t in TOOLS}

INSTRUCTIONS = (
    "Drives the BEAM screen-reader mod for BeamNG.drive. Call `health` first -- it says "
    "whether a world is live and whether the game console answers. Every tool returns a "
    "clear error rather than hanging when the game is not running. `lua_exec` is the "
    "universal surface; `speech_log` is how you assert on what the mod announced; "
    "`speak` is how you give the user instructions mid-test."
)


# ===================================================================================
#  JSON-RPC / MCP protocol
# ===================================================================================


def _rpc_result(id_, result):
    return {"jsonrpc": "2.0", "id": id_, "result": result}


def _rpc_error(id_, code, message):
    return {"jsonrpc": "2.0", "id": id_, "error": {"code": code, "message": message}}


def _call_tool(name, args):
    tool = _TOOLS_BY_NAME.get(name)
    if tool is None:
        raise KeyError(name)
    try:
        payload = tool["handler"](args or {})
        text = payload if isinstance(payload, str) else json.dumps(payload, indent=2, default=str)
        return {"content": [{"type": "text", "text": text}], "isError": False}
    except ToolError as e:
        return {"content": [{"type": "text", "text": str(e)}], "isError": True}
    except Exception as e:
        _log("error", "tool %s failed: %s" % (name, traceback.format_exc()))
        return {
            "content": [{"type": "text", "text": "%s: %s" % (type(e).__name__, e)}],
            "isError": True,
        }


async def _dispatch(msg):
    """Handle one JSON-RPC message. Returns a response dict, or None for a notification."""
    method = msg.get("method")
    id_ = msg.get("id")
    params = msg.get("params") or {}

    if id_ is None:
        return None  # notification: nothing to answer

    if method == "initialize":
        want = params.get("protocolVersion")
        version = want if want in SUPPORTED_PROTOCOLS else FALLBACK_PROTOCOL
        return _rpc_result(
            id_,
            {
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": str(getattr(_deps, "version", "1.0")),
                },
                "instructions": INSTRUCTIONS,
            },
        )

    if method == "ping":
        return _rpc_result(id_, {})

    if method == "tools/list":
        return _rpc_result(
            id_,
            {
                "tools": [
                    {
                        "name": t["name"],
                        "description": t["description"],
                        "inputSchema": t["inputSchema"],
                    }
                    for t in TOOLS
                ]
            },
        )

    if method == "tools/call":
        name = params.get("name")
        args = params.get("arguments") or {}
        if name not in _TOOLS_BY_NAME:
            return _rpc_error(id_, -32602, "Unknown tool: %s" % name)
        loop = asyncio.get_event_loop()
        try:
            # Tool bodies block (UDP round trips, sleeps), so they never run on the event
            # loop. The outer timeout is generous: the tool's own timeout should fire
            # first and report something useful.
            result = await asyncio.wait_for(
                loop.run_in_executor(None, _call_tool, name, args), timeout=60.0
            )
        except asyncio.TimeoutError:
            result = {
                "content": [
                    {"type": "text", "text": "tool %s exceeded 60s and was abandoned" % name}
                ],
                "isError": True,
            }
        return _rpc_result(id_, result)

    return _rpc_error(id_, -32601, "Method not found: %s" % method)


def _origin_ok(request):
    """Guard against DNS-rebinding from a browser on this machine."""
    origin = request.headers.get("Origin")
    if not origin:
        return True
    return origin.startswith("http://127.0.0.1") or origin.startswith("http://localhost")


async def mcp_post(request):
    if not _origin_ok(request):
        return web.Response(status=403, text="forbidden origin")
    try:
        body = await request.json()
    except Exception:
        return web.json_response(_rpc_error(None, -32700, "Parse error"), status=400)

    if isinstance(body, list):
        responses = []
        for msg in body:
            resp = await _dispatch(msg)
            if resp is not None:
                responses.append(resp)
        if not responses:
            return web.Response(status=202)
        return web.json_response(responses)

    resp = await _dispatch(body)
    if resp is None:
        # A notification (notifications/initialized and friends) must be answered with
        # 202 and an empty body, or the client hangs waiting on the handshake.
        return web.Response(status=202)
    return web.json_response(resp)


async def mcp_get(request):
    # No server-initiated SSE stream: nothing here pushes to the client. 405 is the
    # spec's way of saying so, and clients treat it as "no stream" rather than an error.
    return web.Response(status=405, text="no server-initiated stream")


async def mcp_delete(request):
    return web.Response(status=200, text="")


async def health_handler(request):
    return web.Response(text="beamtel mcp server ok\n")


# ===================================================================================
#  Boot -- mirrors nvda_ws_speaker.start_server_in_thread
# ===================================================================================


async def _start_all(host, port):
    global _RUNNER
    app = web.Application()
    app.add_routes(
        [
            web.post("/mcp", mcp_post),
            web.get("/mcp", mcp_get),
            web.delete("/mcp", mcp_delete),
            web.get("/health", health_handler),
        ]
    )
    runner = web.AppRunner(app)
    _RUNNER = runner
    await runner.setup()
    site = web.TCPSite(runner, host, port)
    await site.start()
    _log("info", "MCP server listening on http://%s:%d/mcp" % (host, port))


def _thread_target(host, port):
    global _LOOP
    _LOOP = asyncio.new_event_loop()
    asyncio.set_event_loop(_LOOP)
    try:
        _LOOP.run_until_complete(_start_all(host, port))
    except Exception as e:
        _log("error", "failed to bind %s:%d -- %s" % (host, port, e))
        return
    _LOOP.run_forever()


def init(deps):
    global _deps, _logger
    _deps = deps
    _logger = getattr(deps, "logger", None)


def start(stop_event, port=DEFAULT_PORT, host=HOST):
    """Start the server on a daemon thread. Returns (thread, stop_fn)."""
    global _started_at
    _started_at = time.monotonic()
    if _deps is not None:
        _deps.stop_event = stop_event
    t = threading.Thread(
        target=_thread_target, args=(host, port), name="beamtel-mcp", daemon=True
    )
    t.start()

    def stop():
        try:
            if _LOOP is None:
                return

            async def _shutdown():
                if _RUNNER is not None:
                    await _RUNNER.cleanup()

            fut = asyncio.run_coroutine_threadsafe(_shutdown(), _LOOP)
            try:
                fut.result(timeout=1.0)
            except Exception:
                pass
            _LOOP.call_soon_threadsafe(_LOOP.stop)
        except Exception:
            pass

    return t, stop


def make_udp_sender(logger):
    """Default `send_udp_fn`: one throwaway datagram, the pattern beamtel uses."""

    def _send(port, message):
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            s.sendto(message.encode("utf-8"), ("127.0.0.1", int(port)))
            s.close()
        except Exception as e:
            raise ToolError("failed to send UDP to port %s: %s" % (port, e))

    return _send
