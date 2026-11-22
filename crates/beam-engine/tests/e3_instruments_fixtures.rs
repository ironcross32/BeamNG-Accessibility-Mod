//! The E3 instruments against `fixtures/data/e3_instruments.json`, which
//! `fixtures/gen/e3_instruments.py` writes from beamtel.py itself: every implement, dock,
//! ramp and cannon phrase, and the real listeners driven datagram by datagram (the calls each
//! made and the globals it wrote after every datagram).

use std::sync::{Arc, Once};

use beam_audio::controller::AudioController;
use beam_audio::scan::{ScanError, ScanObject, ScanPoi, ScanSample};
use beam_base::pyfmt::py_strip;
use beam_engine::keyboard::KeyEvent;
use beam_engine::listeners::{
    cannon, dock_phrases as dp, implement, lineup, route_beacon, terrain_scan, trailer,
    wheel_report,
};
use beam_engine::net::{Decode, decode};
use beam_engine::state::{
    CannonAim, CannonShot, DockReading, RampSelf, State, set_audio_controller_ref, state,
};
use beam_engine::timer::monotonic;
use beam_engine::{speech_out, units};
use beam_testutil::{f, load};
use parking_lot::Mutex;
use serde_json::{Value, json};

static ENV: Once = Once::new();

fn setup() -> parking_lot::MutexGuard<'static, ()> {
    let guard = speech_out::TEST_SERIAL.lock();
    ENV.call_once(|| {
        let dir = std::env::temp_dir().join(format!("beam_e3_tests_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: inside `Once`, with TEST_SERIAL held, before any test here reads the
        // environment.
        unsafe { std::env::set_var("LOCALAPPDATA", &dir) };
    });
    speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
    speech_out::reset_for_tests();
    *state() = State::default();
    units::set_units_mode("imperial");
    guard
}

/// A float in canonical comparison form (NaN payloads and signs are not compared).
fn e(x: f64) -> Value {
    if x.is_nan() {
        json!("nan")
    } else {
        json!(x.to_bits())
    }
}

fn eo(x: Option<f64>) -> Value {
    x.map(e).unwrap_or(Value::Null)
}

/// The fixture's `{"bits", "repr"}` floats in the same canonical form.
fn canon(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.contains_key("bits") => e(f(v)),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), canon(x))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
        other => other.clone(),
    }
}

fn fo(v: &Value) -> Option<f64> {
    if v.is_null() { None } else { Some(f(v)) }
}

fn sv(v: &Value) -> String {
    v.as_str()
        .unwrap_or_else(|| panic!("not a str: {v}"))
        .to_string()
}

fn dock_from(d: &Value) -> DockReading {
    DockReading {
        name: sv(&d["name"]),
        range: f(&d["range"]),
        lateral: f(&d["lateral"]),
        vertical: f(&d["vertical"]),
        idx: d["idx"].as_i64().unwrap(),
        count: d["count"].as_i64().unwrap(),
        kind: sv(&d["kind"]),
        lo_z: f(&d["lo_z"]),
        hi_z: f(&d["hi_z"]),
        yaw: f(&d["yaw"]),
        manual: d["manual"].as_bool().unwrap(),
        impl_word: sv(&d["impl_word"]),
        entry_theta: fo(&d["entry_theta"]),
        entry_depth: fo(&d["entry_depth"]),
        mode: sv(&d["mode"]),
        margin: fo(&d["margin"]),
        acquire: d["acquire"].as_bool().unwrap(),
    }
}

fn dock_json(d: &DockReading) -> Value {
    json!({
        "name": d.name, "range": e(d.range), "lateral": e(d.lateral),
        "vertical": e(d.vertical), "idx": d.idx, "count": d.count, "kind": d.kind,
        "lo_z": e(d.lo_z), "hi_z": e(d.hi_z), "yaw": e(d.yaw), "manual": d.manual,
        "impl_word": d.impl_word, "entry_theta": eo(d.entry_theta),
        "entry_depth": eo(d.entry_depth), "mode": d.mode, "margin": eo(d.margin),
        "acquire": d.acquire,
    })
}

