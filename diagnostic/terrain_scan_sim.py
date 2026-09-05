"""Replays audio.py's terrain sonification scanner against known geometry.

    python diagnostic/terrain_scan_sim.py

No fake clock is needed here, unlike dock_tone_sim.py and hydro_steer_sim.py: render_scan is
a pure function of the packet, which is the whole reason the synthesis was put on the
listener thread instead of in the audio callback. That makes every property of the
instrument -- the tuning, the time axis, the click-freedom, the headroom -- checkable
offline with no stream, no device and no game.

The tuning constants are imported from audio.py rather than copied, so hand-tuning there
cannot silently invalidate these checks. Only the *logic* is duplicated here, and several
scenarios are deliberately NEGATIVE: they assert that the obvious wrong implementation
produces a specific wrong answer, so the check cannot pass for free.
"""

import math
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from audio import (  # noqa: E402
    SCAN_DURATION_S,
    SCAN_FAMILY_CODES,
    SCAN_GRAIN_ATTACK_MS,
    SCAN_GRAIN_DECAY_POW,
    SCAN_GRAIN_MS,
    SCAN_ICE_ATTACK_MULT,
    SCAN_LOOSE_FM_RATIO,
    SCAN_MAX_RANGE_M,
    SCAN_MIDDLE_C_HZ,
    SCAN_M_PER_OCTAVE,
    SCAN_OCTAVE_CLAMP,
    SCAN_OBJECT_OCTAVE,
    SCAN_PAVED,
    SCAN_PING_MS,
    SCAN_POI_GAP_MS,
    SCAN_POI_OCTAVE,
    SCAN_POI_PING_MS,
    SCAN_PROP_PING_MS,
    SCAN_PYTHAGOREAN_RATIOS,
    SCAN_REF_LEAD_S,
    SCAN_REF_PING_DB,
    SCAN_REF_PING_MS,
    SCAN_TERRAIN_DB,
    SCAN_TIME_JITTER_MS,
    _scan_envelope,
    _scan_family_wave,
    _scan_grain_envelope,
    _scan_object_ping,
    _scan_poi_doublet,
    _scan_reference_ping,
    _scan_terrain_grain,
    _scan_water_grain,
    render_scan,
    scan_pitch_hz,
    scan_step_from_dz,
)

SR = 48000

failures = []


def check(label, ok, detail=""):
    print(f"   {label}: {'OK' if ok else 'FAIL'}{(' - ' + detail) if detail else ''}")
    if not ok:
        failures.append(label)


def approx(a, b, tol=1e-9):
    return abs(a - b) <= tol


def octave_reduce(ratio):
    """Fold a frequency ratio into [1, 2)."""
    while ratio < 1.0 - 1e-12:
        ratio *= 2.0
    while ratio >= 2.0 - 1e-12:
        ratio /= 2.0
    return ratio


def nearest_table_error(ratio):
    r = octave_reduce(ratio)
    return min(abs(r - t) for t in SCAN_PYTHAGOREAN_RATIOS)


def flat_scan(dz=0.0, bearings=24, rings=25, reach=SCAN_MAX_RANGE_M, water=None):
    out = []
    denom = max(1, rings - 1)
    for s in range(bearings):
        b = -90.0 + s * (180.0 / max(1, bearings - 1))
        for r in range(rings):
            out.append((b, (r / denom) * reach, dz, water))
    return out


def energy_after(buf, t_s):
    i = int(SR * t_s)
    return float(np.sum(np.abs(buf[i:])))


def first_onset(buf, thresh=1e-4, after_s=SCAN_REF_LEAD_S * 0.9):
    """Index of the first sample past the reference ping that carries any signal."""
    i0 = int(SR * after_s)
    mag = np.max(np.abs(buf[i0:]), axis=1)
    idx = np.nonzero(mag > thresh)[0]
    return (i0 + int(idx[0])) if len(idx) else None


def centroid_hz(wave):
    spec = np.abs(np.fft.rfft(wave.astype(np.float64)))
    freqs = np.fft.rfftfreq(len(wave), 1.0 / SR)
    total = float(np.sum(spec))
    return float(np.sum(spec * freqs) / total) if total > 0 else 0.0


# =============================================================================================
print("1. flat ground renders one pitch, and that pitch is middle C")
# The single most load-bearing property. Level ground has to be the reference the listener
# judges everything else against, and the reference ping is only useful if it is literally
# the same note the flat ground produces.
check("level ground is middle C", approx(scan_pitch_hz(0.0), SCAN_MIDDLE_C_HZ, 1e-9),
      f"{scan_pitch_hz(0.0):.3f} Hz")
buf = render_scan(flat_scan(0.0), [], SCAN_MAX_RANGE_M, sr=SR)
seg = buf[int(SR * 1.0) : int(SR * 2.5), 0].astype(np.float64)
spec = np.abs(np.fft.rfft(seg))
peak_hz = float(np.fft.rfftfreq(len(seg), 1.0 / SR)[int(np.argmax(spec))])
check("the rendered flat cloud peaks at middle C", abs(peak_hz - SCAN_MIDDLE_C_HZ) < 3.0,
      f"peak {peak_hz:.1f} Hz")
print()

# =============================================================================================
print("2. every pitch the scanner can produce is a Pythagorean degree")
# Pythagorean rather than 12-TET is the whole point of the tuning, and it is the kind of
# thing that survives a refactor into an ordinary equal-tempered scale without anyone
# noticing by ear. So this asserts membership in the real table AND asserts that the
# equal-tempered table fails the same test.
worst = 0.0
for dz in np.linspace(-60.0, 60.0, 601):
    worst = max(worst, nearest_table_error(scan_pitch_hz(float(dz)) / SCAN_MIDDLE_C_HZ))
