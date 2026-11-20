//! Python-golden tests for E5: status rows, the function catalog and its availability,
//! controller navigation speech, the virtual browser and buffer mode
//! (`rust/fixtures/gen/menus.py`).
//!
//! No real keyboard hook is installed: nothing here starts the command worker or the
//! hotkeys, which is what allows one.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use beam_engine::keyboard::{self, KeyEvent};
use beam_engine::menus::{self, FunctionAction};
use beam_engine::speech_out::{self, get_speech_log};
use beam_engine::state::{OnAdjust, OnEnter, OnEscape, State, state};
use beam_engine::timer::{monotonic, time_time};
use beam_engine::units::set_units_mode;
use beam_engine::virtual_browser::{self, BrowserOpts};
use beam_testutil::{f, load};
use parking_lot::Mutex;
use serde_json::{Value, json};

static SERIAL: Mutex<()> = Mutex::new(());

/// Point the config directory at a scratch folder before anything opens the recorders,
/// as `_beamtel.py` does for the generator: the hill-climb catalog entry must not depend
/// on (or write to) the user's real reports.
fn isolate() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("beam_e5_menus_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: set once, before any test thread reads the environment.
        unsafe { std::env::set_var("LOCALAPPDATA", &dir) };
    });
}

fn opt_f(v: &Value) -> Option<f64> {
    if v.is_null() { None } else { Some(f(v)) }
}

/// Floats written by `deep_fbits` back to plain JSON numbers.
fn unbits(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.contains_key("bits") => json!(f(v)),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), unbits(v))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(unbits).collect()),
        other => other.clone(),
    }
}

