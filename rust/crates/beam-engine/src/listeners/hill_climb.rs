//! Owner: E2. Hill-climb challenge glue: report browsers and the challenge event callbacks (`beamtel.py:6462-6494, 6693-6725`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E2.

use crate::not_ported;

/// `open_hill_climb_report(summary, return_to_history=None, announce_interrupt=True)`.
pub fn open_hill_climb_report(summary: serde_json::Value, return_to_history: Option<i64>, announce_interrupt: bool) {
    let _ = (summary, return_to_history, announce_interrupt);
    not_ported!("open_hill_climb_report");
}

/// `open_hill_climb_history(start_index=0)`.
pub fn open_hill_climb_history(start_index: i64) {
    let _ = start_index;
    not_ported!("open_hill_climb_history");
}

/// `_hill_climb_finalized(summary, auto_open)`: the recorder's finalize callback.
pub fn hill_climb_finalized(summary: serde_json::Value, auto_open: bool) {
    let _ = (summary, auto_open);
    not_ported!("hill_climb_finalized");
}

/// `_on_hill_climb_event(data)`: the UI bridge's challenge-event callback.
pub fn on_hill_climb_event(data: &serde_json::Value) {
    let _ = data;
    not_ported!("on_hill_climb_event");
}
