"""Replay the docking instrument's gate, phase and lock state machine against a fake clock.

Mirrors the docking block in audio.py so the phasing, the detune map and the lock latch can
be checked without an audio device or the game running. Run after touching that block:

    python diagnostic/dock_tone_sim.py

The tuning constants are imported from audio.py rather than copied, so hand-tuning there
cannot silently invalidate these checks. Only the *logic* is duplicated here.

The two things most worth protecting are not obvious from the code. First, the phasing: the
instrument exists because the WL-40 already has an articulation tone, a ground tone, a tilt
scale and possibly the scanner running, so if all three docking dimensions came in at once
the result would be the obstacle detector again. Second, the detune cap: the beat pair must
be a BEAT and not a pitch glide. The first version held one tone fixed and slid the other,
which is heard as a single tone changing pitch -- the fused percept sits at the pair's
average, so moving one partner moves the perceived pitch. Both partners now move
symmetrically about DOCK_REF_HZ, and the detune is specified as a beat rate rather than an
interval, so the centre pitch is nailed and the only thing that varies is the beating.
"""

import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from audio import (  # noqa: E402
    DOCK_AM_ABOVE_HZ,
    DOCK_AM_BELOW_HZ,
    DOCK_ATTACK_TAU,
    DOCK_BEAT_RANGE_M,
    DOCK_BEAT_HZ_PER_M,
    DOCK_BEAT_MAX_HZ,
    DOCK_LATERAL_FULL_M,
    DOCK_LOCK_BEAT_HZ,
    DOCK_LOCK_REARM_BEAT_HZ,
    DOCK_MAX_AZIMUTH_DEG,
    DOCK_MAX_RANGE_M,
    DOCK_PULSE_MAX_HZ,
    DOCK_PULSE_MIN_HZ,
    DOCK_RAMP_BEAT_HZ_PER_DEG,
    DOCK_RAMP_BEAT_RANGE_M,
    DOCK_RAMP_LATERAL_DEADZONE_M,
    DOCK_RAMP_LATERAL_FULL_M,
    DOCK_RAMP_LOCK_LATERAL_M,
    DOCK_RAMP_LOCK_YAW_DEG,
    DOCK_RAMP_MAX_RANGE_M,
    DOCK_RAMP_REARM_LATERAL_M,
    DOCK_RAMP_REARM_YAW_DEG,
    DOCK_REF_HZ,
    DOCK_REL_TAU,
    DOCK_STALE_S,
)

SR = 48000
FRAMES = 512
BLOCK_S = FRAMES / SR

# The mod scans at 10 Hz while the audio callback runs ~94 times a second. Reproducing that
# mismatch matters: the gate and the lock latch are evaluated per callback against values
# that only refresh every tenth of a second, so anything that reacts per-callback to a
# "change" is really reacting to the same number ten times over.
TELEMETRY_HZ = 10.0

failures = []


def check(label, ok, detail=""):
    print(f"   {label}: {'OK' if ok else 'FAIL'}{(' - ' + detail) if detail else ''}")
    if not ok:
        failures.append(label)


def clamp(v, a, b):
    return a if v < a else b if v > b else v


