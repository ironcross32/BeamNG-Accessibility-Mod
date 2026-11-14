"""Golden cases for beam_engine::speech_out: _normalize_speech_value, say() and the tap.

    uv run python rust/fixtures/gen/speech_out.py

say() is driven with the speech backend replaced by a recorder, so nothing is spoken.
Timestamps are left out of the recorded tap entries; everything else is compared.
"""

from __future__ import annotations

import time

from _beamtel import beamtel
from _common import write_fixture

b = beamtel

# ---- _normalize_speech_value -----------------------------------------------------------
NORMALIZE = [
    "hello", "", "   ", " padded ", 0, 1, -7, 12345678901234, 1.0, 2.5, -0.0, 1e-05, 1e16,
    0.1, True, False, None, [], ["one"], ["a", "b"], [["nested"]], [[1, 2]], {}, {"k": "v"},
    {"a": 1, "b": 2}, {"k": ["x"]}, {"k": {"j": 3.5}}, [None], [{}], {"k": None},
    "table: 0x1a2b", " TABLE:0XFF ", "table:0x", "table: 0xzz", "table: 0x12 extra",
    "Table:   0xdeadbeef", "a table: 0x1", [True], {"k": False},
]
normalize = [{"in": v, "out": b._normalize_speech_value(v, "fixture")} for v in NORMALIZE]

# ---- say() -----------------------------------------------------------------------------
spoken = []
b.speech.speak = lambda text, interrupt=True: spoken.append([text, bool(interrupt)])


def reset():
    spoken.clear()
    b.SPEECH_BUFFER.clear()
    with b._speech_tap_lock:
        b.SPEECH_TAP.clear()
    b._command_context = False
    b._speech_protected_until = 0.0
    b._loading_active = False
    b._loading_settling = False


def tap():
    with b._speech_tap_lock:
        return [
            {k: e[k] for k in ("text", "source", "interrupt", "excluded", "spoken", "reason")}
            for e in b.SPEECH_TAP
        ]


SAY = [
    {"name": "plain", "text": "hello"},
    {"name": "strip", "text": "  hello there \n"},
    {"name": "blank", "text": "   "},
    {"name": "empty", "text": ""},
    {"name": "none", "text": None},
    {"name": "list_one", "text": ["one"]},
    {"name": "list_two", "text": ["a", "b"]},
    {"name": "dict_one", "text": {"k": "v"}},
    {"name": "int", "text": 42},
    {"name": "float", "text": 2.0},
    {"name": "bool", "text": True},
    {"name": "lua_pointer", "text": "table: 0x00ff"},
    {"name": "no_interrupt", "text": "quiet", "interrupt": False},
    {"name": "excluded", "text": "not kept", "exclude": True},
    {"name": "source", "text": "src", "source": "ui_udp.toastr"},
    {"name": "loading_suppressed", "text": "x", "loading": True},
    {"name": "settling_suppressed", "text": "x", "settling": True, "exclude": True},
    {"name": "loading_lifecycle_passes", "text": "Loading", "loading": True,
     "source": "loading_lifecycle", "exclude": True},
    {"name": "loading_command_context_passes", "text": "cmd", "loading": True,
     "command_context": True},
    {"name": "protected_demotes", "text": "bg", "protected": True},
    {"name": "protected_ui_bridge_keeps", "text": "ui", "protected": True, "source": "ui_bridge"},
    {"name": "protected_command_keeps", "text": "cmd", "protected": True,
     "command_context": True},
    {"name": "protected_no_interrupt", "text": "q", "protected": True, "interrupt": False},
    {"name": "protected_suppressed_while_loading", "text": "l", "protected": True,
     "loading": True},
]

say_cases = []
for case in SAY:
    reset()
    b._loading_active = bool(case.get("loading"))
    b._loading_settling = bool(case.get("settling"))
    b._command_context = bool(case.get("command_context"))
    if case.get("protected"):
        b._speech_protected_until = time.monotonic() + 1000.0
    b.say(
        case["text"],
        interrupt=case.get("interrupt", True),
        exclude_from_buffer=case.get("exclude", False),
        source=case.get("source", "python"),
    )
    say_cases.append({
        **case,
        "spoken": list(spoken),
        "tap": tap(),
        "buffer": list(b.SPEECH_BUFFER),
    })
reset()

# The replay buffer keeps the newest 100.
for i in range(105):
    b.say(f"line {i}")
buffer_overflow = list(b.SPEECH_BUFFER)
reset()

# ---- get_speech_log --------------------------------------------------------------------
b._speech_tap_seq = 0
for i in range(30):
    b._speech_tap_record(
        f"Entry {i} {'Alpha' if i % 3 == 0 else 'beta'}",
        "python" if i % 2 == 0 else "ui_bridge",
        i % 4 != 0,
        i % 5 == 0,
        i % 7 != 0,
        None if i % 7 != 0 else "loading_suppressed",
    )

QUERIES = [
    {},
    {"last_n": 0},
    {"last_n": None},
    {"last_n": 5},
    {"last_n": -3},
    {"last_n": 100},
    {"since_seq": 10},
    {"since_seq": 0},
    {"since_seq": 30},
    {"since_seq": 99},
    {"since_seq": -5},
    {"source": "ui_bridge"},
    {"source": ""},
    {"contains": "ALPHA"},
    {"contains": ""},
    {"contains": "entry 2"},
    {"spoken_only": True},
    {"since_seq": 20, "source": "python", "spoken_only": True, "last_n": 2},
]


def log_out(q):
    r = b.get_speech_log(**q)
    return {
        "seqs": [e["seq"] for e in r["entries"]],
        "next_seq": r["next_seq"],
        "dropped": r["dropped"],
    }


queries = [{"q": q, "out": log_out(q)} for q in QUERIES]
entries = [
    {k: e[k] for k in ("seq", "text", "source", "interrupt", "excluded", "spoken", "reason")}
    for e in b.get_speech_log(last_n=0)["entries"]
]

# Overflow: 1000 kept, cursor before the oldest reports dropped.
with b._speech_tap_lock:
    b.SPEECH_TAP.clear()
b._speech_tap_seq = 0
for i in range(1005):
    b._speech_tap_record(f"o{i}", "python", True, False, True)
overflow = [{"q": q, "out": log_out(q)} for q in (
    {"since_seq": 0, "last_n": 3}, {"since_seq": 4, "last_n": 3}, {"since_seq": 5, "last_n": 3},
    {"since_seq": 6, "last_n": 3}, {"last_n": 0},
)]

write_fixture("speech_out", {
    "normalize": normalize,
    "say": say_cases,
    "buffer_overflow": buffer_overflow,
    "log_entries": entries,
    "log_queries": queries,
    "log_overflow": overflow,
})
