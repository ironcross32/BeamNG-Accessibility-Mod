"""Wheel slip / side-slide detection and voice placement diagnostics.

    python diagnostic/wheel_slip_sim.py

Scenario 1 replays a real recording: an ETK C on the Ice Pool, three clutch dumps and
several brake lockups, sampled per GFX frame by a temporary vehicle-VM probe
(``data/slip_ice_etkc.csv``: t, ground, electrics.wheelspeed, fastest wheel, mean wheel,
throttle, clutch, gear, rpm, per-wheel speeds). The same recording is run through the
LEGACY rule, which must answer the ~1 s latency the driver reported, so no check can
pass for free.
"""

from __future__ import annotations

import math
import os
import sys

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _ROOT)

import numpy as np  # noqa: E402

import wheel_slip as ws  # noqa: E402

PASSED = 0
_FIXTURE = os.path.join(_ROOT, "diagnostic", "data", "slip_ice_etkc.csv")


def check(label, condition, detail=""):
    global PASSED
    if not condition:
        raise AssertionError(f"{label} FAILED {detail}")
    PASSED += 1
    print(f"  ok  {label}")


def _load():
    rows = []
    with open(_FIXTURE) as fh:
        for line in fh:
            p = line.strip().split(",")
            if len(p) < 10:
                continue
            wheels = [float(x) for x in p[9].split("|")]
            rows.append(
                dict(
                    t=float(p[0]),
                    ground=float(p[1]),
                    elec=float(p[2]),
                    wmax=float(p[3]),
                    wmin=min(wheels),
                    reverse=p[7] == "-1",
                )
            )
    return rows


def _episodes(rows, legacy):
    det = ws.SlipDetector()
    eps, cur = [], None
    for r in rows:
        # +1 s: the detector treats a zero timestamp as "no previous sample".
        res = det.step(
            r["t"] + 1.0,
            r["ground"],
            None,  # the probe recorded speed only; straight-line dumps
            -1.0 if legacy else r["wmax"],
            -1.0 if legacy else r["wmin"],
            r["elec"],
            r["reverse"],
        )
        kind = res.kind if res.active else 0
        if kind != (cur[0] if cur else 0):
            if cur:
                eps.append((cur[0], cur[1], r["t"]))
            cur = (kind, r["t"]) if kind else None
    if cur:
        eps.append((cur[0], cur[1], rows[-1]["t"]))
    return eps


def _first(eps, kind, after, before):
    for k, start, _end in eps:
        if k == kind and after <= start <= before:
            return start
    return None


def _onset(rows, after, test):
    for r in rows:
        if r["t"] >= after and test(r):
            return r["t"]
    return None


def scenario_recording():
    print("1. Ice Pool recording: detection latency")
    rows = _load()
    new = _episodes(rows, legacy=False)
    old = _episodes(rows, legacy=True)
    spin = lambda r: r["wmax"] - r["ground"] > ws.SLIP_ABS_THRESHOLD_MS  # noqa: E731

    for label, after in (("1st gear dump", 16.0), ("2nd gear dump", 22.5), ("rolling dump", 36.5)):
        onset = _onset(rows, after, spin)
        fired = _first(new, ws.KIND_SPIN, onset, onset + 2.0)
        legacy = _first(old, ws.KIND_SPIN, onset, onset + 2.0)
        check(
            f"{label}: new fires within 0.12 s of onset",
            fired is not None and fired - onset <= 0.12,
            f"onset {onset} fired {fired}",
        )
        check(
            f"{label}: legacy is slower (the reported bug)",
            legacy is not None and legacy - onset >= 0.2,
            f"onset {onset} legacy {legacy}",
        )
    # The two standing starts are where the ground gate cost a full second.
    for after in (16.0, 22.5):
        onset = _onset(rows, after, spin)
        legacy = _first(old, ws.KIND_SPIN, onset, onset + 2.0)
        check(
            f"legacy standing start at {after}s is ~1 s late",
            legacy - onset > 0.9,
            f"{legacy - onset:.3f}",
        )

    # Brake lockup at ~19.7 s: all four wheels at zero while the car slides at 2.8 m/s,
    # released again by ~20.03 s. The legacy rule only reported it as the wheels spun up.
    release = 20.03
    lock_new = _first(new, ws.KIND_LOCK, 19.5, 20.5)
    lock_old = _first(old, ws.KIND_LOCK, 19.5, 20.5)
    check("brake lockup detected while the wheels are locked", lock_new is not None and lock_new < release, f"{lock_new}")
    check("legacy reported it only after release", lock_old is not None and lock_old >= release - 0.01, f"{lock_old}")

    # Nothing before the first dump: idling, clutch in, revving at a standstill.
    check("no activation before the first dump", all(start >= 16.7 for _k, start, _e in new), f"{new[:2]}")


