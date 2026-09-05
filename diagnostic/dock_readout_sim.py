"""Replay the docking cane-tap readout against known geometry.

Mirrors _band_name / _dock_phrase in beamtel.py so the wording and, far more importantly,
the SIGN conventions can be checked without the game or an audio device:

    python diagnostic/dock_readout_sim.py

This exists because sign errors in this codebase do not announce themselves. A bearing that
says "right" when it means "left" reads as perfectly plausible speech, and the project has
already lost time to one -- three Lua comments claiming positive meant right, and a bug
report filed against correct code. The readout has three signed axes, so it has three ways
to make that mistake, and each one sends a blind operator the wrong way.

The thresholds are scraped out of beamtel.py rather than copied, because beamtel imports wx,
sounddevice and the speech backend at module scope and cannot be imported here. Only the
*logic* is duplicated.
"""

import math
import os
import sys

_SRC = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "beamtel.py")

_consts = {}
with open(_SRC, encoding="utf-8") as fh:
    for line in fh:
        if (
            line.startswith("IMPL_DOCK_")
            or line.startswith("RAMP_DOCK_")
            or line.startswith("RAMP_CORRIDOR_")
            or line.startswith("RAMP_ALIGN_")
            or line.startswith("RAMP_SELF_")
        ):
            name, _, val = line.partition("=")
            try:
                _consts[name.strip()] = float(val.split("#")[0].strip())
            except ValueError:
                pass

LEVEL_M = _consts["IMPL_DOCK_LEVEL_M"]
YAW_DEG = _consts["IMPL_DOCK_YAW_DEG"]
ENTRY_MIN_M = _consts["IMPL_DOCK_ENTRY_MIN_M"]
ENTRY_EXIT_M = _consts["IMPL_DOCK_ENTRY_EXIT_M"]
CENTRE_M = _consts["RAMP_DOCK_CENTRE_M"]
SQUARE_DEG = _consts["RAMP_DOCK_SQUARE_DEG"]
HEADING_ZERO_DEG = _consts["RAMP_DOCK_HEADING_ZERO_DEG"]
TIGHT_M = _consts["RAMP_DOCK_TIGHT_M"]
PITCH_DEG = _consts["RAMP_DOCK_PITCH_DEG"]
CORRIDOR_ENTER_M = _consts["RAMP_CORRIDOR_ENTER_M"]
CORRIDOR_EXIT_M = _consts["RAMP_CORRIDOR_EXIT_M"]
CORRIDOR_MIN_RANGE_M = _consts["RAMP_CORRIDOR_MIN_RANGE_M"]
# Scraped from audio.py the same way, because the readout has to know where the tones stop in
# order to say so. The mod now feeds this line from well outside that range on purpose.
TONE_RANGE_M = None
with open(os.path.join(os.path.dirname(_SRC), "audio.py"), encoding="utf-8") as fh:
    for line in fh:
        if line.startswith("DOCK_RAMP_MAX_RANGE_M"):
            TONE_RANGE_M = float(line.partition("=")[2].split("#")[0].strip())
            break
assert TONE_RANGE_M, "could not find DOCK_RAMP_MAX_RANGE_M in audio.py"

failures = []


def check(label, ok, detail=""):
    print(f"   {label}: {'OK' if ok else 'FAIL'}{(' - ' + detail) if detail else ''}")
    if not ok:
        failures.append(label)


# --- the logic under test, mirroring beamtel.py -------------------------------------------


def band_name(kind, idx, count):
    if kind == "GAP":
        return "underside" if idx <= 1 else "opening"
    if idx >= count:
        return "roof"
    if idx <= 1:
        return "base"
    return "body"


def fmt(metres):
    return round(float(metres), 2), "meters"


def dock_phrase(d):
    band = band_name(d["kind"], d["idx"], d["count"])
    bits = [f"{d['impl_word']}, {d['name']}"]

    ref = f"{band} {d['idx']} of {d['count']}"
    if d["manual"]:
        ref += ", held"
    tv, tu = fmt(max(0.0, d["hi_z"] - d["lo_z"]))
    bits.append(f"{ref}, {tv} {tu} tall")

    vert = d["vertical"]
    if abs(vert) < LEVEL_M:
        bits.append("level")
    else:
        vv, vu = fmt(abs(vert))
        bits.append(f"{'raise' if vert > 0 else 'lower'} {vv} {vu}")

    lat = d["lateral"]
    if abs(lat) < LEVEL_M:
        bits.append("centred")
    else:
        lv, lu = fmt(abs(lat))
        bits.append(f"{'left' if lat > 0 else 'right'} {lv} {lu}")

    rv, ru = fmt(max(0.0, d["range"]))
    bits.append(f"range {rv} {ru}")

    yaw = d["yaw"]
    if abs(yaw) >= YAW_DEG:
        bits.append(f"face {abs(yaw):.0f} degrees {'left' if yaw > 0 else 'right'}")

    depth = d.get("entry_depth")
    if depth is not None and 0.0 <= depth < ENTRY_MIN_M:
        dv, du = fmt(max(0.0, depth))
        bits.append(f"tines enter {dv} {du}, too steep")

    return ". ".join(bits)


def ramp_bearing(rng, lat, yaw):
    """Mirrors _ramp_bearing_deg in beamtel.py."""
    return ((math.degrees(math.atan2(lat, rng)) + yaw + 180.0) % 360.0) - 180.0


def ramp_acquire(rng, lat, prev):
    """Mirrors _ramp_acquire in beamtel.py."""
    if rng < CORRIDOR_MIN_RANGE_M:
        return True
    return abs(lat) > (CORRIDOR_ENTER_M if prev else CORRIDOR_EXIT_M)


