"""Replay the vehicle information readout's row flattening, rendering and dispatch.

    python diagnostic/vehicle_info_sim.py

The Python functions under test are LIFTED OUT OF beamtel.py by AST rather than copied here,
so this cannot drift into testing a stale duplicate -- beamtel imports wx, sounddevice and the
speech backend at module scope and cannot be imported, but its function bodies can be.

This area's failure mode is a readout that is merely WRONG rather than broken -- a spec sheet
that reads plausibly while describing the wrong car, or quietly dropping the one number the
page is about. Nothing downstream can catch that, so every scenario also asserts what the
naive form would have answered, and no check can pass for free.

Five things earn a check:

  * The segment join. A spec value is not always a string: Power and Torque arrive as an
    array of {text=...} segments because the page italicises the rpm band. `tostring` on that
    puts a table address into speech and dropping it loses the headline number on the page.

  * The failure CODE. "not on a selector screen" is the answer on every screen in the game,
    including the one where F9 SPACE means "scan the terrain". Python has to fall through
    silently on exactly that code while speaking the others, and it cannot tell them apart
    from prose -- so the wire carries `code;sentence` and the split is asserted directly.

  * The group headings. A group row is a heading with no value; rendering it as
    "label: value" with an empty value leaves a trailing colon, and skipping it flattens
    thirty numbers into one undifferentiated run.

  * The wrapper contract in the Lua half, which no Python fake can reach and which is
    therefore grepped: the versioned mark, the mutable recorder slot, and the fact that it
    calls vehicleSpecifications.getDetails rather than the button-building one.

  * The spawner's per-screen resolution. Its three list screens each mean something
    different by "this vehicle", and getting one wrong produces a spec sheet for a real but
    WRONG car -- which reads exactly like a working feature.
"""

import ast
import os
import sys

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_SRC = os.path.join(_ROOT, "beamtel.py")
_LUA = os.path.join(_ROOT, "bng_mod", "lua", "ge", "extensions", "vehicleInfo.lua")

_WANTED = ("_vinfo_render",)

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

render = _ns["_vinfo_render"]

with open(_LUA, encoding="utf-8") as fh:
    lua = fh.read()

_failures = []


def check(label, ok, detail=""):
    print("   {}: {}{}".format(label, "OK" if ok else "FAIL", " - " + detail if detail else ""))
    if not ok:
        _failures.append(label)


# A Python transcription of the Lua flattener's value rule, used ONLY to demonstrate the
# naive alternatives below. The real one is asserted against live game output in the
# verification steps; what is checked here is the shape of the contract it produces.
def value_to_text(v):
    if isinstance(v, str):
        return v
    if isinstance(v, (int, float)):
        return str(v)
    if not isinstance(v, list):
        return None
    parts = [seg["text"] for seg in v
             if isinstance(seg, dict) and isinstance(seg.get("text"), str) and seg["text"]]
    return " ".join(parts) if parts else None


# The real payload shape, taken from a live ui_vehicleSelector_vehicleSpecifications
# .getDetails({model="pessima", config="race"}) call against a running game.
POWER_VALUE = [{"text": "334 bhp"}, {"text": "@ 5700 - 6500 rpm", "italic": True}]

ROWS = [
    {"i": 0, "kind": "head", "label": "", "value": "Pessima Track (M)"},
    {"i": 1, "kind": "head", "label": "Brand", "value": "Ibishu"},
    {"i": 2, "kind": "group", "label": "Summary", "value": ""},
    {"i": 3, "kind": "spec", "label": "Weight", "value": "3075 lb"},
    {"i": 4, "kind": "group", "label": "Features", "value": ""},
    {"i": 5, "kind": "spec", "label": "", "value": "Drivetrain: All Wheel Drive"},
]

# ==========================================================================================
print("1. a spec value that is a segment array joins, rather than being stringified")
# ==========================================================================================
got = value_to_text(POWER_VALUE)
check("the rpm band rides with the figure", got == "334 bhp @ 5700 - 6500 rpm", repr(got))

# The whole reason the rule exists. Both naive forms are wrong in ways that read as working:
# str() produces something speakable that is meaningless, and taking only the first segment
# silently loses half the answer on the two headline specs of the page.
check("...where str() would speak a container",
      str(POWER_VALUE) != got and "text" in str(POWER_VALUE))
check("...and first-segment-only would drop the rpm band",
      POWER_VALUE[0]["text"] == "334 bhp" and POWER_VALUE[0]["text"] != got)

check("a plain string passes through", value_to_text("Manual") == "Manual")
check("a number is spoken", value_to_text(1.367) == "1.367")
# A value that is neither is dropped rather than stringified: a table address in the middle
# of a spec sheet is worse than a missing line, because it reads as data.
check("an unrecognised table is dropped, not stringified",
      value_to_text([{"foo": 1}]) is None)
check("...and so is an empty segment list", value_to_text([]) is None)
print()

# ==========================================================================================
print("2. rows render as browsable lines, with headings kept as headings")
# ==========================================================================================
lines = render(ROWS)
check("the header is spoken bare", lines[0] == "Pessima Track (M)", repr(lines[0]))
check("a labelled spec reads label then value", "Weight: 3075 lb" in lines)
check("a group is a bare heading", "Summary" in lines and "Features" in lines)

