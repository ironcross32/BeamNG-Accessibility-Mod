# ws-speaker (class-free)
# WebSocket-Only version
# Designed to be embedded: start_server_in_thread(lambda text: say(text))

import asyncio, json, os, threading
from aiohttp import web, WSMsgType
from bnh_logger import get_logger

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


# ---------- Core helpers ----------
def engine_offer(text: str):
    global _SPEAK_LAST
    t = (text or "").strip()
    if not t:
        return
    if t == _SPEAK_LAST:
        return
    _SPEAK_LAST = t
    try:
        _SPEAK_FUNC(t)
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
        # Speech logging is now handled by the 'say' function in beamtel.py
        engine_offer(data.get("text", ""))
    elif msg_type == "log":
        level = str(data.get("level", "INFO")).lower()
        msg = str(data.get("msg", ""))
        getattr(logger, level, logger.info)(msg)
    elif msg_type == "hover":
        hover_on(
            data.get("text", ""), item_id=data.get("id"), delay_ms=data.get("delay_ms")
        )
    elif msg_type == "hover_cancel":
        hover_cancel()


# ---------- WebSocket Server Logic ----------
async def ws_handler(request: web.Request):
    ws = web.WebSocketResponse(heartbeat=30)
    await ws.prepare(request)
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
        logger.info("WebSocket client disconnected.")
        try:
            await ws.close()
        except Exception:
            pass
    return ws


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