def dock_phrase_ramp(d):
    """Mirrors _dock_phrase_ramp in beamtel.py."""
    bits = []
    rng, lat, yaw = d["range"], d["lateral"], d["yaw"]

    if d.get("acquire"):
        dv, du = fmt(math.hypot(rng, lat))
        bearing = ramp_bearing(rng, lat, yaw)
        if abs(bearing) < SQUARE_DEG:
            bits.append(f"ahead, {dv} {du}")
        else:
            bits.append(f"{abs(bearing):.0f} {'left' if bearing > 0 else 'right'}, {dv} {du}")
        if rng <= -CORRIDOR_MIN_RANGE_M:
            bits.append("wrong side")
    else:
        rv, ru = fmt(max(0.0, rng))
        if abs(lat) <= CENTRE_M:
            bits.append(f"centred, {rv} {ru}")
        else:
            lv, _lu = fmt(abs(lat))
            bits.append(f"{lv} {'left' if lat > 0 else 'right'}, {rv} {ru}")

        if abs(yaw) < HEADING_ZERO_DEG:
            bits.append("heading zero degrees")
        else:
            heading = max(1, int(math.floor(abs(yaw) + 0.5)))
            bits.append(
                f"heading {heading} degrees {'left' if yaw > 0 else 'right'}"
            )

    margin = None if d.get("acquire") else d.get("margin")
    if margin is not None and margin > -0.999:
        if margin <= 0.0:
            mv, mu = fmt(abs(margin))
            bits.append(f"too narrow by {mv} {mu}")
        elif margin < TIGHT_M:
            mv, mu = fmt(margin)
            bits.append(f"tight, {mv} {mu} each side")

    pitch = d.get("entry_theta")
    if pitch is not None and abs(pitch) >= PITCH_DEG:
        bits.append(f"ramp {'up' if pitch > 0 else 'down'} {abs(pitch):.0f} degrees")

    if math.hypot(rng, lat) >= TONE_RANGE_M:
        bits.append("too far for tones")
    return ". ".join(bits)


def phrase(d):
    """The dispatcher, mirroring _dock_phrase."""
    if d.get("mode") == "RAMP":
        return dock_phrase_ramp(d)
    return dock_phrase(d)


def make_ramp(**kw):
    d = {
        "name": "large cannon", "mode": "RAMP", "range": 8.0, "lateral": 0.0,
        "vertical": 0.0, "idx": 0, "count": 0, "kind": "RAMP", "lo_z": 0.0, "hi_z": 0.0,
        "yaw": 0.0, "manual": False, "impl_word": "Bucket",
        "entry_theta": 0.0, "entry_depth": -1.0, "margin": 0.9,
    }
    d.update(kw)
    return d


def make(**kw):
    d = {
        "name": "pallet", "impl_word": "Forks", "range": 1.0,
        "lateral": 0.0, "vertical": 0.0, "idx": 1, "count": 3,
        "kind": "GAP", "lo_z": 0.0, "hi_z": 0.15, "yaw": 0.0, "manual": False,
        "entry_theta": 0.0, "entry_depth": 1.2,
    }
    d.update(kw)
    return d


# --- the wire parse, mirroring the DOCK: branch of implement_listener ----------------------


def parse_dock(text):
    """The two entry-gate fields are an optional tail, so a mod older than this build keeps
    its readout instead of losing it. bng_mod/ is a live junction into the game install, so
    the two halves genuinely do go out of step."""
    fields = text[5:].rsplit(",", 14)
    if len(fields) != 15:
        fields = text[5:].rsplit(",", 12)
        # elif, not a second if: a successful 15-field split leaves len 15, which
        # is also != 13, so an independent test would immediately re-split it down
        # to 11 and throw the mode and margin away on every ramp packet.
        if len(fields) != 13:
            fields = text[5:].rsplit(",", 10)
    if len(fields) not in (11, 13, 15):
        return None
    long_tail = len(fields) >= 13
    try:
        return {
            "name": fields[0].strip(),
            "range": float(fields[1]),
            "lateral": float(fields[2]),
            "vertical": float(fields[3]),
            "idx": int(fields[4]),
            "count": int(fields[5]),
            "kind": fields[6].strip().upper(),
            "lo_z": float(fields[7]),
            "hi_z": float(fields[8]),
            "yaw": float(fields[9]),
            "manual": fields[10].strip() == "1",
            "impl_word": "Forks",
            "entry_theta": float(fields[11]) if long_tail else None,
            "entry_depth": float(fields[12]) if long_tail else None,
            "mode": fields[13].strip().upper() if len(fields) == 15 else "IMPL",
            "margin": float(fields[14]) if len(fields) == 15 else None,
        }
    except ValueError:
        return None


def entry_edges(depths):
    """Replays the one-shot earcon trigger: fires only on a not-enterable -> enterable
    transition, with the same two thresholds the mod applies, and never on first
    acquisition. Returns the number of firings."""
    ok, fired = None, 0
    for depth in depths:
        if depth is None or depth < 0.0:
            ok = None
            continue
        thresh = ENTRY_EXIT_M if ok else ENTRY_MIN_M
        now = depth >= thresh
        if now and ok is False:
            fired += 1
        ok = now
    return fired


def approach_edges(samples):
    """Count hunting-to-approach handoffs; None represents target loss."""
    acquiring, fired = True, 0
    for sample in samples:
        if sample is None:
            acquiring = True
            continue
        rng, lat = sample
        was_acquiring = acquiring
        acquiring = ramp_acquire(rng, lat, acquiring)
        if was_acquiring and not acquiring:
            fired += 1
    return fired


print(f"tuning: level/centred under {LEVEL_M} m, squareness spoken from {YAW_DEG} degrees")
print()

print("1. vertical sign: positive means the band is ABOVE the cutting edge, so RAISE")
# The mod computes bandMid - implementEdge.z. Forks below a pallet pocket therefore report
# a positive number, and the only correct instruction is to lift.
check("band above the tines says raise", "raise" in dock_phrase(make(vertical=0.30)),
      dock_phrase(make(vertical=0.30)))
check("band below the tines says lower", "lower" in dock_phrase(make(vertical=-0.30)),
      dock_phrase(make(vertical=-0.30)))
check("raise and lower are never both present",
      not ("raise" in dock_phrase(make(vertical=0.3))
           and "lower" in dock_phrase(make(vertical=0.3))))
check("magnitude is spoken unsigned",
      "-" not in dock_phrase(make(vertical=-0.30)).split("lower")[1].split(".")[0],
      "a spoken 'lower minus 0.3' would be read as a double negative")
print()

print("2. lateral sign: positive means the target is to the driver's LEFT")
# Mod-wide convention, shared with the scanner bearing and the compass clicks. The one place
# it could plausibly have been inverted is here, because the natural phrasing is an
# instruction ("steer left") rather than a position, and those are the same direction only
# because the target being left is exactly when you steer left.
check("target to the left says left", "left" in dock_phrase(make(lateral=0.4)),
      dock_phrase(make(lateral=0.4)))
check("target to the right says right", "right" in dock_phrase(make(lateral=-0.4)),
      dock_phrase(make(lateral=-0.4)))
