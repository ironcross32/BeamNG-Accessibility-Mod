//! Owner: I0. Every conversion the engine needs between the workspace's three
//! Python-value types and `serde_json`, in one place.
//!
//! | Type | Crate | Used by |
//! |---|---|---|
//! | [`PyVal`] (`beam_base::pyval::PyValue`, `PyDict`) | beam-base | the config (`load_config_engine`), `AudioController::apply_config`, `gear_report::settings_from_config`, `secretstore` |
//! | [`PyOps`] (`beam_reports::pyops::PyValue`) | beam-reports | `DriftAnnouncer::handle`, `drift_phrase`, the report modules that take a decoded dict |
//! | [`PyJson`] (`beam_road::pyjson::Value`) | beam-road | `DrivingAssistant` (arrival, vehicle, snapshot), `RoadGuidanceFeed` snapshots, `HillClimbChallengeRecorder` (events, summaries, finalize callback), both diagnostic recorders |
//! | `serde_json::Value` | - | the engine's own JSON: the UI bridge payloads, the MCP state snapshot, `ConfigMap` |
//!
//! The three Python types are deliberately not merged. They agree on everything JSON can
//! carry; the conversions between them are lossless (big ints, NaN and the infinities
//! survive). Conversions to `serde_json` are lossy only where JSON-the-standard is: NaN
//! and the infinities become `null`, and an int beyond `u64` becomes the nearest float.

use beam_audio::controller::JunctionProximitySnapshot;
use beam_base::pyval::{self, PyInt};
use beam_road::driving_assistant::AssistantSnapshot;
use beam_road::road_guidance::FeedSnapshot;
use serde_json::{Map, Number, Value};

pub use beam_base::pyval::PyValue as PyVal;
pub use beam_reports::pyops::PyValue as PyOps;
pub use beam_road::pyjson::Value as PyJson;

/// A loaded config as JSON (`beam_base::pyval::to_json_map` of the real `PyDict`).
pub type ConfigMap = Map<String, Value>;

// ---- serde_json <-> each Python type -------------------------------------------------------

