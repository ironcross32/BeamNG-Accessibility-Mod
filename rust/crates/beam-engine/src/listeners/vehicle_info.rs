//! Owner: E4. Vehicle information listener and sender (`beamtel.py:4012-4098, 7594-7600`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E4.

use crate::timer::Event;

/// `vehicle_info_listener(stop_event)`. Runs until `stop_event` is set.
pub fn vehicle_info_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("vehicle_info_listener");
}

/// `_send_vehicle_info_cmd(command)` (port VEHICLE_INFO_CMD_PORT).
pub fn send_vehicle_info_cmd(command: &str) {
    crate::net::send_cmd(
        crate::net::VEHICLE_INFO_CMD_PORT,
        command,
        "vehicle info command",
    );
}