class Dock:
    """The docking block's state machine, one audio buffer at a time.

    Parameterised on mode because the ramp answer reuses every voice and differs only in its
    scaling constants and in the UNIT of the third channel -- metres of height in IMPL, degrees
    of squareness in RAMP. Defaults to IMPL so every scenario written before ramp mode existed
    still exercises exactly what it did.
    """

    def __init__(self, mode="IMPL"):
        self.mode = mode
        self.gate = 0.0
        self.beat = 0.0
        self.lock_armed = False
        self.lock_fires = 0
        self.t = 0.0
        self.last_packet_t = -1e9
        self.range = float("inf")
        self.lateral = 0.0
        # Ramp mode only: hunting for the mouth rather than lined up on the approach to it.
        # Latched by the listener from the same packet, never here -- see update_dock_target.
        self.acquire = False
        self.bearing = 0.0
        self.az = 0.0  # HRTF azimuth the pulse is placed at, positive-LEFT
        self.vertical = 0.0
        self.beat_hz = 0.0
        self.pulse_hz = 0.0
        self.am_hz = None  # None means the tremolo is suppressed
        self.cycle = 0.0  # accumulated pulse-cycle phase
        self.rate = 0.0  # rate it is advancing at
        self.cycle_jumps = []  # per-block phase error vs. rate*dt

    @property
    def max_range(self):
        return DOCK_RAMP_MAX_RANGE_M if self.mode == "RAMP" else DOCK_MAX_RANGE_M

    @property
    def beat_range(self):
        return DOCK_RAMP_BEAT_RANGE_M if self.mode == "RAMP" else DOCK_BEAT_RANGE_M

    @property
    def beat_slope(self):
        return DOCK_RAMP_BEAT_HZ_PER_DEG if self.mode == "RAMP" else DOCK_BEAT_HZ_PER_M

    @property
    def lat_full(self):
        return DOCK_RAMP_LATERAL_FULL_M if self.mode == "RAMP" else DOCK_LATERAL_FULL_M

    def packet(self, range_m, null_value, mode=None, reset_on_mode_change=True,
               lateral_m=0.0, acquire=False, bearing_deg=0.0):
        # The mode arrives ON the packet, not on the toggle, so the unit can never be one
        # packet out of step with the number it describes.
        if mode is not None and mode != self.mode:
            self.mode = mode
            if reset_on_mode_change:
                # Hysteresis on a quantity whose scale just changed ~50x. Leaving the latch
                # live across the boundary chimes "aligned" on the first packet in the new
                # mode. reset_on_mode_change=False reproduces that, so scenario 12 can show it.
                self.lock_armed = False
                self.beat = 0.0
        self.acquire = bool(acquire) and self.mode == "RAMP"
        self.lateral = lateral_m
        if self.acquire:
            # The range channel becomes the straight-line distance to the mouth, derived from
            # the two legs of the same right triangle rather than sent, and the along-range
            # reaches this method SIGNED -- floored at zero it would understate the distance by
            # the whole depth of the wrong side of the machine.
            self.range = math.hypot(range_m, lateral_m)
            self.bearing = bearing_deg
        else:
            self.range = max(0.0, range_m)
            self.bearing = 0.0
        self.vertical = null_value
        self.last_packet_t = self.t

    def block(self, dock_on=True):
        live = (
            dock_on
            and self.range < self.max_range
            and (self.t - self.last_packet_t) < DOCK_STALE_S
        )

        tgt = 1.0 if live else 0.0
        tau = DOCK_ATTACK_TAU if tgt > self.gate else DOCK_REL_TAU
        self.gate += (1.0 - math.exp(-BLOCK_S / tau)) * (tgt - self.gate)
        if self.gate < 0.01 and tgt == 0.0:
            self.gate = 0.0

        if self.gate > 0.0:
            rng_norm = clamp(1.0 - (self.range / self.max_range), 0.0, 1.0)
            self.pulse_hz = DOCK_PULSE_MIN_HZ + (
                DOCK_PULSE_MAX_HZ - DOCK_PULSE_MIN_HZ
            ) * (rng_norm**2)

            # Pulse position accumulates; it is never elapsed-time-modulo-period. Both of
            # those move -- absolute time grows without bound and the period changes with
            # range -- so the remainder teleports somewhere arbitrary in the cycle on every
            # telemetry update. Measured at up to 0.48 of a cycle per block, which is what
            # "stops and starts at irregular intervals" was.
            prev_rate = self.rate if self.rate > 0.0 else self.pulse_hz
            advance = (prev_rate + self.pulse_hz) / 2.0 * BLOCK_S
            self.cycle = (self.cycle + advance) % 1.0
            self.rate = self.pulse_hz

            # Where the pulse is placed. On the approach it is the lateral steering error
            # squeezed into the front hemisphere; while hunting it is the true bearing to the
            # mouth over the whole circle, because a mouth genuinely can be behind you.
            if self.acquire:
                self.az = self.bearing
            elif self.mode == "RAMP":
                lat_abs = abs(self.lateral)
                if lat_abs <= DOCK_RAMP_LATERAL_DEADZONE_M:
                    lat_norm = 0.0
                else:
                    lat_norm = math.copysign(
                        clamp(
                            (lat_abs - DOCK_RAMP_LATERAL_DEADZONE_M)
                            / (self.lat_full - DOCK_RAMP_LATERAL_DEADZONE_M),
                            0.0,
                            1.0,
                        ),
                        self.lateral,
                    )
                self.az = DOCK_MAX_AZIMUTH_DEG * lat_norm
            else:
                self.az = DOCK_MAX_AZIMUTH_DEG * clamp(
                    self.lateral / self.lat_full, -1.0, 1.0
                )

            # The null channel is held out entirely while hunting: squareness to the ramp axis
            # is a correction to a line, and from beside the machine there is no such line.
            btgt = 1.0 if (live and not self.acquire and self.range < self.beat_range) else 0.0
            btau = DOCK_ATTACK_TAU if btgt > self.beat else DOCK_REL_TAU
            self.beat += (1.0 - math.exp(-BLOCK_S / btau)) * (btgt - self.beat)
            if self.beat < 0.01 and btgt == 0.0:
                self.beat = 0.0

            self.beat_hz = min(abs(self.vertical) * self.beat_slope, DOCK_BEAT_MAX_HZ)
            if self.beat_hz <= DOCK_LOCK_BEAT_HZ:
                self.am_hz = None
            else:
                self.am_hz = DOCK_AM_ABOVE_HZ if self.vertical < 0.0 else DOCK_AM_BELOW_HZ

            if self.mode == "RAMP" and self.acquire:
                self.lock_armed = False
            elif self.mode == "RAMP" and (
                abs(self.lateral) >= DOCK_RAMP_REARM_LATERAL_M
                or abs(self.vertical) >= DOCK_RAMP_REARM_YAW_DEG
            ):
                self.lock_armed = True
            elif (
                self.mode == "RAMP"
                and self.lock_armed
                and abs(self.lateral) <= DOCK_RAMP_LOCK_LATERAL_M
                and abs(self.vertical) <= DOCK_RAMP_LOCK_YAW_DEG
                and self.beat > 0.5
            ):
                self.lock_armed = False
                self.lock_fires += 1
            elif self.mode != "RAMP" and self.beat_hz > DOCK_LOCK_REARM_BEAT_HZ:
                self.lock_armed = True
            elif (
                self.mode != "RAMP"
                and self.lock_armed
                and self.beat_hz <= DOCK_LOCK_BEAT_HZ
                and self.beat > 0.5
            ):
                self.lock_armed = False
                self.lock_fires += 1
        else:
            self.beat = 0.0
            self.pulse_hz = 0.0
            self.lock_armed = False

        self.t += BLOCK_S