/// A float as `serde_json` (NaN and the infinities become `null`).
pub fn json_float(x: f64) -> Value {
    Number::from_f64(x)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// An exact Python int spelled in decimal, as `serde_json`.
fn json_int_text(s: &str) -> Value {
    if let Ok(i) = s.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(u) = s.parse::<u64>() {
        return Value::from(u);
    }
    s.parse::<f64>().map(json_float).unwrap_or(Value::Null)
}

/// beam-base config value -> `serde_json` (`beam_base::pyval::to_json`).
pub fn pyval_to_json(v: &PyVal) -> Value {
    pyval::to_json(v)
}

/// `serde_json` -> beam-base config value (`beam_base::pyval::from_json`).
pub fn json_to_pyval(v: &Value) -> PyVal {
    pyval::from_json(v)
}

/// A config dict as [`ConfigMap`].
pub fn pydict_to_config_map(d: &pyval::PyDict) -> ConfigMap {
    pyval::to_json_map(d)
}

/// A [`ConfigMap`] as the config dict the beam-base and beam-audio APIs take.
pub fn config_map_to_pydict(m: &ConfigMap) -> pyval::PyDict {
    pyval::from_json_map(m)
}

/// beam-road value -> `serde_json`.
pub fn pyjson_to_json(v: &PyJson) -> Value {
    match v {
        PyJson::Null => Value::Null,
        PyJson::Bool(b) => Value::Bool(*b),
        PyJson::Int(i) => Value::from(*i),
        PyJson::BigInt(s) => json_int_text(s),
        PyJson::Float(x) => json_float(*x),
        PyJson::Str(s) => Value::String(s.clone()),
        PyJson::List(items) => Value::Array(items.iter().map(pyjson_to_json).collect()),
        PyJson::Dict(d) => Value::Object(
            d.iter()
                .map(|(k, v)| (k.clone(), pyjson_to_json(v)))
                .collect(),
        ),
    }
}

/// `serde_json` -> beam-road value. An integer literal stays an int, anything else a float.
pub fn json_to_pyjson(v: &Value) -> PyJson {
    match v {
        Value::Null => PyJson::Null,
        Value::Bool(b) => PyJson::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                PyJson::Int(i)
            } else if let Some(u) = n.as_u64() {
                PyJson::BigInt(u.to_string())
            } else {
                PyJson::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(s) => PyJson::Str(s.clone()),
        Value::Array(a) => PyJson::List(a.iter().map(json_to_pyjson).collect()),
        Value::Object(o) => PyJson::Dict(
            o.iter()
                .map(|(k, v)| (k.clone(), json_to_pyjson(v)))
                .collect(),
        ),
    }
}

/// beam-reports value -> `serde_json`.
pub fn pyops_to_json(v: &PyOps) -> Value {
    match v {
        PyOps::None => Value::Null,
        PyOps::Bool(b) => Value::Bool(*b),
        PyOps::Int(s) => json_int_text(s),
        PyOps::Float(x) => json_float(*x),
        PyOps::Str(s) => Value::String(s.clone()),
        PyOps::List(items) => Value::Array(items.iter().map(pyops_to_json).collect()),
        PyOps::Dict(d) => Value::Object(
            d.iter()
                .map(|(k, v)| (k.clone(), pyops_to_json(v)))
                .collect(),
        ),
    }
}

/// `serde_json` -> beam-reports value (beam-reports' own `From<&serde_json::Value>`).
pub fn json_to_pyops(v: &Value) -> PyOps {
    PyOps::from(v)
}

// ---- Lossless conversions between the Python types -------------------------------------------

/// A config value handed to a beam-road API.
pub fn pyval_to_pyjson(v: &PyVal) -> PyJson {
    match v {
        PyVal::None => PyJson::Null,
        PyVal::Bool(b) => PyJson::Bool(*b),
        PyVal::Int(PyInt::Small(i)) => PyJson::Int(*i),
        PyVal::Int(PyInt::Big(s)) => PyJson::BigInt(s.clone()),
        PyVal::Float(x) => PyJson::Float(*x),
        PyVal::Str(s) => PyJson::Str(s.clone()),
        PyVal::List(items) => PyJson::List(items.iter().map(pyval_to_pyjson).collect()),
        PyVal::Dict(d) => PyJson::Dict(
            d.iter()
                .map(|(k, v)| (k.clone(), pyval_to_pyjson(v)))
                .collect(),
        ),
    }
}

/// A config value handed to a beam-reports API.
pub fn pyval_to_pyops(v: &PyVal) -> PyOps {
    match v {
        PyVal::None => PyOps::None,
        PyVal::Bool(b) => PyOps::Bool(*b),
        PyVal::Int(PyInt::Small(i)) => PyOps::int(*i),
        PyVal::Int(PyInt::Big(s)) => PyOps::Int(s.clone()),
        PyVal::Float(x) => PyOps::Float(*x),
        PyVal::Str(s) => PyOps::Str(s.clone()),
        PyVal::List(items) => PyOps::List(items.iter().map(pyval_to_pyops).collect()),
        PyVal::Dict(d) => PyOps::Dict(
            d.iter()
                .map(|(k, v)| (k.clone(), pyval_to_pyops(v)))
                .collect(),
        ),
    }
}

/// beam-road value -> beam-reports value.
pub fn pyjson_to_pyops(v: &PyJson) -> PyOps {
    match v {
        PyJson::Null => PyOps::None,
        PyJson::Bool(b) => PyOps::Bool(*b),
        PyJson::Int(i) => PyOps::int(*i),
        PyJson::BigInt(s) => PyOps::Int(s.clone()),
        PyJson::Float(x) => PyOps::Float(*x),
        PyJson::Str(s) => PyOps::Str(s.clone()),
        PyJson::List(items) => PyOps::List(items.iter().map(pyjson_to_pyops).collect()),
        PyJson::Dict(d) => PyOps::Dict(
            d.iter()
                .map(|(k, v)| (k.clone(), pyjson_to_pyops(v)))
                .collect(),
        ),
    }
}

/// beam-reports value -> beam-road value.
pub fn pyops_to_pyjson(v: &PyOps) -> PyJson {
    match v {
        PyOps::None => PyJson::Null,
        PyOps::Bool(b) => PyJson::Bool(*b),
        PyOps::Int(s) => match s.parse::<i64>() {
            Ok(i) => PyJson::Int(i),
            Err(_) => PyJson::BigInt(s.clone()),
        },
        PyOps::Float(x) => PyJson::Float(*x),
        PyOps::Str(s) => PyJson::Str(s.clone()),
        PyOps::List(items) => PyJson::List(items.iter().map(pyops_to_pyjson).collect()),
        PyOps::Dict(d) => PyJson::Dict(
            d.iter()
                .map(|(k, v)| (k.clone(), pyops_to_pyjson(v)))
                .collect(),
        ),
    }
}

// ---- Typed snapshots as the Python dicts -------------------------------------------------------

/// `AudioController.junction_proximity_snapshot()` as the dict audio.py returns:
/// `{"enabled", "active", "age_s", "target": dict(target) if target else None, "mode",
/// "period_s"}`.
pub fn junction_proximity_snapshot_json(s: &JunctionProximitySnapshot) -> Value {
    let target = match &s.target {
        Some(t) if !t.is_empty() => Value::Object(t.clone()),
        _ => Value::Null,
    };
    let mut out = Map::new();
    out.insert("enabled".into(), Value::Bool(s.enabled));
    out.insert("active".into(), Value::Bool(s.active));
    out.insert("age_s".into(), json_float(s.age_s));
    out.insert("target".into(), target);
    out.insert("mode".into(), Value::String(s.mode.to_string()));
    out.insert(
        "period_s".into(),
        s.period_s.map(json_float).unwrap_or(Value::Null),
    );
    Value::Object(out)
}

/// `driving_assistant.snapshot()` as JSON.
pub fn assistant_snapshot_json(s: &AssistantSnapshot) -> Value {
    pyjson_to_json(&s.to_value())
}

/// `ROAD_GUIDANCE_FEED.snapshot()` as JSON.
pub fn feed_snapshot_json(s: &FeedSnapshot) -> Value {
    pyjson_to_json(&s.to_value())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn big() -> &'static str {
        "123456789012345678901234567890"
    }

    #[test]
    fn pyjson_round_trips_through_json() {
        let v = json!({"b": true, "i": -3, "u": u64::MAX, "f": 1.5, "s": "x", "n": null,
                       "l": [1, 2.0, "z"], "d": {"k": 0}});
        let p = json_to_pyjson(&v);
        let PyJson::Dict(d) = &p else {
            panic!("not a dict")
        };
        assert_eq!(
            d.keys().collect::<Vec<_>>(),
            ["b", "i", "u", "f", "s", "n", "l", "d"]
        );
        assert_eq!(d["i"], PyJson::Int(-3));
        assert_eq!(d["u"], PyJson::BigInt(u64::MAX.to_string()));
        assert_eq!(d["f"], PyJson::Float(1.5));
        let PyJson::List(l) = &d["l"] else {
            panic!("not a list")
        };
        assert_eq!(l[1], PyJson::Float(2.0));
        assert_eq!(pyjson_to_json(&p), v);
    }

    #[test]
    fn pyops_round_trips_through_json() {
        let v = json!({"event": "shown", "score": 12, "angle": 33.25, "list": [true, null]});
        let p = json_to_pyops(&v);
        assert_eq!(p.as_dict().unwrap()["score"], PyOps::int(12));
        assert_eq!(pyops_to_json(&p), v);
    }

    #[test]
    fn pyval_round_trips_through_json() {
        let v = json!({"units": "metric", "n": 6, "x": 15.0});
        let m = v.as_object().unwrap().clone();
        let d = config_map_to_pydict(&m);
        assert_eq!(pydict_to_config_map(&d), m);
        assert_eq!(pyval_to_json(&json_to_pyval(&v)), v);
    }

    #[test]
    fn non_finite_floats_and_big_ints() {
        assert_eq!(pyjson_to_json(&PyJson::Float(f64::NAN)), Value::Null);
        assert_eq!(pyops_to_json(&PyOps::Float(f64::INFINITY)), Value::Null);
        assert_eq!(json_float(-0.0), json!(-0.0));
        // Beyond u64: nearest float.
        let j = pyjson_to_json(&PyJson::BigInt(big().into()));
        assert_eq!(j.as_f64(), Some(1.2345678901234568e29));
        assert_eq!(pyops_to_json(&PyOps::Int(big().into())), j);
        assert_eq!(pyops_to_json(&PyOps::Int("-7".into())), json!(-7));
    }

    #[test]
    fn python_types_convert_losslessly() {
        let cfg = PyVal::Dict(
            [
                ("a".to_string(), PyVal::Int(PyInt::Big(big().into()))),
                ("b".to_string(), PyVal::Float(f64::NAN)),
                (
                    "c".to_string(),
                    PyVal::List(vec![PyVal::None, PyVal::Bool(true)]),
                ),
                ("d".to_string(), PyVal::from(-4i64)),
                ("e".to_string(), PyVal::from("s")),
            ]
            .into_iter()
            .collect(),
        );
        let pj = pyval_to_pyjson(&cfg);
        let po = pyval_to_pyops(&cfg);
        let PyJson::Dict(dj) = &pj else { panic!() };
        assert_eq!(dj["a"], PyJson::BigInt(big().into()));
        assert!(matches!(dj["b"], PyJson::Float(x) if x.is_nan()));
        assert_eq!(dj["d"], PyJson::Int(-4));
        let PyOps::Dict(dops) = &po else { panic!() };
        assert_eq!(dops["a"], PyOps::Int(big().into()));
        assert_eq!(dops["d"], PyOps::Int("-4".into()));
        // road <-> reports both ways, keys in order.
        let back = pyops_to_pyjson(&pyjson_to_pyops(&pj));
        let PyJson::Dict(db) = &back else { panic!() };
        assert_eq!(db.keys().collect::<Vec<_>>(), ["a", "b", "c", "d", "e"]);
        assert_eq!(db["a"], dj["a"]);
        assert_eq!(db["c"], dj["c"]);
        assert_eq!(db["d"], dj["d"]);
        assert_eq!(db["e"], dj["e"]);
        assert!(matches!(db["b"], PyJson::Float(x) if x.is_nan()));
        let PyOps::Dict(dops2) = pyjson_to_pyops(&back) else {
            panic!()
        };
        for k in ["a", "c", "d", "e"] {
            assert_eq!(dops2[k], dops[k], "{k}");
        }
    }

    #[test]
    fn junction_proximity_snapshot_matches_audio_py() {
        let s = JunctionProximitySnapshot {
            enabled: true,
            active: false,
            age_s: 0.125,
            target: Some(Map::new()),
            mode: "silent",
            period_s: None,
        };
        let j = junction_proximity_snapshot_json(&s);
        assert_eq!(
            j,
            json!({"enabled": true, "active": false, "age_s": 0.125, "target": null,
                   "mode": "silent", "period_s": null})
        );
        let keys: Vec<_> = j.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            ["enabled", "active", "age_s", "target", "mode", "period_s"]
        );
        let mut t = Map::new();
        t.insert("distance".into(), json!(12.0));
        let s = JunctionProximitySnapshot {
            target: Some(t),
            period_s: Some(0.5),
            mode: "approach",
            ..s
        };
        let j = junction_proximity_snapshot_json(&s);
        assert_eq!(j["target"], json!({"distance": 12.0}));
        assert_eq!(j["period_s"], json!(0.5));
    }

    #[test]
    fn typed_snapshots_keep_python_keys() {
        let a = AssistantSnapshot {
            active: false,
            pending: false,
            following_route: true,
            vehicle: PyJson::Null,
            sequence: -1,
            response_age_s: None,
        };
        let j = assistant_snapshot_json(&a);
        let keys: Vec<_> = j.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            [
                "active",
                "pending",
                "following_route",
                "vehicle",
                "sequence",
                "response_age_s"
            ]
        );
        assert_eq!(j["following_route"], json!(true));
        let f = FeedSnapshot {
            mode: "unavailable".into(),
            packet: None,
            legacy: Some(beam_road::road_guidance::LegacyState {
                state: "ON_ROAD".into(),
                bearing: 10.0,
                distance: 2.0,
                directions: vec![90.0],
            }),
            junction: None,
        };
        assert_eq!(
            feed_snapshot_json(&f),
            json!({"mode": "unavailable", "packet": null,
                   "legacy": {"state": "ON_ROAD", "bearing": 10.0, "distance": 2.0,
                              "directions": [90.0]},
                   "junction": null})
        );
    }
}