print()

print("3. the dead zone reports a state, not a number")
# Soft-body jitter moves the cutting edge a couple of centimetres on a parked machine.
# Reading that out invites a correction that cannot be made.
jitter = LEVEL_M * 0.8
p = dock_phrase(make(vertical=jitter, lateral=-jitter))
check("sub-threshold vertical says level", "level" in p, p)
check("sub-threshold lateral says centred", "centred" in p, p)
check("no number is offered for either", "raise" not in p and "lower" not in p)
just_over = LEVEL_M * 1.2
check("just over threshold does give a number",
      "raise" in dock_phrase(make(vertical=just_over)),
      dock_phrase(make(vertical=just_over)))
print()

print("4. squareness stays silent until it would jam the tines")
check("small angle is not mentioned", "face" not in dock_phrase(make(yaw=YAW_DEG * 0.5)),
      dock_phrase(make(yaw=YAW_DEG * 0.5)))
check("large angle is mentioned", "face" in dock_phrase(make(yaw=YAW_DEG * 2)),
      dock_phrase(make(yaw=YAW_DEG * 2)))
check("angled left reads left", "degrees left" in dock_phrase(make(yaw=20.0)))
check("angled right reads right", "degrees right" in dock_phrase(make(yaw=-20.0)))
print()

print("5. band naming tracks position in the stack, not absolute height")
# Named by ordinal because world Z means nothing spoken aloud, and because what identifies a
# band is where it sits relative to the others -- the lowest void in a pallet is the pocket
# whether the pallet is on the ground or on a truck bed.
check("lowest void is the underside", band_name("GAP", 1, 4) == "underside")
check("a higher void is an opening", band_name("GAP", 3, 4) == "opening")
check("topmost solid is the roof", band_name("SOLID", 4, 4) == "roof")
check("lowest solid is the base", band_name("SOLID", 1, 4) == "base")
check("a middle solid is the body", band_name("SOLID", 2, 4) == "body")
check("a single-band object still names cleanly", band_name("SOLID", 1, 1) == "roof",
      "idx == count wins, which is right: one solid run IS the top of it")
print()

print("6. a manual pick is announced as held, so auto-select is never assumed")
check("manual says held", "held" in dock_phrase(make(manual=True)))
check("auto does not", "held" not in dock_phrase(make(manual=False)))
print()

print("7. negative range from an overlap never reaches speech")
# The gap legitimately reaches zero on contact and the box tier can round through it.
check("negative range is floored", "range 0.0" in dock_phrase(make(range=-0.02)),
      dock_phrase(make(range=-0.02)))
print()

print("8. the entry gate speaks only when the tines cannot go in")
# Every other axis can be nulled perfectly and the tines still not enter, because a tilted
# implement climbs through the band's thickness after a few centimetres of travel. That is the
# whole reason the docking instrument was re-aimed, so it is the one clause that must not be
# lost -- and equally must not be spoken on the expected case, where it would be four words of
# nothing on every tap.
check("an enterable band says nothing about entry",
      "too steep" not in dock_phrase(make(entry_depth=ENTRY_MIN_M * 1.5)),
      dock_phrase(make(entry_depth=ENTRY_MIN_M * 1.5)))
check("a band the tines cannot enter says so",
      "too steep" in dock_phrase(make(entry_depth=0.15)),
      dock_phrase(make(entry_depth=0.15)))
check("...and says how far they do go, which is the actionable half",
      "tines enter 0.15" in dock_phrase(make(entry_depth=0.15)))
check("an unmeasurable depth is silence, not a guess",
      "too steep" not in dock_phrase(make(entry_depth=-1.0)),
      "-1 is the mod's 'could not measure' sentinel")
check("a mod too old to send the fields is also silence",
      "too steep" not in dock_phrase(make(entry_depth=None)))
print()

print("9. the DOCK: wire parse tolerates a mod half older than this build")
new_line = "DOCK:pallet,1.240,-0.120,0.220,2,5,GAP,0.350,0.520,-14.0,1,36.0,0.187"
old_line = "DOCK:pallet,1.240,-0.120,0.220,2,5,GAP,0.350,0.520,-14.0,1"
p_new, p_old = parse_dock(new_line), parse_dock(old_line)
check("the long form parses", p_new is not None)
check("entry fields land in the right slots",
      p_new and abs(p_new["entry_theta"] - 36.0) < 1e-9
      and abs(p_new["entry_depth"] - 0.187) < 1e-9,
      f"theta {p_new['entry_theta']}, depth {p_new['entry_depth']}")
check("the short form still parses", p_old is not None)
check("...and the eleven fields it does carry are unshifted",
      p_old and p_old["kind"] == "GAP" and abs(p_old["yaw"] + 14.0) < 1e-9
      and p_old["manual"] is True)
check("...with the entry fields absent rather than zero",
      p_old and p_old["entry_theta"] is None and p_old["entry_depth"] is None,
      "zero would read as 'perfectly level', which is the opposite of 'unknown'")
check("a name containing a comma cannot shift the numbers",
      (lambda q: q and q["idx"] == 2 and abs(q["entry_depth"] - 0.187) < 1e-9)(
          parse_dock("DOCK:pallet, stacked,1.240,-0.120,0.220,2,5,GAP,0.350,0.520,-14.0,1,36.0,0.187")))
print()

print("10. the entry earcon fires on the transition in, and only then")
check("arriving already enterable is not an event", entry_edges([1.2, 1.2, 1.2]) == 0,
      "the cue answers 'you can go in NOW', so first acquisition must be silent")
check("tilting into range fires once", entry_edges([0.15, 0.15, 1.2, 1.2]) == 1)
check("and out and back in fires again", entry_edges([0.15, 1.2, 0.15, 1.2]) == 2)
# The mod hysteresises the depth and this side applies the same two thresholds, so a machine
# breathing on its suspension between them cannot machine-gun the earcon.
between = [(ENTRY_MIN_M + ENTRY_EXIT_M) / 2 + 0.002 * (1 if i % 2 else -1) for i in range(200)]
check("wobble between the two thresholds fires nothing after the first entry",
      entry_edges([0.15, 1.2] + between) == 1,
      f"{entry_edges([0.15, 1.2] + between)} firings across 200 wobbling ticks")
check("losing the target re-arms rather than firing on re-acquisition",
      entry_edges([0.15, 1.2, None, 1.2]) == 1)
