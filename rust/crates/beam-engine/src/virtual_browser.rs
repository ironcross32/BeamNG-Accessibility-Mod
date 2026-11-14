//! Owner: E5. The generic virtual browser and buffer mode (`beamtel.py:6262-6459`).
//!
//! Callbacks (`OnEnter`, `OnAdjust`, `OnEscape` in `state`) are always invoked with no
//! state guard held. Entry data is `serde_json::Value` (Python passes ints, dicts and None).
//!
//! STUB: signatures fixed by EA; bodies to be ported by E5.

use serde_json::Value;

use crate::keyboard::KeyEvent;
use crate::not_ported;
use crate::state::{OnAdjust, OnEnter, OnEscape};

/// The keyword arguments of `open_virtual_browser`.
#[derive(Clone)]
pub struct BrowserOpts {
    pub title: Option<String>,
    pub on_enter: Option<OnEnter>,
    pub entry_data: Vec<Value>,
    pub start_index: i64,
    pub on_adjust: Option<OnAdjust>,
    pub on_escape: Option<OnEscape>,
    pub announce_interrupt: bool,
}

impl Default for BrowserOpts {
    fn default() -> Self {
        BrowserOpts {
            title: None,
            on_enter: None,
            entry_data: Vec::new(),
            start_index: 0,
            on_adjust: None,
            on_escape: None,
            announce_interrupt: true,
        }
    }
}

impl BrowserOpts {
    /// `title=...` only.
    pub fn titled(title: impl Into<String>) -> Self {
        BrowserOpts {
            title: Some(title.into()),
            ..Default::default()
        }
    }
}

/// `open_virtual_browser(lines, title=None, on_enter=None, entry_data=None, start_index=0,
/// on_adjust=None, on_escape=None, announce_interrupt=True)`.
pub fn open_virtual_browser(lines: Vec<String>, opts: BrowserOpts) {
    let _ = (lines, opts);
    not_ported!("virtual_browser::open_virtual_browser");
}

/// `close_virtual_browser(speak_exit=True)`.
pub fn close_virtual_browser(speak_exit: bool) {
    let _ = speak_exit;
    not_ported!("virtual_browser::close_virtual_browser");
}

/// `_drive_virtual_browser(action)`: a controller action while a browser is open.
pub fn drive_virtual_browser(action: &str) {
    let _ = action;
    not_ported!("virtual_browser::drive_virtual_browser");
}

/// `on_buffer_nav_press(event)`.
pub fn on_buffer_nav_press(event: &KeyEvent) {
    let _ = event;
    not_ported!("virtual_browser::on_buffer_nav_press");
}

/// `toggle_buffer_mode()`.
pub fn toggle_buffer_mode() {
    not_ported!("virtual_browser::toggle_buffer_mode");
}
