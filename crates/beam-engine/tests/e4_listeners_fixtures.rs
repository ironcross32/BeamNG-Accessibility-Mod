//! The E4 listeners and browsers against `fixtures/data/e4_listeners.json`, which
//! `fixtures/gen/e4_listeners.py` writes by running beamtel.py's own listeners and
//! browsers over the same datagrams and key presses.
//!
//! Each case is a list of steps (datagrams through a listener, a call, a browser key).
//! After every step the speech tap, the commands sent, the virtual browser and the area's
//! state are compared with what the Python recorded. This is also the glue half of
//! `audio_debug_row_sim.py` (the "audio debug" cases); its pure half is
//! `beam-reports/tests/audio_debug_row_sim.rs`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Once};

use beam_audio::controller::AudioController;
use beam_base::pyfmt::py_strip;
use beam_engine::listeners::{
    audio_debug, binding_learn, camera, clickspot, environment, nodegrab, slots as slot_listener,
    vehicle_bindings, vehicle_info,
};
use beam_engine::net::{Decode, VEHICLE_INFO_CMD_PORT, decode};
use beam_engine::state::{
    EnvRow, NODEGRAB_MODE_ACTIVE, NODEGRAB_SPACE_HELD, PAGE_TEXT_EVENT, Slots, State, VINFO_EVENT,
    slots, state,
};
use beam_engine::timer::{monotonic, time_time};
use beam_engine::{browsers, console, speech_out, units, virtual_browser};
use beam_testutil::{f, load};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use std::sync::atomic::Ordering;

static SENT: Mutex<Vec<(u16, String)>> = Mutex::new(Vec::new());
static WARPS: Mutex<Vec<[i64; 4]>> = Mutex::new(Vec::new());
static BROADCASTS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static VINFO_REPLIES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static PAGE_REPLIES: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static ENV: Once = Once::new();

fn setup() -> parking_lot::MutexGuard<'static, ()> {
    let guard = speech_out::TEST_SERIAL.lock();
    ENV.call_once(|| {
        let dir = std::env::temp_dir().join(format!("beam_e4_tests_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: inside `Once`, before any test in this binary has touched the state
        // statics (every test calls `setup` first and holds TEST_SERIAL).
        unsafe { std::env::set_var("LOCALAPPDATA", &dir) };
    });
    speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
    beam_engine::listeners::scanner::set_send_hook_for_tests(Some(Box::new(
        |port: u16, payload: &[u8]| {
            SENT.lock()
                .push((port, String::from_utf8_lossy(payload).into_owned()));
            if port == VINFO_PORT {
                let replies = VINFO_REPLIES.lock().clone();
                for r in replies {
                    let text = decode(r.as_bytes(), Decode::Utf8Ignore).unwrap_or_default();
                    let text = py_strip(&text);
                    if !text.is_empty() {
                        vehicle_info::handle_vehicle_info_text(text);
                    }
                }
            }
        },
    )));
    nodegrab::set_warp_hook_for_tests(Some(Box::new(|x, y, w, h| {
        WARPS.lock().push([x, y, w, h]);
    })));
    browsers::set_broadcast_hook_for_tests(Some(Box::new(|msg: &Value| {
        BROADCASTS.lock().push(msg.clone());
        let replies = PAGE_REPLIES.lock().clone();
        for r in replies {
            let lines = r["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap().to_string())
                .collect();
            browsers::on_page_text(
                lines,
                r["title"].as_str().unwrap(),
                r["code"].as_str().unwrap(),
                r["sentence"].as_str().unwrap(),
            );
        }
    })));
    reset();
    guard
}

const VINFO_PORT: u16 = VEHICLE_INFO_CMD_PORT;

fn cancel_timers() {
    browsers::aud_cancel_timer();
    let t = state().env.env_change_timer.take();
    if let Some(t) = t {
        t.cancel();
    }
}

fn reset() {
    virtual_browser::close_virtual_browser(false);
    cancel_timers();
    speech_out::reset_for_tests();
    clear_step();
    *state() = State::default();
    state().tel.compass_min_interval = 0.0;
    *slots() = Slots::default();
    NODEGRAB_MODE_ACTIVE.store(false, Ordering::SeqCst);
    NODEGRAB_SPACE_HELD.store(false, Ordering::SeqCst);
    VINFO_EVENT.clear();
    PAGE_TEXT_EVENT.clear();
    VINFO_REPLIES.lock().clear();
    PAGE_REPLIES.lock().clear();
    units::set_units_mode("imperial");
}

fn clear_step() {
    speech_out::reset_for_tests();
    SENT.lock().clear();
    WARPS.lock().clear();
    BROADCASTS.lock().clear();
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

fn fb(x: f64) -> Value {
    json!({"bits": if x.is_nan() { f64::NAN.to_bits() } else { x.to_bits() }})
}

fn norm(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.contains_key("bits") => fb(f(v)),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), norm(v))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        other => other.clone(),
    }
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("fixture hex"))
        .collect()
}

