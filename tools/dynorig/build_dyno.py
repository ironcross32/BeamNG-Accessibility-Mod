"""Rolling Road (Prototype) -- a drive-on chassis dyno for BeamNG.drive.

WHY THIS EXISTS
---------------
The stock `testroller` rigs are NOT rolling roads and cannot be made into one by
configuration.  Every roller node in them carries `{"frictionCoef":0.0}`, so a
tyre resting in one can exert no longitudinal force at all: the wheel free-revs,
the engine does no work, and TILTING SUCH A RIG CHANGES NOTHING, because there is
nothing for gravity to act against.  Our own notes already record the symptom --
"a free rev at an indicated speed, not the sound of an engine pulling".

What produces load is a DRUM: a rigid body, free to spin, that the tyre drives
through friction.  BeamNG has exactly one mechanism for that -- `rotators` --
and `large_hamster_wheel_barrel.jbeam` is the proof it works when driven by a
car rather than by an engine:

    "rotators":[ ... ["barrel", ["hamster_wheel_speedo"], "center_rr", "center_ll", "f33r", 1] ],
    "powertrain":[ ["shaft","hamster_hub","",1,{"connectedWheel":"barrel","friction":5000}] ],

`inputName` is the EMPTY STRING -- no engine anywhere in the tree.  The drum is
spun by the car and resisted by the shaft's `friction`, and `powertrain/shaft.lua`
also honours `dynamicFriction`, which is drag PROPORTIONAL TO SPEED.  Those two
terms are a road-load curve: a constant rolling-resistance term plus a linear
term standing in for aerodynamic drag.  Add the drum's own rotational inertia and
this is an inertial dyno -- the car accelerates a real mass against a real drag,
which is where the sound of an engine working comes from.

THIS IS THE PROTOTYPE, and it is deliberately the smallest thing that can answer
the one question the full rig rests on: does a car-driven rotator behave at road
speed, and does the V hold the car.  So there is NO incline, NO auto-sizing to
the car, and NO beamtel integration here.  Two scope decisions worth stating:

  * ONE axle is on drums; the other rides a flat deck.  That is how a 2WD chassis
    dyno works, and it removes the wheelbase problem entirely -- the non-driven
    axle sits wherever it lands on a 7.2 m deck, so no config has to match your
    car.  AWD is NOT handled: one axle would be held by the deck while the other
    spun, which tells us nothing about the drums.
  * The deck is symmetric about the V with a ramp at each end, so front- and
    rear-drive are both covered by driving on to different depths.  Front-drive
    stops early; rear-drive drives further in.

GEOMETRY NOTES WORTH KEEPING
----------------------------
* CONTAINMENT IS THE V, NOT A WALL.  Two drums `V_SPACING` apart at `DRUM_R`
  radius cradle the tyre BELOW the deck plane, and the depth is self-adjusting
  across wheel sizes because it falls out of the circle geometry:

      depth = R + r - sqrt((R + r)^2 - (V_SPACING / 2)^2)

  r = 0.25 -> 100 mm, r = 0.32 -> 85 mm, r = 0.45 -> 68 mm.  A wall would have to
  be sized for one wheel; the V sizes itself, and it cannot be climbed over
  because the surface it would be climbed on is spinning away underneath.

* THE DECK SITS AT DRUM-TOP HEIGHT, not at the height a cradled wheel ends up.
  Dropping the deck by the cradle depth would make the car sit level and would
  turn the roll-in into an 85 mm STEP up onto the drum crown.  A smooth roll-in
  is worth more than a level attitude, so the car sits slightly down at the
  driven end and that is intended, not an oversight.

* THE DRUM SPANS THE FULL WIDTH (2.30 m), which is what makes track width
  irrelevant -- the one dimension the stock tilt ramp cannot adjust at all
  (`testroller_tiltramp*.jbeam` has six `wheelbaseHydro` beams and ZERO
  `trackwidthHydro`).  It also costs fewer nodes than per-wheel drums would.

* A CYLINDER IS DEVELOPABLE ALONG ITS AXIS, so ring spacing along X costs no
  accuracy at all -- triangles spanning two rings 1.5 m apart lie exactly on the
  surface.  Only the ANGULAR count matters, and at R = 0.25 the sagitta of a
  20-gon is 0.25 * (1 - cos 9deg) = 3 mm, which a tyre cannot feel.  Hence 20
  segments and only 4 rings: the rings spread collision along the drum, they do
  not round it off.

* TRIANGLES, NOT NODES, ARE THE SURFACE.  Vehicle-to-vehicle contact in BeamNG is
  node-against-triangle, so a drum built only of nodes and beams is one a car
  falls straight through.  The triangles also carry `groundModel`, which is what
  picks the tyre sound -- ASPHALT here, so the rig sounds like road.  Silence
  would read as the drums not being there (docs/level-generation.md: only 14 of
  the game's 32 ground models make any tyre sound at all).

* THE FRAME IS `fixed:true`, welded to the world.  A rig a car can shove is a rig
  that walks out from under the car.  Only the drum group nodes are free, because
  those are the ones the rotator rotates.
"""

