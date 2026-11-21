//! `diagnostic/implement_tone_sim.py`: the loader-implement tone state machines replayed
//! against a fake clock, same scenario numbers and names. `Gate` and `TiltSpanLearner` are
//! the sim's mirrors of the render block; wherever the sim drives the gate or the learner,
//! the real Rust callback is driven beside it (`Twin`) and must agree exactly.

mod audio_harness;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use audio_harness::{Harness, harness};
use beam_audio::consts::{
    IMPL_CLOSING_STALE_S, IMPL_CLOSING_TAU, IMPL_GATE_ATTACK_TAU, IMPL_GATE_CUT, IMPL_GATE_OFF,
    IMPL_GATE_ON, IMPL_GATE_REL_TAU, IMPL_GROUND_CARRIER_HZ, IMPL_GROUND_CLOSING_MPS,
    IMPL_GROUND_DIRT_CURVE, IMPL_GROUND_FAR_M, IMPL_GROUND_GAIN_L, IMPL_GROUND_GAIN_R,
    IMPL_GROUND_INDEX_HI, IMPL_GROUND_INDEX_LO, IMPL_GROUND_JACK_FULL_M, IMPL_GROUND_JACK_INDEX,
    IMPL_GROUND_JACK_OCTAVES, IMPL_GROUND_MAX_DB, IMPL_GROUND_RATIO_BASE, IMPL_GROUND_RATIO_DRIFT,
    IMPL_GROUND_SPAN_DB, IMPL_STREAM_PAN, IMPL_TILT_FALLBACK_SPAN_DEG, IMPL_TILT_GAIN_L,
    IMPL_TILT_GAIN_R, IMPL_TILT_GLIDE_TAU, IMPL_TILT_LEVEL_HZ, IMPL_TILT_MAX_DB,
    IMPL_TILT_MIN_SPAN_DEG, IMPL_TILT_SPAN_DB, IMPL_TILT_SPAN_SWAP_DEG, IMPL_TILT_STEPS_PER_OCT,
    IMPL_TILT_STOP_CONFIRM_DEG,
};
use beam_audio::controller::TelemetryState;
use beam_audio::dsp;

const SR: f64 = 48000.0;
const FRAMES: usize = 512;
const BLOCK_S: f64 = FRAMES as f64 / SR;
const STEPS: i64 = IMPL_TILT_STEPS_PER_OCT;
const TELEMETRY_HZ: f64 = 20.0; // the mod's implement tick; the callback runs ~5x faster

fn check(label: &str, ok: bool, detail: &str) {
    eprintln!(
        "   {label}: {}{}",
        if ok { "OK" } else { "FAIL" },
        if detail.is_empty() {
            String::new()
        } else {
            format!(" - {detail}")
        }
    );
    assert!(ok, "{label}: {detail}");
}

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    let m = if hi < v { hi } else { v };
    if m > lo { m } else { lo }
}

/// The ratio/index/carrier triple for one block. Pure function, no state.
fn ground_params(clearance: f64, lift: f64) -> (f64, f64, f64) {
    let prox = 1.0 - clamp(clearance / IMPL_GROUND_FAR_M, 0.0, 1.0);
    let dirt = prox.powf(IMPL_GROUND_DIRT_CURVE);
    let jack = clamp(lift / IMPL_GROUND_JACK_FULL_M, 0.0, 1.0);
    let jb = jack * jack * (3.0 - 2.0 * jack);
    let ratio_dirty = IMPL_GROUND_RATIO_BASE + IMPL_GROUND_RATIO_DRIFT * dirt;
    let ratio = ratio_dirty + (IMPL_GROUND_RATIO_BASE + 1.0 - ratio_dirty) * jb;
    let index_dirty = IMPL_GROUND_INDEX_LO + (IMPL_GROUND_INDEX_HI - IMPL_GROUND_INDEX_LO) * dirt;
    let index = index_dirty + (IMPL_GROUND_JACK_INDEX - index_dirty) * jb;
    let carrier = IMPL_GROUND_CARRIER_HZ * 2.0f64.powf(jack * IMPL_GROUND_JACK_OCTAVES);
    (ratio, index, carrier)
}

