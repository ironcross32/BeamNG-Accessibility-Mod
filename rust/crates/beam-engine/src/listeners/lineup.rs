//! Owner: E3. Free-camera line-up guide: listener, key layer, toggle (`beamtel.py:2200-2371`).
//!
//! F9 then Alt+G opens a key layer: X mode, arrows distance, T target, M mute, Escape closes.
//! The guide keeps sounding after the layer closes; values last for the session only, and the
//! mute is never written to the config. See docs/camera-lineup.md.

use std::sync::Arc;

use beam_audio::controller::AudioController;
use beam_reports::camera_lineup::{self, LineupPacket};

use crate::keyboard::{
    KeyEvent, kb_enqueue, keyboard_ok, release_arrow_owners, try_hook_suppressed, unhook_suppressed,
};
use crate::net::{CAMERA_LINEUP_LISTEN_PORT, Decode, Recv, UdpListener, decode};
use crate::speech_out::say_excluded;
use crate::state::{State, audio_controller_ref, state};
use crate::timer::{Event, monotonic};
use crate::units::units_mode;

/// cameraLineup.lua sends at 20 Hz.
pub const LINEUP_STALE_S: f64 = 0.5;

/// `_lineup_in_free_cam()`.
fn lineup_in_free_cam(st: &State) -> bool {
    st.lineup.lineup_geom.is_some() && (monotonic() - st.lineup.lineup_stamp) <= LINEUP_STALE_S
}

/// The `set_lineup` arguments `_lineup_push` would pass for this state: `None` for
/// `set_lineup(False)` (muted, disabled or no geometry).
pub fn lineup_tone(st: &State) -> Option<camera_lineup::Tone> {
    let l = &st.lineup;
    let geom = l.lineup_geom.as_ref()?;
    if !l.lineup_enabled || l.lineup_muted {
        return None;
    }
    let (status, miss, bearing) =
        camera_lineup::solve(geom, &l.lineup_mode, l.lineup_offset_m, l.lineup_depth_m);
    Some(camera_lineup::tone_for(status, miss, bearing))
}

/// `_lineup_push(audio_controller=None)`: recompute the tone from the latest geometry and hand
/// it to audio.
pub fn lineup_push(audio_controller: Option<Arc<AudioController>>) {
    let Some(ac) = audio_controller.or_else(audio_controller_ref) else {
        return;
    };
    let tone = lineup_tone(&state());
    match tone {
        None => ac.set_lineup(false, 0.0, 1.0, 1.0, 0.0),
        Some(t) => ac.set_lineup(true, t.amp, t.index, t.ratio, t.az),
    }
}

/// One datagram's decoded text, at monotonic `mono`, then the push.
pub fn handle_lineup_text(audio_controller: &Arc<AudioController>, text: &str, mono: f64) {
    let Some(parsed) = camera_lineup::parse_packet(text) else {
        return;
    };
    {
        let mut st = state();
        match parsed {
            LineupPacket::Off => st.lineup.lineup_geom = None,
            LineupPacket::Geometry(g) => {
                st.lineup.lineup_geom = Some(g);
                st.lineup.lineup_stamp = mono;
            }
        }
    }
    lineup_push(Some(Arc::clone(audio_controller)));
}

/// `camera_lineup_listener(audio_controller, stop_event)`. A feed that stops is aged out in
/// the audio callback, since nothing runs here to notice.
pub fn camera_lineup_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let mut sock = match UdpListener::bind(CAMERA_LINEUP_LISTEN_PORT, 0.2, 1024) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Camera line-up listener failed to bind: {e}");
            return;
        }
    };
    log::info!("Camera line-up listener started on port {CAMERA_LINEUP_LISTEN_PORT}");
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Camera line-up listener socket error.");
                continue;
            }
            Recv::Data(data, _) => {
                let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                let ac = &audio_controller;
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handle_lineup_text(ac, &text, monotonic())
                }));
                if r.is_err() {
                    log::error!("Camera line-up push failed");
                }
            }
        }
    }
}

/// `_lineup_state_suffix()`.
fn lineup_state_suffix(st: &State) -> String {
    let mut text = String::new();
    if st.lineup.lineup_muted {
        text.push_str(", muted");
    }
    if !lineup_in_free_cam(st) {
        text.push_str(", not in free camera");
    }
    text
}