check("every pitch lands on a table degree", worst < 1e-9, f"worst error {worst:.2e}")

et_worst = max(nearest_table_error(2.0 ** (k / 12.0)) for k in range(12))
check("an equal-tempered scale would FAIL this check", et_worst > 1e-4,
      f"12-TET worst error {et_worst:.5f}")

steps = [scan_step_from_dz(float(d)) for d in np.linspace(-60.0, 60.0, 601)]
check("the step index is monotonic in elevation",
      all(b >= a for a, b in zip(steps, steps[1:])))
lim = int(round(SCAN_OCTAVE_CLAMP * 12.0))
check("the pitch pins rather than wrapping past the clamp",
      scan_step_from_dz(1e6) == lim and scan_step_from_dz(-1e6) == -lim,
      f"steps {scan_step_from_dz(-1e6)}..{scan_step_from_dz(1e6)}")
print()

# =============================================================================================
print("3. the fifth is pure 3:2, which is the reason for the tuning")
# A 12-TET fifth is 2 cents narrow. At this register that is a ~2 Hz beat, and a hillside at
# a steady grade lands on the same interval over and over, so the beat is exactly what a
# listener would hear the most of.
f5 = scan_pitch_hz(SCAN_M_PER_OCTAVE * 7.0 / 12.0)
cents = 1200.0 * math.log2(f5 / SCAN_MIDDLE_C_HZ)
check("the fifth is exactly 3:2", approx(f5 / SCAN_MIDDLE_C_HZ, 1.5, 1e-12),
      f"{f5 / SCAN_MIDDLE_C_HZ:.12f}")
check("...i.e. 701.955 cents, not 700", abs(cents - 701.955) < 0.01 and abs(cents - 700.0) > 1.9,
      f"{cents:.3f} cents")
f4 = scan_pitch_hz(SCAN_M_PER_OCTAVE * 5.0 / 12.0)
check("the fourth is exactly 4:3", approx(f4 / SCAN_MIDDLE_C_HZ, 4.0 / 3.0, 1e-12))
print()

# =============================================================================================
print("4. the scan takes the same time no matter how far it reached")
# A fixed duration is what makes "half a second in" mean the same fraction of the way out
# every time. If the length tracked the reach, the mapping would have to be relearned on
# every map and near every terrain edge.
near = render_scan(flat_scan(0.0, reach=60.0), [], 60.0, sr=SR)
far = render_scan(flat_scan(0.0, reach=200.0), [], 200.0, sr=SR)
check("a 60 m scan and a 200 m scan are the same length",
      near.shape[0] == far.shape[0], f"{near.shape[0]} vs {far.shape[0]}")
expected = int(SR * (SCAN_REF_LEAD_S + SCAN_DURATION_S))
tail = max(int(SR * SCAN_GRAIN_MS / 1000.0), int(SR * SCAN_PING_MS / 1000.0))
check("...and that length is the lead plus the duration plus one grain",
      expected < far.shape[0] <= expected + tail + int(SR * SCAN_TIME_JITTER_MS / 1000.0) + 8,
      f"{far.shape[0]} samples = {far.shape[0] / SR:.3f} s")
check("the reference ping fits inside the lead",
      SCAN_REF_LEAD_S >= SCAN_REF_PING_MS / 1000.0,
      f"lead {SCAN_REF_LEAD_S}s vs ping {SCAN_REF_PING_MS}ms")
print()

# =============================================================================================
print("5. time is distance, and it is monotonic")
onsets = []
for rng_m in (0.0, 25.0, 50.0, 100.0, 150.0, 200.0):
    b = render_scan([(0.0, rng_m, 0.0, None)], [], 200.0, sr=SR)
    onsets.append((rng_m, first_onset(b)))
ok = all(a[1] is not None and b[1] is not None and b[1] > a[1]
         for a, b in zip(onsets, onsets[1:]))
check("a farther sample always starts later", ok,
      ", ".join(f"{r:.0f}m@{(o / SR):.3f}s" for r, o in onsets))
near_t = onsets[0][1] / SR
check("the sample AT the vehicle starts as the cloud opens",
      abs(near_t - SCAN_REF_LEAD_S) < (SCAN_TIME_JITTER_MS / 1000.0) + 0.01,
      f"{near_t:.3f}s vs lead {SCAN_REF_LEAD_S}s")
far_t = onsets[-1][1] / SR
check("the sample at full reach lands at the end of the scan",
      abs(far_t - (SCAN_REF_LEAD_S + SCAN_DURATION_S)) < (SCAN_TIME_JITTER_MS / 1000.0) + 0.01,
      f"{far_t:.3f}s vs {SCAN_REF_LEAD_S + SCAN_DURATION_S:.3f}s")
print()

# =============================================================================================
print("6. water: deeper is lower, and it is unmistakably brighter")
# Depth rides on the LAKE BED rather than on a rule of its own, so "deeper is lower" is the
# same elevation map the dry ground uses and the water stays pitch-continuous with its shore.
freqs = [scan_pitch_hz(-d) for d in np.linspace(0.0, 40.0, 81)]
check("deeper water is monotonically lower",
      all(b <= a + 1e-12 for a, b in zip(freqs, freqs[1:])),
      f"{freqs[0]:.1f} Hz at the surface -> {freqs[-1]:.1f} Hz at 40 m")
wc = centroid_hz(_scan_water_grain(SCAN_MIDDLE_C_HZ, SR))
tc = centroid_hz(_scan_terrain_grain(SCAN_MIDDLE_C_HZ, SR))
check("the water grain is far brighter at the SAME pitch", wc > tc * 1.5,
      f"centroid {wc:.0f} Hz vs terrain {tc:.0f} Hz")

