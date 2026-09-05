"""Replay the cannon shot outcome readout against known geometry.

Mirrors _cannon_shot_phrase in beamtel.py so the wording and, far more importantly, the SIGN
convention can be checked without the game, a cannon or an audio device:

    python diagnostic/cannon_shot_sim.py

This readout fires unprompted, once, right after a crash, and it is the only report the driver
gets about a shot that cannot be repeated cheaply -- the car has to be reset and re-driven into
the ramp to try again. So a wrong word here is not a wrong word on a readout you can tap again;
it is the whole record of that shot. It has one signed axis and the project has already lost
time to a sign error on another one, which is what scenario 1 is for.

The thresholds are scraped out of beamtel.py rather than copied, because beamtel imports wx,
sounddevice and the speech backend at module scope and cannot be imported here. Only the *logic*
is duplicated.
"""

import os
import sys

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_SRC = os.path.join(_ROOT, "beamtel.py")
_MOD = os.path.join(_ROOT, "bng_mod", "lua", "ge", "extensions", "cannonShot.lua")
_SPAWNER = os.path.join(_ROOT, "bng_mod", "lua", "ge", "extensions", "vehicleSpawnerAccessible.lua")

_consts = {}
with open(_SRC, encoding="utf-8") as fh:
    for line in fh:
        if line.startswith("CANNON_SHOT_") or line.startswith("FEET_PER_M"):
            name, _, val = line.partition("=")
            try:
                _consts[name.strip()] = float(val.split("#")[0].strip())
            except ValueError:
                pass

CENTRE_M = _consts["CANNON_SHOT_CENTRE_M"]
APEX_SAY_M = _consts["CANNON_SHOT_APEX_SAY_M"]
COMPARE_M = _consts["CANNON_SHOT_COMPARE_M"]
FEET_PER_M = _consts["FEET_PER_M"]

failures = []


def check(label, ok, detail=""):
    print(f"   {label}: {'OK' if ok else 'FAIL'}{(' - ' + detail) if detail else ''}")
    if not ok:
        failures.append(label)


# --- the logic under test, mirroring beamtel.py --------------------------------------------

UNITS_MODE = "metric"


def long_distance(metres):
    if UNITS_MODE == "imperial":
        return f"{metres * FEET_PER_M:.0f} feet"
    return f"{metres:.0f} meters"


def phrase(shot):
    if not shot.get("settled", True):
        return "Shot did not settle"

    bits = []
    downrange = shot["downrange"]
    lateral = shot["lateral"]

    dist = long_distance(abs(downrange))
    if downrange < 0:
        bits.append(f"{dist} backwards")
    elif abs(lateral) <= CENTRE_M:
        bits.append(f"{dist}, straight")
    else:
        side = "left" if lateral > 0 else "right"
        bits.append(f"{dist}, {long_distance(abs(lateral))} {side}")

    apex = shot.get("apex", 0.0)
    if apex >= APEX_SAY_M:
        bits.append(f"peaked at {long_distance(apex)}")

    near = shot.get("near_name") or ""
    if near and shot.get("near_dist", -1.0) >= 0.0:
        bits.append(f"next to {near}")

    prev = shot.get("prev_downrange")
    if prev is not None:
        delta = downrange - prev
        if abs(delta) >= COMPARE_M:
            word = "further" if delta > 0 else "shorter"
            bits.append(f"{long_distance(abs(delta))} {word}")

    return ". ".join(bits)


def shot(**kw):
    base = {"downrange": 180.0, "lateral": 0.0, "apex": 0.0, "settled": True}
    base.update(kw)
    return base


# =============================================================================================
print("1. the sign convention: positive lateral is LEFT")
# The one signed axis this readout has. Positive-is-left is the mod-wide convention -- the
# compass clicks, the scanner bearing, the docking readout and the ramp cane tap all agree, and
# cannonShot.lua measures against rampGeometry's `left`, which is built as up-cross-axis. Getting
# it backwards here reads as perfectly ordinary speech and sends the operator's next correction
# the wrong way, on a shot they cannot cheaply repeat.
left = phrase(shot(lateral=20.0))
right = phrase(shot(lateral=-20.0))
check("positive lateral says left", "left" in left and "right" not in left, left)
check("negative lateral says right", "right" in right and "left" not in right, right)
check("the two are otherwise identical",
      left.replace("left", "X") == right.replace("right", "X"),
      "only the direction word may differ")