import json
import math
import os
import shutil

# ---------------------------------------------------------------- dimensions

DRUM_R = 0.25          # drum radius (m)
DRUM_SEGS = 20         # angular segments; 3 mm sagitta at this radius
DRUM_RING_X = [-1.15, 0.0, 1.15]
DRUM_AXLE_X = [-1.25, -1.15, 0.0, 1.15, 1.25]
MARKER_COUNT = 4      # nodes handed to the rotator; see drum() -- NOT the whole rim

# BEAM STIFFNESS IS BOUNDED BY THE INTEGRATOR, and the bound is what killed the
# first build ("Multiple instabilities detected in 'rollingroad'").  BeamNG steps
# physics at 2000 Hz, and explicit integration is stable only while
# sqrt(k_total / m) < 2 / dt -- so per node,
#
#     (sum of beamSpring over its beams) / nodeWeight  <  (2 / 0.0005)^2 = 1.6e7
#
# The first drum was 25e6 N/m on 4 kg nodes with about seven beams each, i.e.
# 4.4e7: 2.7x over, and the vehicle was deleted on spawn.  For comparison the
# hamster wheel barrel -- the thing this drum is modelled on -- runs 8e6 N/m on
# 75 kg nodes and sits at about 2.1e6, a factor of 7.5 under.  These numbers are
# chosen to sit two orders under the limit, and `dyno_sim.py` scenario 12 now
# asserts it rather than leaving it to be rediscovered.
DRUM_BEAM_SPRING = 800000.0
DRUM_BEAM_DAMP = 500.0
FRAME_BEAM_SPRING = 4000000.0
FRAME_BEAM_DAMP = 500.0

# DRUM SPACING IS SET BY TYRE GRIP, NOT BY HOW DEEP THE DIP FEELS.  A wheel
# escapes the cradle by rolling up the exit drum, which needs a horizontal force
# of W_axle * tan(theta), where theta = asin((S/2) / (R + r)) is the contact
# angle.  The tyre can supply up to mu * W_axle, and mu is about 1.0 on asphalt,
# so the cradle only holds when tan(theta) > 1, i.e. theta > 45 degrees.
#
# Measured in game at S = 0.60: theta is 32.9 degrees on a 0.303 m wheel, so
# escape costs 0.65 * axle load -- far less than the car makes.  Placed
# stationary with its rear axle in the V, an autobello at 40% throttle climbed
# straight out and drove off the rig, exactly as reported from the seat, while
# the drums turned only 26 and 78 degrees.  The drums were never the problem.
#
#   S      r=0.25        r=0.303       r=0.45      min wheel r
#   0.60   36.9d t=0.75  32.9d t=0.65  25.4d t=0.47   0.050
#   0.82   55.1d t=1.43  47.9d t=1.10  35.9d t=0.72   0.160
#
# 0.82 clears tan(theta) = 1 for ordinary car wheels.  A truck wheel (r = 0.45)
# still sits at 0.72 and can climb out under full power -- the geometry cannot
# satisfy both ends of the range at once, because theta falls as the wheel grows
# while the fall-through limit (S < 2(R + r)) rises with it.  Cars are the case
# this rig is for.
V_SPACING = 0.82       # centre-to-centre of the two drums forming the V
AXIS_Z = 0.30          # drum axis height, so the drum tops land on the deck
DECK_Z = AXIS_Z + DRUM_R
DECK_LIP_GAP = 0.08    # how far past the drum crown the deck stops; see build_parts

