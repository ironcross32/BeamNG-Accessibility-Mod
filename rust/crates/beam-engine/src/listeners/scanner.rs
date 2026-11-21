//! Owner: E2. Vehicle scanner listener and callouts, the scanner command sender, gear push and `toggle_scan_mode` (`beamtel.py:796-1120, 3223-3253, 6918-6963`).
//!
//! Also holds the one send seam the E2 listeners share ([`e2_send`]), so the tests can
//! read every command the Python would have put on the wire without binding the game's
//! ports.

use std::net::SocketAddr;
use std::sync::Arc;

use beam_audio::controller::AudioController;
use beam_base::pyfmt::{py_f, py_float_parse, py_strip};
use beam_base::pyval::{py_int_parse, py_max};
use parking_lot::RwLock;

use crate::net::{self, Decode, Recv, SCANNER_CMD_PORT, SCANNER_LISTEN_PORT, UdpListener};
use crate::speech_out::{self, py_repr_str, say};
use crate::state::{IMPL_FLAG_PRESENT, State, driving_assistant, state};
use crate::timer::{Event, monotonic};
use crate::units::{fmt_distance, py_int, units_mode};

// ---- The send seam -------------------------------------------------------------------------

type SendHook = Box<dyn Fn(u16, &[u8]) + Send + Sync>;

static SEND_HOOK: RwLock<Option<SendHook>> = RwLock::new(None);

/// Route every E2 command to `hook` instead of the network (tests only). `None` restores
/// the real sockets.
#[doc(hidden)]
pub fn set_send_hook_for_tests(hook: Option<SendHook>) {
    *SEND_HOOK.write() = hook;
}

/// One E2 command datagram. `what` is the `_send_*_cmd` log noun; `None` is a sender the
/// Python wraps in `except Exception: pass`.
pub(crate) fn e2_send(port: u16, payload: &str, what: Option<&str>) -> bool {
    if let Some(hook) = SEND_HOOK.read().as_ref() {
        hook(port, payload.as_bytes());
        return true;
    }
    match what {
        Some(what) => net::send_cmd(port, payload, what),
        None => {
            net::send_quiet(port, payload.as_bytes());
            true
        }
    }
}

/// Python's `str(addr)` for a `recvfrom` address tuple.
pub(crate) fn py_addr(addr: &SocketAddr) -> String {
    format!("('{}', {})", addr.ip(), addr.port())
}

// ---- The command sender and the gear push --------------------------------------------------

/// `_send_scanner_cmd(command)` (port SCANNER_CMD_PORT).
pub fn send_scanner_cmd(command: &str) {
    e2_send(SCANNER_CMD_PORT, command, Some("scanner command"));
}

/// `_push_gear_direction(gear)`. Called by the telemetry loop while it holds `state_lock`.
///
/// Tells the scanner which end of the vehicle is the business end. Sent only on change,
/// so the 60 Hz loop does not become a 60 Hz send; the Lua side ages it out. The local
/// flag is only a seed for the speech reference: scanner packets carry the direction Lua
/// actually resolved, and that wins.
pub fn push_gear_direction(st: &mut State, gear: &str) {
    let reverse = py_strip(gear).to_uppercase().starts_with('R');
    st.scanner.scanner_ref_reversed = reverse;
    send_scanner_cmd(if reverse { "GEAR:R" } else { "GEAR:F" });
    crate::listeners::obstacle::push_obstacle_state(st, true, Some(gear));
}

// ---- toggle_scan_mode ------------------------------------------------------------------------

