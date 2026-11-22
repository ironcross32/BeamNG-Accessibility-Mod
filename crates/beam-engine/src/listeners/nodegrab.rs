//! Owner: E4. Accessible node grabber listener, Ctrl+Space hook, toggle (`beamtel.py:3419-3571, 7239-7311`).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use beam_audio::controller::AudioController;
use beam_base::pyfmt::{py_float_parse, py_strip};
use beam_base::pyval::py_int_parse;

use crate::keyboard::{self, KeyEvent, KeyEventType, RawHookCallback};
use crate::listeners::scanner::{e2_send, py_addr};
use crate::net::{Decode, NODEGRAB_CMD_PORT, NODEGRAB_LISTEN_PORT, Recv, UdpListener, decode};
use crate::speech_out::say;
use crate::state::{NODEGRAB_MODE_ACTIVE, NODEGRAB_SPACE_HELD, state};
use crate::timer::{Event, time_time};

const DEBOUNCE_SEC: f64 = 0.15;

fn say_excluded(text: &str) {
    say(text, true, true, "python");
}

/// `int(text)`; `Err` where Python raises (uncaught in this listener). Ints beyond i64
/// are treated as malformed.
fn int_field(text: &str) -> Result<i64, String> {
    py_int_parse(text)
        .and_then(|v| v.to_i64())
        .ok_or_else(|| format!("ValueError: invalid literal for int() with base 10: {text:?}"))
}

fn float_field(text: &str) -> Result<f64, String> {
    py_float_parse(text)
        .ok_or_else(|| format!("ValueError: could not convert string to float: {text:?}"))
}

/// `_send_nodegrab_cmd(command)` (port NODEGRAB_CMD_PORT).
pub fn send_nodegrab_cmd(command: &str) {
    e2_send(NODEGRAB_CMD_PORT, command, Some("node grabber command"));
}

/// Where the cursor is warped (`warp_cursor_to_viewport`). Replaced in tests.
#[doc(hidden)]
pub type WarpFn = dyn Fn(i64, i64, i64, i64) + Send + Sync;

static WARP_HOOK: parking_lot::RwLock<Option<Box<WarpFn>>> = parking_lot::RwLock::new(None);

/// Route cursor warps to `hook` instead of the desktop (tests only).
#[doc(hidden)]
pub fn set_warp_hook_for_tests(hook: Option<Box<WarpFn>>) {
    *WARP_HOOK.write() = hook;
}

pub(crate) fn warp(x: i64, y: i64, w: i64, h: i64) {
    if let Some(hook) = WARP_HOOK.read().as_ref() {
        hook(x, y, w, h);
        return;
    }
    crate::win_util::warp_cursor_to_viewport(x, y, w, h);
}

