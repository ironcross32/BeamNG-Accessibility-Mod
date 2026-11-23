//! Python-golden tests for E6, the F9 command handler (`fixtures/gen/f9_commands.py`).
//!
//! Every key and modifier combination is driven through `invoke_f9_command`,
//! `on_next_key_press` or `mcp_press_command`, with every call the handler makes outside
//! itself replaced by a recorder (`f9_commands::set_external_override`), as the generator
//! replaces beamtel's functions. Speech is compared through the speech tap, calls with their
//! arguments and whether the command context was set, and the globals the handler changes.
//! No keyboard hook is installed and nothing reaches the network.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use beam_audio::controller::AudioController;
use beam_engine::f9_commands::{
    self, F9_HELP, invoke_f9_command, mcp_press_command, on_next_key_press,
};
use beam_engine::keyboard::{KeyEvent, KeyEventType};
use beam_engine::speech_out::{self, command_context, get_speech_log, speech_log_path};
use beam_engine::state::{
    CannonAim, DockReading, Mods, RampSelf, State, set_audio_controller_ref, state,
};
use beam_engine::timer::monotonic;
use beam_engine::units::{set_units_mode, units_mode};
use beam_testutil::{f, load};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};

static SERIAL: Mutex<()> = Mutex::new(());

fn isolate() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("beam_e6_f9_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: set once, before anything in this test binary reads the environment.
        unsafe { std::env::set_var("LOCALAPPDATA", &dir) };
    });
}

fn opt_f(v: &Value) -> Option<f64> {
    if v.is_null() { None } else { Some(f(v)) }
}

