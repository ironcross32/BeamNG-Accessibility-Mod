//! Compass click, highlight, cam click, cam highlight (audio.py:4371-4627). Each is a
//! resampled one-shot: the HRTF convolution its trigger made (played at unity), or the
//! plain waveform equal-power panned. A slot whose waveform is missing keeps its cursor.

use crate::callback::block::Block;
use crate::callback::common::{Wave, pan_gains, play_resampled};
use crate::controller::Voice;

#[allow(clippy::too_many_arguments)]
fn play_click(
    buf_l: &mut [f32],
    buf_r: &mut [f32],
    frames: usize,
    pos: &mut f64,
    pitch_mult: f64,
    pan_pos: f64,
    use_hrtf: bool,
    conv_l: &Option<Wave>,
    conv_r: &Option<Wave>,
    waveform: &Option<Wave>,
) {
    if *pos < 0.0 {
        return;
    }
    if let (true, Some(l)) = (use_hrtf, conv_l) {
        let r = conv_r.as_ref().unwrap_or(l);
        play_resampled(buf_l, buf_r, l, r, 1.0, 1.0, pos, pitch_mult, frames);
    } else if let Some(w) = waveform {
        let (lg, rg) = pan_gains(pan_pos);
        play_resampled(buf_l, buf_r, w, w, lg, rg, pos, pitch_mult, frames);
    }
}

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let w = b.waves;
    play_click(
        b.buf_l,
        b.buf_r,
        b.frames,
        &mut v._click_playback_pos,
        v._click_pitch_mult,
        v._click_pan_pos,
        s._click_use_hrtf,
        &s._click_conv_L,
        &s._click_conv_R,
        &w.CLICK_WAVEFORM,
    );
    play_click(
        b.buf_l,
        b.buf_r,
        b.frames,
        &mut v._highlight_click_playback_pos,
        v._highlight_click_pitch_mult,
        v._highlight_click_pan_pos,
        s._hclick_use_hrtf,
        &s._hclick_conv_L,
        &s._hclick_conv_R,
        &w.HIGHLIGHT_CLICK_WAVEFORM,
    );
    // Camera compass click playback
    play_click(
        b.buf_l,
        b.buf_r,
        b.frames,
        &mut v._cam_click_playback_pos,
        v._cam_click_pitch_mult,
        v._cam_click_pan_pos,
        s._cam_click_use_hrtf,
        &s._cam_click_conv_L,
        &s._cam_click_conv_R,
        &w.CAM_CLICK_WAVEFORM,
    );
    // Camera compass highlight click playback
    play_click(
        b.buf_l,
        b.buf_r,
        b.frames,
        &mut v._cam_highlight_click_playback_pos,
        v._cam_highlight_click_pitch_mult,
        v._cam_highlight_click_pan_pos,
        s._cam_hclick_use_hrtf,
        &s._cam_hclick_conv_L,
        &s._cam_hclick_conv_R,
        &w.CAM_HIGHLIGHT_CLICK_WAVEFORM,
    );
}
