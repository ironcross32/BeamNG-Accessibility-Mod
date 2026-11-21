//! Python-golden tests for E7 (`rust/fixtures/gen/e7_ai.py`): the F10 key dispatch with
//! every key and modifier, the AI commands sent on 4449/4459/4464, status speech, the
//! assistant callbacks, both provider request bodies, key validation, the describer worker
//! and its log lines.
//!
//! No test makes a network request (the transport is replaced), captures the screen (the
//! grab is replaced) or installs a system-wide keyboard hook.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Once};

use beam_audio::controller::AudioController;
use beam_base::pyval::from_json;
use beam_engine::ai_describer::{
    self, HttpOutcome, HttpRequest, Opts, Region, VALIDATE_TIMEOUT_SEC, py_json_dumps, py_quote,
};
use beam_engine::ai_layer::{self, F10_HELP};
use beam_engine::capture;
use beam_engine::keyboard::{self, KeyEvent, Layer};
use beam_engine::speech_out::{self, command_context, get_speech_log, set_command_context};
use beam_engine::state::{
    DESCRIBER_BUSY, Mods, SlotInfo, Slots, State, set_audio_controller_ref, slots, state,
};
use beam_engine::timer::monotonic;
use beam_engine::units::set_units_mode;
use beam_engine::win_util;
use beam_testutil::{f, load};
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{Value, json};

static SENT: Mutex<Vec<(u16, String)>> = Mutex::new(Vec::new());
static STAGGERED: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());
static REQUESTS: Mutex<Vec<HttpRequest>> = Mutex::new(Vec::new());
static NEXT_RESPONSE: Mutex<Option<Value>> = Mutex::new(None);
static CAPTURE_FAIL: Mutex<Option<String>> = Mutex::new(None);
static ENV: Once = Once::new();

fn png() -> Vec<u8> {
    let fx = load("e7_ai");
    let h = fx["png_hex"].as_str().unwrap();
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect()
}

