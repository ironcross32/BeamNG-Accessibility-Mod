//! Owner: E1. Telemetry decode and the telemetry loop (`beamtel.py:644-735`,
//! `_report_gear_change` at 4306-4316, `telemetry_loop` at 10526-11481): OutGauge, the
//! Extended layouts V1..V6 with their padding, MotionSim, and everything the loop drives.
//!
//! STUB: signatures fixed by EA; bodies to be ported by E1.

use std::sync::Arc;

use beam_audio::controller::AudioController;
use crate::state::State;
use crate::timer::Event;

/// `telemetry_loop(audio_controller, host="0.0.0.0", port=4444, stop_event=None)`.
///
/// Returns when `stop_event` is set. `Err` is the Python's re-raised `OSError` (a bind
/// failure, or a receive error while not stopping), which ends `_run_engine`.
pub fn telemetry_loop(
    audio_controller: Arc<AudioController>,
    host: &str,
    port: u16,
    stop_event: &Event,
) -> std::io::Result<()> {
    let _ = (audio_controller, host, port);
    not_ported!("telemetry::telemetry_loop (waiting for STOP)");
    while !stop_event.wait_secs(0.25) {}
    Ok(())
}

/// `_report_gear_change(phrase, degree, sport=False)`. Caller holds `state_lock`.
pub fn report_gear_change(st: &State, phrase: Option<&str>, degree: Option<i64>, sport: bool) {
    let _ = (st, phrase, degree, sport);
    not_ported!("telemetry::report_gear_change");
}
