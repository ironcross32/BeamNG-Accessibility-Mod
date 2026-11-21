//! `mcp_server::dispatch` against the real `mcp_server._dispatch`
//! (`rust/fixtures/gen/mcp_dispatch.py`): every JSON-RPC message gives the same response
//! (or raises the same exception) and makes the same calls on the deps. The console
//! sender answers synchronously through `console_tap` by the generator's reply rule, so
//! the Lua-backed tools return at once. Volatile values are zeroed on both sides.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

use beam_engine::mcp_server::{self as ms, Deps};
use beam_engine::speech_out::{SpeechLog, TapEntry};
use beam_road::pyjson::{self, DumpOpts, Value as PyJson};
use beam_testutil::load;
use indexmap::IndexMap;
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{Value, json};

static CALLS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static WORLD: AtomicBool = AtomicBool::new(false);
static VOLATILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"("(?:elapsed_ms|mcp_uptime_s|t)": )-?[0-9.e+-]+"#).unwrap());

const CAM_POSE: &str = r#"{"mode":"orbit","is_free":false,"pos":[1.5,-2,3.25],"yaw_deg":90,"pitch_deg":-10,"roll_deg":0,"player_veh_id":7}"#;
const CAM_LIST: &str = r#"{"global_cameras":["free","orbit"],"vehicle_cameras":["driver","onboard.hood"],"player_veh_id":7,"active":"orbit"}"#;

/// The generator's `reply(cmd)`.
fn reply(cmd: &str) -> Vec<String> {
    if cmd == "CTXLIST" {
        return [
            "CTX|0|GE - Lua",
            "CTX|1|GE - TorqueScript",
            "CTX|2|CEF/UI - JS",
            "CTX|3|vehicle 7 pickup (current)",
            "CTX|4|vehicle 9 etk800",
            "CTXEND",
        ]
        .map(String::from)
        .to_vec();
    }
    if let Some(rest) = cmd.strip_prefix("EXEC|") {
        let code = rest.split_once('|').map_or("", |(_, c)| c);
        if code == ms::CAM_READ_LUA {
            return vec!["RESP|ok".into(), format!("OUT|= {CAM_POSE}"), "EXECEND".into()];
        }
        if code == ms::CAM_LIST_LUA {
            return vec!["RESP|ok".into(), format!("OUT|= {CAM_LIST}"), "EXECEND".into()];
        }
        if code.contains("boom") {
            return vec!["RESP|error|boom happened".into(), "EXECEND".into()];
        }
        return vec![
            "RESP|ok".into(),
            format!("OUT|= {}", code.chars().count()),
            "OUT|printed line".into(),
            "EXECEND".into(),
        ];
    }
    Vec::new()
}

fn deps() -> Deps {
    let mut f9_help = IndexMap::new();
    f9_help.insert(("s", false, false, false), "Speak speed");
    f9_help.insert(("f", true, false, false), "Fuel, with control");
    Deps {
        say_fn: Arc::new(|t, i| CALLS.lock().push(json!(["say", t, i]))),
        get_speech_log_fn: Arc::new(|_, _, _, _, _| SpeechLog {
            entries: vec![TapEntry {
                seq: 1,
                t: 0.0,
                text: "hello".into(),
                source: "test".into(),
                interrupt: false,
                excluded: false,
                spoken: true,
                reason: None,
            }],
            next_seq: 1,
            dropped: false,
        }),
        snapshot_state_fn: Arc::new(|sections| {
            let requested = sections.map(|s| {
                let mut v: Vec<&str> = s.to_vec();
                v.sort();
                v
            });
            json!({"liveness": {"world_active": WORLD.load(Ordering::SeqCst)},
                   "requested": requested})
        }),
        load_config_fn: Arc::new(|| {
            let Value::Object(m) = json!({
                "units": "imperial", "telemetry_protocol": "extended",
                "mcp_server_enabled": true, "mcp_server_port": 4481,
                "ai_describer_api_key": "dpapi:v1:c2VhbGVk",
                "ai_describer_openai_api_key": "",
            }) else {
                unreachable!()
            };
            m
        }),
        send_console_command_fn: Arc::new(|cmd| {
            CALLS.lock().push(json!(["console", cmd]));
            for rec in reply(cmd) {
                ms::console_tap(&rec);
            }
        }),
        send_udp_fn: Arc::new(|port, msg| {
            CALLS.lock().push(json!(["udp", port, msg]));
            Ok(())
        }),
        press_command_fn: Arc::new(|name, ctrl, shift, alt| {
            CALLS.lock().push(json!(["press", name, ctrl, shift, alt]));
            Ok(true)
        }),
        f9_help,
        world_active_fn: Arc::new(|| WORLD.load(Ordering::SeqCst)),
        capture_png_fn: Arc::new(|_, _, _| Err("not used".into())),
        road_diagnostic_fn: Arc::new(|action, label, session, note, limit| {
            CALLS.lock().push(json!(["road_diag", {
                "action": action, "label": label, "session": session,
                "note": note, "limit": limit,
            }]));
            Ok(json!({"active": false}))
        }),
        assistant_diagnostic_fn: Arc::new(|action, kwargs| {
            let mut kw = kwargs.as_object().cloned().unwrap_or_default();
            kw.insert("action".into(), json!(action));
            CALLS.lock().push(json!(["assistant_diag", kw]));
            Ok(json!({"recording": false}))
        }),
        version: "test".into(),
    }
}

/// The response as JSON, with the generator's `normalise` applied to tool text.
fn normalise(resp: &PyJson) -> Value {
    let text = pyjson::dumps(resp, &DumpOpts::default_seps()).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    if let Some(content) = v
        .get_mut("result")
        .and_then(|r| r.get_mut("content"))
        .and_then(Value::as_array_mut)
    {
        for c in content {
            if let Some(Value::String(s)) = c.get_mut("text") {
                *s = VOLATILE.replace_all(s, "${1}0").into_owned();
            }
        }
    }
    v
}

#[test]
fn dispatch_matches_python() {
    ms::reset_for_tests();
    ms::init(deps());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let fx = load("mcp_dispatch");
    let mut failures = Vec::new();
    for (i, case) in fx["cases"].as_array().unwrap().iter().enumerate() {
        WORLD.store(case["world"].as_bool().unwrap(), Ordering::SeqCst);
        CALLS.lock().clear();
        let msg = pyjson::loads(&case["message"].to_string()).unwrap();
        let mut got = serde_json::Map::new();
        got.insert("message".into(), case["message"].clone());
        got.insert("world".into(), case["world"].clone());
        match rt.block_on(ms::dispatch(&msg)) {
            Ok(Some(resp)) => {
                got.insert("response".into(), normalise(&resp));
            }
            Ok(None) => {
                got.insert("response".into(), Value::Null);
            }
            Err(e) => {
                got.insert("raised".into(), json!([e.type_name(), e.message()]));
            }
        }
        got.insert("calls".into(), Value::Array(CALLS.lock().clone()));
        let got = Value::Object(got);
        if &got != case {
            failures.push(format!(
                "case {i} {}:\n  got  {got}\n  want {case}",
                case["message"]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        fx["cases"].as_array().unwrap().len(),
        failures.join("\n")
    );
}