def run(sim, seconds, range_fn, null_fn, dock_on=True, mode=None, lateral_fn=None):
    """Drive the gate at audio rate while the mod's values refresh at 10 Hz.

    null_fn supplies vertical metres in IMPL mode and yaw degrees in RAMP mode. lateral_fn is
    independent and defaults to the centreline.
    """
    n = int(seconds / BLOCK_S)
    next_packet = 0.0
    for _ in range(n):
        if sim.t >= next_packet:
            val = null_fn(sim.t)
            lateral = lateral_fn(sim.t) if lateral_fn is not None else 0.0
            sim.packet(range_fn(sim.t), val, mode=mode,
                       lateral_m=lateral)
            next_packet = sim.t + 1.0 / TELEMETRY_HZ
        sim.block(dock_on=dock_on)


print(
    f"tuning: silent past {DOCK_MAX_RANGE_M} m, pair enters at {DOCK_BEAT_RANGE_M} m, "
    f"beat {DOCK_BEAT_HZ_PER_M:.0f} Hz/m capped at {DOCK_BEAT_MAX_HZ:.0f} Hz"
)
print(
    f"        pulse {DOCK_PULSE_MIN_HZ}-{DOCK_PULSE_MAX_HZ} Hz, "
    f"lock under {DOCK_LOCK_BEAT_HZ} Hz beat, re-arms past {DOCK_LOCK_REARM_BEAT_HZ} Hz"
)
print()

print("1. phasing: no more than two dimensions are live during the acquire phase")
# The whole reason the vertical pair holds off until DOCK_BEAT_RANGE_M. At four metres the
# vertical does not matter yet and you are still steering; three simultaneous channels there
# is how the obstacle detector became something the user switched off and never switched on
# again.
s = Dock()
run(s, 2.0, lambda t: 4.0, lambda t: 0.5)
check("pulse is running at 4 m", s.gate > 0.9, f"gate {s.gate:.3f}")
check("beat pair is silent at 4 m", s.beat == 0.0, f"beat env {s.beat:.3f}")
run(s, 2.0, lambda t: 1.5, lambda t: 0.5)
check("beat pair enters inside the beat range", s.beat > 0.9, f"beat env {s.beat:.3f}")
print()

print("2. past the max range the instrument is silent, not merely quiet")
s = Dock()
run(s, 2.0, lambda t: DOCK_MAX_RANGE_M + 1.0, lambda t: 0.3)
check("gate is hard zero", s.gate == 0.0, f"gate {s.gate:.6f}")
check("a one-pole alone would never have reached it", math.exp(-2.0 / DOCK_REL_TAU) > 0.0)
print()