fn exc_name(msg: &str) -> String {
    msg.split(':').next().unwrap_or(msg).trim().to_string()
}

fn panic_name(p: &(dyn std::any::Any + Send)) -> String {
    let msg = p
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    exc_name(&msg)
}

fn opt_pair(p: &Option<(String, String)>) -> Value {
    match p {
        Some((a, b)) => json!([a, b]),
        None => Value::Null,
    }
}

fn snap(area: &str, ac: &AudioController, want: &Value) -> Value {
    let st = state();
    match area {
        "camera" => {
            let c = &st.camera;
            let s = ac.lock.lock();
            let pitch = |x: f64, key: &str| {
                if want[key].is_null() {
                    Value::Null
                } else {
                    fb(x)
                }
            };
            json!({
                "free_cam_active": c.free_cam_active, "cam_yaw_deg": fb(c.cam_yaw_deg),
                "cam_pitch_deg": fb(c.cam_pitch_deg), "cam_agl": fb(c.cam_agl),
                "cam_agl_valid": c.cam_agl_valid, "cam_veh_bearing": fb(c.cam_veh_bearing),
                "cam_veh_distance": fb(c.cam_veh_distance), "cam_is_free": c.cam_is_free,
                "cam_last_click_heading_deg": fb(c.cam_last_click_heading_deg),
                "cam_compass_click_counter": c.cam_compass_click_counter,
                "cam_last_announced_compass_idx": c.cam_last_announced_compass_idx,
                "click_pitch": pitch(s._cam_click_request_pitch_mult, "click_pitch"),
                "highlight_pitch": pitch(s._cam_highlight_request_pitch_mult, "highlight_pitch"),
                "click": s._cam_click_request,
                "highlight": s._cam_highlight_click_request,
            })
        }
        "nodegrab" => {
            let v = ac.voice.lock();
            json!({
                "mode": NODEGRAB_MODE_ACTIVE.load(Ordering::SeqCst),
                "scanning": st.nodegrab.nodegrab_scanning,
                "pad": st.nodegrab.nodegrab_pad_active,
                "last_cid": st.nodegrab.nodegrab_last_cid,
                "hooked": st.nodegrab.nodegrab_space_hook.is_some(),
                "warps": WARPS.lock().clone(),
                "beep_freq": fb(v._node_beep_freq), "beep_reverse": v._node_beep_reverse,
                "beep_pos": fb(v._node_beep_playback_pos),
            })
        }
        "clickspot" => {
            let c = &st.clickspot;
            let v = ac.voice.lock();
            let row = |t: &(i64, i64, String)| json!([t.0, t.1, t.2]);
            json!({
                "mode": c.clickspot_mode_active,
                "list": c.clickspot_trigger_list.iter().map(row).collect::<Vec<_>>(),
                "loading": c.clickspot_list_loading,
                "building": c.clickspot_trigger_building.as_ref()
                    .map(|b| b.values().map(row).collect::<Vec<_>>()),
                "expected": c.clickspot_expected_count,
                "hover": c.clickspot_last_hover_id,
                "menu_index": st.menu.current_clickspot_index,
                "warps": WARPS.lock().clone(),
                "beep_reverse": v._clickspot_beep_reverse,
                "beep_pos": fb(v._clickspot_beep_playback_pos),
            })
        }
        "bindings" => {
            let b = &st.bindings;
            let rows =
                |l: &Vec<(i64, String)>| l.iter().map(|(i, s)| json!([i, s])).collect::<Vec<_>>();
            json!({
                "list": rows(&b.vehicle_bindings_list),
                "vehicle": b.vehicle_bindings_vehicle,
                "building": b.vehicle_bindings_building.as_ref().map(rows),
                "building_name": b.vehicle_bindings_building_name,
                "learn_active": b.binding_learn_active,
            })
        }
        "env" => {
            let e = &st.env;
            json!({
                "rows": e.env_rows, "level": e.env_level, "can_change": e.env_can_change,
                "building": e.env_building, "unavailable": e.env_unavailable,
                "ambient": e.env_ambient_c.map(fb), "awaiting": e.env_awaiting_change,
                "browser_open": e.env_browser_open,
            })
        }
        "aud" => {
            let a = &st.aud;
            json!({
                "rows": a.aud_rows, "building": a.aud_building, "sfx_rows": a.aud_sfx_rows,
                "sfx_building": a.aud_sfx_building, "sfx_head": a.aud_sfx_head,
                "sfx_stack": a.aud_sfx_stack, "pending": a.aud_pending,
                "pending_start": a.aud_pending_start, "awaiting": a.aud_awaiting,
                "browser": a.aud_browser,
            })
        }
        "vinfo" => json!({
            "rows": st.vinfo.vinfo_rows, "error": opt_pair(&st.vinfo.vinfo_error),
            "absent": st.vinfo.vinfo_absent, "event": VINFO_EVENT.is_set(),
        }),
        "page" => {
            let p = &st.page;
            json!({
                "context": p.ui_screen_context, "screen_title": p.ui_screen_title,
                "lines": p.page_text_lines, "title": p.page_text_title,
                "error": opt_pair(&p.page_text_error), "absent": p.page_text_absent,
                "broadcasts": BROADCASTS.lock().clone(),
            })
        }
        "slots" => {
            let s = slots();
            json!({
                "slots": s.vehicle_slots.iter().map(|(k, v)| json!([k, v.id, v.name]))
                    .collect::<Vec<_>>(),
                "selected": s.selected_slots.iter().collect::<Vec<_>>(),
                "target": s.target_slot,
            })
        }
        other => panic!("unknown area {other}"),
    }
}

