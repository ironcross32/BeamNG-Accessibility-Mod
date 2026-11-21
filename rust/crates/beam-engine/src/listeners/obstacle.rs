//! Owner: E2. Obstacle detector: packet parser, listener, state push and toggle (`beamtel.py:1122-1255, 6966-7035`).

use std::sync::Arc;

use beam_audio::controller::{AudioController, HazardIn};
use beam_base::pyerr::ValueError;
use beam_base::pyfmt::{py_f, py_float_parse, py_strip};
use beam_base::pyval::{py_int_parse, py_max, py_min};

use crate::listeners::scanner::{e2_send, py_addr};
use crate::net::{Decode, OBSTACLE_CMD_PORT, OBSTACLE_LISTEN_PORT, Recv, UdpListener, decode};
use crate::speech_out::{py_repr_str, say};
use crate::state::{
    OBSTACLE_PEDAL_DELTA, OBSTACLE_STATE_HEARTBEAT_S, OBSTACLE_STEERING_DELTA, REVERSE, State,
    state,
};
use crate::timer::{Event, monotonic};

/// The selected static hazard (`parse_obstacle_packet`'s dict).
#[derive(Debug, Clone, PartialEq)]
pub struct StaticHazard {
    pub bearing: f64,
    pub urgency: i64,
    pub gap: f64,
    pub state: i64,
    pub closing: f64,
    pub ttc: f64,
    pub stopping_margin: f64,
    pub legacy: bool,
}

impl StaticHazard {
    /// The dict `audio_controller.update_selected_hazard` receives.
    pub fn to_hazard_in(&self) -> HazardIn {
        HazardIn {
            bearing: Some(self.bearing),
            urgency: Some(self.urgency as f64),
            gap: Some(self.gap),
            state: Some(self.state as f64),
            closing: Some(self.closing),
            ttc: Some(self.ttc),
            stopping_margin: Some(self.stopping_margin),
            legacy: Some(self.legacy),
        }
    }
}

/// `parse_obstacle_packet(text)`'s result.
#[derive(Debug, Clone, PartialEq)]
pub enum ObstaclePacket {
    Clear,
    /// The selected static hazard.
    Static(StaticHazard),
    /// (type, bearing, urgency, distance)
    Terrain(i64, f64, i64, f64),
}

fn pfloat(s: &str) -> Result<f64, ValueError> {
    py_float_parse(s).ok_or_else(|| {
        ValueError::new(format!(
            "could not convert string to float: {}",
            py_repr_str(s)
        ))
    })
}

/// Python's `int(s)`. Values beyond `i64` saturate (Python's int would not).
fn pint(s: &str) -> Result<i64, ValueError> {
    py_int_parse(s)
        .map(|v| v.clamp_py(i64::MIN, i64::MAX))
        .ok_or_else(|| {
            ValueError::new(format!(
                "invalid literal for int() with base 10: {}",
                py_repr_str(s)
            ))
        })
}

/// `parse_obstacle_packet(text)`: static-obstacle CSV in both protocol generations.
/// `Err` is the Python's ValueError.
pub fn parse_obstacle_packet(text: &str) -> Result<ObstaclePacket, ValueError> {
    let text = py_strip(text);
    if text == "0" {
        return Ok(ObstaclePacket::Clear);
    }
    let parts: Vec<&str> = text.split(',').collect();
    let pkt_type = pint(parts[0])?;
    if pkt_type == 2 || pkt_type == 3 {
        if parts.len() != 4 {
            return Err(ValueError::new("terrain packet must have four fields"));
        }
        return Ok(ObstaclePacket::Terrain(
            pkt_type,
            pfloat(parts[1])?,
            pint(parts[2])?,
            pfloat(parts[3])?,
        ));
    }
    if pkt_type != 1 || parts.len() < 5 {
        return Err(ValueError::new("unknown or incomplete obstacle packet"));
    }

    let count = pint(parts[1])?;
    // New wire shape: one legacy triple followed by state, closing, TTC and stopping margin.
    if count == 1 && parts.len() >= 9 {
        let st = pint(parts[5])?;
        if !(1..=3).contains(&st) {
            return Err(ValueError::new("invalid obstacle state"));
        }
        return Ok(ObstaclePacket::Static(StaticHazard {
            bearing: pfloat(parts[2])?,
            urgency: pint(parts[3])?,
            gap: pfloat(parts[4])?,
            state: st,
            closing: pfloat(parts[6])?,
            ttc: pfloat(parts[7])?,
            stopping_margin: pfloat(parts[8])?,
            legacy: false,
        }));
    }

    let mut candidates: Vec<(f64, i64, f64)> = Vec::new();
    let mut i: usize = 0;
    while (i as i64) < count.max(0) {
        let base = 2 + i * 3;
        if base + 2 >= parts.len() {
            break;
        }
        candidates.push((
            pfloat(parts[base])?,
            pint(parts[base + 1])?,
            pfloat(parts[base + 2])?,
        ));
        i += 1;
    }
    if candidates.is_empty() {
        return Err(ValueError::new(
            "legacy static packet contains no complete triples",
        ));
    }
    // max(..., key=urgency): the first of equal maxima wins.
    let mut best = candidates[0];
    for c in &candidates[1..] {
        if c.1 > best.1 {
            best = *c;
        }
    }
    let (bearing, urgency, distance) = best;
    Ok(ObstaclePacket::Static(StaticHazard {
        bearing,
        urgency,
        gap: distance,
        state: if urgency >= 170 { 2 } else { 1 },
        closing: 0.0,
        ttc: -1.0,
        stopping_margin: f64::INFINITY,
        legacy: true,
    }))
}

