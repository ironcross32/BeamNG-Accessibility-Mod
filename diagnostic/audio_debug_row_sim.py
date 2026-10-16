"""Replay the F9-then-J audio debug browser's parsing, wording and stepping.

    python diagnostic/audio_debug_row_sim.py

audio_debug.py is pure, so it is imported directly. The Lua halves cannot run here, so the
checks on them are on their source:

  * The FMOD frequency table in audioDebugAccessible.lua must equal the stock Engine Audio
    Debug window's. The browser steps frequencies on FMOD's 0-1000 scale and speaks the Hz the
    GE side converts to; a drifted table speaks a frequency the engine is not playing.

  * Every control the GE side lists must be a value the vehicle side reports and can set. A
    key on one side only is a row that never appears, or a SET the vehicle answers with
    "Unknown setting".

And on the Python side:

  * Named-field parse: an unknown field is ignored rather than shifting the ones after it.
  * Stepping is not snapped to the grid, so a vehicle's own value stays reachable by stepping
    back, and the range ends report "cannot move" rather than resending the same value.
"""

import os
import re
import sys

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _ROOT)

import audio_debug as ad  # noqa: E402

_GE = os.path.join(_ROOT, "bng_mod", "lua", "ge", "extensions", "audioDebugAccessible.lua")
_VEH = os.path.join(_ROOT, "bng_mod", "lua", "vehicle", "extensions", "beamtelAudioDebug.lua")
_STOCK = os.path.join(
    os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"),
    "Steam", "steamapps", "common", "BeamNG.drive",
    "lua", "ge", "extensions", "editor", "engineAudioDebug.lua",
)

failures = 0


def check(name, ok, detail=""):
    global failures
    print(("PASS " if ok else "FAIL ") + name + ("" if ok else f" -- {detail}"))
    if not ok:
        failures += 1


