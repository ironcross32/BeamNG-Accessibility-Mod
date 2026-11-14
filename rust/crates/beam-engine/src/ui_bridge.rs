//! Owner: E9. `nvda_ws_speaker.py`: the WebSocket server on 8765, the GE-Lua TCP relay on
//! 8766, `broadcast`, the `register_*_callback` hooks and `bnvda_debug_enabled`.
//!
//! STUB: signatures fixed by EA; bodies to be ported by E9. The callback slots and
//! `bnvda_debug_enabled` are real (they are plain globals in the Python).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::RwLock;
use serde_json::Value;

use crate::not_ported;

/// The speak function the bridge calls (`speak_func(text, interrupt=True)`).
pub type SpeakFunc = Arc<dyn Fn(&str, bool) + Send + Sync>;
/// `stop()` returned by `start_server_in_thread`.
pub type StopFn = Box<dyn FnOnce() + Send>;

pub type DomDumpCallback = Arc<dyn Fn(Vec<String>) + Send + Sync>;
/// `(active, focus_text)`.
pub type LoadingStateCallback = Arc<dyn Fn(bool, &str) + Send + Sync>;
pub type SettingsRequestCallback = Arc<dyn Fn() + Send + Sync>;
/// `(context, title)`.
pub type ScreenContextCallback = Arc<dyn Fn(&str, &str) + Send + Sync>;
/// `(lines, title, code, sentence)`.
pub type PageTextCallback = Arc<dyn Fn(Vec<String>, &str, &str, &str) + Send + Sync>;
/// An allowlisted controller action name.
pub type AccessibilityActionCallback = Arc<dyn Fn(&str) + Send + Sync>;
/// A JSON event payload.
pub type JsonCallback = Arc<dyn Fn(&Value) + Send + Sync>;

/// The `_*_CALLBACK` globals.
#[derive(Default, Clone)]
pub struct Callbacks {
    pub dom_dump: Option<DomDumpCallback>,
    pub loading_state: Option<LoadingStateCallback>,
    pub settings_request: Option<SettingsRequestCallback>,
    pub screen_context: Option<ScreenContextCallback>,
    pub page_text: Option<PageTextCallback>,
    pub accessibility_action: Option<AccessibilityActionCallback>,
    pub challenge_event: Option<JsonCallback>,
    pub drift_event: Option<JsonCallback>,
}

pub static CALLBACKS: RwLock<Callbacks> = RwLock::new(Callbacks {
    dom_dump: None,
    loading_state: None,
    settings_request: None,
    screen_context: None,
    page_text: None,
    accessibility_action: None,
    challenge_event: None,
    drift_event: None,
});

/// `_BNVDA_DEBUG` (window.BNVDA_DEBUG mirrored from the UI runtime).
pub static BNVDA_DEBUG: AtomicBool = AtomicBool::new(false);

pub fn register_dom_dump_callback(cb: DomDumpCallback) {
    CALLBACKS.write().dom_dump = Some(cb);
}
pub fn register_loading_state_callback(cb: LoadingStateCallback) {
    CALLBACKS.write().loading_state = Some(cb);
}
pub fn register_settings_request_callback(cb: SettingsRequestCallback) {
    CALLBACKS.write().settings_request = Some(cb);
}
pub fn register_screen_context_callback(cb: ScreenContextCallback) {
    CALLBACKS.write().screen_context = Some(cb);
}
pub fn register_page_text_callback(cb: PageTextCallback) {
    CALLBACKS.write().page_text = Some(cb);
}
pub fn register_accessibility_action_callback(cb: AccessibilityActionCallback) {
    CALLBACKS.write().accessibility_action = Some(cb);
}
pub fn register_challenge_event_callback(cb: JsonCallback) {
    CALLBACKS.write().challenge_event = Some(cb);
}
pub fn register_drift_event_callback(cb: JsonCallback) {
    CALLBACKS.write().drift_event = Some(cb);
}

/// `bnvda_debug_enabled()`.
pub fn bnvda_debug_enabled() -> bool {
    BNVDA_DEBUG.load(Ordering::SeqCst)
}

/// `broadcast(msg_dict)`: send JSON to every WebSocket and TCP client (thread-safe;
/// a no-op with no clients).
pub fn broadcast(msg: &Value) {
    let _ = msg;
    not_ported!("ui_bridge::broadcast");
}

/// `start_server_in_thread(speak_func)`: returns the stop function.
pub fn start_server_in_thread(speak_func: SpeakFunc) -> Result<StopFn, String> {
    let _ = speak_func;
    not_ported!("ui_bridge::start_server_in_thread");
    Ok(Box::new(|| {}))
}
