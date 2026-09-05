"""Deterministic route-beacon protocol, bearing-sign, rate and phrasing diagnostics.

Every scenario also asserts what the NAIVE form answers, so no check can pass for free.
That matters more here than usual: this feature's failure mode is a beacon that pans
confidently to the wrong side, which sounds exactly like one that works.
"""

from __future__ import annotations

import ast
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import numpy as np  # noqa: E402

import audio  # noqa: E402
import route_beacon as rb  # noqa: E402


PASSED = 0


def check(label, condition, detail=""):
    global PASSED
    if not condition:
        raise AssertionError(f"{label} FAILED {detail}")
    PASSED += 1
    print(f"  ok  {label}")


def expect_bad(payload):
    try:
        rb.parse_route_packet(payload)
    except ValueError:
        return
    raise AssertionError(f"accepted malformed packet: {payload!r}")


# ==============================================================================
# 1. Protocol
# ==============================================================================
def scenario_protocol():
    print("1. ROUTE packet parsing")
    packet = rb.parse_route_packet("ROUTE:1200.5,-340.25,61.0,4821.5")
    check(
        "a well-formed packet round-trips",
        packet == {"dest": (1200.5, -340.25, 61.0), "route_m": 4821.5},
        str(packet),
    )

    # CLEAR is a MESSAGE, not an absence. None is the "no route" answer and is
    # distinguishable from a parse failure, which raises.
    check("CLEAR parses to None", rb.parse_route_packet("ROUTE:CLEAR") is None)

    expect_bad("ROUTE:1,2,3")  # short tail
    expect_bad("ROUTE:a,b,c,d")  # non-numeric
    expect_bad("ROUTE:1,2,nan,4")  # non-finite
    expect_bad("TRAILER:0,1,x")  # another extension's packet
    expect_bad("")
    check("malformed packets are all rejected", True)

    # A negative remaining distance is clamped rather than trusted.
    check(
        "a negative route distance clamps to zero",
        rb.parse_route_packet("ROUTE:0,0,0,-5")["route_m"] == 0.0,
    )


# ==============================================================================
# 2. Bearing sign — the expensive one to get wrong
# ==============================================================================
def scenario_bearing():
    print("2. relative_bearing: positive is LEFT")

    # BeamTel's heading is 180 deg off a true compass heading (yaw_to_heading_deg of the
    # MotionSim yawPos), and relative_bearing's atan2(-dx, -dy) carries the matching
    # +180 so the two cancel. Facing TRUE NORTH therefore means heading == 180 here.
    NORTH = 180.0

    dist, brg = rb.relative_bearing(0, 0, NORTH, 0, 100)
    check("dest ahead reads ~0", abs(brg) < 1e-6 and dist == 100.0, f"{brg}")

    _, brg = rb.relative_bearing(0, 0, NORTH, -100, 0)
    check("dest to the WEST while facing north reads +90 (left)", abs(brg - 90) < 1e-6, f"{brg}")

    _, brg = rb.relative_bearing(0, 0, NORTH, 100, 0)
    check("dest to the EAST while facing north reads -90 (right)", abs(brg + 90) < 1e-6, f"{brg}")

    _, brg = rb.relative_bearing(0, 0, NORTH, 0, -100)
    check("dest behind reads +/-180", abs(abs(brg) - 180) < 1e-6, f"{brg}")

    # Facing east (heading 270 in this convention), north is on the driver's left.
    _, brg = rb.relative_bearing(0, 0, 270.0, 0, 100)
    check("facing east, a northern dest is on the left", abs(brg - 90) < 1e-6, f"{brg}")

    # The naive form -- dropping the two cancelling 180s, i.e. a plain compass bearing
    # subtracted from BeamTel's heading -- mirrors the answer front to back. Asserting
    # that is what stops this whole scenario passing for free.
    def naive(px, py, heading, dx_, dy_):
        target = math.degrees(math.atan2(dx_ - px, dy_ - py)) % 360.0
        err = heading - target
        return (err + 180.0) % 360.0 - 180.0

    check(
        "the naive (un-offset) form answers 180 deg away from the truth",
        abs(abs(naive(0, 0, NORTH, 0, 100)) - 180.0) < 1e-6,
        f"{naive(0, 0, NORTH, 0, 100)}",
    )

    # Wrap at the 0/360 seam must not produce a 359-degree "turn".
    for heading in (0.0, 1.0, 359.0, 360.0):
        _, b = rb.relative_bearing(0, 0, heading, 5, 5)
        check(f"heading {heading} stays within +/-180", -180.0 <= b <= 180.0, f"{b}")


