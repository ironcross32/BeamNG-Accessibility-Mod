//! `diagnostic/hydro_steer_sim.py`: the articulation tone/click state machine replayed
//! against a fake clock, same scenario numbers and names. The Python sim mirrors the
//! render block's logic; `Sim` below is that mirror, and `real_callback_tracks_the_mirror`
//! drives the real Rust callback alongside it so the mirror cannot drift from the voice.

mod audio_harness;

use audio_harness::harness;
use beam_audio::consts::{
    HYDRO_CENTER_CLICK_DUR_MS, HYDRO_CENTER_REARM, HYDRO_STEER_ATTACK_TAU, HYDRO_STEER_DEADZONE,
    HYDRO_STEER_FM_DIRT_CURVE, HYDRO_STEER_FM_INDEX_HI, HYDRO_STEER_FM_INDEX_LO,
    HYDRO_STEER_FM_RATIO_HI, HYDRO_STEER_FM_RATIO_LO, HYDRO_STEER_FULL, HYDRO_STEER_HI_HZ,
    HYDRO_STEER_LO_HZ, HYDRO_STEER_MAX_AZIMUTH_DEG, HYDRO_STEER_MAX_DB, HYDRO_STEER_MIN_DB,
    HYDRO_STEER_PITCH_TAU, HYDRO_STEER_REL_TAU,
};
use beam_audio::controller::TelemetryState;
use beam_audio::dsp;
use beam_audio::hrtf::HrtfSet;
use beam_base::pyfmt::py_mod;

const SR: f64 = 48000.0;
const FRAMES: usize = 512;

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    // max(lo, min(hi, v))
    let m = if hi < v { hi } else { v };
    if m > lo { m } else { lo }
}

fn pan_gains(p: f64) -> (f64, f64) {
    let ang = (clamp(p, -1.0, 1.0) + 1.0) * (std::f64::consts::PI / 4.0);
    (ang.cos(), ang.sin())
}

fn linspace_open(a: f64, b: f64, n: usize) -> Vec<f64> {
    dsp::linspace(a, b, n, false)
}

fn linspace(a: f64, b: f64, n: usize) -> Vec<f64> {
    dsp::linspace(a, b, n, true)
}

struct Sim {
    last_samples: Vec<f64>,
    last_pan: (f64, f64),
    phase: f64,
    mod_phase: f64,
    inc: f64,
    ratio: f64,
    index: f64,
    env: f64,
    sm_mag: f64,
    armed: bool,
    pos: f64,
    clicks: usize,
    click_len: i64,
}

impl Sim {
    fn new() -> Self {
        Sim {
            last_samples: Vec::new(),
            last_pan: (0.0, 0.0),
            phase: 0.0,
            mod_phase: 0.0,
            inc: 0.0,
            ratio: 0.0,
            index: 0.0,
            env: 0.0,
            sm_mag: 0.0,
            armed: false,
            pos: -1.0,
            clicks: 0,
            click_len: (SR * HYDRO_CENTER_CLICK_DUR_MS as f64 / 1000.0) as i64,
        }
    }