/// Rebuild the scenario's beamtel globals on a fresh state.
fn apply(s: &Value) {
    let mut st = State::default();
    set_units_mode(s["units"].as_str().unwrap());
    let now = monotonic();
    for (k, v) in s.as_object().unwrap() {
        let t = &mut st.tel;
        match k.as_str() {
            "units" | "trailer_live" | "wheel_live" | "world_active" | "route_live" => {}
            "protocol_mode" => t.protocol_mode = v.as_str().unwrap().to_string(),
            "last_heading" => t.last_heading = f(v),
            "last_speed_ms" => t.last_speed_ms = f(v),
            "last_rpm" => t.last_rpm = f(v),
            "last_gear_byte" => t.last_gear_byte = v.as_i64(),
            "last_gear_str" => t.last_gear_str = v.as_str().map(str::to_string),
            "last_fuel" => t.last_fuel = f(v),
            "last_ev_range_m" => t.last_ev_range_m = f(v),
            "last_motor_power_kw" => t.last_motor_power_kw = f(v),
            "last_motor_torque_nm" => t.last_motor_torque_nm = f(v),
            "last_engtemp" => t.last_engtemp = f(v),
            "last_oiltemp" => t.last_oiltemp = f(v),
            "last_clutch_temp" => t.last_clutch_temp = f(v),
            "_env_ambient_c" => st.env.env_ambient_c = opt_f(v),
            "last_turbo" => t.last_turbo = f(v),
            "last_protocol_flags" => t.last_protocol_flags = v.as_i64().unwrap(),
            "last_air_pressure" => t.last_air_pressure = f(v),
            "last_air_pressure_max" => t.last_air_pressure_max = f(v),
            "last_trip_m" => t.last_trip_m = f(v),
            "last_odometer_m" => t.last_odometer_m = f(v),
            "last_lifetime_m" => t.last_lifetime_m = f(v),
            "last_telemetry_presence" => t.last_telemetry_presence = v.as_i64().unwrap(),
            "last_implement_flags" => t.last_implement_flags = f(v),
            "last_implement_edge_height" => t.last_implement_edge_height = f(v),
            "last_implement_tilt_deg" => t.last_implement_tilt_deg = f(v),
            "last_articulation_deg" => t.last_articulation_deg = f(v),
            "last_trailer_deg" => st.trailer.last_trailer_deg = f(v),
            "last_trailer_id" => st.trailer.last_trailer_id = v.as_i64(),
            "last_trailer_name" => st.trailer.last_trailer_name = v.as_str().unwrap().to_string(),
            "last_wheel_rows" => {
                st.wheels.last_wheel_rows = v.as_array().unwrap().iter().map(unbits).collect()
            }
            "last_tire_pressure_f" => t.last_tire_pressure_f = f(v),
            "last_tire_pressure_fl" => t.last_tire_pressure_fl = f(v),
            "last_tire_pressure_fr" => t.last_tire_pressure_fr = f(v),
            "last_tire_pressure_r" => t.last_tire_pressure_r = f(v),
            "last_tire_pressure_rl" => t.last_tire_pressure_rl = f(v),
            "last_tire_pressure_rr" => t.last_tire_pressure_rr = f(v),
            "last_brake_temp_f" => t.last_brake_temp_f = f(v),
            "last_brake_temp_fl" => t.last_brake_temp_fl = f(v),
            "last_brake_temp_fr" => t.last_brake_temp_fr = f(v),
            "last_brake_temp_r" => t.last_brake_temp_r = f(v),
            "last_brake_temp_rl" => t.last_brake_temp_rl = f(v),
            "last_brake_temp_rr" => t.last_brake_temp_rr = f(v),
            "scan_mode_active" => st.scanner.scan_mode_active = v.as_bool().unwrap(),
            "dock_mode_active" => st.dock.dock_mode_active = v.as_bool().unwrap(),
            "marked_coord_x" => t.marked_coord_x = opt_f(v),
            "free_cam_active" => st.camera.free_cam_active = v.as_bool().unwrap(),
            "_fuel_target_id" => {
                st.fuel.fuel_target_id = if v.is_null() { None } else { Some(v.clone()) }
            }
            "_fuel_in_range" => st.fuel.fuel_in_range = v.as_bool().unwrap(),
            "_fuel_parking" => st.fuel.fuel_parking = v.as_bool().unwrap(),
            "_ui_screen_context" => st.page.ui_screen_context = v.as_str().unwrap().to_string(),
            "clickspot_mode_active" => st.clickspot.clickspot_mode_active = v.as_bool().unwrap(),
            "clickspot_list_loading" => st.clickspot.clickspot_list_loading = v.as_bool().unwrap(),
            "clickspot_trigger_list" => {
                st.clickspot.clickspot_trigger_list = v
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| {
                        (
                            r[0].as_i64().unwrap(),
                            r[1].as_i64().unwrap(),
                            r[2].as_str().unwrap().to_string(),
                        )
                    })
                    .collect()
            }
            other => panic!("fixture sets an unknown global {other}"),
        }
    }
    st.trailer.last_trailer_stamp = if s["trailer_live"] == json!(true) {
        now
    } else {
        0.0
    };
    st.wheels.last_wheel_stamp = if s["wheel_live"] == json!(true) {
        now
    } else {
        0.0
    };
    st.tel.last_telemetry_ts = if s["world_active"] == json!(true) {
        time_time()
    } else {
        0.0
    };
    if s["route_live"] == json!(true) {
        st.route.route_dest_x = Some(1.0);
        st.route.route_dest_y = Some(2.0);
        st.route.route_beacon_stamp = now;
    }
    *state() = st;
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

/// Every utterance since the last reset, as `[text, interrupt, excluded]`.
fn spoken() -> Vec<Value> {
    get_speech_log(None, Some(0), None, None, false)
        .entries
        .iter()
        .map(|e| json!([e.text, e.interrupt, e.excluded]))
        .collect()
}

fn quiet_speech() {
    speech_out::reset_for_tests();
    speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
}

