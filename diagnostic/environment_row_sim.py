"""Replay the F9-then-N environment browser's parsing, wording and stepping.

    python diagnostic/environment_row_sim.py

The functions under test are LIFTED OUT OF beamtel.py by AST rather than copied here, so
this cannot drift into testing a stale duplicate -- beamtel imports wx, sounddevice and the
speech backend at module scope and cannot be imported, but its function bodies can be.

Three things earn a check:

  * The unit round trip. The wire is always Celsius and the user hears Fahrenheit, so an
    adjust has to step in the unit that is SPOKEN. Stepping in Celsius instead moves an
    imperial readout by 1.8 F a press, which skips numbers and makes a requested value
    unreachable -- a control that responds to every press and cannot be aimed.

  * The flat-versus-cycle wording. A level whose temperature curve is not flat has no single
    temperature: the figure shown is only true for the current time of day, and setting one
    replaces the level author's whole day cycle with a constant. The row has to say so, or
    the loss is silent.

  * The named-field parse. The mod's rows are `k=v;k=v`, chosen over positional CSV because
    bng_mod/ is a live junction into the game install and the two halves genuinely do go out
    of step. The property that buys is that an unknown field is IGNORED rather than shifting
    every field after it, so that is asserted directly.
"""

import ast
import os
import sys

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_SRC = os.path.join(_ROOT, "beamtel.py")
_LUA = os.path.join(_ROOT, "bng_mod", "lua", "ge", "extensions", "environmentAccessible.lua")

_WANTED = (
    "_parse_env_fields",
    "_env_float",
    "_env_temp_from_display",
    "_env_row_line",
    "_env_on_adjust",
    "fmt_temp_c_or_f",
)

with open(_SRC, encoding="utf-8") as fh:
    _source = fh.read()

_tree = ast.parse(_source)
_ns = {}
_found = set()
for node in _tree.body:
    if isinstance(node, ast.FunctionDef) and node.name in _WANTED:
        exec(compile(ast.Module([node], []), _SRC, "exec"), _ns)
        _found.add(node.name)

missing = set(_WANTED) - _found
if missing:
    print("FAIL: could not lift from beamtel.py: " + ", ".join(sorted(missing)))
    sys.exit(1)

# ---- stubs for what the lifted bodies reach for ----------------------------------------
_sent = []
_spoken = []

_ns["UNITS_MODE"] = "imperial"
_ns["_env_rows"] = []
_ns["_send_env_cmd"] = lambda cmd: _sent.append(cmd)
_ns["_env_arm_change"] = lambda: None
_ns["say"] = lambda text, **kw: _spoken.append(text)
_ns["logger"] = type("L", (), {"error": staticmethod(lambda *a, **k: None)})()

parse = _ns["_parse_env_fields"]
row_line = _ns["_env_row_line"]
on_adjust = _ns["_env_on_adjust"]

_failures = []


def check(label, ok, detail=""):
    print("   {}: {}{}".format(label, "OK" if ok else "FAIL", " - " + detail if detail else ""))
    if not ok:
        _failures.append(label)


def set_units(mode):
    _ns["UNITS_MODE"] = mode


def temp_row(**over):
    row = {
        "key": "temperature",
        "kind": "numberC",
        "value": "15.00",
        "min": "-60.0",
        "max": "60.0",
        "step": "1",
        "label": "Temperature",
        "curveLo": "15.00",
        "curveHi": "15.00",
        "editable": "1",
    }
    row.update(over)
    return row


# ==========================================================================================
print("1. named-field rows parse, and an unknown field does not shift the others")
# ==========================================================================================
LIVE = ("key=temperature;kind=numberC;value=15.00;min=-60.0;max=60.0;step=1;"
        "label=Temperature;curveLo=15.00;curveHi=15.00;editable=1")
got = parse(LIVE)
check("every field lands", got.get("key") == "temperature" and got.get("value") == "15.00"
      and got.get("editable") == "1", repr(got.get("key")))
check("a label with spaces survives",
      parse("key=restore;label=Restore level default").get("label") == "Restore level default")

# The whole reason this is not positional CSV. A newer mod half sending a field this build
# has never heard of must change nothing about the fields it does know.
future = parse(LIVE + ";humidityPct=41;windChillC=9.5")
check("an unknown field is ignored, not shifted in",
      future.get("value") == "15.00" and future.get("editable") == "1"
      and future.get("humidityPct") == "41")

check("ENV_BEGIN's leading count is kept",
      parse("2;level=smallgrid;canChange=1").get("count") == "2")
check("...alongside its named fields",
      parse("2;level=smallgrid;canChange=1").get("level") == "smallgrid")
print()

# ==========================================================================================
print("2. the readout speaks the user's unit")
# ==========================================================================================
set_units("imperial")
check("15 C reads as 59 F", row_line(temp_row()) == "Temperature, 59 Fahrenheit",
      row_line(temp_row()))
set_units("metric")
check("...and as 15 Celsius in metric", row_line(temp_row()) == "Temperature, 15 Celsius",
      row_line(temp_row()))
set_units("imperial")
print()

# ==========================================================================================
print("3. a level whose curve is NOT flat says so")
# ==========================================================================================
# Setting a temperature on such a level replaces the author's day cycle with one number.
# Announcing only the current figure would make that loss invisible.
cycle = temp_row(value="15.00", curveLo="5.00", curveHi="25.00")
line = row_line(cycle)
check("the daily range is announced", "varies" in line and "41" in line and "77" in line, line)
check("the live figure is still first", line.startswith("Temperature, 59 Fahrenheit"), line)

flat = row_line(temp_row(curveLo="15.00", curveHi="15.40"))
check("a curve flat to within a degree is not called a cycle", "varies" not in flat, flat)

