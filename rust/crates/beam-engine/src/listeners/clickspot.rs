//! Owner: E4. Clickspot listener, staged list, activation and toggle (`beamtel.py:3574-3784, 7314-7356`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E4.

use std::sync::Arc;

use crate::compat::beam_audio::controller::AudioController;
use crate::timer::Event;
use crate::not_ported;

/// `clickspot_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn clickspot_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let _ = (audio_controller, stop_event);
    not_ported!("clickspot_listener");
}

/// `_send_clickspot_cmd(command)` (port CLICKSPOT_CMD_PORT).
pub fn send_clickspot_cmd(command: &str) {
    crate::net::send_cmd(crate::net::CLICKSPOT_CMD_PORT, command, "clickspot command");
}

/// `_activate_clickspot(cache_idx)`: SNAP, then delayed press/release.
pub fn activate_clickspot(cache_idx: i64) {
    let _ = cache_idx;
    not_ported!("activate_clickspot");
}

/// `toggle_clickspot_mode(audio_controller)` (F9 Ctrl+Shift+C).
pub fn toggle_clickspot_mode(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    not_ported!("toggle_clickspot_mode");
}