DECK_HALF_W = 1.30
DECK_Y = 3.60          # deck runs +/- this; long enough for any wheelbase
KERB_Z = DECK_Z + 0.20

RAMP_RUN = 2.50        # 0.55 m rise over 2.5 m -> 12.4 degrees

DECK_X = [-1.30, -0.65, 0.0, 0.65, 1.30]

# Road load.  Force at the tyre is torque / DRUM_R, and BOTH drums are driven by
# the same tyre, so each carries half.  Targeting ~200 N of rolling resistance
# and ~600 N total at 30 m/s gives, per drum:
#     friction        = 200 N * DRUM_R / 2           = 25 Nm
#     dynamicFriction = (400 N / 30) * DRUM_R^2 / 2  = 0.42 Nm per rad/s
SHAFT_FRICTION = 25.0
SHAFT_DYN_FRICTION = 0.42

# Drum mass sets the INERTIA, which is the thing the engine actually accelerates,
# and for a thin shell the equivalent translating mass at the tyre is just the
# drum's mass (I / r^2 = m).  60 nodes at 6.5 kg is ~390 kg per drum, ~780 kg for
# the pair -- a small car's worth of mass to get moving, and the whole of it,
# since the car itself is not going anywhere.  Mass is also the DENOMINATOR of
# the stability ratio above, so making the drum lighter is not free.
DRUM_NODE_WEIGHT = 6.5

# The free-axle variant's axle nodes.  Heavy, because they carry every spoke.
AXLE_NODE_WEIGHT = 60.0

MODEL = "rollingroad"
MOD_NAME = "beam_rolling_road"
MODS_UNPACKED = os.path.join(
    os.environ.get("LOCALAPPDATA", ""),
    "BeamNG", "BeamNG.drive", "current", "mods", "unpacked",
)


def cradle_depth(wheel_r):
    """How far below the deck plane a tyre of this radius settles in the V."""
    a = DRUM_R + wheel_r
    return a - math.sqrt(a * a - (V_SPACING / 2.0) ** 2)


# ------------------------------------------------------------------ emitters

class Part(object):
    def __init__(self):
        self.nodes = []      # (id, x, y, z, props|None)
        self.beams = []      # (id1, id2)
        self.tris = []       # (id1, id2, id3)

    def node(self, nid, x, y, z, props=None):
        self.nodes.append((nid, round(x, 5), round(y, 5), round(z, 5), props))
        return nid

    def beam(self, a, b):
        self.beams.append((a, b))

    def quad(self, a, b, c, d):
        self.tris.append((a, b, c))
        self.tris.append((a, c, d))


def grid(part, name, xs, ys, zf, props=None):
    """A rectangular lattice of nodes; returns ids[iy][ix]."""
    ids = []
    for iy, y in enumerate(ys):
        row = []
        for ix, x in enumerate(xs):
            nid = "%s_%d_%d" % (name, iy, ix)
            part.node(nid, x, y, zf(x, y), props)
            row.append(nid)
        ids.append(row)
    for iy, row in enumerate(ids):
        for ix, nid in enumerate(row):
            if ix + 1 < len(row):
                part.beam(nid, row[ix + 1])
            if iy + 1 < len(ids):
                part.beam(nid, ids[iy + 1][ix])
                if ix + 1 < len(row):        # one diagonal per cell, for shear
                    part.beam(nid, ids[iy + 1][ix + 1])
    return ids


def surface(part, ids):
    """Triangulate a lattice, facing UP.

    Winding is not a detail here: BeamNG collision triangles are ONE-SIDED, so a
    deck wound the other way is a deck a car falls through, and nothing in the
    file would look wrong.  The convention was settled against shipped data
    rather than reasoned about -- `blank_RR` in `testroller_tiltramp_blanks.jbeam`
    is a flat plate you park on, and both of its triangles compute a +Z normal
    with x increasing across the winding and y increasing along it.
    """
    for iy in range(len(ids) - 1):
        for ix in range(len(ids[0]) - 1):
            a = ids[iy][ix]
            b = ids[iy][ix + 1]
            c = ids[iy + 1][ix + 1]
            d = ids[iy + 1][ix]
            part.quad(a, b, c, d)


