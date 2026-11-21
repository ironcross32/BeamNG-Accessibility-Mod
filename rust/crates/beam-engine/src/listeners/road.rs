//! Owner: E2. Road detector listener, road commands, diagnostics control, road status and toggle (`beamtel.py:1258-1477, 7038-7122`).

use std::sync::Arc;

use beam_audio::controller::{AudioController, JsonMap};
use beam_base::pyfmt::{py_float_parse, py_strip};
use beam_road::pyjson::{PyErr, Value as PyJson};
use beam_road::road_guidance::{
    junction_phrase, map_speed_limit_phrase, one_way_phrase, parse_r2_packet, parse_surface_packet,
    signal_phrase, surface_phrase,
};
use serde_json::Value;

use crate::convert::{
    assistant_snapshot_json, json_to_pyjson, junction_proximity_snapshot_json, pyjson_to_json,
};
use crate::listeners::scanner::{e2_send, py_addr};
use crate::net::{Decode, ROAD_CMD_PORT, ROAD_LISTEN_PORT, Recv, UdpListener, decode};
use crate::speech_out::{loading_or_settling, say};
use crate::state::{
    assistant_diagnostics, driving_assistant, hill_climb_challenge,
    road_diagnostic_telemetry_snapshot, road_diagnostics, road_guidance_feed, state,
    world_is_active,
};
use crate::timer::Event;
use crate::units::units_mode;

/// A beam-road dict as the map the audio controller takes.
fn json_map(v: &PyJson) -> Option<JsonMap> {
    match pyjson_to_json(v) {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// How a Python exception leaves the per-datagram body.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub enum RoadRaised {
    /// `OSError`: the outer handler logs a socket error.
    Os(String),
    /// Anything else the listener does not catch: the thread dies.
    Other(String),
}

fn raised(e: PyErr) -> RoadRaised {
    match e {
        PyErr::OSError(m) => RoadRaised::Os(m),
        other => RoadRaised::Other(format!("{}: {}", other.type_name(), other.message())),
    }
}

/// The listener's state between datagrams.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct RoadListenerState {
    /// "DORMANT" / "ON_ROAD" / "OFF_ROAD"
    pub last_state: Option<String>,
    pub last_r2_state: Option<String>,
    pub last_gate_id: Option<String>,
    pub last_private_entry_id: Option<String>,
}

/// The settings the listener reads, copied out under one guard.
struct RoadFlags {
    road_mode_active: bool,
    follow: bool,
    junction_speech: bool,
    stop_sign: bool,
    traffic_light: bool,
    speed_limit: bool,
    junction_earcon: bool,
}

fn road_flags() -> RoadFlags {
    let st = state();
    RoadFlags {
        road_mode_active: st.road_mode_active,
        follow: st.cfg.road_follow_guidance_enabled,
        junction_speech: st.cfg.road_junction_speech_enabled,
        stop_sign: st.cfg.road_stop_sign_speech_enabled,
        traffic_light: st.cfg.road_traffic_light_speech_enabled,
        speed_limit: st.cfg.road_speed_limit_speech_enabled,
        junction_earcon: st.cfg.road_junction_earcon_enabled,
    }
}

