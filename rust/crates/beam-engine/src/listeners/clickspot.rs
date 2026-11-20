//! Owner: E4. Clickspot listener, staged list, activation and toggle (`beamtel.py:3574-3784, 7314-7356`).

use std::collections::BTreeMap;
use std::sync::Arc;

use beam_audio::controller::AudioController;
use beam_base::pyfmt::py_strip;
use beam_base::pyval::{PyInt, py_int_parse};

use crate::listeners::nodegrab::warp;
use crate::listeners::scanner::{e2_send, py_addr};
use crate::net::{CLICKSPOT_CMD_PORT, CLICKSPOT_LISTEN_PORT, Decode, Recv, UdpListener, decode};
use crate::speech_out::{py_repr_str, say};
use crate::state::state;
use crate::timer::{Event, sleep, spawn_daemon, time_time};

const DEBOUNCE_SEC: f64 = 0.15;

fn say_excluded(text: &str) {
    say(text, true, true, "python");
}

/// A Python int as an i64, saturating outside the range (only an absurd wire value gets
/// there; it is then outside every list anyway).
fn sat(v: &PyInt) -> i64 {
    match v.to_i64() {
        Some(n) => n,
        None if v.to_string().starts_with('-') => i64::MIN,
        None => i64::MAX,
    }
}

/// `int(text)` for a field whose ValueError this listener does not catch.
fn int_uncaught(text: &str) -> Result<i64, String> {
    py_int_parse(text).and_then(|v| v.to_i64()).ok_or_else(|| {
        format!(
            "ValueError: invalid literal for int() with base 10: {}",
            py_repr_str(text)
        )
    })
}

/// `_clickspot_set_pending()`: clear published rows while Lua rebuilds them for the
/// active vehicle.
pub fn clickspot_set_pending() {
    let mut st = state();
    let c = &mut st.clickspot;
    c.clickspot_mode_active = true;
    c.clickspot_trigger_list = Vec::new();
    c.clickspot_list_loading = true;
    c.clickspot_trigger_building = None;
    c.clickspot_expected_count = None;
    st.menu.current_clickspot_index = 0;
}

/// `_clickspot_set_off()`: synchronise with a Lua-side detector shutdown or world reset.
pub fn clickspot_set_off() {
    let mut st = state();
    let c = &mut st.clickspot;
    c.clickspot_mode_active = false;
    c.clickspot_trigger_list = Vec::new();
    c.clickspot_list_loading = false;
    c.clickspot_trigger_building = None;
    c.clickspot_expected_count = None;
    st.menu.current_clickspot_index = 0;
}

/// `_clickspot_begin_list(count)`: begin an atomic transfer; publish zero rows now.
pub fn clickspot_begin_list(count: i64) {
    let count = count.max(0);
    let mut st = state();
    let c = &mut st.clickspot;
    c.clickspot_mode_active = true;
    c.clickspot_trigger_list = Vec::new();
    c.clickspot_trigger_building = Some(BTreeMap::new());
    c.clickspot_expected_count = Some(count);
    c.clickspot_list_loading = count > 0;
    st.menu.current_clickspot_index = 0;
    if count == 0 {
        let c = &mut st.clickspot;
        c.clickspot_trigger_building = None;
        c.clickspot_expected_count = None;
    }
}

/// `_clickspot_append_row(cache_idx, trigger_id, display_name)`: stage one row and
/// publish the list once its announced count has arrived.
pub fn clickspot_append_row(cache_idx: i64, trigger_id: i64, display_name: &str) -> bool {
    let mut st = state();
    let (Some(expected), true) = (
        st.clickspot.clickspot_expected_count,
        st.clickspot.clickspot_trigger_building.is_some(),
    ) else {
        log::warn!("Clickspot row arrived outside a list transfer; ignoring it");
        return false;
    };
    if cache_idx < 0 || cache_idx >= expected {
        log::warn!("Clickspot cache index {cache_idx} is outside the announced list");
        return false;
    }
    let c = &mut st.clickspot;
    let Some(building) = c.clickspot_trigger_building.as_mut() else {
        return false;
    };
    building.insert(cache_idx, (cache_idx, trigger_id, display_name.to_string()));
    if building.len() as i64 != expected {
        return false;
    }
    c.clickspot_trigger_list = building.values().cloned().collect();
    c.clickspot_trigger_building = None;
    c.clickspot_expected_count = None;
    c.clickspot_list_loading = false;
    st.menu.current_clickspot_index = 0;
    true
}

/// `_announce_clickspot_action(display_name, failure_reason=None)`: activation feedback,
/// only when its opt-in setting is enabled.
fn announce_clickspot_action(display_name: Option<&str>, failure_reason: Option<&str>) {
    if state().cfg.announce_clickspot_actions {
        let message = match failure_reason {
            None => format!("Jumped to {}", display_name.unwrap_or("None")),
            Some(r) => format!("Cannot jump, {r}"),
        };
        say_excluded(&message);
    }
}