fn vbrowser() -> Value {
    let st = state();
    let v = &st.vbrowser;
    json!({
        "active": v.vbrowser_active, "lines": v.vbrowser_lines, "index": v.vbrowser_index,
        "data": v.vbrowser_entry_data, "enter": v.vbrowser_on_enter.is_some(),
        "adjust": v.vbrowser_on_adjust.is_some(), "escape": v.vbrowser_on_escape.is_some(),
    })
}

/// One "recv" step; the exception name if the listener died.
fn recv(listener: &str, datagrams: &[Vec<u8>], ac: &AudioController) -> Option<String> {
    let text_of = |d: &[u8]| decode(d, Decode::Utf8Ignore).unwrap_or_default();
    let mut last = 0.0;
    for d in datagrams {
        let outcome: Result<(), String> = match listener {
            "camera" => camera::handle_camera_datagram(ac, d),
            "slots" => slot_listener::handle_slot_datagram(d),
            _ => {
                let t = text_of(d);
                let t = py_strip(&t);
                if t.is_empty() {
                    continue;
                }
                match listener {
                    "nodegrab" => nodegrab::handle_nodegrab_text(ac, t, time_time(), &mut last),
                    "clickspot" => clickspot::handle_clickspot_text(ac, t, time_time(), &mut last),
                    "bindings" => {
                        vehicle_bindings::handle_vehicle_bindings_text(t);
                        Ok(())
                    }
                    "env" => environment::handle_environment_text(t),
                    "aud" => {
                        audio_debug::handle_audio_debug_text(t, &|job| job());
                        Ok(())
                    }
                    "vinfo" => {
                        vehicle_info::handle_vehicle_info_text(t);
                        Ok(())
                    }
                    "learn" => binding_learn::handle_binding_learn_text(t),
                    other => panic!("unknown listener {other}"),
                }
            }
        };
        if let Err(e) = outcome {
            return Some(exc_name(&e));
        }
    }
    None
}