    fn block(&mut self, hydro_actual: f64) -> (f64, f64) {
        let frames = FRAMES;
        let abs_h = hydro_actual.abs();
        let dt = frames as f64 / SR;
        let target = if abs_h > HYDRO_STEER_DEADZONE { 1.0 } else { 0.0 };
        let mut peak = 0.0;
        let mut freq = 0.0;
        if target > 0.0 || self.env > 1e-4 {
            let tau = if target > self.env {
                HYDRO_STEER_ATTACK_TAU
            } else {
                HYDRO_STEER_REL_TAU
            };
            let env_end = self.env + (1.0 - (-dt / tau).exp()) * (target - self.env);
            let beta = 1.0 - (-dt / HYDRO_STEER_PITCH_TAU).exp();
            self.sm_mag += beta * (abs_h - self.sm_mag);
            let norm = clamp(
                (self.sm_mag - HYDRO_STEER_DEADZONE) / (HYDRO_STEER_FULL - HYDRO_STEER_DEADZONE),
                0.0,
                1.0,
            );
            let inc = (HYDRO_STEER_LO_HZ + (HYDRO_STEER_HI_HZ - HYDRO_STEER_LO_HZ) * norm) / SR;
            let dirt = norm.powf(HYDRO_STEER_FM_DIRT_CURVE);
            let ratio = HYDRO_STEER_FM_RATIO_LO
                + (HYDRO_STEER_FM_RATIO_HI - HYDRO_STEER_FM_RATIO_LO) * dirt;
            let index = HYDRO_STEER_FM_INDEX_LO
                + (HYDRO_STEER_FM_INDEX_HI - HYDRO_STEER_FM_INDEX_LO) * dirt;
            let inc_prev = if self.inc > 0.0 { self.inc } else { inc };
            let ratio_prev = if self.ratio > 0.0 { self.ratio } else { ratio };
            let index_prev = if self.index > 0.0 { self.index } else { index };
            let inc_arr = linspace_open(inc_prev, inc, frames);
            let ratio_arr = linspace_open(ratio_prev, ratio, frames);
            let index_arr = linspace_open(index_prev, index, frames);
            let cs = dsp::cumsum(&inc_arr);
            let prod: Vec<f64> = inc_arr.iter().zip(&ratio_arr).map(|(a, b)| a * b).collect();
            let cm = dsp::cumsum(&prod);
            let phase_arr: Vec<f64> = cs.iter().map(|c| py_mod(self.phase + c, 1.0)).collect();
            let mod_arr: Vec<f64> = cm.iter().map(|c| py_mod(self.mod_phase + c, 1.0)).collect();
            self.inc = inc;
            self.ratio = ratio;
            self.index = index;
            self.phase = *phase_arr.last().unwrap();
            self.mod_phase = *mod_arr.last().unwrap();
            let amp = 10f64.powf(
                (HYDRO_STEER_MIN_DB + (HYDRO_STEER_MAX_DB - HYDRO_STEER_MIN_DB) * norm) / 20.0,
            );
            let envs = linspace_open(self.env, env_end, frames);
            let tau2 = 2.0 * std::f64::consts::PI;
            let mono: Vec<f64> = (0..frames)
                .map(|k| {
                    amp * (tau2 * phase_arr[k] + index_arr[k] * (tau2 * mod_arr[k]).sin()).sin()
                        * envs[k]
                })
                .collect();
            self.env = env_end;
            peak = mono.iter().fold(0.0f64, |a, x| a.max(x.abs()));
            self.last_samples = mono;
            freq = inc * SR;
            self.last_pan = pan_gains(-hydro_actual);
        } else {
            self.env = 0.0;
            self.inc = 0.0;
            self.ratio = 0.0;
            self.index = 0.0;
            self.sm_mag = abs_h;
            self.last_pan = (0.0, 0.0);
            self.last_samples = vec![0.0; frames];
        }

        if abs_h > HYDRO_STEER_DEADZONE * HYDRO_CENTER_REARM {
            self.armed = true;
        } else if self.armed && abs_h <= HYDRO_STEER_DEADZONE {
            self.armed = false;
            self.pos = 0.0;
            self.clicks += 1;
        }

        if self.pos >= 0.0 {
            let nxt = self.pos as i64 + frames as i64;
            self.pos = if nxt < self.click_len { nxt as f64 } else { -1.0 };
        }
        (peak, freq)
    }
}

/// `scenario(name, values, expect_clicks)`: returns the sim, peaks and freqs.
fn scenario(name: &str, values: &[f64], expect_clicks: Option<usize>) -> (Sim, Vec<f64>, Vec<f64>) {
    let mut s = Sim::new();
    let mut peaks = Vec::new();
    let mut freqs = Vec::new();
    for &v in values {
        let (p, f) = s.block(v);
        peaks.push(p);
        freqs.push(f);
    }
    let pmax = peaks.iter().cloned().fold(f64::MIN, f64::max);
    eprintln!("{name}: clicks={} peak_max={pmax:.4}", s.clicks);
    if let Some(e) = expect_clicks {
        assert_eq!(s.clicks, e, "{name}: expected {e} clicks");
    }
    (s, peaks, freqs)
}

fn seq_articulate() -> Vec<f64> {
    let mut seq = vec![0.0; 20];
    seq.extend(linspace(0.0, 0.6, 60));
    seq.extend(vec![0.6; 100]);
    seq.extend(linspace(0.6, 0.0, 60));
    seq.extend(vec![0.0; 60]);
    seq
}

#[test]
fn s01_normal_car_always_0() {
    scenario("normal car (always 0)", &vec![0.0; 400], Some(0));
}

#[test]
fn s02_articulate_left_hold_straighten() {
    scenario("articulate left, hold, straighten", &seq_articulate(), Some(1));
}

#[test]
fn s03_hunting_just_past_deadzone_below_rearm() {
    let mut hunt = vec![0.0; 10];
    for _ in 0..12 {
        hunt.extend(linspace(0.0, 0.09, 8));
        hunt.extend(linspace(0.09, 0.0, 8));
    }
    scenario("hunting just past deadzone (below rearm)", &hunt, Some(0));
}