fn opt_s(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

fn dock_from(v: &Value) -> Option<DockReading> {
    if v.is_null() {
        return None;
    }
    Some(DockReading {
        name: v["name"].as_str().unwrap().to_string(),
        range: 0.0,
        lateral: 0.0,
        vertical: f(&v["vertical"]),
        idx: v["idx"].as_i64().unwrap(),
        count: v["count"].as_i64().unwrap(),
        kind: v["kind"].as_str().unwrap().to_string(),
        lo_z: 0.0,
        hi_z: 0.0,
        yaw: 0.0,
        manual: false,
        impl_word: String::new(),
        entry_theta: None,
        entry_depth: None,
        mode: v["mode"].as_str().unwrap().to_string(),
        margin: None,
        acquire: false,
    })
}

/// Set one beamtel global, by its Python name, on a state being built.
fn set(st: &mut State, key: &str, v: &Value) {
    let t = &mut st.tel;
    match key {
        "UNITS_MODE" => set_units_mode(v.as_str().unwrap()),
        "speech_logging_active" => speech_out::set_speech_logging_active(v.as_bool().unwrap()),
        "protocol_mode" => t.protocol_mode = v.as_str().unwrap().to_string(),
        "last_speed_ms" => t.last_speed_ms = f(v),
        "last_rpm" => t.last_rpm = f(v),
        "last_fuel" => t.last_fuel = f(v),
        "last_turbo" => t.last_turbo = f(v),
        "last_engtemp" => t.last_engtemp = f(v),
        "last_oiltemp" => t.last_oiltemp = f(v),
        "last_gear_byte" => t.last_gear_byte = v.as_i64(),
        "last_gear_str" => t.last_gear_str = opt_s(v),
        "last_pos_x" => t.last_pos_x = f(v),
        "last_pos_y" => t.last_pos_y = f(v),
        "last_pos_z" => t.last_pos_z = f(v),
        "last_roll_rad" => t.last_roll_rad = f(v),
        "last_pitch_rad" => t.last_pitch_rad = f(v),
        "last_heading" => t.last_heading = f(v),
        "last_rpm_max" => t.last_rpm_max = f(v),
        "last_turbo_max" => t.last_turbo_max = f(v),
        "last_air_pressure" => t.last_air_pressure = f(v),
        "last_air_pressure_max" => t.last_air_pressure_max = f(v),
        "last_telemetry_presence" => t.last_telemetry_presence = v.as_i64().unwrap(),
        "heading_guidance_active" => t.heading_guidance_active = v.as_bool().unwrap(),
        "heading_guidance_target" => t.heading_guidance_target = f(v),
        "drift_mode_active" => t.drift_mode_active = v.as_bool().unwrap(),
        "drift_alert_active" => t.drift_alert_active = v.as_bool().unwrap(),
        "drift_baseline_heading" => t.drift_baseline_heading = f(v),
        "last_drift_check_ts" => t.last_drift_check_ts = f(v),
        "low_speed_mode_active" => t.low_speed_mode_active = v.as_bool().unwrap(),
        "slip_mode_active" => t.slip_mode_active = v.as_bool().unwrap(),
        "coord_guidance_active" => t.coord_guidance_active = v.as_bool().unwrap(),
        "pedal_tones_active" => t.pedal_tones_active = v.as_bool().unwrap(),
        "marked_coord_x" => t.marked_coord_x = opt_f(v),
        "marked_coord_y" => t.marked_coord_y = opt_f(v),
        "_last_coord_bearing_ts" => t.last_coord_bearing_ts = f(v),
        "cam_yaw_deg" => st.camera.cam_yaw_deg = f(v),
        "cam_pitch_deg" => st.camera.cam_pitch_deg = f(v),
        "cam_agl" => st.camera.cam_agl = f(v),
        "cam_agl_valid" => st.camera.cam_agl_valid = v.as_bool().unwrap(),
        "cam_veh_bearing" => st.camera.cam_veh_bearing = f(v),
        "cam_veh_distance" => st.camera.cam_veh_distance = f(v),
        "scan_mode_active" => st.scanner.scan_mode_active = v.as_bool().unwrap(),
        "coupler_dist_mode" => st.scanner.coupler_dist_mode = v.as_bool().unwrap(),
        "last_scanner_bearing" => st.scanner.last_scanner_bearing = f(v),
        "last_scanner_distance" => st.scanner.last_scanner_distance = f(v),
        "last_scanner_target_name" => {
            st.scanner.last_scanner_target_name = v.as_str().unwrap().to_string()
        }
        "last_scanner_approach_deg" => st.scanner.last_scanner_approach_deg = f(v),
        "road_mode_active" => st.road_mode_active = v.as_bool().unwrap(),
        "dock_mode_active" => st.dock.dock_mode_active = v.as_bool().unwrap(),
        "cannon_active" => st.dock.cannon_active = v.as_bool().unwrap(),
        "cannon_kind" => st.dock.cannon_kind = v.as_str().unwrap().to_string(),
        "last_dock" => st.dock.last_dock = dock_from(v),
        "last_dock_fail" => st.dock.last_dock_fail = opt_s(v),
        "last_dock_mode" => st.dock.last_dock_mode = opt_s(v),
        "last_dock_name" => st.dock.last_dock_name = opt_s(v),
        "last_ramp_self" => {
            st.dock.last_ramp_self = if v.is_null() {
                None
            } else {
                Some(RampSelf {
                    pitch: opt_f(&v["pitch"]),
                    lip: opt_f(&v["lip"]),
                    groups: Vec::new(),
                })
            }
        }
        "last_cannon_aim" => {
            st.dock.last_cannon_aim = if v.is_null() {
                None
            } else {
                Some(CannonAim {
                    elevation: f(&v["elevation"]),
                    bearing: f(&v["bearing"]),
                    target_angle: f(&v["target_angle"]),
                    line_angle: f(&v["line_angle"]),
                    range: f(&v["range"]),
                    speed: f(&v["speed"]),
                    reachable: v["reachable"].as_bool().unwrap(),
                    solution_available: v["solution_available"].as_bool().unwrap(),
                    stamp: monotonic() - f(&v["stamp_age"]),
                })
            }
        }
        "_implement_word_current" => {
            st.dock.implement_word_current = v.as_str().unwrap().to_string()
        }
        other => panic!("fixture sets an unknown global {other}"),
    }
}

fn bits(x: f64) -> Value {
    json!({ "bits": x.to_bits() })
}

fn opt_bits(x: Option<f64>) -> Value {
    x.map(bits).unwrap_or(Value::Null)
}

/// The fixture's `{"bits", "repr"}` floats reduced to their bits, everywhere.
fn norm(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.contains_key("bits") => json!({ "bits": m["bits"] }),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), norm(v))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        other => other.clone(),
    }
}