fn failure_json(r: (Vec<String>, Option<(String, String)>)) -> Value {
    json!([r.0, opt_pair(&r.1)])
}

fn call(name: &str, args: &[Value], ac: &Arc<AudioController>) -> Value {
    let s = |i: usize| args.get(i).and_then(Value::as_str);
    match name {
        "send_camera_diag" => camera::send_camera_diag(),
        "_camera_feed_problem" => return json!(camera::camera_feed_problem()),
        "toggle_free_cam_info" => camera::toggle_free_cam_info(Arc::clone(ac)),
        "toggle_nodegrab_mode" => nodegrab::toggle_nodegrab_mode(Arc::clone(ac)),
        "toggle_clickspot_mode" => clickspot::toggle_clickspot_mode(Arc::clone(ac)),
        "open_clickspot_browser" => browsers::open_clickspot_browser(),
        "open_vehicle_bindings_browser" => browsers::open_vehicle_bindings_browser(),
        "toggle_binding_learn_mode" => binding_learn::toggle_binding_learn_mode(),
        "open_environment_browser" => browsers::open_environment_browser(),
        "speak_ambient_temperature" => browsers::speak_ambient_temperature(),
        "_env_change_timed_out" => browsers::env_change_timed_out(),
        "open_audio_debug_browser" => browsers::open_audio_debug_browser(),
        "_aud_timed_out" => browsers::aud_timed_out(),
        "request_vehicle_info" => {
            let timeout = args.get(2).and_then(Value::as_f64).unwrap_or(0.6);
            return failure_json(browsers::request_vehicle_info(s(0), s(1), timeout));
        }
        "request_page_text" => {
            let timeout = args.first().and_then(Value::as_f64).unwrap_or(0.6);
            return failure_json(browsers::request_page_text(timeout));
        }
        "_read_mod_details_page" => return json!(browsers::read_mod_details_page()),
        "open_page_text_browser" => return json!(browsers::open_page_text_browser()),
        "_on_screen_context" => browsers::on_screen_context(s(0).unwrap(), s(1).unwrap()),
        other => panic!("unknown call {other}"),
    }
    Value::Null
}

fn set(name: &str, value: &Value) {
    let mut st = state();
    match name {
        "free_cam_active" => st.camera.free_cam_active = value.as_bool().unwrap(),
        "nodegrab_scanning" => st.nodegrab.nodegrab_scanning = value.as_bool().unwrap(),
        "_aud_rows" => st.aud.aud_rows = value.as_array().unwrap().clone(),
        "_aud_pending" => st.aud.aud_pending = value.as_str().map(str::to_string),
        "_selected_slots" => {
            drop(st);
            slots().selected_slots = value
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap())
                .collect();
        }
        "_target_slot" => {
            drop(st);
            slots().target_slot = value.as_i64();
        }
        other => panic!("unknown setting {other}"),
    }
}

fn apply_init(init: &Value) {
    let Some(m) = init.as_object() else { return };
    let mut st = state();
    for (k, v) in m {
        match k.as_str() {
            "compass_highlight_nth_click" => {
                st.cfg.compass_highlight_nth_click = v.as_i64().unwrap()
            }
            "compass_highlight_enabled" => st.cfg.compass_highlight_enabled = v.as_bool().unwrap(),
            "compass_min_interval" => st.tel.compass_min_interval = f(v),
            "compass_click_interval_deg" => st.cfg.compass_click_interval_deg = f(v),
            "announce_clickspot_actions" => {
                st.cfg.announce_clickspot_actions = v.as_bool().unwrap()
            }
            "announce_binding_learn_description" => {
                st.cfg.announce_binding_learn_description = v.as_bool().unwrap()
            }
            other => panic!("unknown init {other}"),
        }
    }
}

