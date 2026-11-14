//! Golden scripts from `rust/fixtures/gen/assistant_diagnostics.py`.

use std::sync::{Arc, Mutex};

use beam_road::assistant_diagnostics::AssistantDiagnosticRecorder;
use beam_road::pyjson::{DumpOpts, PyErr, Value, dumps, loads};
use beam_road::pytime::DateTime;
use beam_road::road_guidance::parse_r2_packet;
use beam_testutil::{cases, load, s};

#[test]
fn scripts_match_python() {
    let fx = load("assistant_diagnostics");
    let epoch = DateTime::fromisoformat("2026-09-04T03:04:05.678901+00:00").unwrap();
    for (n, script) in cases(&fx, "scripts").iter().enumerate() {
        let tmp = tempfile::tempdir().unwrap();
        let esc = tmp.path().to_string_lossy().replace('\\', "\\\\");
        let clean = |v: &Value| {
            dumps(v, &DumpOpts::default_seps())
                .unwrap()
                .replace(&esc, "<DIR>")
        };
        let clock = Arc::new(Mutex::new(500.0f64));
        let stamp = Arc::new(Mutex::new(0i64));
        let (c1, s1) = (clock.clone(), stamp.clone());
        let rec = AssistantDiagnosticRecorder::with_clocks(
            tmp.path(),
            move || *c1.lock().unwrap(),
            move || epoch.plus_micros(*s1.lock().unwrap()),
        );
        for (i, op) in script["ops"].as_array().unwrap().iter().enumerate() {
            *clock.lock().unwrap() = op["clock"].as_f64().unwrap();
            *stamp.lock().unwrap() = op["stamp"].as_i64().unwrap();
            let val = |key: &str| loads(s(&op[key])).unwrap();
            let res: Result<Value, PyErr> = match s(&op["op"]) {
                "start" => rec.start(op["label"].as_str()),
                "stop" => Ok(rec.stop()),
                "record" => {
                    let Value::Dict(d) = val("event") else {
                        panic!()
                    };
                    rec.record(&d);
                    Ok(Value::Null)
                }
                "record_road" => {
                    rec.record_road(&parse_r2_packet(s(&op["packet"])).unwrap(), val("audio"));
                    Ok(Value::Null)
                }
                "read" => rec.read(&val("since"), &val("limit")),
                "review" => rec.review(op["session"].as_str()),
                "control" => rec.control(
                    s(&op["action"]),
                    None,
                    None,
                    &val("note"),
                    &Value::Int(op["limit"].as_i64().unwrap_or(20)),
                    &Value::Int(0),
                ),
                other => panic!("{other}"),
            };
            let want = &op["res"];
            let ok = match (&res, want.get("ok")) {
                (Ok(v), Some(w)) => clean(v) == s(w),
                (Err(e), None) => {
                    let t = s(&want["err"]);
                    if t == "FileNotFoundError" {
                        matches!(e, PyErr::OSError(_))
                    } else {
                        e.type_name() == t && e.message() == s(&want["msg"])
                    }
                }
                _ => false,
            };
            assert!(
                ok,
                "script {n} op {i} {}: got {res:?}, want {want}",
                op["op"]
            );
            assert_eq!(
                clean(&rec.status()),
                s(&op["status"]),
                "script {n} op {i} status"
            );
        }
        rec.stop();
        let mut names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let files = script["files"].as_object().unwrap();
        assert_eq!(
            names,
            files.keys().cloned().collect::<Vec<_>>(),
            "script {n}"
        );
        for name in names {
            let got = std::fs::read_to_string(tmp.path().join(&name)).unwrap();
            assert_eq!(got, s(&files[&name]), "script {n} file {name}");
        }
    }
}
