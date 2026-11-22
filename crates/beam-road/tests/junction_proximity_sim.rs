//! diagnostic/junction_proximity_sim.py, the parts that exercise this crate: the
//! `proximity` wire validation in `parse_r2_packet`, and
//! `AssistantDiagnosticRecorder.record_road`'s throttle.
//!
//! Not ported here (other owners): the stop-bar rendering block, which drives
//! `audio.AudioController` (`update_junction_proximity`, `_mix_junction_proximity`,
//! the stop-bar envelope and modes), and the `_mcp_snapshot_state` block, which runs
//! a `beamtel.py` function against the feed and the audio controller.

use std::io::Write;
use std::sync::{Arc, Mutex};

use beam_road::assistant_diagnostics::AssistantDiagnosticRecorder;
use beam_road::pyjson::{Value, dict, loads};
use beam_road::pytime::DateTime;
use beam_road::road_guidance::{R2Packet, parse_r2_packet};
use serde_json::{Value as J, json};

fn target() -> J {
    json!({"id": "junction:T", "distance": 120, "speed": 20,
           "action": "approach", "source": "junctionBoundary"})
}

fn with(base: J, extra: J) -> J {
    let mut v = base;
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

fn parse(value: J, state: &str) -> Result<R2Packet, beam_road::pyjson::PyErr> {
    parse_r2_packet(&format!(
        "R2|{}",
        json!({"state": state, "proximity": value})
    ))
}

fn prox_value(p: &R2Packet) -> Value {
    p.proximity.as_ref().unwrap().to_value()
}

fn v(j: J) -> Value {
    loads(&j.to_string()).unwrap()
}

#[derive(Clone)]
struct Buffer(Arc<Mutex<Vec<u8>>>);
impl Write for Buffer {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn wire_validation() {
    // Old mods: the optional tails take their defaults. Python compares dicts, where
    // 120 == 120.0.
    let old = prox_value(&parse(target(), "onRoad").unwrap());
    let want = with(
        target(),
        json!({"signedDistance": 120, "kind": "", "privateEntry": ""}),
    );
    assert!(beam_road::pyjson::py_eq(&old, &v(want)), "old mods");
    let passed = with(
        target(),
        json!({"distance": 0, "signedDistance": -1.5, "kind": "gate", "privateEntry": ""}),
    );
    assert!(beam_road::pyjson::py_eq(
        &prox_value(&parse(passed.clone(), "onRoad").unwrap()),
        &v(passed)
    ));
    let entering = with(
        target(),
        json!({"kind": "junction", "privateEntry": "private:g2"}),
    );
    assert_eq!(
        parse(entering, "onRoad")
            .unwrap()
            .proximity
            .unwrap()
            .private_entry,
        "private:g2"
    );
    assert!(parse(J::Null, "onRoad").unwrap().proximity.is_none());
    for bad in [
        json!([]),
        with(target(), json!({"distance": -1})),
        with(target(), json!({"speed": "NaN-literal"})),
        with(target(), json!({"distance": true})),
        with(target(), json!({"action": "stop"})),
        with(target(), json!({"id": ""})),
        with(target(), json!({"source": "unknown"})),
        with(target(), json!({"signedDistance": "near"})),
        with(target(), json!({"signedDistance": -80})),
        with(target(), json!({"kind": 5})),
        with(target(), json!({"privateEntry": 7})),
    ] {
        let text = format!("R2|{}", json!({"state": "onRoad", "proximity": bad}))
            .replace("\"NaN-literal\"", "NaN");
        assert!(
            parse_r2_packet(&text).is_err(),
            "accepted malformed proximity {bad}"
        );
    }
    assert!(
        parse(target(), "offRoad").is_err(),
        "accepted off-road target"
    );
}

#[test]
fn assistant_recorder_throttles_road_samples() {
    let now = Arc::new(Mutex::new(100.0f64));
    let n = now.clone();
    let recorder = AssistantDiagnosticRecorder::with_clocks(
        "unused",
        move || *n.lock().unwrap(),
        DateTime::utc_now,
    );
    let target_packet = parse(target(), "onRoad").unwrap();
    recorder.record_road(&target_packet, dict([("active", true.into())]));
    assert_eq!(
        recorder.sequence(),
        0,
        "no recording without an observation session"
    );
    let buffer = Buffer(Arc::new(Mutex::new(Vec::new())));
    recorder.set_stream(Some(Box::new(buffer.clone())));
    recorder.record_road(&target_packet, dict([("active", true.into())]));
    recorder.record_road(&target_packet, dict([("active", true.into())]));
    *now.lock().unwrap() = 100.21;
    recorder.record_road(&target_packet, dict([("active", false.into())]));
    let text = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    let rows: Vec<Value> = text.lines().map(|l| loads(l).unwrap()).collect();
    assert_eq!(rows.len(), 2);
    let distance = rows[0]
        .get("data")
        .unwrap()
        .get("proximity")
        .unwrap()
        .get("distance")
        .unwrap();
    assert!(beam_road::pyjson::py_eq(distance, &Value::Int(120)));
    assert_eq!(
        rows[1]
            .get("data")
            .unwrap()
            .get("audio")
            .unwrap()
            .get("active"),
        Some(&Value::Bool(false))
    );
}
