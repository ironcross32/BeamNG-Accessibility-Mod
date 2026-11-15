//! Owner: E2. Road detector listener, road commands, diagnostics control, road status and toggle (`beamtel.py:1258-1477, 7038-7122`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E2.

use std::sync::Arc;

use beam_audio::controller::AudioController;
use crate::timer::Event;

/// `road_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn road_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let _ = (audio_controller, stop_event);
    not_ported!("road_listener");
}

/// `_send_road_command(command)` (port ROAD_CMD_PORT).
pub fn send_road_command(command: &str) -> bool {
    crate::net::send_cmd(crate::net::ROAD_CMD_PORT, command, "road detector command")
}

/// `_send_road_configuration()`: `PRIVATE,<0|1>`.
pub fn send_road_configuration() {
    not_ported!("send_road_configuration");
}

/// `assistant_diagnostic_control(action="status", **kwargs)` (MCP). `Err` is the RuntimeError text.
pub fn assistant_diagnostic_control(
    action: &str,
    kwargs: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let _ = (action, kwargs);
    not_ported!("assistant_diagnostic_control");
    Ok(serde_json::Value::Null)
}

/// `road_diagnostic_control(action, label, session, note, limit=20)` (MCP). `Err` is the RuntimeError/ValueError text.
pub fn road_diagnostic_control(
    action: &str,
    label: Option<&str>,
    session: Option<&str>,
    note: Option<&str>,
    limit: i64,
) -> Result<serde_json::Value, String> {
    let _ = (action, label, session, note, limit);
    not_ported!("road_diagnostic_control");
    Ok(serde_json::Value::Null)
}

/// `speak_road_status()`.
pub fn speak_road_status() {
    not_ported!("speak_road_status");
}

/// `toggle_road_mode(audio_controller)`.
pub fn toggle_road_mode(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    not_ported!("toggle_road_mode");
}
