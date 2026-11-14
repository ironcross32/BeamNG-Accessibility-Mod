//! Owner: E3. Implement, dock, ramp and cannon phrasing and their constants (`beamtel.py:1480-2115`). Pure functions over `state::DockReading`, `state::RampSelf`, `state::CannonShot`..
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E3.

use crate::not_ported;
use crate::state::{CannonShot, DockReading, RampSelf};

pub const IMPL_DOCK_LEVEL_M: f64 = 0.05;
pub const CANNON_READOUT_STALE_S: f64 = 1.5;

/// `_dock_phrase(dock)`: the F9+I cane tap.
pub fn dock_phrase(dock: &DockReading) -> String {
    let _ = dock;
    not_ported!("dock_phrase");
    String::new()
}

/// `_ramp_self_phrase(state)`: the deck readout.
pub fn ramp_self_phrase(state: &RampSelf) -> String {
    let _ = state;
    not_ported!("ramp_self_phrase");
    String::new()
}

/// `_band_name(kind, idx, count)`.
pub fn band_name(kind: &str, idx: i64, count: i64) -> String {
    let _ = (kind, idx, count);
    not_ported!("band_name");
    String::new()
}

/// `_cannon_shot_phrase(shot)`.
pub fn cannon_shot_phrase(shot: &CannonShot) -> String {
    let _ = shot;
    not_ported!("cannon_shot_phrase");
    String::new()
}
