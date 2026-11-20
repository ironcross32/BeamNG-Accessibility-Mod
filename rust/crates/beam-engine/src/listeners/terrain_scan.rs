//! Owner: E3. Terrain scan listener, scan and nearest-POI triggers and their watchdogs
//! (`beamtel.py:2575-2697, 3157-3220`).
//!
//! The synthesis runs on the listener thread, not in the audio callback. A snapshot is one
//! datagram of roughly six kilobytes, hence the 65535 buffer: a short read would fail as a
//! landscape that stops halfway rather than as an error.

use std::sync::Arc;

use beam_audio::consts::SCAN_FAMILY_CODES;
use beam_audio::controller::AudioController;
use beam_audio::scan::{ScanError, ScanObject, ScanPoi, ScanSample};
use beam_base::pyfmt::py_strip;
use beam_reports::poi_guidance::{parse_poi_packet, poi_phrase};
use beam_reports::pyops::PyErrKind;

use crate::listeners::cannon::py_float_field;
use crate::net::{Decode, Recv, TERRAIN_SCAN_LISTEN_PORT, UdpListener, decode};
use crate::speech_out::say_excluded;
use crate::state::{state, world_is_active};
use crate::timer::{Event, sleep, spawn_daemon, time_time};
use crate::units::units_mode;

/// What the listener hands a parsed scan to (`audio_controller.render_and_play_scan`).
pub trait ScanRenderer {
    fn render_and_play_scan(
        &self,
        samples: &[ScanSample],
        objects: &[ScanObject],
        reach_m: f64,
        pois: &[ScanPoi],
    ) -> Result<(), ScanError>;
}

impl ScanRenderer for AudioController {
    fn render_and_play_scan(
        &self,
        samples: &[ScanSample],
        objects: &[ScanObject],
        reach_m: f64,
        pois: &[ScanPoi],
    ) -> Result<(), ScanError> {
        AudioController::render_and_play_scan(self, samples, objects, reach_m, pois)
    }
}

/// A parsed `SCAN` snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedScan {
    pub reach: f64,
    pub samples: Vec<ScanSample>,
    pub objects: Vec<ScanObject>,
    pub pois: Vec<ScanPoi>,
}

/// The rows of a `SCAN` snapshot after its head. `Err` is the ValueError message.
pub fn parse_scan_rows(reach: f64, lines: &[&str]) -> Result<ParsedScan, String> {
    let mut samples = Vec::new();
    let mut objects = Vec::new();
    let mut pois = Vec::new();
    for ln in lines {
        if *ln == "END" {
            break;
        }
        let parts: Vec<&str> = ln.split(',').collect();
        if parts[0] == "S" && parts.len() >= 3 {
            let bearing = py_float_field(parts[1])?;
            let cells = &parts[2..];
            // Range is derived from the cell's INDEX against the reported reach, so the two
            // ends cannot disagree about the time axis.
            let denom = std::cmp::max(1, cells.len() as i64 - 1) as f64;
            for (i, cell) in cells.iter().enumerate() {
                let rng = (i as f64 / denom) * reach;
                // The surface family is an OPTIONAL ':' suffix, stripped FIRST so an older mod
                // half parses as it always did. Missing or unknown means paved.
                let mut cell: &str = cell;
                let mut family = None;
                if let Some((c, code)) = cell.split_once(':') {
                    cell = c;
                    family = SCAN_FAMILY_CODES
                        .iter()
                        .find(|(k, _)| *k == code)
                        .map(|(_, v)| v.to_string());
                }
                if cell == "~" {
                    // No surface. Not zero: zero is level ground.
                    samples.push(ScanSample {
                        bearing_deg: bearing,
                        range_m: rng,
                        dz_m: None,
                        depth_m: None,
                        family: None,
                    });
                } else if let Some((dz, depth)) = cell.split_once('_') {
                    samples.push(ScanSample {
                        bearing_deg: bearing,
                        range_m: rng,
                        dz_m: Some(py_float_field(dz)?),
                        depth_m: Some(py_float_field(depth)?),
                        family: None,
                    });
                } else {
                    samples.push(ScanSample {
                        bearing_deg: bearing,
                        range_m: rng,
                        dz_m: Some(py_float_field(cell)?),
                        depth_m: None,
                        family,
                    });
                }
            }
        } else if parts[0] == "O" && parts.len() >= 4 {
            // The p/v kind tag picks the ping length. Absent means "v", the safer default.
            let kind = if parts.len() >= 5 { parts[4] } else { "v" };
            objects.push(ScanObject {
                bearing_deg: py_float_field(parts[1])?,
                range_m: py_float_field(parts[2])?,
                dz_m: py_float_field(parts[3])?,
                kind: Some(kind.to_string()),
            });
        } else if parts[0] == "P" && parts.len() >= 4 {
            pois.push(ScanPoi {
                bearing_deg: py_float_field(parts[1])?,
                range_m: py_float_field(parts[2])?,
                dz_m: py_float_field(parts[3])?,
            });
        }
    }
    Ok(ParsedScan {
        reach,
        samples,
        objects,
        pois,
    })
}

