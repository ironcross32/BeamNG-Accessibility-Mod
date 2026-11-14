//! Owner: E4. Camera info listener, commands, diagnostics and toggle (`beamtel.py:3256-3416, 7159-7236`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E4.

use std::sync::Arc;

use crate::compat::beam_audio::controller::AudioController;
use crate::timer::Event;
use crate::not_ported;

/// `camera_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn camera_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let _ = (audio_controller, stop_event);
    not_ported!("camera_listener");
}

/// `send_camera_command(command)` (port CAMERA_CMD_PORT).
pub fn send_camera_command(command: &str) -> bool {
    crate::net::send_cmd(crate::net::CAMERA_CMD_PORT, command, &format!("camera command {command}"))
}

/// `_camera_feed_problem()`.
pub fn camera_feed_problem() -> Option<String> {
    not_ported!("camera_feed_problem");
    None
}

/// `send_camera_diag()` (F9 Alt+Shift+A).
pub fn send_camera_diag() {
    not_ported!("send_camera_diag");
}

/// `_format_camera_diag(heights)`.
pub fn format_camera_diag(heights: &str) -> String {
    let _ = heights;
    not_ported!("format_camera_diag");
    String::new()
}

/// `toggle_free_cam_info(audio_controller)` (F9 Alt+F).
pub fn toggle_free_cam_info(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    not_ported!("toggle_free_cam_info");
}
