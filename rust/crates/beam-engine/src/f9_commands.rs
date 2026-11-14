//! Owner: E6. The F9 command handler: `_F9_HELP`, `_invoke_f9_command`,
//! `_mcp_press_command`, `_on_next_key_press` (`beamtel.py:5260-5341, 8259-9352`).
//!
//! STUB: signatures fixed by EA; bodies to be ported by E6.

use std::sync::{Arc, LazyLock};

use indexmap::IndexMap;

use crate::compat::beam_audio::controller::AudioController;
use crate::keyboard::KeyEvent;
use crate::not_ported;

/// A help key: (name, ctrl, shift, alt).
pub type HelpKey = (&'static str, bool, bool, bool);

/// `_F9_HELP`, in the Python's insertion order.
pub static F9_HELP: LazyLock<IndexMap<HelpKey, &'static str>> = LazyLock::new(IndexMap::new);

/// `_invoke_f9_command(name, ctrl, shift, alt)`. `Err` is the Python's
/// `RuntimeError("audio controller is not up yet")`.
pub fn invoke_f9_command(name: &str, ctrl: bool, shift: bool, alt: bool) -> Result<bool, String> {
    let _ = (name, ctrl, shift, alt);
    not_ported!("f9_commands::invoke_f9_command");
    Ok(true)
}

/// `_mcp_press_command(...)`.
pub fn mcp_press_command(name: &str, ctrl: bool, shift: bool, alt: bool) -> Result<bool, String> {
    invoke_f9_command(name, ctrl, shift, alt)
}

/// `_on_next_key_press(event, audio_controller)`: one F9-layer key on the worker.
pub fn on_next_key_press(event: &KeyEvent, audio_controller: Option<Arc<AudioController>>) {
    let _ = (event, audio_controller);
    not_ported!("f9_commands::on_next_key_press");
}