#[test]
fn s04_three_full_out_and_back_sweeps() {
    let mut sweep = Vec::new();
    for _ in 0..3 {
        sweep.extend(linspace(0.0, 0.5, 30));
        sweep.extend(linspace(0.5, 0.0, 30));
        sweep.extend(vec![0.0; 10]);
    }
    scenario("three full out-and-back sweeps", &sweep, Some(3));
}

#[test]
fn s05_left_to_right_through_centre() {
    let mut cross = linspace(0.5, -0.5, 80);
    cross.extend(vec![-0.5; 20]);
    scenario("left to right through centre", &cross, Some(1));
}

#[test]
fn s06_continuity_check() {
    let (_, peaks, _) = scenario("continuity check", &seq_articulate(), None);
    let jump = peaks
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f64, f64::max);
    eprintln!("   max block-to-block peak jump = {jump:.4}");
    assert!(jump < 0.05, "audible step: {jump}");
}

#[test]
fn s07_phase_continuity() {
    let mut s = Sim::new();
    let mut sig = Vec::new();
    for v in seq_articulate() {
        s.block(v);
        sig.extend_from_slice(&s.last_samples);
    }
    let d: Vec<f64> = sig.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    let mut b_max = 0.0f64;
    let mut i = FRAMES;
    while i < sig.len() {
        b_max = b_max.max(d[i - 1]);
        i += FRAMES;
    }
    let all_max = d.iter().cloned().fold(0.0f64, f64::max);
    eprintln!("phase continuity: max boundary step={b_max:.6} vs max anywhere={all_max:.6}");
    assert!(b_max <= all_max * 1.05, "phase discontinuity");
}

fn fm_params(norm: f64) -> (f64, f64, f64) {
    let dirt = norm.powf(HYDRO_STEER_FM_DIRT_CURVE);
    let fc = HYDRO_STEER_LO_HZ + (HYDRO_STEER_HI_HZ - HYDRO_STEER_LO_HZ) * norm;
    let ratio =
        HYDRO_STEER_FM_RATIO_LO + (HYDRO_STEER_FM_RATIO_HI - HYDRO_STEER_FM_RATIO_LO) * dirt;
    let idx = HYDRO_STEER_FM_INDEX_LO + (HYDRO_STEER_FM_INDEX_HI - HYDRO_STEER_FM_INDEX_LO) * dirt;
    (fc, ratio, idx)
}

/// `(inharmonic power fraction, amplitude-weighted spectral centroid)`.
fn timbre(norm: f64) -> (f64, f64) {
    let (fc, ratio, idx) = fm_params(norm);
    let n = (SR as usize) / 2;
    let pi2 = 2.0 * std::f64::consts::PI;
    let w: Vec<f64> = (0..n)
        .map(|k| {
            let t = k as f64 / SR;
            let hann = 0.5 - 0.5 * (pi2 * k as f64 / (n - 1) as f64).cos();
            (pi2 * fc * t + idx * (pi2 * fc * ratio * t).sin()).sin() * hann
        })
        .collect();
    let spec = dsp::rfft(&w, n);
    let p: Vec<f64> = spec.iter().map(|c| c.norm_sqr()).collect();
    let f = dsp::rfftfreq(n, 1.0 / SR);
    let (mut inh, mut tot, mut wf, mut ws) = (0.0, 0.0, 0.0, 0.0);
    for (i, (&pi, &fi)) in p.iter().zip(&f).enumerate() {
        let _ = i;
        let harm = (1..40).any(|k| (fi - k as f64 * fc).abs() < fc * 0.06);
        if !harm {
            inh += pi;
        }
        tot += pi;
        let wv = pi.sqrt();
        wf += wv * fi;
        ws += wv;
    }
    (inh / tot, wf / ws)
}

#[test]
fn s08_fm_timbre() {
    for nrm in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let (_, ratio, idx) = fm_params(nrm);
        let (inh, cen) = timbre(nrm);
        eprintln!(
            "   norm={nrm:4.2} ratio={ratio:5.3} index={idx:4.2} inharmonic={:5.1}% centroid={cen:6.0}Hz",
            inh * 100.0
        );
    }
    let clean = timbre(0.0).0;
    let dirty = timbre(1.0).0;
    assert!(clean < 0.02 && 0.02 < dirty, "clean at straight, dirty at lock: {clean} {dirty}");
    let mid_ratio = fm_params(0.5).1;
    assert!(
        (mid_ratio - HYDRO_STEER_FM_RATIO_LO).abs() < 0.15,
        "piercing mid-range: {mid_ratio}"
    );
    let peak_centroid = timbre(1.0).1;
    assert!(peak_centroid < 3000.0, "too piercing: {peak_centroid}");
}

