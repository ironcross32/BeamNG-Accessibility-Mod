//! The E2 listeners (scanner, obstacle, road, fuel, UI toasts, hill climb) against
//! `rust/fixtures/data/e2_listeners.json`, which `rust/fixtures/gen/e2_listeners.py`
//! writes by running beamtel.py's own listeners over the same datagrams.
//!
//! Speech is compared through the speech tap (text, source, interrupt after the
//! protection window, excluded, spoken), commands through the E2 send seam, and audio
//! through the real `AudioController`'s state.

use std::sync::{Arc, Once};

use beam_audio::controller::{AudioController, Hazard};
use beam_engine::listeners::{fuel, hill_climb, obstacle, road, scanner, ui_toast};
use beam_engine::state::{State, road_guidance_feed, state};
use beam_engine::timer::{monotonic, time_time};
use beam_engine::{convert, speech_out, units};
use beam_testutil::{f, load};
use parking_lot::Mutex;
use serde_json::{Value, json};

static SENT: Mutex<Vec<(u16, String)>> = Mutex::new(Vec::new());
static ENV: Once = Once::new();

/// Serialise, keep every recorder out of the user's real config directory, silence the
/// speech backend and capture every command.
fn setup() -> parking_lot::MutexGuard<'static, ()> {
    let guard = speech_out::TEST_SERIAL.lock();
    ENV.call_once(|| {
        let dir = std::env::temp_dir().join(format!("beam_e2_tests_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: inside `Once`, before any test in this binary has touched the state
        // statics (every test calls `setup` first and the TEST_SERIAL guard is held), so
        // no other thread of ours reads the environment concurrently.
        unsafe { std::env::set_var("LOCALAPPDATA", &dir) };
    });
    speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
    scanner::set_send_hook_for_tests(Some(Box::new(|port: u16, payload: &[u8]| {
        SENT.lock()
            .push((port, String::from_utf8_lossy(payload).into_owned()));
    })));
    reset();
    guard
}

fn reset() {
    speech_out::reset_for_tests();
    SENT.lock().clear();
    *state() = State::default();
    units::set_units_mode("imperial");
}

fn tap() -> Value {
    let log = speech_out::get_speech_log(None, Some(0), None, None, false);
    Value::Array(
        log.entries
            .iter()
            .map(|e| {
                json!({"text": e.text, "source": e.source, "interrupt": e.interrupt,
                       "excluded": e.excluded, "spoken": e.spoken})
            })
            .collect(),
    )
}

fn sent() -> Value {
    Value::Array(SENT.lock().iter().map(|(p, s)| json!([p, s])).collect())
}

/// A float as the fixtures carry it (the `repr` is dropped by [`norm`]).
fn fb(x: f64) -> Value {
    json!({"bits": if x.is_nan() { f64::NAN.to_bits() } else { x.to_bits() }})
}

/// Fixture JSON with every `{"bits", "repr"}` reduced to `{"bits"}` and NaNs made canonical.
fn norm(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.contains_key("bits") => fb(f(v)),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), norm(v))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        other => other.clone(),
    }
}

/// Rust-built JSON with plain floats turned into `{"bits"}` (ints stay ints).
fn bits_json(v: &Value) -> Value {
    match v {
        Value::Number(n) if n.is_f64() => fb(n.as_f64().unwrap_or(f64::NAN)),
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, v)| (k.clone(), bits_json(v))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(bits_json).collect()),
        other => other.clone(),
    }
}

/// Fixture JSON with `{"bits"}` floats turned back into numbers.
fn unbits(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.contains_key("bits") => convert::json_float(f(v)),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), unbits(v))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(unbits).collect()),
        other => other.clone(),
    }
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("fixture hex"))
        .collect()
}

fn name(c: &Value) -> String {
    c.get("name")
        .map(|n| n.to_string())
        .unwrap_or_else(|| c.to_string())
}