print("3. THE BEAT: centre pitch is fixed, only the beat rate moves")
# This is the correction to a first version that failed in listening. Holding one tone fixed
# and sliding the other moves the pair's AVERAGE frequency, and two tones that close fuse
# into one pitch percept sitting at that average -- so what the listener heard was a tone
# gliding up and down, with the beating masked behind it. Placing both partners
# symmetrically keeps the centre pitch nailed.
def partners(beat):
    return DOCK_REF_HZ - beat / 2.0, DOCK_REF_HZ + beat / 2.0


centres, rates = [], []
for err_m in [x * 0.02 for x in range(0, 51)]:
    b = min(err_m * DOCK_BEAT_HZ_PER_M, DOCK_BEAT_MAX_HZ)
    lo, hi = partners(b)
    centres.append((lo + hi) / 2.0)
    rates.append(hi - lo)

check(
    "the centre pitch never moves",
    max(centres) - min(centres) < 1e-9,
    f"centre held at {centres[0]:.1f} Hz across the whole error range",
)
check(
    "the beat rate is exactly the specified rate",
    all(abs(r - min(e * DOCK_BEAT_HZ_PER_M, DOCK_BEAT_MAX_HZ)) < 1e-9
        for r, e in zip(rates, [x * 0.02 for x in range(0, 51)])),
    "naming the beat rate directly is what keeps it independent of DOCK_REF_HZ",
)
check(
    "the whole range stays where beating is heard AS beating",
    max(rates) <= 15.0,
    f"fastest {max(rates):.1f} Hz; past roughly 15 Hz of separation the percept turns "
    f"into roughness and then into two separate pitches",
)
check(
    "a fine error still produces a countable beat",
    0.2 < 0.02 * DOCK_BEAT_HZ_PER_M < 2.0,
    f"2 cm error beats at {0.02 * DOCK_BEAT_HZ_PER_M:.2f} Hz",
)
check(
    "alignment is exact unison",
    partners(0.0)[0] == partners(0.0)[1],
    "the null has to be a texture change, not merely a slower beat",
)
check(
    "the pair is never a musical interval",
    max(rates) / DOCK_REF_HZ < 0.06,
    f"widest spread {1200 * math.log2((DOCK_REF_HZ + max(rates) / 2) / (DOCK_REF_HZ - max(rates) / 2)):.0f} cents",
)
print()

print("4. the lock fires once on the way through, not once per callback")
s = Dock()
# Approach from well above the band, settle onto it, and sit there.
run(s, 1.0, lambda t: 1.0, lambda t: 0.5)
run(s, 2.0, lambda t: 1.0, lambda t: max(0.0, 0.5 - 0.25 * t))
run(s, 3.0, lambda t: 1.0, lambda t: 0.0)
check("locked exactly once", s.lock_fires == 1, f"{s.lock_fires} fires")
print()

print("5. hunting around the null cannot machine-gun the lock")
# Without the re-arm threshold this is the failure mode: soft-body jitter and joystick
# feathering cross zero repeatedly, and each crossing would be another chime.
s = Dock()
run(s, 1.0, lambda t: 1.0, lambda t: 0.5)
run(s, 1.0, lambda t: 1.0, lambda t: 0.0)
first = s.lock_fires
# Wobble by well over the lock window but under the re-arm window.
wobble = (DOCK_LOCK_BEAT_HZ + DOCK_LOCK_REARM_BEAT_HZ) * 0.5 / DOCK_BEAT_HZ_PER_M
run(s, 4.0, lambda t: 1.0, lambda t: wobble * math.sin(t * 12.0))
check("no extra fires while hunting", s.lock_fires == first,
      f"{s.lock_fires - first} extra chimes from a {wobble * 100:.0f} cm wobble")
# But a genuine departure and return must chime again, or the cue is one-shot per session.
run(s, 1.0, lambda t: 1.0, lambda t: 0.5)
run(s, 1.5, lambda t: 1.0, lambda t: 0.0)
check("a real departure and return chimes again", s.lock_fires == first + 1,
      f"{s.lock_fires} total")
print()

print("6. the sign carrier distinguishes above from below, and stops at the null")
s = Dock()
run(s, 1.0, lambda t: 1.0, lambda t: 0.4)  # band above the tines: raise
above_rate = s.am_hz
run(s, 1.0, lambda t: 1.0, lambda t: -0.4)  # band below: lower
below_rate = s.am_hz
check("the two rates differ", above_rate != below_rate,
      f"raise {above_rate} Hz vs lower {below_rate} Hz")
