"""Replay the implement proximity speech state machine against a fake clock.

The point of this one is negative: it checks that the machine STAYS QUIET when it should.
Distance jitter across the enter threshold, a relation flapping at its boundary, and an
object drifting in and out at the leave threshold must each produce exactly one
announcement, while a genuine change of circumstance must produce one immediately.

    python diagnostic/implement_proximity_sim.py

The thresholds are imported from beamtel.py rather than copied. Only the *logic* is
duplicated here, mirroring implement_listener().
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

# beamtel.py pulls in wx, sounddevice and the speech stack at import time, which is far more
# than this needs. Read the thresholds straight out of the source instead.
_SRC = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "beamtel.py"
)
_consts = {}
with open(_SRC, encoding="utf-8") as fh:
    for line in fh:
        if line.startswith("IMPL_PROX_"):
            name, _, val = line.partition("=")
            try:
                _consts[name.strip()] = float(val.split("#")[0].strip())
            except ValueError:
                pass

ENTER_M = _consts["IMPL_PROX_ENTER_M"]
LEAVE_M = _consts["IMPL_PROX_LEAVE_M"]
RELATION_HOLD = _consts["IMPL_PROX_RELATION_HOLD"]
INSIDE_HOLD = _consts["IMPL_PROX_INSIDE_HOLD"]
LEAVE_HOLD = _consts["IMPL_PROX_LEAVE_HOLD"]

TICK = 0.1  # the Lua side scans at 10 Hz

failures = []


def check(label, ok, detail=""):
    print(f"   {label}: {'OK' if ok else 'FAIL'}{(' - ' + detail) if detail else ''}")
    if not ok:
        failures.append(label)


class Machine:
    """Mirrors implement_listener()'s transition logic. Records lines instead of speaking."""

    def __init__(self):
        self.said = []
        self.tracked = None
        self.relation = None
        self.inside = False
        self.pending_relation = None
        self.pending_inside = None
        self.pending_leave = None

    def clear(self, now):
        if self.tracked is None:
            return
        if self.pending_leave is None:
            self.pending_leave = now
        elif now - self.pending_leave >= LEAVE_HOLD:
            self.said.append(f"Clear of {self.tracked}")
            self.tracked = None
            self.relation, self.inside = None, False
            self.pending_relation = self.pending_inside = self.pending_leave = None

    def near(self, now, name, dist, relation, inside):
        if self.tracked is None:
            if dist < ENTER_M:
                self.said.append(f"approaching {name} {relation}")
                self.tracked = name
                self.relation, self.inside = relation, inside
                self.pending_relation = self.pending_inside = self.pending_leave = None
            return

        if name != self.tracked:
            if dist < ENTER_M:
                self.said.append(f"approaching {name} {relation}")
                self.tracked = name
                self.relation, self.inside = relation, inside
                self.pending_relation = self.pending_inside = self.pending_leave = None
            return

        if dist > LEAVE_M:
            if self.pending_leave is None:
                self.pending_leave = now
            elif now - self.pending_leave >= LEAVE_HOLD:
                self.said.append(f"Clear of {self.tracked}")
                self.tracked = None
                self.relation, self.inside = None, False
                self.pending_relation = self.pending_inside = self.pending_leave = None
            return
        self.pending_leave = None

        if relation != self.relation:
            if self.pending_relation is None or self.pending_relation[0] != relation:
                self.pending_relation = (relation, now)
            elif now - self.pending_relation[1] >= RELATION_HOLD:
                self.relation = relation
                self.pending_relation = None
                self.said.append(f"now {relation}")
        else:
            self.pending_relation = None

        if inside != self.inside:
            if self.pending_inside is None or self.pending_inside[0] != inside:
                self.pending_inside = (inside, now)
            elif now - self.pending_inside[1] >= INSIDE_HOLD:
                self.inside = inside
                self.pending_inside = None
                self.said.append("under" if inside else "out from under")
        else:
            self.pending_inside = None


def run(steps):
    """steps: list of (dist|None, relation, inside). None distance means a CLEAR packet."""
    m = Machine()
    t = 0.0
    for dist, relation, inside in steps:
        if dist is None:
            m.clear(t)
        else:
            m.near(t, "Gavril D-Series", dist, relation, inside)
        t += TICK
    return m.said


print(
    f"thresholds: enter {ENTER_M} m  leave {LEAVE_M} m  "
    f"holds relation {RELATION_HOLD}s inside {INSIDE_HOLD}s leave {LEAVE_HOLD}s"
)
print()

print("1. approach and withdraw: one enter line, one leave line")
said = run(
    [(d, "LEVEL", False) for d in [5.0, 4.0, 3.5, 2.8, 2.0, 1.5]]
    + [(d, "LEVEL", False) for d in [2.0, 3.0, 4.0, 5.0, 5.5, 6.0, 6.0, 6.0]]
)
check("exactly two lines", len(said) == 2, str(said))

print("2. distance jitter across the enter threshold speaks once")
jitter = []
for _ in range(30):
    jitter += [(3.05, "LEVEL", False), (2.95, "LEVEL", False)]
said = run(jitter)
check("exactly one line", len(said) == 1, str(said))

