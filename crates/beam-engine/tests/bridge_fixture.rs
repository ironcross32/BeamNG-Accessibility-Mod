//! `ui_bridge` against the real `nvda_ws_speaker.py` (`fixtures/gen/bridge.py`):
//! every message type through `handle_ws_message` (calls made, log records written,
//! exception raised, debug flag), and raw relay lines through `tcp_process_line` (what is
//! written back, and which exception ends the connection).

use std::sync::{Arc, Once};

use beam_base::logger::{self, LogSink};
use beam_engine::ui_bridge as ub;
use beam_road::pyjson::{self, Value as PyJson};
use beam_testutil::load;
use parking_lot::Mutex;
use serde_json::{Value, json};

static CALLS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static LOGS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static TEST: Mutex<()> = Mutex::new(());

struct Capture;

impl LogSink for Capture {
    fn emit(&self, level: log::Level, line: &str) {
        // "YYYY-MM-DD HH:MM:SS [LEVEL] message"
        let msg = line.split_once("] ").map_or(line, |(_, m)| m);
        LOGS.lock().push(json!([logger::level_name(level), msg]));
    }
}

fn setup() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join("beam_bridge_fixture");
        let _ = std::fs::create_dir_all(&dir);
        logger::init_with(dir.join("bnvdahook.log"), false);
        logger::add_sink(Arc::new(Capture));
        let push = |v: Value| CALLS.lock().push(v);
        ub::set_speak_func_for_tests(Some(Arc::new(move |t: &str, i: bool| {
            push(json!(["speak", t, i]))
        })));
        ub::register_dom_dump_callback(Arc::new(|lines| {
            CALLS.lock().push(json!(["dom_dump", lines]))
        }));
        ub::register_loading_state_callback(Arc::new(|active, focus| {
            CALLS.lock().push(json!(["loading_state", active, focus]))
        }));
        ub::register_settings_request_callback(Arc::new(|| {
            CALLS.lock().push(json!(["settings_request"]))
        }));
        ub::register_screen_context_callback(Arc::new(|ctx, title| {
            CALLS.lock().push(json!(["screen_context", ctx, title]))
        }));
        ub::register_page_text_callback(Arc::new(|lines, title, code, sentence| {
            CALLS
                .lock()
                .push(json!(["page_text", lines, title, code, sentence]))
        }));
        ub::register_accessibility_action_callback(Arc::new(|a| {
            CALLS.lock().push(json!(["accessibility_action", a]))
        }));
        ub::register_challenge_event_callback(Arc::new(|d| {
            CALLS.lock().push(json!(["challenge_event", d]))
        }));
        ub::register_drift_event_callback(Arc::new(|d| {
            CALLS.lock().push(json!(["drift_event", d]))
        }));
    });
}

fn reset() {
    ub::reset_for_tests();
    CALLS.lock().clear();
    LOGS.lock().clear();
}

fn to_py(v: &Value) -> PyJson {
    pyjson::loads(&v.to_string()).unwrap()
}

#[test]
fn messages_match_python() {
    let _g = TEST.lock();
    setup();
    let fx = load("bridge");
    let mut failures = Vec::new();
    for case in fx["messages"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        reset();
        for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
            CALLS.lock().clear();
            LOGS.lock().clear();
            let raised = match ub::handle_ws_message(&to_py(&step["message"])) {
                Ok(()) => Value::Null,
                Err(e) => json!([e.type_name(), e.message()]),
            };
            let got = json!({
                "calls": *CALLS.lock(),
                "logs": *LOGS.lock(),
                "raised": raised,
                "debug": ub::bnvda_debug_enabled(),
            });
            let want = json!({
                "calls": step["calls"], "logs": step["logs"],
                "raised": step["raised"], "debug": step["debug"],
            });
            if got != want {
                failures.push(format!(
                    "{name}[{i}] {}:\n  got  {got}\n  want {want}",
                    step["message"]
                ));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn tcp_lines_match_python() {
    let _g = TEST.lock();
    setup();
    let fx = load("bridge");
    let mut failures = Vec::new();
    for case in fx["tcp"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        reset();
        let mut written = Vec::new();
        let mut error = Value::Null;
        for line in case["lines"].as_array().unwrap() {
            // Stored as latin-1 so arbitrary bytes survive JSON.
            let bytes: Vec<u8> = line.as_str().unwrap().chars().map(|c| c as u8).collect();
            match ub::tcp_process_line(&bytes) {
                Ok(Some(reply)) => written.push(String::from_utf8(reply).unwrap()),
                Ok(None) => {}
                Err(e) => {
                    error = json!(e.message());
                    break; // the exception ends the connection
                }
            }
        }
        let invalid: Vec<Value> = LOGS
            .lock()
            .iter()
            .filter_map(|l| l[1].as_str())
            .filter(|m| m.starts_with("[bnvda] Invalid Lua TCP"))
            .map(Value::from)
            .collect();
        if let Some(py_error) = case["error"].as_str() {
            // python-bugs.md 64 (departure): the Python relay dropped the connection here.
            // The Rust logs the message as invalid and goes on reading lines, so the
            // calls the Python made come first and the later lines are handled too.
            let calls = CALLS.lock().clone();
            let py_calls = case["calls"].as_array().unwrap();
            let ok = error.is_null()
                && calls.len() > py_calls.len()
                && calls[..py_calls.len()] == py_calls[..]
                && invalid
                    .iter()
                    .any(|m| m.as_str().unwrap().ends_with(py_error));
            if !ok {
                failures.push(format!(
                    "{name} (departure): error {error}, calls {calls:?}, invalid {invalid:?}"
                ));
            }
            continue;
        }
        let got = json!({"written": written, "calls": *CALLS.lock(), "error": error,
                         "invalid": invalid});
        let want = json!({"written": case["written"], "calls": case["calls"],
                          "error": case["error"], "invalid": case["invalid"]});
        if got != want {
            failures.push(format!("{name}:\n  got  {got}\n  want {want}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