fn r2_packet(
    ac: &AudioController,
    ls: &mut RoadListenerState,
    text: &str,
) -> Result<(), RoadRaised> {
    let packet = match parse_r2_packet(text) {
        Ok(p) => p,
        Err(PyErr::ValueError(exc)) => {
            log::warn!("Ignored malformed road R2 packet: {exc}");
            return Ok(());
        }
        Err(e) => return Err(raised(e)),
    };
    let packet_value = packet.to_value();

    let telemetry_snapshot = json_to_pyjson(&road_diagnostic_telemetry_snapshot());
    hill_climb_challenge()
        .record(&packet_value, &telemetry_snapshot)
        .map_err(raised)?;
    let controls_ready = !loading_or_settling();
    let f = road_flags();
    let assistant_active = driving_assistant().active();
    let awareness_active = controls_ready && (f.road_mode_active || assistant_active);
    let events = road_guidance_feed().accept_r2(
        &packet,
        None,
        awareness_active && f.stop_sign,
        awareness_active && f.traffic_light,
        awareness_active && f.speed_limit,
    );
    let off_road = packet
        .off_road
        .as_ref()
        .and_then(|o| json_map(&o.to_value()));
    let correction = packet
        .correction
        .as_ref()
        .and_then(|c| json_map(&c.to_value()));
    let audio_event = ac.update_road_guidance(
        &packet.state,
        off_road.as_ref(),
        correction.as_ref(),
        f.road_mode_active && f.follow,
    );
    let target = packet.proximity.as_ref();
    let target_map = target.and_then(|t| json_map(&t.to_value()));
    ac.update_junction_proximity(target_map.as_ref(), awareness_active);
    // The assistant drives private roads and through gates, so both get a word. A "gate"
    // is a gate mesh on the carriageway and gets a stop bar; a private road is the map
    // keeping traffic off, almost never with anything across it, so no bar and no "stop".
    // See docs/road-privacy-and-gates.md.
    let announce = awareness_active && driving_assistant().active() && target.is_some();
    if let Some(t) = target
        && announce
        && t.kind == "gate"
        && ls.last_gate_id.as_deref() != Some(t.id.as_str())
    {
        say("Gate ahead. Stop at the bar", false, false, "road_guidance");
    }
    let private_entry = target.map(|t| t.private_entry.clone()).unwrap_or_default();
    if announce
        && !private_entry.is_empty()
        && ls.last_private_entry_id.as_deref() != Some(private_entry.as_str())
    {
        say("Private road ahead", false, false, "road_guidance");
    }
    if let Some(t) = target
        && t.kind == "gate"
    {
        ls.last_gate_id = Some(t.id.clone());
    }
    if !private_entry.is_empty() {
        ls.last_private_entry_id = Some(private_entry);
    }
    let proximity = json_to_pyjson(&junction_proximity_snapshot_json(
        &ac.junction_proximity_snapshot(),
    ));
    assistant_diagnostics().record_road(&packet, proximity);
    let audio_event = beam_road::pyjson::dict([(
        "settled_triggered",
        PyJson::Bool(audio_event.settled_triggered),
    )]);
    road_diagnostics()
        .record(&packet_value, &telemetry_snapshot, Some(&audio_event))
        .map_err(raised)?;

    if f.road_mode_active
        && packet.state == "dormant"
        && ls.last_r2_state.as_deref() != Some("dormant")
    {
        say("No roads detected on this map", true, true, "road_guidance");
    }
    ls.last_r2_state = Some(packet.state.clone());

    if f.road_mode_active && events.orientation && !packet.road_directions.is_empty() {
        let mut directions = packet.road_directions.clone();
        directions.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
        if packet.one_way {
            ac.trigger_road_orientation_chime(directions[0], None);
            say(&one_way_phrase(directions[0]), false, true, "road_guidance");
        } else {
            let second = directions.get(1).copied();
            ac.trigger_road_orientation_chime(directions[0], second);
        }
    }

    let units = units_mode();
    let junction = if driving_assistant().active() {
        None
    } else {
        events.junction.clone()
    };
    let signal = events.signal.as_ref();
    if let Some(sig) = signal
        && awareness_active
    {
        let mut phrase = signal_phrase(&sig.signal, &units, sig.change);
        if let Some(j) = &junction
            && j.phase == "approach"
            && f.junction_speech
        {
            phrase += " ";
            phrase += &junction_phrase(j, &units)
                .unwrap_or_else(|e| panic!("ValueError: {}", e.message()));
        }
        say(&phrase, true, false, "traffic_control");
    }

    if let Some(j) = &junction
        && f.road_mode_active
    {
        if j.phase == "approach" && f.junction_speech && signal.is_none() {
            say(
                &junction_phrase(j, &units)
                    .unwrap_or_else(|e| panic!("ValueError: {}", e.message())),
                false,
                false,
                "road_guidance",
            );
        } else if j.phase == "near" && f.junction_earcon && !ac.junction_proximity_enabled() {
            ac.trigger_road_junction_earcon();
        } else if j.phase == "entered" && f.junction_earcon {
            ac.trigger_road_junction_entry_earcon();
        }
    }
    if let Some(limit) = &events.speed_limit {
        say(
            &map_speed_limit_phrase(limit.value, &units, "Map speed limit"),
            false,
            false,
            "road_speed_limit",
        );
    }
    Ok(())
}

