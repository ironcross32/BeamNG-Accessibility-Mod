//! Vehicle scanner (audio.py:4926-5177): bearing EMA, beep train (bright when aligned),
//! steering-locked steady tone, HRTF, scanner diagnostic capture. A2.
//!
//! `smooth_bearing` (4926-4947) runs BEFORE the frame copies the scanner duck-bus mix;
//! `render` (4949-5177) runs inside it. Gated on the PREVIOUS block's `v._dock_gate_env`
//! and on `coupler_active`. The smoothed bearing replaces `scan_bearing` for this voice
//! (Python rebinds the local); `v._sm_scan_bearing` carries it.

use crate::callback::block::Block;
use crate::controller::Voice;

/// PLACEHOLDER (wave 2): 4926-4947, the shortest-angle EMA on the bearing.
pub fn smooth_bearing(b: &mut Block<'_>, v: &mut Voice) {
    let _ = (b, v);
}

/// PLACEHOLDER (wave 2): adds silence. 4957-5177 (beeps, steady tone, else-branch reset).
pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let _ = (b, v);
}
