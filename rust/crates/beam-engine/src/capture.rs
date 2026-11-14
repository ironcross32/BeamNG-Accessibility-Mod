//! Owner: E7. Screen capture shared by the AI Describer and the MCP screenshot tool
//! (`beamtel.py:9436-9601`): `_send_ui_command`, `_find_game_window`, `_raise_game_window`,
//! `_window_region`, `capture_scene_png`. Holds `state::CAPTURE_LOCK` across the sequence.
//!
//! STUB: signatures fixed by EA; bodies to be ported by E7.

use serde_json::{Value, json};

use crate::net::{UI_TOGGLE_CMD_PORT, send_cmd};
use crate::not_ported;

pub const UI_HIDE_SETTLE_S: f64 = 0.2;
pub const GAME_FOCUS_SETTLE_S: f64 = 0.6;

/// `_send_ui_command(cmd)`: HIDE / SHOW / TOGGLE to uiToggle.lua (port 4464).
pub fn send_ui_command(cmd: &str) {
    send_cmd(UI_TOGGLE_CMD_PORT, cmd, "UI toggle command");
}

/// `capture_scene_png(hide_ui=True, settle_s=UI_HIDE_SETTLE_S, focus_game=False)`:
/// `(png_bytes, meta)`. `Err` carries the message of the exception Python would raise.
pub fn capture_scene_png(
    hide_ui: bool,
    settle_s: f64,
    focus_game: bool,
) -> Result<(Vec<u8>, Value), String> {
    let _ = (hide_ui, settle_s, focus_game);
    not_ported!("capture::capture_scene_png");
    Err("capture not yet ported".to_string())
}

/// The `meta` dict's initial value.
pub fn empty_meta() -> Value {
    json!({"focused_game": false, "game_window_found": false, "region": null})
}
