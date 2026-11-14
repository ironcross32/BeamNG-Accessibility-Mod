//! Owner: E4. Accessible node grabber listener, Ctrl+Space hook, toggle (`beamtel.py:3419-3571, 7239-7311`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E4.

use std::sync::Arc;

use crate::compat::beam_audio::controller::AudioController;
use crate::timer::Event;
use crate::not_ported;

/// `nodegrab_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn nodegrab_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let _ = (audio_controller, stop_event);
    not_ported!("nodegrab_listener");
}

/// `_send_nodegrab_cmd(command)` (port NODEGRAB_CMD_PORT).
pub fn send_nodegrab_cmd(command: &str) {
    crate::net::send_cmd(crate::net::NODEGRAB_CMD_PORT, command, "node grabber command");
}

/// `toggle_nodegrab_mode(audio_controller)` (F9 Ctrl+N).
pub fn toggle_nodegrab_mode(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    not_ported!("toggle_nodegrab_mode");
}