def drum(part, axles, name, y_centre, tie_to):
    """One free-spinning drum: rings of nodes about an X axis, skinned.

    THE RIM MUST BE BEAMED TO THE AXLE.  The first build connected the rings only
    to each other, which is a shell attached to nothing -- a rotator does not
    hold its group in place, it applies torque to a body the beams hold.  Every
    rim node therefore gets three spokes: a radial one to the axle node at its
    own ring, and one to each end of the axle.  Two axle points is what turns a
    sphere of possible positions into the CIRCLE the node is meant to travel on,
    and the third is stiffness.

    Rotation is still free, and that is not a coincidence to be checked by
    experiment: a rotation about the axle preserves every one of these lengths --
    spokes to points on the axis, and hoops within a ring -- so the beam network
    resists the drum's one intended degree of freedom not at all.  It also means
    no beam changes length while spinning, so `beamDamp` never fights the drum
    either.
    """
    axle = {}
    for x in DRUM_AXLE_X:
        nid = "%s_ax%s" % (name, ("%+.2f" % x).replace(".", "").replace("+", "p")
                           .replace("-", "m"))
        axles.node(nid, x, y_centre, AXIS_Z,
                   {"collision": False, "staticCollision": False})
        axle[x] = nid
    for i in range(len(DRUM_AXLE_X) - 1):
        axles.beam(axle[DRUM_AXLE_X[i]], axle[DRUM_AXLE_X[i + 1]])
    # Bearing ties to the welded deck.  Inert in the fixed-axle variant (both
    # ends are `fixed`, so nothing moves) and load-bearing in the free-axle one,
    # where they are the only thing holding the axle up.  The lip rows sit above
    # the axle and on both sides of it, so the tie set is not coplanar with the
    # node it locates -- three anchors in one plane through the node would leave
    # it floppy out of that plane, which is the degenerate case worth avoiding by
    # construction rather than by tuning a spring afterwards.
    for nid in axle.values():
        for anchor in tie_to:
            axles.beam(nid, anchor)

    markers = []
    rings = []
    for ir, x in enumerate(DRUM_RING_X):
        ring = []
        for s in range(DRUM_SEGS):
            th = 2.0 * math.pi * s / DRUM_SEGS
            nid = "%s_%d_%d" % (name, ir, s)
            part.node(nid, x,
                      y_centre + DRUM_R * math.sin(th),
                      AXIS_Z + DRUM_R * math.cos(th))
            ring.append(nid)
        rings.append(ring)

    # THE ROTATOR'S GROUP IS A HANDFUL OF MARKERS, NOT THE BODY.  This is the
    # single most expensive thing learned here, and it is invisible in the jbeam
    # unless you count: `large_hamster_wheel_barrel.jbeam` has 485 nodes, and the
    # group its `rotators` entry names holds exactly FOUR of them --
    # wheelOut_0_m, _8_m, _16_m and _24_m, evenly spaced round the rim and each
    # declared into two groups at once.  The barrel is not turned by the rotator;
    # it turns by ordinary beam physics on its centre-rail axle, and the rotator
    # only MEASURES that rotation and couples the brake and the powertrain to it.
    #
    # Handing the rotator the whole rim -- the obvious reading of "[group]:" --
    # puts those nodes under the wheel system, which then holds them at the
    # angular velocity IT integrates.  With no engine driving them and no ground
    # under them that velocity is zero, so the drum is welded shut while every
    # other part of the file looks right.  Shipped and confirmed twice: the drum
    # would not turn with a welded axle or with a free one, because the axle was
    # never what held it.
    marker_ring = len(rings) // 2
    for k in range(MARKER_COUNT):
        s = (k * DRUM_SEGS) // MARKER_COUNT
        markers.append(rings[marker_ring][s])

    ends = (DRUM_AXLE_X[0], DRUM_AXLE_X[-1])
    for ir, ring in enumerate(rings):
        for s in range(DRUM_SEGS):
            part.beam(ring[s], ring[(s + 1) % DRUM_SEGS])          # hoop
            part.beam(ring[s], axle[DRUM_RING_X[ir]])              # radial spoke
            for x in ends:
                part.beam(ring[s], axle[x])                        # end spokes
    for ir in range(len(rings) - 1):
        for s in range(DRUM_SEGS):
            part.beam(rings[ir][s], rings[ir + 1][s])              # longeron
            part.beam(rings[ir][s], rings[ir + 1][(s + 1) % DRUM_SEGS])

    # Skin, facing OUTWARD.  Same one-sided rule as the deck: wound the other
    # way the tyre drops straight through the drum it is supposed to ride on.
    for ir in range(len(rings) - 1):
        for s in range(DRUM_SEGS):
            s2 = (s + 1) % DRUM_SEGS
            part.quad(rings[ir][s], rings[ir + 1][s],
                      rings[ir + 1][s2], rings[ir][s2])
    return rings, axle, markers


