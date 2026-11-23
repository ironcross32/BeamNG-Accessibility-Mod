//! `diagnostic/dock_tone_sim.py`: the docking instrument's gate, phase and lock state
//! machine replayed against a fake clock, same scenario numbers and names. `Dock` is the
//! sim's mirror of the render block (its pulse-cycle advance is the trapezoid of the
//! block's rate glide, which the real voice integrates sample by sample). For the lock
//! scenarios the real Rust callback is driven beside the mirror and must chime exactly as
//! often; the voice's own output is pinned by the golden buffers in `audio_a3_golden.rs`.

mod audio_harness;

use audio_harness::{Harness, harness};
use beam_audio::consts::{
    DOCK_AM_ABOVE_HZ, DOCK_AM_BELOW_HZ, DOCK_ATTACK_TAU, DOCK_BEAT_HZ_PER_M, DOCK_BEAT_MAX_HZ,
    DOCK_BEAT_RANGE_M, DOCK_LATERAL_FULL_M, DOCK_LOCK_BEAT_HZ, DOCK_LOCK_REARM_BEAT_HZ,
    DOCK_MAX_AZIMUTH_DEG, DOCK_MAX_RANGE_M, DOCK_PULSE_MAX_HZ, DOCK_PULSE_MIN_HZ,
    DOCK_RAMP_BEAT_HZ_PER_DEG, DOCK_RAMP_BEAT_RANGE_M, DOCK_RAMP_LATERAL_DEADZONE_M,
    DOCK_RAMP_LATERAL_FULL_M, DOCK_RAMP_LOCK_LATERAL_M, DOCK_RAMP_LOCK_YAW_DEG,
    DOCK_RAMP_MAX_RANGE_M, DOCK_RAMP_REARM_LATERAL_M, DOCK_RAMP_REARM_YAW_DEG, DOCK_REF_HZ,
    DOCK_REL_TAU, DOCK_STALE_S,
};
use beam_base::pyfmt::py_mod;

const SR: f64 = 48000.0;
const FRAMES: usize = 512;
const BLOCK_S: f64 = FRAMES as f64 / SR;
/// The mod scans at 10 Hz while the callback runs ~94 times a second.
const TELEMETRY_HZ: f64 = 10.0;

fn check(label: &str, ok: bool, detail: &str) {
    eprintln!("   {label}: {} {detail}", if ok { "OK" } else { "FAIL" });
    assert!(ok, "{label}: {detail}");
}

fn clamp(v: f64, a: f64, b: f64) -> f64 {
    if v < a {
        a
    } else if v > b {
        b
    } else {
        v
    }
}

fn pymin(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Impl,
    Ramp,
}

impl Mode {
    fn wire(self) -> &'static str {
        match self {
            Mode::Impl => "IMPL",
            Mode::Ramp => "RAMP",
        }
    }
}

/// The docking block's state machine, one audio buffer at a time.
struct Dock {
    mode: Mode,
    gate: f64,
    beat: f64,
    lock_armed: bool,
    lock_fires: usize,
    t: f64,
    last_packet_t: f64,
    range: f64,
    lateral: f64,
    acquire: bool,
    bearing: f64,
    az: f64,
    vertical: f64,
    beat_hz: f64,
    pulse_hz: f64,
    am_hz: Option<f64>,
    cycle: f64,
    rate: f64,
    /// The real callback driven beside the mirror, and its chime count.
    real: Option<(Harness, usize)>,
}

impl Dock {
    fn new(mode: Mode) -> Self {
        Dock {
            mode,
            gate: 0.0,
            beat: 0.0,
            lock_armed: false,
            lock_fires: 0,
            t: 0.0,
            last_packet_t: -1e9,
            range: f64::INFINITY,
            lateral: 0.0,
            acquire: false,
            bearing: 0.0,
            az: 0.0,
            vertical: 0.0,
            beat_hz: 0.0,
            pulse_hz: 0.0,
            am_hz: None,
            cycle: 0.0,
            rate: 0.0,
            real: None,
        }
    }

    fn imp() -> Self {
        Self::new(Mode::Impl)
    }

    fn ramp() -> Self {
        Self::new(Mode::Ramp)
    }