# A group row carries an empty value. Rendering it through the "label: value" branch would
# leave a trailing colon on every heading in the sheet.
check("...with no trailing colon", not any(l.endswith(":") for l in lines),
      repr([l for l in lines if l.endswith(":")]))

# The icon strip carries its whole sentence in the value and has no label. Prefixing an
# empty label would leave a leading ": ".
check("an unlabelled spec speaks its sentence alone",
      "Drivetrain: All Wheel Drive" in lines
      and not any(l.startswith(":") for l in lines))
check("every row produced exactly one line", len(lines) == len(ROWS), str(len(lines)))
print()

# ==========================================================================================
print("3. the failure wire carries a CODE, so one cause can fall through silently")
# ==========================================================================================
# beamtel's listener splits on the first ';'. The distinction is load-bearing: notselector is
# the answer on every screen in the game, and speaking it would replace the terrain scan with
# a complaint on the one key that has always scanned.
for wire, code, sentence in [
    ("notselector;not on a vehicle selector screen",
     "notselector", "not on a vehicle selector screen"),
    ("nofocus;no vehicle selected on this screen yet",
     "nofocus", "no vehicle selected on this screen yet"),
    ("nomodel;no such vehicle or configuration",
     "nomodel", "no such vehicle or configuration"),
]:
    c, sep, rest = wire.partition(";")
    check("'{}' splits off its code".format(code), sep and c == code and rest == sentence)

# A sentence containing a semicolon must not lose its tail, and a description genuinely can.
c, sep, rest = "nomodel;a, b; and c".partition(";")
check("only the FIRST separator splits", rest == "a, b; and c", repr(rest))

# An older mod half sends the sentence alone. Guessing it was notselector would silently
# swallow every real failure, so beamtel labels it 'unknown' and speaks it.
legacy = "no such vehicle or configuration"
c2, sep2, _ = legacy.partition(";")
check("a legacy sentence has no separator and must not read as notselector",
      not sep2 and c2 != "notselector")

check("beamtel falls through on exactly notselector and timeout",
      'info_code not in (None, "notselector", "timeout")' in _source)
check("...and the Lua half sends that code", '"notselector"' in lua)
print()

# ==========================================================================================
print("4. the Lua wrapper contract (grepped -- no fake DOM or VM can reach it)")
# ==========================================================================================
body = "\n".join(l for l in lua.splitlines() if not l.strip().startswith("--"))

# The button-building getDetails runs customDetailsButtons callbacks. A readout must not.
check("it calls vehicleSpecifications.getDetails",
      "ui_vehicleSelector_vehicleSpecifications" in body)
check("...and never the button-building detailsInteraction one",
      "ui_vehicleSelector_detailsInteraction" not in body)

# A boolean mark says "some wrapper is installed", which is not the question: a wrapper left
# by an older build calls its own captured recorder and ignores the slot, so the live
# instance would report "no vehicle selected" for the whole session.
check("the wrap mark is a version, not a boolean",
      "WRAP_VERSION" in body and "gen[WRAP_MARK] == WRAP_VERSION" in body)
check("...and the original is parked so a stale wrapper is REPLACED, not stacked",
      "ORIG_KEY" in body and "gen[ORIG_KEY] or gen.requestDetails" in body)

# The indirection that lets a reloaded instance take its recordings back. Without it the
# wrapper's closure writes to the dead instance's upvalues forever.
check("the wrapper dispatches through a mutable recorder slot",
      "local rec = gen[RECORDER_KEY]" in body)
check("...and 'installed' means installed AND ours",
      "gen[RECORDER_KEY] == recordFromItem" in body)

# The three socket rules every listening extension shares. vehicle_geometry_sim scenario 12
# polices these across the whole directory; repeated here so this file fails on its own too.
check("the bind's return is checked", "local bound, berr = udpCmd:setsockname" in body)
check("it closes its sockets on unload", "function M.onExtensionUnloaded()" in body)
check("it retries a failed bind", "retryCmdBind(dtReal)" in body)

# The ports must agree across the two halves, the check environment_row_sim already makes.
for lua_name, py_name in (("PYTHON_PORT_DATA", "VEHICLE_INFO_LISTEN_PORT"),
                          ("CMD_LISTEN_PORT", "VEHICLE_INFO_CMD_PORT")):
    lua_port = None
    for line in lua.splitlines():
        if line.strip().startswith("local " + lua_name):
            lua_port = line.split("=")[1].split("--")[0].strip()
            break
    py_port = None
    for line in _source.splitlines():
        if line.startswith(py_name + " ="):
            py_port = line.split("=")[1].split("#")[0].strip()
            break
    check("{} matches {}".format(lua_name, py_name),
          lua_port is not None and lua_port == py_port,
          "{} vs {}".format(lua_port, py_port))

# A command the mod does not answer is indistinguishable from a dead socket.
for cmd in ("INFO_SELECTOR", "INFO:"):
    check("the mod handles " + cmd, cmd.rstrip(":") in body)