/// One decoded, stripped, non-empty datagram. `last_announce_ts` is the listener's speech
/// debounce. `Err` is an exception the Python does not catch: the listener ends.
#[doc(hidden)]
pub fn handle_nodegrab_text(
    ac: &AudioController,
    text: &str,
    now: f64,
    last_announce_ts: &mut f64,
) -> Result<(), String> {
    // The game's controller node grabber (0.39+) toggles with no message of its own and
    // also drops out by itself, so this is spoken whether or not the accessible grabber
    // is on.
    if text.starts_with("PADMODE:") || text.starts_with("PADSYNC:") {
        let pad_on = text.ends_with(":1");
        if text.starts_with("PADMODE:") {
            say_excluded(&format!(
                "Controller node grabber {}",
                if pad_on { "on" } else { "off" }
            ));
        }
        let mut st = state();
        st.nodegrab.nodegrab_pad_active = pad_on;
        if !pad_on && !st.nodegrab.nodegrab_scanning {
            st.nodegrab.nodegrab_last_cid = -1;
        }
        return Ok(());
    }
    if text.starts_with("GRAB:") {
        say_excluded(if text.ends_with(":1") {
            "Grabbed"
        } else {
            "Released"
        });
        return Ok(());
    }
    if text == "PINNED" {
        say_excluded("Node pinned");
        return Ok(());
    }
    if text.starts_with("UNPINNED:") {
        say_excluded("All nodes unpinned");
        return Ok(());
    }
    if text == "UNPIN_FAILED" {
        say_excluded("Unpin unavailable");
        return Ok(());
    }
    if text == "NODE_CLEAR" {
        let was = state().nodegrab.nodegrab_last_cid;
        if was != -1 {
            ac.trigger_node_hover_beep(0.5, true);
            state().nodegrab.nodegrab_last_cid = -1;
        }
        return Ok(());
    }

    // Ignore node hover data if Python knows CTRL isn't held (stale Lua scan). Controller
    // mode scans the screen centre with no key held at all.
    {
        let st = state();
        if !st.nodegrab.nodegrab_scanning
            && !st.nodegrab.nodegrab_pad_active
            && text.starts_with("NODE:")
        {
            return Ok(());
        }
    }

    if let Some(rest) = text.strip_prefix("SNAP:") {
        // SNAP:<x>,<y>,<viewW>,<viewH>,<cid>,<name>,<location>,<groups>,<heightNorm>
        let parts: Vec<&str> = rest.splitn(9, ',').collect();
        if parts.len() >= 4 {
            let (sx, sy) = (int_field(parts[0])?, int_field(parts[1])?);
            let (vw, vh) = (int_field(parts[2])?, int_field(parts[3])?);
            warp(sx, sy, vw, vh);
            // Also announce the snapped node if we have info
            if parts.len() >= 9 {
                let _cid = int_field(parts[4])?;
                let name = parts[5];
                let location = parts[6];
                let groups = parts[7];
                let height_norm = float_field(parts[8])?;
                ac.trigger_node_hover_beep(height_norm, false);
                let mut desc = format!("{name}, {location}");
                if !groups.is_empty() {
                    desc += &format!(", {groups}");
                }
                say_excluded(&desc);
            }
        }
        return Ok(());
    }

    if let Some(rest) = text.strip_prefix("NODE:") {
        // NODE:<cid>,<name>,<location>,<groups>,<heightNorm>
        let parts: Vec<&str> = rest.splitn(5, ',').collect();
        if parts.len() == 5 {
            let cid = int_field(parts[0])?;
            let name = parts[1];
            let location = parts[2];
            let groups = parts[3];
            let height_norm = float_field(parts[4])?;

            state().nodegrab.nodegrab_last_cid = cid;

            // Trigger audio beep immediately
            ac.trigger_node_hover_beep(height_norm, false);

            // Debounce speech
            if now - *last_announce_ts >= DEBOUNCE_SEC {
                *last_announce_ts = now;
                let mut desc = format!("{name}, {location}");
                if !groups.is_empty() {
                    desc += &format!(", {groups}");
                }
                say_excluded(&desc);
            }
        }
    }
    Ok(())
}

/// The CTRL poll at the top of each pass: returns the new `last_ctrl_state`.
#[doc(hidden)]
pub fn poll_ctrl(ctrl_down: bool, last_ctrl_state: bool) -> bool {
    if !NODEGRAB_MODE_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    if ctrl_down && !last_ctrl_state {
        // CTRL just pressed
        state().nodegrab.nodegrab_scanning = true;
        send_nodegrab_cmd("SCAN_ON");
        send_nodegrab_cmd("SNAP");
    } else if !ctrl_down && last_ctrl_state {
        // CTRL just released
        state().nodegrab.nodegrab_scanning = false;
        send_nodegrab_cmd("SCAN_OFF");
    }
    ctrl_down
}

#[cfg(windows)]
fn ctrl_is_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL};
    // SAFETY: GetAsyncKeyState takes a virtual-key code by value and has no preconditions.
    let s = unsafe { GetAsyncKeyState(i32::from(VK_CONTROL.0)) };
    (s as u16 & 0x8000) != 0
}

#[cfg(not(windows))]
fn ctrl_is_down() -> bool {
    false
}