fn shot_from(s: &Value) -> CannonShot {
    CannonShot {
        downrange: f(&s["downrange"]),
        lateral: f(&s["lateral"]),
        apex: f(&s["apex"]),
        flight: f(&s["flight"]),
        ramp_pitch: f(&s["ramp_pitch"]),
        strength: s["strength"].as_i64().unwrap(),
        settled: s["settled"].as_bool().unwrap(),
        index: s["index"].as_i64().unwrap(),
        near_dist: f(&s["near_dist"]),
        near_name: sv(&s["near_name"]),
        vehicle: sv(&s["vehicle"]),
        stamp: f(&s["stamp"]),
        prev_downrange: s.get("prev_downrange").map(f),
    }
}

fn shot_json(s: &CannonShot) -> Value {
    let mut v = json!({
        "downrange": e(s.downrange), "lateral": e(s.lateral), "apex": e(s.apex),
        "flight": e(s.flight), "ramp_pitch": e(s.ramp_pitch), "strength": s.strength,
        "settled": s.settled, "index": s.index, "near_dist": e(s.near_dist),
        "near_name": s.near_name, "vehicle": s.vehicle, "stamp": e(s.stamp),
    });
    if let Some(p) = s.prev_downrange {
        v["prev_downrange"] = e(p);
    }
    v
}

fn aim_json(a: &CannonAim) -> Value {
    json!({
        "elevation": e(a.elevation), "bearing": e(a.bearing),
        "target_angle": e(a.target_angle), "line_angle": e(a.line_angle),
        "range": e(a.range), "speed": e(a.speed), "reachable": a.reachable,
        "solution_available": a.solution_available, "stamp": e(a.stamp),
    })
}

fn ramp_self_json(r: &RampSelf) -> Value {
    json!({
        "pitch": eo(r.pitch), "lip": eo(r.lip),
        "groups": r.groups.iter().map(|(n, p, s)| json!([n, p, e(*s)])).collect::<Vec<_>>(),
    })
}