fn tilt_step(deg: f64, span_up: f64, span_dn: f64) -> i64 {
    let mut span = if deg >= 0.0 { span_up } else { span_dn };
    if span < IMPL_TILT_MIN_SPAN_DEG {
        span = IMPL_TILT_FALLBACK_SPAN_DEG;
    }
    (STEPS as f64 * clamp(deg / span, -1.0, 1.0)).round_ties_even() as i64
}

/// The shared fade gate: Schmitt trigger, plus the properly time-based closing rate.
struct Gate {
    env: f64,
    open: bool,
    prev_clearance: Option<f64>,
    clear_elapsed: f64,
    closing: f64,
}

impl Gate {
    fn new() -> Self {
        Gate {
            env: 0.0,
            open: false,
            prev_clearance: None,
            clear_elapsed: 0.0,
            closing: 0.0,
        }
    }

    fn block(&mut self, activity: f64, clearance: f64) -> f64 {
        self.clear_elapsed += BLOCK_S;
        if Some(clearance) != self.prev_clearance {
            if let Some(prev) = self.prev_clearance {
                if self.clear_elapsed > 1e-4 {
                    let raw = (clearance - prev) / self.clear_elapsed;
                    let alpha = 1.0 - (-self.clear_elapsed / IMPL_CLOSING_TAU).exp();
                    self.closing += alpha * (raw - self.closing);
                }
            }
            self.prev_clearance = Some(clearance);
            self.clear_elapsed = 0.0;
        } else if self.clear_elapsed > IMPL_CLOSING_STALE_S {
            self.closing = 0.0;
        }

        let mut target = if self.open { 1.0 } else { 0.0 };
        if activity > IMPL_GATE_ON || self.closing < -IMPL_GROUND_CLOSING_MPS {
            target = 1.0;
        } else if activity < IMPL_GATE_OFF {
            target = 0.0;
        }
        self.open = target > 0.0;
        let tau = if target > self.env {
            IMPL_GATE_ATTACK_TAU
        } else {
            IMPL_GATE_REL_TAU
        };
        let mut end = self.env + (1.0 - (-BLOCK_S / tau).exp()) * (target - self.env);
        if end < IMPL_GATE_CUT && target == 0.0 {
            end = 0.0;
        }
        self.env = end;
        end
    }
}

/// The mirror gate and the real callback, stepped together. The real voice also needs
/// the implement-present flag (1) and ground-valid flag (4), which the mirror assumes.
struct Twin {
    gate: Gate,
    h: Harness,
}

impl Twin {
    fn new() -> Self {
        Twin {
            gate: Gate::new(),
            h: harness(None),
        }
    }

    fn block(&mut self, activity: f64, clearance: f64) -> f64 {
        let env = self.gate.block(activity, clearance);
        self.h.ctrl.update_telemetry_state(&TelemetryState {
            implement_flags: Some(5),
            implement_activity: Some(activity),
            implement_min_clearance: Some(clearance),
            ..TelemetryState::default()
        });
        self.h.ctrl.render_block(FRAMES);
        let v = self.h.ctrl.voice.lock();
        assert_eq!(
            v._impl_gate_env, env,
            "real gate env diverged from the mirror"
        );
        assert_eq!(
            v._impl_closing, self.gate.closing,
            "real closing rate diverged"
        );
        env
    }
}

/// Drive the gate at audio rate while the telemetry values refresh at 20 Hz.
fn feed(
    twin: &mut Twin,
    seconds: f64,
    activity_fn: &mut dyn FnMut(f64) -> f64,
    clearance_fn: &mut dyn FnMut(f64) -> f64,
) -> Vec<f64> {
    let mut envs = Vec::new();
    let mut t = 0.0;
    let mut next_sample = 0.0;
    let mut act = activity_fn(0.0);
    let mut clear = clearance_fn(0.0);
    while t < seconds {
        if t >= next_sample {
            act = activity_fn(t);
            clear = clearance_fn(t);
            next_sample += 1.0 / TELEMETRY_HZ;
        }
        envs.push(twin.block(act, clear));
        t += BLOCK_S;
    }
    envs
}

