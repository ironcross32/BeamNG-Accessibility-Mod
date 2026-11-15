//! Owner: E3. Terrain scan listener, scan and nearest-POI triggers and their watchdogs (`beamtel.py:2575-2697, 3157-3220`).
//!
//! STUB: signatures fixed by EA (every `pub fn` here is called from another module); bodies to be ported by E3.

use std::sync::Arc;

use beam_audio::controller::AudioController;
use crate::timer::Event;

/// `terrain_scan_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn terrain_scan_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let _ = (audio_controller, stop_event);
    not_ported!("terrain_scan_listener");
}

/// `_send_scan_cmd(command)` (port TERRAIN_SCAN_CMD_PORT).
pub fn send_scan_cmd(command: &str) {
    crate::net::send_cmd(
        crate::net::TERRAIN_SCAN_CMD_PORT,
        command,
        "terrain scan command",
    );
}

/// `trigger_terrain_scan_driving_only()`.
pub fn trigger_terrain_scan_driving_only() {
    not_ported!("trigger_terrain_scan_driving_only");
}

/// `trigger_nearest_poi()` (F9 Shift+W).
pub fn trigger_nearest_poi() {
    not_ported!("trigger_nearest_poi");
}
