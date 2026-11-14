//! Owner: E2. Fuel stations: listener, timers, browser, events and the key (`beamtel.py:6497-6690`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E2.

use crate::timer::Event;
use crate::not_ported;

pub const FUEL_REPLY_TIMEOUT_S: f64 = 2.5;

/// `_send_fuel_cmd(command)` (port FUEL_CMD_PORT).
pub fn send_fuel_cmd(command: &str) {
    crate::net::send_cmd(crate::net::FUEL_CMD_PORT, command, "fuel station command");
}

/// `fuel_station_listener(stop_event)`. Runs until `stop_event` is set.
pub fn fuel_station_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("fuel_station_listener");
}

/// `open_fuel_stations(force_list=False)` (F9 Shift+F).
pub fn open_fuel_stations(force_list: bool) {
    let _ = force_list;
    not_ported!("open_fuel_stations");
}
