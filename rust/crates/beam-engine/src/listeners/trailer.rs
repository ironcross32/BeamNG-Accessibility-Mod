//! Owner: E3. Trailer angle listener and `_trailer_artic_norm` (`beamtel.py:2374-2441, 2554-2572`).
//!
//! The listener writes state only. The tone is driven from the telemetry loop's handoff to
//! audio, which is what keeps the age-out in one place: a value that stops arriving has to
//! expire where it is READ, because nothing runs here to notice the silence.

use beam_base::pyfmt::py_strip;

use crate::listeners::cannon::{py_float_field, py_int_field};
use crate::net::{Decode, Recv, TRAILER_ANGLE_LISTEN_PORT, UdpListener, decode};
use crate::state::{State, TRAILER_FULL_DEG, TRAILER_STALE_SEC, state};
use crate::timer::{Event, monotonic};

/// One datagram's text (decoded and stripped), at monotonic time `mono`.
pub fn handle_trailer_text(text: &str, mono: f64) {
    let Some(body) = text.strip_prefix("TRAILER:") else {
        return;
    };
    // CLEAR is the mod telling us (uncoupling, a vehicle switch, an unmeasurable frame), so it
    // takes effect immediately rather than at the age-out.
    if body == "CLEAR" {
        let mut st = state();
        let t = &mut st.trailer;
        if t.last_trailer_id.is_some() {
            let name = if t.last_trailer_name.is_empty() {
                "?"
            } else {
                t.last_trailer_name.as_str()
            };
            log::info!("Trailer uncoupled ({name}).");
        }
        t.last_trailer_deg = 0.0;
        t.last_trailer_id = None;
        t.last_trailer_name = String::new();
        t.last_trailer_stamp = 0.0;
        return;
    }
    let parts: Vec<&str> = body.split(',').collect();
    if parts.len() < 2 {
        return;
    }
    let parsed = py_float_field(parts[0]).and_then(|d| py_int_field(parts[1]).map(|t| (d, t)));
    let (deg, tid) = match parsed {
        Ok(v) => v,
        Err(e) => {
            log::warn!("Malformed trailer angle packet: {e}");
            return;
        }
    };
    // Positional optional tail: bng_mod/ is a live junction and the halves go out of step.
    let name = if parts.len() >= 3 {
        py_strip(parts[2]).to_string()
    } else {
        "trailer".to_string()
    };
    let mut st = state();
    let t = &mut st.trailer;
    if Some(tid) != t.last_trailer_id {
        log::info!("Trailer coupled: {name} (id {tid}).");
    }
    t.last_trailer_deg = deg;
    t.last_trailer_id = Some(tid);
    t.last_trailer_name = name;
    t.last_trailer_stamp = mono;
}

/// `trailer_angle_listener(stop_event)`. Runs until `stop_event` is set.
pub fn trailer_angle_listener(stop_event: &Event) {
    match UdpListener::bind(TRAILER_ANGLE_LISTEN_PORT, 0.2, 1024) {
        Ok(mut sock) => {
            log::info!("Trailer angle listener started on port {TRAILER_ANGLE_LISTEN_PORT}");
            while !stop_event.is_set() {
                match sock.recv() {
                    Recv::Timeout => continue,
                    Recv::Error(_) => {
                        if stop_event.is_set() {
                            break;
                        }
                        log::error!("Trailer angle listener socket error.");
                    }
                    Recv::Data(data, _) => {
                        let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                        handle_trailer_text(py_strip(&text), monotonic());
                    }
                }
            }
        }
        Err(e) => log::error!("Trailer angle listener failed to bind: {e}"),
    }
    log::info!("Trailer angle listener stopped.");
}

/// `_trailer_artic_norm()`: the trailer angle on the -1..1 scale, or 0.0 for silence.
/// Ported in full by EA because `state::mcp_snapshot_state` needs it.
pub fn trailer_artic_norm(st: &State) -> f64 {
    let t = &st.trailer;
    if t.last_trailer_id.is_none() || t.last_trailer_stamp <= 0.0 {
        return 0.0;
    }
    if (crate::timer::monotonic() - t.last_trailer_stamp) > TRAILER_STALE_SEC {
        return 0.0;
    }
    // max(-1.0, min(1.0, x)): NaN passes through min and max unchanged, as in Python.
    let x = t.last_trailer_deg / TRAILER_FULL_DEG;
    let m = if 1.0 < x { 1.0 } else { x };
    if -1.0 > m { -1.0 } else { m }
}
