//! Old Cannon elevation pulse, centred, unducked (audio.py:5183-5241). Gated on the
//! PREVIOUS block's `v._dock_gate_env`. A2.
//!
//! Deliberately centred: left/right belongs to the scanner's HRTF image, low/high pitch
//! says raise/lower. Withheld until the launcher data gives a valid pre-shot speed;
//! line-of-sight would pretend to be a ballistic null.

use std::f64::consts::PI;

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::consts::*;
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let frames = b.frames;
    let sr = b.sr;
    let cannon_gate = s.scan_active
        && !s.coupler_active
        && v._dock_gate_env <= 0.0
        && s.cannon_aim_have
        && s.cannon_aim_solution_available
        && s.cannon_aim_reachable
        && (b.clock.perf_counter() - s.cannon_aim_stamp) < CANNON_AIM_STALE_S;
    let cannon_error = s.cannon_target_angle - s.cannon_elevation;
    if cannon_gate && cannon_error.abs() > CANNON_AIM_DEADZONE_DEG {
        let norm = py_min(
            1.0,
            (cannon_error.abs() - CANNON_AIM_DEADZONE_DEG)
                / py_max(1e-6, CANNON_AIM_FULL_ERROR_DEG - CANNON_AIM_DEADZONE_DEG),
        );
        let pulse_rate =
            CANNON_AIM_MIN_RATE_HZ + (CANNON_AIM_MAX_RATE_HZ - CANNON_AIM_MIN_RATE_HZ) * norm;
        let freq = if cannon_error > 0.0 {
            CANNON_AIM_RAISE_HZ
        } else {
            CANNON_AIM_LOWER_HZ
        };
        let amp_db = CANNON_AIM_MIN_DB + (CANNON_AIM_MAX_DB - CANNON_AIM_MIN_DB) * norm;
        let gain = 10.0f64.powf(amp_db / 20.0);
        let pulse_inc = pulse_rate / sr;
        let carrier_inc = freq / sr;
        for k in 0..frames {
            let cyc = v._cannon_aim_pulse_phase + pulse_inc * k as f64;
            let frac = py_mod(cyc, 1.0) / CANNON_AIM_DUTY;
            let env = if frac < 1.0 {
                let x = (PI * frac.min(1.0)).sin();
                x * x
            } else {
                0.0
            };
            let carrier = v._cannon_aim_carrier_phase + carrier_inc * k as f64;
            let mono = ((2.0 * PI * carrier).sin() * env * gain) as f32;
            b.buf_l[k] += mono;
            b.buf_r[k] += mono;
        }
        v._cannon_aim_pulse_phase =
            py_mod(v._cannon_aim_pulse_phase + pulse_rate * frames as f64 / sr, 1.0);
        v._cannon_aim_carrier_phase =
            py_mod(v._cannon_aim_carrier_phase + freq * frames as f64 / sr, 1.0);
    } else {
        // A correction resuming after a null starts with a pulse at once instead of
        // returning halfway through the cycle it was in before alignment.
        v._cannon_aim_pulse_phase = 0.0;
    }
}
