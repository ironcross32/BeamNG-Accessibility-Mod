//! Free-camera line-up guide FM tone (audio.py:6974-6977) via `_render_lineup`
//! (3311-3399): amplitude/index ramped across the block, HRIR crossfade when the azimuth
//! moves. Uses `beam_reports::camera_lineup::CARRIER_HZ`. Not ducked. A1.

use crate::callback::block::Block;
use crate::controller::Voice;
use crate::hrtf::HrtfSet;

/// 6977: `self._render_lineup(bufL, bufR, frames, lineup_live, lineup_target)`.
pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let hrtf = b.snap.hrtf();
    render_lineup(
        v,
        hrtf,
        b.sr,
        b.buf_l,
        b.buf_r,
        b.frames,
        b.snap.lineup_live,
        b.snap.lineup_target,
    );
}

/// `_render_lineup(bufL, bufR, frames, live, target)`. `hrtf` is `self._hrtf` only when
/// `_hrtf_user_enabled`.
#[allow(clippy::too_many_arguments)]
pub fn render_lineup(
    v: &mut Voice,
    hrtf: Option<&HrtfSet>,
    sr: f64,
    buf_l: &mut [f32],
    buf_r: &mut [f32],
    frames: usize,
    live: bool,
    target: (f64, f64, f64, f64),
) {
    // PLACEHOLDER (wave 2): adds silence.
    let _ = (v, hrtf, sr, buf_l, buf_r, frames, live, target);
}
