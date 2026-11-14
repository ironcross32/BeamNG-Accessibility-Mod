//! Owner: E4. The browsers built on listener data (`beamtel.py:7545-7570, 7603-8256`):
//! vehicle bindings, vehicle information and page text (with their bounded waits), the
//! environment browser (edits, timeout, refresh), the forced-induction and ambient
//! temperature readouts, the audio-debug engine and sound-library browsers, and the
//! clickspot browser.
//!
//! STUB: signatures fixed by EA; bodies to be ported by E4.

use crate::net::{AUDIO_DEBUG_CMD_PORT, ENV_CMD_PORT, send_cmd};

/// A `(code, sentence)` failure, as `request_vehicle_info` / `request_page_text` return it.
pub type Failure = (String, String);

/// `_send_env_cmd(command)` (port 4475).
pub fn send_env_cmd(command: &str) {
    send_cmd(ENV_CMD_PORT, command, "environment command");
}

/// `_send_aud_cmd(command)` (port 4484).
pub fn send_aud_cmd(command: &str) {
    send_cmd(AUDIO_DEBUG_CMD_PORT, command, "audio debug command");
}

/// `open_vehicle_bindings_browser()` (F9 B).
pub fn open_vehicle_bindings_browser() {
    not_ported!("browsers::open_vehicle_bindings_browser");
}

/// `request_vehicle_info(model=None, config=None, timeout=0.6)`: `(lines, failure)`.
/// Also the vehicle spawner's `request_info_fn`.
pub fn request_vehicle_info(
    model: Option<&str>,
    config: Option<&str>,
    timeout: f64,
) -> (Vec<String>, Option<Failure>) {
    let _ = (model, config, timeout);
    not_ported!("browsers::request_vehicle_info");
    (Vec::new(), Some(("timeout".to_string(), String::new())))
}

/// `_on_screen_context(context, title)`: the UI bridge's screen-context callback.
pub fn on_screen_context(context: &str, title: &str) {
    let _ = (context, title);
    not_ported!("browsers::on_screen_context");
}

/// `_on_page_text(lines, title, code, sentence)`: the UI bridge's page-text callback.
pub fn on_page_text(lines: Vec<String>, title: &str, code: &str, sentence: &str) {
    let _ = (lines, title, code, sentence);
    not_ported!("browsers::on_page_text");
}

/// `request_page_text(timeout=0.6)`: `(lines, failure)`.
pub fn request_page_text(timeout: f64) -> (Vec<String>, Option<Failure>) {
    let _ = timeout;
    not_ported!("browsers::request_page_text");
    (Vec::new(), Some(("timeout".to_string(), String::new())))
}

/// `_read_mod_details_page()` (F9 Space on a mod details page): true when handled.
pub fn read_mod_details_page() -> bool {
    not_ported!("browsers::read_mod_details_page");
    false
}

/// `open_page_text_browser()` (controller Functions menu).
pub fn open_page_text_browser() -> bool {
    not_ported!("browsers::open_page_text_browser");
    false
}

/// `speak_induction_report()` (F9 Alt+T).
pub fn speak_induction_report() {
    not_ported!("browsers::speak_induction_report");
}

/// `speak_ambient_temperature()` (F9 Ctrl+T).
pub fn speak_ambient_temperature() {
    not_ported!("browsers::speak_ambient_temperature");
}

/// `_env_notify_refresh()`: called by the environment listener on ENV_END.
pub fn env_notify_refresh() {
    not_ported!("browsers::env_notify_refresh");
}

/// `open_environment_browser()` (F9 N).
pub fn open_environment_browser() {
    not_ported!("browsers::open_environment_browser");
}

/// `_aud_on_rows()`: dispatched by the audio debug listener on AUD_END.
pub fn aud_on_rows() {
    not_ported!("browsers::aud_on_rows");
}

/// `_aud_on_sfx_rows()`: dispatched on AUD_SFX_END.
pub fn aud_on_sfx_rows() {
    not_ported!("browsers::aud_on_sfx_rows");
}

/// `_aud_on_message(text, is_error)`: dispatched on AUD_ERROR / AUD_DONE.
pub fn aud_on_message(text: &str, is_error: bool) {
    let _ = (text, is_error);
    not_ported!("browsers::aud_on_message");
}

/// `open_audio_debug_browser()` (F9 J).
pub fn open_audio_debug_browser() {
    not_ported!("browsers::open_audio_debug_browser");
}

/// `open_clickspot_browser()` (F9 Ctrl+Shift+Alt+C).
pub fn open_clickspot_browser() {
    not_ported!("browsers::open_clickspot_browser");
}
