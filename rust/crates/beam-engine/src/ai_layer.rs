//! Owner: E7. The F10 AI layer (`beamtel.py:5344-5390, 9366-9445, 9704-10315`):
//! `_F10_HELP`, `_send_ai_command`, the driving assistant's callbacks, slots selection,
//! presets, AI status readouts, `_on_ai_key_press`, `_start_ai_capture`.
//!
//! STUB: signatures fixed by EA; bodies to be ported by E7.

use std::sync::LazyLock;

use indexmap::IndexMap;
use serde_json::Value;

use crate::f9_commands::HelpKey;
use crate::keyboard::KeyEvent;
use crate::net::{AI_CMD_PORT, SLOT_CMD_PORT_OUT, send_cmd};

/// `_F10_HELP` (including the generated digit rows), in the Python's insertion order.
pub static F10_HELP: LazyLock<IndexMap<HelpKey, String>> = LazyLock::new(IndexMap::new);

/// `_send_ai_command(cmd)` (port 4449).
pub fn send_ai_command(cmd: &str) {
    send_cmd(AI_CMD_PORT, cmd, "AI command");
}

/// `_send_slot_command(cmd)` (port 4459).
pub fn send_slot_command(cmd: &str) {
    send_cmd(SLOT_CMD_PORT_OUT, cmd, "slot command");
}

/// `_assistant_cue(kind, choices=())`, the driving assistant's cue callback.
pub fn assistant_cue(kind: &str, choices: &[String]) {
    let _ = (kind, choices);
    not_ported!("ai_layer::assistant_cue");
}

/// `_assistant_arrival(arrival)`, the driving assistant's arrival callback.
pub fn assistant_arrival(arrival: &Value) {
    let _ = arrival;
    not_ported!("ai_layer::assistant_arrival");
}

/// `_speak_status_all_response(payload)` (called by the scanner listener).
pub fn speak_status_all_response(payload: &str) {
    let _ = payload;
    not_ported!("ai_layer::speak_status_all_response");
}

/// `_on_ai_key_press(event)`: one F10-layer key on the worker.
pub fn on_ai_key_press(event: &KeyEvent) {
    let _ = event;
    not_ported!("ai_layer::on_ai_key_press");
}

/// `_start_ai_capture()`.
pub fn start_ai_capture() {
    not_ported!("ai_layer::start_ai_capture");
}

/// `_clear_ai_hook(speak_exit)`.
pub fn clear_ai_hook(speak_exit: bool) {
    let _ = speak_exit;
    not_ported!("ai_layer::clear_ai_hook");
}
