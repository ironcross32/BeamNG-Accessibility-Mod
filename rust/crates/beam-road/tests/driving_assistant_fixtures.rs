//! Golden session scripts from `rust/fixtures/gen/driving_assistant.py`.

use std::sync::{Arc, Mutex};

use beam_road::driving_assistant::{DrivingAssistant, SpeakOpts};
use beam_road::pyjson::{DumpOpts, Value, dumps};
use beam_testutil::{cases, f, load, s, same_f64};
use serde_json::json;

type Log = Arc<Mutex<Vec<serde_json::Value>>>;

#[test]
fn session_scripts_match_python() {
    let fx = load("driving_assistant");
    let mut failures = Vec::new();
    for (n, script) in cases(&fx, "scripts").iter().enumerate() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let tokens: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let tok = {
            let tokens = tokens.clone();
            move |text: &str| {
                let mut t = text.to_string();
                for (i, v) in tokens.lock().unwrap().iter().enumerate() {
                    t = t.replace(v, &format!("<tok{}>", i + 1));
                }
                t
            }
        };
        let now = Arc::new(Mutex::new(1000.0f64));
        let units = s(&script["units"]).to_string();
        let (l1, l2, l3, l4) = (log.clone(), log.clone(), log.clone(), log.clone());
        let clock = now.clone();
        let client = DrivingAssistant::with_clock(
            move |text: &str| l1.lock().unwrap().push(json!(["send", text])),
            move |text: &str, o: SpeakOpts| {
                l2.lock()
                    .unwrap()
                    .push(json!(["speak", text, o.interrupt, o.exclude_from_buffer]))
            },
            Some(Arc::new(move || units.clone())),
            Some(Box::new(move |a: &Value| {
                l4.lock().unwrap().push(json!([
                    "arrival",
                    dumps(a, &DumpOpts::default_seps()).unwrap()
                ]))
            })),
            move || *clock.lock().unwrap(),
        );
        client.set_cue(Arc::new(move |kind: &str, choices: &[String]| {
            l3.lock().unwrap().push(json!(["cue", kind, choices]))
        }));
        let note = |v: Option<String>| {
            if let Some(v) = v.filter(|v| !v.is_empty()) {
                let mut t = tokens.lock().unwrap();
                if !t.contains(&v) {
                    t.push(v);
                }
            }
        };
        let mut old: Option<String> = None;
        for (i, op) in script["ops"].as_array().unwrap().iter().enumerate() {
            *now.lock().unwrap() = f(&op["now"]);
            let start = log.lock().unwrap().len();
            let mut error = serde_json::Value::Null;
            match s(&op["op"]) {
                "toggle" => {
                    if let Some(t) = client.token() {
                        old = Some(t);
                    }
                    client.toggle();
                    note(client.token());
                }
                "status" => {
                    client.status();
                    note(client.status_token());
                }
                "heartbeat" => client.heartbeat(),
                "raw" => {
                    client.receive(s(&op["payload"])).unwrap();
                }
                "receive" => {
                    let js = |v: Option<String>| match v {
                        Some(t) => format!("\"{t}\""),
                        None => "null".to_string(),
                    };
                    let text = s(&op["payload"])
                        .replace("\"{token}\"", &js(client.token()))
                        .replace("\"{status}\"", &js(client.status_token()))
                        .replace("\"{old}\"", &js(old.clone()));
                    if let Err(e) = client.receive(&text) {
                        error = json!([e.type_name(), e.message()]);
                    }
                }
                other => panic!("unknown op {other}"),
            }
            let got_log: Vec<serde_json::Value> = log.lock().unwrap()[start..]
                .iter()
                .map(|e| {
                    if e[0] == "send" {
                        json!(["send", tok(s(&e[1]))])
                    } else {
                        e.clone()
                    }
                })
                .collect();
            let snap = client.snapshot();
            let mut snap_value = snap.to_value();
            if let Value::Dict(d) = &mut snap_value {
                d.shift_remove("response_age_s");
            }
            let snap_text = dumps(&snap_value, &DumpOpts::default_seps()).unwrap();
            let mut bad = Vec::new();
            if got_log != *op["log"].as_array().unwrap() {
                bad.push(format!("log {got_log:?} want {}", op["log"]));
            }
            if op["op"] == "receive" && error != op["error"] {
                bad.push(format!("error {error} want {}", op["error"]));
            }
            if snap_text != s(&op["snapshot"]) {
                bad.push(format!("snapshot {snap_text} want {}", op["snapshot"]));
            }
            let age_ok = match (snap.response_age_s, op["response_age"].is_null()) {
                (None, true) => true,
                (Some(a), false) => same_f64(a, f(&op["response_age"])),
                _ => false,
            };
            let since_ok = match (
                client.seconds_since_arrival(),
                op["since_arrival"].is_null(),
            ) {
                (None, true) => true,
                (Some(a), false) => same_f64(a, f(&op["since_arrival"])),
                _ => false,
            };
            if !age_ok || !since_ok {
                bad.push("ages differ".to_string());
            }
            if tok(&client.token().unwrap_or_default()) != s(&op["token"])
                || tok(&client.status_token().unwrap_or_default()) != s(&op["status_token"])
            {
                bad.push("tokens differ".to_string());
            }
            if !bad.is_empty() {
                failures.push(format!(
                    "script {n} op {i} {}: {}",
                    op["op"],
                    bad.join("; ")
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