/// One datagram's text (decoded and stripped), at wall time `now`. Returns false where the
/// Python listener thread would die on an uncaught exception.
pub fn handle_terrain_text(renderer: &dyn ScanRenderer, text: &str, now: f64) -> bool {
    if text.is_empty() {
        return true;
    }
    if text.starts_with("POI:") {
        state().scan.last_poi_reply_ts = now;
        match parse_poi_packet(text) {
            Ok(poi) => say_excluded(&poi_phrase(&poi, &units_mode())),
            Err(e) if e.kind == PyErrKind::ValueError => {
                log::warn!("Malformed nearest POI packet: {}", e.msg);
                say_excluded("Point of interest response was invalid");
            }
            Err(e) => {
                log::error!(
                    "Terrain scan listener crashed: {}: {}",
                    e.kind.name(),
                    e.msg
                );
                return false;
            }
        }
        return true;
    }
    if text.starts_with("POI_FAIL:") {
        state().scan.last_poi_reply_ts = now;
        let reason = match text.split_once(':').map(|(_, r)| py_strip(r)) {
            Some(r) if !r.is_empty() => r,
            _ => "unknown error",
        };
        say_excluded(&format!("Nearest point of interest unavailable. {reason}"));
        return true;
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let head: Vec<&str> = lines[0].split(',').collect();
    if head[0] == "FAIL" {
        state().scan.last_scan_reply_ts = now;
        say_excluded(&format!(
            "Scan failed. {}",
            if head.len() > 1 { head[1] } else { "unknown" }
        ));
        return true;
    }
    if head[0] != "SCAN" || head.len() < 6 {
        return true;
    }
    // Stamped before the body is parsed: a malformed scan still counts as an answer.
    state().scan.last_scan_reply_ts = now;
    let parsed = py_float_field(head[4]).and_then(|reach| parse_scan_rows(reach, &lines[1..]));
    let scan = match parsed {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Malformed terrain scan packet: {e}");
            return true;
        }
    };
    match renderer.render_and_play_scan(&scan.samples, &scan.objects, scan.reach, &scan.pois) {
        Ok(()) => true,
        // int(NaN) is a ValueError, which the listener catches; int(inf) is an OverflowError,
        // which it does not.
        Err(e) if e.0.contains("infinity") => {
            log::error!("Terrain scan listener crashed: OverflowError: {e}");
            false
        }
        Err(e) => {
            log::warn!("Malformed terrain scan packet: {e}");
            true
        }
    }
}

/// `terrain_scan_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn terrain_scan_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    match UdpListener::bind(TERRAIN_SCAN_LISTEN_PORT, 0.2, 65535) {
        Ok(mut sock) => {
            log::info!("Terrain scan listener started on port {TERRAIN_SCAN_LISTEN_PORT}");
            while !stop_event.is_set() {
                match sock.recv() {
                    Recv::Timeout => continue,
                    Recv::Error(_) => {
                        if stop_event.is_set() {
                            break;
                        }
                        log::error!("Terrain scan listener socket error.");
                    }
                    Recv::Data(data, _) => {
                        let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                        if !handle_terrain_text(&*audio_controller, py_strip(&text), time_time()) {
                            break;
                        }
                    }
                }
            }
        }
        Err(e) => log::error!("Terrain scan listener failed to bind: {e}"),
    }
    log::info!("Terrain scan listener stopped.");
}

/// `_watch_scan_reply(before)`: speak only if the scan never comes back. `before` is sampled
/// by the CALLER before the command goes out, or a fast reply races it.
fn watch_scan_reply(before: f64, timeout_s: f64) {
    let deadline = time_time() + timeout_s;
    while time_time() < deadline {
        if state().scan.last_scan_reply_ts != before {
            return;
        }
        sleep(0.1);
    }
    say_excluded("No scan. Terrain scanner not responding");
}

/// `_send_scan_cmd(command)` (port TERRAIN_SCAN_CMD_PORT).
pub fn send_scan_cmd(command: &str) {
    crate::net::send_cmd(
        crate::net::TERRAIN_SCAN_CMD_PORT,
        command,
        "terrain scan command",
    );
}

/// `trigger_terrain_scan_driving_only()`. Never falls through to a UI context action, so a
/// controller activation delayed past a screen transition cannot press a menu button.
pub fn trigger_terrain_scan_driving_only() {
    if !world_is_active() {
        say_excluded("Terrain scan is only available while driving");
        return;
    }
    let scan_seen_before = state().scan.last_scan_reply_ts;
    send_scan_cmd("SCAN");
    spawn_daemon("watch-scan-reply", move || {
        watch_scan_reply(scan_seen_before, 1.5)
    });
}

/// `_watch_poi_reply(before)`.
fn watch_poi_reply(before: f64, timeout_s: f64) {
    let deadline = time_time() + timeout_s;
    while time_time() < deadline {
        if state().scan.last_poi_reply_ts != before {
            return;
        }
        sleep(0.1);
    }
    say_excluded("No point of interest response. Terrain scanner not responding");
}

/// `trigger_nearest_poi()` (F9 Shift+W).
pub fn trigger_nearest_poi() {
    if !world_is_active() {
        say_excluded("Nearest point of interest is only available while driving");
        return;
    }
    let poi_seen_before = state().scan.last_poi_reply_ts;
    send_scan_cmd("NEAREST_POI");
    spawn_daemon("watch-poi-reply", move || {
        watch_poi_reply(poi_seen_before, 1.5)
    });
}
