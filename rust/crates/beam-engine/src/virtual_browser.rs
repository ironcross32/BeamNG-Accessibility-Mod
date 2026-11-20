//! Owner: E5. The generic virtual browser and buffer mode (`beamtel.py:6262-6459`).
//!
//! Callbacks (`OnEnter`, `OnAdjust`, `OnEscape` in `state`) are always invoked with no
//! state guard held. Entry data is `serde_json::Value` (Python passes ints, dicts and None).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use serde_json::Value;

use crate::keyboard::{self, KeyEvent, KeyHandler};
use crate::speech_out::{say, speech_buffer_snapshot};
use crate::state::{OnAdjust, OnEnter, OnEscape, state};

/// The keyword arguments of `open_virtual_browser`.
#[derive(Clone)]
pub struct BrowserOpts {
    pub title: Option<String>,
    pub on_enter: Option<OnEnter>,
    pub entry_data: Vec<Value>,
    pub start_index: i64,
    pub on_adjust: Option<OnAdjust>,
    pub on_escape: Option<OnEscape>,
    pub announce_interrupt: bool,
}

impl Default for BrowserOpts {
    fn default() -> Self {
        BrowserOpts {
            title: None,
            on_enter: None,
            entry_data: Vec::new(),
            start_index: 0,
            on_adjust: None,
            on_escape: None,
            announce_interrupt: true,
        }
    }
}

impl BrowserOpts {
    /// `title=...` only.
    pub fn titled(title: impl Into<String>) -> Self {
        BrowserOpts {
            title: Some(title.into()),
            ..Default::default()
        }
    }
}

fn say_excluded(text: &str) {
    say(text, true, true, "python");
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_string())
}

// ---- Buffer mode -------------------------------------------------------------------------

/// `on_buffer_nav_press(event)`.
pub fn on_buffer_nav_press(event: &KeyEvent) {
    if !state().menu.buffer_mode_active {
        return;
    }
    // Python holds state_lock across this whole block, speech included; say() takes no
    // state lock, so the order of reads, writes and speech is the same.
    let mut st = state();
    let buffer = speech_buffer_snapshot();
    if buffer.is_empty() {
        say_excluded("Buffer empty");
        return;
    }
    let len = buffer.len() as i64;
    let at = |i: i64| -> &str { &buffer[i as usize] };
    if event.name == "]" {
        // Right bracket, newer messages
        st.menu.current_buffer_index += 1;
        if st.menu.current_buffer_index >= len {
            st.menu.current_buffer_index = len - 1;
            say_excluded(&format!("Bottom: {}", at(st.menu.current_buffer_index)));
        } else {
            say_excluded(at(st.menu.current_buffer_index));
        }
    } else if event.name == "[" {
        // Left bracket, older messages
        st.menu.current_buffer_index -= 1;
        if st.menu.current_buffer_index < 0 {
            st.menu.current_buffer_index = 0;
            say_excluded(&format!("Top: {}", at(st.menu.current_buffer_index)));
        } else {
            say_excluded(at(st.menu.current_buffer_index));
        }
    }
}

/// `toggle_buffer_mode()`.
pub fn toggle_buffer_mode() {
    let active = {
        let mut st = state();
        st.menu.buffer_mode_active = !st.menu.buffer_mode_active;
        if st.menu.buffer_mode_active {
            let len = crate::speech_out::speech_buffer_len() as i64;
            st.menu.current_buffer_index = if len > 0 { len - 1 } else { -1 };
        }
        st.menu.buffer_mode_active
    };
    if active {
        say_excluded("Buffer mode on");
        let handler: KeyHandler = Arc::new(on_buffer_nav_press);
        for key in ["[", "]"] {
            match keyboard::try_hook_suppressed(key, keyboard::kb_enqueue(Arc::clone(&handler))) {
                Ok(rec) => state().menu.buffer_key_hooks.push(rec),
                Err(e) => {
                    log::error!("Failed to hook buffer nav keys: {e}");
                    break;
                }
            }
        }
    } else {
        say_excluded("Buffer mode off");
        let mut hooks = std::mem::take(&mut state().menu.buffer_key_hooks);
        keyboard::unhook_suppressed(&mut hooks);
    }
}

// ---- The virtual browser -----------------------------------------------------------------