check("they are far enough apart to name by ear",
      abs(above_rate - below_rate) >= 2.0,
      f"{abs(above_rate - below_rate):.1f} Hz apart")
check("both stay below the beat rates they coexist with",
      max(above_rate, below_rate) < DOCK_BEAT_MAX_HZ,
      f"tremolo up to {max(above_rate, below_rate)} Hz vs "
      f"{DOCK_BEAT_MAX_HZ:.1f} Hz of beating at full deflection")
run(s, 1.0, lambda t: 1.0, lambda t: 0.0)
check("tremolo stops at the null", s.am_hz is None,
      "a steady unison is the clearest 'stop' available")
print()

print("7. a stale feed fades out rather than freezing on the last reading")
# The mod stops sending entirely when it goes clear, so silence is the only signal. Holding
# the last position would leave a confident tone pointing at something no longer there.
s = Dock()
run(s, 1.0, lambda t: 1.0, lambda t: 0.0)
check("running while fed", s.gate > 0.9, f"gate {s.gate:.3f}")
for _ in range(int((DOCK_STALE_S + 1.0) / BLOCK_S)):
    s.block()
check("faded out after the stale window", s.gate == 0.0, f"gate {s.gate:.6f}")
print()

print("8. THE TRAIN: pulse position accumulates, it does not restart on every update")
# The failure this guards was reported three times as "stops and starts at irregular
# intervals" and misattributed twice. Absolute-time-modulo-a-changing-period does not
# advance, it teleports, and the rate cue underneath it stays perfectly correct the whole
# time -- which is exactly why it was so hard to name from the driver's seat.
s = Dock()
run(s, 1.0, lambda t: 4.5, lambda t: 0.0)
worst, prev_c, prev_r = 0.0, s.cycle, s.rate
n = int(2.0 / BLOCK_S)
for i in range(n):
    rng = 4.5 - (i * BLOCK_S) * 1.15
    s.packet(rng, 0.0)
    s.block()
    expect = (prev_r + s.rate) / 2.0 * BLOCK_S
    actual = (s.cycle - prev_c) % 1.0
    err = abs(actual - (expect % 1.0))
    worst = max(worst, min(err, 1.0 - err))
    prev_c, prev_r = s.cycle, s.rate
check("phase advances by exactly rate x dt every block", worst < 1e-9,
      f"worst error {worst:.2e} cycles")

# And show what the old formulation would have done on the same approach, so the check
# cannot quietly pass for free if someone reverts to it.
worst_old, prev, abs_t = 0.0, None, 60.0
for i in range(n):
    rng = 4.5 - (i * BLOCK_S) * 1.15
    nrm = clamp(1.0 - rng / DOCK_MAX_RANGE_M, 0.0, 1.0)
    hz = DOCK_PULSE_MIN_HZ + (DOCK_PULSE_MAX_HZ - DOCK_PULSE_MIN_HZ) * nrm * nrm
    pos = (abs_t % (1.0 / hz)) * hz
    if prev is not None:
        e = abs(pos - ((prev + hz * BLOCK_S) % 1.0))
        worst_old = max(worst_old, min(e, 1.0 - e))
    prev, abs_t = pos, abs_t + BLOCK_S
check("elapsed-time-modulo-period would teleport", worst_old > 0.1,
      f"worst error {worst_old:.3f} cycles -- half a cycle is maximally wrong")
print()

print("9. pulse rate rises monotonically all the way in and stays in band")
s = Dock()
rates, mono = [], True
for i in range(50):
    rng = DOCK_MAX_RANGE_M - i * 0.1
    run(s, 0.2, lambda t, r=rng: r, lambda t: 0.0)
    if s.gate > 0.5:
        rates.append(s.pulse_hz)
for a, b in zip(rates, rates[1:]):
    if b < a - 1e-9:
        mono = False
check("monotonic", mono, "")
check("within the configured band",
      all(DOCK_PULSE_MIN_HZ - 1e-6 <= r <= DOCK_PULSE_MAX_HZ + 1e-6 for r in rates),
      f"{min(rates):.2f}-{max(rates):.2f} Hz")
# Squared so the rate opens up late. A linear ramp spends most of its resolution out at four
# metres, where you are still steering, and has almost none left for the last half metre.
mid = [r for r in rates if r < (DOCK_PULSE_MIN_HZ + DOCK_PULSE_MAX_HZ) / 2]
check("most of the range sits below the midpoint rate", len(mid) > len(rates) * 0.6,
      f"{len(mid)} of {len(rates)} samples, i.e. the resolution is saved for close in")