    /// Also drive the real callback (instrument switched on, starting in this mode).
    fn with_real(mut self) -> Self {
        let h = harness(None);
        h.ctrl.set_dock_mode(true);
        if self.mode == Mode::Ramp {
            // The real controller starts in IMPL; a first packet in RAMP switches it,
            // which resets exactly what a fresh mirror already has at rest.
            h.ctrl
                .update_dock_target(f64::INFINITY, 0.0, 0.0, "RAMP", false, 0.0);
        }
        self.real = Some((h, 0));
        self
    }

    fn max_range(&self) -> f64 {
        if self.mode == Mode::Ramp {
            DOCK_RAMP_MAX_RANGE_M
        } else {
            DOCK_MAX_RANGE_M
        }
    }
    fn beat_range(&self) -> f64 {
        if self.mode == Mode::Ramp {
            DOCK_RAMP_BEAT_RANGE_M
        } else {
            DOCK_BEAT_RANGE_M
        }
    }
    fn beat_slope(&self) -> f64 {
        if self.mode == Mode::Ramp {
            DOCK_RAMP_BEAT_HZ_PER_DEG
        } else {
            DOCK_BEAT_HZ_PER_M
        }
    }
    fn lat_full(&self) -> f64 {
        if self.mode == Mode::Ramp {
            DOCK_RAMP_LATERAL_FULL_M
        } else {
            DOCK_LATERAL_FULL_M
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn packet_full(
        &mut self,
        range_m: f64,
        null_value: f64,
        mode: Option<Mode>,
        reset_on_mode_change: bool,
        lateral_m: f64,
        acquire: bool,
        bearing_deg: f64,
    ) {
        if let Some(m) = mode {
            if m != self.mode {
                self.mode = m;
                if reset_on_mode_change {
                    self.lock_armed = false;
                    self.beat = 0.0;
                }
            }
        }
        self.acquire = acquire && self.mode == Mode::Ramp;
        self.lateral = lateral_m;
        if self.acquire {
            self.range = range_m.hypot(lateral_m);
            self.bearing = bearing_deg;
        } else {
            self.range = if 0.0 < range_m { range_m } else { 0.0 };
            self.bearing = 0.0;
        }
        self.vertical = null_value;
        self.last_packet_t = self.t;
        if let Some((h, _)) = &self.real {
            h.ctrl.update_dock_target(
                range_m,
                lateral_m,
                null_value,
                self.mode.wire(),
                acquire,
                bearing_deg,
            );
        }
    }

    fn packet(&mut self, range_m: f64, null_value: f64, mode: Option<Mode>, lateral_m: f64) {
        self.packet_full(range_m, null_value, mode, true, lateral_m, false, 0.0);
    }

    fn block(&mut self) {
        let live = self.range < self.max_range() && (self.t - self.last_packet_t) < DOCK_STALE_S;

        let tgt = if live { 1.0 } else { 0.0 };
        let tau = if tgt > self.gate {
            DOCK_ATTACK_TAU
        } else {
            DOCK_REL_TAU
        };
        self.gate += (1.0 - (-BLOCK_S / tau).exp()) * (tgt - self.gate);
        if self.gate < 0.01 && tgt == 0.0 {
            self.gate = 0.0;
        }

        if self.gate > 0.0 {
            let rng_norm = clamp(1.0 - (self.range / self.max_range()), 0.0, 1.0);
            self.pulse_hz =
                DOCK_PULSE_MIN_HZ + (DOCK_PULSE_MAX_HZ - DOCK_PULSE_MIN_HZ) * rng_norm.powf(2.0);

            // Pulse position accumulates; never elapsed-time-modulo-period.
            let prev_rate = if self.rate > 0.0 {
                self.rate
            } else {
                self.pulse_hz
            };
            let advance = (prev_rate + self.pulse_hz) / 2.0 * BLOCK_S;
            self.cycle = py_mod(self.cycle + advance, 1.0);
            self.rate = self.pulse_hz;

            if self.acquire {
                self.az = self.bearing;
            } else if self.mode == Mode::Ramp {
                let lat_abs = self.lateral.abs();
                let lat_norm = if lat_abs <= DOCK_RAMP_LATERAL_DEADZONE_M {
                    0.0
                } else {
                    clamp(
                        (lat_abs - DOCK_RAMP_LATERAL_DEADZONE_M)
                            / (self.lat_full() - DOCK_RAMP_LATERAL_DEADZONE_M),
                        0.0,
                        1.0,
                    )
                    .copysign(self.lateral)
                };
                self.az = DOCK_MAX_AZIMUTH_DEG * lat_norm;
            } else {
                self.az = DOCK_MAX_AZIMUTH_DEG * clamp(self.lateral / self.lat_full(), -1.0, 1.0);
            }

            let btgt = if live && !self.acquire && self.range < self.beat_range() {
                1.0
            } else {
                0.0
            };
            let btau = if btgt > self.beat {
                DOCK_ATTACK_TAU
            } else {
                DOCK_REL_TAU
            };
            self.beat += (1.0 - (-BLOCK_S / btau).exp()) * (btgt - self.beat);
            if self.beat < 0.01 && btgt == 0.0 {
                self.beat = 0.0;
            }

            self.beat_hz = pymin(self.vertical.abs() * self.beat_slope(), DOCK_BEAT_MAX_HZ);
            self.am_hz = if self.beat_hz <= DOCK_LOCK_BEAT_HZ {
                None
            } else if self.vertical < 0.0 {
                Some(DOCK_AM_ABOVE_HZ)
            } else {
                Some(DOCK_AM_BELOW_HZ)
            };

            let ramp = self.mode == Mode::Ramp;
            if ramp && self.acquire {
                self.lock_armed = false;
            } else if ramp
                && (self.lateral.abs() >= DOCK_RAMP_REARM_LATERAL_M
                    || self.vertical.abs() >= DOCK_RAMP_REARM_YAW_DEG)
            {
                self.lock_armed = true;
            } else if ramp
                && self.lock_armed
                && self.lateral.abs() <= DOCK_RAMP_LOCK_LATERAL_M
                && self.vertical.abs() <= DOCK_RAMP_LOCK_YAW_DEG
                && self.beat > 0.5
            {
                self.lock_armed = false;
                self.lock_fires += 1;
            } else if !ramp && self.beat_hz > DOCK_LOCK_REARM_BEAT_HZ {
                self.lock_armed = true;
            } else if !ramp
                && self.lock_armed
                && self.beat_hz <= DOCK_LOCK_BEAT_HZ
                && self.beat > 0.5
            {
                self.lock_armed = false;
                self.lock_fires += 1;
            }
        } else {
            self.beat = 0.0;
            self.pulse_hz = 0.0;
            self.lock_armed = false;
        }

        if let Some((h, fires)) = &mut self.real {
            h.ctrl.render_block(FRAMES);
            h.clock.advance(BLOCK_S);
            if h.ctrl.voice.lock()._dock_lock_pos == FRAMES as f64 {
                *fires += 1;
            }
        }
        self.t += BLOCK_S;
    }

    fn real_fires(&self) -> usize {
        self.real
            .as_ref()
            .map(|r| r.1)
            .expect("driven without the real callback")
    }
}

/// Drive the gate at audio rate while the mod's values refresh at 10 Hz.
fn run(
    sim: &mut Dock,
    seconds: f64,
    range_fn: &dyn Fn(f64) -> f64,
    null_fn: &dyn Fn(f64) -> f64,
    mode: Option<Mode>,
    lateral_fn: Option<&dyn Fn(f64) -> f64>,
) {
    let n = (seconds / BLOCK_S) as usize;
    let mut next_packet = 0.0;
    for _ in 0..n {
        if sim.t >= next_packet {
            let val = null_fn(sim.t);
            let lateral = lateral_fn.map_or(0.0, |f| f(sim.t));
            sim.packet(range_fn(sim.t), val, mode, lateral);
            next_packet = sim.t + 1.0 / TELEMETRY_HZ;
        }
        sim.block();
    }
}

fn c(v: f64) -> impl Fn(f64) -> f64 {
    move |_| v
}

#[test]
fn s01_phasing_no_more_than_two_dimensions_during_acquire() {
    let mut s = Dock::imp();
    run(&mut s, 2.0, &c(4.0), &c(0.5), None, None);
    check(
        "pulse is running at 4 m",
        s.gate > 0.9,
        &format!("gate {:.3}", s.gate),
    );
    check(
        "beat pair is silent at 4 m",
        s.beat == 0.0,
        &format!("beat env {:.3}", s.beat),
    );
    run(&mut s, 2.0, &c(1.5), &c(0.5), None, None);
    check(
        "beat pair enters inside the beat range",
        s.beat > 0.9,
        &format!("{:.3}", s.beat),
    );
}

#[test]
fn s02_past_the_max_range_silent_not_merely_quiet() {
    let mut s = Dock::imp();
    run(&mut s, 2.0, &c(DOCK_MAX_RANGE_M + 1.0), &c(0.3), None, None);
    check(
        "gate is hard zero",
        s.gate == 0.0,
        &format!("gate {:.6}", s.gate),
    );
    check(
        "a one-pole alone would never have reached it",
        (-2.0 / DOCK_REL_TAU).exp() > 0.0,
        "",
    );
}

#[test]
fn s03_the_beat_centre_pitch_fixed_only_the_rate_moves() {
    let partners = |beat: f64| (DOCK_REF_HZ - beat / 2.0, DOCK_REF_HZ + beat / 2.0);
    let errs: Vec<f64> = (0..51).map(|x| x as f64 * 0.02).collect();
    let mut centres = Vec::new();
    let mut rates = Vec::new();
    for &e in &errs {
        let b = pymin(e * DOCK_BEAT_HZ_PER_M, DOCK_BEAT_MAX_HZ);
        let (lo, hi) = partners(b);
        centres.push((lo + hi) / 2.0);
        rates.push(hi - lo);
    }
    let cmax = centres.iter().cloned().fold(f64::MIN, f64::max);
    let cmin = centres.iter().cloned().fold(f64::MAX, f64::min);
    check("the centre pitch never moves", cmax - cmin < 1e-9, "");
    check(
        "the beat rate is exactly the specified rate",
        rates
            .iter()
            .zip(&errs)
            .all(|(r, e)| (r - pymin(e * DOCK_BEAT_HZ_PER_M, DOCK_BEAT_MAX_HZ)).abs() < 1e-9),
        "",
    );
    let rmax = rates.iter().cloned().fold(f64::MIN, f64::max);
    check(
        "the whole range stays where beating is heard AS beating",
        rmax <= 15.0,
        "",
    );
    let fine = 0.02 * DOCK_BEAT_HZ_PER_M;
    check(
        "a fine error still produces a countable beat",
        0.2 < fine && fine < 2.0,
        "",
    );
    check(
        "alignment is exact unison",
        partners(0.0).0 == partners(0.0).1,
        "",
    );
    check(
        "the pair is never a musical interval",
        rmax / DOCK_REF_HZ < 0.06,
        "",
    );
}

#[test]
fn s04_the_lock_fires_once_on_the_way_through() {
    let mut s = Dock::imp().with_real();
    run(&mut s, 1.0, &c(1.0), &c(0.5), None, None);
    run(
        &mut s,
        2.0,
        &c(1.0),
        &|t| (0.5 - 0.25 * t).max(0.0),
        None,
        None,
    );
    run(&mut s, 3.0, &c(1.0), &c(0.0), None, None);
    check(
        "locked exactly once",
        s.lock_fires == 1,
        &format!("{} fires", s.lock_fires),
    );
    assert_eq!(s.real_fires(), s.lock_fires, "real callback chimes");
}

#[test]
fn s05_hunting_around_the_null_cannot_machine_gun_the_lock() {
    let mut s = Dock::imp().with_real();
    run(&mut s, 1.0, &c(1.0), &c(0.5), None, None);
    run(&mut s, 1.0, &c(1.0), &c(0.0), None, None);
    let first = s.lock_fires;
    let wobble = (DOCK_LOCK_BEAT_HZ + DOCK_LOCK_REARM_BEAT_HZ) * 0.5 / DOCK_BEAT_HZ_PER_M;
    run(
        &mut s,
        4.0,
        &c(1.0),
        &|t| wobble * (t * 12.0).sin(),
        None,
        None,
    );
    check("no extra fires while hunting", s.lock_fires == first, "");
    run(&mut s, 1.0, &c(1.0), &c(0.5), None, None);
    run(&mut s, 1.5, &c(1.0), &c(0.0), None, None);
    check(
        "a real departure and return chimes again",
        s.lock_fires == first + 1,
        "",
    );
    assert_eq!(s.real_fires(), s.lock_fires, "real callback chimes");
}

#[test]
fn s06_the_sign_carrier_distinguishes_above_from_below() {
    let mut s = Dock::imp();
    run(&mut s, 1.0, &c(1.0), &c(0.4), None, None);
    let above = s.am_hz.expect("tremolo");
    run(&mut s, 1.0, &c(1.0), &c(-0.4), None, None);
    let below = s.am_hz.expect("tremolo");
    check(
        "the two rates differ",
        above != below,
        &format!("{above} vs {below}"),
    );
    check(
        "they are far enough apart to name by ear",
        (above - below).abs() >= 2.0,
        "",
    );
    check(
        "both stay below the beat rates they coexist with",
        above.max(below) < DOCK_BEAT_MAX_HZ,
        "",
    );
    run(&mut s, 1.0, &c(1.0), &c(0.0), None, None);
    check("tremolo stops at the null", s.am_hz.is_none(), "");
}

#[test]
fn s07_a_stale_feed_fades_out() {
    let mut s = Dock::imp();
    run(&mut s, 1.0, &c(1.0), &c(0.0), None, None);
    check("running while fed", s.gate > 0.9, "");
    for _ in 0..((DOCK_STALE_S + 1.0) / BLOCK_S) as usize {
        s.block();
    }
    check(
        "faded out after the stale window",
        s.gate == 0.0,
        &format!("gate {:.6}", s.gate),
    );
}

#[test]
fn s08_the_train_pulse_position_accumulates() {
    let mut s = Dock::imp();
    run(&mut s, 1.0, &c(4.5), &c(0.0), None, None);
    let (mut worst, mut prev_c, mut prev_r) = (0.0f64, s.cycle, s.rate);
    let n = (2.0 / BLOCK_S) as usize;
    for i in 0..n {
        let rng = 4.5 - (i as f64 * BLOCK_S) * 1.15;
        s.packet(rng, 0.0, None, 0.0);
        s.block();
        let expect = (prev_r + s.rate) / 2.0 * BLOCK_S;
        let actual = py_mod(s.cycle - prev_c, 1.0);
        let err = (actual - py_mod(expect, 1.0)).abs();
        worst = worst.max(err.min(1.0 - err));
        prev_c = s.cycle;
        prev_r = s.rate;
    }
    check(
        "phase advances by exactly rate x dt every block",
        worst < 1e-9,
        &format!("{worst:e}"),
    );

    // The old formulation on the same approach, so the check cannot pass for free.
    let (mut worst_old, mut prev, mut abs_t) = (0.0f64, None::<f64>, 60.0f64);
    for i in 0..n {
        let rng = 4.5 - (i as f64 * BLOCK_S) * 1.15;
        let nrm = clamp(1.0 - rng / DOCK_MAX_RANGE_M, 0.0, 1.0);
        let hz = DOCK_PULSE_MIN_HZ + (DOCK_PULSE_MAX_HZ - DOCK_PULSE_MIN_HZ) * nrm * nrm;
        let pos = py_mod(abs_t, 1.0 / hz) * hz;
        if let Some(p) = prev {
            let e = (pos - py_mod(p + hz * BLOCK_S, 1.0)).abs();
            worst_old = worst_old.max(e.min(1.0 - e));
        }
        prev = Some(pos);
        abs_t += BLOCK_S;
    }
    check(
        "elapsed-time-modulo-period would teleport",
        worst_old > 0.1,
        &format!("{worst_old:.3}"),
    );
}

#[test]
fn s09_pulse_rate_rises_monotonically_and_stays_in_band() {
    let mut s = Dock::imp();
    let mut rates = Vec::new();
    for i in 0..50 {
        let rng = DOCK_MAX_RANGE_M - i as f64 * 0.1;
        run(&mut s, 0.2, &c(rng), &c(0.0), None, None);
        if s.gate > 0.5 {
            rates.push(s.pulse_hz);
        }
    }
    check(
        "monotonic",
        rates.windows(2).all(|w| w[1] >= w[0] - 1e-9),
        "",
    );
    check(
        "within the configured band",
        rates
            .iter()
            .all(|r| (DOCK_PULSE_MIN_HZ - 1e-6..=DOCK_PULSE_MAX_HZ + 1e-6).contains(r)),
        "",
    );
    let mid = rates
        .iter()
        .filter(|&&r| r < (DOCK_PULSE_MIN_HZ + DOCK_PULSE_MAX_HZ) / 2.0)
        .count();
    check(
        "most of the range sits below the midpoint rate",
        mid as f64 > rates.len() as f64 * 0.6,
        &format!("{mid} of {}", rates.len()),
    );
}

#[test]
fn s10_ramp_mode_is_live_where_implement_mode_is_silent() {
    let (mut imp, mut ramp) = (Dock::imp(), Dock::ramp());
    run(&mut imp, 2.0, &c(12.0), &c(0.0), None, None);
    run(&mut ramp, 2.0, &c(12.0), &c(0.0), Some(Mode::Ramp), None);
    check(
        "at 12 m the implement instrument is silent",
        imp.gate == 0.0,
        "",
    );
    check("at 12 m ramp mode is fully up", ramp.gate > 0.99, "");
    let mut far = Dock::ramp();
    run(
        &mut far,
        2.0,
        &c(DOCK_RAMP_MAX_RANGE_M + 1.0),
        &c(0.0),
        Some(Mode::Ramp),
        None,
    );
    check(
        "past the ramp max range it is silent, not merely quiet",
        far.gate == 0.0,
        "",
    );
}

#[test]
fn s11_the_ramp_acquire_phase_is_two_dimensional() {
    let mut acq = Dock::ramp();
    run(&mut acq, 2.0, &c(12.0), &c(2.0), Some(Mode::Ramp), None);
    check(
        "at 12 m the null channel has not entered",
        acq.beat < 0.01,
        "",
    );
    let mut close = Dock::ramp();
    run(&mut close, 2.0, &c(5.0), &c(2.0), Some(Mode::Ramp), None);
    check("at 5 m it is fully in", close.beat > 0.99, "");
    check(
        "the ramp beat range is well outside the un-correctable zone",
        DOCK_RAMP_BEAT_RANGE_M >= 6.0,
        "",
    );
}

#[test]
fn s12_ramp_axes_yaw_drives_beats_lateral_drives_pulse_position() {
    let r = Some(Mode::Ramp);
    let mut yaw = Dock::ramp();
    run(&mut yaw, 1.0, &c(4.0), &c(12.0), r, None);
    check(
        "twelve degrees of yaw produces a 6 Hz beat",
        (yaw.beat_hz - 6.0).abs() < 1e-9,
        "",
    );
    check("yaw alone does not move the pulse", yaw.az == 0.0, "");
    let mut lat = Dock::ramp();
    run(&mut lat, 1.0, &c(4.0), &c(0.0), r, Some(&c(1.0)));
    check("a lateral offset moves the pulse", lat.az > 0.0, "");
    check(
        "lateral alone leaves the pair at unison",
        lat.beat_hz == 0.0,
        "",
    );
    let mut side_l = Dock::ramp();
    run(&mut side_l, 1.0, &c(4.0), &c(6.0), r, None);
    let mut side_r = Dock::ramp();
    run(&mut side_r, 1.0, &c(4.0), &c(-6.0), r, None);
    check(
        "the two heading signs pick different tremolo rates",
        side_l.am_hz != side_r.am_hz,
        "",
    );
    check(
        "slow means turn left and fast means turn right",
        side_r.am_hz == Some(DOCK_AM_ABOVE_HZ) && side_l.am_hz == Some(DOCK_AM_BELOW_HZ),
        "",
    );

    let (mut mi, mut mr) = (Dock::imp(), Dock::ramp());
    run(&mut mi, 1.0, &c(2.0), &c(0.5), None, None);
    run(&mut mr, 1.0, &c(2.0), &c(0.5), r, None);
    check(
        "mode-specific units produce their configured rates",
        (mi.beat_hz - 12.0).abs() < 1e-9 && (mr.beat_hz - 0.25).abs() < 1e-9,
        "",
    );
    let mut sat = Dock::ramp();
    run(&mut sat, 1.0, &c(2.0), &c(24.0), r, None);
    check(
        "twenty-four degrees saturates the beat",
        (sat.beat_hz - DOCK_BEAT_MAX_HZ).abs() < 1e-9,
        "",
    );
    check(
        "the ramp slope is 0.5 Hz per degree",
        (DOCK_RAMP_BEAT_HZ_PER_DEG - 0.5).abs() < 1e-9,
        "",
    );
    check(
        "the ramp pan is scaled for a trough, not a bucket",
        DOCK_RAMP_LATERAL_FULL_M > DOCK_LATERAL_FULL_M * 2.0,
        "",
    );
    let mut centre = Dock::ramp();
    run(
        &mut centre,
        1.0,
        &c(4.0),
        &c(0.0),
        r,
        Some(&c(DOCK_RAMP_LATERAL_DEADZONE_M)),
    );
    let mut full = Dock::ramp();
    run(
        &mut full,
        1.0,
        &c(4.0),
        &c(0.0),
        r,
        Some(&c(DOCK_RAMP_LATERAL_FULL_M)),
    );
    check(
        "15 cm is exactly centred in the pulse image",
        centre.az == 0.0,
        "",
    );
    check(
        "3.0 m still reaches the full 75 degree azimuth",
        (full.az - DOCK_MAX_AZIMUTH_DEG).abs() < 1e-9,
        "",
    );

    let mut lock = Dock::ramp().with_real();
    run(
        &mut lock,
        1.0,
        &c(4.0),
        &c(0.0),
        r,
        Some(&c(DOCK_RAMP_REARM_LATERAL_M)),
    );
    run(
        &mut lock,
        1.0,
        &c(4.0),
        &c(0.0),
        r,
        Some(&c(DOCK_RAMP_LOCK_LATERAL_M + 0.05)),
    );
    check(
        "square but laterally displaced does not lock",
        lock.lock_fires == 0,
        "",
    );
    run(
        &mut lock,
        1.0,
        &c(4.0),
        &c(DOCK_RAMP_LOCK_YAW_DEG + 1.0),
        r,
        Some(&c(0.0)),
    );
    check(
        "centred but crooked does not lock",
        lock.lock_fires == 0,
        "",
    );
    run(&mut lock, 1.0, &c(4.0), &c(0.0), r, Some(&c(0.0)));
    check("correcting both axes locks once", lock.lock_fires == 1, "");
    run(
        &mut lock,
        1.0,
        &c(4.0),
        &c(DOCK_RAMP_REARM_YAW_DEG - 0.1),
        r,
        Some(&c(DOCK_RAMP_REARM_LATERAL_M - 0.01)),
    );
    run(&mut lock, 1.0, &c(4.0), &c(0.0), r, Some(&c(0.0)));
    check(
        "sub-rearm departures do not produce another lock",
        lock.lock_fires == 1,
        "",
    );
    run(
        &mut lock,
        1.0,
        &c(4.0),
        &c(DOCK_RAMP_REARM_YAW_DEG),
        r,
        Some(&c(0.0)),
    );
    run(&mut lock, 1.0, &c(4.0), &c(0.0), r, Some(&c(0.0)));
    check(
        "8 degrees re-arms and a return locks again",
        lock.lock_fires == 2,
        "",
    );
    assert_eq!(lock.real_fires(), lock.lock_fires, "real callback chimes");
}

#[test]
fn s13_changing_mode_does_not_fire_a_spurious_lock_chime() {
    let mut sw = Dock::imp().with_real();
    run(&mut sw, 1.0, &c(2.0), &c(0.4), None, None);
    check(
        "the latch is armed before the switch",
        sw.lock_armed,
        &format!("{:.2} Hz", sw.beat_hz),
    );
    let before = sw.lock_fires;
    run(&mut sw, 1.0, &c(2.0), &c(0.2), Some(Mode::Ramp), None);
    check(
        "no chime across the mode change",
        sw.lock_fires == before,
        "",
    );
    assert_eq!(sw.real_fires(), sw.lock_fires, "real callback chimes");

    // Without the reset, the same sequence chimes.
    let mut bug = Dock::imp();
    run(&mut bug, 1.0, &c(2.0), &c(0.4), None, None);
    let bug_before = bug.lock_fires;
    let n = (1.0 / BLOCK_S) as usize;
    let mut nxt = bug.t;
    for _ in 0..n {
        if bug.t >= nxt {
            bug.packet_full(2.0, 0.2, Some(Mode::Ramp), false, 0.0, false, 0.0);
            nxt = bug.t + 1.0 / TELEMETRY_HZ;
        }
        bug.block();
    }
    check(
        "without the latch reset it WOULD chime",
        bug.lock_fires > bug_before,
        "",
    );
}

fn hunt(sim: &mut Dock, seconds: f64, along: f64, lat: f64, bearing: f64) {
    let n = (seconds / BLOCK_S) as usize;
    let mut nxt = sim.t;
    for _ in 0..n {
        if sim.t >= nxt {
            sim.packet_full(along, 0.0, Some(Mode::Ramp), true, lat, true, bearing);
            nxt = sim.t + 1.0 / TELEMETRY_HZ;
        }
        sim.block();
    }
}

#[test]
fn s14_hunting_for_the_mouth_the_pulse_is_a_beacon() {
    let mut rates = Vec::new();
    let mut h = Dock::ramp();
    for along in [-12.0, -9.0, -6.0, -3.0] {
        hunt(&mut h, 0.4, along, 3.0, 140.0);
        rates.push(h.pulse_hz);
    }
    check(
        "driving toward the mouth from BEHIND the plane speeds the pulse up",
        rates.windows(2).all(|w| w[1] > w[0] + 0.2),
        &format!("{rates:?}"),
    );
    let mut floored = Dock::ramp();
    let mut old = Vec::new();
    for along in [-12.0f64, -9.0, -6.0, -3.0] {
        let n = (0.4 / BLOCK_S) as usize;
        let mut nxt = floored.t;
        for _ in 0..n {
            if floored.t >= nxt {
                floored.packet(along.max(0.0), 0.0, Some(Mode::Ramp), 0.0);
                nxt = floored.t + 1.0 / TELEMETRY_HZ;
            }
            floored.block();
        }
        old.push(floored.pulse_hz);
    }
    let omax = old.iter().cloned().fold(f64::MIN, f64::max);
    let omin = old.iter().cloned().fold(f64::MAX, f64::min);
    check(
        "the floored range WOULD have pinned it at the contact rate",
        omax - omin < 1e-9 && (old[3] - DOCK_PULSE_MAX_HZ).abs() < 1e-9,
        "",
    );
    check(
        "a mouth astern is panned astern",
        h.az.abs() > DOCK_MAX_AZIMUTH_DEG,
        "",
    );

    let mut h2 = Dock::ramp().with_real();
    run(
        &mut h2,
        1.0,
        &c(4.0),
        &c(DOCK_RAMP_REARM_YAW_DEG),
        Some(Mode::Ramp),
        None,
    );
    check(
        "the latch can be armed before acquisition",
        h2.lock_armed,
        "",
    );
    hunt(&mut h2, 1.5, -4.0, 3.0, 140.0);
    check(
        "the null channel is silent while hunting",
        h2.beat == 0.0,
        "",
    );
    check(
        "...and no lock chime can fire from there",
        h2.lock_fires == 0,
        "",
    );
    check(
        "acquisition clears a previously armed latch",
        !h2.lock_armed,
        "",
    );
    run(&mut h2, 1.0, &c(6.0), &c(0.5), Some(Mode::Ramp), None);
    check(
        "reaching the approach brings the null channel in",
        h2.beat > 0.9,
        "",
    );
    check(
        "entering already aligned cannot produce a second cue",
        h2.lock_fires == 0,
        "",
    );
    assert_eq!(h2.real_fires(), h2.lock_fires, "real callback chimes");
    let v = h2.real.as_ref().unwrap().0.ctrl.voice.lock();
    assert!(v._dock_beat_env > 0.9, "real beat env {}", v._dock_beat_env);
}