print()

print("11. the wire parse takes all three shapes, and mode defaults honestly")
line15 = ("DOCK:large cannon,8.000,-0.400,0.000,0,0,RAMP,1.200,1.200,6.0,0,"
          "2.0,-1.000,RAMP,0.900")
line13 = "DOCK:pallet,1.000,0.100,0.220,2,5,GAP,0.350,0.520,-14.0,1,3.0,0.450"
line11 = "DOCK:pallet,1.000,0.100,0.220,2,5,GAP,0.350,0.520,-14.0,1"
p15, p13, p11 = parse_dock(line15), parse_dock(line13), parse_dock(line11)
check("the 15-field form parses", p15 is not None and p15["mode"] == "RAMP")
check("...with its margin", p15 is not None and abs(p15["margin"] - 0.9) < 1e-9,
      f"{p15['margin'] if p15 else None}")
check("the 13-field form still parses", p13 is not None and p13["entry_depth"] == 0.45)
check("the 11-field form still parses", p11 is not None and p11["entry_depth"] is None)
# A mod older than this build has no other mode to be in, so IMPL is the only thing the
# absence can mean -- it is not a guess.
check("an older mod defaults to IMPL, not to nothing",
      p13["mode"] == "IMPL" and p11["mode"] == "IMPL")
check("...and reports no margin rather than zero",
      p13["margin"] is None and p11["margin"] is None,
      "zero would read as exactly touching both walls")
# rsplit from the right, so a comma in a vehicle name cannot shift the numbers.
comma = parse_dock("DOCK:Large, Cannon,8.000,-0.400,0.000,0,0,RAMP,1.200,1.200,"
                   "6.0,0,2.0,-1.000,RAMP,0.900")
check("a comma in the name shifts nothing", comma is not None
      and comma["name"] == "Large, Cannon" and abs(comma["margin"] - 0.9) < 1e-9,
      comma["name"] if comma else "parse failed")
print()

print("12. RAMP SIGNS: positive is LEFT on both axes, as it is everywhere else in the mod")
left = phrase(make_ramp(lateral=0.40))
right = phrase(make_ramp(lateral=-0.40))
check("positive lateral says left", "left" in left and "right" not in left, left)
check("negative lateral says right", "right" in right and "left" not in right, right)
turnL = phrase(make_ramp(yaw=12.0))
turnR = phrase(make_ramp(yaw=-12.0))
check("positive yaw says left", "heading 12 degrees left" in turnL, turnL)
check("negative yaw says right", "heading 12 degrees right" in turnR, turnR)
print()

print("13. ramp mode never borrows implement wording")
# There is no band, no thickness and nothing on a car that goes up or down, so none of that
# vocabulary may leak in -- including from an over-eager mod that fills the vertical field.
banned = ("raise", "lower", "band", "tall", "held", "tines", "opening", "underside",
          "roof", "base", "body", "level")
for d in (make_ramp(), make_ramp(vertical=0.9, lateral=-2.0, yaw=-30.0, margin=-0.4),
          make_ramp(range=0.0, margin=0.05), make_ramp(entry_theta=12.0)):
    out = phrase(d)
    hit = [w for w in banned if w in out.lower()]
    check(f"no implement wording in {out!r}"[:96], not hit, ", ".join(hit))
print()

print("14. clearance is spoken as advice, and silence when it is not known")
# A comfortable margin says NOTHING. The old form read "0.9 meters clearance each side" on
# every tap of a manoeuvre that was going fine, which is most taps -- part of the "too much
# verbiage" this readout was cut back for. Bad news only, the rule the implement readout
# already applies to its entry depth.
check("a comfortable margin is not mentioned at all",
      "clearance" not in phrase(make_ramp(margin=0.9))
      and "0.9" not in phrase(make_ramp(margin=0.9)),
      phrase(make_ramp(margin=0.9)))
check("a small margin says tight",
      "tight" in phrase(make_ramp(margin=TIGHT_M - 0.05)),
      phrase(make_ramp(margin=TIGHT_M - 0.05)))
check("a negative margin says how much too narrow",
      "too narrow by" in phrase(make_ramp(margin=-0.30)),
      phrase(make_ramp(margin=-0.30)))
check("the -1 sentinel says nothing at all",
      "clearance" not in phrase(make_ramp(margin=-1.0))
      and "narrow" not in phrase(make_ramp(margin=-1.0)),
      phrase(make_ramp(margin=-1.0)))
check("an absent margin says nothing at all",
      "clearance" not in phrase(make_ramp(margin=None)),
      phrase(make_ramp(margin=None)))
print()

print("15. the ramp dead zones report a state, not a number to chase")
# 5 cm is a hydraulic-precision figure. Applied to a car it invites a correction the driver
# cannot make, and chatters over the vehicle's own suspension.
check("the ramp centre threshold is looser than the implement one", CENTRE_M > LEVEL_M,
      f"{CENTRE_M} m vs {LEVEL_M} m")
check("just inside centre says centred",
      "centred" in phrase(make_ramp(lateral=CENTRE_M - 0.01)))
check("the exact 15 cm boundary still says centred",
      "centred" in phrase(make_ramp(lateral=CENTRE_M)))
check("just outside gives a number",
      phrase(make_ramp(lateral=CENTRE_M + 0.05)).startswith("0.2 "),
      phrase(make_ramp(lateral=CENTRE_M + 0.05)))
# Heading is always stated on the approach because it is the beat pair's null. Near zero gets
# wording that cannot be mistaken for an omitted or rounded correction.
check("zero heading is explicit and unambiguous",
      "heading zero degrees" in phrase(make_ramp(yaw=0.0)),
      phrase(make_ramp(yaw=0.0)))
check("sub-half-degree heading still says zero",
      "heading zero degrees" in phrase(make_ramp(yaw=HEADING_ZERO_DEG - 0.01)))
check("the threshold starts a directional whole-degree correction",
      "heading 1 degrees left" in phrase(make_ramp(yaw=HEADING_ZERO_DEG)),
      phrase(make_ramp(yaw=HEADING_ZERO_DEG)))
check("small nonzero heading is always spoken",
      "heading 2 degrees left" in phrase(make_ramp(yaw=2.0)))
# The tap remains compact: lateral/range, then heading, with no filler.
plain = phrase(make_ramp(lateral=-0.4, yaw=2.0, margin=0.9, entry_theta=1.0))
check("an approach that is going fine is two short clauses",
      plain.count(". ") == 1 and len(plain.split()) <= 10, plain)
