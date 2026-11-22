//! diagnostic/road_diagnostic_sim.py: durable road-guidance capture, analysis, and
//! the Lua fields it depends on.

use std::sync::{Arc, Mutex};

use beam_road::pyjson::{Value, dict, loads};
use beam_road::road_diagnostics::RoadDiagnosticRecorder;
use serde_json::json;

fn v(j: serde_json::Value) -> Value {
    loads(&j.to_string()).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn packet(
    active: bool,
    settled: bool,
    candidate: bool,
    error: f64,
    lateral_speed: f64,
    steering_input: f64,
    applied_steering: Option<f64>,
    contact_materials: &str,
) -> Value {
    let applied = applied_steering.unwrap_or(steering_input * 0.9);
    v(json!({
        "state": "onRoad", "oneWay": false, "roadDirections": [0, -180], "offRoad": null, "junction": null,
        "correction": {"active": active, "bearing": if active {-12} else {0},
                       "severity": if active {json!(0.6)} else {json!(0)},
                       "phase": if active {"correct"} else {"idle"}, "settled": settled},
        "diagnostic": {
            "edgeId": "hill-a|hill-b", "edgeT": 0.5, "roadRadius": 7.5, "activeBefore": active || settled,
            "targetSide": 1, "targetError": error, "targetTolerance": 1.125,
            "settledHeadingTolerance": 6.0, "settledLateralSpeedTolerance": 0.6,
            "lateralSpeed": lateral_speed, "headingError": 2.0, "correctionBearing": -12.0,
            "withinLateral": error.abs() <= 1.125, "withinHeading": true,
            "withinLateralSpeed": lateral_speed.abs() <= 0.6, "settledCandidate": candidate,
            "steeringInput": steering_input, "steering": applied,
            "contactMaterials": contact_materials,
        },
    }))
}

fn p(active: bool, settled: bool, candidate: bool, error: f64, lateral_speed: f64) -> Value {
    packet(
        active,
        settled,
        candidate,
        error,
        lateral_speed,
        0.2,
        None,
        "10:ASPHALT,30:RUBBER",
    )
}

fn telemetry(x: i64, steering: f64, ground: f64, wheel: f64) -> Value {
    v(
        json!({"position": {"x": x, "y": 0, "z": 50}, "ground_speed_ms": ground,
             "wheel_speed_ms": wheel, "steering_input": steering, "pitch_rad": 0.4}),
    )
}

fn f(v: &Value, path: &[&str]) -> Value {
    let mut cur = v.clone();
    for k in path {
        cur = cur.get(k).cloned().unwrap_or(Value::Null);
    }
    cur
}

#[test]
fn main_scenario() {
    let clock = Arc::new(Mutex::new(100.0f64));
    let (c1, c2) = (clock.clone(), clock.clone());
    let tmp = tempfile::tempdir().unwrap();
    let recorder = RoadDiagnosticRecorder::with_clocks(
        tmp.path(),
        move || *c1.lock().unwrap(),
        move || *c2.lock().unwrap(),
    );
    let add = |d: f64| *clock.lock().unwrap() += d;
    let started = recorder.start(Some("hill / climb")).unwrap();
    assert_eq!(started.get("active"), Some(&Value::Bool(true)));
    let started_path = started.get("path").unwrap().as_str().unwrap().to_string();
    assert!(started_path.contains("hill-climb"));
    recorder.mark("starting uphill").unwrap();

    recorder
        .record(
            &p(true, false, false, 1.2, 0.5),
            &telemetry(0, 0.3, 10.0, 10.0),
            None,
        )
        .unwrap();
    add(0.05);
    recorder
        .record(
            &p(true, false, true, 0.2, 0.2),
            &telemetry(1, -0.3, 10.0, 10.0),
            None,
        )
        .unwrap();
    add(0.05);
    let settled_audio = dict([("settled_triggered", true.into())]);
    recorder
        .record(
            &p(false, true, true, 0.1, 0.1),
            &telemetry(2, 0.0, 10.0, 10.0),
            Some(&settled_audio),
        )
        .unwrap();

    // A second correction and settlement inside three seconds reproduces the
    // repeated-tone signature. Wheel speed running ahead of ground speed overlaps this
    // correction long enough to become a sustained wheelspin episode in the review.
    add(1.0);
    recorder
        .record(
            &packet(
                true,
                false,
                false,
                -1.4,
                0.5,
                0.2,
                Some(0.8),
                "10:ASPHALT,30:RUBBER",
            ),
            &telemetry(3, 0.4, 10.0, 15.0),
            None,
        )
        .unwrap();
    add(0.10);
    recorder
        .record(
            &packet(
                true,
                false,
                false,
                -0.7,
                0.5,
                0.2,
                Some(-0.8),
                "11:DIRT,30:RUBBER",
            ),
            &telemetry(4, -0.4, 10.0, 15.0),
            None,
        )
        .unwrap();
    add(0.10);
    recorder
        .record(
            &packet(
                true,
                false,
                false,
                -0.3,
                0.5,
                0.2,
                Some(0.8),
                "10:ASPHALT,30:RUBBER",
            ),
            &telemetry(5, 0.4, 10.0, 15.0),
            None,
        )
        .unwrap();
    add(0.05);
    recorder
        .record(
            &p(false, true, true, 0.1, 0.1),
            &telemetry(6, 0.0, 10.0, 10.0),
            Some(&settled_audio),
        )
        .unwrap();
    add(0.05);
    recorder
        .record(
            &p(false, false, false, 1.0, 0.5),
            &telemetry(7, 0.0, 10.0, 10.0),
            None,
        )
        .unwrap();

    let summary = recorder.stop().unwrap();
    assert_eq!(f(&summary, &["samples"]), Value::Int(8));
    assert_eq!(
        f(&summary, &["correction", "settled_audio_triggers_s"]),
        Value::List(vec![0.1.into(), 1.35.into()])
    );
    let Value::List(repeated) = f(&summary, &["correction", "repeated_settled"]) else {
        panic!()
    };
    assert_eq!(repeated[0].get("interval_s"), Some(&Value::Float(1.25)));
    let Value::List(episodes) = f(&summary, &["correction", "episodes"]) else {
        panic!()
    };
    assert_eq!(episodes.len(), 2);
    let Value::Int(run) = f(
        &summary,
        &["correction", "longest_settled_candidate_run_samples"],
    ) else {
        panic!()
    };
    assert!(run >= 2);
    assert_eq!(
        f(&summary, &["correction", "settled_band"]),
        dict([
            ("min_lateral_tolerance_m", 1.125.into()),
            ("max_lateral_tolerance_m", 1.125.into()),
            ("heading_tolerance_deg", 6.0.into()),
            ("min_lateral_speed_tolerance_mps", 0.6.into()),
            ("max_lateral_speed_tolerance_mps", 0.6.into()),
        ])
    );
    assert_eq!(
        f(
            &summary,
            &["correction", "steering_source_samples", "lua_vehicle"]
        ),
        Value::Int(8)
    );
    assert_eq!(
        f(&summary, &["correction", "steering_reversals"]),
        Value::Int(2)
    );
    assert_eq!(
        f(&summary, &["max_absolute", "applied_steering"]),
        Value::Float(0.8)
    );
    assert_eq!(
        f(
            &summary,
            &["contact_materials", "sample_sets", "11:DIRT,30:RUBBER"]
        ),
        Value::Int(1)
    );
    let Value::List(changes) = f(&summary, &["contact_materials", "changes"]) else {
        panic!()
    };
    assert_eq!(changes.len(), 2);
    let Value::List(slips) = f(&summary, &["slip", "episodes"]) else {
        panic!()
    };
    assert!(slips[0].get("kind").unwrap().eq_str("wheelspin"));
    assert_eq!(slips[0].get("during_correction"), Some(&Value::Bool(true)));
    let summary_path = f(&summary, &["summary_path"]);
    assert!(std::path::Path::new(summary_path.as_str().unwrap()).is_file());
    let reviewed = recorder.review(None).unwrap();
    assert_eq!(reviewed.get("samples"), summary.get("samples"));
    let Value::List(sessions) = recorder.list_sessions(&Value::Int(20)).unwrap() else {
        panic!()
    };
    assert_eq!(sessions[0].get("summary"), Some(&Value::Bool(true)));

    let text = std::fs::read_to_string(&started_path).unwrap();
    let rows: Vec<Value> = text.lines().map(|l| loads(l).unwrap()).collect();
    assert!(
        rows[0].get("kind").unwrap().eq_str("meta")
            && rows.last().unwrap().get("kind").unwrap().eq_str("end")
    );
    assert!(
        rows.iter()
            .any(|r| r.get("kind").is_some_and(|k| k.eq_str("marker")))
    );

    let lua = std::fs::read_to_string(
        beam_testutil::repo_root()
            .join("bng_mod")
            .join("lua")
            .join("ge")
            .join("extensions")
            .join("roadDetector.lua"),
    )
    .unwrap();
    for token in [
        r#"upper == "DIAG_ON""#,
        r#"upper == "DIAG_OFF""#,
        "diagnostic.activeBefore",
        "diagnostic.targetTolerance",
        "diagnostic.settledHeadingTolerance",
        "diagnostic.settledLateralSpeedTolerance",
        "diagnostic.settledCandidate",
        "diagnostic.clearTicksAfter",
        "diagnostic.lateralDistance",
        "diagnostic.correctionArmed",
        "diagnostic.steeringInput",
        "diagnostic.contactMaterials",
    ] {
        assert!(lua.contains(token), "{token}");
    }
}
