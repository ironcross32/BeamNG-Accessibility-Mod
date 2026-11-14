//! Owner: E4. The accessible developer console's engine half (`beamtel.py:6806-6898`):
//! command history persistence, the MCP console tap, `send_console_command` and the
//! console listener. Records go to the window through `UiSink::console_message`
//! (`wx.CallAfter(console_frame.on_console_message, text)`); U2 parses them.
//!
//! STUB: signatures fixed by EA; bodies to be ported by E4.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::net::{CONSOLE_CMD_PORT, send_cmd};
use crate::not_ported;
use crate::timer::Event;

/// `CONSOLE_HISTORY_MAX`.
pub const CONSOLE_HISTORY_MAX: usize = 50;

/// A tap that sees every console record and returns true to consume it.
pub type ConsoleTap = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// `_mcp_console_tap`.
pub static MCP_CONSOLE_TAP: RwLock<Option<ConsoleTap>> = RwLock::new(None);

/// `CONSOLE_HISTORY_PATH`.
pub fn console_history_path() -> PathBuf {
    beam_base::paths::config_dir().join("console_history.json")
}

/// `_load_console_history()`: most recent last.
pub fn load_console_history() -> Vec<String> {
    not_ported!("console::load_console_history");
    Vec::new()
}

/// `_save_console_history(history)`.
pub fn save_console_history(history: &[String]) {
    let _ = history;
    not_ported!("console::save_console_history");
}

/// `register_console_tap(fn)`; `None` removes it.
pub fn register_console_tap(tap: Option<ConsoleTap>) {
    *MCP_CONSOLE_TAP.write() = tap;
}

/// `send_console_command(msg)` (port 4465).
pub fn send_console_command(msg: &str) {
    send_cmd(CONSOLE_CMD_PORT, msg, "console command");
}

/// `console_listener(stop_event)` (port 4466).
pub fn console_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("console::console_listener");
}
