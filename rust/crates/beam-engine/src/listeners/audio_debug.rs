//! Owner: E4. Audio debug listener (`beamtel.py:3951-4009`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E4.

use crate::timer::Event;

/// `audio_debug_listener(stop_event)`. Runs until `stop_event` is set.
pub fn audio_debug_listener(stop_event: &Event) {
    let _ = stop_event;
    not_ported!("audio_debug_listener");
}
