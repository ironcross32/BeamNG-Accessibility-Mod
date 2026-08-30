# ws-speaker (class-free)
# WebSocket-Only version
# Designed to be embedded: start_server_in_thread(lambda text: say(text))

import asyncio, json, os, re, threading, time
from aiohttp import web, WSMsgType
from bnh_logger import get_logger

# Safety net for raw CSS that can leak from the UI hook when a handler reads the
# textContent of a non-rendered <style> element (happens when the game UI is
# toggled off). The app.js side filters these too; this is a backstop.
_CSS_LEAK_RE = re.compile(r"[#.\w-]+\s*\{[^}]*:[^}]*;|\}\s*[#.\w-]+\s*\{")


def _looks_like_css(t: str) -> bool:
    return bool(_CSS_LEAK_RE.search(t))

HOST = os.getenv("BNVDA_HOST", "127.0.0.1")
WS_PORT = int(os.getenv("BNVDA_WS_PORT", "8765"))
TCP_PORT = int(os.getenv("BNVDA_TCP_PORT", "8766"))

# Initialize the shared logger
logger = get_logger()

# ---------- Global state ----------
_LOOP: asyncio.AbstractEventLoop | None = None
_RUNNER: web.AppRunner | None = None
_TCP_SERVER: asyncio.AbstractServer | None = None
_SPEAK_FUNC = None
_SPEAK_LAST = ""
_SPEAK_LAST_TS = 0.0
# Suppress an identical interrupting repeat only inside this window. Matches the
# same-text gate the UI runtime already applies (bnvdaRuntime.js scheduleSpeak).
# Without an expiry, leaving a list row and coming back to it stayed silent
# forever, because the text still equalled the last thing spoken.
_SPEAK_DEDUP_S = 0.4
_HOVER_TASK: asyncio.Task | None = None
_HOVER_TOKEN = None
_DOM_DUMP_CALLBACK = None
_LOADING_STATE_CALLBACK = None
_SETTINGS_REQUEST_CALLBACK = None
_ACCESSIBILITY_ACTION_CALLBACK = None

# Mirrors window.BNVDA_DEBUG from the CEF/UI JS context. The UI runtime reports it
# whenever it changes, so debug output anywhere in the Python side can be switched on
# at runtime from the accessible console rather than needing a relaunch.
_BNVDA_DEBUG = False

_CLIENTS: set = set()  # connected WebSocket instances
_TCP_CLIENTS: set = set()  # connected GE Lua newline-delimited JSON clients


def register_dom_dump_callback(callback):
    global _DOM_DUMP_CALLBACK
    _DOM_DUMP_CALLBACK = callback

def register_loading_state_callback(callback):
    """Register the application-level loading lifecycle handler."""
    global _LOADING_STATE_CALLBACK
    _LOADING_STATE_CALLBACK = callback


def register_settings_request_callback(callback):
    """Register the handler that answers the UI runtime's settings pull.

    The runtime asks on every transport activation rather than relying on a push:
    beamtel broadcasts whenever the config changes, but the first such broadcast
    happens long before the Lua bridge connects and would be dropped on the floor.
    """
    global _SETTINGS_REQUEST_CALLBACK
    _SETTINGS_REQUEST_CALLBACK = callback


def register_accessibility_action_callback(callback):
    """Register the application handler for allowlisted gameplay actions."""
    global _ACCESSIBILITY_ACTION_CALLBACK
    _ACCESSIBILITY_ACTION_CALLBACK = callback


def bnvda_debug_enabled():
    """True while window.BNVDA_DEBUG is set in the game's UI JS context.

    Read this live at each use site rather than latching it — the flag is flipped from
    the accessible console long after startup, and a UI reload resets it.
    """
    return _BNVDA_DEBUG




# ---------- Core helpers ----------
def engine_offer(text: str, interrupt: bool = True):
    global _SPEAK_LAST, _SPEAK_LAST_TS
    t = (text or "").strip()
    if not t:
        return
    if _looks_like_css(t):
        logger.info("[bnvda] Suppressed CSS-like text from UI hook.")
        return
    # Only dedup against the last utterance for interrupting speech. A queued
    # follow-up (interrupt=False, e.g. a tuning hint) is intentionally a second
    # utterance and must not be swallowed by this guard.
    now = time.monotonic()
    if interrupt and t == _SPEAK_LAST and (now - _SPEAK_LAST_TS) < _SPEAK_DEDUP_S:
        return
    _SPEAK_LAST = t
    _SPEAK_LAST_TS = now
    try:
        _SPEAK_FUNC(t, interrupt)
    except Exception as e:
        logger.error(f"speak error: {e}")