#[test]
fn s09_hrtf_image_width() {
    let root = beam_testutil::repo_root();
    for info in beam_base::hrtf_sets::available_sets() {
        let set = HrtfSet::load_quiet(&root.join(info.filename), None, None);
        if !set.is_loaded() {
            // npz absent in a bare checkout; not a logic failure
            eprintln!("HRTF width [{}]: skipped (not loaded)", info.set_id);
            continue;
        }
        let ild_db = |az: f64| -> f64 {
            let (l, r) = set.get_hrir(az).expect("ir");
            let el: f64 = l.iter().map(|x| x * x).sum();
            let er: f64 = r.iter().map(|x| x * x).sum();
            10.0 * (el.max(1e-20) / er.max(1e-20)).log10()
        };
        let near = ild_db(HYDRO_STEER_MAX_AZIMUTH_DEG * 0.75);
        let full = ild_db(HYDRO_STEER_MAX_AZIMUTH_DEG);
        eprintln!(
            "HRTF width [{}]: ILD {near:.2}dB at 3/4 lock -> {full:.2}dB at full lock",
            info.set_id
        );
        assert!(full > near, "[{}] harder turn reads as narrower", info.set_id);
    }
}

#[test]
fn s10_pan_sanity() {
    let mut s = Sim::new();
    s.block(0.4);
    let (lg, rg) = s.last_pan;
    assert!(lg > rg, "pan at +0.4 should be left-dominant: L={lg:.3} R={rg:.3}");
    let mut s = Sim::new();
    s.block(-0.4);
    let (lg, rg) = s.last_pan;
    assert!(rg > lg, "pan at -0.4 should be right-dominant: L={lg:.3} R={rg:.3}");
}

/// Not in the Python sim: the scenario sequences through the REAL callback, block for
/// block beside the mirror. The continuous state must agree exactly. The click latch is
/// compared only where the frame bend never reads exactly 0.0: the real callback treats an
/// exact 0.0 as "no hydraulic steering" and hands the channel to the (empty) trailer source,
/// and a source change disarms the click, which the mirror does not model.
#[test]
fn real_callback_tracks_the_mirror() {
    let mut hunt = vec![0.01; 10];
    for _ in 0..6 {
        hunt.extend(linspace(0.01, 0.4, 8));
        hunt.extend(linspace(0.4, 0.01, 8));
    }
    let sequences: Vec<(&str, Vec<f64>, bool)> = vec![
        ("articulate", seq_articulate(), false),
        ("sweeps", hunt, true),
        ("cross", {
            let mut c = linspace(0.5, -0.5, 80);
            c.extend(vec![-0.5; 20]);
            c
        }, true),
    ];
    for (name, seq, compare_clicks) in sequences {
        let h = harness(None);
        let mut sim = Sim::new();
        let mut real_clicks = 0;
        for (i, &x) in seq.iter().enumerate() {
            h.ctrl.update_telemetry_state(&TelemetryState {
                last_actual_steering: Some(x),
                ..TelemetryState::default()
            });
            h.ctrl.render_block(FRAMES);
            sim.block(x);
            let v = h.ctrl.voice.lock();
            let what = format!("{name} block {i}");
            assert_eq!(v._hydro_steer_env, sim.env, "{what}: env");
            assert_eq!(v._hydro_steer_sm_mag, sim.sm_mag, "{what}: sm_mag");
            assert_eq!(v._hydro_steer_inc, sim.inc, "{what}: inc");
            assert_eq!(v._hydro_steer_ratio, sim.ratio, "{what}: ratio");
            assert_eq!(v._hydro_steer_index, sim.index, "{what}: index");
            if sim.inc > 0.0 {
                assert_eq!(v._hydro_steer_phase, sim.phase, "{what}: phase");
                assert_eq!(v._hydro_steer_mod_phase, sim.mod_phase, "{what}: mod phase");
            }
            if v._hydro_center_pos == FRAMES as f64 {
                real_clicks += 1;
            }
            if compare_clicks {
                assert_eq!(v._hydro_center_armed, sim.armed, "{what}: armed");
            }
        }
        if compare_clicks {
            assert_eq!(real_clicks, sim.clicks, "{name}: clicks");
        }
    }
}
