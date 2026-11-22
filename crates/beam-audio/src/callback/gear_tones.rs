//! In-flight gear tones (audio.py:7020) via `_mix_gear_tones` (2477-2488), under the
//! lock as in Python. Ported by AA (twelve lines; the tone itself is rendered on the
//! caller's thread by `gear_report::render_tone`).

use crate::callback::block::Block;
use crate::controller::{Shared, Voice};

/// 7020: `self._mix_gear_tones(bufL, bufR, frames)`.
pub fn render(b: &mut Block<'_>, _v: &mut Voice) {
    let mut s = b.shared.lock();
    mix_gear_tones(&mut s, b.buf_l, b.buf_r, b.frames);
}

/// `_mix_gear_tones`, the caller holding the lock.
pub fn mix_gear_tones(s: &mut Shared, buf_l: &mut [f32], buf_r: &mut [f32], frames: usize) {
    s._gear_tones.retain_mut(|entry| {
        let count = frames.min(entry.buf.len().saturating_sub(entry.pos));
        for k in 0..count {
            buf_l[k] += entry.buf[entry.pos + k];
            buf_r[k] += entry.buf[entry.pos + k];
        }
        entry.pos += count;
        entry.pos < entry.buf.len()
    });
}