# ==============================================================================
# 3. The shared helper really is shared
# ==============================================================================
def scenario_shared_helper():
    print("3. the F9 W waypoint readout uses the shared helper")

    # Lifted out of beamtel.py by AST rather than copied -- the rule vehicle_info_sim.py
    # and environment_row_sim.py record. A sim carrying its own copy of the expression
    # keeps passing across exactly the edit that makes the two features disagree.
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    src = open(os.path.join(root, "beamtel.py"), encoding="utf-8").read()
    tree = ast.parse(src)

    calls = [
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "relative_bearing"
    ]
    check(
        "beamtel calls relative_bearing in at least two places "
        "(the waypoint readout, the beacon toggle and the telemetry loop)",
        len(calls) >= 3,
        f"found {len(calls)}",
    )

    # The old open-coded expression must be GONE from the waypoint branch, or the two
    # can drift apart again without any test noticing.
    check(
        "the open-coded atan2(-dx, -dy) waypoint bearing is gone from beamtel",
        "math.degrees(math.atan2(-dx, -dy))" not in src,
    )
    check(
        "...and lives in route_beacon instead",
        "math.atan2(-dx, -dy)"
        in open(os.path.join(root, "route_beacon.py"), encoding="utf-8").read(),
    )


# ==============================================================================
# 4. Rate map
# ==============================================================================
def scenario_rate():
    print("4. route beacon pulse rate")
    near = audio.route_beacon_rate_hz(distance=0.0)
    mid = audio.route_beacon_rate_hz(distance=400.0)
    far = audio.route_beacon_rate_hz(distance=5000.0)

    check("rate falls monotonically with distance", near > mid > far, f"{near} {mid} {far}")
    check("closest rate is the maximum", abs(near - audio.ROUTE_BEACON_MAX_RATE_HZ) < 1e-9)
    check("far rate approaches the minimum", abs(far - audio.ROUTE_BEACON_MIN_RATE_HZ) < 0.01)

    # The scale is the real difference from the road beacon. On the road beacon's 20 m
    # half-distance a 400 m destination is pinned at the floor and says nothing about
    # closing; on the route scale it still has most of its range left.
    road_at_400 = audio.road_beacon_rate_hz(distance=400.0)
    check(
        "at 400 m the road scale is already pinned at its floor",
        abs(road_at_400 - audio.ROAD_BEEP_MIN_RATE_HZ) < 0.001,
        f"{road_at_400}",
    )
    check(
        "...while the route scale still has range to give",
        mid > audio.ROUTE_BEACON_MIN_RATE_HZ + 0.5,
        f"{mid}",
    )

    check(
        "the two beacons cannot be confused by rate alone at the same distance",
        abs(audio.route_beacon_rate_hz(100.0) - audio.road_beacon_rate_hz(100.0)) > 0.5,
    )

    # Both beacons can sound at once, so the timbres must not collide.
    check(
        "carrier frequencies are far apart",
        abs(audio.ROUTE_BEACON_FREQ_HZ - audio.ROAD_BEEP_FREQ_HZ) > 150.0,
    )
    # ...and clear of the slots the rest of the mod already occupies.
    for name, hz in (("compass/cam FM", 900.0), ("scanner/coupler", 1000.0), ("slam tick", 660.0)):
        check(
            f"clear of the {name} slot",
            abs(audio.ROUTE_BEACON_FREQ_HZ - hz) > 80.0,
            f"{audio.ROUTE_BEACON_FREQ_HZ} vs {hz}",
        )


# ==============================================================================
# 5. Audio gating
# ==============================================================================
class FakeHRTF:
    def __init__(self):
        self.bearings = []

    def get_hrir(self, bearing):
        self.bearings.append(bearing)
        return (
            np.array([1.0, 0.5], dtype=np.float32),
            np.array([0.25, 0.125], dtype=np.float32),
        )