/// `_on_vbrowser_nav(event)`.
pub(crate) fn on_vbrowser_nav(event: &KeyEvent) {
    let speech = {
        let mut st = state();
        let vb = &mut st.vbrowser;
        if vb.vbrowser_lines.is_empty() {
            return;
        }
        let len = vb.vbrowser_lines.len() as i64;
        let mut index = vb.vbrowser_index as i64;
        let speech = if event.name == "down" {
            index += 1;
            if index >= len {
                index = len - 1;
                Some(format!("Bottom: {}", vb.vbrowser_lines[index as usize]))
            } else {
                Some(vb.vbrowser_lines[index as usize].clone())
            }
        } else if event.name == "up" {
            index -= 1;
            if index < 0 {
                index = 0;
                Some(format!("Top: {}", vb.vbrowser_lines[index as usize]))
            } else {
                Some(vb.vbrowser_lines[index as usize].clone())
            }
        } else {
            None
        };
        vb.vbrowser_index = index as usize;
        speech
    };
    if let Some(s) = speech {
        say_excluded(&s);
    }
}

/// `_on_vbrowser_adjust(event)`: left/right on a browser row that supports editing.
///
/// Only hooked when a browser passes on_adjust, so every other browser keeps left/right
/// for the game. Shift is the coarse step: a value with a range of a hundred is otherwise
/// a hundred presses away from its far end.
pub(crate) fn on_vbrowser_adjust(event: &KeyEvent) {
    let (on_adjust, idx, line, data) = {
        let st = state();
        let vb = &st.vbrowser;
        let Some(on_adjust) = vb.vbrowser_on_adjust.clone() else {
            return;
        };
        if vb.vbrowser_lines.is_empty() {
            return;
        }
        let idx = vb.vbrowser_index;
        (
            on_adjust,
            idx,
            vb.vbrowser_lines.get(idx).cloned(),
            vb.vbrowser_entry_data.get(idx).cloned(),
        )
    };
    let mut delta: i64 = if event.name == "right" { 1 } else { -1 };
    if keyboard::is_pressed("shift") {
        delta *= 10;
    }
    // `_vbrowser_lines[idx]` raising IndexError is caught by the same handler.
    let Some(line) = line else {
        log::error!("Virtual browser adjust callback error: list index out of range");
        return;
    };
    if let Err(p) = catch_unwind(AssertUnwindSafe(|| on_adjust(idx, &line, data, delta))) {
        log::error!("Virtual browser adjust callback error: {}", panic_text(&*p));
    }
}

/// `_on_vbrowser_escape(event)`.
pub(crate) fn on_vbrowser_escape() {
    let on_escape = state().vbrowser.vbrowser_on_escape.clone();
    if let Some(cb) = on_escape {
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| cb())) {
            log::error!("Virtual browser escape callback error: {}", panic_text(&*p));
        }
        return;
    }
    close_virtual_browser(true);
}

/// `_on_vbrowser_enter(event)`.
pub(crate) fn on_vbrowser_enter() {
    let (on_enter, idx, line, data) = {
        let st = state();
        let vb = &st.vbrowser;
        let Some(on_enter) = vb.vbrowser_on_enter.clone() else {
            return;
        };
        if vb.vbrowser_lines.is_empty() {
            return;
        }
        let idx = vb.vbrowser_index;
        let Some(line) = vb.vbrowser_lines.get(idx).cloned() else {
            // `_vbrowser_lines[idx]` raises outside the callback's try, out to the worker.
            panic!("IndexError: list index out of range");
        };
        (
            on_enter,
            idx,
            line,
            vb.vbrowser_entry_data.get(idx).cloned(),
        )
    };
    if let Err(p) = catch_unwind(AssertUnwindSafe(|| on_enter(idx, &line, data))) {
        log::error!("Virtual browser enter callback error: {}", panic_text(&*p));
    }
}

