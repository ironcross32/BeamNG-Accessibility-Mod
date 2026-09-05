"""Replay the loader-implement tone state machines against a fake clock.

Mirrors the render block in audio.py so the ground-proximity FM ratio/index curve, the
jacking ramp back onto an integer, the quarter-tone tilt quantiser and the shared fade
gate can be checked without an audio device or the game running. Run after touching that
block:

    python diagnostic/implement_tone_sim.py

The tuning constants are imported from audio.py rather than copied, so hand-tuning there
can't silently invalidate these checks. Only the *logic* is duplicated here.
"""
import math
import os
import random
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from audio import (  # noqa: E402
    IMPL_GROUND_FAR_M,
    IMPL_GROUND_CARRIER_HZ,
    IMPL_GROUND_RATIO_BASE,
    IMPL_GROUND_RATIO_DRIFT,
    IMPL_GROUND_DIRT_CURVE,
    IMPL_GROUND_INDEX_LO,
    IMPL_GROUND_INDEX_HI,
    IMPL_GROUND_JACK_FULL_M,
    IMPL_GROUND_JACK_OCTAVES,
    IMPL_GROUND_JACK_INDEX,
    IMPL_GROUND_CLOSING_MPS,
    IMPL_TILT_LEVEL_HZ,
    IMPL_TILT_STEPS_PER_OCT,
    IMPL_TILT_FALLBACK_SPAN_DEG,
    IMPL_TILT_MIN_SPAN_DEG,
    IMPL_TILT_SPAN_SWAP_DEG,
    IMPL_TILT_STOP_CONFIRM_DEG,
    IMPL_STREAM_PAN,
    IMPL_GROUND_GAIN_L,
    IMPL_GROUND_GAIN_R,
    IMPL_TILT_GAIN_L,
    IMPL_TILT_GAIN_R,
    IMPL_TILT_GLIDE_TAU,
    IMPL_GATE_ON,
    IMPL_GATE_OFF,
    IMPL_CLOSING_TAU,
    IMPL_CLOSING_STALE_S,
    IMPL_GATE_ATTACK_TAU,
    IMPL_GATE_REL_TAU,
    IMPL_GATE_CUT,
    IMPL_GROUND_MAX_DB,
    IMPL_GROUND_SPAN_DB,
    IMPL_TILT_MAX_DB,
    IMPL_TILT_SPAN_DB,
)

SR = 48000
FRAMES = 512
BLOCK_S = FRAMES / SR

failures = []


def check(label, ok, detail=""):
    print(f"   {label}: {'OK' if ok else 'FAIL'}{(' - ' + detail) if detail else ''}")
    if not ok:
        failures.append(label)


def clamp(v, lo=0.0, hi=1.0):
    return max(lo, min(hi, v))


def ground_params(clearance, lift):
    """The ratio/index/carrier triple for one block. Pure function, no state."""
    prox = 1.0 - clamp(clearance / IMPL_GROUND_FAR_M)
    dirt = prox**IMPL_GROUND_DIRT_CURVE
    jack = clamp(lift / IMPL_GROUND_JACK_FULL_M)
    jb = jack * jack * (3.0 - 2.0 * jack)
    ratio_dirty = IMPL_GROUND_RATIO_BASE + IMPL_GROUND_RATIO_DRIFT * dirt
    ratio = ratio_dirty + (IMPL_GROUND_RATIO_BASE + 1.0 - ratio_dirty) * jb
    index_dirty = (
        IMPL_GROUND_INDEX_LO + (IMPL_GROUND_INDEX_HI - IMPL_GROUND_INDEX_LO) * dirt
    )
    index = index_dirty + (IMPL_GROUND_JACK_INDEX - index_dirty) * jb
    carrier = IMPL_GROUND_CARRIER_HZ * (2.0 ** (jack * IMPL_GROUND_JACK_OCTAVES))
    return ratio, index, carrier


def tilt_step(deg, span_up, span_dn):
    span = span_up if deg >= 0 else span_dn
    if span < IMPL_TILT_MIN_SPAN_DEG:
        span = IMPL_TILT_FALLBACK_SPAN_DEG
    return int(round(IMPL_TILT_STEPS_PER_OCT * clamp(deg / span, -1.0, 1.0)))


