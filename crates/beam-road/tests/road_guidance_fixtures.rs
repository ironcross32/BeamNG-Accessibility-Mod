//! Golden cases from `fixtures/gen/road_guidance.py`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use beam_road::pyjson::{DumpOpts, PyErr, Value, dict, dumps};
use beam_road::road_guidance::*;
use beam_testutil::{cases, f, load, s, same_f64};

fn check(failures: Vec<String>, total: usize) {
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        total,
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn py_dumps(v: &Value) -> String {
    dumps(v, &DumpOpts::default_seps()).unwrap()
}

/// `outcome()` comparison for a Result whose Ok is a string.
fn same_outcome<E: std::fmt::Display>(got: &Result<String, E>, case: &serde_json::Value) -> bool {
    match (got, case.get("ok"), case.get("msg")) {
        (Ok(g), Some(w), _) => g == s(w),
        (Err(e), None, Some(m)) => e.to_string() == s(m),
        _ => false,
    }
}

#[test]
fn constants() {
    let fx = load("road_guidance");
    assert_eq!(R2_PREFIX, s(&fx["constants"]["R2_PREFIX"]));
    assert!(same_f64(
        R2_STALE_SECONDS,
        f(&fx["constants"]["R2_STALE_SECONDS"])
    ));
}

#[test]
fn parse_r2_packet_matches_python() {
    let fx = load("road_guidance");
    let all = cases(&fx, "packets");
    let mut failures = Vec::new();
    for case in all {
        let text = s(&case["text"]);
        let got = parse_r2_packet(text);
        let ok = match (&got, case.get("ok")) {
            (Ok(p), Some(want)) => py_dumps(&p.to_value()) == s(want),
            (Err(e), None) => e.type_name() == s(&case["err"]) && e.message() == s(&case["msg"]),
            _ => false,
        };
        if !ok {
            let got = got.map(|p| py_dumps(&p.to_value()));
            failures.push(format!("{text:?}: got {got:?}, want {case}"));
        }
    }
    check(failures, all.len());
}

#[test]
fn direction_labels_and_lists_match_python() {
    let fx = load("road_guidance");
    let mut failures = Vec::new();
    for case in cases(&fx, "direction_label") {
        let b = f(&case["bearing"]);
        let got = direction_label(b, case["u_turn"].as_bool().unwrap());
        if !same_outcome(&got, case) {
            failures.push(format!("direction_label({b:?}) = {got:?}, want {case}"));
        }
        // The private copy in beam-base must agree with the canonical one.
        let copy = beam_base::route_beacon::direction_label_for_tests(
            b,
            case["u_turn"].as_bool().unwrap(),
        );
        if copy != got {
            failures.push(format!(
                "route_beacon copy disagrees at {b:?}: {copy:?} vs {got:?}"
            ));
        }
    }
    for case in cases(&fx, "lists") {
        let bs: Vec<f64> = case["bearings"].as_array().unwrap().iter().map(f).collect();
        let conj = s(&case["conjunction"]);
        let a = direction_list(&bs, conj);
        let b = road_direction_list(&bs, conj);
        if !same_outcome(&a, &case["direction_list"])
            || !same_outcome(&b, &case["road_direction_list"])
        {
            failures.push(format!("lists {bs:?} {conj}: {a:?} / {b:?}, want {case}"));
        }
    }
    for case in cases(&fx, "one_way") {
        let b = f(&case["bearing"]);
        if one_way_phrase(b) != s(&case["out"]) {
            failures.push(format!("one_way_phrase({b:?}) = {:?}", one_way_phrase(b)));
        }
    }
    for case in cases(&fx, "format_road_distance") {
        let m = f(&case["metres"]);
        let u = s(&case["units"]);
        if format_road_distance(m, u) != s(&case["out"]) {
            failures.push(format!("format_road_distance({m:?}, {u:?})"));
        }
    }
    check(failures, 1);
}

fn junction_from(v: &serde_json::Value) -> Junction {
    Junction {
        id: s(&v["id"]).to_string(),
        phase: s(&v["phase"]).to_string(),
        entered: v["entered"].as_bool().unwrap(),
        kind: s(&v["kind"]).to_string(),
        distance: f(&v["distance"]),
        exits: v["exits"].as_array().unwrap().iter().map(f).collect(),
    }
}

fn signal_from(v: &serde_json::Value) -> Signal {
    Signal {
        id: s(&v["id"]).to_string(),
        kind: s(&v["kind"]).to_string(),
        state: s(&v["state"]).to_string(),
        phase: s(&v["phase"]).to_string(),
        distance: f(&v["distance"]),
        group_id: s(&v["groupId"]).to_string(),
        preview: v["preview"].as_bool().unwrap(),
    }
}

#[test]
fn phrases_match_python() {
    let fx = load("road_guidance");
    let mut failures = Vec::new();
    for case in cases(&fx, "junction_phrase") {
        let j = junction_from(&case["junction"]);
        let got = junction_phrase(&j, s(&case["units"]));
        if !same_outcome(&got, case) {
            failures.push(format!("junction_phrase({j:?}) = {got:?}, want {case}"));
        }
    }
    for case in cases(&fx, "signal_phrase") {
        let sig = signal_from(&case["signal"]);
        let got = signal_phrase(&sig, s(&case["units"]), case["change"].as_bool().unwrap());
        if got != s(&case["out"]) {
            failures.push(format!("signal_phrase({sig:?}) = {got:?}, want {case}"));
        }
    }
    for case in cases(&fx, "surface") {
        let text = s(&case["text"]);
        let got = parse_surface_packet(text);
        let want = case["out"]
            .as_array()
            .map(|a| (s(&a[0]).to_string(), a[1].as_bool().unwrap()));
        if got != want {
            failures.push(format!(
                "parse_surface_packet({text:?}) = {got:?}, want {want:?}"
            ));
        }
        if let Some((w, r)) = &got
            && surface_phrase(w, *r) != if *r { format!("{w} road") } else { w.clone() }
        {
            failures.push(format!("surface_phrase({w:?})"));
        }
    }
    for case in cases(&fx, "speed_limit_phrase") {
        let v = case["speed"].as_object().map(|_| f(&case["speed"]));
        let got = map_speed_limit_phrase(v, s(&case["units"]), s(&case["label"]));
        if got != s(&case["out"]) {
            failures.push(format!(
                "map_speed_limit_phrase({v:?}) = {got:?}, want {case}"
            ));
        }
    }
    check(failures, 1);
}

fn events_value(e: &R2Events) -> Value {
    dict([
        ("orientation", e.orientation.into()),
        (
            "junction",
            e.junction
                .as_ref()
                .map(|j| j.to_value())
                .unwrap_or(Value::Null),
        ),
        (
            "signal",
            match &e.signal {
                Some(ev) => {
                    let Value::Dict(mut d) = ev.signal.to_value() else {
                        unreachable!()
                    };
                    d.insert("change".into(), ev.change.into());
                    Value::Dict(d)
                }
                None => Value::Null,
            },
        ),
        (
            "speedLimit",
            match &e.speed_limit {
                Some(sl) => dict([("value", sl.value.into())]),
                None => Value::Null,
            },
        ),
    ])
}

#[test]
fn feed_script_matches_python() {
    let fx = load("road_guidance");
    let all = cases(&fx, "feed");
    let now = Arc::new(AtomicU64::new(0f64.to_bits()));
    let clock_now = now.clone();
    let feed =
        RoadGuidanceFeed::with_clock(move || f64::from_bits(clock_now.load(Ordering::SeqCst)));
    let mut failures = Vec::new();
    for (n, op) in all.iter().enumerate() {
        let t = f(&op["now"]);
        now.store(t.to_bits(), Ordering::SeqCst);
        match s(&op["op"]) {
            "accept_r2" => {
                let packet = parse_r2_packet(s(&op["text"])).expect("pool packets parse");
                let flags: Vec<bool> = op["flags"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|b| b.as_bool().unwrap())
                    .collect();
                let explicit = op["explicit"].as_bool().unwrap();
                let ev =
                    feed.accept_r2(&packet, explicit.then_some(t), flags[0], flags[1], flags[2]);
                let got = py_dumps(&events_value(&ev));
                if got != s(&op["out"]) {
                    failures.push(format!("op {n} accept_r2: got {got}, want {}", op["out"]));
                }
            }
            "accept_legacy" => {
                let dirs: Vec<f64> = op["directions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap())
                    .collect();
                let got = feed.accept_legacy(
                    s(&op["state"]),
                    f(&op["bearing"]),
                    f(&op["distance"]),
                    &dirs,
                    None,
                );
                if got != op["out"].as_bool().unwrap() {
                    failures.push(format!("op {n} accept_legacy: got {got}"));
                }
            }
            "check_timeout" => {
                let got = feed.check_timeout(None);
                if got != op["out"].as_bool().unwrap() {
                    failures.push(format!("op {n} check_timeout: got {got}"));
                }
            }
            "r2_recent" => {
                let got = feed.r2_recent(None);
                if got != op["out"].as_bool().unwrap() {
                    failures.push(format!("op {n} r2_recent: got {got}"));
                }
            }
            "arm_orientation" => feed.arm_orientation(),
            "reset" => feed.reset(),
            "status" => {
                let enabled = op["enabled"].as_bool().unwrap();
                let units = s(&op["units"]);
                let st = feed.status_phrase(enabled, units);
                if !same_outcome(&st, &op["status"]) {
                    failures.push(format!("op {n} status: got {st:?}, want {}", op["status"]));
                }
                let sp = feed.speed_limit_phrase(enabled, units, "Map speed limit");
                if sp != s(&op["speed"]) {
                    failures.push(format!("op {n} speed: got {sp:?}, want {}", op["speed"]));
                }
                if feed.snapshot().mode != s(&op["mode"]) {
                    failures.push(format!("op {n} mode: got {}", feed.snapshot().mode));
                }
            }
            other => panic!("unknown op {other}"),
        }
    }
    check(failures, all.len());
}

#[test]
fn parse_errors_keep_python_types() {
    assert!(matches!(
        parse_r2_packet(r#"R2|{"state":["onRoad"]}"#),
        Err(PyErr::TypeError(_))
    ));
}
