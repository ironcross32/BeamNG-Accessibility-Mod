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
import socket
import threading
import time
import traceback

from aiohttp import web

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


def _t_get_config(args):
    return _deps.load_config_fn()


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
        "description": "The user's current beamtel configuration. Read-only by design -- the agent must not silently rewrite the user's settings.",
        "inputSchema": {"type": "object", "properties": {}},
        "handler": _t_get_config,
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