class Gate:
    """The shared fade gate: Schmitt trigger, plus the properly-timebased closing rate."""

    def __init__(self):
        self.env = 0.0
        self.open = False
        self.prev_clearance = None
        self.clear_elapsed = 0.0
        self.closing = 0.0

    def block(self, activity, clearance, ground_ok=True):
        if not ground_ok:
            self.prev_clearance = None
            self.clear_elapsed = 0.0
            self.closing = 0.0
        else:
            self.clear_elapsed += BLOCK_S
            if clearance != self.prev_clearance:
                if self.prev_clearance is not None and self.clear_elapsed > 1e-4:
                    raw = (clearance - self.prev_clearance) / self.clear_elapsed
                    alpha = 1.0 - math.exp(-self.clear_elapsed / IMPL_CLOSING_TAU)
                    self.closing += alpha * (raw - self.closing)
                self.prev_clearance = clearance
                self.clear_elapsed = 0.0
            elif self.clear_elapsed > IMPL_CLOSING_STALE_S:
                self.closing = 0.0

        target = 1.0 if self.open else 0.0
        if activity > IMPL_GATE_ON or self.closing < -IMPL_GROUND_CLOSING_MPS:
            target = 1.0
        elif activity < IMPL_GATE_OFF:
            target = 0.0
        self.open = target > 0.0

        tau = IMPL_GATE_ATTACK_TAU if target > self.env else IMPL_GATE_REL_TAU
        end = self.env + (1.0 - math.exp(-BLOCK_S / tau)) * (target - self.env)
        if end < IMPL_GATE_CUT and target == 0.0:
            end = 0.0
        self.env = end
        return end


TELEMETRY_HZ = 20.0  # the mod's implement tick; the audio callback runs ~10x faster


def feed(gate, seconds, activity_fn, clearance_fn):
    """Drive the gate at audio rate while the telemetry values refresh at 20 Hz.

    Reproducing that rate mismatch is the whole point: the first version measured the
    closing rate against the audio callback's own dt, which overstated it about fivefold
    and let raycast jitter hold the gate open.
    """
    envs = []
    t = 0.0
    next_sample = 0.0
    act, clear = activity_fn(0.0), clearance_fn(0.0)
    while t < seconds:
        if t >= next_sample:
            act, clear = activity_fn(t), clearance_fn(t)
            next_sample += 1.0 / TELEMETRY_HZ
        envs.append(gate.block(act, clear))
        t += BLOCK_S
    return envs


def transitions(envs, thresh=0.5):
    """How many times the gate crossed audible/silent. Chatter shows up here."""
    n, above = 0, envs[0] > thresh
    for e in envs[1:]:
        if (e > thresh) != above:
            above = not above
            n += 1
    return n


print(
    f"tuning: ground far {IMPL_GROUND_FAR_M} m  carrier {IMPL_GROUND_CARRIER_HZ:.0f} Hz  "
    f"ratio {IMPL_GROUND_RATIO_BASE}+{IMPL_GROUND_RATIO_DRIFT} curve {IMPL_GROUND_DIRT_CURVE}  "
    f"index {IMPL_GROUND_INDEX_LO}-{IMPL_GROUND_INDEX_HI}\n"
    f"        jack {IMPL_GROUND_JACK_FULL_M} m / {IMPL_GROUND_JACK_OCTAVES} oct  "
    f"tilt {IMPL_TILT_LEVEL_HZ:.0f} Hz +/-{IMPL_TILT_STEPS_PER_OCT} quarter tones  "
    f"gate {IMPL_GATE_ATTACK_TAU}/{IMPL_GATE_REL_TAU} s"
)
print()

# -----------------------------------------------------------------------------------------
print("1. ground tone: ratio starts on the integer and never reaches the next one")
r_far, i_far, c_far = ground_params(IMPL_GROUND_FAR_M, 0.0)
check(
    "clean at max range",
    abs(r_far - IMPL_GROUND_RATIO_BASE) < 1e-6,
    f"ratio={r_far:.6f}",
)
worst = max(ground_params(c, 0.0)[0] for c in np.linspace(0.0, IMPL_GROUND_FAR_M, 400))
check(
    "never reaches base+1 while approaching",
    worst < IMPL_GROUND_RATIO_BASE + 1.0 - 1e-9,
    f"max ratio approaching = {worst:.4f} (base+1 would sound CLEAN at contact)",
)
check(
    "drift constant stays under 0.5",
    IMPL_GROUND_RATIO_DRIFT < 0.5,
    f"IMPL_GROUND_RATIO_DRIFT={IMPL_GROUND_RATIO_DRIFT}",
)