/// One decoded, stripped, non-empty datagram from 4462.
#[doc(hidden)]
pub fn handle_road_text(
    ac: &AudioController,
    ls: &mut RoadListenerState,
    text: &str,
) -> Result<(), RoadRaised> {
    if text.starts_with("SURFACE|") {
        let surface = parse_surface_packet(text);
        let controls_ready = !loading_or_settling();
        let enabled = state().cfg.road_surface_speech_enabled;
        if let Some((word, on_road)) = surface
            && controls_ready
            && enabled
        {
            say(
                &surface_phrase(&word, on_road),
                false,
                false,
                "road_surface",
            );
        }
        return Ok(());
    }

    if text.starts_with("R2|") {
        return r2_packet(ac, ls, text);
    }

    let feed = road_guidance_feed();
    if text == "DORMANT" {
        let use_legacy = feed.accept_legacy("DORMANT", 0.0, 0.0, &[], None);
        if use_legacy {
            ac.update_road_state(true, 0.0, 0.0);
        }
        if use_legacy && ls.last_state.as_deref() != Some("DORMANT") {
            say("No roads detected on this map", true, true, "python");
            ls.last_state = Some("DORMANT".to_string());
        }
    } else if text.starts_with("ON_ROAD") {
        let parts: Vec<&str> = text.split(',').collect();
        let mut directions: Vec<f64> = Vec::new();
        if parts.len() >= 3
            && let (Some(b_first), Some(b_second)) =
                (py_float_parse(parts[1]), py_float_parse(parts[2]))
        {
            directions = vec![b_first, b_second];
        }
        let use_legacy = feed.accept_legacy("ON_ROAD", 0.0, 0.0, &directions, None);
        if use_legacy {
            ac.update_road_state(true, 0.0, 0.0);
            if !directions.is_empty() {
                ac.trigger_road_orientation_chime(directions[0], Some(directions[1]));
            }
            ls.last_state = Some("ON_ROAD".to_string());
        }
    } else if text.starts_with("OFF_ROAD") {
        let parts: Vec<&str> = text.split(',').collect();
        if parts.len() >= 3
            && let (Some(bearing), Some(distance)) =
                (py_float_parse(parts[1]), py_float_parse(parts[2]))
        {
            let use_legacy = feed.accept_legacy("OFF_ROAD", bearing, distance, &[], None);
            if use_legacy {
                ac.update_road_state(false, bearing, distance);
                ls.last_state = Some("OFF_ROAD".to_string());
            }
        }
    }
    Ok(())
}

/// The read timeout: a newly stale R2 feed silences the road audio.
#[doc(hidden)]
pub fn road_timeout(ac: &AudioController) {
    if road_guidance_feed().check_timeout(None) {
        ac.clear_road_audio();
        log::warn!("Road R2 feed timed out; road audio stopped.");
    }
}

/// `road_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
///
/// Consumes R2 road awareness packets with transparent legacy fallback.
pub fn road_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let mut sock = match UdpListener::bind(ROAD_LISTEN_PORT, 0.2, 65535) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Road detector listener could not bind port {ROAD_LISTEN_PORT}: {e}");
            log::info!("Road detector listener stopped.");
            return;
        }
    };
    log::info!("Road detector listener started on port {ROAD_LISTEN_PORT}");
    let mut first_packet = true;
    let mut ls = RoadListenerState::default();
    while !stop_event.is_set() {
        let outcome = match sock.recv() {
            Recv::Timeout => {
                road_timeout(&audio_controller);
                continue;
            }
            Recv::Error(e) => Err(RoadRaised::Os(e.to_string())),
            Recv::Data(data, addr) => {
                if first_packet {
                    log::info!(
                        "First UDP packet received from road detector (source: {})",
                        py_addr(&addr)
                    );
                    first_packet = false;
                }
                let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                let text = py_strip(&text);
                if text.is_empty() {
                    continue;
                }
                handle_road_text(&audio_controller, &mut ls, text)
            }
        };
        match outcome {
            Ok(()) => {}
            Err(RoadRaised::Os(_)) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Road listener socket error.");
            }
            // Departure (python-bugs.md 6, 37): the Python let this end the thread.
            Err(RoadRaised::Other(m)) => log::error!("Road listener error: {m}"),
        }
    }
    log::info!("Road detector listener stopped.");
}

/// `_send_road_command(command)` (port ROAD_CMD_PORT).
pub fn send_road_command(command: &str) -> bool {
    e2_send(ROAD_CMD_PORT, command, Some("road detector command"))
}

/// `_send_road_configuration()`: `PRIVATE,<0|1>`.
pub fn send_road_configuration() {
    let include = state().cfg.road_include_private;
    send_road_command(&format!("PRIVATE,{}", if include { 1 } else { 0 }));
}

fn kw_str<'a>(kwargs: &'a Value, key: &str) -> Option<&'a str> {
    kwargs.get(key).and_then(Value::as_str)
}

/// `str(action or "status").strip().lower()`.
fn normalise_action(action: &str) -> String {
    let a = if action.is_empty() { "status" } else { action };
    py_strip(a).to_lowercase()
}

