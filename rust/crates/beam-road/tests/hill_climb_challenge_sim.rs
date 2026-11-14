//! diagnostic/hill_climb_challenge_sim.py: lifecycle and aggregation checks for
//! hill-climb reports.

use std::sync::{Arc, Mutex};

use beam_road::challenge_results::{HillClimbChallengeRecorder, completion_speech};
use beam_road::pyjson::{Value, dict, loads};
use beam_road::pytime::DateTime;
use serde_json::json;

fn v(j: serde_json::Value) -> Value {
    loads(&j.to_string()).unwrap()
}

fn packet(state: &str, materials: &str) -> Value {
    v(
        json!({"state": state, "diagnostic": {"contactMaterials": materials}, "correction": {"active": false}}),
    )
}

#[allow(clippy::too_many_arguments)]
fn telemetry(x: f64, y: f64, speed: f64, wheel: f64, throttle: f64, brake: f64, tc: bool) -> Value {
    v(
        json!({"position": {"x": x, "y": y, "z": 50.0 + (y + 780.0) * 0.04}, "speed_ms": speed,
             "ground_speed_ms": speed, "wheel_speed_ms": wheel, "throttle": throttle,
             "brake": brake, "steering": 0.1, "traction_control_active": tc}),
    )
}

fn t(x: f64, y: f64, speed: f64, wheel: f64) -> Value {
    telemetry(x, y, speed, wheel, 0.5, 0.0, false)
}

fn get(v: &Value, path: &[&str]) -> Value {
    let mut cur = v.clone();
    for k in path {
        cur = cur.get(k).cloned().unwrap_or(Value::Null);
    }
    cur
}

