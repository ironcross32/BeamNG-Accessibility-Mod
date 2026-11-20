//! Owner: E7. Window and DPI utilities (`beamtel.py:10317-10478`): thread DPI awareness,
//! `_window_process_name`, `_find_beamng_window`, `warp_cursor_to_viewport`,
//! `_is_beamng_focused`.
//!
//! SNAP projects a world position onto BeamNG's render viewport, in physical render
//! pixels relative to the client area's top-left. SetCursorPos wants desktop coordinates,
//! so the thread is made per-monitor DPI aware while geometry is read and the cursor moved.

/// `_GAME_WINDOW_CLASS`.
pub const GAME_WINDOW_CLASS: &str = "GameEngineMainWindow";
/// `_PROCESS_QUERY_LIMITED_INFORMATION`.
pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

/// `os.path.basename(path).lower()`.
pub fn image_basename_lower(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase()
}

/// The point `warp_cursor_to_viewport` scales: `int(round(x * client_w / float(view_w)))`
/// when both sizes are positive, else `x` unchanged.
pub fn scale_viewport(x: i64, client: i64, view: i64, other_client: i64, other_view: i64) -> i64 {
    // Python scales only when all four of view_w, view_h, client_w, client_h are > 0.
    if view > 0 && other_view > 0 && client > 0 && other_client > 0 {
        let num = (x as i128 * client as i128) as f64;
        (num / view as f64).round_ties_even() as i64
    } else {
        x
    }
}

/// ctypes passes an untyped Python int as a C int masked to 32 bits.
fn c_int_mask(v: i64) -> i32 {
    v as i32
}

