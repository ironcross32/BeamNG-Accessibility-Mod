//! Docking instrument: panned pulse, centred beat pair, lock chime (audio.py:6459-6812). Writes `v._dock_gate_env`, read next block by scanner, cannon_aim and hydro_steer. The frame applies the dock duck bus around it. A3.

use crate::callback::block::Block;
use crate::controller::Voice;

/// PLACEHOLDER (wave 2): adds silence. Fill with the Python block named above.
pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let _ = (b, v);
}