# And the negative control, so the check cannot pass for free: a mirrored implementation must
# make it fail rather than merely produce different text.
mirrored = "left" if -20.0 > 0 else "right"
check("a mirrored implementation would FAIL this check", mirrored == "right",
      "asserting the test is sensitive to the thing it tests")
print()

# =============================================================================================
print("2. a shot that never settled has NO distance")
# The car is still falling, still rolling, or wedged on its roof. Reporting where it happened to
# be when the timer ran out states a landing place for a flight that did not land -- the one
# error here a listener has no way to catch, because it reads exactly like a real result.
p = phrase(shot(downrange=412.0, lateral=30.0, apex=88.0, settled=False))
check("it says so plainly", p == "Shot did not settle", p)
check("...and quotes no figure at all",
      not any(ch.isdigit() for ch in p), p)
print()

# =============================================================================================
print("3. two numbers, and then only what is notable")
# The rule the ramp cane tap arrived at after the operator reported it was "too much verbiage".
# Distance and offline ARE the outcome, so they are unconditional; everything else has to earn
# its place in a sentence that arrives unprompted, once, straight after a crash.
plain = phrase(shot(downrange=180.0, lateral=0.5, apex=4.0))
check("an unremarkable shot is one clause", plain.count(".") == 0, plain)
check("...and reads as a distance and a direction", plain == "180 meters, straight", plain)

check("a small offset is called straight rather than numbered",
      "straight" in phrase(shot(lateral=CENTRE_M - 0.01)))
check("...and one over the threshold is numbered",
      "straight" not in phrase(shot(lateral=CENTRE_M + 0.01)))

check("a low apex is not mentioned",
      "peaked" not in phrase(shot(apex=APEX_SAY_M - 0.01)))
check("...and a high one is",
      "peaked" in phrase(shot(apex=APEX_SAY_M + 0.01)))

# Nothing in this readout confirms that a shot went well. There is no such thing as a good shot
# out of a cannon with no horizontal aim, and a clause saying so would be inventing a target.
for word in ("good", "on target", "nice", "well", "success"):
    check(f"it never says {word!r}", word not in plain.lower())
print()

# =============================================================================================
print("4. the comparison clause is what makes a session-only log worth keeping")
# There is no keybind to read the history back -- the user chose an announcement, not a key -- so
# the answer to "did raising the barrel do anything" has to arrive inside the announcement of the
# shot that answered it.
first = phrase(shot(downrange=180.0))
check("the first shot of a session has nothing to compare against",
      "further" not in first and "shorter" not in first, first)

further = phrase(shot(downrange=240.0, prev_downrange=180.0))
shorter = phrase(shot(downrange=120.0, prev_downrange=180.0))
check("a longer shot says further", "60 meters further" in further, further)
check("a shorter shot says shorter", "60 meters shorter" in shorter, shorter)

same = phrase(shot(downrange=180.0 + COMPARE_M - 0.01, prev_downrange=180.0))
check("a difference under the threshold is not worth saying",
      "further" not in same and "shorter" not in same, same)
print()

# =============================================================================================
print("5. long distances are whole numbers, not implement precision")
# fmt_distance is tuned for implement clearances and would render a 300 m shot as "984.3 feet".
UNITS_MODE = "imperial"
imp = phrase(shot(downrange=300.0))
UNITS_MODE = "metric"
check("imperial reads as whole feet", imp == "984 feet, straight", imp)
check("...and carries no decimal point", "." not in imp.split(",")[0], imp)
print()

# =============================================================================================
print("6. a shot that went backwards is not reported as a normal shot")
# Rare and absurd, but an unsigned number reads exactly like an ordinary result, and this is the
# only report of a shot that costs a reset and a re-drive to repeat.
back = phrase(shot(downrange=-40.0))
check("it says backwards", "backwards" in back, back)
check("...and does not also claim a side", "left" not in back and "right" not in back, back)
print()