/// `toggle_scan_mode(audio_controller)`.
pub fn toggle_scan_mode(audio_controller: Arc<AudioController>) {
    let (active, coupler_run_active) = {
        let mut st = state();
        let s = &mut st.scanner;
        s.scan_mode_active = !s.scan_mode_active;
        s.coupler_dist_mode = false;
        s.last_scanner_target_name = String::new();
        s.last_scanner_distance = f64::INFINITY;
        s.last_scanner_approach_deg = 0.0;
        s.last_scanner_bearing = 0.0;
        (s.scan_mode_active, s.coupler_run_active)
    };

    send_scanner_cmd(if active { "ON" } else { "OFF" });

    audio_controller.set_scan_mode(active);
    if !active && !coupler_run_active {
        audio_controller.set_coupler_tracking(false);
    }

    // On a loader the scanner aims from the implement, not the cab: say so. Gated on the
    // live telemetry flag as well as the name, because the name can go stale across a
    // vehicle switch while implementFlags always describes the vehicle you are in.
    let (word, flags) = {
        let st = state();
        (
            st.dock.implement_word_current.clone(),
            st.tel.last_implement_flags,
        )
    };
    let mut suffix = String::new();
    if active && !word.is_empty() && py_int(flags) & IMPL_FLAG_PRESENT != 0 {
        suffix = format!(", measuring from the {}", word.to_lowercase());
    }
    say(
        &format!(
            "Vehicle scanner {}{suffix}",
            if active { "on" } else { "off" }
        ),
        true,
        true,
        "python",
    );
}

// ---- Phrases ---------------------------------------------------------------------------------

/// `COUPLER_START:` speech: the two coupler tags, the gap to reverse, and the displacement
/// warning (bad news only).
#[doc(hidden)]
pub fn coupler_start_phrase(body: &str) -> String {
    // Positional tail, guarded by length: fields 3 and 4 are optional, so an older mod
    // half still reads the two tags.
    let parts_tags: Vec<&str> = body.split(',').collect();
    let ptag = parts_tags.first().copied().unwrap_or("unknown");
    let ttag = parts_tags.get(1).copied().unwrap_or("unknown");
    let tail = |idx: usize| -> Option<f64> { parts_tags.get(idx).and_then(|p| py_float_parse(p)) };
    let gap_m = tail(2);
    let shifted_m = tail(3);
    let mut msg = format!("Aligned. {ptag} and {ttag}.");
    match gap_m {
        Some(gap) => {
            let (v, u) = fmt_distance(gap);
            msg += &format!(" Reverse {v} {u} to couple.");
        }
        None => msg += " Tracking couplers. Reverse to couple.",
    }
    // A displacement makes the gap above untrue; silence there is how the old build
    // announced a perfect alignment 4.6 metres away.
    if let Some(shifted) = shifted_m
        && shifted > 0.5
    {
        let (v, u) = fmt_distance(shifted);
        msg += &format!(" Could not park you square, moved {v} {u}. Something is in the way.");
    }
    msg
}

/// `COUPLER_DIST:` speech.
#[doc(hidden)]
pub fn coupler_dist_phrase(horiz: f64, height: f64, units: &str) -> String {
    let (horiz_val, height_val, unit) = if units == "imperial" {
        (horiz * 3.28084, height.abs() * 3.28084, "feet")
    } else {
        (horiz, height.abs(), "meters")
    };
    let dist_str = format!("{} {unit}", py_f(horiz_val, 1));
    let height_str = if height_val < 0.05 {
        "level".to_string()
    } else if height > 0.0 {
        format!("{} {unit} high", py_f(height_val, 1))
    } else {
        format!("{} {unit} low", py_f(height_val, 1))
    };
    format!("{dist_str}, {height_str}")
}

/// Python's `str.capitalize()`.
fn py_capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
    }
}

/// `COUPLED_DETECTED:` speech for the parts it switched off.
#[doc(hidden)]
pub fn coupled_phrase(parts_disabled: &[&str]) -> String {
    let mut msg = "Coupled".to_string();
    if !parts_disabled.is_empty() {
        msg += &format!(
            ". {} disabled",
            py_capitalize(&parts_disabled.join(" and "))
        );
    }
    msg
}

/// `AI_STATUS:` speech, or `None` for fewer than five fields.
#[doc(hidden)]
pub fn ai_status_phrase(body: &str) -> Option<String> {
    let parts: Vec<&str> = body.split(',').collect();
    if parts.len() < 5 {
        return None;
    }
    let (mode, aggr, speed, avoid, lane) = (parts[0], parts[1], parts[2], parts[3], parts[4]);
    let speed_str = if speed == "off" {
        "no limit".to_string()
    } else {
        format!("{speed} km/h")
    };
    Some(format!(
        "AI mode {mode}, aggression {aggr}, speed {speed_str}, avoid {avoid}, lane {lane}"
    ))
}