fn setup() -> parking_lot::MutexGuard<'static, ()> {
    let guard = speech_out::TEST_SERIAL.lock();
    ENV.call_once(|| {
        let dir = std::env::temp_dir().join(format!("beam_e7_tests_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: inside `Once`, while TEST_SERIAL is held and before any test of this
        // binary reads the environment.
        unsafe { std::env::set_var("LOCALAPPDATA", &dir) };
    });
    speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
    ai_layer::set_send_hook_for_tests(Some(Box::new(|port: u16, cmd: &str| {
        SENT.lock().push((port, cmd.to_string()));
    })));
    ai_layer::set_staggered_hook_for_tests(Some(Box::new(|cmds: &[String]| {
        STAGGERED.lock().push(cmds.to_vec());
    })));
    ai_describer::set_transport_for_tests(Some(Box::new(|req: &HttpRequest| {
        REQUESTS.lock().push(req.clone());
        let spec = NEXT_RESPONSE
            .lock()
            .clone()
            .unwrap_or(json!({"kind": "exc", "msg": "no response configured"}));
        let body = spec["body"].as_str().unwrap_or("").as_bytes().to_vec();
        match spec["kind"].as_str().unwrap() {
            "ok" => HttpOutcome::Response {
                status: spec["status"].as_u64().unwrap_or(200) as u16,
                body,
            },
            "http" => HttpOutcome::HttpError {
                code: spec["code"].as_u64().unwrap() as u16,
                body,
            },
            "url" => HttpOutcome::UrlError(spec["reason"].as_str().unwrap().to_string()),
            _ => HttpOutcome::Other(spec["msg"].as_str().unwrap().to_string()),
        }
    })));
    let image = png();
    ai_describer::set_capture_hook_for_tests(Some(Box::new(move |region: Option<Region>| {
        assert!(region.is_none());
        match CAPTURE_FAIL.lock().clone() {
            Some(m) => Err(m),
            None => Ok(image.clone()),
        }
    })));
    speech_out::reset_for_tests();
    SENT.lock().clear();
    STAGGERED.lock().clear();
    REQUESTS.lock().clear();
    *state() = State::default();
    *slots() = Slots::default();
    set_units_mode("imperial");
    guard
}

fn tap_len() -> usize {
    get_speech_log(None, Some(0), None, None, false)
        .entries
        .len()
}

fn spoken_since(n: usize) -> Value {
    let log = get_speech_log(None, Some(0), None, None, false);
    Value::Array(
        log.entries[n..]
            .iter()
            .map(|e| json!([e.text, e.interrupt, e.excluded]))
            .collect(),
    )
}

fn take_sent() -> Value {
    let token = Regex::new("[0-9a-f]{32}").unwrap();
    let v = SENT
        .lock()
        .drain(..)
        .map(|(p, c)| json!([p, token.replace_all(&c, "<token>")]))
        .collect();
    Value::Array(v)
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

fn opt_f(v: &Value) -> Option<f64> {
    if v.is_null() { None } else { Some(f(v)) }
}

fn ai_state() -> (Value, Option<f64>) {
    let s = slots();
    let selected: Vec<i64> = s.selected_slots.iter().copied().collect();
    let target = s.target_slot;
    drop(s);
    let st = state();
    let mut police: Vec<i64> = st.ai.police_lights_active.iter().copied().collect();
    police.sort();
    let commands: Vec<Value> = st
        .ai
        .ai_vehicle_commands
        .iter()
        .map(|(vid, c)| json!([vid, c.mode, c.target_id]))
        .collect();
    (
        json!({
            "selected": selected,
            "target": target,
            "pending": st.ai.pending_target_confirm,
            "avoid": st.ai.ai_avoid_mode,
            "lane": st.ai.ai_lane_driving,
            "help": st.kb.input_help_mode,
            "police": police,
            "commands": commands,
            "command_context": command_context(),
        }),
        st.ai.ai_speed_limit_ms,
    )
}

fn load_slots(rows: &Value) {
    let mut s = slots();
    *s = Slots::default();
    for r in rows.as_array().unwrap() {
        s.vehicle_slots.insert(
            r[0].as_i64().unwrap(),
            SlotInfo {
                id: r[1].as_i64().unwrap(),
                name: r[2].as_str().unwrap().to_string(),
            },
        );
    }
}

#[test]
fn f10_help_is_byte_identical() {
    let _g = setup();
    let fx = load("e7_ai");
    let want = fx["f10_help"].as_array().unwrap();
    assert_eq!(want.len(), F10_HELP.len());
    for (w, (k, v)) in want.iter().zip(F10_HELP.iter()) {
        let got = json!([k.0, k.1, k.2, k.3, v]);
        assert_eq!(&got, w);
    }
}

#[test]
fn f10_key_dispatch_matches_python() {
    let _g = setup();
    let fx = load("e7_ai");
    let ac = AudioController::new();
    set_audio_controller_ref(Some(Arc::clone(&ac)));
    for sc in fx["keys"].as_array().unwrap() {
        let name = sc["name"].as_str().unwrap();
        speech_out::reset_for_tests();
        *state() = State::default();
        set_units_mode(sc["units"].as_str().unwrap());
        load_slots(&sc["slots"]);
        set_command_context(true);
        // Busy, so F10+Space only buzzes: the buzz is how the trigger is observed.
        *DESCRIBER_BUSY.lock() = true;
        for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
            let ctx = format!("{name} step {i} {step}");
            SENT.lock().clear();
            STAGGERED.lock().clear();
            ac.voice.lock()._describe_error_playback_pos = -1.0;
            keyboard::kb_open_layer(Layer::F10);
            state().ai.ai_mods = Mods {
                ctrl: step["ctrl"].as_bool().unwrap(),
                shift: step["shift"].as_bool().unwrap(),
                alt: step["alt"].as_bool().unwrap(),
            };
            let before = tap_len();
            let key = step["key"].as_str().unwrap().to_string();
            let err = catch_unwind(AssertUnwindSafe(|| {
                ai_layer::on_ai_key_press(&KeyEvent::down(&key))
            }))
            .err()
            .map(|p| panic_text(&*p));
            // python-bugs.md 58 (departure): only a single digit is a slot key. The Python's
            // substring test let "" (int('') raised) and "12" ("Slot 12 is empty") through.
            if key.is_empty() || (key.len() > 1 && key.chars().all(|c| c.is_ascii_digit())) {
                assert_eq!(err, None, "{ctx}: err");
                let spoken = spoken_since(before).to_string();
                assert!(!spoken.contains("Slot"), "{ctx}: spoken {spoken}");
                take_sent();
                STAGGERED.lock().clear();
                continue;
            }
            assert_eq!(json!(err), step["err"], "{ctx}: err");
            assert_eq!(spoken_since(before), step["spoken"], "{ctx}: spoken");
            assert_eq!(take_sent(), step["sent"], "{ctx}: sent");
            assert_eq!(
                json!(*STAGGERED.lock()),
                step["staggered"],
                "{ctx}: staggered"
            );
            let described = ac.voice.lock()._describe_error_playback_pos == 0.0;
            assert_eq!(json!(described), step["describe"], "{ctx}: describe");
            assert_eq!(
                json!(keyboard::kb_layer() == Some(Layer::F10)),
                step["layer_open"],
                "{ctx}: layer"
            );
            let (mut got, speed) = ai_state();
            let mut want = step["state"].clone();
            let want_speed = opt_f(&want["speed"]);
            want.as_object_mut().unwrap().remove("speed");
            got.as_object_mut().unwrap().remove("speed");
            assert_eq!(got, want, "{ctx}: state");
            assert_eq!(
                speed.map(f64::to_bits),
                want_speed.map(f64::to_bits),
                "{ctx}: speed"
            );
        }
    }
    keyboard::kb_close_layer();
    set_audio_controller_ref(None);
    *DESCRIBER_BUSY.lock() = false;
}

#[test]
fn status_speech_matches_python() {
    let _g = setup();
    let fx = load("e7_ai");
    for c in fx["status_all"].as_array().unwrap() {
        let n = tap_len();
        ai_layer::speak_status_all_response(c["payload"].as_str().unwrap());
        assert_eq!(spoken_since(n), c["spoken"], "{}", c["payload"]);
    }
    for c in fx["ai_status"].as_array().unwrap() {
        load_slots(&c["slots"]);
        {
            let mut st = state();
            st.ai.ai_vehicle_commands.clear();
            for r in c["commands"].as_array().unwrap() {
                st.ai.ai_vehicle_commands.insert(
                    r[0].as_i64().unwrap(),
                    beam_engine::state::AiCommand {
                        mode: r[1].as_str().unwrap().to_string(),
                        target_id: r[2].as_i64(),
                    },
                );
            }
        }
        let n = tap_len();
        ai_layer::speak_ai_status();
        assert_eq!(spoken_since(n), c["spoken"], "{c}");
    }
}

#[test]
fn assistant_callbacks_match_python() {
    let _g = setup();
    let fx = load("e7_ai");
    for c in fx["arrivals"].as_array().unwrap() {
        {
            let mut st = state();
            st.tel.marked_coord_x = None;
            st.tel.marked_coord_y = None;
            st.tel.last_coord_bearing_ts = 5.0;
        }
        let v = beam_road::pyjson::loads(&c["arrival"].to_string()).unwrap();
        ai_layer::assistant_arrival(&v);
        let st = state();
        assert_eq!(st.tel.marked_coord_x, opt_f(&c["x"]), "{c}");
        assert_eq!(st.tel.marked_coord_y, opt_f(&c["y"]), "{c}");
        assert_eq!(st.tel.last_coord_bearing_ts, f(&c["ts"]), "{c}");
    }
    let ac = AudioController::new();
    set_audio_controller_ref(Some(Arc::clone(&ac)));
    for c in fx["cues"].as_array().unwrap() {
        speech_out::set_speech_protected_until(0.0);
        ac.lock.lock()._assistant_cue_queue.clear();
        let kind = c["kind"].as_str().unwrap();
        let choices: Vec<String> = c["choices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        ai_layer::assistant_cue(kind, &choices);
        let protected = speech_out::speech_protected_until() > monotonic();
        assert_eq!(json!(protected), c["protected"], "{c}");
        if kind == "junction" || kind == "off" {
            let queued = !ac.lock.lock()._assistant_cue_queue.is_empty();
            assert_eq!(json!(queued), c["called"], "{c}");
        }
        if kind == "protect" {
            assert_eq!(c["called"], json!(false));
        }
    }
    set_audio_controller_ref(None);
}

fn compare_requests(want: &Value, ctx: &str) {
    let got = REQUESTS.lock().drain(..).collect::<Vec<_>>();
    let want = want.as_array().unwrap();
    assert_eq!(got.len(), want.len(), "{ctx}: request count");
    for (g, w) in got.iter().zip(want) {
        assert_eq!(g.method, w["method"].as_str().unwrap(), "{ctx}: method");
        assert_eq!(g.url, w["url"].as_str().unwrap(), "{ctx}: url");
        let mut headers = serde_json::Map::new();
        for (k, v) in &g.headers {
            headers.insert(k.to_lowercase(), json!(v));
        }
        assert_eq!(Value::Object(headers), w["headers"], "{ctx}: headers");
        let body = g
            .body
            .as_ref()
            .map(|b| String::from_utf8(b.clone()).unwrap());
        assert_eq!(json!(body), w["body"], "{ctx}: body bytes");
        if let Some(b) = &body {
            let gv: Value = serde_json::from_str(b).unwrap();
            let wv: Value = serde_json::from_str(w["body"].as_str().unwrap()).unwrap();
            assert_eq!(gv, wv, "{ctx}: body JSON");
        }
        assert_eq!(g.timeout, f(&w["timeout"]), "{ctx}: timeout");
    }
}

/// `Could not parse API response: ...` carries the JSON parser's own message, which is
/// not reproduced; the prefix is compared.
fn same_error(got: &str, want: &str) -> bool {
    const P: &str = "Could not parse API response: ";
    if want.starts_with(P) {
        got.starts_with(P)
    } else {
        got == want
    }
}

fn opt_s(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

#[test]
fn key_validation_matches_python() {
    let _g = setup();
    let fx = load("e7_ai");
    for c in fx["validations"].as_array().unwrap() {
        let ctx = c.to_string();
        *NEXT_RESPONSE.lock() = Some(c["resp"].clone());
        let o = &c["opts"];
        let opts = Opts {
            model: opt_s(&o["model"]),
            base_url: opt_s(&o["base_url"]),
            reasoning_effort: opt_s(&o["reasoning_effort"]),
            detail: opt_s(&o["detail"]),
        };
        let r = ai_describer::try_validate_api_key(
            c["key"].as_str().unwrap(),
            c["provider"].as_str().unwrap(),
            VALIDATE_TIMEOUT_SEC,
            &opts,
        );
        let want = &c["result"];
        match r {
            Ok((ok, err)) => {
                assert_eq!(json!(ok), want["ok"], "{ctx}");
                match (err, want["err"].as_str()) {
                    (None, None) => {}
                    (Some(g), Some(w)) => assert!(same_error(&g, w), "{ctx}: {g} vs {w}"),
                    (g, w) => panic!("{ctx}: {g:?} vs {w:?}"),
                }
            }
            Err(e) => assert_eq!(json!(e), want["raised"], "{ctx}"),
        }
        compare_requests(&c["requests"], &ctx);
    }
}

fn log_lines() -> Value {
    let path = ai_describer::log_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return json!([]);
    };
    let _ = std::fs::remove_file(&path);
    let stamp = Regex::new(r"^\[\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\] ").unwrap();
    let text = String::from_utf8(bytes).unwrap();
    Value::Array(
        text.split("\r\n")
            .map(|l| json!(stamp.replace(l, "").into_owned()))
            .collect(),
    )
}

#[test]
fn describer_worker_matches_python() {
    let _g = setup();
    let fx = load("e7_ai");
    let _ = std::fs::remove_file(ai_describer::log_path());
    for w in fx["workers"].as_array().unwrap() {
        let ctx = w["name"].as_str().unwrap();
        SENT.lock().clear();
        REQUESTS.lock().clear();
        *NEXT_RESPONSE.lock() = Some(
            w.get("resp")
                .cloned()
                .unwrap_or(json!({"kind": "exc", "msg": "no response configured"})),
        );
        *CAPTURE_FAIL.lock() = w
            .get("capture")
            .and_then(|c| c.as_str())
            .map(str::to_string);
        {
            let mut st = state();
            st.cfg.ai_describer_provider = w["provider"].as_str().unwrap().to_string();
            st.cfg.ai_describer_settings = w["settings"].as_object().unwrap().clone();
            st.cfg.ai_describer_disable_ui_toggle = w.get("disable") == Some(&json!(true));
        }
        *DESCRIBER_BUSY.lock() = true;
        let n = tap_len();
        ai_describer::ai_describe_worker(None);
        assert!(!*DESCRIBER_BUSY.lock(), "{ctx}: busy cleared");
        assert_eq!(spoken_since(n), w["spoken"], "{ctx}: spoken");
        assert_eq!(take_sent(), w["sent"], "{ctx}: sent");
        compare_requests(&w["requests"], ctx);
        assert_eq!(log_lines(), w["log"], "{ctx}: log");
    }
    *CAPTURE_FAIL.lock() = None;
}

#[test]
fn log_lines_and_helpers_match_python() {
    let _g = setup();
    let fx = load("e7_ai");
    let _ = std::fs::remove_file(ai_describer::log_path());
    ai_describer::log_description("line1\nline2  \n");
    ai_describer::log_error("  err\n x \r ");
    ai_describer::log_description("");
    ai_describer::log_error("");
    assert_eq!(log_lines(), fx["log_lines"]);
    for c in fx["misc"]["quote"].as_array().unwrap() {
        assert_eq!(
            py_quote(c[0].as_str().unwrap()),
            c[1].as_str().unwrap(),
            "{c}"
        );
    }
    for c in fx["misc"]["openai_base"].as_array().unwrap() {
        assert_eq!(
            ai_describer::openai_base(c[0].as_str()),
            c[1].as_str().unwrap(),
            "{c}"
        );
    }
    // json.dumps defaults.
    assert_eq!(
        py_json_dumps(&json!({"a": [1, 2.5, "é\n\"x\"", null, true], "b": {}, "😀": "\u{1}"})),
        r#"{"a": [1, 2.5, "\u00e9\n\"x\"", null, true], "b": {}, "\ud83d\ude00": "\u0001"}"#
    );
    let _ = from_json(&json!(1));
}

#[test]
fn capture_sequence_hides_and_restores_the_ui() {
    let _g = setup();
    let (bytes, meta) = capture::capture_scene_png(true, 0.0, false).unwrap();
    assert_eq!(bytes, png());
    assert_eq!(
        meta.to_string(),
        r#"{"focused_game":false,"game_window_found":false,"region":null}"#
    );
    assert_eq!(take_sent(), json!([[4464, "HIDE"], [4464, "SHOW"]]));
    *CAPTURE_FAIL.lock() = Some("grab failed".into());
    let r = capture::capture_scene_png(true, -1.0, false);
    assert_eq!(r.unwrap_err(), "grab failed");
    assert_eq!(take_sent(), json!([[4464, "HIDE"], [4464, "SHOW"]]));
    let _ = capture::capture_scene_png(false, 0.2, false);
    assert_eq!(take_sent(), json!([]));
    *CAPTURE_FAIL.lock() = None;
}

#[test]
fn window_geometry_helpers() {
    assert_eq!(
        capture::region_from_rect(-32000, -32000, -31840, -31972),
        None
    );
    assert_eq!(capture::region_from_rect(0, 0, 63, 500), None);
    assert_eq!(
        capture::region_from_rect(10, -30000, 110, -29900).map(|r| r.top),
        Some(-30000)
    );
    assert_eq!(
        capture::region_from_rect(100, 50, 1380, 770),
        Some(Region {
            left: 100,
            top: 50,
            width: 1280,
            height: 720
        })
    );
    assert_eq!(
        capture::region_from_rect(1, 2, 65, 66)
            .unwrap()
            .to_json()
            .to_string(),
        r#"{"left":1,"top":2,"width":64,"height":64}"#
    );
    // int(round(x * client / float(view))): ties to even, and no scaling unless all four
    // sizes are positive.
    assert_eq!(win_util::scale_viewport(5, 1, 2, 1, 1), 2);
    assert_eq!(win_util::scale_viewport(3, 1, 2, 1, 1), 2);
    assert_eq!(win_util::scale_viewport(960, 1280, 1920, 720, 1080), 640);
    assert_eq!(win_util::scale_viewport(960, 1280, 0, 720, 1080), 960);
    assert_eq!(win_util::scale_viewport(960, 1280, 1920, 0, 1080), 960);
    assert_eq!(
        win_util::image_basename_lower(r"C:\Games\BeamNG.drive.x64.EXE"),
        "beamng.drive.x64.exe"
    );
}

#[test]
fn staggered_presets_send_in_order() {
    let _g = setup();
    ai_layer::run_staggered_with_delay(vec!["A".into(), "B".into(), "C".into()], 0.02);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while SENT.lock().len() < 3 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(take_sent(), json!([[4449, "A"], [4449, "B"], [4449, "C"]]));
}

#[test]
fn trigger_buzzes_when_busy_and_clears_after_a_run() {
    let _g = setup();
    let ac = AudioController::new();
    *DESCRIBER_BUSY.lock() = true;
    ac.voice.lock()._describe_error_playback_pos = -1.0;
    ai_describer::trigger_ai_describe(Some(Arc::clone(&ac)));
    assert_eq!(ac.voice.lock()._describe_error_playback_pos, 0.0);
    // Not busy: the worker runs on its own thread (no key set: it speaks and clears busy).
    *DESCRIBER_BUSY.lock() = false;
    let n = tap_len();
    ai_describer::trigger_ai_describe(None);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while *DESCRIBER_BUSY.lock() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!*DESCRIBER_BUSY.lock());
    assert_eq!(
        spoken_since(n),
        json!([[
            "No Google Gemini API key set. Configure it in the AI Describer tab.",
            true,
            false
        ]])
    );
    let _ = std::fs::remove_file(ai_describer::log_path());
}

#[test]
fn start_and_clear_the_f10_layer() {
    let _g = setup();
    set_command_context(true);
    ai_layer::start_ai_capture();
    assert_eq!(keyboard::kb_layer(), Some(Layer::F10));
    assert!(state().ai.ai_timer.is_none());
    state().ai.pending_target_confirm = Some(3);
    let n = tap_len();
    ai_layer::clear_ai_hook(true);
    assert_eq!(keyboard::kb_layer(), None);
    assert!(!command_context());
    assert_eq!(state().ai.pending_target_confirm, None);
    assert_eq!(spoken_since(n), json!([["Exit", true, true]]));
    ai_layer::send_slot_command("SLOT_VALIDATE");
    assert_eq!(take_sent(), json!([[4459, "SLOT_VALIDATE"]]));
}
