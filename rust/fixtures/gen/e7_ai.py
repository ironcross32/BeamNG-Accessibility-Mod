"""Golden cases for E7: the F10 layer, AI status speech, the driving assistant's
callbacks, both AI Describer request builders, key validation, the describer worker and
its log lines.

Speech, every UDP sender, the staggered preset worker, the describer trigger, the screen
grab and urllib's urlopen are replaced by recorders; nothing reaches the network, the
screen or the game, and no keyboard hook is installed.

    uv run python rust/fixtures/gen/e7_ai.py
"""

from __future__ import annotations

import io
import os
import re
import time
import urllib.error
from types import SimpleNamespace

from _beamtel import beamtel as b
from _common import fbits, write_fixture

import ai_describer as ad

assert ad.LOG_PATH.startswith(os.environ["LOCALAPPDATA"]), ad.LOG_PATH

spoken = []
sent = []
staggered = []
events = []

TOKEN = re.compile(r"[0-9a-f]{32}")


def rec_say(text, interrupt=True, exclude_from_buffer=False, **_k):
    spoken.append([text, interrupt, exclude_from_buffer])


def install_recorders():
    b.say = rec_say
    b._send_ai_command = lambda c: sent.append([4449, c])
    b._send_slot_command = lambda c: sent.append([4459, c])
    b._send_ui_command = lambda c: sent.append([4464, c])
    b.driving_assistant.send = lambda c: sent.append([4449, TOKEN.sub("<token>", c)])
    b.driving_assistant.speak = lambda text, **k: rec_say(text, **k)
    b._run_staggered = lambda cmds: staggered.append(list(cmds))
    b._trigger_ai_describe = lambda ac: events.append("describe")


# ---- F10 key dispatch ---------------------------------------------------------------------

SLOTS4 = [[1, 101, "Alpha"], [2, 102, "Bravo"], [3, 103, "Charlie"], [10, 110, "Juliet"]]
# Small ids, so Python's set iteration order equals insertion order for the lightbars.
SLOTS_SMALL = [[1, 3, "Alpha"], [2, 4, "Bravo"], [3, 5, "Charlie"], [4, 6, "Delta"]]


def k(name, ctrl=False, shift=False, alt=False):
    return {"key": name, "ctrl": ctrl, "shift": shift, "alt": alt}


KEY_SCENARIOS = [
    {"name": "assistant_toggle", "slots": [], "steps": [k("h"), k("h")]},
    {
        "name": "basic_imperial",
        "slots": [],
        "steps": [k("d"), k("t"), k("r"), k("s"), k("c"), k("f"), k("e")]
        + [k(str(i)) for i in range(1, 10)]
        + [k("-"), k("+"), k("+"), k("="), k("add"), k("-"), k("subtract"), k("-"), k("-"),
           k("0"), k("a"), k("a"), k("a"), k("a"), k("l"), k("l"), k("space"), k("x"),
           k("f10"), k("ctrl"), k("left shift"), k("right alt"), k("A")],
    },
    {"name": "metric_speed", "units": "metric", "slots": [],
     "steps": [k("+"), k("+"), k("+"), k("-")]},
    {
        "name": "help_mode",
        "slots": [],
        "steps": [k("/"), k("h"), k("a", ctrl=True), k("add"), k("subtract"), k("1", alt=True),
                  k("1", shift=True), k("0", ctrl=True), k("z"), k("space"), k("?"), k("t")],
    },
    {
        "name": "slots_selection",
        "slots": SLOTS4,
        "steps": [k("1", shift=True), k("2", shift=True), k("@"), k("5", shift=True),
                  k("3", ctrl=True), k("3", shift=True), k("a", ctrl=True), k("2", ctrl=True),
                  k("x"), k("2", ctrl=True), k("1", ctrl=True), k("2", ctrl=True),
                  k("2", ctrl=True), k("c"), k("f"), k("e"), k("t"), k("d"),
                  k("n", ctrl=True), k("c"), k("0", shift=True), k("0", ctrl=True), k(")"),
                  k("1", ctrl=True, shift=True)],
    },
    {
        "name": "chase_without_target",
        "slots": SLOTS4,
        "steps": [k("1", shift=True), k("c"), k("2", shift=True), k("f"), k("e"), k("r")],
    },
    {
        "name": "presets",
        "slots": SLOTS_SMALL,
        "steps": [k("1", alt=True), k("2", alt=True), k("3", alt=True), k("4", alt=True),
                  k("5", alt=True), k("4", ctrl=True), k("5", alt=True), k("1", alt=True),
                  k("5", alt=True), k("t"), k("5", alt=True), k("6", alt=True),
                  k("5", alt=True, ctrl=True)],
    },
    {
        "name": "presets_short",
        "slots": [[2, 7, "Solo"]],
        "steps": [k("1", alt=True), k("2", alt=True), k("3", alt=True), k("4", alt=True),
                  k("5", alt=True), k("2", ctrl=True), k("5", alt=True)],
    },
    {"name": "presets_slot1_only", "slots": [[1, 9, "Only"]],
     "steps": [k("2", alt=True), k("4", alt=True)]},
    {"name": "presets_empty", "slots": [], "steps": [k("1", alt=True), k("5", alt=True)]},
    {
        "name": "odd_names",
        "slots": SLOTS4,
        "steps": [k("", shift=True), k("", ctrl=True), k("", alt=True), k(""),
                  k("12", shift=True), k("23", ctrl=True), k("45", alt=True)],
    },
]