print("2. ground tone: grit is legible from ~1 m, not crammed into the last few cm")
r_half = ground_params(IMPL_GROUND_FAR_M / 2.0, 0.0)[0]
frac = (r_half - IMPL_GROUND_RATIO_BASE) / IMPL_GROUND_RATIO_DRIFT
check(
    "at half range the drift is well under way",
    frac > 0.15,
    f"{frac * 100:.0f}% of full drift at {IMPL_GROUND_FAR_M / 2.0:.2f} m",
)

print("3. ground tone: jacking cleans the ratio up and raises the carrier")
r_jack, i_jack, c_jack = ground_params(0.0, IMPL_GROUND_JACK_FULL_M)
check(
    "ratio lands on the next integer at full jack",
    abs(r_jack - (IMPL_GROUND_RATIO_BASE + 1.0)) < 1e-3,
    f"ratio={r_jack:.6f}",
)
check(
    "index settles to the clean value",
    abs(i_jack - IMPL_GROUND_JACK_INDEX) < 1e-3,
    f"index={i_jack:.4f}",
)
expect_c = IMPL_GROUND_CARRIER_HZ * (2.0**IMPL_GROUND_JACK_OCTAVES)
check(
    "carrier rises by the full octave span",
    abs(c_jack - expect_c) < 1e-6,
    f"{IMPL_GROUND_CARRIER_HZ:.0f} -> {c_jack:.1f} Hz",
)
c_approach = {ground_params(c, 0.0)[2] for c in np.linspace(0.0, IMPL_GROUND_FAR_M, 50)}
check(
    "carrier is FIXED during approach (pitch only ever means lift)",
    len(c_approach) == 1,
    f"{len(c_approach)} distinct carriers seen",
)

print("4. ground tone: the ratio approaches the integer monotonically as lift grows")
ratios = [ground_params(0.0, IMPL_GROUND_JACK_FULL_M * t)[0] for t in np.linspace(0, 1, 200)]
check(
    "monotonic ramp onto the integer",
    all(b >= a - 1e-12 for a, b in zip(ratios, ratios[1:])),
    "a non-monotonic ramp would sound like it was hunting",
)

# -----------------------------------------------------------------------------------------
print("5. tilt tone: the quarter-tone scale")
span = IMPL_TILT_FALLBACK_SPAN_DEG
freqs = []
for st in range(-IMPL_TILT_STEPS_PER_OCT, IMPL_TILT_STEPS_PER_OCT + 1):
    freqs.append(IMPL_TILT_LEVEL_HZ * (2.0 ** (st / float(IMPL_TILT_STEPS_PER_OCT))))
check("49 distinct frequencies", len(set(round(f, 6) for f in freqs)) == 49, f"{len(freqs)} steps")
check("level is exactly 400 Hz", abs(freqs[IMPL_TILT_STEPS_PER_OCT] - 400.0) < 1e-9)
check("down stop is exactly 200 Hz", abs(freqs[0] - 200.0) < 1e-9)
check("up stop is exactly 800 Hz", abs(freqs[-1] - 800.0) < 1e-9)
ratios_t = [b / a for a, b in zip(freqs, freqs[1:])]
step_ratio = 2.0 ** (1.0 / IMPL_TILT_STEPS_PER_OCT)
check(
    "adjacent steps share one fixed interval",
    max(abs(r - step_ratio) for r in ratios_t) < 1e-9,
    f"2^(1/{IMPL_TILT_STEPS_PER_OCT}) = {step_ratio:.9f}",
)
check("level reads step 0", tilt_step(0.0, span, span) == 0)
check("full up reads step +24", tilt_step(span, span, span) == IMPL_TILT_STEPS_PER_OCT)
check("full down reads step -24", tilt_step(-span, span, span) == -IMPL_TILT_STEPS_PER_OCT)
check(
    "past the stop clamps rather than running off the scale",
    tilt_step(span * 3, span, span) == IMPL_TILT_STEPS_PER_OCT,
)

