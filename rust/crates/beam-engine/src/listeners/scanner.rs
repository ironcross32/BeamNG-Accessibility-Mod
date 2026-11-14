//! Owner: E2. Vehicle scanner listener and callouts, the scanner command sender, gear push and `toggle_scan_mode` (`beamtel.py:796-1120, 3223-3253, 6918-6963`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E2.

use std::sync::Arc;

use crate::compat::beam_audio::controller::AudioController;
use crate::timer::Event;
use crate::not_ported;
use crate::state::State;

/// `scanner_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn scanner_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let _ = (audio_controller, stop_event);
    not_ported!("scanner_listener");
}

/// `scanner_callout_thread_fn(stop_event)`. Runs until `stop_event` is set.
pub fn scanner_callout_thread_fn(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("scanner_callout_thread_fn");
}

/// `_send_scanner_cmd(command)` (port SCANNER_CMD_PORT).
pub fn send_scanner_cmd(command: &str) {
    crate::net::send_cmd(crate::net::SCANNER_CMD_PORT, command, "scanner command");
}

/// `_push_gear_direction(gear)`. Called by the telemetry loop while it holds `state_lock`.
pub fn push_gear_direction(st: &mut State, gear: &str) {
    let _ = (st, gear);
    not_ported!("push_gear_direction");
}

/// `toggle_scan_mode(audio_controller)`.
pub fn toggle_scan_mode(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    not_ported!("toggle_scan_mode");
}