/// The periodic callout. Fore/aft is the DRIVER's frame: the bearing is referenced to the
/// direction of travel, so in reverse only those two words flip; left and right stay the
/// driver's physical sides in every gear.
#[doc(hidden)]
pub fn callout_phrase(dist: f64, brg: f64, tname: &str, reversed_ref: bool, units: &str) -> String {
    let a = brg.abs();
    let direction = if a < 45.0 {
        if reversed_ref {
            "behind you"
        } else {
            "in front of you"
        }
    } else if a > 135.0 {
        if reversed_ref {
            "in front of you"
        } else {
            "behind you"
        }
    } else if brg > 0.0 {
        "to the left of you"
    } else {
        "to the right of you"
    };
    let dist_str = if units == "imperial" {
        format!("{} feet", py_f(dist * 3.28084, 0))
    } else {
        format!("{} meters", py_f(dist, 0))
    };
    let prefix = if tname.is_empty() {
        String::new()
    } else {
        format!("{tname}, ")
    };
    format!("{prefix}{dist_str}, {direction}")
}

// ---- The listener ----------------------------------------------------------------------------

/// How a Python exception leaves the per-datagram body.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub enum Raised {
    /// `ValueError` / `UnicodeDecodeError`: the binary `<ff` fallback runs.
    Value(String),
    /// `OSError`: the outer handler logs a socket error.
    Os(String),
    /// Anything else: the thread dies.
    Other(String),
}

fn float_of(s: &str) -> Result<f64, Raised> {
    py_float_parse(s).ok_or_else(|| {
        Raised::Value(format!(
            "could not convert string to float: {}",
            py_repr_str(s)
        ))
    })
}