fn scanner_state(st: &State) -> Value {
    let s = &st.scanner;
    json!({
        "last_scanner_target_name": s.last_scanner_target_name,
        "last_scanner_distance": fb(s.last_scanner_distance),
        "last_scanner_approach_deg": fb(s.last_scanner_approach_deg),
        "last_scanner_bearing": fb(s.last_scanner_bearing),
        "scanner_ref_reversed": s.scanner_ref_reversed,
        "scan_mode_active": s.scan_mode_active,
        "coupler_dist_mode": s.coupler_dist_mode,
        "coupler_run_active": s.coupler_run_active,
    })
}

fn scanner_audio(ac: &AudioController) -> Value {
    let s = ac.lock.lock();
    json!({
        "_scanner_target_bearing": fb(s._scanner_target_bearing),
        "_scanner_target_distance": fb(s._scanner_target_distance),
        "_scan_mode_active": s._scan_mode_active,
        "_coupler_active": s._coupler_active,
        "_coupler_bearing": fb(s._coupler_bearing),
        "_coupler_distance": fb(s._coupler_distance),
        "_coupler_in_range": s._coupler_in_range,
    })
}

fn hazard_json(h: &Option<Hazard>) -> Value {
    match h {
        None => Value::Null,
        Some(h) => json!({
            "bearing": fb(h.bearing), "closing": fb(h.closing), "gap": fb(h.gap),
            "legacy": h.legacy, "state": h.state, "stopping_margin": fb(h.stopping_margin),
            "ttc": fb(h.ttc), "urgency": h.urgency,
        }),
    }
}

// ---- Scanner -------------------------------------------------------------------------------

#[test]
fn scanner_listener_matches_python() {
    let _g = setup();
    let fx = load("e2_listeners");
    for c in fx["scanner"].as_array().unwrap() {
        reset();
        let label = name(c);
        units::set_units_mode(c["units"].as_str().unwrap());
        speech_out::set_loading_for_tests(c["loading"].as_bool().unwrap(), false);
        {
            let mut st = state();
            let init = &c["init"];
            st.scanner.scan_mode_active = init["scan_mode_active"].as_bool().unwrap_or(false);
            st.scanner.coupler_dist_mode = init["coupler_dist_mode"].as_bool().unwrap_or(false);
            st.scanner.coupler_run_active = init["coupler_run_active"].as_bool().unwrap_or(false);
            st.scanner.scanner_ref_reversed =
                init["scanner_ref_reversed"].as_bool().unwrap_or(false);
        }
        let ac = AudioController::new();
        let mut died = Value::Null;
        for d in c["datagrams"].as_array().unwrap() {
            if let Err(scanner::Raised::Other(m)) =
                scanner::handle_scanner_datagram(&ac, &hex(d.as_str().unwrap()))
            {
                died = Value::from(m);
                break;
            }
        }
        assert_eq!(died, c["died"], "{label}: died");
        assert_eq!(tap(), c["tap"], "{label}: speech");
        assert_eq!(sent(), c["sent"], "{label}: sent");
        assert_eq!(scanner_state(&state()), norm(&c["state"]), "{label}: state");
        assert_eq!(scanner_audio(&ac), norm(&c["audio"]), "{label}: audio");
    }
}

#[test]
fn callout_phrases_match_python() {
    let _g = setup();
    let fx = load("e2_listeners");
    for c in fx["callouts"].as_array().unwrap() {
        let dist = f(&c["dist"]);
        let tap = c["tap"].as_array().unwrap();
        if dist == f64::INFINITY {
            assert!(tap.is_empty(), "{c}");
            continue;
        }
        let got = scanner::callout_phrase(
            dist,
            f(&c["brg"]),
            c["tname"].as_str().unwrap(),
            c["reversed"].as_bool().unwrap(),
            c["units"].as_str().unwrap(),
        );
        assert_eq!(tap.len(), 1, "{c}");
        assert_eq!(got, tap[0]["text"].as_str().unwrap(), "{c}");
        assert_eq!(tap[0]["interrupt"], json!(false));
        assert_eq!(tap[0]["excluded"], json!(true));
        assert_eq!(tap[0]["source"], json!("python"));
    }
}