print("3. distance jitter across the LEAVE threshold does not chatter")
seq = [(2.0, "LEVEL", False)] * 5
for _ in range(30):
    seq += [(4.6, "LEVEL", False), (4.4, "LEVEL", False)]
said = run(seq)
check(
    "no leave line while it keeps coming back inside",
    len(said) == 1,
    str(said),
)

print("4. a relation flapping at its boundary must not narrate")
seq = [(2.0, "LEVEL", False)] * 5
for _ in range(30):
    seq += [(2.0, "BELOW", False), (2.0, "LEVEL", False)]
said = run(seq)
check("only the enter line", len(said) == 1, str(said))

print("5. a genuine relation change IS announced, once")
seq = [(2.0, "LEVEL", False)] * 5 + [(2.0, "BELOW", False)] * 20
said = run(seq)
check("enter plus exactly one relation line", len(said) == 2, str(said))
check("and it names the new relation", said[-1] == "now BELOW", str(said))

print("6. sliding the tines under the frame is announced, once")
seq = [(2.0, "LEVEL", False)] * 5 + [(0.5, "LEVEL", True)] * 20
said = run(seq)
check("enter plus exactly one 'under'", said.count("under") == 1, str(said))

print("7. backing out from under is announced, once")
seq = [(2.0, "LEVEL", False)] * 5 + [(0.5, "LEVEL", True)] * 10 + [(0.9, "LEVEL", False)] * 10
said = run(seq)
check("one under, one out", said.count("under") == 1 and said.count("out from under") == 1, str(said))

print("8. an 'inside' flag flickering does not chatter")
seq = [(2.0, "LEVEL", False)] * 5
for _ in range(30):
    seq += [(0.5, "LEVEL", True), (0.5, "LEVEL", False)]
said = run(seq)
check("only the enter line", len(said) == 1, str(said))

print("9. a nearer object takes over without a leave line for the old one")
m = Machine()
t = 0.0
for _ in range(5):
    m.near(t, "Gavril D-Series", 2.0, "LEVEL", False)
    t += TICK
for _ in range(5):
    m.near(t, "Ibishu Covet", 1.0, "ABOVE", False)
    t += TICK
check("two enter lines, no leave line", m.said == ["approaching Gavril D-Series LEVEL", "approaching Ibishu Covet ABOVE"], str(m.said))

print("10. driving straight past without getting close says nothing")
said = run([(d, "LEVEL", False) for d in [6.0, 5.0, 4.0, 3.5, 4.0, 5.0, 6.0]])
check("silent", said == [], str(said))

print("11. a CLEAR packet ends tracking, once")
seq = [(2.0, "LEVEL", False)] * 5 + [(None, None, None)] * 20
said = run(seq)
check("enter plus exactly one leave", len(said) == 2, str(said))

print("12. the alignment mode is announced on a real change, never on drifting in and out")
# The mode announcement carries real information -- the same two tones mean height in
# implement mode and squareness in ramp mode -- so an unannounced switch sends the operator
# the wrong way. But it must be a MODE change that fires it, not a target coming and going:
# hovering at the edge of the ramp feed would otherwise announce on every re-acquisition.
#
# Mirrors the DOCK: branch of implement_listener plus the DOCKCLEAR/DOCKFAIL/IMPLEMENT:
# handlers, which is where the decision not to clear the latch actually lives.
class Modes:
    def __init__(self):
        self.mode = None
        self.said = []

    def dock(self, mode):
        if mode != self.mode:
            self.mode = mode
            self.said.append(mode)

    def dockclear(self):
        pass          # losing the target is not a change of mode

    def dockfail(self):
        pass          # nor is the mod having nothing to say

    def implement(self):
        self.mode = None   # a part swap or vehicle change genuinely does invalidate it

    def toggle_off(self):
        self.mode = None


m = Modes()
for _ in range(30):
    m.dock("RAMP")
check("a steady ramp approach announces once", m.said == ["RAMP"], str(m.said))

# Twenty round trips across the feed's edge: in range, out of range, back in.
m = Modes()
for _ in range(20):
    for _ in range(5):
        m.dock("RAMP")
    m.dockclear()
check("drifting in and out of range never re-announces", m.said == ["RAMP"], str(m.said))

m = Modes()
for _ in range(5):
    m.dock("RAMP")
for _ in range(5):
    m.dockfail()
for _ in range(5):
    m.dock("RAMP")
check("a spell with no reading does not re-announce either", m.said == ["RAMP"], str(m.said))

# ...but climbing out of the car and into the loader must.
m = Modes()
for _ in range(5):
    m.dock("RAMP")
m.implement()
for _ in range(5):
    m.dock("IMPL")
check("switching to a machine with an implement announces the change",
      m.said == ["RAMP", "IMPL"], str(m.said))

# And prove the check is discriminating: clearing the latch on DOCKCLEAR, which is the
# tempting way to write it, chatters twenty times over the same approach.
class Eager(Modes):
    def dockclear(self):
        self.mode = None


e = Eager()
for _ in range(20):
    for _ in range(5):
        e.dock("RAMP")
    e.dockclear()
check("clearing the latch on DOCKCLEAR WOULD chatter", len(e.said) == 20,
      f"{len(e.said)} announcements for one approach")

print()
if failures:
    print(f"{len(failures)} FAILURE(S): " + ", ".join(failures))
    sys.exit(1)
print("all checks passed")
