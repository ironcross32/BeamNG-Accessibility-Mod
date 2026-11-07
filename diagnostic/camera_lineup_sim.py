"""Offline checks for camera_lineup.py (the free-camera line-up guide).

    python diagnostic/camera_lineup_sim.py

Pins the conventions that only the driver's seat would otherwise catch: which side a miss
is on, that the ratio dips for LEFT, that the sound sits on the side of the error, that a
target behind the nose reads as full error from behind, and the spoken wording.
"""

import math
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

import camera_lineup as cl  # noqa: E402

failures = 0


def check(name, cond, detail=""):
    global failures
    if cond:
        print(f"PASS  {name}")
    else:
        failures += 1
        print(f"FAIL  {name}  {detail}")


def geom(cam=(0.0, 0.0), cam_f=(0.0, 1.0), nose=(0.0, 50.0), veh_f=(0.0, -1.0)):
    return cl.Geometry(cam[0], cam[1], cam_f[0], cam_f[1], nose[0], nose[1], veh_f[0], veh_f[1])


# World: +x east, +y north. Camera at origin looking north. Up x forward for north is WEST,
# so the camera's left is -x.

# 1. Head-on, car 50 m north driving south straight at the camera, target offset 0.
st, m = cl.miss_left_m(geom(), cl.HEAD_ON, 0.0, 10.0)
check("1 head-on dead centre is ok and zero", st == "ok" and abs(m) < 1e-9, (st, m))
check("1 dead centre is silent", cl.tone_for(st, m).amp == 0.0)

# 2. Target 3 m to the camera's left (-x). Car heading south, its left is EAST (+x). The
# target is on the car's RIGHT, so the path passes LEFT of it: positive miss, ratio < 1,
# sound on the left (azimuth in 0..180).
st, m = cl.miss_left_m(geom(), cl.HEAD_ON, 3.0, 10.0)
check("2 target on camera's left: path misses LEFT by 3 m", st == "ok" and abs(m - 3.0) < 1e-9, m)
t = cl.tone_for(st, m)
check("2 ratio dips below 1 for left", t.ratio < 1.0, t)
check("2 sound on the left", 0.0 < t.az < 180.0, t.az)
check("2 index between 1.0 and 1.3", cl.INDEX_MIN < t.index < cl.INDEX_MAX, t.index)

# 3. Driving the car 3 m west (onto the target line) lines it up.
st, m = cl.miss_left_m(geom(nose=(-3.0, 50.0)), cl.HEAD_ON, 3.0, 10.0)
check("3 car moved onto the offset line is aligned", abs(m) < 1e-9, m)

# 4. Right miss: ratio above 1, sound on the right.
st, m = cl.miss_left_m(geom(nose=(-6.0, 50.0)), cl.HEAD_ON, 3.0, 10.0)
t = cl.tone_for(st, m)
check("4 path misses RIGHT", m < 0, m)
check("4 ratio above 1 for right", t.ratio > 1.0, t)
check("4 sound on the right", 180.0 < t.az < 360.0, t.az)

# 5. Full scale at and beyond FULL_M; deadzone edge silent.
t = cl.tone_for("ok", cl.FULL_M + 5)
check(
    "5 full scale clamps",
    abs(t.amp - 1.0) < 1e-9
    and abs(t.index - cl.INDEX_MAX) < 1e-9
    and abs(t.ratio - (1 - cl.RATIO_DRIFT)) < 1e-9
    and abs(t.az - 90.0) < 1e-9,
    t,
)
check("5 inside deadzone silent", cl.tone_for("ok", cl.DEADZONE_M * 0.9).amp == 0.0)

# 6. Heading error grows with distance: a 5 degree yaw at 50 m misses by 50 sin 5 = 4.36 m.
yaw = math.radians(5.0)
vf = (math.sin(yaw), -math.cos(yaw))  # turned toward +x (east) = the car's LEFT when facing south
st, m = cl.miss_left_m(geom(veh_f=vf), cl.HEAD_ON, 0.0, 10.0)
check("6 heading error shows as miss", abs(m - 50 * math.sin(yaw)) < 1e-6, m)  # perpendicular miss
check("6 turned to its left misses left", m > 0, m)

# 7. Pointed away: behind, full error, sound from behind.
st, m = cl.miss_left_m(geom(veh_f=(0.0, 1.0)), cl.HEAD_ON, 0.0, 10.0)
t = cl.tone_for(st, m)
check("7 pointed away is behind", st == "behind" and m is None, st)
check("7 behind is full and from behind", t.amp == 1.0 and t.az == 180.0, t)

# 8. Crossing: camera looking north, car 30 m west driving east, target depth 10 m.
g = geom(nose=(-30.0, 10.0), veh_f=(1.0, 0.0))
st, m = cl.miss_left_m(g, cl.CROSSING, 0.0, 10.0)
check("8 crossing at depth is aligned", st == "ok" and abs(m) < 1e-9, m)
g = geom(nose=(-30.0, 14.0), veh_f=(1.0, 0.0))
st, m = cl.miss_left_m(g, cl.CROSSING, 0.0, 10.0)
# Driving east, the car's left is NORTH. Path at y=14 is 4 m north of the target: LEFT.
check("8 crossing too far out misses LEFT", abs(m - 4.0) < 1e-9, m)
check("8 crossing ignores the head-on offset", cl.miss_left_m(g, cl.CROSSING, 7.0, 10.0)[1] == m)
g = geom(nose=(-30.0, 0.0), veh_f=(0.70710678, 0.70710678))
st, m = cl.miss_left_m(g, cl.CROSSING, 0.0, 30.0)
check("8 diagonal pass through the depth point counts", abs(m) < 1e-6, m)