def scenario_sideways():
    print("2. Sideways slide is a slide, not a lockup")
    det = ws.SlipDetector()
    # 10 m/s at 45 degrees off the nose, wheels rolling with the longitudinal component.
    v_long = 10.0 * math.cos(math.radians(45.0))
    res = None
    for i in range(30):
        res = det.step(1.0 + i / 60.0, 10.0, 45.0, v_long, v_long, v_long, False)
    check("no lockup", not res.active, f"{res}")
    check("slide active", res.slide_active and abs(res.slide_deg - 45.0) < 1e-6, f"{res}")
    check("placed toward the travel (left)", abs(res.azimuth_deg - 45.0) < 1e-6, f"{res.azimuth_deg}")
    naive = 10.0 - v_long
    check("naive total-speed rule would call it lockup", naive > ws._threshold(10.0), f"{naive:.2f}")

    # Hysteresis: 10 degrees keeps a slide on, and cannot start one.
    for i in range(30):
        res = det.step(2.0 + i / 60.0, 10.0, 10.0, 10.0, 10.0, 10.0, False)
    check("held at 10 deg once sliding", res.slide_active)
    fresh = ws.SlipDetector()
    for i in range(30):
        res = fresh.step(1.0 + i / 60.0, 10.0, 10.0, 10.0, 10.0, 10.0, False)
    check("10 deg does not start a slide", not res.slide_active)


def scenario_azimuth():
    print("3. Voice placement")
    check("+30 travel -> +30 (left)", ws.voice_azimuth(30.0, 10.0, False) == 30.0)
    check("-30 travel -> -30 (right)", ws.voice_azimuth(-30.0, 10.0, False) == -30.0)
    check("5 deg snaps dead ahead", ws.voice_azimuth(5.0, 10.0, False) == 0.0)
    check("175 deg snaps dead behind", ws.voice_azimuth(175.0, 10.0, True) == 180.0)
    check("standstill forward gear -> ahead", ws.voice_azimuth(90.0, 0.1, False) == 0.0)
    check("standstill reverse -> behind", ws.voice_azimuth(90.0, 0.1, True) == 180.0)
    check("no direction -> gear", ws.voice_azimuth(None, 5.0, True) == 180.0)
    # A sweep behind the car must stay behind: never pass through the front hemisphere.
    worst = min(abs(ws.voice_azimuth(b, 10.0, True)) for b in (150, 160, 170, 180, -170, -160, -150))
    check("sweep across 180 stays behind", worst >= 150.0, f"{worst}")
    # Blend region stays between the two ends rather than flipping sides.
    mid = ws.voice_azimuth(60.0, 1.25, False)
    check("blend is between gear and travel", 0.0 < mid < 60.0, f"{mid}")


def scenario_travel_bearing():
    print("4. travel_bearing sign (BeamTel heading is 180 off compass)")
    # Facing north is heading 180 here (see route_beacon.relative_bearing).
    check("north-facing, moving north -> 0", abs(ws.travel_bearing(0.0, 5.0, 180.0)) < 1e-6)
    check("north-facing, moving west -> +90", abs(ws.travel_bearing(-5.0, 0.0, 180.0) - 90.0) < 1e-6)
    check("north-facing, moving east -> -90", abs(ws.travel_bearing(5.0, 0.0, 180.0) + 90.0) < 1e-6)
    check("north-facing, moving south -> 180", abs(abs(ws.travel_bearing(0.0, -5.0, 180.0)) - 180.0) < 1e-6)
    # Naive: a true-compass bearing of the velocity against BeamTel's offset heading
    # mirrors it front to back -- driving forward would place the voice behind you.
    naive = ws.normalize_bearing(180.0 - math.degrees(math.atan2(0.0, 5.0)))
    check("naive un-offset form reads forward travel as behind", abs(abs(naive) - 180.0) < 1e-6, f"{naive}")


def scenario_standstill_and_legacy():
    print("5. Standstill spin, standstill lockup, legacy fallback")
    det = ws.SlipDetector()
    res = None
    for i in range(12):  # 0.2 s of 10 m/s wheelspin on a car doing 0.1 m/s
        res = det.step(1.0 + i / 60.0, 0.1, None, 10.0, 0.1, 1.0, False)
    check("standstill spin detected (no ground gate)", res.active and res.kind == ws.KIND_SPIN, f"{res}")
    check("standstill spin placed ahead", res.azimuth_deg == 0.0)

    det = ws.SlipDetector()
    for i in range(30):  # parked: every wheel 'locked', ground 0
        res = det.step(1.0 + i / 60.0, 0.0, None, 0.0, 0.0, 0.0, False)
    check("parked car is not a lockup", not res.active)

    det = ws.SlipDetector()
    for i in range(12):  # old mod: same spin, legacy rule gated on ground speed
        res = det.step(1.0 + i / 60.0, 0.1, None, -1.0, -1.0, 10.0, False)
    check("legacy path engaged when the wheel fields are -1", res.legacy)
    check("legacy keeps its ground gate", not res.active)