fn browser_call<R>(f: impl FnOnce(usize, String, Option<Value>) -> R) -> R {
    let (idx, line, data) = {
        let st = state();
        let v = &st.vbrowser;
        let idx = v.vbrowser_index;
        (
            idx,
            v.vbrowser_lines[idx].clone(),
            v.vbrowser_entry_data.get(idx).cloned(),
        )
    };
    f(idx, line, data)
}

#[test]
fn e4_cases_match_python() {
    let _g = setup();
    let fx = load("e4_listeners");
    let mut total = 0;
    for c in fx["cases"].as_array().unwrap() {
        reset();
        let label = c["name"].as_str().unwrap();
        let area = c["area"].as_str().unwrap();
        units::set_units_mode(c["units"].as_str().unwrap());
        apply_init(&c["init"]);
        let ac = AudioController::new();
        for (i, step) in c["steps"].as_array().unwrap().iter().enumerate() {
            clear_step();
            let s = step["step"].as_array().unwrap();
            let kind = s[0].as_str().unwrap();
            let mut died = Value::Null;
            let mut result = Value::Null;
            match kind {
                "recv" => {
                    let data: Vec<Vec<u8>> = s[2]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|d| hex(d.as_str().unwrap()))
                        .collect();
                    died = json!(recv(s[1].as_str().unwrap(), &data, &ac));
                }
                "call" => result = call(s[1].as_str().unwrap(), s[2].as_array().unwrap(), &ac),
                "enter" => {
                    let cb = state().vbrowser.vbrowser_on_enter.clone().unwrap();
                    browser_call(|idx, line, data| cb(idx, &line, data));
                    if area == "clickspot" {
                        std::thread::sleep(std::time::Duration::from_millis(450));
                    }
                }
                "adjust" => {
                    let cb = state().vbrowser.vbrowser_on_adjust.clone().unwrap();
                    let delta = s[1].as_i64().unwrap();
                    browser_call(|idx, line, data| cb(idx, &line, data, delta));
                }
                "escape" => {
                    let cb = state().vbrowser.vbrowser_on_escape.clone();
                    match cb {
                        Some(cb) => cb(),
                        None => virtual_browser::close_virtual_browser(true),
                    }
                }
                "drive" => virtual_browser::drive_virtual_browser(s[1].as_str().unwrap()),
                "set" => set(s[1].as_str().unwrap(), &s[2]),
                "set_mono" => state().last_vehicle_switch_ts = monotonic() + f(&s[2]),
                "vinfo_replies" => {
                    *VINFO_REPLIES.lock() = s[1]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().to_string())
                        .collect()
                }
                "page_replies" => *PAGE_REPLIES.lock() = s[1].as_array().unwrap().clone(),
                other => panic!("unknown step {other}"),
            }
            cancel_timers();
            total += 1;
            let at = format!("{label} step {i} {}", step["step"]);
            assert_eq!(died, step["died"], "{at}: died");
            assert_eq!(result, norm(&step["result"]), "{at}: result");
            assert_eq!(
                tap(),
                python_speech_after_departures(&step["tap"]),
                "{at}: speech"
            );
            assert_eq!(sent(), step["sent"], "{at}: sent");
            assert_eq!(vbrowser(), norm(&step["browser"]), "{at}: browser");
            let want = norm(&step["state"]);
            assert_eq!(snap(area, &ac, &want), want, "{at}: state");
        }
    }
    assert_eq!(total, 258, "every recorded step was replayed");
    reset();
}