def apply_ai(sc):
    b.UNITS_MODE = sc.get("units", "imperial")
    b._vehicle_slots = {s: {"id": i, "name": n} for s, i, n in sc["slots"]}
    b._selected_slots = set()
    b._target_slot = None
    b._pending_target_confirm = None
    b.ai_speed_limit_ms = None
    b.ai_avoid_mode = "auto"
    b.ai_lane_driving = False
    b._police_lights_active = set()
    b._ai_vehicle_commands = {}
    b._input_help_mode = False
    b.ai_timer = None
    b._command_context = True


def ai_state():
    return {
        "selected": sorted(b._selected_slots),
        "target": b._target_slot,
        "pending": b._pending_target_confirm,
        "speed": None if b.ai_speed_limit_ms is None else fbits(b.ai_speed_limit_ms),
        "avoid": b.ai_avoid_mode,
        "lane": b.ai_lane_driving,
        "help": b._input_help_mode,
        "police": sorted(b._police_lights_active),
        "commands": [[vid, c["mode"], c["target_id"]] for vid, c in b._ai_vehicle_commands.items()],
        "command_context": b._command_context,
    }


def run_keys(sc):
    apply_ai(sc)
    steps = []
    for st in sc["steps"]:
        spoken.clear(); sent.clear(); staggered.clear(); events.clear()
        b._ai_mods.update(ctrl=st["ctrl"], shift=st["shift"], alt=st["alt"])
        b._kb_layer = "f10"
        err = None
        try:
            b._on_ai_key_press(SimpleNamespace(event_type="down", name=st["key"]))
        except Exception as exc:  # noqa: BLE001
            err = str(exc)
        steps.append({
            **st,
            "spoken": list(spoken),
            "sent": list(sent),
            "staggered": list(staggered),
            "describe": "describe" in events,
            "layer_open": b._kb_layer == "f10",
            "err": err,
            "state": ai_state(),
        })
    return {"name": sc["name"], "units": sc.get("units", "imperial"), "slots": sc["slots"],
            "steps": steps}


# ---- Status speech, callbacks ------------------------------------------------------------

STATUS_ALL = [
    "", "no vehicles",
    "1,Alpha,chase,Bravo|2,Bravo,flee,|10,Juliet,traffic",
    "3,X,follow,Y,Z",
    "bad|1,A",
    "1,A,unknown|2,B,no response|3,C,weird|4,D,disabled|5,E,stop|6,F,random|7,G,chase|8,H,follow|9,I,flee,J",
    "||",
]


