//! Owner: E4. Environment listener and its row parsing (`beamtel.py:3860-3948, 7573-7591, 7772-7776`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E4.

use crate::state::EnvRow;
use crate::timer::Event;

/// `environment_listener(stop_event)`. Runs until `stop_event` is set.
pub fn environment_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("environment_listener");
}

/// `_parse_env_fields(payload)`.
pub fn parse_env_fields(payload: &str) -> EnvRow {
    let _ = payload;
    not_ported!("parse_env_fields");
    EnvRow::new()
}

/// `_env_float(row, key)`.
pub fn env_float(row: &EnvRow, key: &str) -> Option<f64> {
    let _ = (row, key);
    not_ported!("env_float");
    None
}
