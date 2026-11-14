//! Shift light triangle tone (audio.py:4629-4635). Ported by AA.

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    if !b.snap.shift_on {
        return;
    }
    let inc = b.snap._phase_inc_shift;
    let amp = b.snap.shift_amp;
    let mut last = 0.0;
    for k in 0..b.frames {
        // t = (arange(frames) * inc + phase) % 1.0; s = amp * (2|2t - 1| - 1), float64.
        let t = py_mod(k as f64 * inc + v._phase_shift, 1.0);
        let s = amp * (2.0 * (2.0 * t - 1.0).abs() - 1.0);
        b.buf_l[k] = (b.buf_l[k] as f64 + s) as f32;
        b.buf_r[k] = (b.buf_r[k] as f64 + s) as f32;
        last = t;
    }
    if b.frames > 0 {
        v._phase_shift = py_mod(last + inc, 1.0);
    }
}
