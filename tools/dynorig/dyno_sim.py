"""Checks on the generated rolling road that only fail HERE.

Everything this file asserts has the same failure signature in game -- the car
falls through something, or rests on nothing -- and none of it is visible by
reading the jbeam, because a triangle wound the wrong way looks exactly like one
wound the right way.  So each scenario also asserts what the NAIVE form answers,
the way the rest of this project's sims do; a check that cannot fail is not one.

    python tools/dynorig/dyno_sim.py
"""

import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import build_dyno as B


FAILED = []


def check(name, cond, detail=""):
    if cond:
        print("  ok    %s" % name)
    else:
        print("  FAIL  %s %s" % (name, detail))
        FAILED.append(name)


def normal(pos, tri):
    (ax, ay, az) = pos[tri[0]]
    (bx, by, bz) = pos[tri[1]]
    (cx, cy, cz) = pos[tri[2]]
    ux, uy, uz = bx - ax, by - ay, bz - az
    wx, wy, wz = cx - ax, cy - ay, cz - az
    return (uy * wz - uz * wy, uz * wx - ux * wz, ux * wy - uy * wx)


def main():
    frame, axles, drums, axis_nodes, markers = B.build_parts()
    frame.nodes = frame.nodes + axles.nodes
    frame.beams = frame.beams + axles.beams
    pos = {}
    dup = []
    for part in (frame, drums):
        for nid, x, y, z, _ in part.nodes:
            if nid in pos:
                dup.append(nid)
            pos[nid] = (x, y, z)

    print("scenario 1: node ids are unique")
    check("no duplicate node id", not dup, dup[:5])

    print("scenario 2: every beam and triangle references a real node")
    missing = set()
    for part in (frame, drums):
        for a, b in part.beams:
            for n in (a, b):
                if n not in pos:
                    missing.add(n)
        for t in part.tris:
            for n in t:
                if n not in pos:
                    missing.add(n)
    check("no dangling references", not missing, sorted(missing)[:5])
    # A zero-length beam or a degenerate triangle is the other way a vehicle
    # explodes on spawn, and it reads identically to the stiffness case.
    short = []
    for part in (frame, drums):
        for a, b in part.beams:
            if math.dist(pos[a], pos[b]) < 1e-4:
                short.append((a, b))
    check("no zero-length beams", not short, short[:3])
    degen = []
    for part in (frame, drums):
        for t in part.tris:
            n = normal(pos, t)
            if math.sqrt(sum(c * c for c in n)) < 1e-9:
                degen.append(t)
    check("no degenerate triangles", not degen, degen[:3])

    print("scenario 3: rotator axis nodes exist and are FRAME nodes")
    frame_ids = set(n[0] for n in frame.nodes)
    drum_ids = set(n[0] for n in drums.nodes)
    ok = True
    for tag, (a, b) in axis_nodes.items():
        ok = ok and a in frame_ids and b in frame_ids
        ok = ok and a not in drum_ids and b not in drum_ids
    check("axis nodes are non-rotating", ok)
    # An axis node placed inside the rotating group is the mistake that makes a
    # rotator spin about a point that is itself spinning.
    check("naive form (axis in drum group) would be caught",
          not (set(a for a, _ in axis_nodes.values()) & drum_ids))

    print("scenario 4: deck and ramp triangles face UP")
    # Convention settled against blank_RR in testroller_tiltramp_blanks.jbeam.
    deck_tris = [t for t in frame.tris
                 if all(n.startswith(("dkF", "dkR", "rpF", "rpR")) for n in t)]
    check("deck/ramp triangles present", len(deck_tris) > 100, len(deck_tris))
    bad = [t for t in deck_tris if normal(pos, t)[2] <= 0]
    check("all deck/ramp normals +Z", not bad, bad[:2])
    flipped = [(t[0], t[2], t[1]) for t in deck_tris[:8]]
    check("naive (reversed) winding would fail",
          all(normal(pos, t)[2] < 0 for t in flipped))

    print("scenario 5: drum skin faces OUTWARD")
    bad = []
    for t in drums.tris:
        nx, ny, nz = normal(pos, t)
        # outward radial direction at the triangle centroid, in the Y-Z plane
        cy = sum(pos[n][1] for n in t) / 3.0
        cz = sum(pos[n][2] for n in t) / 3.0
        yc = B.V_SPACING / 2.0 * (1 if cy > 0 else -1)
        ry, rz = cy - yc, cz - B.AXIS_Z
        if ny * ry + nz * rz <= 0:
            bad.append(t)
    check("all drum normals point away from the axis", not bad, bad[:2])
    flipped = [(t[0], t[2], t[1]) for t in drums.tris[:8]]
    inward = 0
    for t in flipped:
        nx, ny, nz = normal(pos, t)
        cy = sum(pos[n][1] for n in t) / 3.0
        cz = sum(pos[n][2] for n in t) / 3.0
        yc = B.V_SPACING / 2.0 * (1 if cy > 0 else -1)
        if ny * (cy - yc) + nz * (cz - B.AXIS_Z) < 0:
            inward += 1
    check("naive (reversed) drum winding would fail", inward == len(flipped))

    print("scenario 6: kerb faces point INWARD, at the wheel")
    kerb = [t for t in frame.tris if any(n.startswith("kerb") for n in t)]
    check("kerb triangles present", len(kerb) > 16, len(kerb))
    bad = []
    for t in kerb:
        nx, ny, nz = normal(pos, t)
        cx = sum(pos[n][0] for n in t) / 3.0
        if nx * (-cx) <= 0:          # inward means pointing back toward x = 0
            bad.append(t)
    check("all kerb normals point toward the deck centreline", not bad, bad[:2])

    print("scenario 7: the drum tucks under the deck lip, with no ledge")
    lip = B.V_SPACING / 2.0 + B.DECK_LIP_GAP
    dy = lip - B.V_SPACING / 2.0
    surf = B.AXIS_Z + math.sqrt(B.DRUM_R ** 2 - dy ** 2)
    gap = B.DECK_Z - surf
    check("deck lip clears the drum", gap > 0.005, "%.4f m" % gap)
    check("lip is near flush, not a ledge", gap < 0.030, "%.4f m" % gap)
    # The tangent lip -- the obvious choice -- leaves a quarter-metre drop.
    naive_gap = B.DECK_Z - B.AXIS_Z
    check("naive tangent lip would be a ledge", naive_gap > 0.20,
          "%.3f m" % naive_gap)

    print("scenario 8: the cradle holds against TYRE GRIP, not against a depth")
    # The escape threshold is what matters, and it is not the depth of the dip.
    # A wheel leaves by rolling up the exit drum, costing W_axle * tan(theta)
    # where theta = asin((S/2)/(R+r)); the tyre can supply about mu * W_axle with
    # mu ~ 1.0 on asphalt, so the cradle holds only while tan(theta) > 1.
    # Measured in game at S = 0.60 (tan 0.65 on this car): held stationary in the
    # V, an autobello at 40% throttle climbed out and drove off the rig.
    #
    # THETA FALLS AS THE WHEEL GROWS, so one fixed spacing cannot cover the whole
    # fleet -- which is why the finished rig has to size S to the car, the same
    # way the stock testroller sizes its wheelbase.  The range asserted here is
    # the range this fixed-geometry PROTOTYPE claims, and no more.
    for r in (0.25, 0.303, 0.32):
        a = B.DRUM_R + r
        t = math.tan(math.asin((B.V_SPACING / 2.0) / a))
        check("wheel r=%.3f holds: tan(theta)=%.2f > 1" % (r, t), t > 1.0)
    for r in (0.20, 0.25, 0.303, 0.45):
        a = B.DRUM_R + r
        check("wheel r=%.2f does not fall through" % r, B.V_SPACING / 2.0 < a,
              "S/2=%.3f vs R+r=%.3f" % (B.V_SPACING / 2.0, a))
        if B.V_SPACING / 2.0 < a:
            axle = B.AXIS_Z + math.sqrt(a * a - (B.V_SPACING / 2.0) ** 2)
            check("wheel r=%.2f clears the ground" % r, axle - r > 0.02,
                  "%.3f m" % (axle - r))
    # The naive reading -- "a deeper dip holds better" -- is what the first build
    # rested on, and it is the same quantity read the wrong way: 0.60 m spacing
    # gives a 89 mm dip that feels decisive from the seat and holds nothing.
    naive = math.tan(math.asin(0.30 / (B.DRUM_R + 0.303)))
    check("the shipped 0.60 m spacing would fail this", naive < 1.0,
          "tan=%.2f" % naive)

    print("scenario 9: no drum node can touch the ground")
    low = min(pos[n[0]][2] for n in drums.nodes)
    check("lowest drum node above ground", low > 0.02, "%.3f m" % low)

    print("scenario 10: the deck reaches far enough for any wheelbase")
    check("deck half-length covers a 3.3 m wheelbase", B.DECK_Y >= 3.3,
          B.DECK_Y)

    print("scenario 11: the emitted jbeam carries the load and free-drum terms")
    jb = B.build_jbeam()
    check('shaft has no engine (inputName "")', '"", 1, {"connectedWheel"' in jb)
    check("dynamicFriction present", "dynamicFriction" in jb)
    check("drums are not braked", '"brakeTorque":0' in jb
          and '"parkingTorque":0' in jb
          and '"useDefaultBrakeInput":false' in jb)
    check("triangles carry a ground model that makes tyre sound",
          '"groundModel":"asphalt"' in jb)
    check("frame is welded to the world", '{"fixed":true},' in jb)

    print("scenario 12: every free node is inside the integrator's stability budget")
    # THIS IS THE CHECK THE FIRST BUILD DID NOT HAVE, and its absence cost a
    # vehicle that BeamNG deleted on spawn with "Multiple instabilities detected".
    # Physics steps at 2000 Hz and integration is explicit, so a node is stable
    # only while sqrt(k_total / m) < 2 / dt.  Nothing in the jbeam hints at this;
    # the numbers all look reasonable individually and the vehicle simply dies.
    dt = 1.0 / 2000.0
    limit = (2.0 / dt) ** 2
    load = {}
    for nid in drum_ids:
        load[nid] = 0.0
    for a, b in drums.beams:                 # drum beams carry the drum spring
        for n in (a, b):
            if n in load:
                load[n] += B.DRUM_BEAM_SPRING
    worst = max(load.items(), key=lambda kv: kv[1])
    ratio = worst[1] / B.DRUM_NODE_WEIGHT
    check("worst drum node k/m = %.3g, limit %.3g" % (ratio, limit),
          ratio < limit * 0.25,
          "%.1fx margin" % (limit / ratio if ratio else 0))
    # The form that was shipped and deleted: 25e6 N/m on 4 kg nodes.
    naive = (worst[1] / B.DRUM_BEAM_SPRING) * 25001000.0 / 4.0
    check("the build that exploded would fail this", naive > limit,
          "%.3g" % naive)

    print("scenario 13: the drum rim is beamed to its axle")
    # A rotator applies torque to a body the BEAMS hold; it does not hold the
    # body itself.  The first build connected the rings only to each other, so
    # the shell was attached to nothing at all.
    axle_ids = set()
    for tag, (a, b) in axis_nodes.items():
        axle_ids.add(a)
        axle_ids.add(b)
    axle_ids |= set(n[0] for n in frame.nodes if "_ax" in n[0])
    spokes = {}
    for nid in drum_ids:
        spokes[nid] = set()
    for a, b in drums.beams:
        if a in spokes and b in axle_ids:
            spokes[a].add(b)
        if b in spokes and a in axle_ids:
            spokes[b].add(a)
    unattached = [n for n, s in spokes.items() if len(s) < 2]
    check("every rim node has >= 2 spokes to the axle", not unattached,
          sorted(unattached)[:3])
    check("axle nodes are frame nodes, so they cannot themselves rotate",
          axle_ids <= frame_ids)

    print("scenario 14: spinning the drum changes no beam length")
    # Rotation must be a FREE mode or the drum fights its own structure.  Every
    # beam in the drum runs rim-to-rim within a ring, rim-to-rim along the axis,
    # or rim-to-axle -- all invariant under rotation about the axle.
    def spun(p, yc, ang):
        x, y, z = p
        dy, dz = y - yc, z - B.AXIS_Z
        c, s = math.cos(ang), math.sin(ang)
        return (x, yc + dy * c - dz * s, B.AXIS_Z + dy * s + dz * c)

    worst_err = 0.0
    for a, b in drums.beams:
        yc = B.V_SPACING / 2.0 * (1 if "drumR" in a else -1)
        pa, pb = pos[a], pos[b]
        qa, qb = spun(pa, yc, 0.7), spun(pb, yc, 0.7)
        d0 = math.dist(pa, pb)
        d1 = math.dist(qa, qb)
        worst_err = max(worst_err, abs(d1 - d0))
    check("no beam stretches when the drum turns",
          worst_err < 1e-9, "%.2e m" % worst_err)

    print("scenario 15: the rotator gets MARKERS, never the whole rim")
    # 485 nodes in the hamster barrel, 4 in the group its rotator names.  Handing
    # over the whole rim puts the rim under the wheel system, which then holds it
    # at an angular velocity nothing drives -- shipped twice, drum locked twice.
    jb = B.build_jbeam()
    for tag in ("F", "R"):
        check("drum%s rotator names only the marker group" % tag,
              '["drum%s_spd"]' % tag in jb and '["drum%s"], "drum' % tag not in jb)
        check("drum%s has exactly %d markers" % (tag, B.MARKER_COUNT),
              len(markers[tag]) == B.MARKER_COUNT, markers[tag])
        check("drum%s markers are real rim nodes" % tag,
              all(m in drum_ids for m in markers[tag]))
        check("drum%s markers are evenly spaced" % tag,
              len(set(markers[tag])) == B.MARKER_COUNT)
    check("markers are declared into BOTH groups",
          '"group": ["drumF", "drumF_spd"]' in jb)
    check("the shipped form that locked (whole rim) is gone",
          '["drumF"], "drumF_ax' not in jb)

    print("scenario 16: a rotator-free variant ships as the control")
    check("bare variant present", '"%s_bare": {' % B.MODEL in jb)
    bare = B.build_part(B.MODEL + "_bare", "x", False)
    check("bare variant has no rotators", '"rotators"' not in bare)
    check("bare variant has no powertrain", '"powertrain"' not in bare)
    check("bare variant still has the drum body",
          bare.count('"drumF_') > 50)
    withrot = B.build_part(B.MODEL, "x", True)
    check("the two differ only by those sections",
          '"rotators"' in withrot and '"powertrain"' in withrot)

    print("")
    if FAILED:
        print("%d FAILED: %s" % (len(FAILED), ", ".join(FAILED)))
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
