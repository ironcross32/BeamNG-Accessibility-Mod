//! Owner: E3. Trailer angle listener and `_trailer_artic_norm` (`beamtel.py:2374-2441, 2554-2572`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E3.

use crate::timer::Event;
use crate::not_ported;
use crate::state::{State, TRAILER_FULL_DEG, TRAILER_STALE_SEC};

/// `trailer_angle_listener(stop_event)`. Runs until `stop_event` is set.
pub fn trailer_angle_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("trailer_angle_listener");
}

/// `_trailer_artic_norm()`: the trailer angle on the -1..1 scale, or 0.0 for silence.
/// Ported in full by EA because `state::mcp_snapshot_state` needs it.
pub fn trailer_artic_norm(st: &State) -> f64 {
    let t = &st.trailer;
    if t.last_trailer_id.is_none() || t.last_trailer_stamp <= 0.0 {
        return 0.0;
    }
    if (crate::timer::monotonic() - t.last_trailer_stamp) > TRAILER_STALE_SEC {
        return 0.0;
    }
    // max(-1.0, min(1.0, x)): NaN passes through min and max unchanged, as in Python.
    let x = t.last_trailer_deg / TRAILER_FULL_DEG;
    let m = if 1.0 < x { 1.0 } else { x };
    if -1.0 > m { -1.0 } else { m }
}
