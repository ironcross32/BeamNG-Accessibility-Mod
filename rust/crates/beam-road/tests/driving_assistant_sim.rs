//! diagnostic/driving_assistant_sim.py: session protocol checks without the GUI or
//! audio application.

use std::sync::{Arc, Mutex, OnceLock};

use beam_road::assistant_diagnostics::AssistantDiagnosticRecorder;
use beam_road::driving_assistant::{DrivingAssistant, SpeakOpts};
use beam_road::pyjson::{PyErr, Value};
use serde_json::json;

/// Captures `bnvdahook` error records, like `assertLogs("bnvdahook", level="ERROR")`.
struct Capture(Mutex<Vec<String>>);
impl log::Log for Capture {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target() == "bnvdahook" && m.level() <= log::Level::Error
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            self.0
                .lock()
                .unwrap()
                .push(format!("ERROR:bnvdahook:{}", r.args()));
        }
    }
    fn flush(&self) {}
}

fn capture() -> &'static Capture {
    static C: OnceLock<&'static Capture> = OnceLock::new();
    C.get_or_init(|| {
        let c: &'static Capture = Box::leak(Box::new(Capture(Mutex::new(Vec::new()))));
        let _ = log::set_logger(c);
        log::set_max_level(log::LevelFilter::Error);
        c
    })
}

struct Checks {
    sent: Arc<Mutex<Vec<String>>>,
    spoken: Arc<Mutex<Vec<String>>>,
    client: DrivingAssistant,
}

impl Checks {
    fn new() -> Self {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let spoken = Arc::new(Mutex::new(Vec::new()));
        let (a, b) = (sent.clone(), spoken.clone());
        let client = DrivingAssistant::new(
            move |t: &str| a.lock().unwrap().push(t.to_string()),
            move |t: &str, _: SpeakOpts| b.lock().unwrap().push(t.to_string()),
            None,
            None,
        );
        Checks {
            sent,
            spoken,
            client,
        }
    }

    fn response(&self, kind: &str, sequence: i64, token: Option<String>) {
        let token = token.or_else(|| self.client.token());
        let payload = json!({"token": token, "kind": kind, "sequence": sequence,
                             "vehicle": 42, "message": kind});
        self.client.receive(&payload.to_string()).unwrap();
    }

    fn spoken(&self) -> Vec<String> {
        self.spoken.lock().unwrap().clone()
    }
}

#[test]
fn test_activation_requires_confirmation() {
    let c = Checks::new();
    c.client.toggle();
    assert!(!c.client.active());
    assert!(c.spoken().is_empty());
    c.response("on", 1, None);
    assert!(c.client.active());
    assert_eq!(c.client.vehicle(), Value::Int(42));
}

#[test]
fn test_old_session_cannot_reactivate_or_speak() {
    let c = Checks::new();
    c.client.toggle();
    let old = c.client.token();
    c.response("off", 3, None);
    c.client.toggle();
    let count = c.spoken().len();
    c.response("on", 100, old.clone());
    c.response("say", 101, old);
    assert!(!c.client.active());
    assert_eq!(c.spoken().len(), count);
}

#[test]
fn test_duplicate_and_reordered_packets() {
    let c = Checks::new();
    c.client.toggle();
    c.response("on", 2, None);
    c.response("on", 2, None);
    c.response("say", 1, None);
    assert_eq!(c.spoken(), ["on"]);
    let token = c.client.token();
    c.response("off", 4, None);
    c.response("on", 3, token);
    assert!(!c.client.active());
}

#[test]
fn test_raw_errors_are_logged_never_spoken() {
    let cap = capture();
    let c = Checks::new();
    c.client.toggle();
    let raw = "lua/vehicle/extensions/beamtelAssistant.lua:223: attempt to index nil";
    c.client
        .receive(
            &json!({"token": c.client.token(), "kind": "error", "sequence": 1,
                         "message": "", "data": {"detail": raw}})
            .to_string(),
        )
        .unwrap();
    assert!(cap.0.lock().unwrap().iter().any(|l| l.contains(raw)));
    c.client
        .receive(
            &json!({"token": c.client.token(), "kind": "off", "sequence": 2,
                         "message": "Driving assistant fault. Steering assistance off"})
            .to_string(),
        )
        .unwrap();
    assert_eq!(
        c.spoken(),
        ["Driving assistant fault. Steering assistance off"]
    );
    assert!(!c.spoken().iter().any(|t| t.contains(".lua")));
}

#[test]
fn test_toggle_off_uses_current_token() {
    let c = Checks::new();
    c.client.toggle();
    let token = c.client.token().unwrap();
    c.response("on", 1, None);
    c.client.toggle();
    assert_eq!(
        c.sent.lock().unwrap().last().unwrap(),
        &format!("ASSISTANT_OFF:{token}")
    );
}