/// How many times the gate crossed audible/silent. Chatter shows up here.
fn transitions(envs: &[f64]) -> usize {
    let thresh = 0.5;
    let mut n = 0;
    let mut above = envs[0] > thresh;
    for &e in &envs[1..] {
        if (e > thresh) != above {
            above = !above;
            n += 1;
        }
    }
    n
}

/// CPython's `random.Random` (MT19937, `init_by_array` seeding, 53-bit `random()`).
struct PyRandom {
    mt: [u32; 624],
    idx: usize,
}

impl PyRandom {
    fn new(seed: u32) -> Self {
        let mut r = PyRandom {
            mt: [0; 624],
            idx: 624,
        };
        r.init_genrand(19650218);
        let key = [seed];
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..624.max(key.len()) {
            let prev = r.mt[i - 1] ^ (r.mt[i - 1] >> 30);
            r.mt[i] = (r.mt[i] ^ prev.wrapping_mul(1664525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                r.mt[0] = r.mt[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..623 {
            let prev = r.mt[i - 1] ^ (r.mt[i - 1] >> 30);
            r.mt[i] = (r.mt[i] ^ prev.wrapping_mul(1566083941)).wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                r.mt[0] = r.mt[623];
                i = 1;
            }
        }
        r.mt[0] = 0x8000_0000;
        r
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..624 {
            let prev = self.mt[i - 1] ^ (self.mt[i - 1] >> 30);
            self.mt[i] = 1812433253u32.wrapping_mul(prev).wrapping_add(i as u32);
        }
        self.idx = 624;
    }

    fn genrand(&mut self) -> u32 {
        if self.idx >= 624 {
            for k in 0..624 {
                let y = (self.mt[k] & 0x8000_0000) | (self.mt[(k + 1) % 624] & 0x7fff_ffff);
                let mut v = self.mt[(k + 397) % 624] ^ (y >> 1);
                if y & 1 != 0 {
                    v ^= 0x9908_b0df;
                }
                self.mt[k] = v;
            }
            self.idx = 0;
        }
        let mut y = self.mt[self.idx];
        self.idx += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    fn random(&mut self) -> f64 {
        let a = (self.genrand() >> 5) as f64;
        let b = (self.genrand() >> 6) as f64;
        (a * 67108864.0 + b) * (1.0 / 9007199254740992.0)
    }

    fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.random()
    }
}

#[test]
fn py_random_matches_cpython() {
    let mut r = PyRandom::new(0xB0A7);
    assert_eq!(r.random(), 0.21580663052381344);
    assert_eq!(r.uniform(-0.004, 0.004), 0.0002807933437343523);
}

#[test]
fn s01_ground_ratio_starts_on_the_integer_and_never_reaches_the_next() {
    let (r_far, _, _) = ground_params(IMPL_GROUND_FAR_M, 0.0);
    check(
        "clean at max range",
        (r_far - IMPL_GROUND_RATIO_BASE).abs() < 1e-6,
        "",
    );
    let worst = dsp::linspace(0.0, IMPL_GROUND_FAR_M, 400, true)
        .iter()
        .map(|&c| ground_params(c, 0.0).0)
        .fold(f64::MIN, f64::max);
    check(
        "never reaches base+1 while approaching",
        worst < IMPL_GROUND_RATIO_BASE + 1.0 - 1e-9,
        &format!("max ratio approaching = {worst:.4}"),
    );
    check(
        "drift constant stays under 0.5",
        IMPL_GROUND_RATIO_DRIFT < 0.5,
        "",
    );
}

#[test]
fn s02_ground_grit_is_legible_from_about_a_metre() {
    let r_half = ground_params(IMPL_GROUND_FAR_M / 2.0, 0.0).0;
    let frac = (r_half - IMPL_GROUND_RATIO_BASE) / IMPL_GROUND_RATIO_DRIFT;
    check(
        "at half range the drift is well under way",
        frac > 0.15,
        &format!("{frac}"),
    );
}

#[test]
fn s03_jacking_cleans_the_ratio_and_raises_the_carrier() {
    let (r_jack, i_jack, c_jack) = ground_params(0.0, IMPL_GROUND_JACK_FULL_M);
    check(
        "ratio lands on the next integer at full jack",
        (r_jack - (IMPL_GROUND_RATIO_BASE + 1.0)).abs() < 1e-3,
        "",
    );
    check(
        "index settles to the clean value",
        (i_jack - IMPL_GROUND_JACK_INDEX).abs() < 1e-3,
        "",
    );
    let expect_c = IMPL_GROUND_CARRIER_HZ * 2.0f64.powf(IMPL_GROUND_JACK_OCTAVES);
    check(
        "carrier rises by the full octave span",
        (c_jack - expect_c).abs() < 1e-6,
        "",
    );
    let mut carriers: Vec<u64> = dsp::linspace(0.0, IMPL_GROUND_FAR_M, 50, true)
        .iter()
        .map(|&c| ground_params(c, 0.0).2.to_bits())
        .collect();
    carriers.sort();
    carriers.dedup();
    check(
        "carrier is FIXED during approach (pitch only ever means lift)",
        carriers.len() == 1,
        &format!("{} distinct carriers seen", carriers.len()),
    );
}

#[test]
fn s04_ratio_approaches_the_integer_monotonically_as_lift_grows() {
    let ratios: Vec<f64> = dsp::linspace(0.0, 1.0, 200, true)
        .iter()
        .map(|&t| ground_params(0.0, IMPL_GROUND_JACK_FULL_M * t).0)
        .collect();
    check(
        "monotonic ramp onto the integer",
        ratios.windows(2).all(|w| w[1] >= w[0] - 1e-12),
        "",
    );
}

#[test]
fn s05_tilt_quarter_tone_scale() {
    let span = IMPL_TILT_FALLBACK_SPAN_DEG;
    let freqs: Vec<f64> = (-STEPS..=STEPS)
        .map(|st| IMPL_TILT_LEVEL_HZ * 2.0f64.powf(st as f64 / STEPS as f64))
        .collect();
    let mut rounded: Vec<i64> = freqs
        .iter()
        .map(|f| (f * 1e6).round_ties_even() as i64)
        .collect();
    rounded.sort();
    rounded.dedup();
    check("49 distinct frequencies", rounded.len() == 49, "");
    check(
        "level is exactly 400 Hz",
        (freqs[STEPS as usize] - 400.0).abs() < 1e-9,
        "",
    );
    check(
        "down stop is exactly 200 Hz",
        (freqs[0] - 200.0).abs() < 1e-9,
        "",
    );
    check(
        "up stop is exactly 800 Hz",
        (freqs[freqs.len() - 1] - 800.0).abs() < 1e-9,
        "",
    );
    let step_ratio = 2.0f64.powf(1.0 / STEPS as f64);
    check(
        "adjacent steps share one fixed interval",
        freqs
            .windows(2)
            .map(|w| (w[1] / w[0] - step_ratio).abs())
            .fold(0.0, f64::max)
            < 1e-9,
        "",
    );
    check("level reads step 0", tilt_step(0.0, span, span) == 0, "");
    check(
        "full up reads step +24",
        tilt_step(span, span, span) == STEPS,
        "",
    );
    check(
        "full down reads step -24",
        tilt_step(-span, span, span) == -STEPS,
        "",
    );
    check(
        "past the stop clamps rather than running off the scale",
        tilt_step(span * 3.0, span, span) == STEPS,
        "",
    );
}

#[test]
fn s06_asymmetric_learned_stops_still_put_level_at_400_hz() {
    let (up, dn) = (32.0, 18.0);
    check(
        "level is step 0 on a lopsided range",
        tilt_step(0.0, up, dn) == 0,
        "",
    );
    check(
        "up stop still reaches +24",
        tilt_step(up, up, dn) == STEPS,
        "",
    );
    check(
        "down stop still reaches -24",
        tilt_step(-dn, up, dn) == -STEPS,
        "",
    );
}

/// Mirror of the span learning in audio.py's tilt block.
struct TiltSpanLearner {
    seen_up: f64,
    seen_dn: f64,
    stop_up: f64,
    stop_dn: f64,
    span_up: f64,
    span_dn: f64,
    confirm: bool, // false = the old "running max IS the span" behaviour
    swap: bool,    // false = adopt a new span the instant it is confirmed
}

impl TiltSpanLearner {
    fn new(confirm: bool, swap: bool) -> Self {
        TiltSpanLearner {
            seen_up: 0.0,
            seen_dn: 0.0,
            stop_up: 0.0,
            stop_dn: 0.0,
            span_up: 0.0,
            span_dn: 0.0,
            confirm,
            swap,
        }
    }

    fn feed(&mut self, deg: f64) -> i64 {
        if self.confirm {
            if deg > self.seen_up {
                self.seen_up = deg;
            } else if deg < self.seen_up - IMPL_TILT_STOP_CONFIRM_DEG {
                self.stop_up = self.seen_up;
            }
            if -deg > self.seen_dn {
                self.seen_dn = -deg;
            } else if -deg < self.seen_dn - IMPL_TILT_STOP_CONFIRM_DEG {
                self.stop_dn = self.seen_dn;
            }
        } else {
            self.seen_up = if deg > self.seen_up {
                deg
            } else {
                self.seen_up
            };
            self.stop_up = self.seen_up;
            self.seen_dn = if -deg > self.seen_dn {
                -deg
            } else {
                self.seen_dn
            };
            self.stop_dn = self.seen_dn;
        }
        if !self.swap || deg.abs() < IMPL_TILT_SPAN_SWAP_DEG {
            self.span_up = self.stop_up;
            self.span_dn = self.stop_dn;
        }
        tilt_step(deg, self.span_up, self.span_dn)
    }
}

fn sweep(learner: &mut TiltSpanLearner, angles: &[f64]) -> Vec<i64> {
    angles.iter().map(|&a| learner.feed(a)).collect()
}

fn max_jump(steps: &[i64]) -> i64 {
    steps
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .max()
        .unwrap_or(0)
}

const REAL_UP_STOP: f64 = 28.0; // what the WL-40's bucket reaches; the fallback assumes 50

fn first_stroke() -> Vec<f64> {
    (0..61).map(|i| REAL_UP_STOP * i as f64 / 60.0).collect()
}

#[test]
fn s06b_the_first_stroke_snap() {
    let old = sweep(&mut TiltSpanLearner::new(false, false), &first_stroke());
    let new = sweep(&mut TiltSpanLearner::new(true, true), &first_stroke());
    let pinned = old.iter().filter(|&&s| s == STEPS).count();
    check(
        "the old form pins the whole first stroke to the top of the scale",
        pinned > old.len() / 2,
        &format!("{pinned}/{} samples at +24", old.len()),
    );
    check(
        "the first stroke now climbs one step at a time",
        max_jump(&new) <= 1,
        &format!(
            "biggest jump {} (old form: {})",
            max_jump(&new),
            max_jump(&old)
        ),
    );
    check(
        "and it reads the fallback scale, not a pegged one",
        *new.last().unwrap()
            == tilt_step(
                REAL_UP_STOP,
                IMPL_TILT_FALLBACK_SPAN_DEG,
                IMPL_TILT_FALLBACK_SPAN_DEG,
            ),
        "",
    );

    let mut learner = TiltSpanLearner::new(true, true);
    let mut out_back = first_stroke();
    out_back.extend(first_stroke().into_iter().rev());
    sweep(&mut learner, &out_back);
    let span_ok = learner.span_up == REAL_UP_STOP;
    check(
        "a completed stroke confirms the stop and rescales the whole range",
        span_ok && learner.feed(REAL_UP_STOP) == STEPS,
        &format!("span {:.1} deg", learner.span_up),
    );

    let mut learner = TiltSpanLearner::new(true, true);
    let steps = sweep(&mut learner, &out_back);
    check(
        "the rescale never shows up as a jump in the readout",
        max_jump(&steps) <= 1,
        "adopting the learned span at angle would jump it by ~9 steps",
    );
}

/// Not in the Python sim: the same out-and-back strokes through the REAL callback (gate
/// held open, so every block's step is the computed one) give the mirror's steps exactly.
#[test]
fn s06b_real_callback_learns_like_the_mirror() {
    let h = harness(None);
    let mut learner = TiltSpanLearner::new(true, true);
    let mut angles = first_stroke();
    angles.extend(first_stroke().into_iter().rev());
    angles.extend(first_stroke());
    for (i, &deg) in angles.iter().enumerate() {
        h.ctrl.update_telemetry_state(&TelemetryState {
            implement_flags: Some(1),
            implement_activity: Some(0.5),
            implement_tilt_deg: Some(deg),
            ..TelemetryState::default()
        });
        h.ctrl.render_block(FRAMES);
        let want = learner.feed(deg);
        let v = h.ctrl.voice.lock();
        assert_eq!(v._impl_tilt_step, Some(want), "block {i} at {deg} deg");
        assert_eq!(v._impl_tilt_span_up, learner.span_up, "block {i}: span_up");
    }
}

#[test]
fn s06c_the_two_voices_are_not_rendered_from_one_point() {
    check(
        "the ground tone leans left and the tilt tone leans right",
        IMPL_GROUND_GAIN_L > IMPL_GROUND_GAIN_R && IMPL_TILT_GAIN_R > IMPL_TILT_GAIN_L,
        "",
    );
    check(
        "the split is exactly mirrored, so neither voice is quietly favoured",
        (IMPL_GROUND_GAIN_L - IMPL_TILT_GAIN_R).abs() < 1e-12
            && (IMPL_GROUND_GAIN_R - IMPL_TILT_GAIN_L).abs() < 1e-12,
        "",
    );
    check(
        "equal power: total loudness is unchanged from the centred version",
        (IMPL_GROUND_GAIN_L.powi(2) + IMPL_GROUND_GAIN_R.powi(2) - 1.0).abs() < 1e-12,
        "",
    );
    check(
        "the offset is constant, so it can never be read as a bearing",
        0.0 < IMPL_STREAM_PAN && IMPL_STREAM_PAN < 1.0,
        "",
    );
}

#[test]
fn s07_tilt_glide_is_fast_enough_that_a_step_reads_as_a_step() {
    let beta = 1.0 - (-BLOCK_S / IMPL_TILT_GLIDE_TAU).exp();
    let step_ratio = 2.0f64.powf(1.0 / STEPS as f64);
    let mut inc = 400.0 / SR;
    let target = 400.0 * step_ratio / SR;
    let mut blocks = 0;
    while (inc - target).abs() > (target - 400.0 / SR).abs() * 0.1 && blocks < 100 {
        inc += beta * (target - inc);
        blocks += 1;
    }
    check(
        "a one-step change settles within ~2 buffers",
        blocks <= 2,
        &format!("{blocks}"),
    );
}

#[test]
fn s08_fade_gate_sharp_in_about_a_second_out() {
    let mut g = Twin::new();
    let mut n = 0;
    while g.block(1.0, 1.0) < 0.9 && n < 500 {
        n += 1;
    }
    check(
        "reaches 0.9 within 100 ms of actuation",
        n as f64 * BLOCK_S <= 0.1,
        &format!("{:.0} ms", n as f64 * BLOCK_S * 1000.0),
    );
    let mut n = 0;
    while g.block(0.0, 1.0) > 0.0 && n < 2000 {
        n += 1;
    }
    let s = n as f64 * BLOCK_S;
    check(
        "silent about a second after release",
        (0.5..=2.0).contains(&s),
        &format!("{s:.2} s"),
    );
}

#[test]
fn s09_a_momentary_joystick_release_must_not_drop_it() {
    let mut g = Twin::new();
    for _ in 0..60 {
        g.block(1.0, 1.0);
    }
    let mut low = 0.0;
    for _ in 0..(0.2 / BLOCK_S) as usize {
        low = g.block(0.35, 1.0);
    }
    check(
        "gate stays open through a 200 ms gap",
        low > 0.9,
        &format!("env={low:.3}"),
    );
}

#[test]
fn s10_approaching_the_ground_opens_it_with_the_joystick_released() {
    let mut g = Twin::new();
    let envs = feed(&mut g, 3.0, &mut |_| 0.0, &mut |t| 2.0 - 0.6 * t);
    let env = *envs.last().unwrap();
    check(
        "closing on the ground alone opens the gate",
        env > 0.9,
        &format!("env={env:.3}"),
    );
}

fn noisy_clearance(
    rng: Rc<RefCell<PyRandom>>,
    jitter: Rc<RefCell<HashMap<i64, f64>>>,
) -> impl FnMut(f64) -> f64 {
    move |t| {
        let k = (t * TELEMETRY_HZ).round_ties_even() as i64;
        let mut j = jitter.borrow_mut();
        *j.entry(k)
            .or_insert_with(|| 0.85 + rng.borrow_mut().uniform(-0.004, 0.004))
    }
}

/// Scenarios 11 and 12 share one generator, seeded once, as the Python module does.
#[test]
fn s11_s12_the_chatter_bug() {
    let rng = Rc::new(RefCell::new(PyRandom::new(0xB0A7)));
    let jitter = Rc::new(RefCell::new(HashMap::new()));

    // 11. parked machine, noisy telemetry, must stay silent
    let mut g = Twin::new();
    let r2 = rng.clone();
    let envs = feed(
        &mut g,
        20.0,
        &mut move |_| r2.borrow_mut().uniform(0.0, IMPL_GATE_OFF * 0.9),
        &mut noisy_clearance(rng.clone(), jitter.clone()),
    );
    let peak = envs.iter().cloned().fold(f64::MIN, f64::max);
    check(
        "silent for 20 s of idling",
        peak == 0.0,
        &format!("peak env={peak:.4}, {} transitions", transitions(&envs)),
    );

    // 12. idle jitter must not sneak past the closing-rate term either
    let mut g = Twin::new();
    let envs = feed(
        &mut g,
        20.0,
        &mut |_| 0.0,
        &mut noisy_clearance(rng, jitter),
    );
    let peak = envs.iter().cloned().fold(f64::MIN, f64::max);
    check(
        "raycast jitter alone never opens the gate",
        peak == 0.0,
        &format!("{peak:.4}"),
    );
}

#[test]
fn s13_activity_hovering_at_the_threshold_must_not_flutter() {
    let mut g = Twin::new();
    let envs = feed(
        &mut g,
        20.0,
        &mut |t| (IMPL_GATE_ON + IMPL_GATE_OFF) / 2.0 + 0.005 * (t * 9.0).sin(),
        &mut |_| 1.0,
    );
    let n = transitions(&envs);
    check(
        "no flutter between the two thresholds",
        n <= 1,
        &format!("{n} transitions"),
    );
}

#[test]
fn s14_real_work_still_reads_as_continuous() {
    let mut g = Twin::new();
    let envs = feed(
        &mut g,
        4.0,
        &mut |t| {
            if (1.5..1.7).contains(&t) || (2.8..3.0).contains(&t) {
                0.30 // released, but the ram is still coasting
            } else {
                0.9
            }
        },
        &mut |_| 1.0,
    );
    let n = transitions(&envs);
    check(
        "one continuous sound through the whole cycle",
        n == 1,
        &format!("{n} transitions"),
    );
}

#[test]
fn s15_both_level_maps_rise_with_urgency_at_any_configured_level() {
    let ground_db = |level: f64, t: f64| level - IMPL_GROUND_SPAN_DB * (1.0 - t);
    let tilt_db = |level: f64, t: f64| level - IMPL_TILT_SPAN_DB * (1.0 - t);
    let maps: [(&str, &dyn Fn(f64, f64) -> f64, f64); 2] = [
        ("ground", &ground_db, IMPL_GROUND_MAX_DB),
        ("tilt", &tilt_db, IMPL_TILT_MAX_DB),
    ];
    for (name, f, default) in maps {
        for level in [default, -45.0, -80.0] {
            let dbs: Vec<f64> = (0..41).map(|i| f(level, i as f64 / 40.0)).collect();
            let rising = dbs.windows(2).all(|w| w[1] >= w[0] - 1e-9);
            check(
                &format!("{name} tone at {level:.1} dBFS rises toward the loud end"),
                rising && dbs[40] > dbs[0],
                "",
            );
            check(
                &format!("{name} tone at {level:.1} dBFS peaks at exactly the configured level"),
                (dbs[40] - level).abs() < 1e-9,
                "",
            );
        }
    }
    check(
        "the default ground level is unchanged by the rewrite",
        (ground_db(IMPL_GROUND_MAX_DB, 0.0) - (IMPL_GROUND_MAX_DB - IMPL_GROUND_SPAN_DB)).abs()
            < 1e-9
            && (ground_db(IMPL_GROUND_MAX_DB, 1.0) - IMPL_GROUND_MAX_DB).abs() < 1e-9,
        "",
    );
}