locked = row_line(temp_row(editable="0"))
check("a scenario-locked row says locked", locked.endswith(", locked"), locked)

missing_val = row_line(temp_row(value=""))
check("a missing value is 'not available', never 0",
      missing_val == "Temperature, not available", missing_val)
print()

# ==========================================================================================
print("4. the restore row")
# ==========================================================================================
act = {"key": "restore", "kind": "action", "label": "Restore level default",
       "value": "15.00", "curveLo": "15.00", "curveHi": "15.00", "editable": "1"}
check("a flat default quotes one figure",
      row_line(act) == "Restore level default, 59 Fahrenheit", row_line(act))
act_cycle = dict(act, curveLo="5.00", curveHi="25.00")
check("a curved default quotes the range",
      row_line(act_cycle) == "Restore level default, 41 to 77 Fahrenheit", row_line(act_cycle))
act_none = dict(act, value="", curveLo="", curveHi="")
check("nothing captured reads as not available",
      row_line(act_none) == "Restore level default, not available", row_line(act_none))
print()

# ==========================================================================================
print("5. adjusting steps in the SPOKEN unit, one number at a time")
# ==========================================================================================
# The bug this guards: stepping the Celsius value by 1 moves an imperial readout by 1.8 F,
# so pressing right five times from 59 F gives 59, 61, 62, 64, 66, 68 -- numbers skipped,
# and no key press can ever land on 60.
set_units("imperial")


def press(row, delta, times=1):
    """Feed a row through _env_on_adjust `times`, applying each result back into the row."""
    seen = []
    cur = dict(row)
    for _ in range(times):
        _sent.clear()
        _ns["_env_rows"] = [cur]
        on_adjust(0, "", 0, delta)
        if not _sent:
            break
        celsius = float(_sent[-1].split("=")[1])
        cur = dict(cur, value="{:.2f}".format(celsius))
        seen.append(_ns["fmt_temp_c_or_f"](celsius)[0])
    return seen

check("five presses right give five consecutive Fahrenheit numbers",
      press(temp_row(), +1, 5) == [60, 61, 62, 63, 64], str(press(temp_row(), +1, 5)))
check("and left walks straight back down",
      press(temp_row(), -1, 3) == [58, 57, 56], str(press(temp_row(), -1, 3)))

set_units("metric")
check("metric steps by whole Celsius", press(temp_row(), +1, 3) == [16, 17, 18],
      str(press(temp_row(), +1, 3)))
set_units("imperial")

check("shift's coarse step is ten of the spoken unit",
      press(temp_row(), +10, 1) == [69], str(press(temp_row(), +10, 1)))
print()

# ==========================================================================================
print("6. the adjust respects the band and the lock")
# ==========================================================================================
_sent.clear()
_spoken.clear()
_ns["_env_rows"] = [temp_row(value="60.00")]
on_adjust(0, "", 0, +10)
check("a press at the ceiling clamps rather than running away",
      _sent and abs(float(_sent[-1].split("=")[1]) - 60.0) < 1e-6, str(_sent))

_sent.clear()
_ns["_env_rows"] = [temp_row(value="-60.00")]
on_adjust(0, "", 0, -10)
check("...and at the floor", _sent and abs(float(_sent[-1].split("=")[1]) + 60.0) < 1e-6,
      str(_sent))

_sent.clear()
_spoken.clear()
_ns["_env_rows"] = [temp_row(editable="0")]
on_adjust(0, "", 0, +1)
check("a locked row sends nothing and says so",
      not _sent and _spoken == ["Locked"], str(_sent) + str(_spoken))

_sent.clear()
_ns["_env_rows"] = [dict(temp_row(), kind="action")]
on_adjust(0, "", 0, +1)
check("an action row ignores left and right", not _sent, str(_sent))

_sent.clear()
_ns["_env_rows"] = [temp_row()]
on_adjust(0, "", 5, +1)
check("an out-of-range row index is dropped, not indexed", not _sent, str(_sent))
print()

# ==========================================================================================
print("7. the two halves agree on the ports and the command names")
# ==========================================================================================
with open(_LUA, encoding="utf-8") as fh:
    lua = fh.read()

for name, want in (("PYTHON_PORT_DATA", "ENV_LISTEN_PORT"), ("CMD_LISTEN_PORT", "ENV_CMD_PORT")):
    lua_port = None
    for line in lua.splitlines():
        if line.strip().startswith("local " + name):
            lua_port = line.split("=")[1].split("--")[0].strip()
            break
    py_port = None
    for line in _source.splitlines():
        if line.startswith(want + " ="):
            py_port = line.split("=")[1].split("#")[0].strip()
            break
    check("{} matches {}".format(name, want), lua_port is not None and lua_port == py_port,
          "{} vs {}".format(lua_port, py_port))

# A command the mod does not answer is indistinguishable from a dead socket.
for cmd in ("REQUEST", "SET:", "RESTORE"):
    check("the mod handles " + cmd, cmd.rstrip(":") in lua)

# core_environment.onUpdate returns early on a curve with fewer than two points, so a
# one-point curve is a set that reports success and freezes the temperature where it was.
check("a flat set writes TWO curve points, not one",
      "{{0, celsius}, {1, celsius}}" in lua)

# setEditorDirty() reads like the cheap refresh and is a no-op outside the editor; onInit()
# is the one that actually re-reads the cached curve.
check("the refresh goes through core_environment.onInit",
      "core_environment.onInit()" in lua and "setEditorDirty" not in lua.split("--")[0])
print()

if _failures:
    print("FAILED: " + ", ".join(_failures))
    sys.exit(1)
print("All scenarios passed.")
