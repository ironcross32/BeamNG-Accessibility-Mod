//! Owner: E3. Free-camera line-up guide: listener, key layer, toggle (`beamtel.py:2200-2371`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E3.

use std::sync::Arc;

use crate::compat::beam_audio::controller::AudioController;
use crate::timer::Event;

pub const LINEUP_STALE_S: f64 = 0.5;

/// `camera_lineup_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn camera_lineup_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let _ = (audio_controller, stop_event);
    not_ported!("camera_lineup_listener");
}

/// `_lineup_push(audio_controller=None)`.
pub fn lineup_push(audio_controller: Option<Arc<AudioController>>) {
    let _ = audio_controller;
    not_ported!("lineup_push");
}

/// `_lineup_close_layer(speak=False)`.
pub fn lineup_close_layer(speak: bool) {
    let _ = speak;
    not_ported!("lineup_close_layer");
}

/// `open_camera_lineup()` (F9 Alt+G).
pub fn open_camera_lineup() {
    not_ported!("open_camera_lineup");
}