# The optional fourth field is the authoritative water tag. In particular, dz_0.0 must
# not collapse back to terrain just because there is no measurable water column at that
# shoreline sample. Render the packet-level forms so this guards the selector in
# render_scan, rather than merely proving that the two grain generators differ.
dry_same_pitch = render_scan([(0.0, 50.0, 0.0, None)], [], 200.0, sr=SR)
zero_depth_water = render_scan([(0.0, 50.0, 0.0, 0.0)], [], 200.0, sr=SR)
positive_depth_water = render_scan([(0.0, 50.0, 0.0, 1.0)], [], 200.0, sr=SR)
cloud_start = int(SR * SCAN_REF_LEAD_S)
dry_rendered_c = centroid_hz(dry_same_pitch[cloud_start:, 0])
zero_water_c = centroid_hz(zero_depth_water[cloud_start:, 0])
check("a zero-depth water cell selects the bright water grain",
      zero_water_c > dry_rendered_c * 1.5,
      f"centroid {zero_water_c:.0f} Hz vs dry {dry_rendered_c:.0f} Hz")
check("the water tag is authoritative at zero depth",
      np.allclose(zero_depth_water, positive_depth_water, rtol=0.0, atol=1e-7))
check("water and dry cells at identical pitch stay spectrally distinct",
      abs(zero_water_c - dry_rendered_c) > 300.0,
      f"centroid gap {zero_water_c - dry_rendered_c:.0f} Hz")
oc = centroid_hz(_scan_object_ping(SCAN_MIDDLE_C_HZ * 2.0, SR))
check("the object ping is brighter still", oc > wc, f"centroid {oc:.0f} Hz")
print()

# =============================================================================================
print("7. headroom is structural, not a tuned hope")
# Six hundred grains summing sixteen deep have no useful analytic peak bound, which is why
# the cloud is normalised rather than levelled per grain. Nothing here may rely on the
# callback's final clip to stay clean -- that clip exists to catch the SUM of every voice in
# the mod, and a scan that arrives already at the ceiling would make every other cue distort.
dense = flat_scan(0.0)
objs = [(float(b), float(r), 2.0) for b in range(-80, 81, 20) for r in range(10, 200, 40)]
b = render_scan(dense, objs, 200.0, sr=SR)
peak = float(np.max(np.abs(b)))
ceiling = 10.0 ** (SCAN_TERRAIN_DB / 20.0) + 10.0 ** (SCAN_REF_PING_DB / 20.0)
check("a dense scan stays well under full scale", peak < 0.999, f"peak {peak:.4f}")
check("...and under the cloud level plus the ping level", peak <= ceiling + 1e-6,
      f"peak {peak:.4f} vs {ceiling:.4f}")
loud = render_scan(dense, objs, 200.0, sr=SR, level_db=0.0)
check("even at a 0 dBFS setting it does not exceed full scale",
      float(np.max(np.abs(loud))) < 1.0 + 10.0 ** (SCAN_REF_PING_DB / 20.0) + 1e-6,
      f"peak {float(np.max(np.abs(loud))):.4f}")
print()

# =============================================================================================
print("8. no clicks anywhere")
# The user asked for this one directly. Every grain fades in and out; the check is that the
# fade is actually applied, and the negative half asserts that a grain WITHOUT it starts at
# a non-zero sample -- i.e. that this test could fail.
for name, wave in (
    ("terrain grain", _scan_terrain_grain(SCAN_MIDDLE_C_HZ, SR)),
    ("water grain", _scan_water_grain(SCAN_MIDDLE_C_HZ, SR)),
    ("object ping", _scan_object_ping(SCAN_MIDDLE_C_HZ * 2.0, SR)),
    ("reference ping", _scan_reference_ping(SR)),
):
    check(f"{name} starts and ends at silence",
          abs(float(wave[0])) < 1e-6 and abs(float(wave[-1])) < 1e-6,
          f"{float(wave[0]):.2e} .. {float(wave[-1]):.2e}")

n = int(SR * SCAN_GRAIN_MS / 1000.0)
t = np.arange(n) / float(SR)
naive = np.sin(2.0 * math.pi * 333.0 * t + 1.0)  # no envelope, arbitrary start phase
check("an un-faded grain WOULD click", abs(float(naive[0])) > 0.1,
      f"first sample {float(naive[0]):.3f}")

b = render_scan(flat_scan(0.0), [], 200.0, sr=SR)
check("the finished buffer starts and ends at silence",
      abs(float(b[0, 0])) < 1e-6 and abs(float(b[-1, 0])) < 1e-6)
print()

# =============================================================================================
print("9. no surface is silence, never a plateau at level")
# A missing sample rendered as 0.0 would be level ground, so the map running out would sound
# exactly like a flat plain -- a lie the listener has no way to catch, and the one place
# where being wrong is worse than saying nothing.
none_buf = render_scan(flat_scan(None), [], 200.0, sr=SR)
plateau_buf = render_scan(flat_scan(0.0), [], 200.0, sr=SR)
e_none = energy_after(none_buf, SCAN_REF_LEAD_S + 0.2)
e_flat = energy_after(plateau_buf, SCAN_REF_LEAD_S + 0.2)
check("an all-missing scan is silent after the reference ping", e_none < 1e-3,
      f"energy {e_none:.2e}")
check("...where the naive zero-fill form reports a plateau", e_flat > 100.0,
      f"energy {e_flat:.1f}")
mixed = render_scan(
    [(0.0, 20.0, 0.0, None), (0.0, 120.0, None, None)], [], 200.0, sr=SR
)
check("a single missing sample drops out without disturbing its neighbours",
      first_onset(mixed) is not None)
print()