def scenario_classify():
    print("6. classify() for recorded samples")
    new = ws.classify(
        {"ground_speed_ms": 10.0, "travel_bearing_deg": 40.0, "wheel_max_driven_ms": 7.7, "wheel_min_ms": 7.6}
    )
    check("sideways sample: slide, no slip", new["slide"] and new["kind"] == "none", f"{new}")
    spin = ws.classify({"ground_speed_ms": 0.2, "wheel_max_driven_ms": 9.0, "wheel_min_ms": 0.2})
    check("standstill spin sample", spin["kind"] == "wheelspin", f"{spin}")
    old = ws.classify({"ground_speed_ms": 5.0, "wheel_speed_ms": 1.0})
    check("old sample (no wheel fields) uses legacy rule", old["kind"] == "lockup", f"{old}")


class _FakeHRTF:
    """Records the azimuths asked for; left ear louder for 0 < az < 180 (+LEFT)."""

    def __init__(self):
        self.azimuths = []

    def get_hrir(self, az):
        self.azimuths.append(az)
        s = math.sin(math.radians(az))
        return (
            np.array([1.0 + s, 0.2], dtype=np.float32),
            np.array([1.0 - s, 0.2], dtype=np.float32),
        )


def _render(ctrl, state, blocks=40, frames=480):
    ctrl.update_telemetry_state(state)
    out = np.zeros((frames, 2), dtype=np.float32)
    left = right = 0.0
    for _ in range(blocks):
        out.fill(0)
        ctrl._audio_callback(out, frames, None, None)
        left = float(np.sqrt(np.mean(out[:, 0] ** 2)))
        right = float(np.sqrt(np.mean(out[:, 1] ** 2)))
    return left, right


def scenario_audio():
    print("7. Audio: voices render, localise toward travel, fade out")
    import logging

    import audio

    ctrl = audio.AudioController(logging.getLogger("wheel_slip_sim"))
    if not ctrl._is_enabled:
        print("  -- skipped: audio dependencies unavailable")
        return
    ctrl._regenerate_waveforms()
    spin, lock, nlo, nhi = ctrl._slip_voice_tables()
    for name, tab, f0 in (("spin", spin, audio.SLIP_SPIN_HZ), ("lock", lock, audio.SLIP_LOCK_HZ)):
        mag = np.abs(np.fft.rfft(tab))
        top_harm = int(np.nonzero(mag > mag.max() * 1e-3)[0].max())
        check(f"{name} table reaches ~{audio.SLIP_HARMONIC_TOP_HZ:.0f} Hz", top_harm * f0 > 3000.0, f"{top_harm * f0}")
    lock_mag = np.abs(np.fft.rfft(lock))
    check("lock table has odd harmonics only", lock_mag[2] < lock_mag[3] * 1e-3)
    check("noise loop is seamless (periodic)", abs(float(nlo[0]) - float(nlo[-1])) < 0.5)

    ctrl._hrtf = _FakeHRTF()
    ctrl._hrtf_user_enabled = True
    base = {"slip_active": True, "slip_kind": 1, "slip_mag": 5.0, "slide_active": False,
            "slide_deg": 0.0, "slip_azimuth_deg": 60.0}
    l, r = _render(ctrl, base)
    check("spin voice audible", l > 1e-4)
    check("travel +60 (left) renders left-heavy", l > r * 1.2, f"L {l:.4f} R {r:.4f}")
    check("azimuth converged near 60", abs(ctrl._hrtf.azimuths[-1] - 60.0) < 2.0, f"{ctrl._hrtf.azimuths[-1]}")

    slide = dict(base, slip_active=False, slide_active=True, slide_deg=30.0, slip_azimuth_deg=-30.0)
    l, r = _render(ctrl, slide)
    check("slide voice alone, travel right, renders right-heavy", r > l * 1.2, f"L {l:.4f} R {r:.4f}")

    # Swing from +170 to -170 across dead astern: the smoothed azimuth must stay behind.
    _render(ctrl, dict(base, slip_azimuth_deg=170.0))
    ctrl._hrtf.azimuths.clear()
    _render(ctrl, dict(base, slip_azimuth_deg=-170.0), blocks=20)
    front = [a for a in ctrl._hrtf.azimuths if (a % 360.0) < 90.0 or (a % 360.0) > 270.0]
    check("swing across 180 never passes through the front", not front, f"{front[:3]}")

    off = dict(base, slip_active=False, slide_active=False)
    l, r = _render(ctrl, off, blocks=150)
    check("both voices fade to silence", l < 1e-5 and r < 1e-5, f"{l} {r}")
    check("overlap tail released when silent", ctrl._slip_overlap_L is None)

    # Stereo fallback keeps the side.
    ctrl._hrtf = None
    ctrl._hrtf_user_enabled = False
    l, r = _render(ctrl, dict(base, slip_azimuth_deg=90.0), blocks=60)
    check("stereo fallback: left travel pans left", l > r, f"L {l:.4f} R {r:.4f}")


def main():
    scenario_recording()
    scenario_sideways()
    scenario_azimuth()
    scenario_travel_bearing()
    scenario_standstill_and_legacy()
    scenario_classify()
    scenario_audio()
    print(f"\nall {PASSED} checks passed")


if __name__ == "__main__":
    main()