#[test]
fn callout_thread_speaks_once_per_interval() {
    let _g = setup();
    {
        let mut st = state();
        st.cfg.scanner_distance_callout_enabled = true;
        st.cfg.scanner_distance_callout_interval = 10.0;
        st.scanner.scan_mode_active = true;
        st.scanner.last_scanner_distance = 12.4;
        st.scanner.last_scanner_bearing = 100.0;
        st.scanner.last_scanner_target_name = "Pessima".into();
    }
    let stop = Arc::new(beam_engine::timer::Event::new());
    let s2 = Arc::clone(&stop);
    let t = std::thread::spawn(move || scanner::scanner_callout_thread_fn(&s2));
    std::thread::sleep(std::time::Duration::from_millis(2300));
    stop.set();
    t.join().unwrap();
    let tap = tap();
    assert_eq!(tap.as_array().unwrap().len(), 1, "{tap}");
    assert_eq!(
        tap[0]["text"],
        json!("Pessima, 41 feet, to the left of you")
    );
}

#[test]
fn toggle_scan_mode_matches_python() {
    let _g = setup();
    let fx = load("e2_listeners");
    for c in fx["toggles"].as_array().unwrap() {
        reset();
        {
            let mut st = state();
            st.dock.implement_word_current = c["word"].as_str().unwrap().to_string();
            st.tel.last_implement_flags = f(&c["flags"]);
        }
        let ac = AudioController::new();
        for step in c["steps"].as_array().unwrap() {
            scanner::toggle_scan_mode(Arc::clone(&ac));
            assert_eq!(scanner_state(&state()), norm(&step["state"]), "{c}");
            assert_eq!(scanner_audio(&ac), norm(&step["audio"]), "{c}");
        }
        assert_eq!(tap(), c["tap"], "{c}");
        assert_eq!(sent(), c["sent"], "{c}");
    }
    // A coupling run outlives the scanner toggle.
    reset();
    let ac = AudioController::new();
    ac.set_coupler_tracking(true);
    {
        let mut st = state();
        st.scanner.coupler_run_active = true;
        st.scanner.scan_mode_active = true;
    }
    scanner::toggle_scan_mode(Arc::clone(&ac));
    let kept = &fx["toggle_run_kept"];
    assert_eq!(
        json!(ac.lock.lock()._coupler_active),
        kept["coupler_active"]
    );
    assert_eq!(tap(), kept["tap"]);
    assert_eq!(sent(), kept["sent"]);
}

#[test]
fn gear_push_matches_python() {
    let _g = setup();
    let fx = load("e2_listeners");
    for c in fx["gear_pushes"].as_array().unwrap() {
        reset();
        {
            let mut st = state();
            scanner::push_gear_direction(&mut st, c["gear"].as_str().unwrap());
        }
        assert_eq!(
            json!(state().scanner.scanner_ref_reversed),
            c["reversed"],
            "{c}"
        );
        assert_eq!(sent(), c["sent"], "{c}");
    }
}

// ---- Obstacles -----------------------------------------------------------------------------

fn parse_json(text: &str) -> Value {
    match obstacle::parse_obstacle_packet(text) {
        Err(e) => json!({"err": e.message()}),
        Ok(obstacle::ObstaclePacket::Clear) => json!({"kind": "clear", "payload": null}),
        Ok(obstacle::ObstaclePacket::Terrain(t, b, u, d)) => {
            json!({"kind": "terrain", "payload": [t, fb(b), u, fb(d)]})
        }
        Ok(obstacle::ObstaclePacket::Static(h)) => json!({"kind": "static", "payload": {
            "bearing": fb(h.bearing), "urgency": h.urgency, "gap": fb(h.gap),
            "state": h.state, "closing": fb(h.closing), "ttc": fb(h.ttc),
            "stopping_margin": fb(h.stopping_margin), "legacy": h.legacy,
        }}),
    }
}