# ---------------------------------------------------------------- jbeam text

def fmt_node(n):
    nid, x, y, z, props = n
    base = '["%s", %.5f, %.5f, %.5f' % (nid, x, y, z)
    if props:
        return base + ", " + json.dumps(props) + "],"
    return base + "],"


def build_parts(fixed_axle=True):
    frame = Part()
    axles = Part()
    drums = Part()

    # Reference nodes.  `back:` at +Y and `left:` at +X is the mod-wide reading
    # of BeamNG's axes (docs/lua-geometry.md), which makes the prop's forward -Y,
    # so a car driving on from the +Y ramp travels the prop's own forward.
    frame.node("ref", 0.0, 0.0, DECK_Z + 0.60, {"collision": False})
    frame.node("refB", 0.0, 0.20, DECK_Z + 0.60, {"collision": False})
    frame.node("refL", 0.20, 0.0, DECK_Z + 0.60, {"collision": False})
    frame.node("refU", 0.0, 0.0, DECK_Z + 0.80, {"collision": False})
    for a, b in (("ref", "refB"), ("ref", "refL"), ("ref", "refU"),
                 ("refB", "refL"), ("refL", "refU"), ("refU", "refB")):
        frame.beam(a, b)

    # Deck, in two panels either side of the V.
    #
    # WHERE THE PANEL ENDS IS THE ROLL-IN.  The obvious lip is where the deck is
    # tangent to the drum -- V_SPACING/2 + DRUM_R -- and it is wrong, because a
    # cylinder is tangent to a horizontal plane at its CROWN and tangent to a
    # vertical one at its side.  Ending there leaves the deck edge 0.25 m
    # directly above the drum's flank with the crown a further 0.25 m inboard:
    # a ledge, not a ramp, and a small wheel can catch its edge.
    #
    # The panel therefore runs almost to the crown (`DECK_LIP_GAP` past it), where
    # the drum surface is 13 mm below the deck plane and the two are effectively
    # flush.  The drum's outer flank tucks UNDER the deck lip, which is what a
    # real dyno looks like, and the wheel rolls deck -> crown -> cradle with
    # nothing to trip on.  Checked against the extremes: a 0.32 m wheel sits
    # wholly inside the V and never reaches the lip at all, and a 0.45 m one
    # passes 88 mm above it.
    lip = V_SPACING / 2.0 + DECK_LIP_GAP
    panels = []
    for tag, ys in (("dkF", [-DECK_Y, -2.85, -2.10, -1.35, -lip]),
                    ("dkR", [lip, 1.35, 2.10, 2.85, DECK_Y])):
        ids = grid(frame, tag, DECK_X, ys, lambda x, y: DECK_Z)
        surface(frame, ids)
        panels.append((ids, ys))

    # Kerbs.  A car on drums wanders laterally; this is the guide that keeps it
    # on.  Deck only -- a kerb across a ramp is a kerb you drive into.
    for pi, (ids, ys) in enumerate(panels):
        for side, x in (("r", -DECK_HALF_W), ("l", +DECK_HALF_W)):
            top = []
            for iy in range(len(ys)):
                nid = "kerb%d%s_%d" % (pi, side, iy)
                frame.node(nid, x, ys[iy], KERB_Z)
                top.append(nid)
            ix = 0 if side == "r" else len(DECK_X) - 1
            for iy in range(len(ys)):
                frame.beam(top[iy], ids[iy][ix])
                if iy + 1 < len(ys):
                    frame.beam(top[iy], top[iy + 1])
                    # Kerb faces INWARD, at the wheel -- the -X kerb needs a +X
                    # normal and vice versa, so the two sides are wound
                    # oppositely.  Facing them outward would leave the guide
                    # that keeps the car on the drums invisible to the car.
                    lo_a, lo_b = ids[iy][ix], ids[iy + 1][ix]
                    if side == "r":
                        frame.quad(lo_a, lo_b, top[iy + 1], top[iy])
                    else:
                        frame.quad(lo_a, top[iy], top[iy + 1], lo_b)

    # Ramps at both ends.
    #
    # ROWS MUST ASCEND IN Y.  `surface` derives its facing from the lattice
    # order, so a panel whose rows run the other way comes out wound backwards
    # and therefore one-sided the wrong way -- which for the FRONT ramp, built
    # from the deck edge outward, is exactly what happened: the deck was right,
    # the rear ramp was right, and the front ramp alone was a surface you fall
    # through.  Sorting is the fix rather than a per-panel sign, because the sign
    # is invisible at the call site and the sort cannot be got wrong.
    for tag, y0, sgn in (("rpR", DECK_Y, +1.0), ("rpF", -DECK_Y, -1.0)):
        ys = sorted(y0 + sgn * RAMP_RUN * t
                    for t in (0.0, 0.25, 0.5, 0.75, 1.0))

        def zf(x, y, y0=y0):
            return DECK_Z * (1.0 - abs(y - y0) / RAMP_RUN)

        ids = grid(frame, tag, DECK_X, ys, zf)
        surface(frame, ids)

    # Drums last, because their bearing ties anchor to the deck lip rows -- the
    # row nearest the V on each panel, which straddle the axle in Y and span it
    # in X.
    lip_ids = [panels[0][0][-1][ix] for ix in range(len(DECK_X))]
    lip_ids += [panels[1][0][0][ix] for ix in range(len(DECK_X))]

    axis_nodes = {}
    markers = {}
    for tag, yc in (("F", -V_SPACING / 2.0), ("R", +V_SPACING / 2.0)):
        _, axle, marks = drum(drums, axles, "drum%s" % tag, yc, lip_ids)
        axis_nodes[tag] = (axle[DRUM_AXLE_X[0]], axle[DRUM_AXLE_X[-1]])
        markers[tag] = marks

    return frame, axles, drums, axis_nodes, markers