print()

print("10. ramp mode is live where implement mode is silent, and vice versa")
# The ranges are per mode on purpose. Raising the implement's globally would stretch its pulse
# rate across a loader's whole working envelope; leaving the ramp's at 6 m would mean the
# instrument only speaks up once you are already committed to a line.
imp, ramp = Dock(), Dock("RAMP")
run(imp, 2.0, lambda t: 12.0, lambda t: 0.0)
run(ramp, 2.0, lambda t: 12.0, lambda t: 0.0, mode="RAMP")
check("at 12 m the implement instrument is silent", imp.gate == 0.0, f"gate {imp.gate:.3f}")
check("at 12 m ramp mode is fully up", ramp.gate > 0.99, f"gate {ramp.gate:.3f}")
far = Dock("RAMP")
run(far, 2.0, lambda t: DOCK_RAMP_MAX_RANGE_M + 1.0, lambda t: 0.0, mode="RAMP")
check("past the ramp max range it is silent, not merely quiet", far.gate == 0.0,
      f"gate {far.gate:.3f}")
print()

print("11. the ramp acquire phase is two-dimensional, not three")
# The same check as scenario 1 and it exists for the same reason: three live channels during a
# phase where you are still steering is how the obstacle detector became unusable. How precisely
# you sit between the walls does not matter at fifteen metres; it is everything at four.
acq = Dock("RAMP")
run(acq, 2.0, lambda t: 12.0, lambda t: 2.0, mode="RAMP")
check("at 12 m the null channel has not entered", acq.beat < 0.01, f"beat env {acq.beat:.3f}")
close = Dock("RAMP")
run(close, 2.0, lambda t: 5.0, lambda t: 2.0, mode="RAMP")
check("at 5 m it is fully in", close.beat > 0.99, f"beat env {close.beat:.3f}")
check("the ramp beat range is well outside the un-correctable zone",
      DOCK_RAMP_BEAT_RANGE_M >= 6.0,
      f"{DOCK_RAMP_BEAT_RANGE_M} m -- below ~4 m an alignment error needs reversing out")
print()

print("12. RAMP AXES: yaw drives beats; lateral drives only pulse position")
yaw = Dock("RAMP")
run(yaw, 1.0, lambda t: 4.0, lambda t: 12.0, mode="RAMP")
check("twelve degrees of yaw produces a 6 Hz beat",
      abs(yaw.beat_hz - 6.0) < 1e-9, f"{yaw.beat_hz:.2f} Hz")
check("yaw alone does not move the pulse", yaw.az == 0.0, f"{yaw.az:.1f} deg")
lat = Dock("RAMP")
run(lat, 1.0, lambda t: 4.0, lambda t: 0.0, mode="RAMP",
    lateral_fn=lambda t: 1.0)
check("a lateral offset moves the pulse", lat.az > 0.0, f"{lat.az:.1f} deg")
check("lateral alone leaves the pair at unison", lat.beat_hz == 0.0,
      f"{lat.beat_hz:.2f} Hz")
# The tremolo is the categorical heading cue. Positive yaw means turn LEFT, and the FAST rate
# is selected on a negative value, so fast means turn RIGHT.
sideL = Dock("RAMP")
run(sideL, 1.0, lambda t: 4.0, lambda t: 6.0, mode="RAMP")
sideR = Dock("RAMP")
run(sideR, 1.0, lambda t: 4.0, lambda t: -6.0, mode="RAMP")
check("the two heading signs pick different tremolo rates", sideL.am_hz != sideR.am_hz,
      f"left {sideL.am_hz} Hz vs right {sideR.am_hz} Hz")
check("slow means turn left and fast means turn right",
      sideR.am_hz == DOCK_AM_ABOVE_HZ and sideL.am_hz == DOCK_AM_BELOW_HZ,
      "arbitrary, and therefore asserted rather than remembered")

mi, mr = Dock(), Dock("RAMP")
run(mi, 1.0, lambda t: 2.0, lambda t: 0.5)
run(mr, 1.0, lambda t: 2.0, lambda t: 0.5, mode="RAMP")
check("mode-specific units produce their configured rates",
      abs(mi.beat_hz - 12.0) < 1e-9 and abs(mr.beat_hz - 0.25) < 1e-9,
      f"IMPL 0.5 m -> {mi.beat_hz:.2f} Hz; RAMP 0.5 deg -> {mr.beat_hz:.2f} Hz")