# =============================================================================================
print("10. positive bearing is LEFT")
# The mod-wide convention, guarded here for the same reason dock_readout_sim.py guards it
# elsewhere: a sign error in a spatial cue reads as entirely plausible output.
left = render_scan([(45.0, 100.0, 0.0, None)], [], 200.0, sr=SR)
right = render_scan([(-45.0, 100.0, 0.0, None)], [], 200.0, sr=SR)
lo = int(SR * (SCAN_REF_LEAD_S + 0.1))
el, er = float(np.sum(np.abs(left[lo:, 0]))), float(np.sum(np.abs(left[lo:, 1])))
check("a bearing of +45 is louder on the LEFT", el > er * 1.3, f"L {el:.1f} vs R {er:.1f}")
rl, rr = float(np.sum(np.abs(right[lo:, 0]))), float(np.sum(np.abs(right[lo:, 1])))
check("a bearing of -45 is louder on the RIGHT", rr > rl * 1.3, f"L {rl:.1f} vs R {rr:.1f}")
check("the two are mirror images", approx(el, rr, 1e-3) and approx(er, rl, 1e-3))
print()

# =============================================================================================
print("11. the mod and audio.py agree about the reach")
# Python scales its whole time axis by the reach the mod reports, so the two only ever agree
# because the ceiling is the same number in both files. ramp_resolve_sim.lua guards the
# equivalent invariant for the docking feed the same way.
LUA = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "bng_mod", "lua", "ge", "extensions", "terrainScanner.lua",
)
try:
    src = open(LUA, encoding="utf-8").read()
    import re

    m = re.search(r"\nlocal SCAN_MAX_RANGE_M\s*=\s*([\d.]+)", src)
    check("terrainScanner.lua declares SCAN_MAX_RANGE_M", m is not None)
    if m:
        check("...and it matches audio.py", approx(float(m.group(1)), SCAN_MAX_RANGE_M, 1e-9),
              f"lua {m.group(1)} vs python {SCAN_MAX_RANGE_M}")
    bm = re.search(r"\nlocal SCAN_BEARINGS\s*=\s*(\d+)", src)
    rm = re.search(r"\nlocal SCAN_RINGS\s*=\s*(\d+)", src)
    check("the grid is declared and is a half circle worth of spokes",
          bm is not None and rm is not None and int(bm.group(1)) >= 8,
          f"{bm.group(1) if bm else '?'} x {rm.group(1) if rm else '?'}")
    check("the mod reports missing ground as the silence sentinel, not as zero",
          '"~"' in src)
    primary_ray = "local h = rayAt(p.x, p.y, SCAN_RAY_TOP_Z)"
    heightmap_fallback = "if h == nil then h = heightmapAt(p.x, p.y, scan.origin.z) end"
    check("visible-surface raycasts precede the TerrainBlock fallback",
          primary_ray in src and heightmap_fallback in src
          and src.index(primary_ray) < src.index(heightmap_fallback))
    check("River water is bounds-prefiltered and containment-tested",
          'findClassObjects(class)' in src and 'inXY(w.bounds, p.x, p.y)' in src
          and ':containsPoint(vec3(' in src and 'segment >= 0' in src)
    check("WaterBlock is footprint-scoped while WaterPlane can be unbounded",
          'class == "WaterPlane" then desc.bounds = nil' in src
          and 'w.class ~= "River" and inXY(w.bounds, p.x, p.y)' in src)
    check("reference, surface, and bed rays all spend the shared budget",
          src.count("scan.rayCount = scan.rayCount + 1") == 3
          and src.count("be:getSurfaceHeightBelow") == 2,
          # One occurrence is the explanatory comment; the only executable call is in
          # rayAt, so no unbudgeted call site can bypass the wrapper.
          f"{src.count('scan.rayCount = scan.rayCount + 1')} counters, "
          f"{src.count('be:getSurfaceHeightBelow')} textual ray mentions")
    check("diagnostics retain the visible range, cell counts, and ray total",
          all(token in src for token in (
              "lastScanDiag", "surfaceMin", "surfaceMax", "waterCells",
              "dryCells", "missingCells", "rayCount",
          )))
    if rm:
        # Onset jitter must stay under HALF a ring's spacing in time, or two rings can swap
        # and "later means farther" stops being true. The spacing shrinks whenever
        # SCAN_DURATION_S is shortened, so the two constants are coupled across the two
        # files and nothing else would catch it.
        spacing_ms = SCAN_DURATION_S * 1000.0 / max(1, int(rm.group(1)) - 1)
        check("the onset jitter stays inside half a ring's spacing",
              SCAN_TIME_JITTER_MS < spacing_ms / 2.0,
              f"jitter {SCAN_TIME_JITTER_MS} ms vs spacing {spacing_ms:.1f} ms")
    check("bearings are built as up-cross-forward, i.e. positive is LEFT",
          "vec3(0, 0, 1):cross(fwd)" in src)
except OSError as e:
    check("terrainScanner.lua is readable", False, str(e))
print()



# =============================================================================================
print("12. the grain melds, and it does not buy that with the time axis")
# The complaint this scenario exists for is that six hundred grains read as six hundred
# events. Overlap was never what was missing -- the old 90 ms flat-top grain already reached
# past a ring's 83.3 ms spacing. What made a grain an EVENT is that it sat at full level for
# 74 ms and then stopped. So the assertions are about the SHAPE, not about the length.
GRAIN_N = int(SR * SCAN_GRAIN_MS / 1000.0)
genv = _scan_grain_envelope(GRAIN_N, SR)