def read(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read()


# ---- parse -----------------------------------------------------------------------------------
row = ad.parse_fields(
    "key=mainGain;kind=number;futureField=x;label=Main gain;unit=dB;value=-3.5;min=-30;max=10;step=0.5"
)
check("unknown field ignored, later fields intact",
      row.get("label") == "Main gain" and row.get("value") == "-3.5", row)
check("bare leading count kept", ad.parse_fields("20;soundCount=2").get("count") == "20")

# ---- wording ---------------------------------------------------------------------------------
check("negative decibels spelled out", ad.row_line(row) == "Main gain, minus 3.5 decibels",
      ad.row_line(row))
zero = dict(row, value="-0.0001")
check("near-zero is plain 0", ad.row_line(zero) == "Main gain, 0 decibels", ad.row_line(zero))

for hz, want in (("120", "120 hertz"), ("1044", "1.04 kilohertz"), ("2500", "2.5 kilohertz"),
                 ("22000", "22 kilohertz"), ("382.35", "382 hertz")):
    freq = {"kind": "number", "label": "Low shelf frequency", "unit": "Hz", "value": "500", "hz": hz}
    got = ad.row_line(freq)
    check(f"{hz} Hz spoken as {want}", got == f"Low shelf frequency, {want}", got)

sel = {"kind": "select", "index": "1", "count": "2", "ref": "exhaust", "device": "mainEngine",
       "blend": "V8_cross_exhaust", "modified": "1"}
check("selector line", ad.row_line(sel) == "Sound 2 of 2: exhaust, mainEngine, V8_cross_exhaust, changed",
      ad.row_line(sel))
check("restore with nothing changed says so",
      ad.row_line({"kind": "action", "key": "restore", "label": "Restore vehicle defaults",
                   "editable": "0"}) == "Restore vehicle defaults, nothing changed")
check("folder line", ad.sfx_line({"kind": "dir", "name": "UI", "count": "71"}) == "UI, folder, 71 items")
check("one-item folder", ad.sfx_line({"kind": "dir", "name": "DLC", "count": "1"}) == "DLC, folder, 1 item")

# ---- stepping --------------------------------------------------------------------------------
freq = {"kind": "number", "value": "382.3529", "min": "0", "max": "1000", "step": "10"}
seen = []
r = dict(freq)
for _ in range(5):
    v = ad.stepped_value(r, 1)
    seen.append(v)
    r["value"] = str(v)
check("five presses, five distinct increasing values",
      len(set(seen)) == 5 and seen == sorted(seen), seen)
back = ad.stepped_value(dict(freq, value=str(ad.stepped_value(freq, 1))), -1)
check("step up then down returns the vehicle's own value", back == 382.3529, back)
check("shift step is ten steps", ad.stepped_value(freq, 10) == 482.3529)
check("at the top, cannot move", ad.stepped_value(dict(freq, value="1000"), 1) is None)
check("clamped at the bottom", ad.stepped_value(dict(freq, value="5"), -1) == 0)
ratio = {"kind": "number", "value": "0.65", "min": "0", "max": "1", "step": "0.01"}
check("ratio step free of float noise", ad.stepped_value(ratio, 1) == 0.66, ad.stepped_value(ratio, 1))
check("wire number", ad.wire_number(0.66) == "0.66" and ad.wire_number(-3.5) == "-3.5")
check("select wraps forward", ad.select_target({"index": "1", "count": "2"}, 1) == 0)
check("select wraps back", ad.select_target({"index": "0", "count": "3"}, -10) == 2)
check("single sound cannot select", ad.select_target({"index": "0", "count": "1"}, 1) is None)

# ---- Lua source --------------------------------------------------------------------------------
ge = read(_GE)
veh = read(_VEH)

ge_keys = set(re.findall(r'\{key = "(\w+)",\s+label', ge))
veh_keys = set(re.findall(r"\bout\.(\w+) =", veh))
check("GE controls and vehicle values name the same keys",
      ge_keys == veh_keys and len(ge_keys) == 16,
      f"GE only {sorted(ge_keys - veh_keys)}, vehicle only {sorted(veh_keys - ge_keys)}")

# PARAM maps the plain keys; mainGain, muffling and cylinders are branches of their own in set().
param_block = veh.split("local PARAM = {")[1].split("}")[0]
veh_settable = set(re.findall(r'(\w+) = "\w+"', param_block))
veh_settable |= {k for k in ("mainGain", "muffling", "cylinders") if f'key == "{k}"' in veh}
check("every GE control is settable on the vehicle side", ge_keys <= veh_settable,
      sorted(ge_keys - veh_settable))

ours = re.search(r"local FMOD_HZ = \{([^}]*)\}", ge)
ours = [float(x) for x in ours.group(1).split(",")] if ours else None
if os.path.exists(_STOCK):
    stock = re.search(r"local fmodtable = \{([^}]*)\}", read(_STOCK))
    stock = [float(x) for x in stock.group(1).split(",")] if stock else None
    check("FMOD table matches the stock Engine Audio Debug window", ours == stock and ours,
          f"ours {ours}, stock {stock}")
else:
    print("SKIP FMOD table vs stock: game install not found at " + _STOCK)


# The GE conversion, mirrored, against values measured in game (FMOD value -> Hz the GE side sent).
def fmod_to_hz(f):
    f = max(0, min(f, 1000))
    seg = min(int(f // 100), 9)
    return ours[seg] + (f - seg * 100) / 100 * (ours[seg + 1] - ours[seg])


for f, hz in ((250, 120), (729.6296, 3500), (514.0625, 750), (560, 1044), (1000, 22000), (0, 20)):
    check(f"FMOD {f} is {hz} Hz", abs(fmod_to_hz(f) - hz) < 0.5, fmod_to_hz(f))

print()
print("ALL PASS" if failures == 0 else f"{failures} FAILED")
sys.exit(1 if failures else 0)
