//! Owner: E10. The mod installer core of `config_ui.py:41-335`: `install_mod_interactive`,
//! the freeroam-layout cleanup and the stamped mod-version comparison. Prompts go through
//! `ui_sink::UiSink::message_box`.
//!
//! STUB: signatures fixed by EA; bodies to be ported by E10.

use crate::not_ported;
use crate::ui_sink::UiSink;

/// `install_mod_interactive(parent)`: true when the mod was installed.
pub fn install_mod_interactive(sink: &dyn UiSink) -> bool {
    let _ = sink;
    not_ported!("mod_install::install_mod_interactive");
    false
}
