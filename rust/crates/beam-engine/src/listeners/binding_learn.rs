//! Owner: E4. Learn Bindings Mode: sender, toggle, keepalive, phrase, listener (`beamtel.py:7370-7542`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E4.

use crate::timer::Event;
use crate::not_ported;

pub const BINDING_LEARN_KEEPALIVE_S: f64 = 1.0;

/// `_send_binding_learn_cmd(command)` (port BINDING_LEARN_CMD_PORT).
pub fn send_binding_learn_cmd(command: &str) {
    crate::net::send_cmd(crate::net::BINDING_LEARN_CMD_PORT, command, "binding learn command");
}

/// `toggle_binding_learn_mode()` (F9 Shift+B).
pub fn toggle_binding_learn_mode() {
    not_ported!("toggle_binding_learn_mode");
}

/// `binding_learn_keepalive_thread_fn(stop_event)`. Runs until `stop_event` is set.
pub fn binding_learn_keepalive_thread_fn(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("binding_learn_keepalive_thread_fn");
}

/// `_binding_learn_phrase(row)`.
pub fn binding_learn_phrase(row: &serde_json::Value) -> String {
    let _ = row;
    not_ported!("binding_learn_phrase");
    String::new()
}

/// `binding_learn_listener(stop_event)`. Runs until `stop_event` is set.
pub fn binding_learn_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("binding_learn_listener");
}