#[test]
fn parse_obstacle_packet_matches_python() {
    let fx = load("e2_listeners");
    for c in fx["obstacle_parse"].as_array().unwrap() {
        let text = c["text"].as_str().unwrap();
        assert_eq!(
            parse_json(text),
            norm(&c["out"]),
            "parse_obstacle_packet({text:?})"
        );
    }
}

#[test]
fn obstacle_listener_matches_python() {
    let _g = setup();
    let fx = load("e2_listeners");
    for c in fx["obstacle"].as_array().unwrap() {
        reset();
        let ac = AudioController::new();
        let mut ls = obstacle::ObstacleListenerState::default();
        for d in c["datagrams"].as_array().unwrap() {
            let raw = hex(d.as_str().unwrap());
            let text =
                beam_engine::net::decode(&raw, beam_engine::net::Decode::Utf8Ignore).unwrap();
            let text = beam_base::pyfmt::py_strip(&text);
            if text.is_empty() {
                continue;
            }
            obstacle::handle_obstacle_text(&ac, &mut ls, text);
        }
        let label = name(c);
        assert_eq!(tap(), c["tap"], "{label}: speech");
        let s = ac.lock.lock();
        let audio = json!({
            "_obstacle_emergency_latched": s._obstacle_emergency_latched,
            "_obstacle_hazard": hazard_json(&s._obstacle_hazard),
            "_terrain_type": s._terrain_type,
        });
        assert_eq!(audio, norm(&c["audio"]), "{label}: audio");
    }
}

#[test]
fn push_obstacle_state_matches_python() {
    let _g = setup();
    let fx = load("e2_listeners");
    for c in fx["pushes"].as_array().unwrap() {
        reset();
        let result = {
            let mut st = state();
            st.obstacle.obstacle_mode_active = true;
            st.tel.protocol_mode = c["proto"].as_str().unwrap().to_string();
            st.tel.last_gear_str = c["gear_str"].as_str().map(str::to_string);
            st.tel.last_gear_byte = c["gear_byte"].as_i64();
            st.tel.last_steering_input = f(&c["steer"]);
            st.tel.last_throttle = f(&c["thr"]);
            st.tel.last_brake = f(&c["brk"]);
            st.obstacle.obstacle_state_last = c["last"].as_array().map(|l| {
                (
                    l[0].as_str().unwrap().to_string(),
                    f(&l[1]),
                    f(&l[2]),
                    f(&l[3]),
                )
            });
            st.obstacle.obstacle_state_last_sent = if c["recent"].as_bool().unwrap() {
                monotonic() + 1000.0
            } else {
                0.0
            };
            obstacle::push_obstacle_state(
                &mut st,
                c["force"].as_bool().unwrap(),
                c["gear"].as_str(),
            )
        };
        assert_eq!(json!(result), c["result"], "{c}");
        assert_eq!(sent(), c["sent"], "{c}");
        let after = state()
            .obstacle
            .obstacle_state_last
            .clone()
            .map(|(d, a, b, cc)| json!([d, fb(a), fb(b), fb(cc)]))
            .unwrap_or(Value::Null);
        assert_eq!(after, norm(&c["after"]), "{c}");
    }
    reset();
    let off = obstacle::push_obstacle_state(&mut state(), true, None);
    assert_eq!(json!(off), fx["pushes_off"]);
    assert_eq!(sent(), json!([]));
}

#[test]
fn toggle_obstacle_mode_matches_python() {
    let _g = setup();
    let fx = load("e2_listeners");
    for c in fx["obstacle_toggles"].as_array().unwrap() {
        reset();
        state().cfg.obstacle_warning_sensitivity = c["sensitivity"].as_str().unwrap().to_string();
        let ac = AudioController::new();
        obstacle::toggle_obstacle_mode(Arc::clone(&ac));
        let on_audio = ac.lock.lock()._obstacle_mode_active;
        obstacle::toggle_obstacle_mode(Arc::clone(&ac));
        assert_eq!(tap(), c["tap"], "{c}");
        assert_eq!(sent(), c["sent"], "{c}");
        assert_eq!(json!(on_audio), c["on_audio"], "{c}");
        assert_eq!(
            json!(ac.lock.lock()._obstacle_mode_active),
            c["off_audio"],
            "{c}"
        );
        assert!(state().obstacle.obstacle_state_last.is_none());
    }
}

