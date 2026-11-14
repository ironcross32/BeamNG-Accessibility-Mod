//! Golden cases from `rust/fixtures/gen/road_diagnostics.py`: summaries, and the
//! exact bytes a recorded session leaves on disk.

use std::sync::{Arc, Mutex};

use beam_road::pyjson::{DumpOpts, Value, dumps, loads};
use beam_road::pytime::DateTime;
use beam_road::road_diagnostics::*;
use beam_testutil::{cases, f, load, s, same_f64};

#[test]
fn constants() {
    let fx = load("road_diagnostics");
    let c = &fx["constants"];
    assert_eq!(SCHEMA_VERSION, c["SCHEMA_VERSION"].as_i64().unwrap());
    for (v, k) in [
        (REPEATED_SETTLED_WINDOW_S, "REPEATED_SETTLED_WINDOW_S"),
        (STEERING_DEADZONE, "STEERING_DEADZONE"),
        (BEARING_DEADZONE_DEG, "BEARING_DEADZONE_DEG"),
        (SLIP_SUSTAIN_S, "SLIP_SUSTAIN_S"),
    ] {
        assert!(same_f64(v, f(&c[k])), "{k}");
    }
}

#[test]
fn analyze_records_matches_python() {
    let fx = load("road_diagnostics");
    let all = cases(&fx, "analyze");
    let mut failures = Vec::new();
    for (n, case) in all.iter().enumerate() {
        let records: Vec<Value> = case["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| loads(s(l)).unwrap())
            .collect();
        let got = analyze_records(&records, Some("session.ndjson"))
            .and_then(|v| dumps(&v, &DumpOpts::indented(2).allow_nan(false)));
        let ok = match (&got, case.get("ok")) {
            (Ok(g), Some(w)) => g == s(w),
            (Err(e), None) => e.type_name() == s(&case["err"]) && e.message() == s(&case["msg"]),
            _ => false,
        };
        if !ok {
            failures.push(format!(
                "case {n}: got\n{}\nwant\n{}",
                got.unwrap_or_else(|e| format!("{e:?}")),
                case.get("ok")
                    .map(|v| s(v).to_string())
                    .unwrap_or_else(|| case.to_string())
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} failed:\n{}",
        failures.len(),
        all.len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn fixed_utc() -> DateTime {
    DateTime::fromisoformat("2026-09-04T12:30:05.123456+00:00").unwrap()
}

fn fixed_local() -> DateTime {
    DateTime::fromisoformat("2026-09-04T08:30:05.123456").unwrap()
}

#[test]
fn recorder_bytes_match_python() {
    let fx = load("road_diagnostics");
    for (n, case) in cases(&fx, "recorder").iter().enumerate() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().into_owned();
        let escaped = dir.replace('\\', "\\\\");
        let clock = Arc::new(Mutex::new(f(&case["clock0"])));
        let wall = Arc::new(Mutex::new(f(&case["wall0"])));
        let (c1, w1) = (clock.clone(), wall.clone());
        let rec = RoadDiagnosticRecorder::with_clocks(
            tmp.path(),
            move || *c1.lock().unwrap(),
            move || *w1.lock().unwrap(),
        )
        .with_datetimes(fixed_utc, fixed_local);
        let show = |v: &Value| {
            dumps(v, &DumpOpts::default_seps())
                .unwrap()
                .replace(&escaped, "<DIR>")
        };
        for (i, op) in case["script"].as_array().unwrap().iter().enumerate() {
            let what = s(&op["op"]);
            *clock.lock().unwrap() = f(&op["clock"]);
            *wall.lock().unwrap() = f(&op["wall"]);
            match what {
                "start" => {
                    let label = s(&op["label"]);
                    let out = rec.start(Some(label)).unwrap();
                    assert_eq!(show(&out), s(&op["out"]), "case {n} op {i}");
                }
                "mark" => {
                    let out = rec.mark(s(&op["note"])).unwrap();
                    assert_eq!(show(&out), s(&op["out"]), "case {n} op {i}");
                }
                "record" => {
                    let road = loads(s(&op["road"])).unwrap();
                    let tel = loads(s(&op["telemetry"])).unwrap();
                    let audio = loads(s(&op["audio"])).unwrap();
                    let out = rec.record(&road, &tel, Some(&audio)).unwrap();
                    assert_eq!(out, op["out"].as_bool().unwrap(), "case {n} op {i}");
                }
                "stop" => {
                    let out = rec.stop().unwrap();
                    assert_eq!(show(&out), s(&op["out"]), "case {n} op {i}");
                }
                other => panic!("{other}"),
            }
        }
        let mut names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let want = case["files"].as_object().unwrap();
        assert_eq!(
            names,
            want.keys().cloned().collect::<Vec<_>>(),
            "case {n} file names"
        );
        for name in names {
            let got = std::fs::read_to_string(tmp.path().join(&name))
                .unwrap()
                .replace(&escaped, "<DIR>");
            assert_eq!(got, s(&want[&name]), "case {n} file {name}");
        }
        assert!(rec.review(None).is_ok());
        let listed = rec.list_sessions(&Value::Int(20)).unwrap();
        assert!(matches!(&listed, Value::List(l) if l.len() == 1));
    }
}
