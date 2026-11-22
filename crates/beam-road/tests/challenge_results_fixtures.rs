//! Golden cases from `fixtures/gen/challenge_results.py`.
//!
//! The local-time wording follows this machine's time zone, as the fixture did when it
//! was generated; regenerate the fixture on a machine in another zone.

use std::sync::{Arc, Mutex};

use beam_road::challenge_results::*;
use beam_road::pyjson::{DumpOpts, PyErr, Value, dumps, loads, py_eq};
use beam_road::pytime::DateTime;
use beam_testutil::{cases, load, s};
use serde_json::json;

fn v(text: &serde_json::Value) -> Value {
    loads(s(text)).unwrap()
}

fn from_serde(x: &serde_json::Value) -> Value {
    loads(&x.to_string()).unwrap()
}

fn check(failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn same_lines(got: &Result<Vec<String>, PyErr>, want: &serde_json::Value) -> bool {
    match (got, want.get("ok")) {
        (Ok(lines), Some(w)) => {
            let w: Vec<&str> = w.as_array().unwrap().iter().map(s).collect();
            lines.iter().map(String::as_str).collect::<Vec<_>>() == w
        }
        (Err(e), None) => e.type_name() == s(&want["err"]) && e.message() == s(&want["msg"]),
        _ => false,
    }
}

fn same_text(got: &Result<String, PyErr>, want: &serde_json::Value) -> bool {
    match (got, want.get("ok")) {
        (Ok(g), Some(w)) => g == s(w),
        (Err(e), None) => e.type_name() == s(&want["err"]) && e.message() == s(&want["msg"]),
        _ => false,
    }
}

#[test]
fn formatting_matches_python() {
    let fx = load("challenge_results");
    let mut failures = Vec::new();
    for case in cases(&fx, "format") {
        let val = v(&case["value"]);
        let x = Some(&val);
        for (got, key) in [
            (format_time(x), "time"),
            (format_elapsed_words(x), "words"),
            (format_speed(x, "metric"), "speed_metric"),
            (format_speed(x, "imperial"), "speed_imperial"),
            (format_speed(x, "Metric"), "speed_other"),
            (format_distance(x, "metric"), "dist_metric"),
            (format_distance(x, "imperial"), "dist_imperial"),
        ] {
            if got != s(&case[key]) {
                failures.push(format!(
                    "{key}({}) = {got:?}, want {}",
                    case["value"], case[key]
                ));
            }
        }
    }
    for case in cases(&fx, "datetime") {
        let val = v(&case["value"]);
        let got = format_local_datetime(Some(&val));
        if !same_text(&got, case) {
            failures.push(format!(
                "format_local_datetime({}) = {got:?}, want {case}",
                case["value"]
            ));
        }
    }
    for case in cases(&fx, "surface") {
        let (x, y) = (v(&case["x"]), v(&case["y"]));
        let got = surface_for_position(Some(&x), Some(&y));
        if got != s(&case["out"]) {
            failures.push(format!("surface({}, {}) = {got}", case["x"], case["y"]));
        }
    }
    for case in cases(&fx, "contacts") {
        let p = v(&case["packet"]);
        let got = contact_flags(Some(&p));
        let ok = match (&got, case.get("ok")) {
            (Ok(g), Some(w)) => py_eq(g, &from_serde(w)),
            (Err(e), None) => e.type_name() == s(&case["err"]) && e.message() == s(&case["msg"]),
            _ => false,
        };
        if !ok {
            failures.push(format!(
                "contact_flags({}) = {got:?}, want {case}",
                case["packet"]
            ));
        }
    }
    check(failures);
}

#[test]
fn analyze_attempt_matches_python() {
    let fx = load("challenge_results");
    let mut failures = Vec::new();
    for (n, case) in cases(&fx, "analyze").iter().enumerate() {
        let Value::List(samples) = v(&case["samples"]) else {
            panic!()
        };
        let Value::List(checkpoints) = v(&case["checkpoints"]) else {
            panic!()
        };
        let Value::Dict(event) = v(&case["event"]) else {
            panic!()
        };
        let got = analyze_attempt(&samples, &checkpoints, &event)
            .and_then(|r| dumps(&r, &DumpOpts::indented(2)));
        if !same_text(&got, case) {
            failures.push(format!(
                "case {n}: got {got:?}\nwant {}",
                case.get("ok").unwrap_or(case)
            ));
        }
    }
    check(failures);
}

#[test]
fn report_wording_matches_python() {
    let fx = load("challenge_results");
    let mut failures = Vec::new();
    for case in cases(&fx, "summaries") {
        let summary = v(&case["summary"]);
        let units = s(&case["units"]);
        let r = HillClimbChallengeRecorder::report_lines(&summary, units);
        if !same_lines(&r, &case["report"]) {
            failures.push(format!(
                "report_lines {}: {r:?}\nwant {}",
                case["summary"], case["report"]
            ));
        }
        let h = HillClimbChallengeRecorder::history_line(&summary, units);
        if !same_text(&h, &case["history"]) {
            failures.push(format!(
                "history_line {}: {h:?} want {}",
                case["summary"], case["history"]
            ));
        }
        let sp = completion_speech(&summary);
        if !same_text(&sp, &case["speech"]) {
            failures.push(format!(
                "completion_speech {}: {sp:?} want {}",
                case["summary"], case["speech"]
            ));
        }
    }
    check(failures);
}

#[test]
fn recorder_lifecycle_and_bytes_match_python() {
    let fx = load("challenge_results");
    let epoch = DateTime::fromisoformat("2026-09-04T00:00:00+00:00").unwrap();
    for (n, case) in cases(&fx, "recorder").iter().enumerate() {
        let tmp = tempfile::tempdir().unwrap();
        let esc = tmp.path().to_string_lossy().replace('\\', "\\\\");
        let clean = {
            let esc = esc.clone();
            move |t: String| t.replace(&esc, "<DIR>")
        };
        let micros = Arc::new(Mutex::new(0i64));
        let log: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let (m1, m2, l1) = (micros.clone(), micros.clone(), log.clone());
        let clean_cb = clean.clone();
        let rec = HillClimbChallengeRecorder::with_clocks(
            tmp.path(),
            move || 100.0 + *m1.lock().unwrap() as f64 / 1e6,
            move || epoch.plus_micros(*m2.lock().unwrap()),
            Some(Arc::new(move |summary: &Value, auto: bool| {
                l1.lock().unwrap().push(json!([
                    clean_cb(dumps(summary, &DumpOpts::default_seps()).unwrap()),
                    auto,
                    HillClimbChallengeRecorder::report_lines(summary, "metric").unwrap(),
                    HillClimbChallengeRecorder::history_line(summary, "imperial").unwrap(),
                    completion_speech(summary).unwrap(),
                ]))
            })),
        )
        .unwrap();
        for (i, op) in case["script"].as_array().unwrap().iter().enumerate() {
            *micros.lock().unwrap() = op["micros"].as_i64().unwrap();
            let start = log.lock().unwrap().len();
            match s(&op["op"]) {
                "record" => {
                    let got = rec.record(&v(&op["packet"]), &v(&op["telemetry"])).unwrap();
                    assert_eq!(got, op["out"].as_bool().unwrap(), "case {n} op {i}");
                }
                "event" => {
                    let tel = v(&op["telemetry"]);
                    let tel = (!tel.is_null()).then_some(&tel);
                    let got = rec.handle_event(&v(&op["event"]), tel).unwrap();
                    let got = got.map(|g| dumps(&g, &DumpOpts::default_seps()).unwrap());
                    assert_eq!(
                        got.unwrap_or_else(|| "null".into()),
                        s(&op["out"]),
                        "case {n} op {i}"
                    );
                }
                "shutdown" => rec.shutdown().unwrap(),
                other => panic!("{other}"),
            }
            let got_log = log.lock().unwrap()[start..].to_vec();
            assert_eq!(
                &got_log,
                op["log"].as_array().unwrap(),
                "case {n} op {i} log"
            );
            assert_eq!(
                rec.is_capturing(),
                op["capturing"].as_bool().unwrap(),
                "case {n} op {i}"
            );
        }
        let reports: Vec<String> = rec
            .list_reports(100)
            .iter()
            .map(|r| clean(dumps(r, &DumpOpts::default_seps()).unwrap()))
            .collect();
        let want: Vec<&str> = case["reports"].as_array().unwrap().iter().map(s).collect();
        assert_eq!(reports, want, "case {n} reports");
        let mut names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let files = case["files"].as_object().unwrap();
        assert_eq!(
            names,
            files.keys().cloned().collect::<Vec<_>>(),
            "case {n} names"
        );
        for name in names {
            let got = clean(std::fs::read_to_string(tmp.path().join(&name)).unwrap());
            assert_eq!(got, s(&files[&name]), "case {n} file {name}");
        }
    }
}