/// The generator's `view()`.
fn view() -> Map<String, Value> {
    let st = state();
    let t = &st.tel;
    let m = st.kb.capture_mods;
    let mut out = Map::new();
    let mut put = |k: &str, v: Value| {
        out.insert(k.to_string(), v);
    };
    put("UNITS_MODE", json!(units_mode()));
    put("heading_guidance_active", json!(t.heading_guidance_active));
    put("heading_guidance_target", bits(t.heading_guidance_target));
    put("coupler_dist_mode", json!(st.scanner.coupler_dist_mode));
    put("drift_mode_active", json!(t.drift_mode_active));
    put("drift_alert_active", json!(t.drift_alert_active));
    put("drift_baseline_heading", bits(t.drift_baseline_heading));
    put("last_drift_check_ts", json!(t.last_drift_check_ts > 0.0));
    put("low_speed_mode_active", json!(t.low_speed_mode_active));
    put("slip_mode_active", json!(t.slip_mode_active));
    put("coord_guidance_active", json!(t.coord_guidance_active));
    put("pedal_tones_active", json!(t.pedal_tones_active));
    put("marked_coord_x", opt_bits(t.marked_coord_x));
    put("marked_coord_y", opt_bits(t.marked_coord_y));
    put("_last_coord_bearing_ts", bits(t.last_coord_bearing_ts));
    put("dock_mode_active", json!(st.dock.dock_mode_active));
    put(
        "last_dock",
        st.dock
            .last_dock
            .as_ref()
            .map(|d| json!([d.name, d.mode]))
            .unwrap_or(Value::Null),
    );
    put("last_dock_fail", json!(st.dock.last_dock_fail));
    put("last_dock_mode", json!(st.dock.last_dock_mode));
    put("last_dock_name", json!(st.dock.last_dock_name));
    put(
        "speech_logging_active",
        json!(speech_out::speech_logging_active()),
    );
    put("_input_help_mode", json!(st.kb.input_help_mode));
    put("_capture_mods", json!([m.ctrl, m.shift, m.alt]));
    put("_command_context", json!(command_context()));
    out
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

fn mods3(v: &Value) -> (bool, bool, bool) {
    match v.as_array() {
        Some(a) => (
            a[0].as_bool().unwrap(),
            a[1].as_bool().unwrap(),
            a[2].as_bool().unwrap(),
        ),
        None => (false, false, false),
    }
}

#[test]
fn help_table_matches_python() {
    let fx = load("f9_commands");
    let want = fx["help"].as_array().unwrap();
    let got: Vec<Value> = F9_HELP
        .iter()
        .map(|((k, c, s, a), d)| json!([k, c, s, a, d]))
        .collect();
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(want) {
        assert_eq!(g, w);
    }
}

#[test]
fn every_key_and_modifier_matches_python() {
    isolate();
    let _g = SERIAL.lock();
    let _s = speech_out::TEST_SERIAL.lock();
    let fx = load("f9_commands");
    let base = fx["base"].as_object().unwrap();
    let defaults = fx["default_returns"].as_object().unwrap().clone();
    let view_keys: Vec<&str> = fx["view_keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();

    let calls: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let returns: Arc<Mutex<Map<String, Value>>> = Arc::new(Mutex::new(Map::new()));
    {
        let calls = Arc::clone(&calls);
        let returns = Arc::clone(&returns);
        f9_commands::set_external_override(Some(Arc::new(move |name: &str, args: &Value| {
            calls.lock().push(json!([name, args, command_context()]));
            returns.lock().get(name).cloned().unwrap_or(Value::Null)
        })));
    }
    let ac = AudioController::new();
    let log_path = speech_log_path().display().to_string();
    let mut failures = Vec::new();

    for case in fx["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();

        // State: Python's import-time values, then the scenario, then the case.
        speech_out::reset_for_tests();
        speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
        for line in case["buffer"].as_array().into_iter().flatten() {
            speech_out::say(line.as_str().unwrap(), true, false, "python");
        }
        let base_seq = get_speech_log(None, Some(0), None, None, false).next_seq;
        let mut st = State::default();
        let scen = &fx["scenarios"][case["scenario"].as_str().unwrap_or("idle")];
        for layer in [Some(base), scen.as_object(), case["state"].as_object()]
            .into_iter()
            .flatten()
        {
            for (k, v) in layer {
                set(&mut st, k, v);
            }
        }
        let (pc, ps, pa) = mods3(&case["preset_mods"]);
        st.kb.capture_mods = Mods {
            ctrl: pc,
            shift: ps,
            alt: pa,
        };
        st.kb.input_help_mode = case["help_mode"].as_bool().unwrap_or(false);
        st.kb.next_key_timer = None;
        *state() = st;
        speech_out::set_command_context(case["preset_ctx"].as_bool().unwrap_or(false));
        let no_ac = case["no_ac"].as_bool().unwrap_or(false);
        set_audio_controller_ref(if no_ac { None } else { Some(Arc::clone(&ac)) });
        {
            let mut r = returns.lock();
            *r = defaults.clone();
            if let Some(o) = case["returns"].as_object() {
                for (k, v) in o {
                    r.insert(k.clone(), v.clone());
                }
            }
        }
        calls.lock().clear();
        let before = view();

        let key = case["key"].as_str().unwrap();
        let (c, s, a) = mods3(&case["mods"]);
        let entry = case["entry"].as_str().unwrap_or("invoke");
        let run = catch_unwind(AssertUnwindSafe(|| match entry {
            "invoke" => invoke_f9_command(key, c, s, a),
            "mcp" => mcp_press_command(key, c, s, a),
            _ => {
                let event_type = if case["event_type"].as_str() == Some("up") {
                    KeyEventType::Up
                } else {
                    KeyEventType::Down
                };
                on_next_key_press(
                    &KeyEvent {
                        name: key.to_string(),
                        event_type,
                    },
                    Some(Arc::clone(&ac)),
                );
                Ok(true)
            }
        }));
        let err: Value = match run {
            Ok(Ok(true)) => Value::Null,
            Ok(Ok(false)) => json!("returned False"),
            Ok(Err(_)) => json!("RuntimeError"),
            Err(p) => {
                let text = panic_text(&*p);
                json!(text.split(':').next().unwrap_or_default())
            }
        };
        let after = view();

        let spoken: Vec<Value> = get_speech_log(Some(base_seq as i64), Some(0), None, None, false)
            .entries
            .iter()
            .map(|e| {
                json!([
                    e.text.replace(&log_path, "<SPEECH_LOG_PATH>"),
                    e.interrupt,
                    e.excluded
                ])
            })
            .collect();
        let mut changes = Map::new();
        for k in &view_keys {
            if after[*k] != before[*k] {
                changes.insert(k.to_string(), after[*k].clone());
            }
        }

        let mut check = |what: &str, got: Value, want: Value| {
            if got != want {
                failures.push(format!("{name}: {what}\n  rust:   {got}\n  python: {want}"));
            }
        };
        // python-bugs.md 56, 57 (departure): a non-finite reading is spoken as unknown
        // where the Python raised (or read "100 percent"). These cases are pinned to
        // the Rust's own answer rather than the Python's.
        let departed: Option<(&str, Value)> = match name {
            "r/nan" | "invoke/restores_after_raise" => Some(("RPM unknown", json!(false))),
            "f/nan" => Some(("Fuel level unknown", json!(false))),
            "speed/nan" => Some(("Speed unknown. Speed limit unknown", json!(false))),
            "i/cannon/nan" => Some(("Inclination unknown, strength 100%", json!(true))),
            "e/nan" => Some(("Engine temperature unknown", json!(false))),
            _ => None,
        };
        if let Some((text, excluded)) = departed {
            check("err", err, Value::Null);
            check(
                "spoken",
                Value::Array(spoken),
                json!([[text, true, excluded]]),
            );
            continue;
        }
        check("err", err, case["err"].clone());
        check("spoken", Value::Array(spoken), case["spoken"].clone());
        check(
            "calls",
            Value::Array(calls.lock().clone()),
            norm(&case["calls"]),
        );
        check("changes", Value::Object(changes), norm(&case["changes"]));
    }

    f9_commands::set_external_override(None);
    set_audio_controller_ref(None);
    speech_out::set_speak_override(None);
    speech_out::reset_for_tests();
    *state() = State::default();
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        fx["cases"].as_array().unwrap().len(),
        failures
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