def hover_cancel():
    global _HOVER_TASK, _HOVER_TOKEN
    task = _HOVER_TASK
    _HOVER_TASK, _HOVER_TOKEN = None, None
    if task and not task.done():
        task.cancel()


def hover_on(text: str, item_id=None, delay_ms=1000):
    global _HOVER_TASK, _HOVER_TOKEN
    try:
        delay_s = max(0.05, float(delay_ms) / 1000.0)
    except Exception:
        delay_s = 1.0
    token = item_id if item_id is not None else (text or "")
    _HOVER_TOKEN = token
    hover_cancel()

    async def _later(payload_text: str, tok):
        try:
            await asyncio.sleep(delay_s)
            if tok == _HOVER_TOKEN and payload_text:
                engine_offer(payload_text.strip())
        except asyncio.CancelledError:
            return

    _HOVER_TASK = _LOOP.create_task(_later((text or "").strip(), token))


# ---------- Message Handlers (called by WebSocket) ----------
def handle_ws_message(data):
    if not isinstance(data, dict):
        return
    msg_type = data.get("type")
    if msg_type == "speak":
        text_val = data.get("text", "")
        if not isinstance(text_val, str):
            logger.warning(f"[UNSPEAKABLE] speak text is {type(text_val).__name__}, not str: {text_val!r}")
            text_val = str(text_val) if text_val is not None else ""
        # interrupt defaults to True; a queued follow-up sets interrupt:false so
        # it plays after the current utterance instead of cutting it off.
        engine_offer(text_val, bool(data.get("interrupt", True)))
    elif msg_type == "log":
        level = str(data.get("level", "INFO")).lower()
        msg = str(data.get("msg", ""))
        getattr(logger, level, logger.info)(msg)
    elif msg_type == "debug_state":
        global _BNVDA_DEBUG
        new_state = bool(data.get("enabled", False))
        if new_state != _BNVDA_DEBUG:
            _BNVDA_DEBUG = new_state
            logger.info("[bnvda] BNVDA_DEBUG %s", "enabled" if new_state else "disabled")
    elif msg_type == "hover":
        hover_on(
            data.get("text", ""), item_id=data.get("id"), delay_ms=data.get("delay_ms")
        )
    elif msg_type == "hover_cancel":
        hover_cancel()
    elif msg_type == "dom_dump_result":
        if _DOM_DUMP_CALLBACK:
            try:
                _DOM_DUMP_CALLBACK(data.get("lines", []))
            except Exception as e:
                logger.error(f"dom_dump_result callback error: {e}")

    elif msg_type == "settings_request":
        if _SETTINGS_REQUEST_CALLBACK:
            try:
                _SETTINGS_REQUEST_CALLBACK()
            except Exception as e:
                logger.error(f"settings_request callback error: {e}")

    elif msg_type == "loading_state":
        if _LOADING_STATE_CALLBACK:
            try:
                _LOADING_STATE_CALLBACK(
                    bool(data.get("active", False)),
                    str(data.get("focusText", "") or ""),
                )
            except Exception as e:
                logger.error(f"loading_state callback error: {e}")

    elif msg_type == "accessibility_action":
        action = data.get("action")
        if action not in {
            "status_up",
            "status_down",
            "status_repeat",
            "next_menu",
            "previous_menu",
            "activate",
        }:
            logger.warning(
                "[bnvda] Ignoring unknown accessibility action: %r", action
            )
            return
        if _ACCESSIBILITY_ACTION_CALLBACK:
            try:
                _ACCESSIBILITY_ACTION_CALLBACK(action)
            except Exception as e:
                logger.error(f"accessibility_action callback error: {e}")

# ---------- WebSocket Server Logic ----------
async def ws_handler(request: web.Request):
    logger.info(
        "[bnvda] WebSocket handshake attempt from %s origin=%r",
        request.remote,
        request.headers.get("Origin"),
    )
    ws = web.WebSocketResponse(heartbeat=30)
    ws.headers["Access-Control-Allow-Origin"] = "*"
    ws.headers["Access-Control-Allow-Private-Network"] = "true"
    try:
        await ws.prepare(request)
    except Exception as e:
        logger.error(f"[bnvda] WebSocket handshake failed: {e}")
        raise
    _CLIENTS.add(ws)
    logger.info("WebSocket client connected.")
    try:
        async for msg in ws:
            if msg.type == WSMsgType.TEXT:
                try:
                    data = json.loads(msg.data)
                    if data.get("type") == "transport_ping":
                        await ws.send_json({"type": "transport_pong", "transport": "direct-websocket"})
                    else:
                        handle_ws_message(data)
                except Exception as e:
                    logger.warning(f"[bnvda] Invalid WebSocket message: {e}")
            elif msg.type == WSMsgType.ERROR:
                logger.error(f"ws error: {ws.exception()}")
    except Exception as e:
        logger.error(f"ws exception: {e}")
    finally:
        _CLIENTS.discard(ws)
        logger.info("WebSocket client disconnected.")
        try:
            await ws.close()
        except Exception:
            pass
    return ws


