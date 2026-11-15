//! Owner: E5. The low-level keyboard hook, the shared F9/F10 layer hook, the
//! command-dispatch worker, suppressed-key hook records, arrow ownership and
//! `install_hotkeys` (`beamtel.py:5046-5259, 5393-5408, 5978-6034, 9355-9363, 10481-10520`).
//!
//! The hook callback must only classify and enqueue (the reason is at `beamtel.py:5066`):
//! it takes no lock, which is why `_kb_layer` and `_live_mods` are atomics here rather than
//! `State` fields. `_capture_mods`, `_input_help_mode` and `next_key_timer` are touched only
//! on the worker and live in `state::State::kb`.
//!
//! STUB: signatures fixed by EA; bodies to be ported by E5. E5 may add submodules under
//! `src/keyboard/`.

use std::sync::Arc;

use beam_audio::controller::AudioController;
use crate::state::HookRecord;

/// `command_timeout_sec`.
pub const COMMAND_TIMEOUT_SEC: f64 = 4.0;

/// Key down or up (`keyboard.KEY_DOWN` / `KEY_UP`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventType {
    Down,
    Up,
}

/// The fields handlers read from a `keyboard.KeyboardEvent` (`_SynthKeyEvent`).
/// `name` is the `keyboard` library's key name ("f9", "up", "space", "a", "?", ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub name: String,
    pub event_type: KeyEventType,
}

impl KeyEvent {
    /// `_SynthKeyEvent(name)` / `SimpleNamespace(event_type="down", name=name)`.
    pub fn down(name: &str) -> KeyEvent {
        KeyEvent {
            name: name.to_string(),
            event_type: KeyEventType::Down,
        }
    }
}

/// A handler run on the command worker for a suppressed key.
pub type KeyHandler = Arc<dyn Fn(&KeyEvent) + Send + Sync>;
/// A raw hook callback run INSIDE the low-level hook: returns true to pass the key on.
pub type RawHookCallback = Arc<dyn Fn(&KeyEvent) -> bool + Send + Sync>;

/// Which layer owns the keyboard (`_kb_layer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    F9,
    F10,
}

/// `KEYBOARD_OK`: the hook could be installed (elevated, not blocked).
pub fn keyboard_ok() -> bool {
    not_ported!("keyboard::keyboard_ok");
    false
}

/// `_command_dispatch(fn, *args)`: queue work on the serial `command-dispatch` worker.
/// Safe inside a hook or any callback.
pub fn command_dispatch(f: Box<dyn FnOnce() + Send + 'static>) {
    not_ported!("keyboard::command_dispatch (running inline)");
    f();
}

/// `_kb_enqueue(handler)`: wrap a suppressed-key handler so its body runs on the worker.
pub fn kb_enqueue(handler: KeyHandler) -> KeyHandler {
    handler
}

/// `_start_command_worker()`.
pub fn start_command_worker() {
    not_ported!("keyboard::start_command_worker");
}

/// `_hook_suppressed(key, handler)`: a suppressing key-down hook; key-ups pass.
pub fn hook_suppressed(key: &str, handler: KeyHandler) -> Option<HookRecord> {
    let _ = (key, handler);
    not_ported!("keyboard::hook_suppressed");
    None
}

/// `keyboard.hook_key(key, cb, suppress=True)` with the callback deciding per event
/// (`_install_nodegrab_hooks`). Same record shape as [`hook_suppressed`].
pub fn hook_key_raw(key: &str, cb: RawHookCallback) -> Option<HookRecord> {
    let _ = (key, cb);
    not_ported!("keyboard::hook_key_raw");
    None
}

/// `_unhook_suppressed(records)`: undo the records and clear the list.
pub fn unhook_suppressed(records: &mut Vec<HookRecord>) {
    records.clear();
}

/// `keyboard.is_pressed(name)` ("shift", "ctrl").
pub fn is_pressed(name: &str) -> bool {
    let _ = name;
    false
}

/// `_kb_layer`.
pub fn kb_layer() -> Option<Layer> {
    None
}

/// `_kb_open_layer(which)`.
pub fn kb_open_layer(which: Layer) {
    let _ = which;
    not_ported!("keyboard::kb_open_layer");
}

/// `_kb_close_layer()`.
pub fn kb_close_layer() {
    not_ported!("keyboard::kb_close_layer");
}

/// `_clear_next_key_hook(speak_exit)`.
pub fn clear_next_key_hook(speak_exit: bool) {
    let _ = speak_exit;
    not_ported!("keyboard::clear_next_key_hook");
}

/// `_start_next_key_capture(audio_controller)`.
pub fn start_next_key_capture(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    not_ported!("keyboard::start_next_key_capture");
}

/// `release_arrow_owners(reason="")`.
pub fn release_arrow_owners(reason: &str) {
    let _ = reason;
    not_ported!("keyboard::release_arrow_owners");
}

/// `install_hotkeys(audio_controller)`: F9 and F10 (also forces the speech library load).
pub fn install_hotkeys(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    not_ported!("keyboard::install_hotkeys");
}
