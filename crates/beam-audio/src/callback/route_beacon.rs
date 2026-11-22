//! Route beacon pulse (audio.py:5577-5622). Inside the road duck bus. A2.
//!
//! Where the map route's destination is, as the crow flies. Structurally the road beacon
//! (rate timer, one convolved pulse, mixed across blocks) because one learned relationship
//! between pulse rate and closeness is worth more than a second instrument; the distance
//! scale and the timbre differ, since the two can sound at once and point different ways.

use crate::callback::block::Block;
use crate::callback::common::render_directional_pulse;
use crate::callback::road_beacon::play_pulse;
use crate::consts::*;
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let frames = b.frames;
    let route_beacon_active = s.route_beacon_on && s.route_beacon_have;
    if route_beacon_active {
        if let Some(base_pulse) = b.waves.ROUTE_BEACON_WAVEFORM.as_ref() {
            let dt_rb = frames as f64 / b.sr;
            let rate_hz = route_beacon_rate_hz(s.route_beacon_distance);
            let interval_sec = 1.0 / py_max(0.01, rate_hz);

            v._route_beacon_timer += dt_rb;
            if v._route_beacon_timer >= interval_sec {
                v._route_beacon_timer = 0.0;
                // Convolved once per pulse at the bearing current when it fires: re-aiming
                // a pulse mid-flight would smear the image the HRTF exists to place.
                let (l, r) = render_directional_pulse(
                    s._hrtf.as_deref(),
                    s._hrtf_user_enabled,
                    base_pulse,
                    s.route_beacon_bearing,
                );
                v._route_beacon_pulse_L = Some(l);
                v._route_beacon_pulse_R = Some(r);
                v._route_beacon_playback_pos = 0.0;
            }

            if v._route_beacon_playback_pos >= 0.0 {
                if let (Some(pl), Some(pr)) = (
                    v._route_beacon_pulse_L.clone(),
                    v._route_beacon_pulse_R.clone(),
                ) {
                    v._route_beacon_playback_pos = play_pulse(
                        b.buf_l,
                        b.buf_r,
                        &pl,
                        &pr,
                        v._route_beacon_playback_pos,
                        frames,
                    );
                }
            }
        }
    }

    if !route_beacon_active && v._route_beacon_playback_pos >= 0.0 {
        v._route_beacon_playback_pos = -1.0;
        v._route_beacon_pulse_L = None;
        v._route_beacon_pulse_R = None;
    }
}
