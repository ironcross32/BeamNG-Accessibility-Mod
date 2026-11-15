//! Owner: E2. Obstacle detector: packet parser, listener, state push and toggle (`beamtel.py:1122-1255, 6966-7035`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E2.

use std::sync::Arc;

use beam_audio::controller::AudioController;
use crate::state::State;
use crate::timer::Event;
use beam_base::pyerr::ValueError;

/// `parse_obstacle_packet(text)`'s result.
#[derive(Debug, Clone, PartialEq)]
pub enum ObstaclePacket {
    Clear,
    /// The selected static hazard dict (bearing, urgency, gap, state, closing, ttc,
    /// stopping_margin, legacy).
    Static(serde_json::Value),
    /// (type, bearing, urgency, distance)
    Terrain(i64, f64, i64, f64),
}

/// `parse_obstacle_packet(text)`. `Err` is the Python's ValueError.
pub fn parse_obstacle_packet(text: &str) -> Result<ObstaclePacket, ValueError> {
    let _ = text;
    not_ported!("parse_obstacle_packet");
    Err(ValueError::new("not yet ported"))
}

/// `obstacle_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn obstacle_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let _ = (audio_controller, stop_event);
    not_ported!("obstacle_listener");
}

/// `_send_obstacle_command(command)` (port OBSTACLE_CMD_PORT).
pub fn send_obstacle_command(command: &str) -> bool {
    crate::net::send_cmd(
        crate::net::OBSTACLE_CMD_PORT,
        command,
        "obstacle detector command",
    )
}

/// `_push_obstacle_state(force=False, gear=None)`. Called with `state_lock` held by the telemetry loop.
pub fn push_obstacle_state(st: &mut State, force: bool, gear: Option<&str>) -> bool {
    let _ = (st, force, gear);
    not_ported!("push_obstacle_state");
    false
}

/// `_send_obstacle_configuration()`.
pub fn send_obstacle_configuration() -> bool {
    not_ported!("send_obstacle_configuration");
    false
}

/// `toggle_obstacle_mode(audio_controller)`.
pub fn toggle_obstacle_mode(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    not_ported!("toggle_obstacle_mode");
}
