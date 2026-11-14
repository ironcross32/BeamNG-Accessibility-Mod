//! Owner: E7. Window and DPI utilities (`beamtel.py:10317-10478`): thread DPI awareness,
//! `_window_process_name`, `_find_beamng_window`, `warp_cursor_to_viewport`,
//! `_is_beamng_focused`.
//!
//! STUB: signatures fixed by EA; bodies to be ported by E7.

use crate::not_ported;

/// `warp_cursor_to_viewport(view_x, view_y, view_w=0, view_h=0)`.
pub fn warp_cursor_to_viewport(view_x: i64, view_y: i64, view_w: i64, view_h: i64) {
    let _ = (view_x, view_y, view_w, view_h);
    not_ported!("win_util::warp_cursor_to_viewport");
}

/// `_is_beamng_focused()`: the foreground window's title contains "BeamNG".
pub fn is_beamng_focused() -> bool {
    not_ported!("win_util::is_beamng_focused");
    false
}

/// `_find_beamng_window()`: the render window's HWND.
pub fn find_beamng_window() -> Option<isize> {
    not_ported!("win_util::find_beamng_window");
    None
}
