//! The audio half of `diagnostic/wheel_slip_sim.py`: scenario 7. Scenarios 1-6 (the
//! detector) are `beam-reports/tests/wheel_slip_sim.rs`.
//!
//! The Python scenario swaps in a fake HRTF (`_FakeHRTF`, left ear louder for
//! 0 < az < 180) that also records every azimuth asked for. `HrtfSet` is a concrete type
//! with no injection point, so this uses the real MIT KEMAR set for the left/right checks,
//! and reads the azimuth the voice asks for straight from its smoothed vector
//! (`degrees(atan2(_slip_az_y, _slip_az_x)) % 360`, exactly the value passed to
//! `get_hrir`) after every block.

use std::f64::consts::PI;
use std::sync::Arc;

use beam_audio::AudioController;
use beam_audio::consts::{SLIP_HARMONIC_TOP_HZ, SLIP_LOCK_HZ, SLIP_PITCH_RISE_OCT, SLIP_SPIN_HZ};
use beam_audio::controller::TelemetryState;
use beam_audio::dsp;
use beam_base::pyfmt::py_mod;

fn rms(x: &[f32]) -> f64 {
    let v: Vec<f64> = x.iter().map(|&s| s as f64 * s as f64).collect();
    dsp::mean(&v).sqrt()
}

#[derive(Clone, Copy)]
struct Slip {
    slip_active: bool,
    slip_kind: i64,
    slip_mag: f64,
    slide_active: bool,
    slide_deg: f64,
    slip_azimuth_deg: f64,
    slip_ratio: Option<f64>,
}

const BASE: Slip = Slip {
    slip_active: true,
    slip_kind: 1,
    slip_mag: 5.0,
    slide_active: false,
    slide_deg: 0.0,
    slip_azimuth_deg: 60.0,
    slip_ratio: None,
};

fn state(s: Slip) -> TelemetryState {
    TelemetryState {
        slip_active: Some(s.slip_active),
        slip_kind: Some(s.slip_kind),
        slip_mag: Some(s.slip_mag),
        slide_active: Some(s.slide_active),
        slide_deg: Some(s.slide_deg),
        slip_azimuth_deg: Some(s.slip_azimuth_deg),
        slip_ratio: s.slip_ratio,
        ..TelemetryState::default()
    }
}

/// The azimuth the voice hands `get_hrir` this block.
fn asked_azimuth(ctrl: &AudioController) -> f64 {
    let v = ctrl.voice.lock();
    py_mod(v._slip_az_y.atan2(v._slip_az_x).to_degrees(), 360.0)
}

/// `_render`: the last block's left/right RMS; `azimuths` collects every block's.
fn render(
    ctrl: &AudioController,
    s: Slip,
    blocks: usize,
    frames: usize,
    azimuths: &mut Vec<f64>,
) -> (f64, f64) {
    ctrl.update_telemetry_state(&state(s));
    let (mut left, mut right) = (0.0, 0.0);
    for _ in 0..blocks {
        let (l, r) = ctrl.render_block(frames);
        azimuths.push(asked_azimuth(ctrl));
        left = rms(&l);
        right = rms(&r);
    }
    (left, right)
}

fn mags_f32(x: &[f32]) -> Vec<f64> {
    dsp::rfft_f32(x, x.len())
        .iter()
        .map(|c| (c.re as f64).hypot(c.im as f64))
        .collect()
}