#[test]
fn status_rows_match_python() {
    isolate();
    let _g = SERIAL.lock();
    let fx = load("menus");
    for case in fx["status"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        apply(&case["state"]);
        let st = state();
        let rows = menus::status_rows(&st);
        let want = case["rows"].as_array().unwrap();
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        let want_labels: Vec<&str> = want.iter().map(|r| r["label"].as_str().unwrap()).collect();
        assert_eq!(labels, want_labels, "{name}: row labels");
        for (r, w) in rows.iter().zip(want) {
            let label = &r.label;
            assert_eq!(
                r.category.as_deref(),
                w["category"].as_str(),
                "{name}/{label}: category"
            );
            let available = (r.is_available)(&st);
            assert_eq!(
                available,
                w["available"].as_bool().unwrap(),
                "{name}/{label}: available"
            );
            if !available {
                continue;
            }
            match catch_unwind(AssertUnwindSafe(|| (r.get_value)(&st))) {
                Ok((value, unit)) => {
                    let text = format!("{value} {unit}").trim().to_string();
                    assert_eq!(
                        Some(text.as_str()),
                        w["value_text"].as_str(),
                        "{name}/{label}"
                    );
                }
                Err(p) => {
                    let err = w["err"].as_str().unwrap_or_else(|| {
                        panic!(
                            "{name}/{label}: Rust raised {} where Python did not",
                            panic_text(&*p)
                        )
                    });
                    assert!(
                        panic_text(&*p).starts_with(err),
                        "{name}/{label}: {}",
                        panic_text(&*p)
                    );
                }
            }
        }
    }
}

#[test]
fn function_catalog_matches_python() {
    isolate();
    let fx = load("menus");
    let items = menus::function_items();
    let want = fx["function_catalog"].as_array().unwrap();
    assert_eq!(items.len(), want.len());
    for (i, w) in items.iter().zip(want) {
        assert_eq!(i.label, w["label"].as_str().unwrap());
        assert_eq!(i.category, w["category"].as_str().unwrap(), "{}", i.label);
        match &i.action {
            FunctionAction::Command(n, c, s, a) => {
                assert_eq!(json!([n, c, s, a]), w["command"], "{}", i.label)
            }
            FunctionAction::Handler(_) => assert_eq!(w["handler"], json!(true), "{}", i.label),
        }
    }
}

