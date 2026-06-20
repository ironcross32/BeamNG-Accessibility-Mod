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

# Initialize the shared logger
logger = get_logger()

# ---------- Global state ----------
_LOOP: asyncio.AbstractEventLoop | None = None
_SPEAK_FUNC = None
_SPEAK_LAST = ""
_HOVER_TASK: asyncio.Task | None = None
_HOVER_TOKEN = None
_DETAILS_CALLBACK = None
_VEHICLE_SELECTOR_CALLBACK = None
_DOM_DUMP_CALLBACK = None

_CLIENTS: set = set()  # connected WebSocket instances


def register_details_callbacks(on_details, on_selector_state):
    global _DETAILS_CALLBACK, _VEHICLE_SELECTOR_CALLBACK
    _DETAILS_CALLBACK = on_details
    _VEHICLE_SELECTOR_CALLBACK = on_selector_state


def register_dom_dump_callback(callback):
    global _DOM_DUMP_CALLBACK
    _DOM_DUMP_CALLBACK = callback



# ---------- Menu navigation key injection ----------
# BeamNG's in-menu navigation is driven by the game engine, not by DOM events:
# the UI only emits *untrusted* echo keydown events after the engine has already
# moved the crossfire focus, so synthetic DOM keys (or .focus()) cannot drive it.
# To accelerate list navigation we therefore inject *real* OS-level numpad
# keystrokes, which the game reads as genuine input. The UI hook (app.js) detects
# a held direction in a dropdown and asks us to inject N extra steps.
#
# Numpad scan codes (Set 1, non-extended): Numpad2 = 0x50 (down/next),
# Numpad8 = 0x48 (up/prev). Non-extended is what distinguishes the numpad keys
# from the arrow keys, which is exactly what BeamNG binds menu navigation to.
_NAV_SCAN_DOWN = 0x50  # Numpad 2
_NAV_SCAN_UP = 0x48    # Numpad 8
# Space presses out so the engine registers each as a discrete menu step rather
# than coalescing a same-frame burst (the game samples input ~per frame).
_NAV_INJECT_SPACING = 0.018
_NAV_INJECT_MAX = 40


def _inject_nav_keys(direction, count):
    scan = _NAV_SCAN_DOWN if direction > 0 else _NAV_SCAN_UP
    try:
        n = int(count)
    except (TypeError, ValueError):
        return
    n = max(0, min(n, _NAV_INJECT_MAX))
    if n <= 0:
        return

    def _run():
        try:
            import keyboard
        except Exception as e:  # pragma: no cover - keyboard always present at runtime
            logger.error(f"[nav_inject] keyboard import failed: {e}")
            return
        for _ in range(n):
            try:
                keyboard.press(scan)
                keyboard.release(scan)
            except Exception as e:
                logger.error(f"[nav_inject] key send failed: {e}")
                return
            time.sleep(_NAV_INJECT_SPACING)

    threading.Thread(target=_run, name="nav-inject", daemon=True).start()


# ---------- Core helpers ----------
def engine_offer(text: str, interrupt: bool = True):
    global _SPEAK_LAST
    t = (text or "").strip()
    if not t:
        return
    if _looks_like_css(t):
        logger.info("[bnvda] Suppressed CSS-like text from UI hook.")
        return
    # Only dedup against the last utterance for interrupting speech. A queued
    # follow-up (interrupt=False, e.g. a tuning hint) is intentionally a second
    # utterance and must not be swallowed by this guard.
    if interrupt and t == _SPEAK_LAST:
        return
    _SPEAK_LAST = t
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
    elif msg_type == "nav_inject":
        _inject_nav_keys(data.get("dir", 1), data.get("count", 1))
    elif msg_type == "hover":
        hover_on(
            data.get("text", ""), item_id=data.get("id"), delay_ms=data.get("delay_ms")
        )
    elif msg_type == "hover_cancel":
        hover_cancel()
    elif msg_type == "vehicle_details":
        if _DETAILS_CALLBACK:
            try:
                _DETAILS_CALLBACK(data.get("lines", []))
            except Exception as e:
                logger.error(f"vehicle_details callback error: {e}")
    elif msg_type == "vehicle_selector_state":
        if _VEHICLE_SELECTOR_CALLBACK:
            try:
                _VEHICLE_SELECTOR_CALLBACK(data.get("open", False))
            except Exception as e:
                logger.error(f"vehicle_selector_state callback error: {e}")
    elif msg_type == "dom_dump_result":
        if _DOM_DUMP_CALLBACK:
            try:
                _DOM_DUMP_CALLBACK(data.get("lines", []))
            except Exception as e:
                logger.error(f"dom_dump_result callback error: {e}")


# ---------- WebSocket Server Logic ----------
async def ws_handler(request: web.Request):
    ws = web.WebSocketResponse(heartbeat=30)
    await ws.prepare(request)
    _CLIENTS.add(ws)
    logger.info("WebSocket client connected.")
    try:
        async for msg in ws:
            if msg.type == WSMsgType.TEXT:
                try:
                    handle_ws_message(json.loads(msg.data))
                except Exception:
                    pass
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


# ---------- Broadcast to connected clients ----------
def broadcast(msg_dict: dict):
    """Send a JSON message to all connected WebSocket clients (thread-safe)."""
    if not _LOOP or not _CLIENTS:
        return
    payload = json.dumps(msg_dict)
    async def _send_all():
        for ws in list(_CLIENTS):
            try:
                await ws.send_str(payload)
            except Exception:
                pass
    _LOOP.call_soon_threadsafe(asyncio.ensure_future, _send_all())


# ---------- Boot / Thread ----------
async def _start_all(host: str, ws_port: int):
    app = web.Application()
    app.add_routes([web.get("/", ws_handler)])
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, host, ws_port)
    await site.start()
    logger.info(f"BNVDA WebSocket server listening on ws://{host}:{ws_port}/")


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
                _LOOP.call_soon_threadsafe(_LOOP.stop)
        except Exception:
            pass

    return t, stop