print("6. tilt tone: asymmetric learned stops still put level at 400 Hz")
up, dn = 32.0, 18.0  # deliberately lopsided travel
check("level is step 0 on a lopsided range", tilt_step(0.0, up, dn) == 0)
check("up stop still reaches +24", tilt_step(up, up, dn) == IMPL_TILT_STEPS_PER_OCT)
check("down stop still reaches -24", tilt_step(-dn, up, dn) == -IMPL_TILT_STEPS_PER_OCT)

print("6b. THE FIRST-STROKE SNAP: a running extreme is not a mechanical stop")


class TiltSpanLearner:
    """Mirror of the span learning in audio.py's tilt block."""

    def __init__(self, confirm=True, swap=True):
        self.seen_up = self.seen_dn = 0.0
        self.stop_up = self.stop_dn = 0.0
        self.span_up = self.span_dn = 0.0
        self.confirm = confirm  # False = the old "running max IS the span" behaviour
        self.swap = swap  # False = adopt a new span the instant it is confirmed

    def feed(self, deg):
        if self.confirm:
            if deg > self.seen_up:
                self.seen_up = deg
            elif deg < self.seen_up - IMPL_TILT_STOP_CONFIRM_DEG:
                self.stop_up = self.seen_up
            if -deg > self.seen_dn:
                self.seen_dn = -deg
            elif -deg < self.seen_dn - IMPL_TILT_STOP_CONFIRM_DEG:
                self.stop_dn = self.seen_dn
        else:
            self.stop_up = self.seen_up = max(self.seen_up, deg)
            self.stop_dn = self.seen_dn = max(self.seen_dn, -deg)
        if (not self.swap) or abs(deg) < IMPL_TILT_SPAN_SWAP_DEG:
            self.span_up, self.span_dn = self.stop_up, self.stop_dn
        return tilt_step(deg, self.span_up, self.span_dn)


def sweep(learner, angles):
    return [learner.feed(a) for a in angles]


REAL_UP_STOP = 28.0  # what the WL-40's bucket actually reaches; the fallback assumes 50
first_stroke = [REAL_UP_STOP * i / 60.0 for i in range(61)]  # 0 -> stop, smoothly

old = sweep(TiltSpanLearner(confirm=False, swap=False), list(first_stroke))
new = sweep(TiltSpanLearner(), list(first_stroke))
check(
    "the old form pins the whole first stroke to the top of the scale",
    old.count(IMPL_TILT_STEPS_PER_OCT) > len(old) // 2,
    f"{old.count(IMPL_TILT_STEPS_PER_OCT)}/{len(old)} samples at +24 - this was the snap",
)
check(
    "the first stroke now climbs one step at a time",
    max(abs(b - a) for a, b in zip(new, new[1:])) <= 1,
    f"biggest single-tick jump {max(abs(b - a) for a, b in zip(new, new[1:]))} steps"
    f" (old form: {max(abs(b - a) for a, b in zip(old, old[1:]))})",
)
check(
    "and it reads the fallback scale, not a pegged one",
    new[-1] == tilt_step(REAL_UP_STOP, IMPL_TILT_FALLBACK_SPAN_DEG, IMPL_TILT_FALLBACK_SPAN_DEG),
    f"step {new[-1]} at the real stop, fallback span in use",
)

# Full stroke out and back, then out again: the second pass must use the learned stop.
learner = TiltSpanLearner()
sweep(learner, first_stroke + list(reversed(first_stroke)))
check(
    "a completed stroke confirms the stop and rescales the whole range",
    learner.span_up == REAL_UP_STOP and learner.feed(REAL_UP_STOP) == IMPL_TILT_STEPS_PER_OCT,
    f"span {learner.span_up:.1f} deg -> the real stop now reaches +24",
)

# The rescale itself must be inaudible: it may only land where both scales agree on step 0.
learner = TiltSpanLearner()
angles = first_stroke + list(reversed(first_stroke))
steps = sweep(learner, angles)
check(
    "the rescale never shows up as a jump in the readout",
    max(abs(b - a) for a, b in zip(steps, steps[1:])) <= 1,
    "adopting the learned span at angle would jump it by ~9 steps",
)