/// `camera_lineup.target_phrase(...)` for the current state. Python lets the (unreachable)
/// error propagate to the command worker, which logs it.
fn target_phrase(st: &State) -> Result<String, String> {
    let l = &st.lineup;
    camera_lineup::target_phrase(
        &l.lineup_mode,
        l.lineup_offset_m,
        l.lineup_depth_m,
        &units_mode(),
    )
    .map_err(|e| e.msg)
}

/// `_lineup_on_key(event)`, run on the command worker.
pub fn lineup_on_key(event: &KeyEvent) {
    if !state().lineup.lineup_layer_open {
        return;
    }
    let name = event.name.to_lowercase();
    // keyboard hooks the key as "escape" but REPORTS it as "esc".
    if name == "esc" || name == "escape" {
        lineup_close_layer(true);
        return;
    }
    let speech: Result<Option<String>, String> = {
        let mut st = state();
        match name.as_str() {
            "x" => {
                st.lineup.lineup_mode = if st.lineup.lineup_mode == camera_lineup::HEAD_ON {
                    camera_lineup::CROSSING.to_string()
                } else {
                    camera_lineup::HEAD_ON.to_string()
                };
                target_phrase(&st).map(Some)
            }
            "up" | "right" | "down" | "left" => {
                let delta = if name == "up" || name == "right" {
                    1
                } else {
                    -1
                };
                let l = &mut st.lineup;
                let (offset, depth) = camera_lineup::step_value(
                    &l.lineup_mode,
                    l.lineup_offset_m,
                    l.lineup_depth_m,
                    delta,
                );
                l.lineup_offset_m = offset;
                l.lineup_depth_m = depth;
                camera_lineup::value_phrase(&l.lineup_mode, offset, depth, &units_mode())
                    .map(Some)
                    .map_err(|e| e.msg)
            }
            "t" => target_phrase(&st).map(|p| Some(p + &lineup_state_suffix(&st))),
            "m" => {
                st.lineup.lineup_muted = !st.lineup.lineup_muted;
                Ok(Some(
                    if st.lineup.lineup_muted {
                        "Guide muted"
                    } else {
                        "Guide on"
                    }
                    .to_string(),
                ))
            }
            _ => Ok(None),
        }
    };
    match speech {
        Ok(Some(text)) => say_excluded(&text),
        Ok(None) => {}
        Err(e) => {
            log::error!("Command handler raised: {e}");
            return;
        }
    }
    lineup_push(None);
}

/// `_lineup_close_layer(speak=False)`: hand the guide's keys back. The guide keeps sounding.
pub fn lineup_close_layer(speak: bool) {
    let (was_open, mut hooks) = {
        let mut st = state();
        let was_open = st.lineup.lineup_layer_open;
        st.lineup.lineup_layer_open = false;
        (was_open, std::mem::take(&mut st.lineup.lineup_hooks))
    };
    unhook_suppressed(&mut hooks);
    if speak && was_open {
        say_excluded("Line-up keys closed");
    }
}

/// `open_camera_lineup()` (F9 Alt+G), a toggle. Off -> arms the guide and takes its keys
/// until Escape. On -> turns the guide off entirely and releases the keys; values stay.
pub fn open_camera_lineup() {
    let enabled = state().lineup.lineup_enabled;
    if enabled {
        state().lineup.lineup_enabled = false;
        lineup_close_layer(false);
        lineup_push(None);
        say_excluded("Camera line-up off");
        return;
    }
    release_arrow_owners("camera line-up opened");
    {
        let mut st = state();
        st.lineup.lineup_enabled = true;
        st.lineup.lineup_layer_open = true;
    }
    if keyboard_ok() {
        for key in ["x", "t", "m", "up", "down", "left", "right", "escape"] {
            // Queued: Escape unhooks the very hooks the keyboard layer is iterating to reach it.
            match try_hook_suppressed(key, kb_enqueue(Arc::new(lineup_on_key))) {
                Ok(rec) => state().lineup.lineup_hooks.push(rec),
                Err(e) => {
                    log::error!("Failed to hook camera line-up keys: {e}");
                    break;
                }
            }
        }
    }
    let text = {
        let st = state();
        target_phrase(&st).map(|p| format!("Camera line-up. {p}{}", lineup_state_suffix(&st)))
    };
    match text {
        Ok(t) => say_excluded(&t),
        Err(e) => {
            log::error!("Command handler raised: {e}");
            return;
        }
    }
    lineup_push(None);
}