/// `nodegrab_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn nodegrab_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let mut last_ctrl_state = false;
    // faster polling for CTRL state
    let mut sock = match UdpListener::bind(NODEGRAB_LISTEN_PORT, 0.05, 2048) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Node grabber listener could not bind port {NODEGRAB_LISTEN_PORT}: {e}");
            log::info!("Node grabber listener stopped.");
            return;
        }
    };
    log::info!("Node grabber listener started on port {NODEGRAB_LISTEN_PORT}");
    let mut first_packet = true;
    let mut last_announce_ts = 0.0;

    while !stop_event.is_set() {
        // Poll CTRL key state via Win32 API (more reliable than keyboard hooks)
        let ctrl = if NODEGRAB_MODE_ACTIVE.load(Ordering::SeqCst) {
            ctrl_is_down()
        } else {
            false
        };
        last_ctrl_state = poll_ctrl(ctrl, last_ctrl_state);

        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Node grabber listener socket error.");
            }
            Recv::Data(data, addr) => {
                if first_packet {
                    log::info!(
                        "First UDP packet from node grabber (source: {})",
                        py_addr(&addr)
                    );
                    first_packet = false;
                }
                let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                let text = py_strip(&text);
                if text.is_empty() {
                    continue;
                }
                let now = time_time();
                if let Err(e) =
                    handle_nodegrab_text(&audio_controller, text, now, &mut last_announce_ts)
                {
                    log::error!("Node grabber listener ended: {e}");
                    break;
                }
            }
        }
    }
    log::info!("Node grabber listener stopped.");
}

/// `_nodegrab_on_space(event)`: Ctrl+Space while the node grabber is on releases every
/// pinned node.
///
/// Runs inside the low-level hook, so it only classifies the key and queues the send.
/// Plain Space passes through untouched, so the handbrake still works. Auto-repeat is
/// ignored until Space is released, or holding the chord would send UNPIN thirty times a
/// second. Returns true to pass the key on.
#[doc(hidden)]
pub fn nodegrab_on_space(event: &KeyEvent) -> bool {
    if event.event_type == KeyEventType::Up {
        NODEGRAB_SPACE_HELD.store(false, Ordering::SeqCst);
        return true;
    }
    if !NODEGRAB_MODE_ACTIVE.load(Ordering::SeqCst) || !keyboard::is_pressed("ctrl") {
        return true;
    }
    if !NODEGRAB_SPACE_HELD.load(Ordering::SeqCst) {
        NODEGRAB_SPACE_HELD.store(true, Ordering::SeqCst);
        keyboard::command_dispatch(Box::new(|| send_nodegrab_cmd("UNPIN")));
    }
    false
}

/// `_install_nodegrab_hooks()`. No mouse hook: the game's own strength toast already
/// speaks the wheel, and a pin is announced from the Lua side's PINNED.
fn install_nodegrab_hooks() {
    if state().nodegrab.nodegrab_space_hook.is_some() {
        return;
    }
    NODEGRAB_SPACE_HELD.store(false, Ordering::SeqCst);
    // Not `hook_suppressed`: that never shows the handler KEY_UP, and the repeat guard
    // needs it.
    let cb: RawHookCallback = Arc::new(nodegrab_on_space);
    match keyboard::try_hook_key_raw("space", cb) {
        Ok(rec) => state().nodegrab.nodegrab_space_hook = Some(rec),
        Err(e) => log::error!("Failed to install node grabber Ctrl+Space hook: {e}"),
    }
}

/// `_remove_nodegrab_hooks()`.
fn remove_nodegrab_hooks() {
    let rec = {
        let mut st = state();
        st.nodegrab.nodegrab_scanning = false;
        st.nodegrab.nodegrab_space_hook.take()
    };
    if let Some(rec) = rec {
        keyboard::unhook_suppressed(&mut vec![rec]);
    }
}

/// `toggle_nodegrab_mode(audio_controller)` (F9 Ctrl+N).
pub fn toggle_nodegrab_mode(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    let active = !NODEGRAB_MODE_ACTIVE.load(Ordering::SeqCst);
    NODEGRAB_MODE_ACTIVE.store(active, Ordering::SeqCst);
    state().nodegrab.nodegrab_last_cid = -1;

    send_nodegrab_cmd(if active { "ON" } else { "OFF" });

    if active {
        install_nodegrab_hooks();
    } else {
        remove_nodegrab_hooks();
    }

    say_excluded(&format!(
        "Accessible node grabber {}",
        if active { "on" } else { "off" }
    ));
}
