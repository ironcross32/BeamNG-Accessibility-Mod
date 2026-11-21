//! `vehicle_spawner` against `rust/fixtures/data/vehicle_spawner.json`, which
//! `rust/fixtures/gen/vehicle_spawner.py` writes by driving the real Python module.
//!
//! Each scenario is a list of steps: datagrams through `handle_packet`, key handlers run
//! as the worker runs them, key releases, F11 toggles and single ticker passes. After every
//! step the speech, the commands sent on 4461, the pings, the info requests and any handler
//! error are compared with the Python's, and the module state where the Python recorded it.
//! A second test drives the suppressing hooks through the keyboard listener (no OS hook).

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};

use beam_engine::keyboard::{self, KeyEvent};
use beam_engine::state::SlotInfo;
use beam_engine::vehicle_spawner::{self as vs, InitArgs, TestHooks};
use beam_testutil::{f, load, same_f64};
use indexmap::IndexMap;
use parking_lot::Mutex;
use serde_json::{Value, json};

static SERIAL: Mutex<()> = Mutex::new(());
static EVENTS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static MODS: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);
static CLOCK: Mutex<f64> = Mutex::new(1000.0);
static PICKS: Mutex<VecDeque<usize>> = Mutex::new(VecDeque::new());
static SLOTS: Mutex<Option<IndexMap<i64, SlotInfo>>> = Mutex::new(None);
static FOCUSED: AtomicBool = AtomicBool::new(true);
type InfoReply = (Vec<String>, Option<(String, String)>);
static INFO: Mutex<Option<InfoReply>> = Mutex::new(None);
static F11: Once = Once::new();

fn push(v: Value) {
    EVENTS.lock().push(v);
}

fn setup() -> parking_lot::MutexGuard<'static, ()> {
    let g = SERIAL.lock();
    vs::init(InitArgs {
        say_fn: Arc::new(|t: &str, interrupt: bool, exclude: bool, source: &str| {
            assert!(
                interrupt && exclude && source == "python",
                "say flags for {t:?}"
            );
            push(json!(["say", t]));
        }),
        is_focused_fn: Arc::new(|| FOCUSED.load(Ordering::SeqCst)),
        get_slots_fn: Arc::new(|| SLOTS.lock().clone().unwrap_or_default()),
        close_others_fn: Arc::new(|| push(json!(["close_others"]))),
        ping_fn: Arc::new(|rung, az, pitch| {
            push(json!(["ping", rung, az.to_bits(), pitch.to_bits()]))
        }),
        request_info_fn: Arc::new(|model, config, timeout| {
            assert_eq!(timeout, 0.6);
            push(json!([
                "info_req",
                model.unwrap_or(""),
                config.unwrap_or("")
            ]));
            INFO.lock().clone().unwrap_or_default()
        }),
    });
    vs::set_test_hooks(Some(TestHooks {
        send: Some(Arc::new(|c: &str| push(json!(["send", c])))),
        clock: Some(Arc::new(|| *CLOCK.lock())),
        pressed: Some(Arc::new(|name: &str| {
            MODS.lock()
                .as_ref()
                .and_then(|m| m.get(name).copied())
                .unwrap_or(false)
        })),
        choice: Some(Arc::new(|_n| PICKS.lock().pop_front().unwrap_or(0))),
        no_ticker: true,
    }));
    vs::reset_for_tests();
    EVENTS.lock().clear();
    *MODS.lock() = None;
    *CLOCK.lock() = 1000.0;
    PICKS.lock().clear();
    *SLOTS.lock() = None;
    FOCUSED.store(true, Ordering::SeqCst);
    *INFO.lock() = None;
    g
}

/// Fixture pings carry `fbits` floats; compare on the bits.
fn normalize(ev: &Value) -> Value {
    match ev.as_array() {
        Some(a) if a.first() == Some(&json!("ping")) => {
            json!(["ping", a[1], f(&a[2]).to_bits(), f(&a[3]).to_bits()])
        }
        _ => ev.clone(),
    }
}