// ---- Road ----------------------------------------------------------------------------------

#[test]
fn road_listener_matches_python() {
    let _g = setup();
    let fx = load("e2_listeners");
    for c in fx["road"].as_array().unwrap() {
        reset();
        let label = name(c);
        units::set_units_mode(c["units"].as_str().unwrap());
        let flags = &c["flags"];
        let flag = |k: &str| flags[k].as_bool().unwrap();
        {
            let mut st = state();
            st.road_mode_active = flag("road_mode_active");
            st.cfg.road_follow_guidance_enabled = flag("road_follow_guidance_enabled");
            st.cfg.road_junction_speech_enabled = flag("road_junction_speech_enabled");
            st.cfg.road_stop_sign_speech_enabled = flag("road_stop_sign_speech_enabled");
            st.cfg.road_traffic_light_speech_enabled = flag("road_traffic_light_speech_enabled");
            st.cfg.road_speed_limit_speech_enabled = flag("road_speed_limit_speech_enabled");
            st.cfg.road_surface_speech_enabled = flag("road_surface_speech_enabled");
            st.cfg.road_junction_earcon_enabled = flag("road_junction_earcon_enabled");
        }
        road_guidance_feed().reset();
        speech_out::set_loading_for_tests(c["loading"].as_bool().unwrap(), false);
        let ac = AudioController::new();
        ac.set_road_mode(flag("road_mode_active"));
        let mut ls = road::RoadListenerState::default();
        let mut died = Value::Null;
        for t in c["texts"].as_array().unwrap() {
            let text = beam_base::pyfmt::py_strip(t.as_str().unwrap());
            if text.is_empty() {
                continue;
            }
            if let Err(road::RoadRaised::Other(m)) = road::handle_road_text(&ac, &mut ls, text) {
                died = Value::from(m);
                break;
            }
        }
        if died.is_null() {
            // The final empty read is a timeout, as the Python's fake socket ends with one.
            road::road_timeout(&ac);
        }
        assert_eq!(died, c["died"], "{label}: died");
        assert_eq!(tap(), c["tap"], "{label}: speech");
        assert_eq!(sent(), c["sent"], "{label}: sent");
        assert_eq!(
            json!({"_road_on_road": ac.lock.lock()._road_on_road}),
            c["audio"],
            "{label}: audio"
        );
        assert_eq!(
            json!(road_guidance_feed().snapshot().mode),
            c["feed_mode"],
            "{label}: feed mode"
        );
    }
    road_guidance_feed().reset();
}

#[test]
fn road_status_and_toggle_match_python() {
    let _g = setup();
    let fx = load("e2_listeners");
    for c in fx["road_status"].as_array().unwrap() {
        reset();
        road_guidance_feed().reset();
        state().road_mode_active = c["road_mode_active"].as_bool().unwrap();
        road::speak_road_status();
        assert_eq!(tap(), c["tap"], "{c}");
    }
    for c in fx["road_toggles"].as_array().unwrap() {
        reset();
        state().cfg.road_include_private = c["include_private"].as_bool().unwrap();
        let ac = AudioController::new();
        road::toggle_road_mode(Arc::clone(&ac));
        let on_audio = ac.lock.lock()._road_mode_active;
        road::toggle_road_mode(Arc::clone(&ac));
        assert_eq!(tap(), c["tap"], "{c}");
        assert_eq!(sent(), c["sent"], "{c}");
        assert_eq!(json!(on_audio), c["on_audio"], "{c}");
        assert_eq!(
            json!(ac.lock.lock()._road_mode_active),
            c["off_audio"],
            "{c}"
        );
    }
    road_guidance_feed().reset();
}