# The ramp's own inclination is context, not an instruction: the driver of the car cannot
# change it, so it stays quiet until it is steep enough to change the approach.
check("a level ramp says nothing about pitch",
      "ramp up" not in phrase(make_ramp(entry_theta=1.0)))
check("a steep ramp does", "ramp up" in phrase(make_ramp(entry_theta=PITCH_DEG + 2)),
      phrase(make_ramp(entry_theta=PITCH_DEG + 2)))
print()

print("16. the implement readout is byte-identical to before the split")
# The dispatcher must not have changed a single word of the answer it already gave. This is
# what makes splitting _dock_phrase provably non-regressive rather than merely tested.
cases = [
    make(), make(vertical=0.22), make(vertical=-0.22), make(lateral=0.12),
    make(lateral=-0.12), make(yaw=-14.0, manual=True), make(range=-0.5),
    make(entry_depth=0.05), make(entry_depth=-1.0), make(kind="SOLID", idx=5, count=5),
]
same = all(phrase(c) == dock_phrase(c) for c in cases)
check("every implement case routes to the unchanged wording", same)
check("...and none of them is mistaken for a ramp",
      all("Cannon ramp" not in phrase(c) for c in cases))
print()

print("17. hunting for the mouth is a different QUESTION, not a quieter answer")
# The bug this phase exists to fix: the along-range used to be floored at zero in the mod, so
# from anywhere behind the mouth plane -- which is most of a lap around a sixteen-metre cannon
# -- the readout said "mouth 0.0 feet" while the driver was nowhere near it, and the only two
# other numbers on offer were corrections to a line they were not on.
behind = make_ramp(range=-9.0, lateral=4.2, yaw=123.0, margin=-3.1, acquire=True)
said = phrase(behind)
check("a mouth behind you is reported at its REAL distance", "0.0" not in said.split(",")[1],
      said)
check("...and named as being the far side of the mouth plane", "wrong side" in said, said)

# TWO FRAMES, TWO VOCABULARIES. The bearing clause is measured from the driver's nose; the
# side clause is measured against the mouth plane. Both are routinely true at once -- aimed
# straight at an entrance you have already driven past is an ordinary state while hunting --
# and the first wording used a direction word for each, producing "ahead, 73.9 feet. behind".
# Nothing was wrong with the numbers; the utterance simply read as self-contradictory.
DIRECTION_WORDS = ("ahead", "behind", "left", "right", "front", "back")
mixed = []
for along in (-40.0, -12.0, -3.0, -0.6, 0.0, 3.0, 12.0, 40.0):
    for across in (-9.0, -4.0, 0.0, 4.0, 9.0):
        for face in (-170.0, -95.0, -20.0, 0.0, 20.0, 95.0, 170.0):
            d = make_ramp(range=along, lateral=across, yaw=face,
                          acquire=ramp_acquire(along, across, True))
            words = [w for w in DIRECTION_WORDS if w in phrase(d)]
            # This invariant belongs to acquisition speech. On the approach, lateral and
            # heading are independent corrections in the same driver frame and may
            # legitimately point in opposite directions.
            if d["acquire"] and len(words) > 1:
                mixed.append((phrase(d), words))
check("no acquisition utterance mixes two frames of direction", not mixed,
      mixed[0][0] if mixed else
      f"{len(list(DIRECTION_WORDS)) and 8 * 5 * 7} geometries, "
      "bearing and mouth-plane clauses never collide")
# ...and prove the scan is discriminating rather than passing for free: the wording it
# replaced collides on the very first geometry it is handed.
old = "ahead, 73.9 feet" + ". behind"
check("the wording it replaced WOULD collide",
      len([w for w in DIRECTION_WORDS if w in old]) > 1, repr(old))
check("the bearing to it leads, so the first thing heard is actionable",
      said.startswith("82 right"), said)
# Lateral offset, squareness and clearance are all corrections to a line. Off the line they
# are facts about where you happen to be standing, and one of them is actively alarming:
# "too narrow by three metres" is about the driver's current position, not about the car.
check("no lateral correction while hunting", "right 13" not in said and "left 4" not in said)
check("no clearance verdict while hunting", "narrow" not in said and "clearance" not in said,
      said)

# The bearing is derived, not sent, so its two terms have to compose correctly.
check("dead ahead reads as ahead", abs(ramp_bearing(10.0, 0.0, 0.0)) < 1e-9)
check("a mouth off to the left of the nose reads positive-LEFT",
      ramp_bearing(10.0, 5.0, 0.0) > 0, f"{ramp_bearing(10.0, 5.0, 0.0):+.1f} deg")
check("...and to the right, negative", ramp_bearing(10.0, -5.0, 0.0) < 0,
      f"{ramp_bearing(10.0, -5.0, 0.0):+.1f} deg")
# Sitting exactly on the axis but past the mouth: the mouth is directly astern of the axis,
# so with the nose along the axis the bearing must be a half turn, not zero.
check("directly past the mouth is astern, not ahead",
      abs(abs(ramp_bearing(-9.0, 0.0, 0.0)) - 180.0) < 1e-9,
      f"{ramp_bearing(-9.0, 0.0, 0.0):+.1f} deg")
# Turning the vehicle must move the bearing by exactly as much, in the same direction.
b0 = ramp_bearing(10.0, 0.0, 0.0)
b1 = ramp_bearing(10.0, 0.0, 30.0)
check("yaw enters the bearing one for one", abs((b1 - b0) - 30.0) < 1e-9,
      f"{b0:+.1f} -> {b1:+.1f} deg")

# The corridor latch is hysteretic, and it has to be: it swaps what the pan MEANS.
check("the handoff starts by 6 m so large lateral errors are exposed early",
      CORRIDOR_ENTER_M >= 6.0, f"enter at {CORRIDOR_ENTER_M:.1f} m")
check("the exit boundary preserves wide hysteresis",
      CORRIDOR_EXIT_M >= CORRIDOR_ENTER_M * 1.5,
      f"{CORRIDOR_ENTER_M:.1f} m enter / {CORRIDOR_EXIT_M:.1f} m exit")
check("behind the mouth plane is always hunting, however well centred",
      ramp_acquire(-0.2, 0.0, False))