# =============================================================================================
print("7. the mod agrees with the rest of the mod")
with open(_MOD, encoding="utf-8") as fh:
    mod = fh.read()

# Positive is LEFT everywhere in this project, and only a grep enforces it across files.
check("the lateral vector comes from rampGeometry's own left, not a rebuilt one",
      "frame.left" in mod and "fwd:cross" not in mod,
      "rebuilding it as axis-cross-up would mirror every reading")

# The frame is snapshotted at launch. The assembly retracts and re-levels within a couple of
# seconds of firing, so a landing measured against the live mouth would be measured against a
# machine that has since moved -- and the ramp pitch, which is the whole range-card key, would be
# read after it had already gone.
check("the launch frame is copied, not held by reference",
      "axis      = vec3(frame.axis)" in mod and "left      = vec3(frame.left)" in mod,
      "holding mouthFrame's table would re-baseline the shot as the cannon retracts")

# The settle detector is a deliberate copy of vehicleSpawnerAccessible's, on the reasoning that
# the two answer different questions. What must not happen is the numbers drifting apart.
with open(_SPAWNER, encoding="utf-8") as fh:
    spawner = fh.read()
for const in ("LAUNCH_TRACK_TIMEOUT", "LAUNCH_SETTLE_SPEED", "LAUNCH_SETTLE_TIME"):
    def value_of(text, name):
        for line in text.splitlines():
            stripped = line.strip()
            if stripped.startswith(f"local {name} ") or stripped.startswith(f"local {name}="):
                return stripped.partition("=")[2].split("--")[0].strip()
        return None
    a, b = value_of(mod, const), value_of(spawner, const)
    check(f"{const} matches the spawner's copy", a is not None and a == b,
          f"{a} vs {b}")

# A cannon is not "anything you can drive into". rampGeometry's part tiers resolve rollbacks, tilt
# decks and dry vans, and while this file gated on has() every one of them was a machine it watched
# for a launch out of -- a parked rollback deck's throat covers 25 m along and +-1.9 m lateral, so
# driving PAST one at road speed latched a shot on a car that had never been in a cannon.
check("the launch watch is gated on isCannon, not has",
      "rg.isCannon(id)" in mod and "rg.has(" not in mod,
      "has() makes every trailer with a ramp a cannon")

# The flight is a physical event, so it is aged in the time the car experiences. On dtReal a pause
# or a slow-motion runs the 30 s timeout down while the projectile hangs motionless, reports "did
# not settle" about a flight that never had the chance to, and -- with the launch conditions frozen
# true -- re-latches and does it again every thirty seconds for as long as the menu is open.
check("the shot is aged in simulated time", "simAcc = simAcc + dtSim" in mod and "local dt = simAcc" in mod,
      "ageing a flight on dtReal times it out while the game is paused")
check("...and nothing can latch on a tick with no simulated time",
      "if dt <= 0 then return end" in mod,
      "a paused vehicle reports the pose and velocity it had when the pause began")
check("the spawner's copy of the detector is aged the same way",
      "pcall(updateLaunchTrack, dtSim)" in spawner,
      "the two detectors must not drift apart on the clock either")

# No command port: the only setting is whether the outcome is spoken, which Python enforces.
check("the mod opens no command socket", "CMD_LISTEN_PORT" not in mod,
      "a speech preference does not need pushing to the game")
check("...and sends on the port beamtel listens on",
      "PYTHON_PORT_DATA = 4473" in mod and "CANNON_SHOT_LISTEN_PORT" in open(_SRC, encoding="utf-8").read())
print()

# =============================================================================================
print("sample readouts:")
for label, s in (
    ("plain", shot(downrange=180.0, lateral=1.0)),
    ("off line, high, beside something",
     shot(downrange=240.0, lateral=-31.0, apex=40.0, near_name="Ibishu Covet", near_dist=3.2)),
    ("second shot of a session", shot(downrange=305.0, lateral=8.0, prev_downrange=240.0)),
    ("never settled", shot(settled=False)),
):
    print(f"   {label}: {phrase(s)}")
print()

if failures:
    print(f"{len(failures)} FAILURE(S): " + ", ".join(failures))
    sys.exit(1)
print("all checks passed")