fn ramp_self_from(v: &Value) -> RampSelf {
    RampSelf {
        pitch: fo(&v["pitch"]),
        lip: fo(&v["lip"]),
        groups: v["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| (sv(&g[0]), g[1].as_i64().unwrap(), f(&g[2])))
            .collect(),
    }
}

/// `per_units` results: `{"imperial": {"ok": ...}, "metric": {...}}`.
fn check_units(label: &str, expected: &Value, run: &dyn Fn() -> String) {
    for u in ["imperial", "metric"] {
        units::set_units_mode(u);
        let want = &expected[u];
        if let Some(ok) = want.get("ok") {
            assert_eq!(run(), sv(ok), "{label} [{u}]");
        } else {
            let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
            assert!(got.is_err(), "{label} [{u}]: Python raised {want}");
        }
    }
    units::set_units_mode("imperial");
}

// =========================================================================================
//  Phrases
// =========================================================================================

#[test]
fn phrases_match_python() {
    let _g = setup();
    let fx = load("e3_instruments");
    let p = &fx["phrases"];

    for c in p["band_name"].as_array().unwrap() {
        let got = dp::band_name(
            c[0].as_str().unwrap(),
            c[1].as_i64().unwrap(),
            c[2].as_i64().unwrap(),
        );
        assert_eq!(got, sv(&c[3]), "band_name {c}");
    }
    for c in p["implement_word"].as_array().unwrap() {
        assert_eq!(dp::implement_word(c[0].as_str().unwrap()), sv(&c[1]), "{c}");
    }
    for c in p["ramp_bearing_deg"].as_array().unwrap() {
        let got = dp::ramp_bearing_deg(f(&c[0]), f(&c[1]), f(&c[2]));
        assert_eq!(e(got), canon(&c[3]), "ramp_bearing_deg {c}");
    }
    for c in p["ramp_acquire"].as_array().unwrap() {
        let got = dp::ramp_acquire(f(&c[0]), f(&c[1]), c[2].as_bool().unwrap());
        assert_eq!(got, c[3].as_bool().unwrap(), "ramp_acquire {c}");
    }
    for c in p["ramp_align_phrase"].as_array().unwrap() {
        let payload = sv(&c[0]);
        check_units(&format!("ramp_align {payload:?}"), &c[1], &|| {
            dp::ramp_align_phrase(&payload)
        });
    }
    for c in p["cannon_shot_phrase"].as_array().unwrap() {
        let shot = shot_from(&c[0]);
        check_units(&format!("cannon {}", c[0]), &c[1], &|| {
            dp::cannon_shot_phrase(&shot)
        });
    }
    for c in p["dock_phrase"].as_array().unwrap() {
        let dock = dock_from(&c[0]);
        check_units(&format!("dock {}", c[0]), &c[1], &|| dp::dock_phrase(&dock));
    }
    for c in p["ramp_self"].as_array().unwrap() {
        let payload = sv(&c[0]);
        let parsed = dp::parse_ramp_self(&payload);
        assert_eq!(
            parsed.as_ref().map(ramp_self_json).unwrap_or(Value::Null),
            canon(&c[1]),
            "parse_ramp_self {payload:?}"
        );
        if let Some(rs) = parsed {
            // Phrase from the Python-parsed state, so a parse difference cannot mask it.
            let state_py = ramp_self_from(&c[1]);
            check_units(&format!("ramp_self {payload:?}"), &c[2], &|| {
                dp::ramp_self_phrase(&state_py)
            });
            let _ = rs;
        }
    }
    for c in p["ramp_hyd_words"].as_array().unwrap() {
        assert_eq!(dp::ramp_hyd_words(c[0].as_str().unwrap()), sv(&c[1]), "{c}");
    }
    for c in p["ramp_hyd_strip_namespace"].as_array().unwrap() {
        let names: Vec<String> = c[0].as_array().unwrap().iter().map(sv).collect();
        let want: Vec<String> = c[1].as_array().unwrap().iter().map(sv).collect();
        assert_eq!(dp::ramp_hyd_strip_namespace(&names), want, "{c}");
    }
}

// =========================================================================================
//  Listeners
// =========================================================================================

fn listener<'a>(fx: &'a Value, name: &str) -> &'a Value {
    fx["listeners"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["name"] == name)
        .unwrap_or_else(|| panic!("no listener scenario {name}"))
}

fn apply_set(set: &Value) {
    for (k, v) in set.as_object().unwrap() {
        match k.as_str() {
            "dock_mode_active" => state().dock.dock_mode_active = v.as_bool().unwrap(),
            "announce_implement_proximity" => {
                state().cfg.announce_implement_proximity = v.as_bool().unwrap()
            }
            "announce_cannon_shot" => state().cfg.announce_cannon_shot = v.as_bool().unwrap(),
            "UNITS_MODE" => units::set_units_mode(v.as_str().unwrap()),
            "_lineup_enabled" => state().lineup.lineup_enabled = v.as_bool().unwrap(),
            "_lineup_layer_open" => state().lineup.lineup_layer_open = v.as_bool().unwrap(),
            other => panic!("unknown setting {other}"),
        }
    }
}

fn step_text(step: &Value) -> Option<String> {
    let hex = step["data"].as_str()?;
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    Some(decode(&bytes, Decode::Utf8Ignore).unwrap())
}

/// Speech said since the last call, as `["say", text, excluded]`.
fn take_says() -> Vec<Value> {
    let log = speech_out::get_speech_log(None, Some(0), None, None, false);
    speech_out::reset_for_tests();
    log.entries
        .iter()
        .map(|t| json!(["say", t.text, t.excluded]))
        .collect()
}

/// The fixture's say calls in the same form (`interrupt` is not compared: the tap records it
/// after the protection window).
fn py_says(calls: &Value) -> Vec<Value> {
    calls
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c[0] == "say")
        .map(|c| json!(["say", c[1], c[3]]))
        .collect()
}

#[derive(Default)]
struct RecSink {
    calls: Mutex<Vec<Value>>,
}

impl RecSink {
    fn push(&self, v: Value) {
        self.calls.lock().push(v);
    }
    fn take(&self) -> Vec<Value> {
        std::mem::take(&mut *self.calls.lock())
    }
}