check("wide of the corridor drops back to hunting",
      ramp_acquire(10.0, CORRIDOR_EXIT_M + 0.1, False))
check("...but a hair over the enter width does not, once on the approach",
      not ramp_acquire(10.0, CORRIDOR_ENTER_M + 0.1, False))
check("coming in from outside needs the tighter width to claim the approach",
      ramp_acquire(10.0, CORRIDOR_ENTER_M + 0.1, True)
      and not ramp_acquire(10.0, CORRIDOR_ENTER_M - 0.1, True))
wobble = False
flips = 0
for i in range(200):
    lat = CORRIDOR_ENTER_M + 0.02 * (1 if i % 2 else -1)
    nxt = ramp_acquire(12.0, lat, wobble)
    if nxt != wobble:
        flips += 1
    wobble = nxt
check("wobbling on the boundary does not flip the instrument", flips <= 1,
      f"{flips} change(s) over 200 ticks")
handoffs = approach_edges([
    (12.0, CORRIDOR_ENTER_M + 1.0),
    (12.0, CORRIDOR_ENTER_M - 0.1),
    (12.0, CORRIDOR_ENTER_M + 0.1),
    (12.0, CORRIDOR_EXIT_M - 0.1),
])
check("one corridor entry produces exactly one handoff cue", handoffs == 1,
      f"{handoffs} cue(s)")
check("leaving and re-entering the corridor produces one new cue",
      approach_edges([
          (12.0, CORRIDOR_ENTER_M - 0.1),
          (12.0, CORRIDOR_EXIT_M + 0.1),
          (12.0, CORRIDOR_ENTER_M - 0.1),
      ]) == 2)
check("target loss resets the handoff edge",
      approach_edges([
          (12.0, CORRIDOR_ENTER_M - 0.1),
          None,
          (12.0, CORRIDOR_ENTER_M - 0.1),
      ]) == 2)
print()

print("18. an instrument that can see the mouth but cannot sonify it SAYS SO")
# The regression this scenario exists for: parked three metres from a sixteen-metre cannon at
# the barrel end, the mouth sat outside the mod's 20 m feed ceiling, so the mod sent DOCKCLEAR,
# which wipes the reason as well as the reading -- and F9+I answered "Nothing in range" about a
# machine the scanner was reporting nine feet away. The feed now runs to 45 m while the tones
# stop at 25, and the gap is named rather than left as unexplained silence.
faraway = make_ramp(range=-30.0, lateral=6.0, yaw=100.0, acquire=True)
out = phrase(faraway)
check("a mouth beyond the tone range still reads out", out.startswith(("1", "2", "3", "4",
      "5", "6", "7", "8", "9")), out)
check("...and says why nothing is audible", "too far for tones" in out, out)
near = phrase(make_ramp(range=6.0, lateral=0.3))
check("inside the tone range it is not mentioned", "too far" not in near, near)
check("the readout's ceiling is the audio one, not the mod's",
      TONE_RANGE_M < 45.0, f"tones stop at {TONE_RANGE_M} m, the feed runs to 45")
print()

print("ramp hunting sample:", phrase(behind))
print("ramp far sample:", out)
print("ramp sample:", phrase(make_ramp(name="large cannon", range=8.0, lateral=-0.42,
                                       yaw=6.0, margin=0.9, entry_theta=2.0)))
print("ramp sample:", phrase(make_ramp(name="large cannon", range=2.1, lateral=1.55,
                                       yaw=-11.0, margin=-0.35, entry_theta=9.0)))
print()

print("sample:", dock_phrase(make(name="Gavril D-Series", impl_word="Forks",
                                  range=1.24, lateral=-0.12, vertical=0.22,
                                  idx=2, count=5, kind="GAP", lo_z=0.35, hi_z=0.52,
                                  yaw=-14.0, manual=True)))
print()

# =================================================================================================
# 18. The ramp align teleport's spoken outcome.
#
# Mirrors _ramp_align_phrase. Two things here are worth a check rather than a reading: the
# not-measured sentinel is the literal NA and must be SILENT rather than "you do not fit" -- a
# false "it will not fit" about a machine that fits is how a driver learns to ignore the clause
# -- and a genuinely negative margin must survive the parse, because that is the one case the
# clause exists for. An earlier draft used -1 for "not measured", which made a real -1.00 m
# margin unsayable; the sentinel is a string precisely so it cannot collide with a real value.
# =================================================================================================
print("18. the ramp align readout")

ALIGN_TIGHT_M = _consts["RAMP_ALIGN_TIGHT_M"]
ALIGN_SNUG_M = _consts["RAMP_ALIGN_SNUG_M"]
ALIGN_LIP_SAY_M = _consts["RAMP_ALIGN_LIP_SAY_M"]


def ramp_align_phrase(payload, fmt_fn=fmt):
    if payload.startswith("FAIL,"):
        return payload[5:].strip()
    if not payload.startswith("OK,"):
        return ""
    bits = payload[3:].split(",")
    target = bits[0].strip() or "ramp"

    def num(i):
        try:
            return float(bits[i])
        except (IndexError, ValueError):
            return None

    gap, margin, lip = num(1), num(2), num(3)
    if gap is None:
        phrase = f"Aligned to {target}"
    else:
        val, unit = fmt_fn(gap)
        phrase = f"Aligned to {target}, {val} {unit} back"
    if lip is not None and lip >= ALIGN_LIP_SAY_M:
        lv, lu = fmt_fn(lip)
        phrase += f", ramp not down, lip {lv} {lu} up"
    if margin is not None:
        if margin <= ALIGN_TIGHT_M:
            phrase += ", you do not fit"
        elif margin < ALIGN_SNUG_M:
            phrase += ", tight"
    return phrase


def fmt_ft(metres):
    return round(float(metres) * 3.28084, 1), "feet"


ok_wide = ramp_align_phrase("OK,large cannon,6.10,0.85")
check("a comfortable align says only where you are",
      "fit" not in ok_wide and "tight" not in ok_wide, ok_wide)
check("...and names the machine and the gap", "large cannon" in ok_wide and "6.1" in ok_wide,
      ok_wide)

check("the standoff is spoken in the configured unit",
      ramp_align_phrase("OK,large cannon,6.10,0.85", fmt_ft).find("20.0 feet") > 0,
      ramp_align_phrase("OK,large cannon,6.10,0.85", fmt_ft))