/// One text datagram (`data.decode("ascii").strip()` already applied).
fn handle_scanner_text(ac: &AudioController, text: &str) -> Result<(), Raised> {
    if let Some(body) = text.strip_prefix("COUPLER_START:") {
        say(&coupler_start_phrase(body), true, true, "python");
        state().scanner.coupler_run_active = true;
        ac.set_coupler_tracking(true);
    } else if let Some(reason) = text.strip_prefix("COUPLER_FAIL:") {
        say(reason, true, true, "python");
    } else if text == "COUPLER_LOST" {
        say("Coupler tracking lost", true, true, "python");
        state().scanner.coupler_run_active = false;
        ac.set_coupler_tracking(false);
    } else if let Some(body) = text.strip_prefix("COUPLER:") {
        let cparts: Vec<&str> = body.split(',').collect();
        if cparts.len() >= 3 {
            let c_bearing = float_of(cparts[0])?;
            let c_distance = float_of(cparts[1])?;
            let c_in_range = match py_int_parse(cparts[2]) {
                Some(v) => !v.is_zero(),
                None => {
                    return Err(Raised::Value(format!(
                        "invalid literal for int() with base 10: {}",
                        py_repr_str(cparts[2])
                    )));
                }
            };
            ac.update_coupler_target(c_bearing, c_distance, c_in_range);
        }
    } else if let Some(body) = text.strip_prefix("COUPLER_DIST:") {
        let dparts: Vec<&str> = body.split(',').collect();
        if dparts.len() >= 2 {
            let horiz = float_of(dparts[0])?;
            let height = float_of(dparts[1])?;
            say(
                &coupler_dist_phrase(horiz, height, &units_mode()),
                false,
                true,
                "python",
            );
        }
    } else if let Some(mode) = text.strip_prefix("COUPLER_DIST_MODE:") {
        say(
            &format!(
                "Coupler distance {}",
                if mode == "ON" { "on" } else { "off" }
            ),
            true,
            true,
            "python",
        );
    } else if let Some(reason) = text.strip_prefix("COUPLER_DIST_FAIL:") {
        say(reason, true, true, "python");
    } else if text == "COUPLED_DETECTED:" {
        // The trailer is physically coupled: switch off distance tracking and the scanner.
        // The homing tone is stopped unconditionally: it outlives the scanner toggle, so
        // this is the only thing that reliably ends it.
        let mut parts_disabled: Vec<&str> = Vec::new();
        state().scanner.coupler_run_active = false;
        ac.set_coupler_tracking(false);
        let was_dist = {
            let mut st = state();
            std::mem::replace(&mut st.scanner.coupler_dist_mode, false)
        };
        if was_dist {
            e2_send(SCANNER_CMD_PORT, "COUPLER_DIST", None);
            parts_disabled.push("distance tracking");
        }
        let was_scan = {
            let mut st = state();
            let s = &mut st.scanner;
            if s.scan_mode_active {
                s.scan_mode_active = false;
                s.last_scanner_target_name = String::new();
                s.last_scanner_distance = f64::INFINITY;
                s.last_scanner_approach_deg = 0.0;
                s.last_scanner_bearing = 0.0;
                true
            } else {
                false
            }
        };
        if was_scan {
            e2_send(SCANNER_CMD_PORT, "OFF", None);
            ac.set_scan_mode(false);
            parts_disabled.push("scanner");
        }
        say(&coupled_phrase(&parts_disabled), true, true, "python");
    } else if let Some(mode) = text.strip_prefix("ATTACH_MONITOR:") {
        say(
            &format!("Attach monitor {}", if mode == "ON" { "on" } else { "off" }),
            true,
            true,
            "python",
        );
    } else if let Some(mode) = text.strip_prefix("COUPLER_MODE:") {
        say(
            &format!("Couplers {}", if mode == "ON" { "on" } else { "off" }),
            true,
            true,
            "python",
        );
    } else if text == "ALIGN_OK" {
        say(
            "Aligned to trailer. Reverse straight back and press L to couple.",
            true,
            true,
            "python",
        );
    } else if let Some(reason) = text.strip_prefix("ALIGN_FAIL:") {
        say(&format!("Alignment failed. {reason}"), true, true, "python");
    } else if let Some(payload) = text.strip_prefix("ASSISTANT:") {
        use beam_road::pyjson::PyErr;
        match driving_assistant().receive(payload) {
            Ok(()) => {}
            Err(PyErr::ValueError(m)) => return Err(Raised::Value(m)),
            Err(PyErr::OSError(m)) => return Err(Raised::Os(m)),
            Err(e) => return Err(Raised::Other(format!("{}: {}", e.type_name(), e.message()))),
        }
    } else if let Some(msg) = text.strip_prefix("AI_OK:") {
        log::info!("AI response: {msg}");
    } else if let Some(msg) = text.strip_prefix("AI_ERR:") {
        say(&format!("AI error. {msg}"), true, true, "python");
    } else if let Some(name) = text.strip_prefix("TARGET_NAME:") {
        state().scanner.last_scanner_target_name = name.to_string();
        speech_out::protect_speech();
        say(name, true, true, "python");
    } else if let Some(name) = text.strip_prefix("SWITCHED:") {
        {
            let mut st = state();
            st.scanner.last_scanner_target_name = String::new();
            st.scanner.last_scanner_distance = f64::INFINITY;
            st.last_vehicle_switch_ts = monotonic();
        }
        if speech_out::queue_loading_pending_vehicle(name) {
            log::info!("Queued loading ready vehicle: {}", py_repr_str(name));
        } else {
            speech_out::protect_speech();
            say(name, true, true, "vehicle_switch");
        }
    } else if let Some(payload) = text.strip_prefix("AI_STATUS_ALL:") {
        crate::ai_layer::speak_status_all_response(payload);
    } else if let Some(body) = text.strip_prefix("AI_STATUS:") {
        if let Some(phrase) = ai_status_phrase(body) {
            say(&phrase, true, true, "python");
        }
    } else {
        // "bearing,distance,approachDeg,direction". distance is the surface GAP, which
        // reaches zero on contact, so it is floored. The direction field is the scanner's
        // own resolved reference end and wins over the gear we pushed; it is missing on an
        // older mod, where the pushed gear is still the best guess.
        let parts: Vec<&str> = text.split(',').collect();
        if parts.len() >= 2 {
            let bearing = float_of(parts[0])?;
            let distance = py_max(0.0, float_of(parts[1])?);
            let approach = if parts.len() >= 3 {
                float_of(parts[2])?
            } else {
                0.0
            };
            {
                let mut st = state();
                st.scanner.last_scanner_distance = distance;
                st.scanner.last_scanner_approach_deg = approach;
                st.scanner.last_scanner_bearing = bearing;
                if parts.len() >= 4 {
                    let d = float_of(parts[3])?;
                    if d.is_nan() {
                        return Err(Raised::Value("cannot convert float NaN to integer".into()));
                    }
                    if d.is_infinite() {
                        return Err(Raised::Other(
                            "OverflowError: cannot convert float infinity to integer".into(),
                        ));
                    }
                    st.scanner.scanner_ref_reversed = d.trunc() < 0.0;
                }
            }
            ac.update_scanner_target(bearing, distance);
        }
    }
    Ok(())
}