/// `open_virtual_browser(lines, title=None, on_enter=None, entry_data=None, start_index=0,
/// on_adjust=None, on_escape=None, announce_interrupt=True)`.
pub fn open_virtual_browser(lines: Vec<String>, opts: BrowserOpts) {
    if crate::vehicle_spawner::is_modal_open() {
        crate::vehicle_spawner::close_modal();
    }
    close_virtual_browser(false);
    crate::listeners::lineup::lineup_close_layer(false);
    if lines.is_empty() {
        say_excluded("No information available");
        return;
    }
    let has_enter = opts.on_enter.is_some();
    let has_adjust = opts.on_adjust.is_some();
    let first_line = {
        let mut st = state();
        let vb = &mut st.vbrowser;
        vb.vbrowser_lines = lines;
        let last = vb.vbrowser_lines.len() as i64 - 1;
        vb.vbrowser_index = opts.start_index.min(last).max(0) as usize;
        vb.vbrowser_active = true;
        vb.vbrowser_on_enter = opts.on_enter;
        vb.vbrowser_on_adjust = opts.on_adjust;
        vb.vbrowser_on_escape = opts.on_escape;
        vb.vbrowser_entry_data = opts.entry_data;
        vb.vbrowser_lines[vb.vbrowser_index].clone()
    };
    if keyboard::keyboard_ok() {
        let nav: KeyHandler = Arc::new(on_vbrowser_nav);
        let escape: KeyHandler = Arc::new(|_e: &KeyEvent| on_vbrowser_escape());
        let enter: KeyHandler = Arc::new(|_e: &KeyEvent| on_vbrowser_enter());
        let adjust: KeyHandler = Arc::new(on_vbrowser_adjust);
        let mut plan: Vec<(&str, KeyHandler)> =
            vec![("up", Arc::clone(&nav)), ("down", Arc::clone(&nav))];
        // Escape especially must be queued: close_virtual_browser unhooks the very hook
        // that delivered it.
        plan.push(("escape", escape));
        if has_enter {
            plan.push(("enter", enter));
        }
        // Left/right are only taken when a browser actually edits something. Suppressing
        // them unconditionally would swallow steering for every read-only list in the mod.
        if has_adjust {
            plan.push(("left", Arc::clone(&adjust)));
            plan.push(("right", adjust));
        }
        for (key, handler) in plan {
            match keyboard::try_hook_suppressed(key, keyboard::kb_enqueue(handler)) {
                Ok(rec) => state().vbrowser.vbrowser_hooks.push(rec),
                Err(e) => {
                    log::error!("Failed to hook virtual browser keys: {e}");
                    break;
                }
            }
        }
    }
    let text = match &opts.title {
        Some(title) if !title.is_empty() => format!("{title}. {first_line}"),
        _ => first_line,
    };
    say(&text, opts.announce_interrupt, true, "python");
}

/// `close_virtual_browser(speak_exit=True)`.
pub fn close_virtual_browser(speak_exit: bool) {
    let mut hooks = {
        let mut st = state();
        st.env.env_browser_open = false;
        st.aud.aud_browser = None;
        let vb = &mut st.vbrowser;
        vb.vbrowser_active = false;
        vb.vbrowser_on_enter = None;
        vb.vbrowser_on_adjust = None;
        vb.vbrowser_on_escape = None;
        vb.vbrowser_entry_data = Vec::new();
        std::mem::take(&mut vb.vbrowser_hooks)
    };
    keyboard::unhook_suppressed(&mut hooks);
    {
        let mut st = state();
        st.vbrowser.vbrowser_lines = Vec::new();
        st.vbrowser.vbrowser_index = 0;
    }
    if speak_exit {
        say_excluded("Exit");
    }
}

/// `_drive_virtual_browser(action)`: move the open browser from a controller's
/// accessibility actions.
pub fn drive_virtual_browser(action: &str) {
    if action == "next_menu" || action == "previous_menu" {
        on_vbrowser_escape();
        return;
    }
    if action == "status_up" {
        on_vbrowser_nav(&KeyEvent::down("up"));
        return;
    }
    if action == "status_down" {
        on_vbrowser_nav(&KeyEvent::down("down"));
        return;
    }
    if action == "activate" && state().vbrowser.vbrowser_on_enter.is_some() {
        on_vbrowser_enter();
        return;
    }
    // status_repeat, and activate on a read-only list: re-read where we are. A browser
    // with no enter action has nothing to activate, and silence there would read as the
    // pad having stopped working.
    let line = {
        let st = state();
        let vb = &st.vbrowser;
        if vb.vbrowser_lines.is_empty() {
            None
        } else {
            Some(vb.vbrowser_lines[vb.vbrowser_index].clone())
        }
    };
    if let Some(line) = line {
        say_excluded(&line);
    }
}