#[test]
fn main_scenario() {
    let clock = Arc::new(Mutex::new(100.0f64));
    let epoch = DateTime::fromisoformat("2026-09-04T00:00:00+00:00").unwrap();
    let completed: Arc<Mutex<Vec<(Value, bool)>>> = Arc::new(Mutex::new(Vec::new()));
    let (c1, c2, done) = (clock.clone(), clock.clone(), completed.clone());
    let tmp = tempfile::tempdir().unwrap();
    let recorder = HillClimbChallengeRecorder::with_clocks(
        tmp.path(),
        move || *c1.lock().unwrap(),
        move || epoch.plus_micros(((*c2.lock().unwrap() - 100.0) * 1e6).round_ties_even() as i64),
        Some(Arc::new(move |summary: &Value, auto: bool| {
            done.lock().unwrap().push((summary.clone(), auto))
        })),
    )
    .unwrap();
    let add = |d: f64| *clock.lock().unwrap() += d;
    let event = |e: serde_json::Value, tel: Option<Value>| {
        recorder.handle_event(&v(e), tel.as_ref()).unwrap()
    };

    let action = event(
        json!({"type": "challenge_event", "challenge_id": "hill_climb",
                              "attempt_id": "sim/one", "event": "started"}),
        None,
    );
    assert_eq!(action, Some(dict([("capture", true.into())])));

    recorder
        .record(&packet("onRoad", "10:ASPHALT"), &t(0.0, -780.0, 8.0, 8.0))
        .unwrap();
    add(0.05);
    recorder
        .record(&packet("onRoad", "10:ASPHALT"), &t(0.0, -779.5, 10.0, 15.0))
        .unwrap();
    add(0.05);
    recorder
        .record(
            &packet("onRoad", "20:RUMBLE_STRIP"),
            &telemetry(8.0, -779.0, 12.0, 17.0, 0.5, 0.0, true),
        )
        .unwrap();
    add(0.05);
    recorder
        .record(
            &packet("onRoad", "30:GRAVEL"),
            &telemetry(11.0, -778.5, 10.0, 5.0, 0.0, 0.8, false),
        )
        .unwrap();
    add(0.50); // deliberately rejected packet gap
    recorder
        .record(&packet("offRoad", "10:ASPHALT"), &t(15.0, -778.0, 0.0, 0.0))
        .unwrap();

    event(
        json!({"type": "challenge_event", "challenge_id": "hill_climb", "attempt_id": "sim/one",
               "event": "checkpoint", "checkpoint_index": 1, "checkpoint_name": "Checkpoint 1",
               "race_time_s": 0.65}),
        Some(t(15.0, -778.0, 14.0, 10.0)),
    );
    assert_eq!(
        event(
            json!({"type": "challenge_event", "challenge_id": "hill_climb", "attempt_id": "sim/one",
                     "event": "race_complete", "race_time_s": 1.0, "recovery_count": 1}),
            None
        ),
        Some(dict([("capture", false.into())]))
    );
    event(
        json!({"type": "challenge_event", "challenge_id": "hill_climb", "attempt_id": "sim/one",
               "event": "attempt_aggregated", "official_time_s": 6.0, "raw_time_s": 1.0,
               "penalty_s": 5.0, "recovery_count": 1, "new_best": true}),
        None,
    );

    let (summary, auto) = {
        let c = completed.lock().unwrap();
        assert!(c.len() == 1 && c[0].1);
        c[0].clone()
    };
    assert!(auto);
    let stats = get(&summary, &["statistics"]);
    assert!(get(&summary, &["status"]).eq_str("completed"));
    assert_eq!(get(&stats, &["official_time_s"]), Value::Float(6.0));
    assert_eq!(get(&stats, &["penalty_s"]), Value::Float(5.0));
    assert_eq!(get(&stats, &["samples"]), Value::Int(5));
    assert_eq!(get(&stats, &["packet_gaps", "count"]), Value::Int(1));
    assert_eq!(
        get(&stats, &["durations", "off_asphalt_s"]),
        Value::Float(0.1)
    );
    assert_eq!(
        get(&stats, &["durations", "rumble_contact_s"]),
        Value::Float(0.05)
    );
    assert_eq!(
        get(&stats, &["durations", "gravel_contact_s"]),
        Value::Float(0.05)
    );
    assert_eq!(
        get(&stats, &["durations", "wheelspin_s"]),
        Value::Float(0.1)
    );
    assert_eq!(get(&stats, &["durations", "lockup_s"]), Value::Float(0.05));
    assert_eq!(
        get(&stats, &["departures"]),
        dict([("left", 0.into()), ("right", 1.into()), ("total", 1.into())])
    );
    let Value::List(cps) = get(&stats, &["checkpoints"]) else {
        panic!()
    };
    assert_eq!(cps[0].get("speed_mps"), Some(&Value::Float(14.0)));
    assert_eq!(get(&stats, &["recoveries"]), Value::Int(1));
    assert!(get(&stats, &["official_confirmed"]).truthy());
    assert!(get(&summary, &["personal_best", "new_best"]).truthy());
    assert!(
        completion_speech(&summary)
            .unwrap()
            .contains("New personal best")
    );
    let report_lines = HillClimbChallengeRecorder::report_lines(&summary, "imperial").unwrap();
    assert!(
        report_lines
            .iter()
            .any(|l| l.starts_with("Started, ") && l.contains(" at "))
    );
    assert!(
        report_lines
            .iter()
            .any(|l| l.starts_with("Finished, ") && l.contains(" at "))
    );
    assert!(report_lines.iter().any(|l| l.starts_with("Average speed")));
    assert!(
        report_lines
            .iter()
            .any(|l| l.starts_with("Time on asphalt"))
    );
    assert!(
        report_lines
            .iter()
            .any(|l| l.starts_with("Navigation on-road time"))
    );
    assert!(!report_lines.iter().any(|l| l.starts_with("Data quality")));
    assert!(
        report_lines
            .iter()
            .any(|l| l.starts_with("Checkpoint 1 reached at 0.650 seconds into the run"))
    );
    let history_line = HillClimbChallengeRecorder::history_line(&summary, "imperial").unwrap();
    assert!(!history_line.contains("sim/one") && !history_line.contains("2026-09-04T"));

    let raw_path = get(&summary, &["raw_path"]);
    let text = std::fs::read_to_string(raw_path.as_str().unwrap()).unwrap();
    let raw_rows: Vec<Value> = text.lines().map(|l| loads(l).unwrap()).collect();
    let samples: Vec<&Value> = raw_rows
        .iter()
        .filter(|r| r.get("kind").is_some_and(|k| k.eq_str("sample")))
        .collect();
    assert_eq!(samples.len(), 5);
    assert!(
        samples
            .iter()
            .all(|r| r.get("telemetry").unwrap().get("ground_speed_ms").is_some())
    );
    assert!(raw_rows[0].get("kind").unwrap().eq_str("meta"));
    assert!(raw_rows.last().unwrap().get("kind").unwrap().eq_str("end"));
    assert!(std::path::Path::new(get(&summary, &["summary_path"]).as_str().unwrap()).is_file());
    assert!(
        recorder.list_reports(100)[0]
            .get("attempt_id")
            .unwrap()
            .eq_str("sim/one")
    );

    // BeamNG starts the race once while staging and again when the countdown ends. The
    // second event replaces that short trace without polluting report history.
    let completed_count = completed.lock().unwrap().len();
    event(
        json!({"type": "challenge_event", "challenge_id": "hill_climb",
                 "attempt_id": "sim-staging", "event": "started"}),
        None,
    );
    recorder
        .record(
            &packet("onRoad", "10:ASPHALT"),
            &telemetry(0.0, -796.0, 0.0, 10.0, 0.0, 1.0, false),
        )
        .unwrap();
    let staging_path = recorder.attempt_raw_path().unwrap();
    event(
        json!({"type": "challenge_event", "challenge_id": "hill_climb",
                 "attempt_id": "sim-two", "event": "started", "replace_active": true}),
        None,
    );
    assert_eq!(completed.lock().unwrap().len(), completed_count);
    assert!(!staging_path.exists());

    // An actual abort retains the timed trace but does not auto-open it.
    recorder
        .record(&packet("onRoad", "10:ASPHALT"), &t(0.0, -780.0, 10.0, 10.0))
        .unwrap();
    event(
        json!({"type": "challenge_event", "challenge_id": "hill_climb",
                 "attempt_id": "sim-two", "event": "aborted"}),
        None,
    );
    let c = completed.lock().unwrap();
    let last = c.last().unwrap();
    assert!(get(&last.0, &["status"]).eq_str("aborted") && !last.1);
}