def status_all_cases():
    out = []
    for p in STATUS_ALL:
        spoken.clear()
        b._speak_status_all_response(p)
        out.append({"payload": p, "spoken": list(spoken)})
    return out


AI_STATUS = [
    {"slots": [], "commands": []},
    {"slots": SLOTS4, "commands": [[101, "chase", 102], [102, "follow", 999], [103, "flee", None],
                                   [999, "stop", None]]},
    {"slots": SLOTS4, "commands": [[101, "disabled", None], [102, "traffic", None],
                                   [103, "random", None], [110, "mystery", None]]},
]


def ai_status_cases():
    out = []
    for c in AI_STATUS:
        b._vehicle_slots = {s: {"id": i, "name": n} for s, i, n in c["slots"]}
        b._ai_vehicle_commands = {v: {"mode": m, "target_id": t} for v, m, t in c["commands"]}
        spoken.clear()
        b._speak_ai_status()
        out.append({**c, "spoken": list(spoken)})
    return out


ARRIVALS = [
    {"offRoad": True, "x": 1, "y": 2.5},
    {"offRoad": False, "x": 1, "y": 2},
    {"offRoad": True, "x": "1", "y": 2},
    {"offRoad": 1, "x": True, "y": -3},
    {"offRoad": True, "x": 4.25},
    {},
]


def arrival_cases():
    out = []
    for a in ARRIVALS:
        b.marked_coord_x = None
        b.marked_coord_y = None
        b._last_coord_bearing_ts = 5.0
        b._assistant_arrival(a)
        out.append({
            "arrival": a,
            "x": None if b.marked_coord_x is None else fbits(b.marked_coord_x),
            "y": None if b.marked_coord_y is None else fbits(b.marked_coord_y),
            "ts": fbits(b._last_coord_bearing_ts),
        })
    return out


class FakeController:
    def __init__(self):
        self.calls = []

    def trigger_assistant_cue(self, kind, choices):
        self.calls.append([kind, list(choices)])


def cue_cases():
    out = []
    for kind, choices in [("off", []), ("protect", []), ("junction", ["left", "right"]),
                          ("junction", ["straight"]), ("other", [])]:
        fake = FakeController()
        b.audio_controller_ref = fake
        b._speech_protected_until = 0.0
        b._assistant_cue(kind, tuple(choices))
        out.append({"kind": kind, "choices": choices,
                    "protected": b._speech_protected_until > time.monotonic(),
                    "called": bool(fake.calls)})
    b.audio_controller_ref = None
    return out


# ---- HTTP ----------------------------------------------------------------------------------

PNG = b"\x89PNG\r\n\x1a\nfake-image\x00\xff"
requests = []
next_response = []


class FakeResp:
    def __init__(self, status, body):
        self.status = status
        self._body = body

    def read(self):
        return self._body

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


def fake_urlopen(req, timeout=None):
    headers = {k.lower(): v for k, v in req.header_items()}
    requests.append({
        "method": req.get_method(),
        "url": req.full_url,
        "headers": {h: headers[h] for h in ("content-type", "authorization") if h in headers},
        "body": None if req.data is None else req.data.decode("utf-8"),
        "timeout": fbits(timeout),
    })
    spec = next_response[0]
    kind = spec["kind"]
    body = spec.get("body", "").encode("utf-8")
    if kind == "ok":
        return FakeResp(spec.get("status", 200), body)
    if kind == "http":
        raise urllib.error.HTTPError(req.full_url, spec["code"], "err", {}, io.BytesIO(body))
    if kind == "url":
        raise urllib.error.URLError(spec["reason"])
    raise Exception(spec["msg"])


ad.urllib.request.urlopen = fake_urlopen


