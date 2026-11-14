//! Owner: E10. `updater.py`: check, download, validate, stage, the helper `.cmd`, the
//! pending flag, `NullGate`, and the startup/manual flow. Prompts and progress go through
//! `ui_sink::UiSink`.
//!
//! STUB: signatures fixed by EA; bodies to be ported by E10.

use crate::not_ported;
use crate::ui_sink::UiSink;

/// `APP_VERSION` (must equal the release tag; read by the window title too).
pub const APP_VERSION: &str = "0.2.1";

/// The updater's view of the deferred game launch (`LaunchGate` / `NullGate`).
pub trait Gate: Send + Sync {
    fn allow(&self);
    fn deny(&self);
}

/// `NullGate`: decides nothing (the manual "Check for Updates" button).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullGate;

impl Gate for NullGate {
    fn allow(&self) {}
    fn deny(&self) {}
}

/// `run_startup_flow(frame, gate, manual=False)`. Exactly one of `allow`/`deny` fires on
/// every path (allow in a finally).
pub fn run_startup_flow(sink: &dyn UiSink, gate: &dyn Gate, manual: bool) {
    let _ = (sink, manual);
    not_ported!("updater::run_startup_flow");
    gate.allow();
}
