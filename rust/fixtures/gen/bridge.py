"""Golden cases for beam_engine::ui_bridge against nvda_ws_speaker.py.

    uv run python rust/fixtures/gen/bridge.py

* ``messages``: sequences of decoded messages through the real ``handle_ws_message``,
  with a recorder registered for every callback and for the speech function. After each
  message: the calls it made, the log records it wrote (level and message), the
  exception it raised if any, and ``bnvda_debug_enabled()``.
* ``tcp``: raw lines through the real ``tcp_handler`` (an asyncio StreamReader and a
  recording writer): what it wrote back, the calls, and whether an exception ended the
  connection, which drops every later line.

``hover`` / ``hover_cancel`` are left out: they schedule on the bridge's event loop.
"""

from __future__ import annotations

import asyncio
import logging

from _common import write_fixture

import nvda_ws_speaker as ws

calls: list = []
logs: list = []


class Capture(logging.Handler):
    def emit(self, record):
        logs.append([record.levelname, record.getMessage()])


ws.logger.addHandler(Capture(level=logging.INFO))


def rec(name):
    return lambda *args: calls.append([name, *args])


ws._SPEAK_FUNC = lambda text, interrupt: calls.append(["speak", text, interrupt])
ws.register_dom_dump_callback(rec("dom_dump"))
ws.register_loading_state_callback(rec("loading_state"))
ws.register_settings_request_callback(rec("settings_request"))
ws.register_screen_context_callback(rec("screen_context"))
ws.register_page_text_callback(rec("page_text"))
ws.register_accessibility_action_callback(rec("accessibility_action"))
ws.register_challenge_event_callback(rec("challenge_event"))
ws.register_drift_event_callback(rec("drift_event"))


def reset():
    ws._SPEAK_LAST = ""
    ws._SPEAK_LAST_TS = 0.0
    ws._BNVDA_DEBUG = False
    calls.clear()
    logs.clear()


def run_messages(name, messages):
    reset()
    steps = []
    for m in messages:
        calls.clear()
        logs.clear()
        raised = None
        try:
            ws.handle_ws_message(m)
        except Exception as exc:  # noqa: BLE001 -- the transport decides what this does
            raised = [type(exc).__name__, str(exc)]
        steps.append({"message": m, "calls": list(calls), "logs": list(logs),
                      "raised": raised, "debug": ws.bnvda_debug_enabled()})
    return {"name": name, "steps": steps}


class Writer:
    def __init__(self):
        self.written = []

    def get_extra_info(self, key):
        return ("127.0.0.1", 1)

    def write(self, data):
        self.written.append(data.decode("utf-8"))

    async def drain(self):
        pass

    def close(self):
        pass

    async def wait_closed(self):
        pass


def run_tcp(name, lines):
    reset()

    async def go():
        reader = asyncio.StreamReader()
        for line in lines:
            reader.feed_data(line)
        reader.feed_eof()
        writer = Writer()
        await ws.tcp_handler(reader, writer)
        return writer.written

    written = asyncio.run(go())
    errors = [m for lvl, m in logs if m.startswith("[bnvda] Lua TCP relay error: ")]
    return {"name": name, "lines": [l.decode("latin-1") for l in lines],
            "written": written, "calls": list(calls),
            "error": errors[0][len("[bnvda] Lua TCP relay error: "):] if errors else None,
            "invalid": [m for lvl, m in logs if m.startswith("[bnvda] Invalid Lua TCP")]}


S = lambda text, **kw: {"type": "speak", "text": text, **kw}  # noqa: E731

messages = [
    run_messages("speak", [
        S("Hello"), S("Hello"), S("Hello", interrupt=False), S("  padded  "), S(""),
        S("   "), {"type": "speak"}, S(5), S(None), S(["a"]), S("x", interrupt="yes"),
        S("y", interrupt=0), S(".foo { color: red; }"), S("a } b { c: d;"),
        S("Plain text with a { brace"),
    ]),
    run_messages("log", [
        {"type": "log", "level": "info", "msg": "one"},
        {"type": "log", "level": "WARNING", "msg": "two"},
        {"type": "log", "level": "error", "msg": "three"},
        # "critical" is left out: Python logs CRITICAL, Rust's log crate has no such level
        # and writes ERROR. bnvdaRuntime.js only ever sends info, warn and error.
        {"type": "log", "level": "debug", "msg": "five"},
        {"type": "log", "level": "nope", "msg": "six"},
        {"type": "log", "msg": 7},
        {"type": "log", "level": 3, "msg": "eight"},
        {"type": "log", "level": "warn", "msg": "nine"},
        {"type": "log", "level": "handlers", "msg": "ten"},
    ]),
    run_messages("debug_state", [
        {"type": "debug_state", "enabled": True}, {"type": "debug_state", "enabled": True},
        {"type": "debug_state", "enabled": False}, {"type": "debug_state", "enabled": "x"},
        {"type": "debug_state"},
    ]),
    run_messages("callbacks", [
        {"type": "dom_dump_result", "lines": ["a", "b"]},
        {"type": "dom_dump_result"},
        {"type": "settings_request"},
        {"type": "loading_state", "active": True, "focusText": "Gridmap"},
        {"type": "loading_state", "active": "yes", "focusText": None},
        {"type": "loading_state"},
        {"type": "screen_context", "context": "mod_details", "title": "A mod"},
        {"type": "screen_context", "context": None, "title": 5},
        {"type": "page_text", "lines": ["one", 2, None], "title": "T", "code": "", "sentence": "s"},
        {"type": "page_text", "lines": "not a list", "title": None, "code": "nopage"},
        {"type": "challenge_event", "event": "start", "id": 3},
        {"type": "drift_event", "score": 1.5},
    ]),
    run_messages("actions", [
        {"type": "accessibility_action", "action": "status_up"},
        {"type": "accessibility_action", "action": "activate"},
        {"type": "accessibility_action", "action": "bogus"},
        {"type": "accessibility_action"},
        {"type": "accessibility_action", "action": ["status_up"]},
    ]),
    run_messages("ignored", [
        {"type": "nope"}, {"text": "no type"}, ["not", "a", "dict"], "text", None,
        {"type": None},
    ]),
]

tcp = [
    run_tcp("ping and messages", [
        b'{"type":"transport_ping"}\n',
        b'{"type":"speak","text":"Hi"}\n',
        b'{"type":"accessibility_action","action":"next_menu"}\n',
    ]),
    run_tcp("bad lines are skipped", [
        b"not json\n", b"\xff\xfe\n", b'{"type":"speak","text":"after"}\n',
    ]),
    run_tcp("a non-object ends the connection", [
        b'{"type":"speak","text":"first"}\n', b"[1, 2]\n",
        b'{"type":"speak","text":"never"}\n',
    ]),
    run_tcp("an unhashable action ends the connection", [
        b'{"type":"accessibility_action","action":["x"]}\n',
        b'{"type":"speak","text":"never"}\n',
    ]),
]

write_fixture("bridge", {"messages": messages, "tcp": tcp})