def gem_ok(parts, finish="STOP"):
    import json
    return {"kind": "ok", "body": json.dumps({"candidates": [{"content": {"parts": parts},
                                                               "finishReason": finish}]})}


def oai_ok(obj):
    import json
    return {"kind": "ok", "body": json.dumps(obj)}


VALIDATIONS = [
    {"provider": "gemini", "key": "   ", "opts": {}, "resp": {"kind": "ok"}},
    {"provider": "gemini", "key": " gk+/ &é ", "opts": {"model": "x"}, "resp": {"kind": "ok"}},
    {"provider": "gemini", "key": "gk", "opts": {},
     "resp": {"kind": "http", "code": 400, "body": '{"error": {"message": "API key not valid."}}'}},
    {"provider": "gemini", "key": "gk", "opts": {}, "resp": {"kind": "http", "code": 403, "body": ""}},
    {"provider": "gemini", "key": "gk", "opts": {},
     "resp": {"kind": "http", "code": 500, "body": "  plain failure \n"}},
    {"provider": "gemini", "key": "gk", "opts": {},
     "resp": {"kind": "http", "code": 500, "body": '{"error": "flat"}'}},
    {"provider": "gemini", "key": "gk", "opts": {},
     "resp": {"kind": "http", "code": 500, "body": '{"error": {"message": 42}}'}},
    {"provider": "gemini", "key": "gk", "opts": {}, "resp": {"kind": "url", "reason": "timed out"}},
    {"provider": "gemini", "key": "gk", "opts": {}, "resp": {"kind": "exc", "msg": "timed out"}},
    {"provider": "openai", "key": "sk", "opts": {"model": "gpt-4o", "base_url": "https://proxy.example/v1/",
                                                  "reasoning_effort": "high", "detail": "low"},
     "resp": {"kind": "ok"}},
    {"provider": "openai", "key": "sk", "opts": {}, "resp": {"kind": "ok", "body": "{}"}},
    {"provider": "openai", "key": "", "opts": {}, "resp": {"kind": "ok"}},
    {"provider": "openai", "key": "sk", "opts": {"model": ""},
     "resp": {"kind": "http", "code": 401, "body": '{"error": {"message": "Incorrect API key"}}'}},
    {"provider": "openai", "key": "sk", "opts": {}, "resp": {"kind": "url", "reason": "Timed Out"}},
    {"provider": "openai", "key": "sk", "opts": {}, "resp": {"kind": "url", "reason": "refused"}},
    {"provider": "openai", "key": "sk", "opts": {}, "resp": {"kind": "exc", "msg": "reset"}},
    {"provider": "openai", "key": "sk", "opts": {}, "resp": {"kind": "ok", "body": "not json"}},
    {"provider": "openai", "key": "sk", "opts": {"base_url": "api.example.com/v1"}, "resp": {"kind": "ok"}},
    {"provider": "openai", "key": "sk", "opts": {"base_url": "  "}, "resp": {"kind": "ok", "body": "{}"}},
    {"provider": "nobody", "key": "gk", "opts": {}, "resp": {"kind": "ok"}},
]


def validation_cases():
    out = []
    for v in VALIDATIONS:
        requests.clear()
        next_response[:] = [v["resp"]]
        try:
            ok, err = ad.validate_api_key(v["key"], provider=v["provider"], **v["opts"])
            res = {"ok": ok, "err": err}
        except Exception as exc:  # noqa: BLE001
            res = {"raised": str(exc)}
        out.append({**v, "result": res, "requests": list(requests)})
    return out


def log_lines():
    if not os.path.exists(ad.LOG_PATH):
        return []
    with open(ad.LOG_PATH, "rb") as fh:
        data = fh.read().decode("utf-8")
    os.remove(ad.LOG_PATH)
    return [re.sub(r"^\[\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\] ", "", ln) for ln in data.split("\r\n")]


OPENAI_EXTRAS = {"ai_describer_openai_base_url": "https://api.openai.com/v1",
                 "ai_describer_openai_reasoning_effort": "low",
                 "ai_describer_openai_detail": "auto"}

