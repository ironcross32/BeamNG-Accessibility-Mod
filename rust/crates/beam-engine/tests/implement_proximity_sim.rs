//! `diagnostic/implement_proximity_sim.py`: the implement proximity speech state machine
//! against a fake clock. The point is negative: it must STAY QUIET when it should.
//!
//! The Python sim mirrors `implement_listener` with simplified wording ("approaching X
//! LEVEL", "now BELOW", "under", "out from under"); this drives the real
//! `ImplementListener` with `NEAR:` / `CLEAR` / `DOCK:` datagrams and matches the real
//! wording. Scenario numbers and order follow the sim.

use beam_engine::listeners::dock_phrases::{
    IMPL_PROX_ENTER_M, IMPL_PROX_INSIDE_HOLD, IMPL_PROX_LEAVE_HOLD, IMPL_PROX_LEAVE_M,
    IMPL_PROX_RELATION_HOLD,
};
use beam_engine::listeners::implement::{ImplementListener, ImplementSink};
use beam_engine::state::{State, state};
use beam_engine::{speech_out, units};
use parking_lot::Mutex;

const TICK: f64 = 0.1; // the Lua side scans at 10 Hz
const NAME: &str = "Gavril D-Series";

#[derive(Default)]
struct Said(Mutex<Vec<String>>);

impl ImplementSink for Said {
    fn say(&self, t: &str) {
        self.0.lock().push(t.to_string());
    }
    fn send_implement_cmd(&self, _: &str) {}
    fn clear_dock_target(&self) {}
    fn clear_cannon_aim(&self) {}
    fn update_cannon_aim(&self, _: f64, _: f64, _: f64, _: bool, _: bool) {}
    fn trigger_slam_cue(&self, _: &str) {}
    fn update_dock_target(&self, _: f64, _: f64, _: f64, _: &str, _: bool, _: f64) {}
    fn trigger_dock_approach_cue(&self) {}
    fn trigger_entry_cue(&self) {}
}

fn setup() -> parking_lot::MutexGuard<'static, ()> {
    let g = speech_out::TEST_SERIAL.lock();
    *state() = State::default();
    units::set_units_mode("metric");
    g
}

struct Run {
    sink: Said,
    l: ImplementListener,
    t: f64,
}

impl Run {
    fn new() -> Run {
        let sink = Said::default();
        let l = ImplementListener::start(&sink, 0.0);
        Run { sink, l, t: 0.0 }
    }
    fn send(&mut self, text: &str) {
        self.l.handle_text(&self.sink, text, self.t, self.t);
        self.t += TICK;
    }
    fn near(&mut self, name: &str, dist: f64, relation: &str, inside: bool) {
        let line = format!(
            "NEAR:{name},{dist},{relation},{},0",
            if inside { 1 } else { 0 }
        );
        self.send(&line);
    }
    fn said(&self) -> Vec<String> {
        self.sink.0.lock().clone()
    }
}

/// steps: (dist or None for a CLEAR packet, relation, inside).
fn run(steps: &[(Option<f64>, &str, bool)]) -> Vec<String> {
    let mut r = Run::new();
    for (d, rel, inside) in steps {
        match d {
            None => r.send("CLEAR"),
            Some(d) => r.near(NAME, *d, rel, *inside),
        }
    }
    r.said()
}

