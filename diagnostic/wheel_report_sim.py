"""Assert wheel_report.py's labels, and the cross-VM chunk in wheelReport.lua.

wheel_report.py is pure, so this needs no game. Run after touching it or wheelReport.lua:

    python diagnostic/wheel_report_sim.py
"""
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
import wheel_report as wr  # noqa: E402

failures = 0


def check(name, ok, detail=""):
    global failures
    if not ok:
        failures += 1
    print(f"{'OK  ' if ok else 'FAIL'} {name}{(' -- ' + detail) if detail else ''}")


def wheel(lon, lat, width=0.265, psi="90", brake="60", broken="0"):
    return f"{lon},{lat},{width},{psi},{brake},{broken}"


def packet(*vehicles):
    lines = ["WHEELS"]
    for role, dual, wheels in vehicles:
        lines.append(f"V,{role},{1000 + role},Vehicle {role}")
        lines.append(f"{dual}|" + ";".join(wheels))
    return "\n".join(lines)


def labels(text):
    return [r["label"] for r in wr.all_rows(wr.parse_packet(text))]


# 1. A four-wheel car reads exactly as the old fixed-slot rows did.
car = packet((0, 0, [wheel(-1.3, -0.8), wheel(1.4, 0.8), wheel(1.4, -0.8), wheel(-1.3, 0.8)]))
got = labels(car)
check("car labels match the fixed slots",
      got == ["front left", "front right", "rear left", "rear right"], str(got))

# 1b. The ATV as measured live: its origin is off the centreline, so lateral must be centred.
atv = "WHEELS\nV,0,39148,Scout\n0|0.6,-0.4,0.62,1.2,,0;0.6,1.7,0.62,1.2,,0;-1.6,-0.4,0.62,1.2,,0;-1.6,1.7,0.62,1.2,,0"
got = labels(atv)
check("off-centre origin still reads left and right",
      got == ["front left", "front right", "rear left", "rear right"], str(got))
trike = packet((0, 0, [wheel(1.0, 1.3), wheel(1.0, 0.1), wheel(-1.0, 0.7)]))
got = labels(trike)
check("trike's single rear wheel is centred", got == ["front left", "front right", "rear"], str(got))

# 2. The Percheron box truck as measured live: front, simplified-dual drive axle, tag axle.
perch = packet((0, 1, [
    wheel(-2.63, -0.82, 0.59), wheel(-2.63, 0.83, 0.59),
    wheel(-3.98, -0.99), wheel(-3.98, 0.99),
    wheel(1.97, -0.99), wheel(1.97, 0.99),
]))
got = labels(perch)
check("Percheron: three axles, the drive axle dual", got == [
    "front left", "front right",
    "axle 2 left dual", "axle 2 right dual",
    "rear left", "rear right",
], str(got))

# 3. Without simplifiedDuallies a wide single is not called dual.
wide = packet((0, 0, [wheel(1.5, 0.9), wheel(1.5, -0.9), wheel(-1.5, 0.9, 0.59), wheel(-1.5, -0.9, 0.59)]))
got = labels(wide)
check("wide singles without the switch are not dual", "dual" not in " ".join(got), str(got))

# 4. Real duals (two physics wheels per end) read outward on the left, inward on the right.
real_duals = packet((0, 0, [
    wheel(2.0, 1.0), wheel(2.0, -1.0),
    wheel(-2.0, 0.73), wheel(-2.0, 1.08), wheel(-2.0, -0.73), wheel(-2.0, -1.08),
]))
got = labels(real_duals)
check("real duals are inner and outer, left to right", got == [
    "front left", "front right",
    "rear left outer", "rear left inner", "rear right inner", "rear right outer",
], str(got))

# 5. A motorcycle has no sides.
bike = packet((0, 0, [wheel(0.7, 0.0), wheel(-0.7, 0.01)]))
got = labels(bike)
check("motorcycle reads front and rear", got == ["front", "rear"], str(got))

# 6. One single-axle trailer: "trailer left". Two trailers are numbered.
one = packet((0, 0, [wheel(1.4, 0.8), wheel(1.4, -0.8), wheel(-1.3, 0.8), wheel(-1.3, -0.8)]),
             (1, 0, [wheel(-0.2, 0.9), wheel(-0.2, -0.9)]))
got = labels(one)
check("single trailer is unnumbered", got[4:] == ["trailer left", "trailer right"], str(got))
two = packet((0, 0, [wheel(1.4, 0.8), wheel(1.4, -0.8)]),
             (2, 0, [wheel(0.0, 0.9), wheel(0.0, -0.9)]),
             (1, 0, [wheel(3.0, 0.9), wheel(3.0, -0.9), wheel(-3.0, 0.9), wheel(-3.0, -0.9)]))
got = labels(two)
check("two trailers are numbered, in role order", got[2:] == [
    "trailer 1 front left", "trailer 1 front right", "trailer 1 rear left", "trailer 1 rear right",
    "trailer 2 left", "trailer 2 right",
], str(got))

# 7. Tandem axles 1.3 m apart stay two axles; duals at the same station stay one.
tandem = packet((0, 0, [wheel(5.0, 1.0), wheel(5.0, -1.0), wheel(-1.0, 1.0), wheel(-1.0, -1.0),
                        wheel(-2.3, 1.0), wheel(-2.3, -1.0)]))
got = labels(tandem)
check("tandem rear axles are separate", got[2:] == [
    "axle 2 left", "axle 2 right", "rear left", "rear right"], str(got))

# 8. Unknown is None, never zero, and zero survives as zero.
rows = wr.all_rows(wr.parse_packet(packet((0, 0, [
    wheel(1.0, 0.8, psi="", brake=""), wheel(1.0, -0.8, psi="0", brake="0")]))))
check("empty fields are unknown", rows[0]["psi"] is None and rows[0]["brake_c"] is None, str(rows[0]))
check("zero is a reading", rows[1]["psi"] == 0.0 and rows[1]["brake_c"] == 0.0, str(rows[1]))

# 9. Broken flag, malformed wheel skipped, foreign and empty packets.
rows = wr.all_rows(wr.parse_packet(packet((0, 0, [wheel(1.0, 0.8, broken="1"), "junk,1", wheel(1.0, -0.8)]))))
check("broken wheel flagged, junk skipped",
      len(rows) == 2 and rows[0]["broken"] and not rows[1]["broken"], str(rows))
check("non-WHEELS packet is None", wr.parse_packet("TRAILER:CLEAR") is None)
check("bare WHEELS is no vehicles", wr.parse_packet("WHEELS") == [])
check("vehicle with an empty payload has no rows",
      wr.all_rows(wr.parse_packet("WHEELS\nV,0,5,Prop\n0|")) == [])

# 10. The cross-VM chunk: no percent sign, no doubled close bracket, and positive-left lateral.
src = open(os.path.join(ROOT, "bng_mod", "lua", "ge", "extensions", "wheelReport.lua"),
           encoding="utf-8").read()
m = re.search(r"local VEHICLE_CHUNK = \[\[\n(.*?)\n\]\]", src, re.S)
check("VEHICLE_CHUNK found", m is not None)
if m:
    chunk = m.group(1)
    check("chunk has no percent sign", "%" not in chunk)
    check("chunk has no doubled close bracket", "]]" not in chunk)
    check("chunk builds lateral as up:cross(fwd)", "up:cross(fwd)" in chunk)

print()
print("ALL PASSED" if failures == 0 else f"{failures} FAILED")
sys.exit(1 if failures else 0)