#[test]
fn road_diagnostic_control_refusals_match_python() {
    let _g = setup();
    assert_eq!(
        road::road_diagnostic_control("bogus", None, None, None, 20),
        Err("action must be start, stop, status, mark, review, or list".to_string())
    );
    assert_eq!(
        road::road_diagnostic_control("  START ", None, None, None, 20),
        Err("no live world; start the recording once driving telemetry is active".to_string())
    );
    state().tel.last_telemetry_ts = time_time() + 1000.0;
    assert_eq!(
        road::road_diagnostic_control("start", None, None, None, 20),
        Err("road detection is off; enable it before starting the recording".to_string())
    );
    state().road_mode_active = true;
    state().cfg.road_follow_guidance_enabled = false;
    assert_eq!(
        road::road_diagnostic_control("start", None, None, None, 20),
        Err("road-follow guidance is disabled in configuration".to_string())
    );
    let status = road::road_diagnostic_control("", None, None, None, 20).unwrap();
    assert_eq!(status["active"], json!(false));
    assert_eq!(status["road_detection_active"], json!(true));
    assert_eq!(status["road_guidance_enabled"], json!(false));
    assert_eq!(
        road::road_diagnostic_control("mark", None, None, Some("  "), 20),
        Err("marker note is required".to_string())
    );
    assert_eq!(
        road::road_diagnostic_control("stop", None, None, None, 20),
        Err("no road diagnostic recording is active".to_string())
    );
    assert_eq!(sent(), json!([[4463, "DIAG_OFF"]]));
    state().tel.last_telemetry_ts = 0.0;
    assert_eq!(
        road::assistant_diagnostic_control("start", &json!({})),
        Err("no live world; start recording once driving telemetry is active".to_string())
    );
    let st = road::assistant_diagnostic_control("", &json!({})).unwrap();
    assert_eq!(st["recording"], json!(false));
    assert_eq!(st["assistant"]["active"], json!(false));
    assert_eq!(
        road::assistant_diagnostic_control("nope", &json!({})),
        Err("action must be start, stop, status, read, mark, review, or list".to_string())
    );
}

// ---- Fuel ----------------------------------------------------------------------------------

fn fuel_state_json() -> Value {
    let st = state();
    let fu = &st.fuel;
    json!({
        "pending": fu.fuel_pending,
        "target_id": fu.fuel_target_id.clone().unwrap_or(Value::Null),
        "target_name_str": fu.fuel_target_name,
        "in_range": fu.fuel_in_range,
        "parking": fu.fuel_parking,
        "rows": bits_json(&Value::Array(fu.fuel_rows.clone())),
        "building": fu.fuel_building.clone().map(|b| bits_json(&Value::Array(b))).unwrap_or(Value::Null),
    })
}

fn cancel_fuel_timer() {
    let t = state().fuel.fuel_timer.take();
    if let Some(t) = t {
        t.cancel();
    }
}