sat = Dock("RAMP")
run(sat, 1.0, lambda t: 2.0, lambda t: 24.0, mode="RAMP")
check("twenty-four degrees saturates the beat", abs(sat.beat_hz - DOCK_BEAT_MAX_HZ) < 1e-9,
      f"{sat.beat_hz:.2f} Hz")
check("the ramp slope is 0.5 Hz per degree",
      abs(DOCK_RAMP_BEAT_HZ_PER_DEG - 0.5) < 1e-9)
check("the ramp pan is scaled for a trough, not a bucket",
      DOCK_RAMP_LATERAL_FULL_M > DOCK_LATERAL_FULL_M * 2,
      f"{DOCK_RAMP_LATERAL_FULL_M} m vs {DOCK_LATERAL_FULL_M} m")
centre = Dock("RAMP")
run(centre, 1.0, lambda t: 4.0, lambda t: 0.0, mode="RAMP",
    lateral_fn=lambda t: DOCK_RAMP_LATERAL_DEADZONE_M)
full = Dock("RAMP")
run(full, 1.0, lambda t: 4.0, lambda t: 0.0, mode="RAMP",
    lateral_fn=lambda t: DOCK_RAMP_LATERAL_FULL_M)
check("15 cm is exactly centred in the pulse image", centre.az == 0.0,
      f"{centre.az:.1f} deg")
check("3.0 m still reaches the full 75 degree azimuth",
      abs(full.az - DOCK_MAX_AZIMUTH_DEG) < 1e-9, f"{full.az:.1f} deg")

lock = Dock("RAMP")
run(lock, 1.0, lambda t: 4.0, lambda t: 0.0, mode="RAMP",
    lateral_fn=lambda t: DOCK_RAMP_REARM_LATERAL_M)
run(lock, 1.0, lambda t: 4.0, lambda t: 0.0, mode="RAMP",
    lateral_fn=lambda t: DOCK_RAMP_LOCK_LATERAL_M + 0.05)
check("square but laterally displaced does not lock", lock.lock_fires == 0)
run(lock, 1.0, lambda t: 4.0, lambda t: DOCK_RAMP_LOCK_YAW_DEG + 1.0,
    mode="RAMP", lateral_fn=lambda t: 0.0)
check("centred but crooked does not lock", lock.lock_fires == 0)
run(lock, 1.0, lambda t: 4.0, lambda t: 0.0, mode="RAMP",
    lateral_fn=lambda t: 0.0)
check("correcting both axes locks once", lock.lock_fires == 1,
      f"{lock.lock_fires} fires")
run(lock, 1.0, lambda t: 4.0, lambda t: DOCK_RAMP_REARM_YAW_DEG - 0.1,
    mode="RAMP", lateral_fn=lambda t: DOCK_RAMP_REARM_LATERAL_M - 0.01)
run(lock, 1.0, lambda t: 4.0, lambda t: 0.0, mode="RAMP",
    lateral_fn=lambda t: 0.0)
check("sub-rearm departures do not produce another lock", lock.lock_fires == 1)
run(lock, 1.0, lambda t: 4.0, lambda t: DOCK_RAMP_REARM_YAW_DEG,
    mode="RAMP", lateral_fn=lambda t: 0.0)
run(lock, 1.0, lambda t: 4.0, lambda t: 0.0, mode="RAMP",
    lateral_fn=lambda t: 0.0)
check("8 degrees re-arms and a return locks again", lock.lock_fires == 2,
      f"{lock.lock_fires} fires")
print()

print("13. changing mode does not fire a spurious lock chime")
# Drive 0.4 m of height error in implement mode -- 9.6 Hz, well past the re-arm threshold, so
# the latch is live -- then switch to ramp mode at 0.2 degrees yaw, which is inside its lock
# window. Nothing has been aligned; only the unit and scale changed, and they changed
# by an order of magnitude, which is exactly what makes a latch carried across the boundary a
# lie rather than a rounding error.
sw = Dock()
run(sw, 1.0, lambda t: 2.0, lambda t: 0.4)
check("the latch is armed before the switch", sw.lock_armed, f"beat {sw.beat_hz:.2f} Hz")
before = sw.lock_fires
run(sw, 1.0, lambda t: 2.0, lambda t: 0.2, mode="RAMP")
check("no chime across the mode change", sw.lock_fires == before,
      f"{sw.lock_fires - before} chime(s)")