print("6c. the two voices must not be rendered from the same point in space")
check(
    "the ground tone leans left and the tilt tone leans right",
    IMPL_GROUND_GAIN_L > IMPL_GROUND_GAIN_R and IMPL_TILT_GAIN_R > IMPL_TILT_GAIN_L,
    f"ground L/R {IMPL_GROUND_GAIN_L:.3f}/{IMPL_GROUND_GAIN_R:.3f},"
    f" tilt {IMPL_TILT_GAIN_L:.3f}/{IMPL_TILT_GAIN_R:.3f}"
    " - centring both is what made them fuse into one perceived tone",
)
check(
    "the split is exactly mirrored, so neither voice is quietly favoured",
    abs(IMPL_GROUND_GAIN_L - IMPL_TILT_GAIN_R) < 1e-12
    and abs(IMPL_GROUND_GAIN_R - IMPL_TILT_GAIN_L) < 1e-12,
)
check(
    "equal power: total loudness is unchanged from the centred version",
    abs(IMPL_GROUND_GAIN_L**2 + IMPL_GROUND_GAIN_R**2 - 1.0) < 1e-12,
    "a linear pan here would quietly lose 3 dB at the configured level",
)
check(
    "the offset is constant, so it can never be read as a bearing",
    isinstance(IMPL_GROUND_GAIN_L, float) and 0.0 < IMPL_STREAM_PAN < 1.0,
    f"pan {IMPL_STREAM_PAN} - hard-panning would make each voice mono-eared instead",
)

print("7. tilt tone: the glide is fast enough that a step reads as a step")
beta = 1.0 - math.exp(-BLOCK_S / IMPL_TILT_GLIDE_TAU)
inc = 400.0 / SR
target = 400.0 * step_ratio / SR
blocks = 0
while abs(inc - target) > abs(target - 400.0 / SR) * 0.1 and blocks < 100:
    inc += beta * (target - inc)
    blocks += 1
check(
    "a one-step change settles within ~2 buffers",
    blocks <= 2,
    f"{blocks} buffers ({blocks * BLOCK_S * 1000:.1f} ms)",
)

# -----------------------------------------------------------------------------------------
print("8. fade gate: sharp in, ~1 s out")
g = Gate()
n = 0
while g.block(1.0, 1.0) < 0.9 and n < 500:
    n += 1
check(
    "reaches 0.9 within 100 ms of actuation",
    n * BLOCK_S <= 0.1,
    f"{n * BLOCK_S * 1000:.0f} ms",
)
n = 0
while g.block(0.0, 1.0) > 0.0 and n < 2000:
    n += 1
check(
    "silent about a second after release",
    0.5 <= n * BLOCK_S <= 2.0,
    f"{n * BLOCK_S:.2f} s",
)

print("9. fade gate: a momentary joystick release must not drop it")
g = Gate()
for _ in range(60):
    g.block(1.0, 1.0)
low = 0.0
for _ in range(int(0.2 / BLOCK_S)):
    low = g.block(0.35, 1.0)
check("gate stays open through a 200 ms gap", low > 0.9, f"env={low:.3f}")

print("10. fade gate: approaching the ground opens it with the joystick released")
g = Gate()
env = feed(g, 3.0, lambda t: 0.0, lambda t: 2.0 - 0.6 * t)[-1]
check(
    "closing on the ground alone opens the gate",
    env > 0.9,
    f"env={env:.3f} - going deaf here would silence the approach that matters most",
)

print("11. THE CHATTER BUG: parked machine, noisy telemetry, must stay silent")
# A soft-body loader never sits perfectly still. The rams breathe and the ground raycast
# jitters by a few mm; both arrive at 20 Hz while the audio callback runs at ~100 Hz.
rng = random.Random(0xB0A7)
jitter = {}


def noisy_clearance(t):
    k = round(t * TELEMETRY_HZ)
    if k not in jitter:
        jitter[k] = 0.85 + rng.uniform(-0.004, 0.004)  # +/-4 mm of raycast noise
    return jitter[k]