#[test]
fn fuel_stations_match_python() {
    let _g = setup();
    let fx = load("e2_listeners");
    for c in fx["fuel"].as_array().unwrap() {
        reset();
        let label = name(c);
        units::set_units_mode(c["units"].as_str().unwrap());
        state().tel.last_telemetry_ts = if c["world"].as_bool().unwrap() {
            time_time() + 1000.0
        } else {
            0.0
        };
        let mut last_browser: Option<fuel::FuelBrowser> = None;
        for (i, step) in c["steps"].as_array().unwrap().iter().enumerate() {
            speech_out::reset_for_tests();
            SENT.lock().clear();
            let mut browsers: Vec<Value> = Vec::new();
            match step["kind"].as_str().unwrap() {
                "key" => fuel::open_fuel_stations(step["arg"].as_bool().unwrap()),
                "timeout" => fuel::fuel_timed_out(),
                "choose" => {
                    let b = last_browser.as_ref().expect("a browser to choose from");
                    let row = b.data[step["arg"].as_u64().unwrap() as usize].clone();
                    fuel::fuel_choose(Some(row));
                }
                "recv" => {
                    let text = beam_base::pyfmt::py_strip(step["arg"].as_str().unwrap());
                    // The browser `_fuel_open_browser` would open from the rows FS_END lands.
                    if text == "FS_END" {
                        let st = state();
                        if let Some(rows) = &st.fuel.fuel_building
                            && st.fuel.fuel_pending
                        {
                            let b = fuel::fuel_browser(
                                rows,
                                st.fuel.fuel_target_id.as_ref(),
                                st.fuel.fuel_target_name.as_deref(),
                                &units::units_mode(),
                            );
                            if !b.lines.is_empty() {
                                browsers.push(json!({
                                    "lines": b.lines, "title": b.title,
                                    "data": bits_json(&Value::Array(b.data.clone())),
                                }));
                                last_browser = Some(b);
                            }
                        }
                    }
                    if !text.is_empty() {
                        fuel::handle_fuel_text(text, &|job: Box<dyn FnOnce() + Send>| job());
                    }
                }
                other => panic!("unknown step {other}"),
            }
            cancel_fuel_timer();
            let where_ = format!("{label} step {i} ({})", step["arg"]);
            // The generator stubs the browser out; the real one announces its title.
            let mut spoken = tap();
            if let Some(title) = browsers.first().and_then(|b| b["title"].as_str()) {
                spoken
                    .as_array_mut()
                    .unwrap()
                    .retain(|e| !e["text"].as_str().unwrap_or("").starts_with(title));
            }
            assert_eq!(spoken, step["tap"], "{where_}: speech");
            assert_eq!(sent(), step["sent"], "{where_}: sent");
            assert_eq!(
                Value::Array(browsers),
                norm(&step["browsers"]),
                "{where_}: browser"
            );
            let want = &step["state"];
            let got = fuel_state_json();
            for key in [
                "pending",
                "target_id",
                "target_name_str",
                "in_range",
                "parking",
            ] {
                assert_eq!(got[key], want[key], "{where_}: {key}");
            }
            assert_eq!(got["rows"], norm(&want["rows"]), "{where_}: rows");
            assert_eq!(
                got["building"],
                norm(&want["building"]),
                "{where_}: building"
            );
        }
    }
}

#[test]
fn fuel_rows_round_trip() {
    let row = beam_reports::fuel_stations::parse_row(
        r#"FS:{"id":"a","name":"N","energy":["diesel"],"distance":3,"bearing":370,"pumps":2,"compatible":false,"target":true}"#,
    )
    .unwrap();
    assert_eq!(fuel::row_from_json(&fuel::row_to_json(&row)), row);
}

// ---- UI toasts -----------------------------------------------------------------------------

#[test]
fn ui_toasts_match_python() {
    let _g = setup();
    let fx = load("e2_listeners");
    for c in fx["ui"].as_array().unwrap() {
        reset();
        for d in c["datagrams"].as_array().unwrap() {
            ui_toast::handle_ui_datagram(&hex(d.as_str().unwrap()));
        }
        assert_eq!(c["died"], Value::Null);
        assert_eq!(tap(), c["tap"], "{c}");
    }
}

// ---- Hill climb ----------------------------------------------------------------------------

#[test]
fn hill_climb_glue_matches_python() {
    let fx = load("e2_listeners");
    for c in fx["hill_quality"].as_array().unwrap() {
        let summary = convert::json_to_pyjson(&unbits(&c["summary"]));
        let got = match hill_climb::data_quality_line(&summary) {
            Ok(line) => json!({"line": line}),
            Err(e) => json!({"err": e.type_name()}),
        };
        assert_eq!(got, c["out"], "{c}");
    }
    for c in fx["hill_titles"].as_array().unwrap() {
        assert_eq!(
            hill_climb::history_title(c["n"].as_u64().unwrap() as usize),
            c["title"].as_str().unwrap()
        );
    }
}
