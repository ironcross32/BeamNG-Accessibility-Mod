"""Golden cases for beam_base::pyfmt: Python's round, repr, %g, %f, float() and %."""

from __future__ import annotations

import random
import struct

from _common import fbits, write_fixture


def bits(x):
    return struct.unpack("<Q", struct.pack("<d", float(x)))[0]


def values():
    rnd = random.Random(20261113)
    out = [
        0.0, -0.0, 0.5, -0.5, 1.5, -1.5, 2.5, -2.5, 3.5, 2.675, -2.675, 1.005, 0.125,
        0.375, 0.045, 1.115, 2.345, 1e16, -1e16, 1e15, 1e17, 1.5e16, 9999999999999998.0,
        1e-7, 1e-5, 1e-4, 0.0001, 0.00011, 1e-6, 123456789.123, 0.1, 0.2, 0.3, 1 / 3,
        2 / 3, 5e-324, 2.2250738585072014e-308, 1.7976931348623157e308, 1e308, 1e22,
        1e23, 4.35, 4.45, 0.05, 0.15, 0.25, 0.35, 999.5, 9.995, 99.95, 0.95, 0.995,
        1234.5, 1250.0, 1350.0, 500.0, 1500.0, 2500.0, 501.0, 499.0, 12345678901234567.0,
        2.0 ** 53, 2.0 ** 53 + 2, 2.0 ** 63, -(2.0 ** 63), 2.0 ** 64, 1e19, 3.0e-10,
        float("nan"), float("inf"), float("-inf"), 359.99999999999994, 360.0, -360.0,
        180.0, -180.0, 0.30000000000000004, 1e-320, 6.02214076e23, 1.0, 10.0, 100.0,
    ]
    for k in range(-40, 41):
        out.append(k / 8)
        out.append(k + 0.5)
    for _ in range(400):
        out.append(rnd.uniform(-1, 1) * 10 ** rnd.randint(-30, 30))
    for _ in range(400):
        out.append(round(rnd.uniform(-2000, 2000), rnd.randint(0, 4)))
    for _ in range(200):
        # x.xx5-style decimal ties, which are not ties in binary
        d = rnd.randint(1, 4)
        out.append(float(f"{rnd.randint(-99999, 99999)}.{'0' * (d - 1)}5"))
    for _ in range(150):
        out.append(rnd.uniform(-400, 400))
    for _ in range(100):
        out.append(struct.unpack("<d", struct.pack("<Q", rnd.getrandbits(64)))[0])
    return out


ROUND_N = (-3, -2, -1, 0, 1, 2, 3, 4, 6)
ROUND_N_EXTRA = (-400, -309, -308, -300, -20, 10, 15, 17, 20, 30, 300, 323, 324, 400)
G_P = (0, 1, 2, 3, 6, 10, 17)
F_P = (0, 1, 2, 3)
MODS = (360.0, -7.0, 1.0, 0.1)


def try_float(fn):
    try:
        return bits(fn())
    except (OverflowError, ValueError) as exc:
        return type(exc).__name__


def try_int(fn):
    try:
        return str(fn())
    except (OverflowError, ValueError) as exc:
        return type(exc).__name__


def main():
    cases = []
    for i, x in enumerate(values()):
        case = {
            "x": fbits(x),
            "round": {str(n): try_float(lambda n=n: round(x, n)) for n in ROUND_N},
            "round0": try_int(lambda: round(x)),
            "repr": repr(x),
            "str": str(x),
            "fstr": f"{x}",
            "g": {str(p): format(x, f".{p}g") for p in G_P},
            "g_default": format(x, "g"),
            "pct_g": "%g" % x,
            "f": {str(p): format(x, f".{p}f") for p in F_P},
            "mod": {repr(m): try_float(lambda m=m: x % m) for m in MODS},
            "int": try_int(lambda: int(x)),
        }
        if i % 4 == 0:
            case["round_extra"] = {
                str(n): try_float(lambda n=n: round(x, n)) for n in ROUND_N_EXTRA
            }
        cases.append(case)

    strings = [
        "1", "1.5", " 1.5 ", "\t-2\n", "+3", "-0", "-0.0", "1e5", "1E5", "1e+05", "1e-05",
        ".5", "5.", "-.5", "+.5e1", ".", "", " ", "e5", "1e", "1e+", "1_000", "1__000",
        "_1", "1_", "1_000.000_1", "1e1_0", "1._5", "1_.5", "inf", "-inf", "+inf",
        "Infinity", "-INFINITY", "infin", "nan", "-nan", "+NaN", "nan(1)", "0x10", "1,5",
        "1.5.", "--1", "+-1", "1e500", "-1e500", "1e-500", "abc", "12abc", "1 2",
        "Â 1.5â€ƒ", "\x1c2\x1f", "â€‹1", "1.5\x00", "9007199254740993",
        "0.1000000000000000055511151231257827", "2.675", "00012", "1e0001", "  +7.25e-3  ",
        "ROUTE", "4821.5", "-340.25", "61.0", "1200.5", "-5", "Ù¡Ù¢",
    ]
    floats = []
    for s in strings:
        try:
            floats.append({"s": s, "ok": fbits(float(s))})
        except ValueError:
            floats.append({"s": s, "err": "ValueError"})

    spaces = [c for c in range(0x110000) if chr(c).isspace()]

    write_fixture(
        "pyfmt",
        {
            "cases": cases,
            "float_parse": floats,
            "isspace": spaces,
        },
    )


if __name__ == "__main__":
    main()