/// The listener's state between datagrams.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct ObstacleListenerState {
    pub static_protocol_logged: bool,
    /// `terrain_announced[2]`, `terrain_announced[3]`.
    pub terrain_announced: [bool; 2],
}

/// One decoded, stripped, non-empty datagram.
#[doc(hidden)]
pub fn handle_obstacle_text(ac: &AudioController, ls: &mut ObstacleListenerState, text: &str) {
    let packet = match parse_obstacle_packet(text) {
        Ok(p) => p,
        Err(exc) => {
            log::warn!("Malformed obstacle packet: {exc}");
            return;
        }
    };
    match packet {
        ObstaclePacket::Clear => {
            ac.update_selected_hazard(None);
            ls.terrain_announced = [false, false];
        }
        ObstaclePacket::Static(hazard) => {
            if !ls.static_protocol_logged {
                ls.static_protocol_logged = true;
                if hazard.legacy {
                    log::warn!(
                        "Obstacle detector is sending the legacy multi-obstacle protocol; new audio is active, but predictive TTC/surface-gap detection is not. Restart BeamNG or reload obstacleDetector."
                    );
                } else {
                    log::info!(
                        "Predictive obstacle protocol active (surface gap, explicit state, closing speed and TTC)."
                    );
                }
            }
            ac.update_selected_hazard(Some(&hazard.to_hazard_in()));
        }
        ObstaclePacket::Terrain(pkt_type, bearing, urgency, distance) => {
            ac.update_obstacle(pkt_type, bearing, urgency as f64, distance);
            if pkt_type == 2 && !ls.terrain_announced[0] {
                say("Drop-off ahead", true, true, "python");
                ls.terrain_announced = [true, false];
            } else if pkt_type == 3 && !ls.terrain_announced[1] {
                say("Steep hill ahead", true, true, "python");
                ls.terrain_announced = [false, true];
            }
        }
    }
}

/// `obstacle_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn obstacle_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    // Text CSV format: "type,bearing,urgency,distance" or "0" for all clear.
    let mut sock = match UdpListener::bind(OBSTACLE_LISTEN_PORT, 0.2, 1024) {
        Ok(s) => s,
        Err(e) => {
            log::error!(
                "Obstacle detector listener could not bind port {OBSTACLE_LISTEN_PORT}: {e}"
            );
            log::info!("Obstacle detector listener stopped.");
            return;
        }
    };
    log::info!("Obstacle detector listener started on port {OBSTACLE_LISTEN_PORT}");
    let mut first_packet = true;
    let mut ls = ObstacleListenerState::default();
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Obstacle listener socket error.");
            }
            Recv::Data(data, addr) => {
                if first_packet {
                    log::info!(
                        "First UDP packet received from obstacle detector (source: {})",
                        py_addr(&addr)
                    );
                    first_packet = false;
                }
                let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                let text = py_strip(&text);
                if text.is_empty() {
                    continue;
                }
                handle_obstacle_text(&audio_controller, &mut ls, text);
            }
        }
    }
    log::info!("Obstacle detector listener stopped.");
}

/// `_send_obstacle_command(command)` (port OBSTACLE_CMD_PORT).
pub fn send_obstacle_command(command: &str) -> bool {
    e2_send(
        OBSTACLE_CMD_PORT,
        command,
        Some("obstacle detector command"),
    )
}

