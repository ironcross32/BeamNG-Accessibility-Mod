//! Placement ping (audio.py:4334-4369): resampled with linear interpolation, either the
//! HRTF convolution the trigger made (unity gain) or the rung's ping, equal-power panned.

use crate::callback::block::Block;
use crate::callback::common::{pan_gains, play_resampled};
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    if v._ping_playback_pos < 0.0 {
        return;
    }
    let snap = b.snap;
    let waves = b.waves.PLACEMENT_PING_WAVEFORMS.as_deref().unwrap_or(&[]);
    let (ping_l, ping_r, lg, rg) = match (&snap._ping_conv_L, &snap._ping_conv_R) {
        (Some(l), Some(r)) if snap._ping_use_hrtf => (l.clone(), r.clone(), 1.0, 1.0),
        _ if !waves.is_empty() => {
            // waves[min(rung, len - 1)]; a negative rung indexes from the end, as numpy.
            let n = waves.len() as i64;
            let mut i = v._ping_rung.min(n - 1);
            if i < 0 {
                i = i.rem_euclid(n);
            }
            let w = waves[i as usize].clone();
            let (lg, rg) = pan_gains(v._ping_pan_pos);
            (w.clone(), w, lg, rg)
        }
        _ => {
            v._ping_playback_pos = -1.0;
            return;
        }
    };
    play_resampled(
        b.buf_l,
        b.buf_r,
        &ping_l,
        &ping_r,
        lg,
        rg,
        &mut v._ping_playback_pos,
        v._ping_pitch_mult,
        b.frames,
    );
}