fn run_step(step: &Value) {
    let op = step["op"].as_str().expect("op");
    let r = catch_unwind(AssertUnwindSafe(|| match op {
        "packet" => vs::handle_packet(step["text"].as_str().expect("text")),
        "key" => {
            let name = step["name"].as_str().expect("name");
            vs::dispatch_key(name, &KeyEvent::down(name));
        }
        "release" => vs::on_place_release(&KeyEvent::up(step["name"].as_str().expect("name"))),
        "f11" => vs::toggle_modal(),
        "close" => vs::close_modal(),
        "mods" => {
            let m = step["mods"]
                .as_object()
                .expect("mods")
                .iter()
                .map(|(k, v)| (k.clone(), v.as_bool().unwrap_or(false)))
                .collect();
            *MODS.lock() = Some(m);
        }
        "clock" => *CLOCK.lock() = f(&step["t"]),
        "tick" => vs::tick_for_tests(*CLOCK.lock()),
        "picks" => {
            *PICKS.lock() = step["picks"]
                .as_array()
                .expect("picks")
                .iter()
                .map(|v| v.as_u64().expect("pick") as usize)
                .collect()
        }
        "slots" => {
            let m = step["slots"]
                .as_object()
                .expect("slots")
                .iter()
                .map(|(k, v)| {
                    (
                        k.parse::<i64>().expect("slot key"),
                        SlotInfo {
                            id: v["id"].as_i64().expect("id"),
                            name: v["name"].as_str().expect("name").to_string(),
                        },
                    )
                })
                .collect();
            *SLOTS.lock() = Some(m);
        }
        "focus" => FOCUSED.store(step["v"].as_bool().expect("v"), Ordering::SeqCst),
        "info" => {
            let lines = step["lines"]
                .as_array()
                .expect("lines")
                .iter()
                .map(|v| v.as_str().expect("line").to_string())
                .collect();
            let failure = step.get("failure").and_then(Value::as_array).map(|a| {
                (
                    a[0].as_str().expect("code").to_string(),
                    a[1].as_str().expect("sentence").to_string(),
                )
            });
            *INFO.lock() = Some((lines, failure));
        }
        other => panic!("unknown op {other}"),
    }));
    if r.is_err() {
        push(json!(["error"]));
    }
}

#[test]
fn scenarios_match_python() {
    let fixture = load("vehicle_spawner");
    let (mut steps, mut events, mut states) = (0, 0, 0);
    for sc in fixture["scenarios"].as_array().expect("scenarios") {
        let _g = setup();
        let name = sc["name"].as_str().expect("name");
        for (i, st) in sc["steps"].as_array().expect("steps").iter().enumerate() {
            EVENTS.lock().clear();
            run_step(&st["step"]);
            let got: Vec<Value> = EVENTS.lock().clone();
            let want: Vec<Value> = st["events"]
                .as_array()
                .expect("events")
                .iter()
                .map(normalize)
                .collect();
            steps += 1;
            events += want.len();
            assert_eq!(
                got, want,
                "scenario {name} step {i} {}: events differ",
                st["step"]
            );
            if !st["state"].is_null() {
                states += 1;
                let snap = vs::snapshot_for_tests();
                assert_eq!(
                    snap, st["state"],
                    "scenario {name} step {i} {}: state differs",
                    st["step"]
                );
            }
        }
        vs::reset_for_tests();
    }
    assert!(
        steps > 700 && events > 500 && states > 250,
        "fixture coverage shrank: {steps} steps, {events} events, {states} states"
    );
}

#[test]
fn pure_helpers_match_python() {
    let fixture = load("vehicle_spawner");
    let pure = &fixture["pure"];
    for c in pure["offsets"].as_array().expect("offsets") {
        let got = vs::compute_arrangement_offsets(
            c["type"].as_str().expect("type"),
            c["variant"].as_str().expect("variant"),
            c["n"].as_i64().expect("n"),
            f(&c["s"]),
        );
        let want: Vec<(f64, f64)> = c["out"]
            .as_array()
            .expect("out")
            .iter()
            .map(|p| (f(&p[0]), f(&p[1])))
            .collect();
        assert_eq!(got.len(), want.len(), "offsets {c}");
        for (g, w) in got.iter().zip(&want) {
            assert!(
                same_f64(g.0, w.0) && same_f64(g.1, w.1),
                "offsets {c}: {got:?}"
            );
        }
    }
    for c in pure["repeat"].as_array().expect("repeat") {
        let got = vs::place_repeat_interval(f(&c["held"]));
        assert!(same_f64(got, f(&c["out"])), "repeat {c}: {got}");
    }
    for c in pure["cross"].as_array().expect("cross") {
        let got = vs::place_cross_rung(
            c["old"].as_i64().expect("old"),
            c["new"].as_i64().expect("new"),
        );
        assert_eq!(got, c["out"].as_i64().expect("out"), "cross {c}");
    }
}