check("a negative margin is called out",
      "you do not fit" in ramp_align_phrase("OK,tilt deck,6.10,-1.00"),
      ramp_align_phrase("OK,tilt deck,6.10,-1.00"))
check("...including a margin of exactly -1, which the old sentinel swallowed",
      "you do not fit" in ramp_align_phrase("OK,tilt deck,6.10,-1.00"),
      "-1 as a sentinel makes the one case this clause exists for unsayable")
check("exactly zero clearance is not fitting",
      "you do not fit" in ramp_align_phrase("OK,dry van,6.10,0.00"),
      "0 means touching both walls, not clear")
check("a few centimetres a side is called tight",
      ramp_align_phrase("OK,dry van,6.10,0.04").endswith(", tight"),
      ramp_align_phrase("OK,dry van,6.10,0.04"))

na = ramp_align_phrase("OK,large cannon,6.10,NA")
check("an unmeasured margin is SILENT, not a warning",
      "fit" not in na and "tight" not in na, na)

# The lip. A rollback's own three poses, measured in game: 1.30 m home and level, 0.95 m on full
# tilt alone, 0.15 m with the bed fully out and tilted. The align is geometrically perfect
# against all three and used to say only where you were, which is how a driver ends up parked
# twenty feet in front of the back of a truck.
stowed = ramp_align_phrase("OK,us semi,6.10,0.85,1.30", fmt_ft)
tilted = ramp_align_phrase("OK,us semi,6.10,0.85,0.95", fmt_ft)
ready = ramp_align_phrase("OK,us semi,6.10,0.85,0.15", fmt_ft)
check("a stowed deck is called out", "ramp not down" in stowed, stowed)
check("...with the height, so the driver knows what they are looking at",
      "4.3 feet up" in stowed, stowed)
check("tilt ALONE is still not down — the bed is what brings the lip in",
      "ramp not down" in tilted, tilted)
check("a deployed ramp says nothing about the lip", "lip" not in ready, ready)
check("...and still reports the width margin it always did",
      ramp_align_phrase("OK,us semi,6.10,0.04,0.15").endswith(", tight"),
      ramp_align_phrase("OK,us semi,6.10,0.04,0.15"))
check("an unmeasured lip is SILENT, never a zero that reads as 'on the ground'",
      "lip" not in ramp_align_phrase("OK,us semi,6.10,0.85,NA"),
      ramp_align_phrase("OK,us semi,6.10,0.85,NA"))
check("...as is a mod half that does not send the field at all",
      "lip" not in ramp_align_phrase("OK,us semi,6.10,0.85"),
      ramp_align_phrase("OK,us semi,6.10,0.85"))
both = ramp_align_phrase("OK,us semi,6.10,0.04,1.30", fmt_ft)
check("the lip is spoken BEFORE the width, because a ramp you cannot climb settles it",
      both.index("not down") < both.index("tight"), both)

check("a failure is spoken verbatim",
      ramp_align_phrase("FAIL,nearest is Old Cannon, 5 metres, no ramp on it")
      == "nearest is Old Cannon, 5 metres, no ramp on it")
check("...and an unknown payload says nothing", ramp_align_phrase("WAT") == "")

print()

print("ramp align sample:", ramp_align_phrase("OK,large cannon,6.10,0.85", fmt_ft))
print("ramp align sample:", ramp_align_phrase("OK,tilt deck 30 ft,6.10,-0.22", fmt_ft))
print()

# ==================================================================================================
#  19. The deck readout: what YOUR OWN ramp machine's ramp is doing
# ==================================================================================================
# F9+I is one key with several answers, and until now none of them was about the machine under
# you. From the seat of a us_semi rollback the key said "Docking instrument is off" -- correct,
# and useless. This is the answer that fills that in.
#
# The two properties worth asserting are the two that produce confident, plausible, WRONG speech:
# a "could not measure" sentinel rendered as a real angle, and a namespace prefix stripped from a
# machine where it was the only descriptive word in the name.

SELF_LEVEL_DEG = _consts["RAMP_SELF_LEVEL_DEG"]
SELF_DIST_STROKE_M = _consts["RAMP_SELF_DISTANCE_STROKE_M"]


def parse_ramp_self(payload):
    payload = (payload or "").strip()
    if not payload or payload.upper() == "NONE":
        return None
    fields = payload.split(";")
    head = fields[0].split(",")
    try:
        pitch = float(head[0])
    except (ValueError, IndexError):
        return None
    try:
        lip = float(head[1])
    except (ValueError, IndexError):
        lip = None
    groups = []
    for chunk in fields[1:]:
        if not chunk:
            continue
        bits = chunk.split(":")
        if len(bits) != 3:
            continue
        try:
            groups.append((bits[0], int(bits[1]), int(bits[2]) / 100.0))
        except ValueError:
            continue
    return {
        "pitch": None if pitch < -180.0 else pitch,
        "lip": None if (lip is None or lip < -900.0) else lip,
        "groups": groups,
    }


def ramp_hyd_words(name):
    out = []
    for i, ch in enumerate(name):
        if ch == "_":
            out.append(" ")
        elif ch.isupper() and i > 0 and not name[i - 1].isupper() and name[i - 1] != "_":
            out.append(" ")
            out.append(ch.lower())
        else:
            out.append(ch.lower())
    return " ".join("".join(out).split())


def ramp_hyd_strip_namespace(names, require_two=True):
    if require_two and len(names) < 2:
        return names
    heads = {n.split("_")[0] for n in names}
    if len(heads) != 1:
        return names
    head = next(iter(heads))
    stripped = [n[len(head) + 1:] for n in names]
    if any(not s for s in stripped):
        return names
    return stripped


def ramp_self_phrase(state, fmt_fn=None, opts=None):
    fmt_fn = fmt_fn or fmt
    opts = opts or {}
    bits = []
    pitch = state.get("pitch")
    if pitch is None:
        bits.append("Ramp angle unavailable")
    elif abs(pitch) < SELF_LEVEL_DEG:
        bits.append("Ramp level")
    else:
        bits.append("Ramp {} {:.0f} degrees".format("up" if pitch > 0 else "down", abs(pitch)))
    lip = state.get("lip")
    if lip is not None:
        if lip < ALIGN_LIP_SAY_M:
            bits.append("Lip on the ground")
        else:
            lv, lu = fmt_fn(lip)
            bits.append("Lip {} {} up".format(lv, lu))
    groups = state.get("groups") or []
    labels = ramp_hyd_strip_namespace(
        [g[0] for g in groups], require_two=not opts.get("stripAlways"))
    for (_n, pct, stroke), label in zip(groups, labels):
        words = ramp_hyd_words(label)
        if stroke >= SELF_DIST_STROKE_M:
            out_v, out_u = fmt_fn(stroke * pct / 100.0)
            full_v, _ = fmt_fn(stroke)
            bits.append("{} {} of {} {}".format(words, out_v, full_v, out_u))
        else:
            bits.append("{} {} percent".format(words, pct))
    return ". ".join(bits)