fn rep(n: usize, s: (Option<f64>, &'static str, bool)) -> Vec<(Option<f64>, &'static str, bool)> {
    vec![s; n]
}

fn count(said: &[String], suffix: &str) -> usize {
    said.iter().filter(|s| s.as_str() == suffix).count()
}

#[test]
fn thresholds_are_the_ported_ones() {
    assert_eq!(
        (IMPL_PROX_ENTER_M, IMPL_PROX_LEAVE_M),
        (3.0, 4.5),
        "enter/leave metres"
    );
    assert_eq!(
        (
            IMPL_PROX_RELATION_HOLD,
            IMPL_PROX_INSIDE_HOLD,
            IMPL_PROX_LEAVE_HOLD
        ),
        (0.40, 0.25, 0.30)
    );
}

#[test]
fn scenario_01_approach_and_withdraw() {
    let _g = setup();
    let mut s: Vec<_> = [5.0, 4.0, 3.5, 2.8, 2.0, 1.5]
        .iter()
        .map(|d| (Some(*d), "LEVEL", false))
        .collect();
    s.extend(
        [2.0, 3.0, 4.0, 5.0, 5.5, 6.0, 6.0, 6.0]
            .iter()
            .map(|d| (Some(*d), "LEVEL", false)),
    );
    let said = run(&s);
    assert_eq!(said.len(), 2, "{said:?}");
    assert_eq!(
        said[0],
        format!("Bucket approaching {NAME}, level with it, 2.8 meters")
    );
    assert_eq!(said[1], format!("Clear of {NAME}"));
}

#[test]
fn scenario_02_enter_jitter_speaks_once() {
    let _g = setup();
    let mut s = Vec::new();
    for _ in 0..30 {
        s.push((Some(3.05), "LEVEL", false));
        s.push((Some(2.95), "LEVEL", false));
    }
    let said = run(&s);
    assert_eq!(said.len(), 1, "{said:?}");
}

#[test]
fn scenario_03_leave_jitter_does_not_chatter() {
    let _g = setup();
    let mut s = rep(5, (Some(2.0), "LEVEL", false));
    for _ in 0..30 {
        s.push((Some(4.6), "LEVEL", false));
        s.push((Some(4.4), "LEVEL", false));
    }
    let said = run(&s);
    assert_eq!(
        said.len(),
        1,
        "no leave line while it keeps coming back: {said:?}"
    );
}

#[test]
fn scenario_04_relation_flap_is_silent() {
    let _g = setup();
    let mut s = rep(5, (Some(2.0), "LEVEL", false));
    for _ in 0..30 {
        s.push((Some(2.0), "BELOW", false));
        s.push((Some(2.0), "LEVEL", false));
    }
    let said = run(&s);
    assert_eq!(said.len(), 1, "{said:?}");
}

#[test]
fn scenario_05_genuine_relation_change() {
    let _g = setup();
    let mut s = rep(5, (Some(2.0), "LEVEL", false));
    s.extend(rep(20, (Some(2.0), "BELOW", false)));
    let said = run(&s);
    assert_eq!(said.len(), 2, "{said:?}");
    assert_eq!(
        said[1],
        format!("{NAME}, now below it"),
        "names the new relation"
    );
}

#[test]
fn scenario_06_sliding_under() {
    let _g = setup();
    let mut s = rep(5, (Some(2.0), "LEVEL", false));
    s.extend(rep(20, (Some(0.5), "LEVEL", true)));
    let said = run(&s);
    assert_eq!(count(&said, &format!("Bucket under {NAME}")), 1, "{said:?}");
}

#[test]
fn scenario_07_backing_out() {
    let _g = setup();
    let mut s = rep(5, (Some(2.0), "LEVEL", false));
    s.extend(rep(10, (Some(0.5), "LEVEL", true)));
    s.extend(rep(10, (Some(0.9), "LEVEL", false)));
    let said = run(&s);
    assert_eq!(count(&said, &format!("Bucket under {NAME}")), 1, "{said:?}");
    assert_eq!(
        count(&said, &format!("Bucket clear of {NAME}")),
        1,
        "{said:?}"
    );
}

#[test]
fn scenario_08_inside_flicker_is_silent() {
    let _g = setup();
    let mut s = rep(5, (Some(2.0), "LEVEL", false));
    for _ in 0..30 {
        s.push((Some(0.5), "LEVEL", true));
        s.push((Some(0.5), "LEVEL", false));
    }
    let said = run(&s);
    assert_eq!(said.len(), 1, "{said:?}");
}

#[test]
fn scenario_09_nearer_object_takes_over() {
    let _g = setup();
    let mut r = Run::new();
    for _ in 0..5 {
        r.near(NAME, 2.0, "LEVEL", false);
    }
    for _ in 0..5 {
        r.near("Ibishu Covet", 1.0, "ABOVE", false);
    }
    assert_eq!(
        r.said(),
        vec![
            format!("Bucket approaching {NAME}, level with it, 2.0 meters"),
            "Bucket approaching Ibishu Covet, above it, 1.0 meters".to_string(),
        ]
    );
}

#[test]
fn scenario_10_driving_past_is_silent() {
    let _g = setup();
    let s: Vec<_> = [6.0, 5.0, 4.0, 3.5, 4.0, 5.0, 6.0]
        .iter()
        .map(|d| (Some(*d), "LEVEL", false))
        .collect();
    assert!(run(&s).is_empty());
}

#[test]
fn scenario_11_clear_ends_tracking_once() {
    let _g = setup();
    let mut s = rep(5, (Some(2.0), "LEVEL", false));
    s.extend(rep(20, (None, "", false)));
    let said = run(&s);
    assert_eq!(said.len(), 2, "{said:?}");
}

fn ramp_line() -> &'static str {
    "DOCK:large cannon,12.0,1.0,0.0,0,0,RAMP,0,0,0.0,0,0.0,-1.0,RAMP,0.9"
}
fn impl_line() -> &'static str {
    "DOCK:pallet,1.0,0.0,0.0,1,3,GAP,0.0,0.15,0.0,0,0.0,1.2"
}