WORKERS = [
    {"name": "gemini_no_key", "provider": "gemini", "settings": {"ai_describer_api_key": "  "}},
    {"name": "openai_no_key", "provider": "openai", "settings": {}},
    {"name": "provider_empty_falls_back", "provider": "", "settings": {}},
    {"name": "undecryptable", "provider": "gemini", "settings": {"ai_describer_api_key": "dpapi:v1:AAAA"}},
    {"name": "capture_fails", "provider": "gemini", "settings": {"ai_describer_api_key": "gk"},
     "capture": "boom"},
    {"name": "gemini_ok", "provider": "gemini",
     "settings": {"ai_describer_api_key": " gk ", "ai_describer_model": "models/gemini-3-flash-preview"},
     "resp": gem_ok([{"text": "thinking", "thought": True}, {"text": " A road. "},
                     {"text": "Trees."}])},
    {"name": "gemini_25_flash_http", "provider": "gemini",
     "settings": {"ai_describer_api_key": "gk", "ai_describer_model": "models/gemini-2.5-flash"},
     "resp": {"kind": "http", "code": 429, "body": '{"error": {"message": "Quota exceeded."}}'}},
    {"name": "gemini_20_blocked", "provider": "gemini",
     "settings": {"ai_describer_api_key": "gk", "ai_describer_model": "models/gemini-2.0-flash"},
     "resp": {"kind": "ok", "body": '{"promptFeedback": {"blockReason": "SAFETY"}}'}},
    {"name": "gemini_no_candidates", "provider": "gemini",
     "settings": {"ai_describer_api_key": "gk", "ai_describer_model": "models/gemini-pro-latest"},
     "resp": {"kind": "ok", "body": '{"candidates": []}'}},
    {"name": "gemini_max_tokens", "provider": "gemini", "settings": {"ai_describer_api_key": "gk"},
     "resp": gem_ok([{"text": "x", "thought": True}], "MAX_TOKENS")},
    {"name": "gemini_empty_other", "provider": "gemini", "settings": {"ai_describer_api_key": "gk"},
     "resp": {"kind": "ok", "body": '{"candidates": [{"content": {}}]}'}},
    {"name": "gemini_timeout", "provider": "gemini", "settings": {"ai_describer_api_key": "gk"},
     "resp": {"kind": "exc", "msg": "The read operation timed out"}},
    {"name": "gemini_list_result", "provider": "gemini", "settings": {"ai_describer_api_key": "gk"},
     "resp": {"kind": "ok", "body": "[1, 2]"}},
    {"name": "gemini_non_str_text", "provider": "gemini", "settings": {"ai_describer_api_key": "gk"},
     "resp": gem_ok([{"text": "a"}, {"text": 5}])},
    {"name": "gemini_int_model", "provider": "gemini",
     "settings": {"ai_describer_api_key": "gk", "ai_describer_model": 5}},
    {"name": "int_key", "provider": "gemini", "settings": {"ai_describer_api_key": 7}},
    {"name": "no_ui_toggle", "provider": "gemini", "settings": {"ai_describer_api_key": "gk"},
     "disable": True, "resp": gem_ok([{"text": "Night."}])},
    {"name": "openai_ok_reasoning", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1",
                  "ai_describer_openai_model": "gpt-5.6-sol",
                  "ai_describer_openai_reasoning_effort": "high",
                  "ai_describer_openai_detail": "low",
                  "ai_describer_openai_base_url": " https://proxy.example/v1/ "},
     "resp": oai_ok({"output": [{"type": "reasoning", "content": [{"type": "output_text", "text": "no"}]},
                                {"type": "message", "content": [{"type": "output_text", "text": "A car. "},
                                                                {"type": "output_text", "text": "Rain."}]}]})},
    {"name": "openai_gpt4o_bad_choices", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1",
                  "ai_describer_openai_model": "gpt-4o",
                  "ai_describer_openai_reasoning_effort": "extreme",
                  "ai_describer_openai_detail": "weird"},
     "resp": oai_ok({"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}})},
    {"name": "openai_none_effort_incomplete", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1",
                  "ai_describer_openai_model": "o3",
                  "ai_describer_openai_reasoning_effort": "none"},
     "resp": oai_ok({"status": "incomplete", "incomplete_details": {"reason": "content_filter"}})},
    {"name": "openai_refusal", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1"},
     "resp": oai_ok({"output": [{"type": "message", "content": [{"type": "refusal", "refusal": "No."}]}]})},
    {"name": "openai_refusal_empty", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1"},
     "resp": oai_ok({"output": [{"type": "message", "content": [{"type": "refusal"}]}]})},
    {"name": "openai_empty", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1"},
     "resp": oai_ok({"status": "completed"})},
    {"name": "openai_url_timeout", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1"},
     "resp": {"kind": "url", "reason": "timed out"}},
    {"name": "openai_other", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1"},
     "resp": {"kind": "exc", "msg": "connection reset"}},
    {"name": "openai_no_scheme", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1",
                  "ai_describer_openai_base_url": "api.example.com/v1"}},
    {"name": "openai_int_base_url", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1",
                  "ai_describer_openai_base_url": 5}},
    {"name": "openai_multiline", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1"},
     "resp": oai_ok({"output": [{"type": "message", "content": [
         {"type": "output_text", "text": "Line one.\nLine two.\r\n"}]}]})},
    {"name": "openai_error_multiline", "provider": "openai",
     "settings": {**OPENAI_EXTRAS, "ai_describer_openai_api_key": "sk-1"},
     "resp": {"kind": "http", "code": 500, "body": "  bad\ngateway  "}},
]