impl implement::ImplementSink for RecSink {
    fn say(&self, text: &str) {
        self.push(json!(["say", text, true, true]));
    }
    fn send_implement_cmd(&self, command: &str) {
        self.push(json!(["send_implement_cmd", command]));
    }
    fn clear_dock_target(&self) {
        self.push(json!(["clear_dock_target"]));
    }
    fn clear_cannon_aim(&self) {
        self.push(json!(["clear_cannon_aim"]));
    }
    fn update_cannon_aim(&self, el: f64, b: f64, t: f64, s: bool, r: bool) {
        self.push(json!(["update_cannon_aim", e(el), e(b), e(t), s, r]));
    }
    fn trigger_slam_cue(&self, cue: &str) {
        self.push(json!(["trigger_slam_cue", cue]));
    }
    fn update_dock_target(&self, r: f64, l: f64, n: f64, m: &str, a: bool, b: f64) {
        self.push(json!(["update_dock_target", e(r), e(l), e(n), m, a, e(b)]));
    }
    fn trigger_dock_approach_cue(&self) {
        self.push(json!(["trigger_dock_approach_cue"]));
    }
    fn trigger_entry_cue(&self) {
        self.push(json!(["trigger_entry_cue"]));
    }
}

impl terrain_scan::ScanRenderer for RecSink {
    fn render_and_play_scan(
        &self,
        samples: &[ScanSample],
        objects: &[ScanObject],
        reach_m: f64,
        pois: &[ScanPoi],
    ) -> Result<(), ScanError> {
        let s: Vec<Value> = samples
            .iter()
            .map(|x| {
                json!([
                    e(x.bearing_deg),
                    e(x.range_m),
                    eo(x.dz_m),
                    eo(x.depth_m),
                    x.family
                ])
            })
            .collect();
        let o: Vec<Value> = objects
            .iter()
            .map(|x| json!([e(x.bearing_deg), e(x.range_m), e(x.dz_m), x.kind]))
            .collect();
        let p: Vec<Value> = pois
            .iter()
            .map(|x| json!([e(x.bearing_deg), e(x.range_m), e(x.dz_m)]))
            .collect();
        self.push(json!(["render_and_play_scan", s, o, e(reach_m), p]));
        Ok(())
    }
}

fn dock_snap() -> Value {
    let st = state();
    let d = &st.dock;
    json!({
        "last_dock": d.last_dock.as_ref().map(dock_json),
        "last_dock_fail": d.last_dock_fail,
        "last_dock_mode": d.last_dock_mode,
        "last_dock_name": d.last_dock_name,
        "cannon_kind": d.cannon_kind,
        "cannon_active": d.cannon_active,
        "last_cannon_aim": d.last_cannon_aim.as_ref().map(aim_json),
        "last_ramp_self": d.last_ramp_self.as_ref().map(ramp_self_json),
        "_implement_word_current": d.implement_word_current,
    })
}

#[test]
fn implement_listener_matches_python() {
    let _g = setup();
    let fx = load("e3_instruments");
    for name in ["probe", "proximity", "dock", "cannon_ramp_slam", "metric"] {
        *state() = State::default();
        units::set_units_mode("imperial");
        speech_out::reset_for_tests();
        let sc = listener(&fx, name);
        let sink = RecSink::default();
        let mut l = implement::ImplementListener::start(&sink, f(&sc["start_now"]));
        assert_eq!(
            Value::Array(sink.take()),
            canon(&sc["prelude"]),
            "{name} prelude"
        );
        for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
            apply_set(&step["set"]);
            let (now, mono) = (f(&step["now"]), f(&step["mono"]));
            match step_text(step) {
                None => l.on_timeout(&sink, now),
                Some(text) => {
                    let text = py_strip(&text);
                    if !text.is_empty() {
                        l.handle_text(&sink, text, now, mono);
                    }
                }
            }
            assert_eq!(
                Value::Array(sink.take()),
                canon(&step["calls"]),
                "{name} step {i} ({:?}) calls",
                step_text(step)
            );
            assert_eq!(dock_snap(), canon(&step["snap"]), "{name} step {i} snap");
        }
    }
}