/// One decoded, stripped, non-empty datagram from clickspotAccessible.lua. `Err` is an
/// exception the Python listener does not catch: the listener ends.
#[doc(hidden)]
pub fn handle_clickspot_text(
    ac: &AudioController,
    text: &str,
    now: f64,
    last_announce_ts: &mut f64,
) -> Result<(), String> {
    if text == "HOVER_CLEAR" {
        let last = state().clickspot.clickspot_last_hover_id;
        if last != -1 {
            ac.trigger_clickspot_beep(true);
        }
        state().clickspot.clickspot_last_hover_id = -1;
        return Ok(());
    }
    if text == "TRIGGER_LIST_PENDING" {
        clickspot_set_pending();
        return Ok(());
    }
    if text == "TRIGGER_LIST_OFF" {
        clickspot_set_off();
        return Ok(());
    }
    if let Some(rest) = text.strip_prefix("TRIGGER_LIST:") {
        let Some(count) = py_int_parse(rest) else {
            log::warn!("Malformed clickspot list count: {}", py_repr_str(text));
            return Ok(());
        };
        clickspot_begin_list(sat(&count));
        if count == PyInt::Small(0) {
            say_excluded("No clickspots found on this vehicle");
        } else {
            let plural = if count != PyInt::Small(1) { "s" } else { "" };
            say_excluded(&format!("{count} clickspot{plural} found"));
        }
        return Ok(());
    }
    if let Some(rest) = text.strip_prefix("TRIGGER:") {
        // TRIGGER:<cacheIndex>,<triggerId>,<displayName>
        let parts: Vec<&str> = rest.splitn(3, ',').collect();
        if parts.len() == 3 {
            let (Some(cache_idx), Some(trigger_id)) =
                (py_int_parse(parts[0]), py_int_parse(parts[1]))
            else {
                log::warn!("Malformed clickspot row: {}", py_repr_str(text));
                return Ok(());
            };
            clickspot_append_row(sat(&cache_idx), sat(&trigger_id), parts[2]);
        }
        return Ok(());
    }
    if let Some(rest) = text.strip_prefix("HOVER:") {
        // HOVER:<triggerId>,<displayName>
        let parts: Vec<&str> = rest.splitn(2, ',').collect();
        if parts.len() == 2 {
            let trigger_id = int_uncaught(parts[0])?;
            let display_name = parts[1];
            state().clickspot.clickspot_last_hover_id = trigger_id;

            // Forward beep (mouse moved onto clickspot)
            ac.trigger_clickspot_beep(false);

            // Debounce speech
            if now - *last_announce_ts >= DEBOUNCE_SEC {
                *last_announce_ts = now;
                say_excluded(display_name);
            }
        }
        return Ok(());
    }
    if let Some(rest) = text.strip_prefix("SNAP_OK:") {
        // SNAP_OK:<x>,<y>,<viewW>,<viewH>,<triggerId>,<displayName>
        let parts: Vec<&str> = rest.splitn(6, ',').collect();
        if parts.len() >= 6 {
            let (sx, sy) = (int_uncaught(parts[0])?, int_uncaught(parts[1])?);
            let (vw, vh) = (int_uncaught(parts[2])?, int_uncaught(parts[3])?);
            let display_name = parts[5];
            warp(sx, sy, vw, vh);
            // Menu activation never beeps; its success speech is opt-in.
            announce_clickspot_action(Some(display_name), None);
        }
        return Ok(());
    }
    if let Some(reason) = text.strip_prefix("SNAP_FAIL:") {
        announce_clickspot_action(None, Some(reason));
    }
    Ok(())
}

/// `clickspot_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn clickspot_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let mut sock = match UdpListener::bind(CLICKSPOT_LISTEN_PORT, 0.2, 2048) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Clickspot listener could not bind port {CLICKSPOT_LISTEN_PORT}: {e}");
            log::info!("Clickspot listener stopped.");
            return;
        }
    };
    log::info!("Clickspot listener started on port {CLICKSPOT_LISTEN_PORT}");
    let mut first_packet = true;
    let mut last_announce_ts = 0.0;
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Clickspot listener socket error.");
            }
            Recv::Data(data, addr) => {
                if first_packet {
                    log::info!(
                        "First UDP packet from clickspot detector (source: {})",
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
                    handle_clickspot_text(&audio_controller, text, now, &mut last_announce_ts)
                {
                    log::error!("Clickspot listener ended: {e}");
                    break;
                }
            }
        }
    }
    log::info!("Clickspot listener stopped.");
}

/// `_send_clickspot_cmd(command)` (port CLICKSPOT_CMD_PORT).
pub fn send_clickspot_cmd(command: &str) {
    e2_send(CLICKSPOT_CMD_PORT, command, Some("clickspot command"));
}

/// `_activate_clickspot(cache_idx)`: SNAP, then a delayed press and release, through
/// Lua's command path.
pub fn activate_clickspot(cache_idx: i64) {
    send_clickspot_cmd(&format!("SNAP:{cache_idx}"));
    spawn_daemon("clickspot-exec", move || {
        sleep(0.1); // small delay to let SNAP/cursor warp complete
        send_clickspot_cmd(&format!("EXEC:{cache_idx},1"));
        sleep(0.15);
        send_clickspot_cmd(&format!("EXEC:{cache_idx},0"));
    });
}

/// `_clickspot_browser_on_enter(idx, line, data)`: snap the cursor to the trigger and
/// execute press+release.
pub fn clickspot_browser_on_enter(data: Option<&serde_json::Value>) {
    if let Some(idx) = data.and_then(serde_json::Value::as_i64) {
        activate_clickspot(idx);
    }
}

/// `toggle_clickspot_mode(audio_controller)` (F9 Ctrl+Shift+C).
pub fn toggle_clickspot_mode(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    let turning_on = !state().clickspot.clickspot_mode_active;
    if turning_on {
        clickspot_set_pending();
    } else {
        clickspot_set_off();
    }
    send_clickspot_cmd(if turning_on { "ON" } else { "OFF" });
    say_excluded(&format!(
        "Clickspot detection {}",
        if turning_on { "on" } else { "off" }
    ));
}
