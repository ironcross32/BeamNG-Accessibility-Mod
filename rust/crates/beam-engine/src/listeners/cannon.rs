//! Owner: E3. Cannon shot listener (`beamtel.py:2117-2197`).
//!
//! One short line per shot, arriving when the car comes to rest. The mod owns the tracking
//! (only it can see the firing axis and the ramp's live pitch); this owns every word. No tone
//! on purpose: the crash has just made a great deal of noise.

use beam_base::pyfmt::{py_f, py_float_parse, py_strip};

use crate::listeners::dock_phrases::{cannon_shot_phrase, wire_int};
use crate::net::{CANNON_SHOT_LISTEN_PORT, Decode, Recv, UdpListener, decode};
use crate::speech_out::{py_repr_str, say_plain};
use crate::state::{CannonShot, state};
use crate::timer::{Event, monotonic};

/// `float(text)`, with Python's error message.
pub(crate) fn py_float_field(text: &str) -> Result<f64, String> {
    py_float_parse(text)
        .ok_or_else(|| format!("could not convert string to float: {}", py_repr_str(text)))
}

/// `int(text)`, with Python's error message.
pub(crate) fn py_int_field(text: &str) -> Result<i64, String> {
    wire_int(text).ok_or_else(|| {
        format!(
            "invalid literal for int() with base 10: {}",
            py_repr_str(text)
        )
    })
}

/// The `SHOT:` line's fields, in the order Python evaluates them. `Err` is the ValueError
/// message.
fn parse_shot(parts: &[&str], stamp: f64) -> Result<CannonShot, String> {
    Ok(CannonShot {
        downrange: py_float_field(parts[0])?,
        lateral: py_float_field(parts[1])?,
        apex: py_float_field(parts[2])?,
        flight: py_float_field(parts[3])?,
        ramp_pitch: py_float_field(parts[4])?,
        strength: py_int_field(parts[5])?,
        settled: py_strip(parts[6]) == "1",
        index: py_int_field(parts[7])?,
        near_dist: py_float_field(parts[8])?,
        near_name: py_strip(parts[9]).to_string(),
        vehicle: py_strip(parts[10]).to_string(),
        stamp,
        prev_downrange: None,
    })
}

/// One datagram's text (already decoded and stripped), at monotonic time `mono`.
pub fn handle_cannon_text(text: &str, mono: f64) {
    let Some(body) = text.strip_prefix("SHOT:") else {
        return;
    };
    let parts: Vec<&str> = body.split(',').collect();
    if parts.len() < 11 {
        return;
    }
    let mut shot = match parse_shot(&parts, mono) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Malformed cannon shot packet: {e}");
            return;
        }
    };
    // The comparison is against the shot before this one, which the mod tracks: a beamtel
    // restart mid-session leaves Python with an empty history and the mod with the real one.
    if parts.len() >= 12 {
        match py_float_field(parts[11]) {
            Ok(prev) => {
                if prev > -1e8 {
                    shot.prev_downrange = Some(prev);
                }
            }
            Err(e) => {
                log::warn!("Malformed cannon shot packet: {e}");
                return;
            }
        }
    } else {
        let st = state();
        if let Some(last) = st.dock.cannon_shot_session.last() {
            shot.prev_downrange = Some(last.downrange);
        }
    }

    let announce = {
        let mut st = state();
        st.dock.cannon_shot_session.push(shot.clone());
        st.dock.last_cannon_shot = Some(shot.clone());
        st.cfg.announce_cannon_shot
    };

    log::info!(
        "Cannon shot {}: {} m downrange, {} m lateral, apex {} m, {} s, ramp pitch {} deg, strength {}{}",
        shot.index,
        py_f(shot.downrange, 1),
        py_f(shot.lateral, 1),
        py_f(shot.apex, 1),
        py_f(shot.flight, 1),
        py_f(shot.ramp_pitch, 1),
        shot.strength,
        if shot.settled { "" } else { " (never settled)" },
    );
    // Unsolicited, so it goes in the review buffer.
    if announce {
        say_plain(&cannon_shot_phrase(&shot));
    }
}

/// `cannon_shot_listener(stop_event)`. Runs until `stop_event` is set.
pub fn cannon_shot_listener(stop_event: &Event) {
    match UdpListener::bind(CANNON_SHOT_LISTEN_PORT, 0.2, 2048) {
        Ok(mut sock) => {
            log::info!("Cannon shot listener started on port {CANNON_SHOT_LISTEN_PORT}");
            while !stop_event.is_set() {
                match sock.recv() {
                    Recv::Timeout => continue,
                    Recv::Error(_) => {
                        if stop_event.is_set() {
                            break;
                        }
                        log::error!("Cannon shot listener socket error.");
                    }
                    Recv::Data(data, _) => {
                        let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                        handle_cannon_text(py_strip(&text), monotonic());
                    }
                }
            }
        }
        Err(e) => log::error!("Cannon shot listener failed to bind: {e}"),
    }
    log::info!("Cannon shot listener stopped.");
}