fn load_catalog() {
    for p in [
        "CATALOG_BEGIN:1",
        r#"CATALOG_ITEM:{"model":"covet","name":"Covet","brand":"Ibishu","configs":[{"key":"a","name":"A","isDefault":true}]}"#,
        "CATALOG_END",
    ] {
        vs::handle_packet(p);
    }
}

#[test]
fn modal_hooks_suppress_and_enqueue() {
    let _g = setup();
    load_catalog();
    // F11 goes through the suppressing hotkey; the toggle runs on the spawner's worker.
    F11.call_once(vs::install_f11_hotkey);
    assert!(!keyboard::deliver_for_tests("f11", &KeyEvent::down("f11")));
    assert!(!keyboard::deliver_for_tests("f11", &KeyEvent::up("f11")));
    assert!(!vs::is_modal_open(), "the hook only enqueues");
    assert_eq!(vs::run_pending_for_tests(), 1);
    assert!(vs::is_modal_open());
    assert_eq!(vs::snapshot_for_tests()["hooks"], 42);
    EVENTS.lock().clear();

    // A modal key's down is eaten and handled on the worker; its up passes, unless the
    // editor hooks the release too (the arrows and editor letters).
    assert!(!keyboard::deliver_for_tests("f", &KeyEvent::down("f")));
    assert!(keyboard::deliver_for_tests("f", &KeyEvent::up("f")));
    assert!(!keyboard::deliver_for_tests(
        "right",
        &KeyEvent::down("right")
    ));
    assert!(!keyboard::deliver_for_tests(
        "right",
        &KeyEvent::up("right")
    ));
    assert!(EVENTS.lock().is_empty(), "nothing runs inside the hook");
    assert_eq!(vs::run_pending_for_tests(), 3);
    assert_eq!(
        EVENTS.lock().clone(),
        vec![json!(["say", "Filters. Brand: Ibishu, unchecked. 1 of 1."])],
        "f opens the filter dialog; right on it does nothing; the release is a no-op"
    );

    // Closing releases every key.
    vs::close_modal();
    assert_eq!(vs::snapshot_for_tests()["hooks"], 0);
    assert!(keyboard::deliver_for_tests("f", &KeyEvent::down("f")));
    assert!(keyboard::deliver_for_tests("f", &KeyEvent::up("f")));
    assert!(keyboard::deliver_for_tests(
        "right",
        &KeyEvent::down("right")
    ));
    assert!(keyboard::deliver_for_tests("right", &KeyEvent::up("right")));
    assert_eq!(vs::run_pending_for_tests(), 0);
    vs::reset_for_tests();
}

#[test]
fn closing_while_the_editor_is_up_parks_it() {
    let _g = setup();
    load_catalog();
    vs::toggle_modal();
    for k in ["enter", "enter", "left", "tab", "w", "enter"] {
        vs::dispatch_key(k, &KeyEvent::down(k));
    }
    let snap = vs::snapshot_for_tests();
    assert_eq!(snap["screen"], "place3d");
    assert_eq!(snap["place_active"], true);
    vs::dispatch_key("up", &KeyEvent::down("up"));
    vs::close_modal();
    let snap = vs::snapshot_for_tests();
    assert_eq!(snap["place_active"], false);
    assert_eq!(snap["modal"], false);
    // A held key no longer steps once the modal is gone.
    EVENTS.lock().clear();
    *CLOCK.lock() = 1005.0;
    vs::tick_for_tests(1005.0);
    assert!(EVENTS.lock().is_empty());
    vs::reset_for_tests();
}
