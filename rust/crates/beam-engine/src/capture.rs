//! Owner: E7. Screen capture shared by the AI Describer and the MCP screenshot tool
//! (`beamtel.py:9436-9601`): `_send_ui_command`, `_find_game_window`, `_raise_game_window`,
//! `_window_region`, `capture_scene_png`. Holds `state::CAPTURE_LOCK` across the sequence.

use beam_base::pyval::py_max;
use serde_json::{Value, json};

use crate::ai_describer::Region;
use crate::net::UI_TOGGLE_CMD_PORT;
use crate::state::CAPTURE_LOCK;

/// How long to let the game render a UI-free frame after HIDE before grabbing.
pub const UI_HIDE_SETTLE_S: f64 = 0.2;
/// How long to wait after raising the game window before grabbing. Longer than the UI
/// settle: a minimized BeamNG reports itself as "- background" and throttles rendering,
/// so it needs time to come back up and draw a real frame.
pub const GAME_FOCUS_SETTLE_S: f64 = 0.6;
pub const GAME_WINDOW_HINT: &str = "BeamNG.drive";

/// `_send_ui_command(cmd)`: HIDE / SHOW / TOGGLE to uiToggle.lua (port 4464).
pub fn send_ui_command(cmd: &str) {
    crate::ai_layer::e7_send(UI_TOGGLE_CMD_PORT, cmd, "UI toggle command");
}

/// `_window_region`'s test on a window rectangle: an mss region, or None if degenerate.
/// A minimized window parks at -32000; anything tiny or off-screen is not a frame.
pub fn region_from_rect(left: i64, top: i64, right: i64, bottom: i64) -> Option<Region> {
    let (w, h) = (right - left, bottom - top);
    if w < 64 || h < 64 || left < -30000 || top < -30000 {
        return None;
    }
    Some(Region {
        left,
        top,
        width: w,
        height: h,
    })
}

#[cfg(windows)]
mod win {
    use super::{GAME_WINDOW_HINT, Region, region_from_rect};
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindowRect, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE,
        SetForegroundWindow, ShowWindow,
    };
    use windows::core::BOOL;

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut core::ffi::c_void)
    }

    unsafe extern "system" fn enum_cb(h: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the Vec owned by `find_game_window` for the duration of the
        // EnumWindows call; `h` is a window handle supplied by the system.
        unsafe {
            let found = &mut *(data.0 as *mut Vec<isize>);
            if !IsWindowVisible(h).as_bool() {
                return BOOL(1);
            }
            let n = GetWindowTextLengthW(h);
            if n == 0 {
                return BOOL(1);
            }
            let mut buf = vec![0u16; n.max(0) as usize + 1];
            let got = GetWindowTextW(h, &mut buf).max(0) as usize;
            let title = String::from_utf16_lossy(&buf[..got.min(buf.len())]);
            if title.starts_with(GAME_WINDOW_HINT) {
                found.push(h.0 as isize);
            }
        }
        BOOL(1)
    }

    /// `_find_game_window()`: matched on the title rather than the process, because the
    /// mod's own window is called "BeamNG Accessibility".
    pub fn find_game_window() -> Option<isize> {
        let mut found: Vec<isize> = Vec::new();
        // SAFETY: the callback only writes into `found`, which outlives the call.
        let _ = unsafe {
            EnumWindows(
                Some(enum_cb),
                LPARAM(&mut found as *mut Vec<isize> as isize),
            )
        };
        found.first().copied()
    }

    /// `_raise_game_window(hwnd)`: restore and foreground; returns the previous
    /// foreground window. The input queues are attached first, because beamtel runs
    /// elevated and SetForegroundWindow is refused to a process that does not own the
    /// foreground. Every step is best-effort.
    pub fn raise_game_window(h: isize) -> isize {
        // SAFETY: plain Win32 calls on window handles; none take pointers we own.
        unsafe {
            let prev = GetForegroundWindow().0 as isize;
            let w = hwnd(h);
            if IsIconic(w).as_bool() {
                let _ = ShowWindow(w, SW_RESTORE);
            }
            let cur = GetCurrentThreadId();
            let tgt = GetWindowThreadProcessId(w, None);
            let mut attached = false;
            if tgt != 0 && tgt != cur {
                attached = AttachThreadInput(cur, tgt, true).as_bool();
            }
            let _ = BringWindowToTop(w);
            let _ = SetForegroundWindow(w);
            if attached {
                let _ = AttachThreadInput(cur, tgt, false);
            }
            prev
        }
    }

    /// `_window_region(hwnd)`.
    pub fn window_region(h: isize) -> Option<Region> {
        let mut r = RECT::default();
        // SAFETY: `r` is a live RECT for the call.
        if unsafe { GetWindowRect(hwnd(h), &mut r) }.is_err() {
            return None;
        }
        region_from_rect(r.left as i64, r.top as i64, r.right as i64, r.bottom as i64)
    }

    pub fn set_foreground(h: isize) {
        // SAFETY: plain Win32 call on a window handle.
        let _ = unsafe { SetForegroundWindow(hwnd(h)) };
    }
}

#[cfg(not(windows))]
mod win {
    use super::Region;
    pub fn find_game_window() -> Option<isize> {
        None
    }
    pub fn raise_game_window(_h: isize) -> isize {
        0
    }
    pub fn window_region(_h: isize) -> Option<Region> {
        None
    }
    pub fn set_foreground(_h: isize) {}
}

pub use win::find_game_window;

/// `capture_scene_png(hide_ui=True, settle_s=UI_HIDE_SETTLE_S, focus_game=False)`:
/// `(png_bytes, meta)`. `Err` carries the message of the exception Python would raise.
///
/// The describer fires from a keypress while the game is foreground, so it does not raise
/// the window (`focus_game=False`). An agent's screenshot fires while BeamNG is typically
/// minimized and parked off-screen, so it raises it; `meta` reports what was actually
/// captured so a wrong picture can be diagnosed instead of believed. The UI is always
/// restored, and the previously focused window is always put back.
pub fn capture_scene_png(
    hide_ui: bool,
    settle_s: f64,
    focus_game: bool,
) -> Result<(Vec<u8>, Value), String> {
    let mut meta = empty_meta();
    let _guard = CAPTURE_LOCK.lock();
    let hwnd = if focus_game {
        win::find_game_window()
    } else {
        None
    };
    meta["game_window_found"] = json!(hwnd.is_some());
    let mut prev: isize = 0;
    let mut region: Option<Region> = None;
    let result = {
        if let Some(h) = hwnd {
            prev = win::raise_game_window(h);
            crate::timer::sleep(GAME_FOCUS_SETTLE_S);
            region = win::window_region(h);
            meta["region"] = region.map_or(Value::Null, |r| r.to_json());
            meta["focused_game"] = json!(region.is_some());
            if region.is_none() {
                meta["warning"] = json!(
                    "the game window would not come up (still minimized or off-screen); \
                     this is a grab of the primary monitor instead"
                );
            }
        }
        if hide_ui {
            send_ui_command("HIDE");
            crate::timer::sleep(py_max(0.0, settle_s));
        }
        crate::ai_describer::capture_region(region)
    };
    if hide_ui {
        send_ui_command("SHOW");
    }
    if prev != 0 {
        win::set_foreground(prev);
    }
    result.map(|png| (png, meta))
}

/// The `meta` dict's initial value.
pub fn empty_meta() -> Value {
    json!({"focused_game": false, "game_window_found": false, "region": null})
}