#[test]
fn camera_diag_phrases_match_python() {
    let _g = setup();
    let fx = load("e4_listeners");
    for c in fx["camera_diag"].as_array().unwrap() {
        units::set_units_mode(c["units"].as_str().unwrap());
        assert_eq!(
            camera::format_camera_diag(c["heights"].as_str().unwrap()),
            c["out"].as_str().unwrap(),
            "{c}"
        );
    }
    reset();
}

fn env_row(v: &Value) -> EnvRow {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect()
}

#[test]
fn env_rows_match_python() {
    let _g = setup();
    let fx = load("e4_listeners");
    for c in fx["env_lines"].as_array().unwrap() {
        units::set_units_mode(c["units"].as_str().unwrap());
        let row = env_row(&c["row"]);
        let got = match catch_unwind(AssertUnwindSafe(|| browsers::env_row_line(&row))) {
            Ok(s) => json!({"ok": s}),
            Err(p) => json!({"err": panic_name(&*p)}),
        };
        assert_eq!(got, c["out"], "{c}");
    }
    for c in fx["env_parse"].as_array().unwrap() {
        let got = environment::parse_env_fields(c["payload"].as_str().unwrap());
        assert_eq!(serde_json::to_value(&got).unwrap(), c["out"], "{c}");
    }
    for c in fx["env_display"].as_array().unwrap() {
        units::set_units_mode(c["units"].as_str().unwrap());
        let got = browsers::env_temp_from_display(c["v"].as_i64().unwrap() as f64);
        assert_eq!(got.to_bits(), f(&c["out"]).to_bits(), "{c}");
    }
    reset();
}

#[test]
fn vinfo_render_matches_python() {
    let _g = setup();
    let fx = load("e4_listeners");
    for c in fx["vinfo_render"].as_array().unwrap() {
        let rows = c["rows"].as_array().unwrap().clone();
        let got = catch_unwind(AssertUnwindSafe(|| browsers::vinfo_render(&rows)));
        match got {
            Ok(lines) => assert_eq!(json!(lines), c["out"], "{c}"),
            Err(p) => assert_eq!(json!(panic_name(&*p)), c["err"], "{c}"),
        }
    }
}

#[test]
fn learn_phrases_match_python() {
    let fx = load("e4_listeners");
    for c in fx["learn_phrases"].as_array().unwrap() {
        let empty = Map::new();
        let row = c["row"].as_object().unwrap_or(&empty);
        let got = binding_learn::try_binding_learn_phrase(row, c["describe"].as_bool().unwrap());
        match got {
            Ok(s) => assert_eq!(json!(s), c["out"], "{c}"),
            Err(e) => assert_eq!(json!(exc_name(&e)), c["err"], "{c}"),
        }
    }
}