#[test]
fn test_malformed_response() {
    let c = Checks::new();
    c.client.toggle();
    for payload in [
        "bad",
        "null",
        "[]",
        r#"{"token":false}"#,
        r#"{"sequence":null}"#,
    ] {
        c.client.receive(payload).unwrap();
    }
    assert!(!c.client.active());
    assert!(c.spoken().is_empty());
}

#[test]
fn test_heartbeat_recovers_lost_confirmation() {
    let c = Checks::new();
    c.client.toggle();
    c.client
        .receive(
            &json!({"token": c.client.token(), "kind": "alive", "sequence": 2,
                         "active": true, "vehicle": 42})
            .to_string(),
        )
        .unwrap();
    assert!(c.client.active());
    assert_eq!(c.spoken().len(), 1);
}

#[test]
fn test_disabled_status_does_not_start_session() {
    let c = Checks::new();
    c.client.status();
    c.response("status", 0, c.client.status_token());
    assert_eq!(c.spoken(), ["status"]);
    assert!(!c.client.active());
    assert!(c.client.token().is_none());
}

#[test]
fn test_cues_are_ordered_and_disengagement_ignores_duplicate() {
    let c = Checks::new();
    let cues = Arc::new(Mutex::new(Vec::<(String, Vec<String>)>::new()));
    let k = cues.clone();
    c.client
        .set_cue(Arc::new(move |kind: &str, choices: &[String]| {
            k.lock().unwrap().push((kind.into(), choices.to_vec()))
        }));
    c.client.toggle();
    let token = c.client.token();
    c.response("on", 1, None);
    c.client
        .receive(
            &json!({"token": token, "kind": "junction", "sequence": 2, "message": "right,left"})
                .to_string(),
        )
        .unwrap();
    assert_eq!(
        *cues.lock().unwrap(),
        vec![(
            "junction".to_string(),
            vec!["left".to_string(), "right".to_string()]
        )]
    );
    assert_eq!(c.spoken(), ["on"]);
    c.response("off", 3, token.clone());
    c.response("off", 3, token);
    assert_eq!(
        cues.lock().unwrap().last().unwrap(),
        &("off".to_string(), vec![])
    );
    assert_eq!(cues.lock().unwrap().len(), 2);
}

#[test]
fn test_timeout_plays_off_once() {
    let c = Checks::new();
    let cues = Arc::new(Mutex::new(Vec::<String>::new()));
    let k = cues.clone();
    c.client.set_cue(Arc::new(move |kind: &str, _: &[String]| {
        k.lock().unwrap().push(kind.into())
    }));
    c.client.toggle();
    c.response("on", 1, None);
    c.client.set_last_response(c.client.last_response() - 6.0);
    c.client.heartbeat();
    c.client.heartbeat();
    assert_eq!(*cues.lock().unwrap(), ["off"]);
    assert!(!c.client.active());
}

#[test]
fn test_diagnostics_persist_and_stay_silent() {
    let dir = tempfile::tempdir().unwrap();
    let c = Checks::new();
    let recorder = Arc::new(AssistantDiagnosticRecorder::new(dir.path()));
    c.client.set_diagnostics(Some(recorder.clone()));
    recorder.start(Some("test")).unwrap();
    c.client.toggle();
    c.response("on", 1, None);
    c.client
        .receive(
            &json!({"token": c.client.token(), "kind": "diagnostic", "sequence": 2,
            "data": {"vehicle": {"locked": "j", "chosen": "left", "automatic": true}}})
            .to_string(),
        )
        .unwrap();
    assert_eq!(c.spoken(), ["on"]);
    let status = recorder.status();
    let chosen = status
        .get("latest")
        .unwrap()
        .get("vehicle")
        .unwrap()
        .get("chosen")
        .unwrap();
    assert!(chosen.eq_str("left"));
    let first = recorder.read(&Value::Int(0), &Value::Int(1)).unwrap();
    let next_seq = first.get("next_seq").unwrap().clone();
    let second = recorder.read(&next_seq, &Value::Int(100)).unwrap();
    let Some(Value::List(records)) = second.get("records") else {
        panic!()
    };
    let seq = records[0].get("seq").unwrap().py_int().unwrap();
    assert!(seq > next_seq.py_int().unwrap());
    recorder.stop();
    let review = recorder.review(None).unwrap();
    assert_eq!(review.get("samples"), Some(&Value::Int(1)));
    assert_eq!(
        review.get("automatic_choices_observed"),
        Some(&Value::Int(1))
    );
    assert!(matches!(
        recorder.review(Some("../escape.ndjson")),
        Err(PyErr::ValueError(_))
    ));
}