/// `assistant_diagnostic_control(action="status", **kwargs)` (MCP). `Err` is the RuntimeError text.
pub fn assistant_diagnostic_control(
    action: &str,
    kwargs: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let action = normalise_action(action);
    if action == "start" && !world_is_active() {
        return Err("no live world; start recording once driving telemetry is active".to_string());
    }
    let get = |key: &str, default: PyJson| -> PyJson {
        match kwargs.get(key) {
            Some(v) => json_to_pyjson(v),
            None => default,
        }
    };
    let result = assistant_diagnostics()
        .control(
            &action,
            kw_str(kwargs, "label"),
            kw_str(kwargs, "session"),
            &get("note", PyJson::Null),
            &get("limit", PyJson::Int(100)),
            &get("since_seq", PyJson::Int(0)),
        )
        .map_err(|e| e.message().to_string())?;
    if action == "start" || action == "stop" {
        crate::ai_layer::send_ai_command(if action == "start" {
            "ASSISTANT_DIAG_ON"
        } else {
            "ASSISTANT_DIAG_OFF"
        });
    }
    let mut result = pyjson_to_json(&result);
    if let Value::Object(m) = &mut result {
        m.insert(
            "assistant".into(),
            assistant_snapshot_json(&driving_assistant().snapshot()),
        );
    }
    Ok(result)
}

/// `road_diagnostic_control(action, label, session, note, limit=20)` (MCP). `Err` is the RuntimeError/ValueError text.
///
/// The MCP-facing lifecycle for a loss-resistant road-guidance recording.
pub fn road_diagnostic_control(
    action: &str,
    label: Option<&str>,
    session: Option<&str>,
    note: Option<&str>,
    limit: i64,
) -> Result<serde_json::Value, String> {
    let action = normalise_action(action);
    let err = |e: PyErr| e.message().to_string();
    let (road_mode_active, follow) = {
        let st = state();
        (st.road_mode_active, st.cfg.road_follow_guidance_enabled)
    };
    let diag = road_diagnostics();
    match action.as_str() {
        "start" => {
            if !world_is_active() {
                return Err(
                    "no live world; start the recording once driving telemetry is active"
                        .to_string(),
                );
            }
            if !road_mode_active {
                return Err(
                    "road detection is off; enable it before starting the recording".to_string(),
                );
            }
            if !follow {
                return Err("road-follow guidance is disabled in configuration".to_string());
            }
            let label = label.filter(|l| !l.is_empty()).unwrap_or("hill-climb");
            let mut result = pyjson_to_json(&diag.start(Some(label)).map_err(err)?);
            let sent = send_road_command("DIAG_ON");
            if let Value::Object(m) = &mut result {
                if !sent {
                    m.insert(
                        "warning".into(),
                        Value::from(
                            "recording started, but Lua diagnostic mode could not be requested",
                        ),
                    );
                }
                let st = state();
                m.insert(
                    "road_detection_active".into(),
                    Value::Bool(st.road_mode_active),
                );
                m.insert(
                    "road_guidance_enabled".into(),
                    Value::Bool(st.cfg.road_follow_guidance_enabled),
                );
            }
            Ok(result)
        }
        "stop" => {
            send_road_command("DIAG_OFF");
            Ok(pyjson_to_json(&diag.stop().map_err(err)?))
        }
        "mark" => Ok(pyjson_to_json(&diag.mark(note.unwrap_or("")).map_err(err)?)),
        "review" => Ok(pyjson_to_json(
            &diag
                .review(session.filter(|s| !s.is_empty()))
                .map_err(err)?,
        )),
        "list" => {
            let sessions = diag.list_sessions(&PyJson::Int(limit)).map_err(err)?;
            Ok(serde_json::json!({ "sessions": pyjson_to_json(&sessions) }))
        }
        "status" => {
            let mut result = pyjson_to_json(&diag.status());
            if let Value::Object(m) = &mut result {
                m.insert(
                    "road_detection_active".into(),
                    Value::Bool(road_mode_active),
                );
                m.insert("road_guidance_enabled".into(), Value::Bool(follow));
            }
            Ok(result)
        }
        _ => Err("action must be start, stop, status, mark, review, or list".to_string()),
    }
}

/// `speak_road_status()`.
pub fn speak_road_status() {
    let road_mode_active = state().road_mode_active;
    let enabled = road_mode_active || driving_assistant().active();
    let phrase = road_guidance_feed()
        .status_phrase(enabled, &units_mode())
        .unwrap_or_else(|e| panic!("ValueError: {}", e.message()));
    say(&phrase, true, true, "road_guidance");
}

/// `toggle_road_mode(audio_controller)`.
pub fn toggle_road_mode(audio_controller: Arc<AudioController>) {
    let active = {
        let mut st = state();
        st.road_mode_active = !st.road_mode_active;
        st.road_mode_active
    };

    send_road_command(if active { "ON" } else { "OFF" });
    send_road_configuration();

    if !driving_assistant().active() {
        road_guidance_feed().reset();
    } else if active {
        road_guidance_feed().arm_orientation();
    }
    audio_controller.set_road_mode(active);

    say(
        &format!("Road detection {}", if active { "on" } else { "off" }),
        true,
        true,
        "python",
    );
}