async def health_handler(request: web.Request):
    """A CEF-readable probe which is deliberately independent of WebSockets."""
    logger.info(
        "[bnvda] HTTP health probe from %s origin=%r",
        request.remote,
        request.headers.get("Origin"),
    )
    return web.json_response(
        {"ok": True, "service": "bnvda", "websocket_clients": len(_CLIENTS)},
        headers={
            "Access-Control-Allow-Origin": "*",
            "Access-Control-Allow-Private-Network": "true",
            "Cache-Control": "no-store",
        },
    )


async def tcp_handler(reader: asyncio.StreamReader, writer: asyncio.StreamWriter):
    peer = writer.get_extra_info("peername")
    _TCP_CLIENTS.add(writer)
    logger.info(f"[bnvda] Lua TCP relay connected from {peer}.")
    try:
        while line := await reader.readline():
            try:
                data = json.loads(line.decode("utf-8"))
                if data.get("type") == "transport_ping":
                    writer.write(b'{"type":"transport_pong","transport":"lua-tcp"}\n')
                    await writer.drain()
                else:
                    handle_ws_message(data)
            except (UnicodeDecodeError, json.JSONDecodeError) as e:
                logger.warning(f"[bnvda] Invalid Lua TCP relay message: {e}")
    except Exception as e:
        logger.error(f"[bnvda] Lua TCP relay error: {e}")
    finally:
        _TCP_CLIENTS.discard(writer)
        writer.close()
        try:
            await writer.wait_closed()
        except Exception:
            pass
        logger.info(f"[bnvda] Lua TCP relay disconnected from {peer}.")


# ---------- Broadcast to connected clients ----------
def broadcast(msg_dict: dict):
    """Send a JSON message to all connected WebSocket clients (thread-safe)."""
    if not _LOOP or (not _CLIENTS and not _TCP_CLIENTS):
        return
    payload = json.dumps(msg_dict)
    async def _send_all():
        for ws in list(_CLIENTS):
            try:
                await ws.send_str(payload)
            except Exception:
                pass
        wire = payload.encode("utf-8") + b"\n"
        for writer in list(_TCP_CLIENTS):
            try:
                writer.write(wire)
                await writer.drain()
            except Exception:
                _TCP_CLIENTS.discard(writer)
    _LOOP.call_soon_threadsafe(asyncio.ensure_future, _send_all())


# ---------- Boot / Thread ----------
async def _start_all(host: str, ws_port: int):
    global _RUNNER, _TCP_SERVER
    app = web.Application()
    app.add_routes([web.get("/", ws_handler), web.get("/health", health_handler)])
    runner = web.AppRunner(app)
    _RUNNER = runner
    await runner.setup()
    site = web.TCPSite(runner, host, ws_port)
    await site.start()
    logger.info(f"BNVDA WebSocket server listening on ws://{host}:{ws_port}/")
    _TCP_SERVER = await asyncio.start_server(
        tcp_handler, host, TCP_PORT, limit=16 * 1024 * 1024
    )
    logger.info(f"BNVDA Lua TCP relay listening on {host}:{TCP_PORT}")


def _thread_target(host: str, ws_port: int):
    global _LOOP
    _LOOP = asyncio.new_event_loop()
    asyncio.set_event_loop(_LOOP)
    _LOOP.run_until_complete(_start_all(host, ws_port))
    _LOOP.run_forever()


def start_server_in_thread(speak_func, host: str = HOST, ws_port: int = WS_PORT):
    global _SPEAK_FUNC
    _SPEAK_FUNC = speak_func
    t = threading.Thread(
        target=_thread_target, args=(host, ws_port), name="bnvda-ws", daemon=True
    )
    t.start()

    def stop():
        try:
            if _LOOP:
                async def _shutdown():
                    for ws in list(_CLIENTS):
                        await ws.close()
                    for writer in list(_TCP_CLIENTS):
                        writer.close()
                    if _TCP_SERVER:
                        _TCP_SERVER.close()
                        await _TCP_SERVER.wait_closed()
                    if _RUNNER:
                        await _RUNNER.cleanup()
                    _LOOP.stop()

                asyncio.run_coroutine_threadsafe(_shutdown(), _LOOP)
        except Exception:
            pass

    return t, stop
