//! Owner: E3. Route beacon listener, `_route_is_set`, `toggle_route_beacon` (`beamtel.py:2478-2551, 7125-7156`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E3.

use std::sync::Arc;

use beam_audio::controller::AudioController;
use crate::state::{ROUTE_STALE_SEC, State};
use crate::timer::Event;

/// `route_beacon_listener(stop_event)`. Runs until `stop_event` is set.
pub fn route_beacon_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("route_beacon_listener");
}

/// `_route_is_set()`. Ported in full by EA (menus and the telemetry loop call it under the lock).
pub fn route_is_set(st: &State) -> bool {
    if st.route.route_dest_x.is_none() {
        return false;
    }
    (crate::timer::monotonic() - st.route.route_beacon_stamp) <= ROUTE_STALE_SEC
}

/// `toggle_route_beacon(audio_controller)` (F9 Ctrl+W).
pub fn toggle_route_beacon(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    not_ported!("toggle_route_beacon");
}