/// Run a speech/state-only listener's steps through `handle`.
fn run_simple(fx: &Value, name: &str, handle: &dyn Fn(&str, f64, f64), snap: &dyn Fn() -> Value) {
    *state() = State::default();
    units::set_units_mode("imperial");
    speech_out::reset_for_tests();
    let sc = listener(fx, name);
    for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
        apply_set(&step["set"]);
        if let Some(text) = step_text(step) {
            handle(&text, f(&step["now"]), f(&step["mono"]));
        }
        assert_eq!(take_says(), py_says(&step["calls"]), "{name} step {i} says");
        assert_eq!(snap(), canon(&step["snap"]), "{name} step {i} snap");
    }
}

#[test]
fn cannon_listener_matches_python() {
    let _g = setup();
    let fx = load("e3_instruments");
    run_simple(
        &fx,
        "cannon",
        &|t, _now, mono| cannon::handle_cannon_text(py_strip(t), mono),
        &|| {
            let st = state();
            json!({
                "last_cannon_shot": st.dock.last_cannon_shot.as_ref().map(shot_json),
                "cannon_shot_session":
                    st.dock.cannon_shot_session.iter().map(shot_json).collect::<Vec<_>>(),
            })
        },
    );
}

#[test]
fn trailer_listener_matches_python() {
    let _g = setup();
    let fx = load("e3_instruments");
    run_simple(
        &fx,
        "trailer",
        &|t, _now, mono| trailer::handle_trailer_text(py_strip(t), mono),
        &|| {
            let st = state();
            let t = &st.trailer;
            json!({
                "last_trailer_deg": e(t.last_trailer_deg), "last_trailer_id": t.last_trailer_id,
                "last_trailer_name": t.last_trailer_name,
                "last_trailer_stamp": e(t.last_trailer_stamp),
            })
        },
    );
}

#[test]
fn route_listener_matches_python() {
    let _g = setup();
    let fx = load("e3_instruments");
    run_simple(
        &fx,
        "route",
        &|t, _now, mono| route_beacon::handle_route_text(py_strip(t), mono),
        &|| {
            let st = state();
            let r = &st.route;
            json!({
                "route_dest_x": eo(r.route_dest_x), "route_dest_y": eo(r.route_dest_y),
                "route_remaining_m": e(r.route_remaining_m),
                "route_beacon_stamp": e(r.route_beacon_stamp),
            })
        },
    );
}

#[test]
fn wheel_report_listener_matches_python() {
    let _g = setup();
    let fx = load("e3_instruments");
    run_simple(
        &fx,
        "wheels",
        &|t, _now, mono| wheel_report::handle_wheel_report_text(t, mono),
        &|| {
            let st = state();
            let rows: Vec<Value> = st
                .wheels
                .last_wheel_rows
                .iter()
                .map(|r| {
                    json!({
                        "label": r["label"], "broken": r["broken"],
                        "psi": r["psi"].as_f64().map(e).unwrap_or(Value::Null),
                        "brake_c": r["brake_c"].as_f64().map(e).unwrap_or(Value::Null),
                    })
                })
                .collect();
            json!({"last_wheel_rows": rows, "last_wheel_stamp": e(st.wheels.last_wheel_stamp)})
        },
    );
}

#[test]
fn terrain_listener_matches_python() {
    let _g = setup();
    let fx = load("e3_instruments");
    *state() = State::default();
    let sc = listener(&fx, "terrain");
    let sink = RecSink::default();
    for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
        apply_set(&step["set"]);
        let text = step_text(step).unwrap();
        assert!(terrain_scan::handle_terrain_text(
            &sink,
            py_strip(&text),
            f(&step["now"])
        ));
        let renders: Vec<Value> = canon(&step["calls"])
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c[0] == "render_and_play_scan")
            .cloned()
            .collect();
        // Python's object tuples always carry the kind; pass `pois=` was a keyword.
        assert_eq!(sink.take(), renders, "terrain step {i} render");
        assert_eq!(
            take_says(),
            py_says(&step["calls"]),
            "terrain step {i} says"
        );
        let st = state();
        assert_eq!(
            json!({"_last_scan_reply_ts": e(st.scan.last_scan_reply_ts),
                   "_last_poi_reply_ts": e(st.scan.last_poi_reply_ts)}),
            canon(&step["snap"]),
            "terrain step {i} snap"
        );
    }
}