def worker_cases():
    out = []
    for w in WORKERS:
        spoken.clear(); sent.clear(); requests.clear()
        next_response[:] = [w.get("resp", {"kind": "exc", "msg": "no response configured"})]
        b.ai_describer_provider = w["provider"]
        b.ai_describer_settings = dict(w["settings"])
        b.ai_describer_disable_ui_toggle = w.get("disable", False)

        def grab(region=None, _w=w):
            if "capture" in _w:
                raise RuntimeError(_w["capture"])
            assert region is None
            return PNG

        ad.capture_region = grab
        b._describer_busy = True
        b._ai_describe_worker(None)
        assert b._describer_busy is False
        out.append({**w, "spoken": list(spoken), "sent": list(sent),
                    "requests": list(requests), "log": log_lines()})
    return out


def misc():
    return {
        "quote": [[s, ad.urllib.parse.quote(s)] for s in
                  ["abc", "a b/c?d=e&f", "-_.~", "é€😀", "+%", ""]],
        "openai_base": [[s, ad._openai_base(s)] for s in
                        [None, "", "  ", "https://x/v1/", " https://x/v1// ", "http://y"]],
        "log": [],
    }


def log_cases():
    ad.log_description("line1\nline2  \n")
    ad.log_error("  err\n x \r ")
    ad.log_description(None)
    ad.log_error("")
    return log_lines()


def main():
    install_recorders()
    if os.path.exists(ad.LOG_PATH):
        os.remove(ad.LOG_PATH)
    fixture = {
        "png_hex": PNG.hex(),
        "f10_help": [[k_[0], k_[1], k_[2], k_[3], v] for k_, v in b._F10_HELP.items()],
        "keys": [run_keys(sc) for sc in KEY_SCENARIOS],
        "status_all": status_all_cases(),
        "ai_status": ai_status_cases(),
        "arrivals": arrival_cases(),
        "cues": cue_cases(),
        "validations": validation_cases(),
        "workers": worker_cases(),
        "misc": misc(),
        "log_lines": log_cases(),
    }
    write_fixture("e7_ai", fixture)


if __name__ == "__main__":
    main()