#[cfg(windows)]
mod imp {
    use super::{
        GAME_WINDOW_CLASS, PROCESS_QUERY_LIMITED_INFORMATION, c_int_mask, image_basename_lower,
        scale_viewport,
    };
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32, QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        SetThreadDpiAwarenessContext,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetClientRect, GetForegroundWindow, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SetCursorPos,
    };
    use windows::core::{BOOL, PWSTR};

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut core::ffi::c_void)
    }

    /// `_set_thread_dpi_aware()`: the old context, or None. Scoped to the thread so the
    /// window's own scaling is untouched.
    fn set_thread_dpi_aware() -> Option<DPI_AWARENESS_CONTEXT> {
        // SAFETY: no pointers; changes only this thread's awareness.
        let prev =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if prev.0.is_null() { None } else { Some(prev) }
    }

    /// `_restore_thread_dpi(previous)`.
    fn restore_thread_dpi(previous: Option<DPI_AWARENESS_CONTEXT>) {
        if let Some(p) = previous {
            // SAFETY: as above.
            let _ = unsafe { SetThreadDpiAwarenessContext(p) };
        }
    }

    /// `_window_process_name(hwnd)`: the owning process's lowercase image name, or "".
    pub fn window_process_name(h: isize) -> String {
        // SAFETY: `pid`, `buf` and `size` are live for each call; the process handle is
        // closed before returning.
        unsafe {
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd(h), Some(&mut pid));
            if pid == 0 {
                return String::new();
            }
            let Ok(handle) = OpenProcess(
                PROCESS_ACCESS_RIGHTS(PROCESS_QUERY_LIMITED_INFORMATION),
                false,
                pid,
            ) else {
                return String::new();
            };
            let mut buf = [0u16; 260];
            let mut size: u32 = 260;
            let ok = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut size,
            );
            let _ = CloseHandle(handle);
            if ok.is_err() {
                return String::new();
            }
            image_basename_lower(&String::from_utf16_lossy(&buf[..(size as usize).min(260)]))
        }
    }

    struct Found {
        exact: Vec<isize>,
        fallback: Vec<isize>,
    }

    unsafe extern "system" fn enum_cb(h: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `Found` owned by `find_beamng_window` for the duration of
        // the EnumWindows call; `h` is a window handle supplied by the system.
        unsafe {
            let found = &mut *(data.0 as *mut Found);
            if !IsWindowVisible(h).as_bool() {
                return BOOL(1);
            }
            let mut cls = [0u16; 256];
            let n = GetClassNameW(h, &mut cls).max(0) as usize;
            if String::from_utf16_lossy(&cls[..n.min(256)]) == GAME_WINDOW_CLASS {
                found.exact.push(h.0 as isize);
                return BOOL(0);
            }
            let length = GetWindowTextLengthW(h);
            if length <= 0 {
                return BOOL(1);
            }
            let mut buf = vec![0u16; length as usize + 1];
            let got = GetWindowTextW(h, &mut buf).max(0) as usize;
            let title = String::from_utf16_lossy(&buf[..got.min(buf.len())]);
            if title.contains("BeamNG") && window_process_name(h.0 as isize).starts_with("beamng") {
                found.fallback.push(h.0 as isize);
            }
        }
        BOOL(1)
    }

    /// `_find_beamng_window()`: the render window by class, falling back to a title match
    /// only when the owning process is the game (an Explorer window on the game folder is
    /// called "BeamNG.drive - File Explorer").
    pub fn find_beamng_window() -> Option<isize> {
        let mut found = Found {
            exact: Vec::new(),
            fallback: Vec::new(),
        };
        // EnumWindows "fails" when the callback stops it early; the Python ignores the
        // return value too.
        // SAFETY: the callback only writes into `found`, which outlives the call.
        let _ = unsafe { EnumWindows(Some(enum_cb), LPARAM(&mut found as *mut Found as isize)) };
        found.exact.first().or(found.fallback.first()).copied()
    }

    pub fn warp_cursor_to_viewport(view_x: i64, view_y: i64, view_w: i64, view_h: i64) {
        let previous = set_thread_dpi_aware();
        let (mut x, mut y) = (view_x, view_y);
        if let Some(h) = find_beamng_window() {
            let mut rect = RECT::default();
            // SAFETY: `rect` and `origin` are live for their calls.
            unsafe {
                if GetClientRect(hwnd(h), &mut rect).is_ok() {
                    let client_w = (rect.right - rect.left) as i64;
                    let client_h = (rect.bottom - rect.top) as i64;
                    // The render resolution can differ from the client area (windowed
                    // at a non-native size, or a resolution scale).
                    let (sx, sy) = (
                        scale_viewport(x, client_w, view_w, client_h, view_h),
                        scale_viewport(y, client_h, view_h, client_w, view_w),
                    );
                    x = sx;
                    y = sy;
                }
                let mut origin = POINT { x: 0, y: 0 };
                if ClientToScreen(hwnd(h), &mut origin).as_bool() {
                    x = x.saturating_add(origin.x as i64);
                    y = y.saturating_add(origin.y as i64);
                }
            }
        } else {
            log::warn!("No BeamNG window found; warping to raw viewport coordinates.");
        }
        // SAFETY: plain Win32 call.
        let _ = unsafe { SetCursorPos(c_int_mask(x), c_int_mask(y)) };
        restore_thread_dpi(previous);
    }

    pub fn is_beamng_focused() -> bool {
        // SAFETY: `buf` is live for the call.
        unsafe {
            let h = GetForegroundWindow();
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(h, &mut buf).max(0) as usize;
            String::from_utf16_lossy(&buf[..n.min(256)]).contains("BeamNG")
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn window_process_name(_h: isize) -> String {
        String::new()
    }
    pub fn find_beamng_window() -> Option<isize> {
        None
    }
    pub fn warp_cursor_to_viewport(_x: i64, _y: i64, _w: i64, _h: i64) {}
    pub fn is_beamng_focused() -> bool {
        false
    }
}

/// `_window_process_name(hwnd)`.
pub fn window_process_name(hwnd: isize) -> String {
    imp::window_process_name(hwnd)
}

/// `warp_cursor_to_viewport(view_x, view_y, view_w=0, view_h=0)`.
pub fn warp_cursor_to_viewport(view_x: i64, view_y: i64, view_w: i64, view_h: i64) {
    imp::warp_cursor_to_viewport(view_x, view_y, view_w, view_h);
}

/// `_is_beamng_focused()`: the foreground window's title contains "BeamNG".
pub fn is_beamng_focused() -> bool {
    imp::is_beamng_focused()
}

/// `_find_beamng_window()`: the render window's HWND.
pub fn find_beamng_window() -> Option<isize> {
    imp::find_beamng_window()
}
