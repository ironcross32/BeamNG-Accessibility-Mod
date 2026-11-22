//! Off-road beacon pulse (audio.py:5516-5575). Inside the road duck bus. A2.
//!
//! The pulsed road beacon is exclusively for acquiring the road from off-road.

use crate::callback::block::Block;
use crate::callback::common::{Wave, render_directional_pulse};
use crate::consts::*;
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let frames = b.frames;
    let road_beacon_active = s.road_active && !s.road_on_road && s.road_beacon_available;
    if road_beacon_active {
        if let Some(base_pulse) = b.waves.ROAD_BEEP_WAVEFORM.as_ref() {
            let dt_rd = frames as f64 / b.sr;
            let rate_hz = road_beacon_rate_hz(s.road_distance);
            let interval_sec = 1.0 / py_max(0.01, rate_hz);

            v._road_beep_timer += dt_rd;
            if v._road_beep_timer >= interval_sec {
                v._road_beep_timer = 0.0;
                // HRTF-convolved at `road_bearing % 360`, else the negated stereo pan:
                // exactly `_render_directional_pulse`.
                let (l, r) = render_directional_pulse(
                    s._hrtf.as_deref(),
                    s._hrtf_user_enabled,
                    base_pulse,
                    s.road_bearing,
                );
                v._road_pulse_L = Some(l);
                v._road_pulse_R = Some(r);
                v._road_playback_pos = 0.0;
            }

            if v._road_playback_pos >= 0.0 {
                if let (Some(pl), Some(pr)) = (v._road_pulse_L.clone(), v._road_pulse_R.clone()) {
                    v._road_playback_pos =
                        play_pulse(b.buf_l, b.buf_r, &pl, &pr, v._road_playback_pos, frames);
                }
            }
        }
    }

    if !road_beacon_active && v._road_playback_pos >= 0.0 {
        v._road_playback_pos = -1.0;
        v._road_pulse_L = None;
        v._road_pulse_R = None;
    }
}

/// Mix one block of a pre-rendered stereo pulse from `int(pos)`; returns the next cursor
/// (`-1.0` once the pulse is done). Shared with the route beacon.
pub(crate) fn play_pulse(
    buf_l: &mut [f32],
    buf_r: &mut [f32],
    pulse_l: &Wave,
    pulse_r: &Wave,
    pos: f64,
    frames: usize,
) -> f64 {
    let pulse_len = pulse_l.len() as i64;
    let start_i = pos.trunc() as i64;
    let num_to_mix = (frames as i64).min(pulse_len - start_i);
    if num_to_mix > 0 && start_i >= 0 {
        let st = start_i as usize;
        for k in 0..num_to_mix as usize {
            buf_l[k] += pulse_l[st + k];
            buf_r[k] += pulse_r[st + k];
        }
    }
    let next_pos = start_i + frames as i64;
    if next_pos >= pulse_len {
        -1.0
    } else {
        next_pos as f64
    }
}
