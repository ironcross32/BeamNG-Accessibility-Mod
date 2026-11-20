//! Owner: E4. The accessible developer console's engine half (`beamtel.py:6806-6898`):
//! command history persistence, the MCP console tap, `send_console_command` and the
//! console listener. Records go to the window through `UiSink::console_message`
//! (`wx.CallAfter(console_frame.on_console_message, text)`); U2 parses them.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beam_base::pyval::{PyValue, json_loads};
use parking_lot::RwLock;

use crate::listeners::scanner::e2_send;
use crate::net::{CONSOLE_CMD_PORT, CONSOLE_RESP_PORT, Decode, Recv, bind_with_rcvbuf, decode};
use crate::timer::Event;

/// `CONSOLE_HISTORY_MAX`: cap on persisted accessible-console command history.
pub const CONSOLE_HISTORY_MAX: usize = 50;

/// A tap that sees every console record and returns true to consume it.
pub type ConsoleTap = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// `_mcp_console_tap`: set by the MCP server when it starts; `None` otherwise.
pub static MCP_CONSOLE_TAP: RwLock<Option<ConsoleTap>> = RwLock::new(None);

/// `CONSOLE_HISTORY_PATH`.
pub fn console_history_path() -> PathBuf {
    beam_base::paths::config_dir().join("console_history.json")
}

/// `_load_console_history()`: most recent last.
pub fn load_console_history() -> Vec<String> {
    load_console_history_from(&console_history_path())
}

/// [`load_console_history`] from an explicit file.
#[doc(hidden)]
pub fn load_console_history_from(path: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Vec::new();
    };
    match json_loads(&text) {
        Ok(PyValue::List(items)) => {
            let all: Vec<String> = items.iter().map(PyValue::py_str).collect();
            let start = all.len().saturating_sub(CONSOLE_HISTORY_MAX);
            all[start..].to_vec()
        }
        _ => Vec::new(),
    }
}

/// `json.dumps(s)` with the defaults (`ensure_ascii=True`).
fn json_ascii_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The bytes `json.dump(history[-CONSOLE_HISTORY_MAX:], f)` writes.
#[doc(hidden)]
pub fn console_history_json(history: &[String]) -> String {
    let start = history.len().saturating_sub(CONSOLE_HISTORY_MAX);
    let mut out = String::from("[");
    for (i, h) in history[start..].iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        json_ascii_string(h, &mut out);
    }
    out.push(']');
    out
}

/// `_save_console_history(history)`.
pub fn save_console_history(history: &[String]) {
    save_console_history_to(&console_history_path(), history);
}

/// [`save_console_history`] to an explicit file.
#[doc(hidden)]
pub fn save_console_history_to(path: &Path, history: &[String]) {
    if let Err(e) = std::fs::write(path, console_history_json(history)) {
        log::error!("Failed to save console history: {e}");
    }
}

/// `register_console_tap(fn)`; `None` removes it.
pub fn register_console_tap(tap: Option<ConsoleTap>) {
    *MCP_CONSOLE_TAP.write() = tap;
}

/// `send_console_command(msg)` (port 4465): one command datagram to consoleAccessible.lua.
pub fn send_console_command(msg: &str) {
    e2_send(CONSOLE_CMD_PORT, msg, Some("console command"));
}

/// One datagram: through the MCP tap, then to the window.
///
/// The MCP server correlates its EXECs by ordering plus the EXECEND sentinel, so it must
/// also SWALLOW the records it owns: the window speaks any single-line result aloud, and
/// an agent exec talking over the user mid-test is what this prevents. LOG records are
/// copied there but never consumed.
#[doc(hidden)]
pub fn handle_console_datagram(data: &[u8]) {
    let text = decode(data, Decode::Utf8Replace).unwrap_or_default();
    let tap = MCP_CONSOLE_TAP.read().clone();
    if let Some(tap) = tap {
        // A tap fault must never kill this listener.
        if let Ok(true) = catch_unwind(AssertUnwindSafe(|| tap(&text))) {
            return;
        }
    }
    crate::ui_sink::ui_sink().console_message(&text);
}

/// `console_listener(stop_event)` (port 4466): responses, context list and log stream
/// from consoleAccessible.lua, one record per datagram.
pub fn console_listener(stop_event: &Event) {
    // A single large dump is chunked into many datagrams arriving in a burst; enlarge the
    // receive buffer so the kernel doesn't drop them before this thread drains them.
    let mut sock = match bind_with_rcvbuf(CONSOLE_RESP_PORT, 0.2, 8192, 1 << 20) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Console listener could not bind port {CONSOLE_RESP_PORT}: {e}");
            return;
        }
    };
    log::info!("Console listener started on port {CONSOLE_RESP_PORT}");
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            // Any other OSError ends the listener for the session, as in the Python.
            Recv::Error(_) => break,
            Recv::Data(data, _addr) => handle_console_datagram(data),
        }
    }
}
