//! Owner: E4. Vehicle-specific bindings listener and sender (`beamtel.py:3787-3857, 7359-7367`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E4.

use crate::timer::Event;

/// `vehicle_bindings_listener(stop_event)`. Runs until `stop_event` is set.
pub fn vehicle_bindings_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("vehicle_bindings_listener");
}

/// `_send_vehicle_bindings_cmd(command)` (port VEHICLE_BINDINGS_CMD_PORT).
pub fn send_vehicle_bindings_cmd(command: &str) {
    crate::net::send_cmd(
        crate::net::VEHICLE_BINDINGS_CMD_PORT,
        command,
        "vehicle bindings command",
    );
}
