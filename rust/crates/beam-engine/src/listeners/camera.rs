//! Owner: E4. Camera info listener, commands, diagnostics and toggle (`beamtel.py:3256-3416, 7159-7236`).

use std::sync::Arc;

use beam_audio::controller::AudioController;
use beam_base::pyfmt::{py_f, py_float_parse, py_mod, py_strip};

use crate::listeners::scanner::{e2_send, py_addr};
use crate::net::{CAMERA_CMD_PORT, CAMERA_LISTEN_PORT, Decode, Recv, UdpListener, decode};
use crate::speech_out::say;
use crate::state::{CAMERA_STALE_SEC, VEHICLE_SWITCH_QUIET_S, state};
use crate::timer::{Event, monotonic, time_time};
use crate::units::{COMPASS_NAMES, compass_zone_index, units_mode};

fn say_excluded(text: &str) {
    say(text, true, true, "python");
}

/// `int(text) == 1`; `None` where `int()` raises (and, here, for ints beyond i64, which
/// can never equal 1 anyway).
fn int_is_one(text: &str) -> Option<bool> {
    let v = beam_base::pyval::py_int_parse(text)?;
    Some(v.to_i64() == Some(1))
}

/// One datagram from cameraInfo.lua. `Err` is an exception the Python listener does not
/// catch, which ends the thread.
#[doc(hidden)]
pub fn handle_camera_datagram(ac: &AudioController, data: &[u8]) -> Result<(), String> {
    // Diagnostic replies answer a one-shot query, so they arrive whether or not the live
    // feed is switched on and are handled before that gate.
    if data.starts_with(b"DIAG:") {
        let text = decode(data, Decode::AsciiReplace).unwrap_or_default();
        let stripped = py_strip(&text);
        let body = stripped.get("DIAG:".len()..).unwrap_or("");
        let (heights, detail) = match body.split_once('|') {
            Some((h, d)) => (h, d),
            None => (body, ""),
        };
        log::info!(
            "CAMERA DIAG: {}",
            if detail.is_empty() { heights } else { detail }
        );
        say_excluded(&format_camera_diag(heights));
        return Ok(());
    }

    if !state().camera.free_cam_active {
        return Ok(());
    }
    // `except (ValueError, UnicodeDecodeError): pass`.
    let Some(text) = decode(data, Decode::AsciiStrict) else {
        return Ok(());
    };
    let text = py_strip(&text);
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() < 6 {
        return Ok(());
    }
    let mut nums = [0.0f64; 5];
    for (i, n) in nums.iter_mut().enumerate() {
        match py_float_parse(parts[i]) {
            Some(v) => *n = v,
            None => return Ok(()),
        }
    }
    let [yaw, pitch, agl, bearing, distance] = nums;
    let Some(is_free) = int_is_one(parts[5]) else {
        return Ok(());
    };
    // 7th field is optional: an older Lua mod sends six, and its AGL was always presented
    // as valid.
    let agl_valid = if parts.len() < 7 {
        true
    } else {
        match int_is_one(parts[6]) {
            Some(v) => v,
            None => return Ok(()),
        }
    };

    let now = time_time();
    let mut speech: Option<String> = None;
    {
        let mut st = state();
        let interval = st.cfg.compass_click_interval_deg;
        let highlight_enabled = st.cfg.compass_highlight_enabled;
        let nth = st.cfg.compass_highlight_nth_click;
        let min_interval = st.tel.compass_min_interval;
        let switch_ts = st.last_vehicle_switch_ts;
        let c = &mut st.camera;
        c.cam_yaw_deg = yaw;
        c.cam_pitch_deg = pitch;
        c.cam_agl = agl;
        c.cam_agl_valid = agl_valid;
        c.cam_last_packet_ts = now;
        c.cam_veh_bearing = bearing;
        c.cam_veh_distance = distance;
        c.cam_is_free = is_free;

        // Camera compass click logic (same as vehicle compass)
        let heading = yaw;
        let mut delta_heading = heading - c.cam_last_click_heading_deg;
        if delta_heading > 180.0 {
            delta_heading -= 360.0;
        }
        if delta_heading < -180.0 {
            delta_heading += 360.0;
        }

        if delta_heading.abs() >= interval {
            c.cam_compass_click_counter += 1;
            // math.cos raises ValueError on an infinite argument: caught, the packet is
            // dropped with the counter already bumped.
            if heading.is_infinite() {
                return Ok(());
            }
            let pitch_mult = 1.0 + 0.25 * heading.to_radians().cos();

            if highlight_enabled && c.cam_compass_click_counter >= nth {
                ac.trigger_cam_compass_highlight(heading, pitch_mult * 1.5);
                c.cam_compass_click_counter = 0;
            } else {
                ac.trigger_cam_compass_click(heading, pitch_mult);
            }

            if interval == 0.0 {
                return Err("ZeroDivisionError: float division by zero".to_string());
            }
            let q = heading / interval;
            if q.is_nan() {
                return Ok(()); // round(nan): ValueError, caught
            }
            if q.is_infinite() {
                return Err("OverflowError: cannot convert float infinity to integer".to_string());
            }
            let num_intervals = q.round_ties_even();
            c.cam_last_click_heading_deg = py_mod(num_intervals * interval, 360.0);
        }

        // Camera compass direction announcements
        let current_compass_idx = compass_zone_index(heading);
        if current_compass_idx != c.cam_last_announced_compass_idx {
            if current_compass_idx != -1 {
                let in_switch_quiet = (monotonic() - switch_ts) < VEHICLE_SWITCH_QUIET_S;
                if (now - c.cam_last_compass_ts) >= min_interval && !in_switch_quiet {
                    speech = Some(format!(
                        "Camera {}",
                        COMPASS_NAMES[current_compass_idx as usize]
                    ));
                    c.cam_last_compass_ts = now;
                }
            }
            c.cam_last_announced_compass_idx = current_compass_idx;
        }
    }
    // Python speaks inside the lock; say() takes no state lock, so the order is the same.
    if let Some(s) = speech {
        say_excluded(&s);
    }
    Ok(())
}