#[test]
fn console_history_matches_python() {
    let fx = load("e4_listeners");
    for c in fx["history_saves"].as_array().unwrap() {
        let hist: Vec<String> = c["history"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            console::console_history_json(&hist),
            c["bytes"].as_str().unwrap(),
            "{c}"
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("console_history.json");
    for c in fx["history_loads"].as_array().unwrap() {
        match c["content"].as_str() {
            Some(h) => std::fs::write(&path, hex(h)).unwrap(),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
        assert_eq!(
            json!(console::load_console_history_from(&path)),
            c["out"],
            "{c}"
        );
    }
    // Round trip through the real file.
    let hist = vec!["one".to_string(), "caf\u{e9}".to_string()];
    console::save_console_history_to(&path, &hist);
    assert_eq!(console::load_console_history_from(&path), hist);
}

#[test]
fn console_tap_consumes_and_window_gets_the_rest() {
    let _g = setup();
    let sink = Arc::new(beam_engine::ui_sink::RecordingSink::new());
    beam_engine::ui_sink::set_ui_sink(sink.clone());
    console::register_console_tap(Some(Arc::new(|t: &str| t.starts_with("RESP|"))));
    console::handle_console_datagram(b"RESP|mine");
    console::handle_console_datagram(b"LOG|theirs\xff");
    console::register_console_tap(Some(Arc::new(|_t: &str| panic!("tap fault"))));
    console::handle_console_datagram(b"OUT|after a fault");
    console::register_console_tap(None);
    console::handle_console_datagram(b"CTXEND");
    beam_engine::ui_sink::clear_ui_sink();
    use beam_engine::ui_sink::UiEvent::Console;
    assert_eq!(
        sink.events(),
        vec![
            Console("LOG|theirs\u{FFFD}".into()),
            Console("OUT|after a fault".into()),
            Console("CTXEND".into()),
        ]
    );
}

/// python-bugs.md 52 (departure): a negative clickspot count says there are none
/// instead of "-2 clickspots found".
fn python_speech_after_departures(tap: &Value) -> Value {
    let mut out = tap.clone();
    if let Some(items) = out.as_array_mut() {
        for it in items {
            let text = it["text"].as_str().unwrap_or("");
            if let Some(n) = text.strip_suffix(" clickspots found")
                && n.starts_with('-')
            {
                it["text"] = json!("No clickspots found on this vehicle");
            }
        }
    }
    out
}

#[test]
fn induction_report_matches_python() {
    let _g = setup();
    let fx = load("e4_listeners");
    for c in fx["induction"].as_array().unwrap() {
        clear_step();
        units::set_units_mode(c["units"].as_str().unwrap());
        let replies: Vec<String> = c["replies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let died = browsers::induction_collect(replies)
            .and_then(|(r, a)| browsers::induction_speak(r.as_ref(), a))
            .err()
            .map(|e| exc_name(&e));
        if c["died"].is_null() {
            assert_eq!(json!(died), c["died"], "{c}");
            assert_eq!(tap(), c["tap"], "{c}");
        } else {
            // python-bugs.md 7, 53 (departure): where the Python thread died silently,
            // the Rust carries on and always speaks something.
            assert_eq!(died, None, "{c}");
            let spoken = tap();
            assert_ne!(spoken, json!([]), "nothing spoken for {c}");
        }
        assert_eq!(c["sent"], json!([[4448, "INDUCTION"]]), "{c}");
    }
    reset();
}

#[test]
fn nodegrab_ctrl_poll_and_space_hook() {
    let _g = setup();
    // Off: no sends, the edge state is dropped.
    assert!(!nodegrab::poll_ctrl(true, false));
    assert_eq!(sent(), json!([]));
    NODEGRAB_MODE_ACTIVE.store(true, Ordering::SeqCst);
    assert!(nodegrab::poll_ctrl(true, false));
    assert!(state().nodegrab.nodegrab_scanning);
    assert!(nodegrab::poll_ctrl(true, true));
    assert!(!nodegrab::poll_ctrl(false, true));
    assert!(!state().nodegrab.nodegrab_scanning);
    assert_eq!(
        sent(),
        json!([[4455, "SCAN_ON"], [4455, "SNAP"], [4455, "SCAN_OFF"]])
    );
    // Key-up always passes and clears the repeat guard.
    use beam_engine::keyboard::KeyEvent;
    NODEGRAB_SPACE_HELD.store(true, Ordering::SeqCst);
    assert!(nodegrab::nodegrab_on_space(&KeyEvent::up("space")));
    assert!(!NODEGRAB_SPACE_HELD.load(Ordering::SeqCst));
    // Without Ctrl held, plain Space passes on (the handbrake).
    assert!(nodegrab::nodegrab_on_space(&KeyEvent::down("space")));
    reset();
}

#[test]
fn binding_learn_keepalive_sends_only_while_active() {
    let _g = setup();
    let stop = Arc::new(beam_engine::timer::Event::new());
    let s2 = Arc::clone(&stop);
    state().bindings.binding_learn_active = true;
    let t = std::thread::spawn(move || binding_learn::binding_learn_keepalive_thread_fn(&s2));
    std::thread::sleep(std::time::Duration::from_millis(1500));
    state().bindings.binding_learn_active = false;
    std::thread::sleep(std::time::Duration::from_millis(1100));
    stop.set();
    t.join().unwrap();
    assert_eq!(sent(), json!([[4480, "KEEPALIVE"]]));
    reset();
}