# And prove the check is discriminating rather than passing for free: without the reset, the
# same sequence chimes.
bug = Dock()
run(bug, 1.0, lambda t: 2.0, lambda t: 0.4)
bug_before = bug.lock_fires
n = int(1.0 / BLOCK_S)
nxt = bug.t
for _ in range(n):
    if bug.t >= nxt:
        bug.packet(2.0, 0.2, mode="RAMP", lateral_m=0.0, reset_on_mode_change=False)
        nxt = bug.t + 1.0 / TELEMETRY_HZ
    bug.block()
check("without the latch reset it WOULD chime", bug.lock_fires > bug_before,
      f"{bug.lock_fires - bug_before} spurious chime(s) announcing an alignment that is not one")
print()

print("14. hunting for the mouth: the pulse is a beacon, and the range channel MOVES")
# The failure this phase was built for. The mod used to floor the along-axis range at zero, so
# everywhere behind the mouth plane -- which is most of a lap around a sixteen-metre cannon --
# the instrument was fed a constant 0.0. The pulse therefore sat at its contact rate and never
# changed, and the operator's report was exactly that: the tones do not move.


def hunt(sim, seconds, along_fn, lat_fn, bearing_fn):
    n = int(seconds / BLOCK_S)
    nxt = sim.t
    for _ in range(n):
        if sim.t >= nxt:
            sim.packet(along_fn(sim.t), 0.0, mode="RAMP", lateral_m=lat_fn(sim.t),
                       acquire=True, bearing_deg=bearing_fn(sim.t))
            nxt = sim.t + 1.0 / TELEMETRY_HZ
        sim.block()


rates = []
h = Dock("RAMP")
for along in (-12.0, -9.0, -6.0, -3.0):
    hunt(h, 0.4, lambda t, a=along: a, lambda t: 3.0, lambda t: 140.0)
    rates.append(h.pulse_hz)
check("driving toward the mouth from BEHIND the plane speeds the pulse up",
      all(b > a + 0.2 for a, b in zip(rates, rates[1:])),
      " -> ".join(f"{r:.2f}" for r in rates))
# ...and the same run with the old floor in place, to show the check is not passing for free.
floored = Dock("RAMP")
old = []
for along in (-12.0, -9.0, -6.0, -3.0):
    n = int(0.4 / BLOCK_S)
    nxt = floored.t
    for _ in range(n):
        if floored.t >= nxt:
            floored.packet(max(0.0, along), 0.0, mode="RAMP")
            nxt = floored.t + 1.0 / TELEMETRY_HZ
        floored.block()
    old.append(floored.pulse_hz)
check("the floored range WOULD have pinned it at the contact rate",
      max(old) - min(old) < 1e-9 and abs(old[-1] - DOCK_PULSE_MAX_HZ) < 1e-9,
      f"{old[-1]:.2f} Hz throughout, from twelve metres away on the wrong side")

# The pulse is placed at the mouth's true bearing, over the whole circle. The approach-phase
# pan cannot express "behind you" at all -- it is a steering error, capped at the front
# hemisphere -- and behind you is exactly where the mouth is during this phase.
check("a mouth astern is panned astern", abs(h.az) > DOCK_MAX_AZIMUTH_DEG,
      f"{h.az:.0f} deg, past the {DOCK_MAX_AZIMUTH_DEG:.0f} deg steering-error ceiling")
h2 = Dock("RAMP")
run(h2, 1.0, lambda t: 4.0, lambda t: DOCK_RAMP_REARM_YAW_DEG,
    mode="RAMP")
check("the latch can be armed before acquisition", h2.lock_armed)
hunt(h2, 1.5, lambda t: -4.0, lambda t: 3.0, lambda t: 140.0)
check("the null channel is silent while hunting", h2.beat == 0.0, f"beat env {h2.beat:.3f}")
check("...and no lock chime can fire from there", h2.lock_fires == 0,
      "an 'aligned' chime from beside the machine would be a lie")
check("acquisition clears a previously armed latch", not h2.lock_armed)
# Coming round into the corridor is what brings it in, which is the cue that there is now a
# line to be square to.
run(h2, 1.0, lambda t: 6.0, lambda t: 0.5, mode="RAMP")
check("reaching the approach brings the null channel in", h2.beat > 0.9,
      f"beat env {h2.beat:.3f}")
check("entering already aligned cannot produce a second cue", h2.lock_fires == 0)
print()

if failures:
    print(f"{len(failures)} FAILURE(S): " + ", ".join(failures))
    sys.exit(1)
print("all checks passed")
