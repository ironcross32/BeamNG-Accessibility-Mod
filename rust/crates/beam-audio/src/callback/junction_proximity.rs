//! Stop-bar tone (audio.py:7018) via `_mix_junction_proximity` (2413-2460) and
//! `_stop_bar_render_state` (2381-2411). Python runs the whole mix under `self.lock`, so
//! this takes `&mut Shared`. Not ducked. A2.

use crate::callback::block::Block;
use crate::consts::StopBarState;
use crate::controller::{JsonMap, Shared, Voice};

/// 7018: `self._mix_junction_proximity(bufL, bufR, frames)`.
pub fn render(b: &mut Block<'_>, _v: &mut Voice) {
    let now = b.clock.monotonic();
    let mut s = b.shared.lock();
    mix_junction_proximity(&mut s, now, b.buf_l, b.buf_r, b.frames);
}

/// `_mix_junction_proximity`, the caller holding the lock. `now` is `time.monotonic()`.
pub fn mix_junction_proximity(
    s: &mut Shared,
    now: f64,
    buf_l: &mut [f32],
    buf_r: &mut [f32],
    frames: usize,
) {
    // PLACEHOLDER (wave 2): adds silence.
    let _ = (s, now, buf_l, buf_r, frames);
}

/// `_stop_bar_render_state(target)`: advance the stopped and on-bar hysteresis for one
/// block and return the state.
pub fn stop_bar_render_state(s: &mut Shared, target: Option<&JsonMap>) -> StopBarState {
    // PLACEHOLDER (wave 2).
    let _ = (s, target);
    StopBarState::Silent
}