/// `camera_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn camera_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let mut sock = match UdpListener::bind(CAMERA_LISTEN_PORT, 0.2, 1024) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Camera info listener could not bind port {CAMERA_LISTEN_PORT}: {e}");
            log::info!("Camera info listener stopped.");
            return;
        }
    };
    log::info!("Camera info listener started on port {CAMERA_LISTEN_PORT}");
    let mut first_packet = true;
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Camera listener socket error.");
            }
            Recv::Data(data, addr) => {
                if first_packet {
                    log::info!(
                        "First UDP packet received from camera info (source: {})",
                        py_addr(&addr)
                    );
                    first_packet = false;
                }
                if let Err(e) = handle_camera_datagram(&audio_controller, data) {
                    log::error!("Camera info listener ended: {e}");
                    break;
                }
            }
        }
    }
    log::info!("Camera info listener stopped.");
}

/// `send_camera_command(command)` (port CAMERA_CMD_PORT). False if it couldn't be sent.
pub fn send_camera_command(command: &str) -> bool {
    e2_send(
        CAMERA_CMD_PORT,
        command,
        Some(&format!("camera command {command}")),
    )
}

/// `_camera_feed_problem()`: why the camera readouts can't be trusted right now, or None
/// when they're live.
///
/// The Lua side stops streaming whenever the mod reloads and cannot tell us, so without
/// this the readouts keep speaking the last packet, frozen but indistinguishable from a
/// live value. Asking for the feed again here means the next press usually works.
pub fn camera_feed_problem() -> Option<String> {
    if !state().camera.free_cam_active {
        return Some("Camera info is off".to_string());
    }
    let age = time_time() - state().camera.cam_last_packet_ts;
    if age > CAMERA_STALE_SEC {
        send_camera_command("ON");
        return Some("Camera info is not updating, reconnecting".to_string());
    }
    None
}

/// `send_camera_diag()` (F9 Alt+Shift+A): a one-shot camera/ground dump, answered on the
/// camera data port whether or not the live feed is on.
pub fn send_camera_diag() {
    if send_camera_command("DIAG") {
        say_excluded("Camera diagnostic requested");
    } else {
        say_excluded("Camera diagnostic failed to send");
    }
}

/// `_format_camera_diag(heights)`: `heights` is "<cameraZ>,<groundZ>" in metres, groundZ
/// "nan" when no ground was found, or "ERR". Converted here so this and the Alt+A
/// altitude never report one height as two numbers.
pub fn format_camera_diag(heights: &str) -> String {
    let vals: Vec<&str> = heights.split(',').take(2).collect();
    let parsed: Option<Vec<f64>> = vals.iter().map(|v| py_float_parse(v)).collect();
    let (cam_z_m, ground_m) = match parsed.as_deref() {
        Some([a, b]) => (*a, *b),
        _ => return "Camera diagnostic failed, see the log.".to_string(),
    };
    let (conv, unit) = if units_mode() == "imperial" {
        (3.28084, "feet")
    } else {
        (1.0, "meters")
    };
    if ground_m.is_nan() {
        return format!(
            "Camera height {} {unit}. No ground found below.",
            py_f(cam_z_m * conv, 1)
        );
    }
    format!(
        "Camera height {} {unit}. Ground {}. Above ground {} {unit}.",
        py_f(cam_z_m * conv, 1),
        py_f(ground_m * conv, 1),
        py_f((cam_z_m - ground_m) * conv, 1)
    )
}

/// `toggle_free_cam_info(audio_controller)` (F9 Alt+F).
pub fn toggle_free_cam_info(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    let active = {
        let mut st = state();
        st.camera.free_cam_active = !st.camera.free_cam_active;
        st.camera.free_cam_active
    };
    send_camera_command(if active { "ON" } else { "OFF" });
    if active {
        let mut st = state();
        let c = &mut st.camera;
        c.cam_last_click_heading_deg = 0.0;
        c.cam_compass_click_counter = 0;
        c.cam_last_announced_compass_idx = -1;
        c.cam_last_compass_ts = 0.0;
    }
    say_excluded(&format!(
        "Camera info {}",
        if active { "on" } else { "off" }
    ));
}