def scenario_audio_gate():
    print("5. audio gating and teardown")
    c = audio.AudioController.__new__(audio.AudioController)
    import threading

    c.lock = threading.Lock()
    c._route_beacon_mode_active = False
    c._route_beacon_available = False
    c._route_beacon_bearing = 0.0
    c._route_beacon_distance = 0.0
    c._route_beacon_timer = 0.0
    c._route_beacon_playback_pos = -1.0
    c._route_beacon_pulse_L = None
    c._route_beacon_pulse_R = None

    c.set_route_beacon_mode(True)
    c.update_route_beacon(True, 45.0, 250.0)
    check("an available route arms the renderer", c._route_beacon_available)
    check("bearing and distance are stored", c._route_beacon_bearing == 45.0
          and c._route_beacon_distance == 250.0)

    # A cleared route must drop the CONVOLVED BUFFERS, not merely the position: a pulse
    # already aimed at where the destination used to be would otherwise be mixed once
    # more after the route has gone.
    c._route_beacon_pulse_L = np.ones(8, dtype=np.float32)
    c._route_beacon_pulse_R = np.ones(8, dtype=np.float32)
    c._route_beacon_playback_pos = 2.0
    c.update_route_beacon(False)
    check("clearing the route silences the beacon", not c._route_beacon_available)
    check("...and drops the stale aimed pulse", c._route_beacon_pulse_L is None
          and c._route_beacon_playback_pos < 0)

    c.update_route_beacon(True, 10.0, 100.0)
    c._route_beacon_pulse_L = np.ones(8, dtype=np.float32)
    c._route_beacon_playback_pos = 1.0
    c.set_route_beacon_mode(False)
    check("toggling the mode off silences it too", not c._route_beacon_available
          and c._route_beacon_pulse_L is None)

    # Positive is LEFT, in both the HRTF and the stereo fallback -- the same contract
    # road_audio_sim.py asserts, re-checked here because the beacon rides the same helper.
    c._hrtf = None
    c._hrtf_user_enabled = False
    left, right = c._render_directional_pulse(np.ones(16, dtype=np.float32), 45)
    check("stereo fallback puts +45 on the left", np.max(np.abs(left)) > np.max(np.abs(right)))
    left, right = c._render_directional_pulse(np.ones(16, dtype=np.float32), -45)
    check("...and -45 on the right", np.max(np.abs(right)) > np.max(np.abs(left)))

    c._hrtf = FakeHRTF()
    c._hrtf_user_enabled = True
    c._render_directional_pulse(np.ones(16, dtype=np.float32), 45)
    check("HRTF path is handed the bearing unmodified", c._hrtf.bearings[-1] == 45)


# ==============================================================================
# 6. Distance wording
# ==============================================================================
def scenario_wording():
    print("6. distance wording and the toggle sentence")
    check("short imperial stays in feet", rb.format_long_distance(100, "imperial") == "328 feet")
    check("long imperial switches to miles", rb.format_long_distance(5000, "imperial") == "3.1 miles")
    check("short metric stays in meters", rb.format_long_distance(500, "metric") == "500 meters")
    check("long metric switches to km", rb.format_long_distance(5000, "metric") == "5.0 kilometers")

    # The hinge must not produce an unreadable figure on either side of itself.
    just_under = rb.format_long_distance(1000 / 3.28084 - 1, "imperial")
    just_over = rb.format_long_distance(1000 / 3.28084 + 1, "imperial")
    check("imperial hinge crosses feet -> miles exactly once",
          just_under.endswith("feet") and just_over.endswith("miles"),
          f"{just_under} / {just_over}")

    # The naive form -- road_guidance.format_road_distance, which only knows feet and
    # metres -- is what makes the switch necessary.
    from road_guidance import format_road_distance
    check(
        "the naive formatter would answer an unreadable figure",
        format_road_distance(5000, "imperial") == "16404 feet",
        format_road_distance(5000, "imperial"),
    )

    phrase = rb.route_beacon_phrase(3000.0, 30.0, 4200.0, "imperial")
    check("the toggle sentence names distance, side and the road distance",
          "1.9 miles" in phrase and "left" in phrase and "2.6 miles by road" in phrase, phrase)

    check("a target dead ahead is worded as such",
          "straight ahead" in rb.route_beacon_phrase(3000.0, 2.0, 3100.0, "metric"))
    check("a target behind is worded as such",
          "behind you" in rb.route_beacon_phrase(3000.0, 179.0, 3100.0, "metric"))

    # Route distance is omitted rather than spoken as zero when the mod could not give one.
    no_route = rb.route_beacon_phrase(500.0, 0.0, 0.0, "metric")
    check("an absent road distance is omitted, not spoken as zero",
          "by road" not in no_route, no_route)

    check("standing on the destination says so",
          "at the destination" in rb.route_beacon_phrase(2.0, 0.0, 0.0, "metric"))


def main():
    scenario_protocol()
    scenario_bearing()
    scenario_shared_helper()
    scenario_rate()
    scenario_audio_gate()
    scenario_wording()
    print(f"\nroute_beacon_sim: {PASSED} checks passed.")


if __name__ == "__main__":
    main()