g = Gate()
# Residual activity below the Lua deadband: the ram jitter has already been deadbanded
# away on the mod side, so what arrives here is a trickle, not zero.
envs = feed(g, 20.0, lambda t: rng.uniform(0.0, IMPL_GATE_OFF * 0.9), noisy_clearance)
check(
    "silent for 20 s of idling",
    max(envs) == 0.0,
    f"peak env={max(envs):.4f}, {transitions(envs)} transitions",
)

print("12. chatter: idle jitter must not sneak past the closing-rate term either")
g = Gate()
envs = feed(g, 20.0, lambda t: 0.0, noisy_clearance)
check(
    "raycast jitter alone never opens the gate",
    max(envs) == 0.0,
    f"peak env={max(envs):.4f} - measuring the rate against the audio dt broke this",
)

print("13. chatter: activity hovering at the threshold must not flutter")
g = Gate()
# Sits right between the two Schmitt levels, which is where a single threshold flaps.
envs = feed(
    g,
    20.0,
    lambda t: (IMPL_GATE_ON + IMPL_GATE_OFF) / 2.0 + 0.005 * math.sin(t * 9.0),
    lambda t: 1.0,
)
check(
    "no flutter between the two thresholds",
    transitions(envs) <= 1,
    f"{transitions(envs)} audible/silent transitions in 20 s",
)

print("14. real work still reads as continuous, not chopped up")
g = Gate()
# Lifting for 4 s with two brief joystick releases, the way a real cycle goes.
def work(t):
    if 1.5 <= t < 1.7 or 2.8 <= t < 3.0:
        return 0.30  # released, but the ram is still coasting
    return 0.9


envs = feed(g, 4.0, work, lambda t: 1.0)
check(
    "one continuous sound through the whole cycle",
    transitions(envs) == 1,
    f"{transitions(envs)} transitions (should just be the initial rise)",
)

print("15. both level maps rise with urgency at ANY configured level")
# The map used to be MIN_DB + (configured - MIN_DB) * t, which reads as "interpolate from the
# floor up to the user's level" and is correct only while the user's level sits above the
# floor. The configurator allows anything down to -120 dBFS, and below the constant the
# expression inverts: at -40 the ground tone came out at -30 two metres off the ground and -40
# at contact, so it got quieter as the bucket approached and loudest when there was nothing to
# report. Both settings exist to make the tone less obtrusive, so the broken half of the range
# is exactly the half someone bothered to change it for.


def ground_db(level_db, t):
    return level_db - IMPL_GROUND_SPAN_DB * (1.0 - t)


def tilt_db(level_db, t):
    return level_db - IMPL_TILT_SPAN_DB * (1.0 - t)


for name, fn, default in (
    ("ground", ground_db, IMPL_GROUND_MAX_DB),
    ("tilt", tilt_db, IMPL_TILT_MAX_DB),
):
    for level in (default, -45.0, -80.0):
        ts = [i / 40.0 for i in range(41)]
        dbs = [fn(level, t) for t in ts]
        rising = all(b >= a - 1e-9 for a, b in zip(dbs, dbs[1:]))
        check(
            f"{name} tone at {level:.1f} dBFS rises toward the loud end",
            rising and dbs[-1] > dbs[0],
            f"{dbs[0]:.1f} dB at rest -> {dbs[-1]:.1f} dB at full",
        )
        check(
            f"{name} tone at {level:.1f} dBFS peaks at exactly the configured level",
            abs(dbs[-1] - level) < 1e-9,
            f"peak {dbs[-1]:.2f} dB",
        )

# And the default config must be bit-identical to the old map, or this is a retune in disguise.
check(
    "the default ground level is unchanged by the rewrite",
    abs(ground_db(IMPL_GROUND_MAX_DB, 0.0) - (IMPL_GROUND_MAX_DB - IMPL_GROUND_SPAN_DB))
    < 1e-9
    and abs(ground_db(IMPL_GROUND_MAX_DB, 1.0) - IMPL_GROUND_MAX_DB) < 1e-9,
    f"{ground_db(IMPL_GROUND_MAX_DB, 0.0):.1f} .. {ground_db(IMPL_GROUND_MAX_DB, 1.0):.1f} dB",
)

print()
if failures:
    print(f"{len(failures)} FAILURE(S): " + ", ".join(failures))
    sys.exit(1)
print("all checks passed")