#[test]
fn function_availability_matches_python() {
    isolate();
    let _g = SERIAL.lock();
    let fx = load("menus");
    for case in fx["functions"].as_array().unwrap() {
        apply(&case["state"]);
        let st = state();
        let got: Vec<String> = menus::function_items()
            .into_iter()
            .filter(|i| i.label != "Hill-climb reports" && (i.is_available)(&st))
            .map(|i| i.label)
            .collect();
        let want: Vec<String> = case["available"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(got, want, "{}", case["name"]);
    }
}

#[test]
fn navigation_speech_matches_python() {
    isolate();
    let _g = SERIAL.lock();
    let _s = speech_out::TEST_SERIAL.lock();
    let fx = load("menus");
    let commands = Arc::new(Mutex::new(Vec::<Value>::new()));
    let activations = Arc::new(Mutex::new(Vec::<i64>::new()));
    let c2 = Arc::clone(&commands);
    menus::set_f9_invoker_override(Some(Arc::new(move |n: &str, c, s, a| {
        c2.lock().push(json!([n, c, s, a]))
    })));
    let a2 = Arc::clone(&activations);
    menus::set_clickspot_activator_override(Some(Arc::new(move |i| a2.lock().push(i))));
    for case in fx["navigation"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        apply(&case["state"]);
        quiet_speech();
        commands.lock().clear();
        activations.lock().clear();
        for (n, step) in case["steps"].as_array().unwrap().iter().enumerate() {
            let action = step["action"].as_str().unwrap();
            speech_out::reset_for_tests();
            let (c0, a0) = (commands.lock().len(), activations.lock().len());
            let r = catch_unwind(AssertUnwindSafe(|| {
                menus::navigate_accessibility_menu(action)
            }));
            let label = format!("{name} step {n} ({action})");
            assert_eq!(r.is_err(), !step["err"].is_null(), "{label}: raised");
            assert_eq!(Value::Array(spoken()), step["spoken"], "{label}: speech");
            assert_eq!(
                Value::Array(commands.lock()[c0..].to_vec()),
                step["commands"],
                "{label}"
            );
            assert_eq!(
                json!(activations.lock()[a0..]),
                step["activations"],
                "{label}"
            );
            let st = state();
            assert_eq!(
                json!(st.menu.current_accessibility_screen_index),
                step["screen"],
                "{label}"
            );
            assert_eq!(
                json!(st.menu.current_status_metric_index),
                step["status_index"],
                "{label}"
            );
            assert_eq!(
                json!(st.menu.current_functions_item_index),
                step["functions_index"],
                "{label}"
            );
            assert_eq!(
                json!(st.menu.current_clickspot_index),
                step["clickspot_index"],
                "{label}"
            );
        }
    }
    menus::set_f9_invoker_override(None);
    menus::set_clickspot_activator_override(None);
    speech_out::set_speak_override(None);
    speech_out::reset_for_tests();
}

type Calls = Arc<Mutex<Vec<Value>>>;

fn recorders(calls: &Calls) -> (OnEnter, OnAdjust, OnEscape) {
    let c1 = Arc::clone(calls);
    let c2 = Arc::clone(calls);
    let c3 = Arc::clone(calls);
    (
        Arc::new(move |i, line: &str, d: Option<Value>| {
            c1.lock().push(json!(["enter", i, line, d]))
        }),
        Arc::new(move |i, line: &str, d: Option<Value>, delta| {
            c2.lock().push(json!(["adjust", i, line, d, delta]))
        }),
        Arc::new(move || c3.lock().push(json!(["escape"]))),
    )
}

#[test]
fn virtual_browser_matches_python() {
    isolate();
    let _g = SERIAL.lock();
    let _s = speech_out::TEST_SERIAL.lock();
    let fx = load("menus");
    let cases = fx["vbrowser"].as_array().unwrap();
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let (on_enter, on_adjust, on_escape) = recorders(&calls);
    let lines = || vec!["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()];
    let nav = |n: &str| keyboard_nav(n);
    *state() = State::default();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        quiet_speech();
        calls.lock().clear();
        match name {
            "read_only" => {
                virtual_browser::open_virtual_browser(lines(), BrowserOpts::titled("List"));
                for n in ["up", "down", "down", "down", "up", "left"] {
                    nav(n);
                }
                for a in [
                    "status_repeat",
                    "activate",
                    "status_down",
                    "status_up",
                    "next_menu",
                ] {
                    virtual_browser::drive_virtual_browser(a);
                }
            }
            "with_enter" => {
                virtual_browser::open_virtual_browser(
                    lines(),
                    BrowserOpts {
                        on_enter: Some(Arc::clone(&on_enter)),
                        entry_data: vec![json!(10), json!({"k": "v"})],
                        start_index: 7,
                        announce_interrupt: false,
                        ..Default::default()
                    },
                );
                virtual_browser::drive_virtual_browser("activate");
                nav("up");
                virtual_browser::drive_virtual_browser("activate");
                nav("up");
                press("enter");
                press("escape");
            }
            "with_adjust_escape" => {
                virtual_browser::open_virtual_browser(
                    lines(),
                    BrowserOpts {
                        title: Some(String::new()),
                        on_adjust: Some(Arc::clone(&on_adjust)),
                        on_escape: Some(Arc::clone(&on_escape)),
                        start_index: -4,
                        ..Default::default()
                    },
                );
                press("right");
                press("left");
                virtual_browser::drive_virtual_browser("previous_menu");
                virtual_browser::close_virtual_browser(true);
            }
            "empty" => {
                virtual_browser::open_virtual_browser(Vec::new(), BrowserOpts::titled("Nothing"));
                virtual_browser::drive_virtual_browser("status_repeat");
            }
            "closed_nav" => {
                virtual_browser::close_virtual_browser(false);
                nav("down");
                press("enter");
            }
            other => panic!("unknown vbrowser case {other}"),
        }
        assert_eq!(Value::Array(spoken()), case["spoken"], "{name}: speech");
        assert_eq!(
            Value::Array(calls.lock().clone()),
            case["callbacks"],
            "{name}: callbacks"
        );
        let st = state();
        assert_eq!(json!(st.vbrowser.vbrowser_active), case["active"], "{name}");
        assert_eq!(json!(st.vbrowser.vbrowser_index), case["index"], "{name}");
        assert_eq!(json!(st.vbrowser.vbrowser_lines), case["lines"], "{name}");
        assert!(
            st.vbrowser.vbrowser_hooks.is_empty(),
            "{name}: hooks released"
        );
    }
    speech_out::set_speak_override(None);
    speech_out::reset_for_tests();
}

/// A browser key as the hook delivers it: through the suppressed hook's queued handler.
/// The browser's own handlers are crate-private, so drive them the way a key press does.
fn press(key: &str) {
    let hooked = !state().vbrowser.vbrowser_hooks.is_empty();
    if !hooked {
        // `_on_vbrowser_enter(None)` with no browser: nothing happens.
        return;
    }
    keyboard::deliver_for_tests(key, &KeyEvent::down(key));
    keyboard::run_pending_commands_for_tests();
}

fn keyboard_nav(n: &str) {
    if state().vbrowser.vbrowser_hooks.is_empty() {
        // `_on_vbrowser_nav` with no lines returns at once; so does "left" with no adjust.
        return;
    }
    keyboard::deliver_for_tests(n, &KeyEvent::down(n));
    keyboard::run_pending_commands_for_tests();
}

#[test]
fn buffer_mode_matches_python() {
    isolate();
    let _g = SERIAL.lock();
    let _s = speech_out::TEST_SERIAL.lock();
    let fx = load("menus");
    *state() = State::default();
    for case in fx["buffer"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        quiet_speech();
        for line in case["buffer"].as_array().unwrap() {
            speech_out::say(line.as_str().unwrap(), true, false, "python");
        }
        let _ = speech_out::get_speech_log(None, Some(0), None, None, false);
        // Only what the mode says from here on.
        let base = get_speech_log(None, Some(0), None, None, false).next_seq;
        state().menu.buffer_mode_active = false;
        virtual_browser::toggle_buffer_mode();
        let start = state().menu.current_buffer_index;
        for k in case["keys"].as_array().unwrap() {
            virtual_browser::on_buffer_nav_press(&KeyEvent::down(k.as_str().unwrap()));
        }
        let end = state().menu.current_buffer_index;
        virtual_browser::toggle_buffer_mode();
        virtual_browser::on_buffer_nav_press(&KeyEvent::down("["));
        let said: Vec<Value> = get_speech_log(Some(base as i64), Some(0), None, None, false)
            .entries
            .iter()
            .map(|e| json!([e.text, e.interrupt, e.excluded]))
            .collect();
        assert_eq!(Value::Array(said), case["spoken"], "{name}: speech");
        assert_eq!(json!(start), case["start_index"], "{name}");
        assert_eq!(json!(end), case["end_index"], "{name}");
        assert!(
            state().menu.buffer_key_hooks.is_empty(),
            "{name}: hooks released"
        );
    }
    speech_out::set_speak_override(None);
    speech_out::reset_for_tests();
}

#[test]
fn unknown_and_handler_items_do_not_reach_the_f9_layer() {
    isolate();
    // The handler items carry a callable, never a key; Terrain scan's is the driving-only
    // scan itself.
    let items = menus::function_items();
    let terrain = items.iter().find(|i| i.label == "Terrain scan").unwrap();
    match &terrain.action {
        FunctionAction::Handler(h) => assert!(Arc::ptr_eq(h, &menus::terrain_scan_handler())),
        FunctionAction::Command(..) => panic!("Terrain scan must be a direct handler"),
    }
}