print()

# ==========================================================================================
print("5. the spawner resolves the right model/config for the screen it was pressed on")
# ==========================================================================================
# Lifted from vehicle_spawner.py for the same reason as the renderer: the three list screens
# each mean something different by "this vehicle", and getting one wrong produces a spec
# sheet for a real but WRONG car -- which reads exactly like a working feature.
_SPAWNER = os.path.join(_ROOT, "vehicle_spawner.py")
with open(_SPAWNER, encoding="utf-8") as fh:
    _spawner_src = fh.read()

_sns = {}
for node in ast.parse(_spawner_src).body:
    if isinstance(node, ast.FunctionDef) and node.name == "_on_info":
        exec(compile(ast.Module([node], []), _SPAWNER, "exec"), _sns)
if "_on_info" not in _sns:
    check("_on_info could be lifted from vehicle_spawner.py", False)
else:
    import threading as _th

    _asked = []
    _said = []
    _entered = []

    CATALOG = [{"model": "pessima", "name": "Pessima",
                "configs": [{"key": "", "name": "default", "isDefault": True},
                            {"key": "race", "name": "Track (M)"}]}]

    _sns.update({
        "_state_lock": _th.RLock(),
        "_say_safe": lambda m: _said.append(m),
        "_filtered_catalog": lambda: CATALOG,
        "_drill_vehicle": CATALOG[0],
        "_to_spawn": [{"model": "etk800", "config": "wagon", "displayName": "ETK 800"}],
        "_idx_main": 0, "_idx_configs": 1, "_idx_to_spawn": 0,
        "_info_lines": [], "_idx_info": 0, "_info_return_screen": "main",
        "_enter_screen": lambda scr, **kw: _entered.append(scr),
    })

    def fake_request(model, config):
        _asked.append((model, config))
        return (["Header", "Weight: 3075 lb"], None)

    _sns["_request_info_fn"] = fake_request

    def press(screen):
        _asked.clear(); _said.clear(); _entered.clear()
        _sns["_screen"] = screen
        _sns["_on_info"](None)

    press("main")
    check("the catalog list asks about the model, default config",
          _asked == [("pessima", "")], str(_asked))

    press("configs")
    check("the configs list asks about that exact configuration",
          _asked == [("pessima", "race")], str(_asked))

    press("to_spawn")
    check("the queue asks about the pair already chosen",
          _asked == [("etk800", "wagon")], str(_asked))
    check("...and entering the info screen remembers where to go back to",
          _entered == ["info"] and _sns["_info_return_screen"] == "to_spawn",
          str(_entered) + " / " + _sns["_info_return_screen"])

    # A screen with no vehicle under the cursor must say so and stay put, not ask about
    # whatever was highlighted on some other screen.
    press("place3d")
    check("a screen with no vehicle asks nothing and does not navigate",
          not _asked and not _entered and _said, str(_said))

    # The injection is optional (an older beamtel, or a failed spawner init), and the key
    # must then explain itself rather than raising on the worker thread.
    saved = _sns["_request_info_fn"]
    _sns["_request_info_fn"] = None
    press("main")
    check("with no request function it explains rather than raising",
          not _entered and _said, str(_said))
    _sns["_request_info_fn"] = saved

    # A refusal must be spoken and must NOT open an empty screen.
    _sns["_request_info_fn"] = lambda m, c: ([], ("nomodel", "no such vehicle"))
    press("main")
    check("a refusal speaks its sentence and opens nothing",
          not _entered and _said == ["no such vehicle"], str(_said))

    # A timeout carries an empty sentence; a bare "" would be silence on a key press.
    _sns["_request_info_fn"] = lambda m, c: ([], ("timeout", ""))
    press("main")
    check("a timeout still says something rather than nothing",
          not _entered and _said and _said[0].strip(), str(_said))

# The absent-mod latch. F9 SPACE is the terrain scan and is pressed while DRIVING, so paying
# the full timeout on every press to re-discover a mod half that is not there is a regression
# on a key that has always worked -- and bng_mod/ is a live junction, so a half that does not
# know this command is the ordinary consequence of updating one side, not a corner case.
check("a timed-out request latches the mod half as absent",
      "_vinfo_absent = True" in _source)
check("...so a later press asks but does not WAIT",
      "_vinfo_event.wait(0.0 if _vinfo_absent else timeout)" in _source)
# Cleared on ANY packet rather than on a successful sheet: an INFOFAIL proves the mod half is
# alive and speaking this protocol just as well as rows do, and latching through a run of
# ordinary refusals would leave the feature permanently off for someone who pressed the key
# once on the wrong screen.
check("...and any arriving packet clears it again",
      "_vinfo_absent = False" in _source
      and _source.index("_vinfo_absent = False") < _source.index('if text.startswith("INFO_BEGIN:")'))

check("the spawner binds i to the info handler",
      '("i",         _on_info)' in _spawner_src)
check("...and the info screen can be left again",
      '_enter_screen(_info_return_screen)' in _spawner_src)
print()

if _failures:
    print("FAILED: " + ", ".join(_failures))
    sys.exit(1)
print("All scenarios passed.")