fn mode_announcements(said: &[String]) -> Vec<String> {
    said.iter()
        .filter(|s| s.starts_with("Ramp alignment") || s.starts_with("Implement alignment"))
        .cloned()
        .collect()
}

#[test]
fn scenario_12_mode_announced_on_real_change_only() {
    let _g = setup();
    state().dock.dock_mode_active = true;

    let mut r = Run::new();
    for _ in 0..30 {
        r.send(ramp_line());
    }
    assert_eq!(
        mode_announcements(&r.said()),
        vec!["Ramp alignment, large cannon"]
    );

    // Twenty round trips across the feed's edge.
    let mut r = Run::new();
    *state() = State::default();
    state().dock.dock_mode_active = true;
    for _ in 0..20 {
        for _ in 0..5 {
            r.send(ramp_line());
        }
        r.send("DOCKCLEAR");
    }
    assert_eq!(
        mode_announcements(&r.said()).len(),
        1,
        "drifting in and out never re-announces"
    );

    let mut r = Run::new();
    *state() = State::default();
    state().dock.dock_mode_active = true;
    for _ in 0..5 {
        r.send(ramp_line());
    }
    for _ in 0..5 {
        r.send("DOCKFAIL:nothing");
    }
    for _ in 0..5 {
        r.send(ramp_line());
    }
    assert_eq!(
        mode_announcements(&r.said()).len(),
        1,
        "a spell with no reading"
    );

    // Climbing out of the car and into the loader must announce.
    let mut r = Run::new();
    *state() = State::default();
    state().dock.dock_mode_active = true;
    for _ in 0..5 {
        r.send(ramp_line());
    }
    r.send("IMPLEMENT:loader_bucket");
    for _ in 0..5 {
        r.send(impl_line());
    }
    assert_eq!(
        mode_announcements(&r.said()),
        vec!["Ramp alignment, large cannon", "Implement alignment"]
    );

    // The check is discriminating: clearing the latch on DOCKCLEAR WOULD chatter.
    let mut r = Run::new();
    *state() = State::default();
    state().dock.dock_mode_active = true;
    for _ in 0..20 {
        for _ in 0..5 {
            r.send(ramp_line());
        }
        r.send("DOCKCLEAR");
        state().dock.last_dock_mode = None;
        state().dock.last_dock_name = None;
    }
    assert_eq!(mode_announcements(&r.said()).len(), 20);
}
