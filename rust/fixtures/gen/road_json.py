"""Golden cases for beam_road::pyjson: json.loads acceptance and json.dumps bytes.

The recorders in beam-road write NDJSON and summary files the Python build reopens,
with the four json.dumps option sets the Python modules use. Each accepted document
is re-encoded with all four so the Rust encoder is checked byte for byte.
"""

from __future__ import annotations

import json
import random

from _common import write_fixture

OPTION_SETS = {
    # road_diagnostics rows
    "compact_ascii_nonan": dict(separators=(",", ":"), allow_nan=False),
    # challenge_results rows
    "compact_utf8": dict(separators=(",", ":"), ensure_ascii=False),
    # assistant_diagnostics rows
    "default_utf8": dict(ensure_ascii=False),
    # summaries
    "indent2": dict(indent=2),
    "indent2_nonan": dict(indent=2, allow_nan=False),
}


def encode_all(value):
    out = {}
    for name, opts in OPTION_SETS.items():
        try:
            out[name] = {"ok": json.dumps(value, **opts)}
        except ValueError as exc:
            out[name] = {"err": type(exc).__name__, "msg": str(exc)}
    return out


def main():
    rnd = random.Random(1258)
    docs = [
        "null", "true", "false", "0", "-0", "1", "-1", "12345678901234567890", "-9223372036854775808",
        "9223372036854775807", "9223372036854775808", "1.0", "-0.0", "1e5", "1E+5", "1e-7", "1.5e300",
        "1e400", "-1e400", "0.1", "123.456", "1e16", "1e15", "12345678.9", "NaN", "Infinity", "-Infinity",
        '"x"', '""', '"\\u00e9"', '"\\ud83d\\ude00"', '"é"', '"\\n\\t\\r\\b\\f\\"\\\\\\/"', '"\\u007f"',
        '"\\u0001"', '"日本語"', '"emoji 😀"', '"\x7f"',
        "[]", "{}", "[1, 2.0, \"3\", null, true]", '{"b": 1, "a": 2}', '{"a": 1, "a": 2, "b": 3}',
        '{"nested": {"list": [1, [2, {"x": []}], {}]}, "z": {"y": {}}}',
        ' \t\n {"padded": 1} \r\n', '[1,2,3]', '{"k":"v"}',
        # rejected
        "", " ", "nan", "inf", "01", "1.", ".5", "+1", "1e", "[1,]", '{"a":1,}', "{'a': 1}", '{"a" 1}',
        "[1 2]", '"unterminated', '"bad \\x escape"', '"ctl \x01 char"', "[", "]", "tru", "nul",
        '{"a":1} x', "1 2", '"\\u12"', "-", "--1", "1.e5", "Infinityx", "-NaN",
    ]
    for _ in range(60):
        docs.append(json.dumps(random_value(rnd, 0), ensure_ascii=rnd.random() < 0.5))

    cases = []
    for text in docs:
        try:
            value = json.loads(text)
        except json.JSONDecodeError:
            cases.append({"text": text, "decodes": False})
            continue
        cases.append({"text": text, "decodes": True, "encoded": encode_all(value)})

    write_fixture("road_json", {"cases": cases})


def random_value(rnd, depth):
    kind = rnd.randrange(9 if depth < 3 else 6)
    if kind == 0:
        return None
    if kind == 1:
        return rnd.random() < 0.5
    if kind == 2:
        return rnd.randint(-10**6, 10**6)
    if kind == 3:
        return rnd.choice([0.0, -0.0, 0.1, 1e-5, 1e16, 2.5, 1e22, 123.456789, -3.75, float("nan"),
                           float("inf"), rnd.uniform(-1e4, 1e4), rnd.uniform(-1, 1)])
    if kind in (4, 5):
        return "".join(rnd.choice("ab \"\\\n\té日😀\x01\x7f/") for _ in range(rnd.randrange(6)))
    if kind in (6, 7):
        return [random_value(rnd, depth + 1) for _ in range(rnd.randrange(4))]
    return {random_value_key(rnd): random_value(rnd, depth + 1) for _ in range(rnd.randrange(4))}


def random_value_key(rnd):
    return rnd.choice(["a", "b", "key", "é", "x y", "😀", "q\"uote"])


if __name__ == "__main__":
    main()