/// One datagram from 4445: the text protocol, falling back to the old binary `<ff`
/// format when the text does not decode or parse.
#[doc(hidden)]
pub fn handle_scanner_datagram(ac: &AudioController, data: &[u8]) -> Result<(), Raised> {
    let result = match net::decode(data, Decode::AsciiStrict) {
        None => Err(Raised::Value("UnicodeDecodeError".into())),
        Some(t) => handle_scanner_text(ac, py_strip(&t)),
    };
    match result {
        Err(Raised::Value(_)) => {
            if data.len() == 8 {
                let bearing = f32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                let distance = f32::from_le_bytes([data[4], data[5], data[6], data[7]]);
                ac.update_scanner_target(bearing as f64, distance as f64);
            }
            Ok(())
        }
        other => other,
    }
}

/// `scanner_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn scanner_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    // This shared port also carries assistant JSON, including long committed paths. A
    // truncated datagram cannot be reassembled, hence the full-size buffer.
    let mut sock = match UdpListener::bind(SCANNER_LISTEN_PORT, 0.2, 65535) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Vehicle scanner listener could not bind port {SCANNER_LISTEN_PORT}: {e}");
            log::info!("Vehicle scanner listener stopped.");
            return;
        }
    };
    log::info!("Vehicle scanner listener started on port {SCANNER_LISTEN_PORT}");
    let mut first_packet = true;
    while !stop_event.is_set() {
        let outcome = match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(e) => Err(Raised::Os(e.to_string())),
            Recv::Data(data, addr) => {
                if first_packet {
                    log::info!(
                        "First UDP packet received from vehicle scanner (source: {})",
                        py_addr(&addr)
                    );
                    first_packet = false;
                }
                handle_scanner_datagram(&audio_controller, data)
            }
        };
        match outcome {
            Ok(()) | Err(Raised::Value(_)) => {}
            Err(Raised::Os(_)) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Scanner listener socket error.");
            }
            // Departure (python-bugs.md 9, 34): the Python let this end the thread.
            Err(Raised::Other(m)) => log::error!("Scanner listener error: {m}"),
        }
    }
    log::info!("Vehicle scanner listener stopped.");
}

/// `scanner_callout_thread_fn(stop_event)`. Runs until `stop_event` is set.
pub fn scanner_callout_thread_fn(stop_event: &Event) {
    let mut last_callout_ts = 0.0;
    while !stop_event.is_set() {
        stop_event.wait_secs(1.0);
        if stop_event.is_set() {
            break;
        }
        let (enabled, active, interval) = {
            let st = state();
            (
                st.cfg.scanner_distance_callout_enabled,
                st.scanner.scan_mode_active,
                st.cfg.scanner_distance_callout_interval,
            )
        };
        if !enabled || !active {
            continue;
        }
        let now = monotonic();
        if now - last_callout_ts < interval {
            continue;
        }
        let (dist, brg, tname, reversed_ref) = {
            let st = state();
            (
                st.scanner.last_scanner_distance,
                st.scanner.last_scanner_bearing,
                st.scanner.last_scanner_target_name.clone(),
                st.scanner.scanner_ref_reversed,
            )
        };
        if dist == f64::INFINITY {
            continue;
        }
        last_callout_ts = now;
        say(
            &callout_phrase(dist, brg, &tname, reversed_ref, &units_mode()),
            false,
            true,
            "python",
        );
    }
}