def build_part(part_name, label, use_rotator):
    frame, axles, drums, axis_nodes, markers = build_parts()

    out = []
    w = out.append
    w('"%s": {' % part_name)
    w('    "information":{')
    w('        "name":"%s",' % label)
    w('        "authors":"BEAM",')
    w('        "value":0,')
    w('    },')
    w('    "slotType":"main",')
    w('    "refNodes":[')
    w('        ["ref:", "back:", "left:", "up:"],')
    w('        ["ref", "refB", "refL", "refU"],')
    w('    ],')
    w('    "cameraExternal":{')
    w('        "distance":12,')
    w('        "distanceMin":2,')
    w('        "offset":{"x":0.0, "y":0.0, "z":0.5},')
    w('        "fov":65,')
    w('    },')
    # Free drums.  brakeTorque and parkingTorque are 0 and useDefaultBrakeInput
    # is false ON PURPOSE: a prop nobody is sitting in can still have a parking
    # brake asserted, and a dyno whose drums are held reads from the seat as a
    # wall.  All load is meant to come from the shaft, where it can be tuned.
    # THE DRUM TURNS BY BEAM PHYSICS, NOT BECAUSE OF THIS SECTION.  The rotator
    # is handed only the four MARKER nodes (see drum()), so it measures the
    # rotation, carries the brake, and couples the powertrain's road-load terms
    # to it -- the job it does on the hamster barrel, where it likewise names 4
    # nodes out of 485.  Give it the whole rim and the wheel system owns those
    # nodes instead, holding them at an angular velocity nothing drives.
    #
    # The no-rotator variant exists to prove that division: it is the same drum
    # with these two sections deleted, so it can only spin by beam physics.  If
    # both turn, the rotator is doing what it should; if only the bare one does,
    # even four marker nodes are too many and the load has to come from Lua.
    if use_rotator:
        w('    "rotators":[')
        w('        ["name","[group]:","node1:","node2:","nodeArm:","wheelDir"],')
        w('        {"radius":%.3f},' % DRUM_R)
        w('        {"brakeTorque":0, "brakeSpring":100, "parkingTorque":0},')
        w('        {"useDefaultBrakeInput":false},')
        for tag in ("F", "R"):
            a, b = axis_nodes[tag]
            extra = ', {"speedo":true}' if tag == "R" else ''
            w('        ["drum%s", ["drum%s_spd"], "%s", "%s", "ref", 1%s],'
              % (tag, tag, a, b, extra))
        w('    ],')
        # inputName "" -- no engine in the tree at all, exactly as the hamster
        # barrel does it.  `friction` is the constant road-load term and
        # `dynamicFriction` the speed-proportional one (powertrain/shaft.lua).
        w('    "powertrain":[')
        w('        ["type", "name", "inputName", "inputIndex"],')
        for tag in ("F", "R"):
            w('        ["shaft", "hub%s", "", 1, {"connectedWheel":"drum%s", '
              '"friction":%.2f, "dynamicFriction":%.2f, "uiName":"Drum %s"}],'
              % (tag, tag, SHAFT_FRICTION, SHAFT_DYN_FRICTION, tag))
        w('    ],')

    w('    "nodes":[')
    w('        ["id", "posX", "posY", "posZ"],')
    w('        {"frictionCoef":0.9},')
    w('        {"nodeMaterial":"|NM_METAL"},')
    w('        {"selfCollision":false},')
    w('        {"collision":true},')
    w('        {"nodeWeight":25},')
    w('        {"group":"frame"},')
    w('        {"fixed":true},')
    for n in frame.nodes:
        w('        ' + fmt_node(n))
    # THE AXLE IS THE VARIABLE THIS BUILD EXISTS TO TEST.  No shipped rotator has
    # a `fixed` axis node -- the hamster barrel's centre nodes are held by its
    # frame's bearing beams, and the square wheel's hub nodes hang off the
    # suspension -- so a welded axis is the one thing here with no precedent, and
    # the first thing to suspect when a drum does not turn.
    # The axle stays welded.  The free-axle variant was built and driven and
    # behaved identically, which is what ruled the axle out and sent the search
    # to the rotator group instead -- so there is no case for carrying two.
    w('        {"group":"axle"},')
    w('        {"nodeWeight":50},')
    for n in axles.nodes:
        w('        ' + fmt_node(n))
    w('        {"fixed":false},')
    w('        {"nodeWeight":%.1f},' % DRUM_NODE_WEIGHT)
    w('        {"nodeMaterial":"|NM_RUBBER"},')
    w('        {"staticCollision":false},')
    for tag in ("F", "R"):
        w('        {"group":"drum%s"},' % tag)
        marks = set(markers[tag])
        for n in drums.nodes:
            if n[0].startswith("drum%s_" % tag):
                if n[0] in marks:
                    # Two groups at once, exactly as the hamster barrel's four
                    # marker nodes are declared.  The node stays part of the
                    # drum body; it is additionally visible to the rotator.
                    nid, x, y, z, _ = n
                    n = (nid, x, y, z,
                         {"group": ["drum%s" % tag, "drum%s_spd" % tag]})
                w('        ' + fmt_node(n))
    w('        {"group":""},')
    w('    ],')

    # Two spring rates, because the two structures have different stability
    # budgets.  The frame is entirely `fixed:true` -- both ends of every one of
    # its beams are welded to the world, so nothing there can move and its
    # stiffness is free.  The drum is the part that has to satisfy
    # k_total / m < 1.6e7 per node (see DRUM_BEAM_SPRING).
    w('    "beams":[')
    w('        ["id1:", "id2:"],')
    w('        {"beamDeform":"FLT_MAX", "beamStrength":"FLT_MAX"},')
    w('        {"beamSpring":%d, "beamDamp":%d},'
      % (FRAME_BEAM_SPRING, FRAME_BEAM_DAMP))
    for a, b in frame.beams + axles.beams:
        w('        ["%s", "%s"],' % (a, b))
    w('        {"beamSpring":%d, "beamDamp":%d},'
      % (DRUM_BEAM_SPRING, DRUM_BEAM_DAMP))
    for a, b in drums.beams:
        w('        ["%s", "%s"],' % (a, b))
    w('    ],')

    # groundModel is what selects the tyre sound; ASPHALT so the rig sounds like
    # road under the wheels rather than like nothing at all.
    w('    "triangles":[')
    w('        ["id1:","id2:","id3:"],')
    w('        {"groundModel":"asphalt"},')
    w('        {"dragCoef":0},')
    for t in frame.tris + drums.tris:
        w('        ["%s", "%s", "%s"],' % t)
    w('    ],')
    w('},')
    return "\n".join(out)


