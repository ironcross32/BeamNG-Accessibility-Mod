//! Owner: E4. Vehicle slot listener (`beamtel.py:4101-4145`); writes `state::slots()`.
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E4.

use crate::timer::Event;
use crate::not_ported;

/// `slot_listener(stop_event)`. Runs until `stop_event` is set.
pub fn slot_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("slot_listener");
}