/// `_push_obstacle_state(force=False, gear=None)`. Called with `state_lock` held by the telemetry loop.
///
/// Pushes driver intent on material changes and as a half-second heartbeat.
pub fn push_obstacle_state(st: &mut State, force: bool, gear: Option<&str>) -> bool {
    if !st.obstacle.obstacle_mode_active {
        return false;
    }
    let now = monotonic();
    let gear_value: String = match gear {
        Some(g) => g.to_string(),
        None => {
            if st.tel.protocol_mode == "extended" {
                st.tel.last_gear_str.clone().unwrap_or_default()
            } else if st.tel.last_gear_byte == Some(REVERSE) {
                "R".to_string()
            } else {
                "F".to_string()
            }
        }
    };
    let direction = if py_strip(&gear_value).to_uppercase().starts_with('R') {
        "R"
    } else {
        "F"
    };
    // **Deliberate difference (python-bugs.md 30, user decision 2026-11-21).** The Python
    // always sent 0 (its global was never written), so this sign was never exercised.
    // obstacleDetector.lua rotates its ray fan about the vehicle's up axis by
    // `steering * maxShift`, and a positive angle turns toward the LEFT of the direction of
    // travel (`rotateVectorAroundAxis(travel, up, centre)`, travel = -fwd in reverse).
    // BeamNG's steering_input is positive to the RIGHT. Going forward the path bends the
    // way the wheel points, so the fan wants -steering; in reverse a right-steered car's
    // rear swings right, which is the left of the (rearward) direction of travel, so
    // +steering. `0.0 - s` keeps a centred wheel at "0.000" rather than "-0.000".
    // A NaN reading is no bend: the Python's clamp turned it into full right lock.
    let raw = st.tel.last_steering_input;
    let steering = if raw.is_nan() {
        0.0
    } else {
        py_max(-1.0, py_min(1.0, raw))
    };
    let path_bend = if direction == "R" {
        steering
    } else {
        0.0 - steering
    };
    let new_state = (
        direction.to_string(),
        path_bend,
        py_max(0.0, py_min(1.0, st.tel.last_throttle)),
        py_max(0.0, py_min(1.0, st.tel.last_brake)),
    );
    let last = &st.obstacle.obstacle_state_last;
    let mut changed = last.as_ref().is_none_or(|l| new_state.0 != l.0);
    if let Some(l) = last {
        changed = changed || (new_state.1 - l.1).abs() >= OBSTACLE_STEERING_DELTA;
        changed = changed || (new_state.2 - l.2).abs() >= OBSTACLE_PEDAL_DELTA;
        changed = changed || (new_state.3 - l.3).abs() >= OBSTACLE_PEDAL_DELTA;
    }
    if !(force
        || changed
        || now - st.obstacle.obstacle_state_last_sent >= OBSTACLE_STATE_HEARTBEAT_S)
    {
        return false;
    }
    let command = format!(
        "STATE,{},{},{},{}",
        new_state.0,
        py_f(new_state.1, 3),
        py_f(new_state.2, 3),
        py_f(new_state.3, 3)
    );
    if send_obstacle_command(&command) {
        st.obstacle.obstacle_state_last = Some(new_state);
        st.obstacle.obstacle_state_last_sent = now;
        return true;
    }
    false
}

/// The `SENSITIVITY,<level>` command for a configured sensitivity.
#[doc(hidden)]
pub fn sensitivity_command(configured: &str) -> String {
    let mut sensitivity = if configured.is_empty() {
        "normal".to_string()
    } else {
        configured.to_lowercase()
    };
    if !matches!(sensitivity.as_str(), "early" | "normal" | "late") {
        sensitivity = "normal".to_string();
    }
    format!("SENSITIVITY,{sensitivity}")
}

/// `_send_obstacle_configuration()`.
pub fn send_obstacle_configuration() -> bool {
    let configured = state().cfg.obstacle_warning_sensitivity.clone();
    send_obstacle_command(&sensitivity_command(&configured))
}

/// `toggle_obstacle_mode(audio_controller)`.
pub fn toggle_obstacle_mode(audio_controller: Arc<AudioController>) {
    let active = {
        let mut st = state();
        st.obstacle.obstacle_mode_active = !st.obstacle.obstacle_mode_active;
        st.obstacle.obstacle_mode_active
    };

    send_obstacle_command(if active { "ON" } else { "OFF" });
    if active {
        send_obstacle_configuration();
        let mut st = state();
        push_obstacle_state(&mut st, true, None);
    } else {
        let mut st = state();
        st.obstacle.obstacle_state_last = None;
        st.obstacle.obstacle_state_last_sent = 0.0;
    }

    audio_controller.set_obstacle_mode(active);

    say(
        &format!("Obstacle detection {}", if active { "on" } else { "off" }),
        true,
        true,
        "python",
    );
}
