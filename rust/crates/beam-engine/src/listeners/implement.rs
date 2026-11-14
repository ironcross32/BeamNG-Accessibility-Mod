//! Owner: E3. Implement proximity / docking / ramp / cannon listener and its sender (`beamtel.py:2700-3154`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E3.

use std::sync::Arc;

use crate::compat::beam_audio::controller::AudioController;
use crate::timer::Event;

/// `implement_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn implement_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let _ = (audio_controller, stop_event);
    not_ported!("implement_listener");
}

/// `_send_implement_cmd(command)` (port IMPLEMENT_CMD_PORT).
pub fn send_implement_cmd(command: &str) {
    crate::net::send_cmd(
        crate::net::IMPLEMENT_CMD_PORT,
        command,
        "implement proximity command",
    );
}
