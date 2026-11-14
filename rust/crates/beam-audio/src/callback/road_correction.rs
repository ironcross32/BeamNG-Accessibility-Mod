//! Road correction pips / unwind doublet and the settled sweep (audio.py:5624-5654), with
//! `_render_road_correction_block` (2515-2620). Inside the road duck bus. A2.
//!
//! `_stop_road_correction_voice` is `AudioController::stop_road_correction_voice_locked`.

use crate::callback::block::Block;
use crate::controller::Voice;
use crate::hrtf::HrtfSet;

/// PLACEHOLDER (wave 2): adds silence. 5624-5654: the correction block when
/// `road_active and road_on_road` (else stop the voice), then the settled sweep.
pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let _ = (b, v);
}

/// `_render_road_correction_block(frames, active, bearing_deg, severity, phase)`:
/// `None` where Python returns None, else the `(L, R)` float32 block.
/// `hrtf` is `self._hrtf` only when `_hrtf_user_enabled`; `amp` is `_road_correction_amp`.
#[allow(clippy::too_many_arguments)]
pub fn render_road_correction_block(
    v: &mut Voice,
    hrtf: Option<&HrtfSet>,
    sr: f64,
    amp: f64,
    frames: usize,
    active: bool,
    bearing_deg: f64,
    severity: f64,
    phase: &str,
) -> Option<(Vec<f32>, Vec<f32>)> {
    // PLACEHOLDER (wave 2).
    let _ = (
        v,
        hrtf,
        sr,
        amp,
        frames,
        active,
        bearing_deg,
        severity,
        phase,
    );
    None
}
