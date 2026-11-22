//! Golden cases from `fixtures/gen/drift_speech.py`.

mod common;

use std::sync::{Arc, Mutex};

use beam_reports::drift_speech::*;
use beam_reports::pyops::PyValue;
use beam_testutil::{cases, f, load, s};
use common::*;

#[test]
fn constant() {
    let fx = load("drift_speech");
    assert_eq!(QUIET_TIMEOUT_S, f(&fx["QUIET_TIMEOUT_S"]));
}

#[test]
fn drift_phrase_matches_python() {
    let fx = load("drift_speech");
    let all = cases(&fx, "drift_phrase");
    let mut failures = Vec::new();
    for case in all {
        let ev = dec(&case["event"]);
        let got = drift_phrase(&ev);
        let ok = matches_outcome(&got, case, true, |r, want| {
            let want = dec(want);
            match r {
                None => want == PyValue::None,
                Some((p, i)) => {
                    want == PyValue::List(vec![PyValue::Str(p.clone()), PyValue::Bool(*i)])
                }
            }
        });
        if !ok {
            failures.push(format!(
                "drift_phrase({}) = {got:?}, want {case}",
                case["event"]
            ));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn announcer_script_matches_python() {
    let fx = load("drift_speech");
    let now = Arc::new(Mutex::new(100.0f64));
    let spoken: Arc<Mutex<Vec<(String, bool, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let (n2, s2) = (now.clone(), spoken.clone());
    let ann = DriftAnnouncer::new(
        Box::new(move |t, i, src| s2.lock().unwrap().push((t.to_string(), i, src.to_string()))),
        true,
        Box::new(move || *n2.lock().unwrap()),
    );
    for (i, step) in cases(&fx, "announcer_script").iter().enumerate() {
        let before = spoken.lock().unwrap().len();
        match s(&step["op"]) {
            "quiet" => assert_eq!(
                ann.quiet(),
                step["out"].as_bool().unwrap(),
                "step {i}: {step}"
            ),
            "advance" => *now.lock().unwrap() += f(&step["arg"]),
            "set_enabled" => ann.set_enabled(step["arg"].as_bool().unwrap()),
            "handle" => {
                let got = ann.handle(&dec(&step["arg"]));
                let raised = step["raised"].as_str();
                assert_eq!(got.err().map(|e| e.kind.name()), raised, "step {i}: {step}");
            }
            other => panic!("unknown op {other}"),
        }
        let new: Vec<(String, bool, String)> = spoken.lock().unwrap()[before..].to_vec();
        let want: Vec<(String, bool, String)> = step["spoken"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    s(&w[0]).to_string(),
                    w[1].as_bool().unwrap(),
                    s(&w[2]).to_string(),
                )
            })
            .collect();
        assert_eq!(new, want, "step {i}: {step}");
    }
}