# 9. Degenerate frames are unknown and silent.
st, m = cl.miss_left_m(geom(cam_f=(0.0, 0.0)), cl.HEAD_ON, 0.0, 10.0)
check("9 degenerate camera is unknown and silent", st == "unknown" and cl.tone_for(st, m).amp == 0)

# 10. Stepping and clamping.
o, d = cl.step_value(cl.HEAD_ON, 0.0, 10.0, -1)
check("10 head-on steps through zero to the right", o == -1.0 and d == 10.0, (o, d))
o, d = cl.step_value(cl.CROSSING, 0.0, cl.DEPTH_MIN_M, -1)
check("10 depth clamps at minimum", d == cl.DEPTH_MIN_M, d)

# 11. Wording.
check("11 left", cl.value_phrase(cl.HEAD_ON, 3.0, 10.0, "metric") == "3 meters left")
check("11 right", cl.value_phrase(cl.HEAD_ON, -1.0, 10.0, "metric") == "1 meter right")
check("11 zero", cl.value_phrase(cl.HEAD_ON, 0.0, 10.0, "metric") == "through the camera")
check("11 depth", cl.value_phrase(cl.CROSSING, 0.0, 12.0, "metric") == "12 meters out")
check("11 feet", cl.value_phrase(cl.CROSSING, 0.0, 3.0, "imperial") == "10 feet out")
check(
    "11 target",
    cl.target_phrase(cl.HEAD_ON, 3.0, 10.0, "metric") == "Head-on, 3 meters left of the camera",
)

# 12. Packet parsing.
check("12 OFF", cl.parse_packet("LINEUP:OFF") == "OFF")
p = cl.parse_packet("LINEUP:1,2,0,1,3,4,0,-1")
check("12 live", isinstance(p, cl.Geometry) and p.nose_y == 4.0, p)
check("12 short packet rejected", cl.parse_packet("LINEUP:1,2,3") is None)
check("12 tail tolerated", isinstance(cl.parse_packet("LINEUP:1,2,0,1,3,4,0,-1,extra"), cl.Geometry))
check("12 nan rejected", cl.parse_packet("LINEUP:nan,2,0,1,3,4,0,-1") is None)

# 13. Position follows the bearing, mirrored onto the error side, continuously round to behind.
# Target 3 m on the camera's left, car 50 m out: target is atan(3/50) = 3.4 deg to the car's
# RIGHT, the path misses LEFT, so the sound sits 3.4 deg LEFT.
st, m, b = cl.solve(geom(), cl.HEAD_ON, 3.0, 10.0)
t = cl.tone_for(st, m, b)
check("13 bearing negative for a target on the right", b < 0, b)
check("13 sound mirrored to the error side", abs(t.az - (-b)) < 1e-9 and 0 < t.az < 90, t.az)
# Target abeam on the car's right (car beside the camera, driving past it).
st, m, b = cl.solve(geom(nose=(3.0, 0.0), veh_f=(0.0, 1.0)), cl.HEAD_ON, 0.0, 10.0)
check("13 target abeam is 'behind' at 90 degrees", st == "behind" and abs(abs(b) - 90) < 1e-6, (st, b))
# Pointed straight away: sound straight behind.
st, m, b = cl.solve(geom(veh_f=(0.0, 1.0)), cl.HEAD_ON, 0.0, 10.0)
t = cl.tone_for(st, m, b)
check("13 pointed away sounds from behind", abs(t.az - 180.0) < 1e-6, t.az)
# Continuity: sweeping the heading round, the azimuth never jumps more than the heading moved
# (away from the exact rear, where left and right meet at 180).
prev = None
worst = 0.0
for deg10 in range(-1700, 1701, 5):
    yaw = math.radians(deg10 / 10.0)
    vf = (math.sin(yaw), -math.cos(yaw))
    st, m, b = cl.solve(geom(veh_f=vf), cl.HEAD_ON, 3.0, 10.0)
    az = cl.tone_for(st, m, b).az
    if prev is not None and cl.tone_for(st, m, b).amp > 0:
        d = abs(((az - prev) + 180) % 360 - 180)
        worst = max(worst, d)
    prev = az
check("13 azimuth continuous as the car turns", worst < 2.0, worst)

# 14. Ratio drift follows the angle: proportional to |bearing| / 180, left below 1.
for yaw_deg in (10.0, 45.0, 90.0, 150.0):
    yaw = math.radians(yaw_deg)
    vf = (math.sin(yaw), -math.cos(yaw))  # turned to the car's LEFT, so it misses LEFT
    st, m, b = cl.solve(geom(veh_f=vf), cl.HEAD_ON, 0.0, 10.0)
    t = cl.tone_for(st, m, b)
    want = 1.0 - cl.RATIO_DRIFT * abs(b) / 180.0
    check(f"14 ratio at {yaw_deg:.0f} deg turn is {want:.4f}", abs(t.ratio - want) < 1e-9, t.ratio)
st, m, b = cl.solve(geom(veh_f=(0.001, 1.0)), cl.HEAD_ON, 0.0, 10.0)
t = cl.tone_for(st, m, b)
check("14 ratio at the full drift straight behind", abs(abs(t.ratio - 1.0) - cl.RATIO_DRIFT) < 1e-4, t.ratio)
st, m, b = cl.solve(geom(veh_f=(-math.sin(0.5), -math.cos(0.5))), cl.HEAD_ON, 0.0, 10.0)
check("14 right miss drifts above 1", cl.tone_for(st, m, b).ratio > 1.0)

print()
print("ALL PASS" if failures == 0 else f"{failures} FAILED")
sys.exit(1 if failures else 0)
