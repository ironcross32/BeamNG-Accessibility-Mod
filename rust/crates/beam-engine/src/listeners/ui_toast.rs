//! Owner: E2. UI toast listener, port 4579 (`ui_listener`, `beamtel.py:738-793`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E2.

use crate::timer::Event;

/// `ui_listener(stop_event)`. Runs until `stop_event` is set.
pub fn ui_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("ui_listener");
}
