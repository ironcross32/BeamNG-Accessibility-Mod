//! Owner: E3. Wheel report listener (`beamtel.py:2444-2475`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E3.

use crate::timer::Event;
use crate::not_ported;

/// `wheel_report_listener(stop_event)`. Runs until `stop_event` is set.
pub fn wheel_report_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("wheel_report_listener");
}