#[test]
fn scenario_7_audio_voices_render_localise_toward_travel_fade_out() {
    let ctrl: Arc<AudioController> = AudioController::new();
    ctrl.regenerate_waveforms();
    let tables = ctrl.slip_voice_tables();
    let (spin, lock, nlo, _nhi) = (&tables.0, &tables.1, &tables.2, &tables.3);
    for (name, tab, f0) in [("spin", spin, SLIP_SPIN_HZ), ("lock", lock, SLIP_LOCK_HZ)] {
        let mag = mags_f32(tab);
        let peak = mag.iter().cloned().fold(f64::MIN, f64::max);
        let top_harm = mag
            .iter()
            .rposition(|&m| m > peak * 1e-3)
            .expect("some harmonic");
        assert!(
            top_harm as f64 * f0 > 3000.0,
            "{name} table reaches ~{SLIP_HARMONIC_TOP_HZ} Hz: {}",
            top_harm as f64 * f0
        );
    }
    let lock_mag = mags_f32(lock);
    assert!(
        lock_mag[2] < lock_mag[3] * 1e-3,
        "lock table has odd harmonics only"
    );
    assert!(
        (nlo[0] as f64 - *nlo.last().unwrap() as f64).abs() < 0.5,
        "noise loop is seamless (periodic)"
    );

    let path = beam_testutil::repo_root().join("hrtf_mit_kemar.npz");
    assert!(ctrl.load_hrtf(&path, None, None, None), "HRTF set loads");
    ctrl.lock.lock()._hrtf_user_enabled = true;
    let mut az = Vec::new();
    let (l, r) = render(&ctrl, BASE, 40, 480, &mut az);
    assert!(l > 1e-4, "spin voice audible: {l}");
    assert!(
        l > r * 1.2,
        "travel +60 (left) renders left-heavy: L {l:.4} R {r:.4}"
    );
    let last = *az.last().unwrap();
    assert!(
        (last - 60.0).abs() < 2.0,
        "azimuth converged near 60: {last}"
    );

    let slide = Slip {
        slip_active: false,
        slide_active: true,
        slide_deg: 30.0,
        slip_azimuth_deg: -30.0,
        ..BASE
    };
    let (l, r) = render(&ctrl, slide, 40, 480, &mut az);
    assert!(
        r > l * 1.2,
        "slide voice alone, travel right, renders right-heavy: L {l:.4} R {r:.4}"
    );

    // Swing from +170 to -170 across dead astern: the smoothed azimuth must stay behind.
    render(
        &ctrl,
        Slip {
            slip_azimuth_deg: 170.0,
            ..BASE
        },
        40,
        480,
        &mut az,
    );
    az.clear();
    render(
        &ctrl,
        Slip {
            slip_azimuth_deg: -170.0,
            ..BASE
        },
        20,
        480,
        &mut az,
    );
    let front: Vec<f64> = az
        .iter()
        .copied()
        .filter(|a| py_mod(*a, 360.0) < 90.0 || py_mod(*a, 360.0) > 270.0)
        .collect();
    assert!(
        front.is_empty(),
        "swing across 180 never passes through the front: {front:?}"
    );

    let off = Slip {
        slip_active: false,
        slide_active: false,
        ..BASE
    };
    let (l, r) = render(&ctrl, off, 150, 480, &mut az);
    assert!(l < 1e-5 && r < 1e-5, "both voices fade to silence: {l} {r}");
    assert!(
        ctrl.voice.lock()._slip_overlap_L.is_none(),
        "overlap tail released when silent"
    );

    // Pitch follows the slip ratio: measure the fundamental on the stereo path.
    {
        let mut s = ctrl.lock.lock();
        s._hrtf = None;
        s._hrtf_user_enabled = false;
    }
    let pitch = |ratio: f64| -> f64 {
        let frames = 4800usize;
        ctrl.update_telemetry_state(&state(Slip {
            slip_ratio: Some(ratio),
            slip_azimuth_deg: 0.0,
            ..BASE
        }));
        let mut out = (Vec::new(), Vec::new());
        for _ in 0..20 {
            out = ctrl.render_block(frames);
        }
        // np.hanning(M): 0.5 - 0.5 cos(2 pi n / (M - 1)).
        let m = frames as f64;
        let sig: Vec<f64> = out
            .0
            .iter()
            .enumerate()
            .map(|(n, &x)| x as f64 * (0.5 - 0.5 * (2.0 * PI * n as f64 / (m - 1.0)).cos()))
            .collect();
        let spec: Vec<f64> = dsp::rfft(&sig, frames).iter().map(|c| c.norm()).collect();
        let freqs = dsp::rfftfreq(frames, 1.0 / ctrl.samplerate());
        let mut best = (f64::MIN, 0.0);
        for (f, s) in freqs.iter().zip(&spec) {
            if *f > 150.0 && *f < 600.0 && *s > best.0 {
                best = (*s, *f);
            }
        }
        best.1
    };
    let lo = pitch(0.0);
    let hi = pitch(1.0);
    assert!(
        (lo - SLIP_SPIN_HZ).abs() < 15.0,
        "ratio 0 plays the base spin pitch: {lo}"
    );
    assert!(
        (hi - SLIP_SPIN_HZ * 2f64.powf(SLIP_PITCH_RISE_OCT)).abs() < 15.0,
        "ratio 1 plays the top of the range: {hi}"
    );

    // Stereo fallback keeps the side.
    let (l, r) = render(
        &ctrl,
        Slip {
            slip_azimuth_deg: 90.0,
            ..BASE
        },
        60,
        480,
        &mut az,
    );
    assert!(
        l > r,
        "stereo fallback: left travel pans left: L {l:.4} R {r:.4}"
    );
}
