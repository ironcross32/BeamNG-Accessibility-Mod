//! Owner: E9. `mcp_server.py`: the MCP automation server over loopback HTTP.
//!
//! `Deps` carries exactly what `_run_engine` injects (beamtel.py:12541-12560), as typed
//! callables. STUB: signatures fixed by EA; bodies to be ported by E9.

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::Value;

use crate::f9_commands::HelpKey;
use crate::not_ported;
use crate::speech_out::SpeechLog;

pub const HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 4481;

/// `say_fn(text, interrupt=True)`.
pub type SayFn = Arc<dyn Fn(&str, bool) + Send + Sync>;
/// `get_speech_log_fn(since_seq, last_n, source, contains, spoken_only)`.
pub type SpeechLogFn =
    Arc<dyn Fn(Option<i64>, Option<i64>, Option<&str>, Option<&str>, bool) -> SpeechLog + Send + Sync>;
/// `snapshot_state_fn(sections)`.
pub type SnapshotFn = Arc<dyn Fn(Option<&[&str]>) -> Value + Send + Sync>;
/// `load_config_fn()`.
pub type LoadConfigFn = Arc<dyn Fn() -> serde_json::Map<String, Value> + Send + Sync>;
/// `send_console_command_fn(msg)`.
pub type SendConsoleFn = Arc<dyn Fn(&str) + Send + Sync>;
/// `send_udp_fn(port, message)`; `Err` is the ToolError text.
pub type SendUdpFn = Arc<dyn Fn(u16, &str) -> Result<(), String> + Send + Sync>;
/// `press_command_fn(name, ctrl, shift, alt)`.
pub type PressFn = Arc<dyn Fn(&str, bool, bool, bool) -> Result<bool, String> + Send + Sync>;
/// `world_active_fn()`.
pub type WorldActiveFn = Arc<dyn Fn() -> bool + Send + Sync>;
/// `capture_png_fn(hide_ui, settle_s, focus_game)`.
pub type CaptureFn = Arc<dyn Fn(bool, f64, bool) -> Result<(Vec<u8>, Value), String> + Send + Sync>;
/// `road_diagnostic_fn(action, label, session, note, limit)`.
pub type RoadDiagFn = Arc<
    dyn Fn(&str, Option<&str>, Option<&str>, Option<&str>, i64) -> Result<Value, String>
        + Send
        + Sync,
>;
/// `assistant_diagnostic_fn(action, **kwargs)`.
pub type AssistantDiagFn = Arc<dyn Fn(&str, &Value) -> Result<Value, String> + Send + Sync>;

/// `mcp_server.Deps(...)`. `stop_event` is the process `timer::STOP`; `logger` is the log facade.
#[derive(Clone)]
pub struct Deps {
    pub say_fn: SayFn,
    pub get_speech_log_fn: SpeechLogFn,
    pub snapshot_state_fn: SnapshotFn,
    pub load_config_fn: LoadConfigFn,
    pub send_console_command_fn: SendConsoleFn,
    pub send_udp_fn: SendUdpFn,
    pub press_command_fn: PressFn,
    pub f9_help: IndexMap<HelpKey, &'static str>,
    pub world_active_fn: WorldActiveFn,
    pub capture_png_fn: CaptureFn,
    pub road_diagnostic_fn: RoadDiagFn,
    pub assistant_diagnostic_fn: AssistantDiagFn,
    pub version: String,
}

/// `init(deps)`.
pub fn init(deps: Deps) {
    let _ = deps;
    not_ported!("mcp_server::init");
}

/// `start(stop_event, port)`: returns the stop function.
pub fn start(port: u16) -> Result<Box<dyn FnOnce() + Send>, String> {
    let _ = port;
    not_ported!("mcp_server::start");
    Ok(Box::new(|| {}))
}

/// `console_tap(record)`: true when the record belongs to an in-flight EXEC.
pub fn console_tap(record: &str) -> bool {
    let _ = record;
    false
}

/// `make_udp_sender(logger)`: one throwaway datagram per call.
pub fn make_udp_sender() -> SendUdpFn {
    Arc::new(|port, message| {
        crate::net::send_udp(port, message.as_bytes())
            .map_err(|e| format!("failed to send UDP to port {port}: {e}"))
    })
}