fn lineup_snap_rs(st: &State) -> Value {
    let l = &st.lineup;
    json!({
        "_lineup_mode": l.lineup_mode, "_lineup_offset_m": e(l.lineup_offset_m),
        "_lineup_depth_m": e(l.lineup_depth_m), "_lineup_muted": l.lineup_muted,
        "_lineup_geom": l.lineup_geom.map(|g| json!([e(g.cam_x), e(g.cam_y), e(g.cam_fx),
            e(g.cam_fy), e(g.nose_x), e(g.nose_y), e(g.veh_fx), e(g.veh_fy)])),
    })
}

/// The `set_lineup` call Python made, checked against the controller's stored target.
fn check_set_lineup(ac: &AudioController, calls: &Value, label: &str) {
    for c in canon(calls).as_array().unwrap() {
        if c[0] != "set_lineup" {
            continue;
        }
        let s = ac.lock.lock();
        let active = c[1].as_bool().unwrap();
        assert_eq!(s._lineup_active, active, "{label} active");
        let want = if active {
            json!([c[2], c[3], c[4], c[5]])
        } else {
            json!([e(0.0), e(1.0), e(1.0), e(0.0)])
        };
        let t = s._lineup_target;
        assert_eq!(
            json!([e(t.0), e(t.1), e(t.2), e(t.3)]),
            want,
            "{label} target"
        );
    }
}

#[test]
fn lineup_listener_matches_python() {
    let _g = setup();
    let fx = load("e3_instruments");
    let sc = listener(&fx, "lineup");
    let ac = AudioController::new();
    for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
        apply_set(&step["set"]);
        let text = step_text(step).unwrap();
        lineup::handle_lineup_text(&ac, &text, f(&step["mono"]));
        check_set_lineup(&ac, &step["calls"], &format!("lineup step {i}"));
        let mut want = canon(&step["snap"]);
        let stamp = want
            .as_object_mut()
            .unwrap()
            .remove("_lineup_stamp")
            .unwrap();
        assert_eq!(lineup_snap_rs(&state()), want, "lineup step {i} snap");
        assert_eq!(
            e(state().lineup.lineup_stamp),
            stamp,
            "lineup step {i} stamp"
        );
    }
}

#[test]
fn lineup_keys_match_python() {
    let _g = setup();
    let fx = load("e3_instruments");
    let ac = AudioController::new();
    set_audio_controller_ref(Some(Arc::clone(&ac)));
    {
        let mut st = state();
        st.lineup.lineup_enabled = true;
        st.lineup.lineup_layer_open = true;
        st.lineup.lineup_geom =
            match beam_reports::camera_lineup::parse_packet("LINEUP:0,0,0,1,10,-20,0,1") {
                Some(beam_reports::camera_lineup::LineupPacket::Geometry(g)) => Some(g),
                _ => panic!("geometry"),
            };
    }
    for (i, k) in fx["lineup_keys"].as_array().unwrap().iter().enumerate() {
        units::set_units_mode(k["units"].as_str().unwrap());
        // Python's monotonic is 5000.2 or 5001.0 against a stamp of 5000.0.
        state().lineup.lineup_stamp = monotonic() - (f(&k["mono"]) - 5000.0);
        lineup::lineup_on_key(&KeyEvent::down(k["key"].as_str().unwrap()));
        let label = format!("key {i} {}", k["key"]);
        assert_eq!(take_says(), py_says(&k["calls"]), "{label} says");
        check_set_lineup(&ac, &k["calls"], &label);
        let mut want = canon(&k["snap"]);
        want.as_object_mut().unwrap().remove("_lineup_stamp");
        assert_eq!(lineup_snap_rs(&state()), want, "{label} snap");
    }
    set_audio_controller_ref(None);
    units::set_units_mode("imperial");
}