def build_jbeam():
    """Both variants in one file.  Only one part is ever loaded at a time, so the
    node names may repeat between them."""
    return ("{\n"
            + build_part(MODEL, "Rolling Road (Prototype)", True) + "\n"
            + build_part(MODEL + "_bare",
                         "Rolling Road (Prototype, no rotator)", False) + "\n"
            + "}\n")


# ---------------------------------------------------------------- packaging

def write_mod(root):
    veh = os.path.join(root, "vehicles", MODEL)
    if os.path.isdir(root):
        shutil.rmtree(root)
    os.makedirs(veh)

    with open(os.path.join(veh, "%s.jbeam" % MODEL), "w") as f:
        f.write(build_jbeam())

    with open(os.path.join(veh, "info.json"), "w") as f:
        json.dump({
            "Name": "Rolling Road (Prototype)",
            "Author": "BEAM",
            "Type": "Prop",
            "default_pc": "proto",
        }, f, indent=2)

    for cfg, part, desc in (
        ("proto", MODEL,
         "Single-axle rolling road. Drive on from either ramp until the "
         "driven axle drops into the cradle."),
        ("bare", MODEL + "_bare",
         "As Prototype, but with no rotator or powertrain at all, so the drum "
         "can only turn by beam physics. Inertia is the only load."),
    ):
        with open(os.path.join(veh, "%s.pc" % cfg), "w") as f:
            json.dump({"format": 2, "model": MODEL,
                       "mainPartName": part, "mainPartPath": "/" + part,
                       "parts": {"main": part}}, f, indent=2)
        with open(os.path.join(veh, "info_%s.json" % cfg), "w") as f:
            json.dump({
                "Configuration": ("Prototype" if cfg == "proto"
                                  else "Prototype, no rotator"),
                "Description": desc,
                "Value": 0,
                "Weight": 2000,
            }, f, indent=2)
    return veh


def main():
    frame, axles, drums, _, _ = build_parts()
    root = os.path.join(MODS_UNPACKED, MOD_NAME)
    veh = write_mod(root)
    print("wrote %s" % veh)
    print("  frame  %4d nodes  %4d beams  %4d triangles"
          % (len(frame.nodes), len(frame.beams), len(frame.tris)))
    print("  axles  %4d nodes  %4d beams" % (len(axles.nodes), len(axles.beams)))
    print("  drums  %4d nodes  %4d beams  %4d triangles"
          % (len(drums.nodes), len(drums.beams), len(drums.tris)))
    print("  deck top %.2f m, drum top %.2f m, ramp %.1f deg, rig %.1f m long"
          % (DECK_Z, AXIS_Z + DRUM_R,
             math.degrees(math.atan2(DECK_Z, RAMP_RUN)),
             2.0 * (DECK_Y + RAMP_RUN)))
    for r in (0.25, 0.32, 0.45):
        print("  cradle depth for a %.2f m wheel: %3.0f mm"
              % (r, cradle_depth(r) * 1000.0))


if __name__ == "__main__":
    main()
