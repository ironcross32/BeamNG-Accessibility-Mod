//! Owner: E3. Route beacon listener, `_route_is_set`, `toggle_route_beacon`
//! (`beamtel.py:2478-2551, 7125-7156`).
//!
//! The listener writes state only. The beacon's bearing is derived in the telemetry loop from
//! the 60 Hz MotionSim position and heading, and the age-out is applied there too.

use std::sync::Arc;

use beam_audio::controller::AudioController;
use beam_base::pyfmt::{py_f, py_strip};
use beam_base::route_beacon::{parse_route_packet, relative_bearing, route_beacon_phrase};

use crate::net::{Decode, ROUTE_BEACON_LISTEN_PORT, Recv, UdpListener, decode};
use crate::speech_out::say_excluded;
use crate::state::{ROUTE_STALE_SEC, State, state};
use crate::timer::{Event, monotonic};
use crate::units::units_mode;

/// One datagram's text (decoded and stripped), at monotonic time `mono`.
pub fn handle_route_text(text: &str, mono: f64) {
    if !text.starts_with("ROUTE:") {
        return;
    }
    let packet = match parse_route_packet(text) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Malformed route beacon packet: {e}");
            return;
        }
    };
    let mut st = state();
    let r = &mut st.route;
    // CLEAR is the mod telling us (route cleared, arrival, level change): immediate, not an
    // age-out a second and a half later.
    let Some(packet) = packet else {
        if r.route_dest_x.is_some() {
            log::info!("Navigation route cleared.");
        }
        r.route_dest_x = None;
        r.route_dest_y = None;
        r.route_remaining_m = 0.0;
        r.route_beacon_stamp = 0.0;
        return;
    };
    let (dx, dy, _dz) = packet.dest;
    if r.route_dest_x.is_none() {
        log::info!(
            "Navigation route set: destination {}, {}.",
            py_f(dx, 1),
            py_f(dy, 1)
        );
    }
    r.route_dest_x = Some(dx);
    r.route_dest_y = Some(dy);
    r.route_remaining_m = packet.route_m;
    r.route_beacon_stamp = mono;
}

/// `route_beacon_listener(stop_event)`. Runs until `stop_event` is set.
pub fn route_beacon_listener(stop_event: &Event) {
    match UdpListener::bind(ROUTE_BEACON_LISTEN_PORT, 0.2, 1024) {
        Ok(mut sock) => {
            log::info!("Route beacon listener started on port {ROUTE_BEACON_LISTEN_PORT}");
            while !stop_event.is_set() {
                match sock.recv() {
                    Recv::Timeout => continue,
                    Recv::Error(_) => {
                        if stop_event.is_set() {
                            break;
                        }
                        log::error!("Route beacon listener socket error.");
                    }
                    Recv::Data(data, _) => {
                        let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                        handle_route_text(py_strip(&text), monotonic());
                    }
                }
            }
        }
        Err(e) => log::error!("Route beacon listener failed to bind: {e}"),
    }
    log::info!("Route beacon listener stopped.");
}

/// `_route_is_set()`. Ported in full by EA (menus and the telemetry loop call it under the lock).
pub fn route_is_set(st: &State) -> bool {
    if st.route.route_dest_x.is_none() {
        return false;
    }
    (crate::timer::monotonic() - st.route.route_beacon_stamp) <= ROUTE_STALE_SEC
}

/// `toggle_route_beacon(audio_controller)` (F9 Ctrl+W).
///
/// Refuses to switch on with no route: a mode that reports itself on and then makes no sound
/// is indistinguishable from a broken one.
pub fn toggle_route_beacon(audio_controller: Arc<AudioController>) {
    let mut st = state();
    if !st.route.route_beacon_active {
        if !route_is_set(&st) {
            drop(st);
            audio_controller.set_route_beacon_mode(false);
            say_excluded("No route set. Choose a destination on the map first.");
            return;
        }
        st.route.route_beacon_active = true;
        let (px, py, heading) = (st.tel.last_pos_x, st.tel.last_pos_y, st.tel.last_heading);
        // Python reads the destination bare; route_is_set has just confirmed it is present.
        let dest_x = st.route.route_dest_x.unwrap_or(f64::NAN);
        let dest_y = st.route.route_dest_y.unwrap_or(f64::NAN);
        let remaining = st.route.route_remaining_m;
        drop(st);
        audio_controller.set_route_beacon_mode(true);
        let (dist, brg) = relative_bearing(px, py, heading, dest_x, dest_y);
        let phrase = route_beacon_phrase(dist, brg, remaining, &units_mode())
            .unwrap_or_else(|e| panic!("ValueError: {e}"));
        say_excluded(&phrase);
        return;
    }
    st.route.route_beacon_active = false;
    drop(st);
    audio_controller.set_route_beacon_mode(false);
    say_excluded("Route beacon off");
}
