"""Golden cases for beam_reports::pyops (Python semantics the report modules share)."""

from __future__ import annotations

import html
import json
import math
import random

from _common import fbits, write_fixture
from _pyvalue import enc, outcome_enc


def main():
    rnd = random.Random(1717)
    nan, inf = float("nan"), float("inf")

    json_texts = [
        "", " ", "null", "true", "false", "NaN", "Infinity", "-Infinity", "-NaN", "nan",
        "0", "-0", "00", "01", "1", "-1", "123456789012345678901234567890", "1.5", "1.", ".5",
        "1e5", "1E+5", "1e-5", "1e", "1e+", "-", "-a", "1.5e400", "-1e400", "1e-400", "2.50",
        '"a"', '"\\u00e9"', '"\\ud83d\\ude00"', '"\\n\\t\\"\\\\\\/\\b\\f\\r"', '"\\x"', '"\\u12"',
        '"tab\there"', '"unterminated', '"é😀"', "[]", "[ ]", "[1,2]", "[1,]", "[,1]", "[1 2]",
        "{}", '{"a":1}', '{"a":1,}', '{a:1}', '{"a" 1}', '{"a":1 "b":2}', '{"b":1,"a":2,"b":3}',
        ' \t\n\r{"x": [1, {"y": null}]} \n', '{"x":1} extra', '{"x":1}{}', "﻿{}", "[1]x",
        " {}", "{} ", "tru", "nul", "True", '{"k": NaN, "j": -Infinity}', "[01]",
        "1_000", "+1", '"\u0001"', "[" * 5 + "]" * 5, '{"a": {"b": {"c": [true, false, null]}}}',
        "[1.0, 1, -0.0, 0.0, 1e2, 1E2, 3.14159]", '"\\u0041\\u00df"', "[\"a\",\"b\"]",
    ]
    loads = [{"text": t, **outcome_enc(json.loads, t)} for t in json_texts]

    values = [None, True, False, 0, 1, -1, 7, 10**30, -(10**30), 10**400, 0.0, -0.0, 1.5, -2.5,
              nan, inf, -inf, 1e300, 123.456, 2.5, 3.5, "", "0", "1.5", "abc", " 12 ", "1_000",
              "١٢", "nan", "-inf", "1e3", "١.٥", "0x10", "+5", "-0", " ", " 12 ",
              "1\x1c", "it's", 'say "hi"', "both ' and \"", "back\\slash", "tab\tnl\ncr\r",
              "\x00\x7f\x80\xa0é​ ﻿\U0001F600\U000E0001", [], [1, "a", None],
              {}, {"k": [1.0, True], "z": {"y": "q"}}, ["it's"], {"a'b": 'c"d'}]
    conversions = []
    for v in values:
        conversions.append({
            "value": enc(v),
            "float": outcome_enc(float, v),
            "int": outcome_enc(int, v),
            "str": str(v),
            "repr": repr(v),
            "truthy": bool(v),
        })

    float_to_int = []
    for x in [0.0, -0.0, 0.5, -0.5, 1.5, 2.5, -2.5, 1e20, -1e20, 1e300, 123456789.987, nan, inf,
              -inf, 4503599627370497.0, 0.49999999999999994] + [rnd.uniform(-1e6, 1e6) for _ in range(40)]:
        float_to_int.append({
            "x": fbits(x),
            "int": outcome_enc(int, x),
            "round_int": outcome_enc(lambda y: int(round(y)), x),
        })

    groups = [{"int": str(n), "out": f"{n:,}"} for n in
              [0, 1, -1, 12, 123, 1234, -1234, 12345, 123456, 1234567, -1234567, 10**30, -(10**21)]]

    html_cases = [
        "plain", "&amp;", "&amp", "&ampx", "&amp;x", "&lt;b&gt;", "&#62;", "&#x3e;", "&#X3E;",
        "&#0;", "&#13;", "&#128;", "&#150;", "&#xD800;", "&#x110000;", "&#1114111;", "&#1;",
        "&#xFFFE;", "&#99999999999999999999;", "&#;", "&#x;", "&#xg;", "&#65", "&#x41", "&",
        "& amp;", "&;", "&notit;", "&notin;", "&noti", "&not", "&Aacute;", "&Aacute", "&AElig",
        "&unknown;", "&ab;", "&a", "a&b&c", "&&amp;", "&amp;amp;", "&" + "a" * 40 + ";",
        "&nbsp;x&nbsp", "&quot;&apos;&#39;", "&lang;&rang;", "&NotNestedGreaterGreater;",
        "&fjlig;", "Foot &amp; start<br/>Asphalt", "&\tamp;", "&am\rp;", "&#x1F600;", "&#55357;",
        "&#x80;", "&#x9F;", "&#127;", "&#11;", "&#xb;",
    ]
    unescape = [{"in": s, "out": html.unescape(s)} for s in html_cases]

    hyp_pairs = [(0.0, 0.0), (3.0, 4.0), (-3.0, 4.0), (1e-320, 3e-320), (5e-324, 5e-324),
                 (1e308, 1e308), (inf, nan), (nan, 1.0), (-inf, 2.0), (0.0, -0.0), (1.0, 0.0),
                 (0.70710678, 0.70710678), (1e-6, 0.0), (1e-7, 1e-7), (2.2250738585072014e-308, 1e-310)]
    hyp_pairs += [(rnd.uniform(-100, 100), rnd.uniform(-100, 100)) for _ in range(200)]
    hyp_pairs += [(rnd.uniform(-1, 1) * 10 ** rnd.randint(-300, 300),
                   rnd.uniform(-1, 1) * 10 ** rnd.randint(-300, 300)) for _ in range(100)]
    hypot = [{"x": fbits(x), "y": fbits(y), "out": fbits(math.hypot(x, y))} for x, y in hyp_pairs]

    mm = []
    for a, b in [(0.0, -0.0), (-0.0, 0.0), (1.0, nan), (nan, 1.0), (1.0, 2.0), (2.0, 1.0)]:
        mm.append({"a": fbits(a), "b": fbits(b), "min": fbits(min(a, b)), "max": fbits(max(a, b))})

    sorts = []
    for _ in range(30):
        n = rnd.randint(0, 20)
        keys = [rnd.choice([0, 1, 2, 3, 2.5, -1]) for _ in range(n)]
        order = sorted(range(n), key=lambda i: keys[i])
        sorts.append({"keys": [fbits(k) for k in keys], "order": order})

    write_fixture("pyops", {
        "json_loads": loads,
        "conversions": conversions,
        "float_to_int": float_to_int,
        "group_thousands": groups,
        "html_unescape": unescape,
        "hypot": hypot,
        "minmax": mm,
        "sort": sorts,
        "isprintable_sample": [{"cp": cp, "printable": chr(cp).isprintable()}
                               for cp in [0, 0x1f, 0x20, 0x7e, 0x7f, 0xa0, 0xad, 0x378, 0x2028,
                                          0x3000, 0xd800, 0xe000, 0xfeff, 0x1f600, 0xe0001, 0x10ffff]],
    })


if __name__ == "__main__":
    main()