def flat_top_envelope(n, sr, fade_ms=8.0):
    """The pre-change window, rebuilt here as the negative control. It is not imported,
    because the whole point is that audio.py no longer contains it."""
    e = np.ones(n, dtype=np.float64)
    f = min(int(sr * fade_ms / 1000.0), n // 2)
    if f > 0:
        r = 0.5 - 0.5 * np.cos(np.linspace(0.0, math.pi, f))
        e[:f] *= r
        e[-f:] *= r[::-1]
    return e


oenv = flat_top_envelope(int(SR * 90.0 / 1000.0), SR)  # 90.0 is the old SCAN_GRAIN_MS
check("the grain envelope ends at exactly zero", genv[-1] == 0.0, f"{genv[-1]!r}")
# The value alone is not the claim. A LINEAR decay also ends at zero and still ticks, because
# it arrives there with a non-zero slope; cubic arrives with zero slope, which is why no
# release taper is needed and why adding one back would undo the reason for the exponent.
check("...and arrives there with zero SLOPE, which is what makes a taper unnecessary",
      abs(genv[-1] - genv[-2]) < 1e-9, f"final slope {abs(genv[-1] - genv[-2]):.2e}")
lin = 1.0 - np.arange(GRAIN_N) / float(GRAIN_N - 1)
check("...where a linear decay reaches zero with a slope three orders larger",
      abs(lin[-1] - lin[-2]) > 1e-6, f"linear slope {abs(lin[-1] - lin[-2]):.2e}")
check("there is no plateau: at half its length the grain is far below full level",
      genv[GRAIN_N // 2] < 0.2, f"{genv[GRAIN_N // 2]:.3f}")
check("...where the flat-topped form is still at full level there",
      oenv[len(oenv) // 2] > 0.99, f"{oenv[len(oenv) // 2]:.3f}")
new_slope = float(np.abs(np.diff(genv[int(0.8 * GRAIN_N):])).max())
old_slope = float(np.abs(np.diff(oenv[int(0.8 * len(oenv)):])).max())
check("the flat-topped form's offset is two orders steeper than the cubic's decay",
      old_slope > new_slope * 50.0, f"old {old_slope:.2e} vs new {new_slope:.2e}")
# Energy centroid, in closed form 1/(2p+2) for a (1-u)^p amplitude envelope. It moves EARLIER
# than the old form's, so the melding is not paid for out of distance resolution.
e2 = genv ** 2
tg = np.arange(GRAIN_N) / float(SR)
cen_new = float((e2 * tg).sum() / e2.sum())
o2 = oenv ** 2
to = np.arange(len(oenv)) / float(SR)
cen_old = float((o2 * to).sum() / o2.sum())
# A bare (1-u)^p amplitude envelope has its energy centroid at 1/(2p+2) in closed form --
# 1/8 of the grain at p = 3. The measured figure sits a little later than that, and the whole
# of the difference is the attack, which suppresses the earliest energy; asserting the closed
# form exactly would be asserting that the attack does not exist.
cen_frac = cen_new / (SCAN_GRAIN_MS / 1000.0)
closed = 1.0 / (2 * SCAN_GRAIN_DECAY_POW + 2)
check("the grain's energy centroid sits just past the bare cubic's 1/(2p+2)",
      closed <= cen_frac < closed + SCAN_GRAIN_ATTACK_MS / SCAN_GRAIN_MS,
      f"{cen_new * 1000.0:.1f} ms of {SCAN_GRAIN_MS:.0f} ms "
      f"= {cen_frac:.3f}, closed form {closed:.3f}")
check("...and is EARLIER than the old flat-topped grain's, not later",
      cen_new < cen_old,
      f"new {cen_new * 1000.0:.1f} ms vs old {cen_old * 1000.0:.1f} ms")
# The tail is what laps the next ring. Read the ring count from the mod so the two files stay
# coupled, exactly as the jitter ceiling in scenario 11 does.
try:
    import re as _re
    _src = open(os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "bng_mod", "lua", "ge", "extensions", "terrainScanner.lua"), encoding="utf-8").read()
    _rm = _re.search(r"\nlocal SCAN_RINGS\s*=\s*(\d+)", _src)
    ring_ms = SCAN_DURATION_S * 1000.0 / max(1, int(_rm.group(1)) - 1)
    db = 20.0 * np.log10(np.maximum(genv, 1e-12) / genv.max())
    audible_ms = float(np.nonzero(db > -20.0)[0][-1]) / SR * 1000.0
    check("the grain is still sounding when the next ring starts",
          audible_ms > ring_ms,
          f"audible to {audible_ms:.1f} ms vs ring spacing {ring_ms:.1f} ms")
except (OSError, AttributeError) as e:
    check("the ring spacing is readable from the mod", False, str(e))
# The reference ping deliberately keeps a BODY. Whole-envelope cubic there would drop its
# energy centroid to an eighth of its length and turn a 250 ms pitch reference into a click,
# and an absolute pitch cannot be judged against a transient.
renv = _scan_envelope(int(SR * SCAN_REF_PING_MS / 1000.0), SR)
check("the reference ping keeps a sustained body and only its RELEASE is cubic",
      renv[len(renv) // 3] > 0.99 and renv[-1] == 0.0,
      f"third {renv[len(renv) // 3]:.3f}, end {renv[-1]!r}")
print()

# =============================================================================================
print("13. surface families are separated on three axes, and none of them is pitch")
# Pitch is elevation, time is distance, pan is bearing and level carries nothing, so surface
# has only TIMBRE left. Three independent axes rather than five shades of brightness:
# brightness, roughness (FM sidebands) and shimmer (ice's detuned octaves).
F0 = scan_pitch_hz(0.0)
grains = {f: _scan_terrain_grain(F0, SR, f)
          for f in (SCAN_PAVED, "grass", "dirt", "gravel", "ice")}
cen = {f: centroid_hz(w) for f, w in grains.items()}
check("brightness is ordered loose < grass < paved",
      cen["dirt"] < cen["grass"] and cen["gravel"] < cen["grass"]
      and cen["grass"] < cen[SCAN_PAVED],
      ", ".join(f"{f} {c / F0:.2f}" for f, c in cen.items()))
check("...and the loose pair is a clear tier below grass, not a rounding error",
      cen["grass"] - max(cen["dirt"], cen["gravel"]) > 0.05 * F0,
      f"gap {(cen['grass'] - max(cen['dirt'], cen['gravel'])) / F0:.3f} f0")
check("ice is brighter than paved and darker than water",
      cen[SCAN_PAVED] < cen["ice"] < centroid_hz(_scan_water_grain(F0, SR)),
      f"paved {cen[SCAN_PAVED] / F0:.2f}, ice {cen['ice'] / F0:.2f}, "
      f"water {centroid_hz(_scan_water_grain(F0, SR)) / F0:.2f}")


def partial_db(wave, target_hz):
    """Level at a frequency relative to the waveform's own strongest partial."""
    spec = np.abs(np.fft.rfft(wave.astype(np.float64)))
    freqs = np.fft.rfftfreq(len(wave), 1.0 / SR)
    i = int(np.argmin(np.abs(freqs - target_hz)))
    lo, hi = max(0, i - 3), i + 4
    return 20.0 * np.log10(max(float(spec[lo:hi].max()), 1e-12) / float(spec.max()))


# Roughness. The modulator sits BELOW the carrier, as asked, so the sidebands land at
# f0*(1 -/+ ratio). The lower one is the reason the ratio is 5/6 and not 1/2: at 1/2 it would
# be f0/2, an octave under the fundamental, and a listener reads that as a LOWER NOTE -- the
# surface channel leaking into the elevation channel.
check("the FM modulator is below the carrier", SCAN_LOOSE_FM_RATIO < 1.0,
      f"ratio {SCAN_LOOSE_FM_RATIO:.4f}")
lower = F0 * (1.0 - SCAN_LOOSE_FM_RATIO)
check("...and its lower sideband is more than an octave under the fundamental",
      lower < F0 / 2.0, f"{lower:.1f} Hz against f0 {F0:.1f} Hz")
d_side = partial_db(grains["dirt"], lower)
v_side = partial_db(grains["gravel"], lower)
check("dirt and gravel both carry that sideband, well below the fundamental",
      -25.0 < d_side < -5.0 and -25.0 < v_side < -5.0,
      f"dirt {d_side:.1f} dB, gravel {v_side:.1f} dB")
check("...gravel is audibly rougher than dirt, which is what tells them apart",
      v_side > d_side + 3.0, f"gravel {v_side:.1f} dB vs dirt {d_side:.1f} dB")
check("...and neither paved nor grass has anything there, which is what tells THEM apart",
      partial_db(grains[SCAN_PAVED], lower) < -40.0
      and partial_db(grains["grass"], lower) < -40.0,
      f"paved {partial_db(grains[SCAN_PAVED], lower):.1f} dB, "
      f"grass {partial_db(grains['grass'], lower):.1f} dB")
# Ice against WATER, which is the pair brightness separates least well. Ice is octaves only,
# so the robust discriminator is the absence of a third harmonic entirely.
check("ice carries no third harmonic where water carries a strong one",
      partial_db(grains["ice"], F0 * 3.0) < -35.0
      and partial_db(_scan_water_grain(F0, SR), F0 * 3.0) > -20.0,
      f"ice {partial_db(grains['ice'], F0 * 3.0):.1f} dB, "
      f"water {partial_db(_scan_water_grain(F0, SR), F0 * 3.0):.1f} dB")
# The blanket guard: nothing about a surface may move the pitch.
for fam, w in grains.items():
    spec = np.abs(np.fft.rfft(w.astype(np.float64)))
    freqs = np.fft.rfftfreq(len(w), 1.0 / SR)
    peak_hz = float(freqs[int(np.argmax(spec))])
    check(f"the {fam} grain still sounds the elevation pitch and nothing else",
          approx(peak_hz, F0, 6.0), f"{peak_hz:.1f} Hz vs {F0:.1f} Hz")
print()

# =============================================================================================
print("14. no family is louder than its neighbours, and that is a property of the code")
# The user's requirement was that the FM must not make these tones louder than the ones
# beside them. Peak normalisation alone happens to land within 5% at today's indices, but
# that is luck at those particular numbers. The match is one-sided -- attenuate only -- so
# "no louder" holds however an index is later nudged, and it cannot fight the peak clamp.
for dz in (0.0, 20.0, -30.0, 40.0, -40.0):
    hz = scan_pitch_hz(dz)
    ref = _scan_terrain_grain(hz, SR, SCAN_PAVED).astype(np.float64)
    ref_rms = float(np.sqrt(np.mean(ref * ref)))
    for fam in ("grass", "dirt", "gravel", "ice"):
        w = _scan_terrain_grain(hz, SR, fam).astype(np.float64)
        rms = float(np.sqrt(np.mean(w * w)))
        rel = 20.0 * np.log10(rms / ref_rms)
        # Two-sided, so a family is neither louder NOR quieter. An attenuate-only rule
        # satisfies "never louder" and still left ice 1.21 dB down at the top of the clamp
        # range, which is level carrying information again.
        check(f"{fam} at dz {dz:+.0f} matches paved's loudness exactly",
              abs(rel) < 0.05 and float(np.max(np.abs(w))) < 1.5,
              f"{rel:+.3f} dB, peak {float(np.max(np.abs(w))):.3f}")
# Negative control: the same five families under PEAK normalisation alone -- the rule this
# replaced -- spread by well over a decibel across the clamp range, so the check above cannot
# be passing for free. That spread is also the evidence the file already carried without
# naming it: SCAN_WATER_DB_OFFSET exists precisely because a brighter voice at equal peak
# comes out louder.
peak_only_spread = 0.0
for dz in (0.0, 20.0, -30.0, 40.0, -40.0):
    hz = scan_pitch_hz(dz)
    ref = _scan_terrain_grain(hz, SR, SCAN_PAVED).astype(np.float64)
    ref_rms = float(np.sqrt(np.mean(ref * ref)))
    rels = []
    for fam in ("grass", "dirt", "gravel", "ice"):
        n = int(SR * SCAN_GRAIN_MS / 1000.0)
        mult = SCAN_ICE_ATTACK_MULT if fam == "ice" else 1.0
        raw = _scan_family_wave(hz, np.arange(n) / float(SR), SR, fam)
        raw = raw * _scan_grain_envelope(n, SR, mult)
        raw = raw / float(np.max(np.abs(raw)))
        rels.append(20.0 * np.log10(float(np.sqrt(np.mean(raw * raw))) / ref_rms))
    peak_only_spread = max(peak_only_spread, max(rels) - min(rels))
check("...where peak normalisation alone lets the families spread by over a decibel",
      peak_only_spread > 1.0, f"spread {peak_only_spread:.2f} dB")
print()

# =============================================================================================
print("15. an absent, unknown or water-covered family is today's tone")
# bng_mod/ is a live junction, so the two halves genuinely do go out of step. Every one of
# these has to degrade to the instrument as it was, not to an error and not to a guess.
paved_5 = render_scan([(0.0, 50.0, 0.0, None, SCAN_PAVED)], [], 200.0, sr=SR)
absent_4 = render_scan([(0.0, 50.0, 0.0, None)], [], 200.0, sr=SR)
none_5 = render_scan([(0.0, 50.0, 0.0, None, None)], [], 200.0, sr=SR)
unknown = render_scan([(0.0, 50.0, 0.0, None, "quicksand")], [], 200.0, sr=SR)
check("a four-field sample from an older mod half renders as paved, bit for bit",
      np.array_equal(absent_4, paved_5))
check("an explicit empty family renders as paved, bit for bit",
      np.array_equal(none_5, paved_5))
check("a family name from a NEWER mod half renders as paved rather than raising",
      np.array_equal(unknown, paved_5))
for fam in ("grass", "dirt", "gravel", "ice"):
    b = render_scan([(0.0, 50.0, 0.0, None, fam)], [], 200.0, sr=SR)
    check(f"...while {fam} genuinely differs from paved",
          not np.array_equal(b, paved_5))
water_fam = render_scan([(0.0, 50.0, 0.0, 1.0, "gravel")], [], 200.0, sr=SR)
water_plain = render_scan([(0.0, 50.0, 0.0, 1.0)], [], 200.0, sr=SR)
check("depth beats family: the material under a lake is not what you are looking at",
      np.array_equal(water_fam, water_plain))
check("every code the mod can send is a family audio.py knows",
      all(v in ("paved", "dirt", "gravel", "grass", "ice")
          for v in SCAN_FAMILY_CODES.values()),
      str(sorted(set(SCAN_FAMILY_CODES.values()))))
print()

# =============================================================================================
print("16. a POI is two events, and it cannot be read as two cells")
doub = _scan_poi_doublet(scan_pitch_hz(0.0, SCAN_POI_OCTAVE), SR)
win = int(SR * 0.002)
env_f = np.array([np.abs(doub[i:i + win]).max() for i in range(0, len(doub) - win)])
above = env_f > 0.08
onsets_n = int(np.count_nonzero(np.diff(above.astype(int)) == 1)) + (1 if above[0] else 0)
check("the doublet is exactly two events", onsets_n == 2, f"{onsets_n} onsets")
gap_ms = SCAN_POI_PING_MS + SCAN_POI_GAP_MS
try:
    check("...and they are closer together than one ring, so they are one event not two "
          "cells", gap_ms < ring_ms, f"{gap_ms:.1f} ms vs ring spacing {ring_ms:.1f} ms")
except NameError:
    check("the ring spacing was resolved in scenario 12", False)
single = _scan_object_ping(scan_pitch_hz(0.0, SCAN_OBJECT_OCTAVE), SR)
env_s = np.array([np.abs(single[i:i + win]).max() for i in range(0, len(single) - win)])
a_s = env_s > 0.08
check("...where the object ping is exactly one, so the count is the cue",
      int(np.count_nonzero(np.diff(a_s.astype(int)) == 1)) + (1 if a_s[0] else 0) == 1)
# Compared as centroid over the voice's OWN fundamental: the two sit in the same register by
# design, so an absolute-Hz comparison would be answering a different question.
pf = scan_pitch_hz(0.0, SCAN_POI_OCTAVE)
of = scan_pitch_hz(0.0, SCAN_OBJECT_OCTAVE)
check("...and the two are timbrally distinct as well as rhythmically",
      abs(centroid_hz(doub) / pf - centroid_hz(single) / of) > 0.3,
      f"POI {centroid_hz(doub) / pf:.2f} f0 vs object {centroid_hz(single) / of:.2f} f0")
check("both halves of the doublet are at the SAME pitch",
      np.array_equal(doub[:int(SR * SCAN_POI_PING_MS / 1000.0)],
                     doub[len(doub) - int(SR * SCAN_POI_PING_MS / 1000.0):]))
check("a scan with no pois argument matches one given an empty list",
      np.array_equal(render_scan([(0.0, 50.0, 0.0, None)], [], 200.0, sr=SR),
                     render_scan([(0.0, 50.0, 0.0, None)], [], 200.0, sr=SR, pois=[])))
poi_buf = render_scan([], [], 200.0, sr=SR, pois=[(0.0, 50.0, 0.0)])
check("a POI on its own renders", first_onset(poi_buf) is not None)
print()

# =============================================================================================
print("17. the object kind is read, and it changes length rather than identity")
# The p/v tag has been on the wire and discarded. It now picks the ping LENGTH -- an axis
# nothing else about an object uses -- so a cone ticks where a car rings, at no cost in new
# timbres to learn and with no possibility of confusion with the POI's rhythm.
veh = _scan_object_ping(scan_pitch_hz(0.0, SCAN_OBJECT_OCTAVE), SR, "v")
prop = _scan_object_ping(scan_pitch_hz(0.0, SCAN_OBJECT_OCTAVE), SR, "p")
check("a prop ping is shorter than a vehicle ping",
      len(prop) < len(veh)
      and approx(len(prop) / len(veh), SCAN_PROP_PING_MS / SCAN_PING_MS, 0.01),
      f"{len(prop)} vs {len(veh)} samples")
check("...but they sound the same elevation pitch",
      approx(float(np.fft.rfftfreq(len(veh), 1.0 / SR)[
                 int(np.argmax(np.abs(np.fft.rfft(veh.astype(np.float64)))))]),
             float(np.fft.rfftfreq(len(prop), 1.0 / SR)[
                 int(np.argmax(np.abs(np.fft.rfft(prop.astype(np.float64)))))]), 30.0))
o3 = render_scan([], [(0.0, 50.0, 0.0)], 200.0, sr=SR)
ov = render_scan([], [(0.0, 50.0, 0.0, "v")], 200.0, sr=SR)
op = render_scan([], [(0.0, 50.0, 0.0, "p")], 200.0, sr=SR)
check("a three-field object from an older mod half renders as a vehicle",
      np.array_equal(o3, ov))
check("...and the tag is genuinely consulted, not ignored", not np.array_equal(ov, op))
check("an unrecognised kind falls back to vehicle, the safer default",
      np.array_equal(render_scan([], [(0.0, 50.0, 0.0, "?")], 200.0, sr=SR), ov))
check("the object ping still ends at silence after the release change",
      abs(float(veh[0])) < 1e-6 and abs(float(veh[-1])) < 1e-6
      and abs(float(veh[-1]) - float(veh[-2])) < 1e-6,
      f"{float(veh[-1]):.2e}, slope {abs(float(veh[-1]) - float(veh[-2])):.2e}")
print()

# =============================================================================================
print("18. the mod half agrees about families, roads and POIs")
try:
    lsrc = open(LUA, encoding="utf-8").read()
    # Comment lines are stripped because the prose explaining these rules has to write the
    # very tokens the greps look for -- the trap vehicle_geometry_sim.lua scenario 12 fell
    # into and documents.
    code = "\n".join(ln for ln in lsrc.split("\n") if not ln.lstrip().startswith("--"))
    for code_char in SCAN_FAMILY_CODES:
        check(f"the mod can emit the family code {code_char!r}",
              ('"%s"' % code_char) in code, )
    check("the material lookup is present",
          "getMaterialIdxWs" in code and "getGroundmodelName" in code)
    # data.name is nil on every one of the game's 60 ground-model registrations, so the
    # registry cannot be walked and keyed by name however obvious that looks; the way in is
    # getGroundModelIDByName. Asserted here because the obvious form runs clean and silently
    # classifies nothing at all -- which sounds exactly like a map with no materials.
    check("families are classified by collisiontype id, resolved through the by-name lookup",
          "collisiontype" in code and "getGroundModelIDByName" in code
          and "getGroundModelByID" in code)
    check("...and the family table is representative NAMES, not transcribed ids",
          "FAMILY_NAMES" in code and '"GRAVEL"' in code and '"SNOW"' in code)
    check("the material lookup spends no rays, so the budget counters are still three",
          lsrc.count("scan.rayCount = scan.rayCount + 1") == 3
          and lsrc.count("be:getSurfaceHeightBelow") == 2,
          f"{lsrc.count('scan.rayCount = scan.rayCount + 1')} counters")
    check("a ray that disagrees with the heightmap reports no family, so a bridge deck is "
          "not painted with the riverbed under it",
          "MAT_TERRAIN_AGREE_M" in code)
    check("the road overlay is built from the navigation graph and bucketed",
          "map.getMap" in code and "ROAD_BUCKET_SIZE_M" in code
          and "ROAD_Z_TOLERANCE_M" in code)
    check("POIs come from the game's own big-map aggregation",
          "gameplay_rawPois" in code and "getRawPoiListByLevel" in code
          and "getCurrentLevelIdentifier" in code)
    check("POIs are capped, so a mission cluster cannot bury the terrain bed",
          "POI_MAX" in code)
    check("the named nearest-POI request uses the same big-map source",
          'cmd == "NEAREST_POI"' in code and "nearestPoi" in code
          and 'send("POI:" .. jsonEncode(result))' in code)
    check("equal-distance duplicates prefer the marker with a description",
          "richer" in code and "best.description" in code)
    check("POI names and descriptions use the game locale helper",
          "translateWithOrWithoutContext" in code)
    check("the diag reports the family histogram, the overlay count and the POI count",
          all(tok in code for tok in ("famCounts", "roadCells", "roadEdges", "poiCount",
                                      "hasTerrain")))
except OSError as e:
    check("terrainScanner.lua is readable", False, str(e))
print()

# =============================================================================================
if failures:
    print(f"{len(failures)} FAILURE(S): " + ", ".join(failures))
    sys.exit(1)
print("all checks passed")
