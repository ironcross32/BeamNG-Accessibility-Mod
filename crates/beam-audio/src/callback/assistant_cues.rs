//! Queued driving-assistant cues (audio.py:7019) via `_mix_assistant_cues` (2490-2506),
//! under the lock as in Python. Ported by AA (the shared delayed-pulse queue).

use crate::callback::block::Block;
use crate::callback::common::mix_pulse_queue;
use crate::controller::{Shared, Voice};

/// 7019: `self._mix_assistant_cues(bufL, bufR, frames)`.
pub fn render(b: &mut Block<'_>, _v: &mut Voice) {
    let mut s = b.shared.lock();
    mix_assistant_cues(&mut s, b.buf_l, b.buf_r, b.frames);
}

/// `_mix_assistant_cues`, the caller holding the lock.
pub fn mix_assistant_cues(s: &mut Shared, buf_l: &mut [f32], buf_r: &mut [f32], frames: usize) {
    mix_pulse_queue(&mut s._assistant_cue_queue, buf_l, buf_r, frames);
}