# The exact wire line the mod builds for a us_semi tc82s_rollback sitting level and fully home,
# read out of the running game: three hydraulic groups, strokes 0.52 m, 5.52 m and 0.22 m.
HOME = "0.0,1.30;upfit_tilt:1:52;upfit_extendRetract:0:552;upfit_extendRetractFeet:0:22"
# ...and at 10.2 degrees of tilt with the bed run out to 88 percent, the pose the readout is for.
WORKING = "10.1,0.15;upfit_tilt:100:52;upfit_extendRetract:100:552;upfit_extendRetractFeet:0:22"

home = ramp_self_phrase(parse_ramp_self(HOME), fmt_ft)
work = ramp_self_phrase(parse_ramp_self(WORKING), fmt_ft)

check("a level deck says so rather than reading out a fraction of a degree",
      home.startswith("Ramp level"), home)
check("a raised deck gives the angle and the direction",
      work.startswith("Ramp up 10 degrees"), work)

# The three real poses, measured in game. The lip is the only one of the three figures that
# answers "can a car get onto this", and it needs BOTH controls: tilt alone moves it 1.30 -> 0.95.
check("a deployed ramp says the lip is down", "Lip on the ground" in work, work)
check("a stowed deck gives the height instead", "Lip 4.3 feet up" in home, home)
TILT_ONLY = "10.2,0.95;upfit_tilt:100:52;upfit_extendRetract:0:552"
tilt_only = ramp_self_phrase(parse_ramp_self(TILT_ONLY), fmt_ft)
check("...and tilting WITHOUT running the bed out is still not down",
      "Lip 3.1 feet up" in tilt_only, tilt_only)
check("an unmeasured lip is silent, never a zero reading as 'on the ground'",
      "Lip" not in ramp_self_phrase(parse_ramp_self("0.0,-999.00;upfit_tilt:1:52")),
      ramp_self_phrase(parse_ramp_self("0.0,-999.00;upfit_tilt:1:52")))
check("...as is a mod half that sends no lip field at all",
      "Lip" not in ramp_self_phrase(parse_ramp_self("0.0;upfit_tilt:1:52")),
      ramp_self_phrase(parse_ramp_self("0.0;upfit_tilt:1:52")))

check("a long stroke reads as a DISTANCE, in the configured unit",
      "extend retract 18.1 of 18.1 feet" in work, work)
check("...and a short one keeps the percentage, because '1 of 2 feet' is worse than 100 percent",
      "tilt 100 percent" in work, work)
check("a fully home deck reads zero rather than going silent",
      "extend retract 0.0 of 18.1 feet" in home, home)

# The shared namespace. Six syllables of "upfit" across three groups distinguish nothing.
check("a prefix shared by every group is dropped", "upfit" not in work, work)
check("...but a lone group keeps its whole name, because one name cannot establish a namespace",
      ramp_self_phrase(parse_ramp_self("0.0;upfit_tilt:34:52"))
      == "Ramp level. upfit tilt 34 percent",
      ramp_self_phrase(parse_ramp_self("0.0;upfit_tilt:34:52")))
check("...and the always-strip form loses the only descriptive word there",
      ramp_self_phrase(parse_ramp_self("0.0;upfit_tilt:34:52"), opts={"stripAlways": True})
      == "Ramp level. tilt 34 percent",
      "if this ever matches the guarded form the guard has stopped doing anything")
check("an unshared prefix is left alone",
      "deck" in ramp_self_phrase(parse_ramp_self("0.0;deck_tilt:5:52;bed_slide:5:52")),
      ramp_self_phrase(parse_ramp_self("0.0;deck_tilt:5:52;bed_slide:5:52")))

# camelCase is the game's own convention for these names, and a screen reader reads the run-on
# form as one nonsense word.
check("camelCase humps become spaces",
      ramp_hyd_words("extendRetractFeet") == "extend retract feet",
      ramp_hyd_words("extendRetractFeet"))

# The sentinel. -999 is what the mod sends when mouthFrame could not be read, and rendering it
# is the one failure here a listener cannot catch.
sentinel = ramp_self_phrase(parse_ramp_self("-999.0;upfit_tilt:1:52;upfit_extendRetract:0:552"))
check("an unmeasurable angle NAMES itself", sentinel.startswith("Ramp angle unavailable"), sentinel)
check("...and is never spoken as a number", "999" not in sentinel, sentinel)
check("...while the rams it does have still read out", "extend retract" in sentinel, sentinel)
check("a real steep angle is NOT mistaken for the sentinel",
      ramp_self_phrase(parse_ramp_self("-25.0;")) == "Ramp down 25 degrees",
      ramp_self_phrase(parse_ramp_self("-25.0;")))

# NONE is what a car gets, and it must clear rather than persist: climbing out of the hauler and
# into a hatchback with a stale deck reading is the readout describing a machine you left behind.
check("NONE is not a state", parse_ramp_self("NONE") is None)
check("an empty line is not a state", parse_ramp_self("") is None)
check("garbage is dropped rather than half-decoded", parse_ramp_self("wat;x:y:z") is None)
check("a ramp with no hydraulics at all still reports its pitch",
      ramp_self_phrase(parse_ramp_self("12.0;")) == "Ramp up 12 degrees",
      ramp_self_phrase(parse_ramp_self("12.0;")))
check("a malformed group is skipped without taking the good ones with it",
      ramp_self_phrase(parse_ramp_self("0.0;broken;upfit_tilt:7:52"))
      == "Ramp level. upfit tilt 7 percent",
      ramp_self_phrase(parse_ramp_self("0.0;broken;upfit_tilt:7:52")))

print()
print("deck sample (home):   ", home)
print("deck sample (working):", work)
print()

if failures:
    print(f"{len(failures)} FAILURE(S): " + ", ".join(failures))
    sys.exit(1)
print("all checks passed")
