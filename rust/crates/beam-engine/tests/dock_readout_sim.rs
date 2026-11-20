//! `diagnostic/dock_readout_sim.py`: the docking cane tap, the ramp readout, the ramp align
//! outcome and the deck readout, against the real phrasing and the real listener.
//!
//! Sign errors here do not announce themselves: a bearing that says "right" when it means
//! "left" reads as perfectly plausible speech. Scenario numbers follow the Python sim
//! (which has two scenarios numbered 18, kept as 18 and 18b). The sim mirrors the logic and
//! scrapes beamtel.py for the thresholds; this calls the ported functions and constants
//! directly. Its `fmt` (metric, two decimals) is `UNITS_MODE = "metric"`, its `fmt_ft` is
//! imperial. The entry and corridor edge counters drive the real `ImplementListener`.

use beam_audio::consts::DOCK_RAMP_MAX_RANGE_M;
use beam_engine::listeners::dock_phrases::*;
use beam_engine::listeners::implement::{ImplementListener, ImplementSink, parse_dock_line};
use beam_engine::state::{DockReading, State, state};
use beam_engine::{speech_out, units};
use parking_lot::Mutex;

const LEVEL_M: f64 = IMPL_DOCK_LEVEL_M;
const YAW_DEG: f64 = IMPL_DOCK_YAW_DEG;
const ENTRY_MIN_M: f64 = IMPL_DOCK_ENTRY_MIN_M;
const ENTRY_EXIT_M: f64 = IMPL_DOCK_ENTRY_EXIT_M;
const CENTRE_M: f64 = RAMP_DOCK_CENTRE_M;
const HEADING_ZERO_DEG: f64 = RAMP_DOCK_HEADING_ZERO_DEG;
const TIGHT_M: f64 = RAMP_DOCK_TIGHT_M;
const PITCH_DEG: f64 = RAMP_DOCK_PITCH_DEG;
const CORRIDOR_ENTER_M: f64 = RAMP_CORRIDOR_ENTER_M;
const CORRIDOR_EXIT_M: f64 = RAMP_CORRIDOR_EXIT_M;
const TONE_RANGE_M: f64 = DOCK_RAMP_MAX_RANGE_M;

fn metric() -> parking_lot::MutexGuard<'static, ()> {
    let g = speech_out::TEST_SERIAL.lock();
    speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
    speech_out::reset_for_tests();
    *state() = State::default();
    units::set_units_mode("metric");
    g
}

fn make() -> DockReading {
    DockReading {
        name: "pallet".into(),
        range: 1.0,
        lateral: 0.0,
        vertical: 0.0,
        idx: 1,
        count: 3,
        kind: "GAP".into(),
        lo_z: 0.0,
        hi_z: 0.15,
        yaw: 0.0,
        manual: false,
        impl_word: "Forks".into(),
        entry_theta: Some(0.0),
        entry_depth: Some(1.2),
        mode: "IMPL".into(),
        margin: None,
        acquire: false,
    }
}

fn make_ramp() -> DockReading {
    DockReading {
        name: "large cannon".into(),
        mode: "RAMP".into(),
        range: 8.0,
        idx: 0,
        count: 0,
        kind: "RAMP".into(),
        hi_z: 0.0,
        impl_word: "Bucket".into(),
        entry_depth: Some(-1.0),
        margin: Some(0.9),
        ..make()
    }
}

fn with(mut d: DockReading, f: impl FnOnce(&mut DockReading)) -> DockReading {
    f(&mut d);
    d
}

fn p(d: &DockReading) -> String {
    dock_phrase(d)
}

#[derive(Default)]
struct Count {
    entry: Mutex<usize>,
    approach: Mutex<usize>,
}

impl ImplementSink for Count {
    fn say(&self, _: &str) {}
    fn send_implement_cmd(&self, _: &str) {}
    fn clear_dock_target(&self) {}
    fn clear_cannon_aim(&self) {}
    fn update_cannon_aim(&self, _: f64, _: f64, _: f64, _: bool, _: bool) {}
    fn trigger_slam_cue(&self, _: &str) {}
    fn update_dock_target(&self, _: f64, _: f64, _: f64, _: &str, _: bool, _: f64) {}
    fn trigger_dock_approach_cue(&self) {
        *self.approach.lock() += 1;
    }
    fn trigger_entry_cue(&self) {
        *self.entry.lock() += 1;
    }
}

/// The one-shot earcon trigger through the real listener; `None` is target loss.
fn entry_edges(depths: &[Option<f64>]) -> usize {
    let sink = Count::default();
    let mut l = ImplementListener::start(&sink, 0.0);
    for d in depths {
        match d {
            None => l.handle_text(&sink, "DOCKCLEAR", 1.0, 1.0),
            Some(d) => l.handle_text(
                &sink,
                &format!("DOCK:pallet,1.0,0.0,0.0,1,3,GAP,0.0,0.15,0.0,0,0.0,{d}"),
                1.0,
                1.0,
            ),
        }
    }
    *sink.entry.lock()
}

/// Hunting-to-approach handoffs through the real listener; `None` is target loss.
fn approach_edges(samples: &[Option<(f64, f64)>]) -> usize {
    let sink = Count::default();
    let mut l = ImplementListener::start(&sink, 0.0);
    for s in samples {
        match s {
            None => l.handle_text(&sink, "DOCKCLEAR", 1.0, 1.0),
            Some((r, lat)) => l.handle_text(
                &sink,
                &format!("DOCK:c,{r},{lat},0.0,0,0,RAMP,0,0,0.0,0,0.0,-1.0,RAMP,0.9"),
                1.0,
                1.0,
            ),
        }
    }
    *sink.approach.lock()
}

#[test]
fn scenario_01_vertical_sign() {
    let _g = metric();
    assert!(p(&with(make(), |d| d.vertical = 0.30)).contains("raise"));
    assert!(p(&with(make(), |d| d.vertical = -0.30)).contains("lower"));
    let both = p(&with(make(), |d| d.vertical = 0.3));
    assert!(!(both.contains("raise") && both.contains("lower")));
    let low = p(&with(make(), |d| d.vertical = -0.30));
    let after = low
        .split("lower")
        .nth(1)
        .unwrap()
        .split('.')
        .next()
        .unwrap();
    assert!(!after.contains('-'), "magnitude is spoken unsigned: {low}");
}

#[test]
fn scenario_02_lateral_sign() {
    let _g = metric();
    assert!(p(&with(make(), |d| d.lateral = 0.4)).contains("left"));
    assert!(p(&with(make(), |d| d.lateral = -0.4)).contains("right"));
}

#[test]
fn scenario_03_dead_zone() {
    let _g = metric();
    let jitter = LEVEL_M * 0.8;
    let s = p(&with(make(), |d| {
        d.vertical = jitter;
        d.lateral = -jitter
    }));
    assert!(s.contains("level"), "{s}");
    assert!(s.contains("centred"), "{s}");
    assert!(!s.contains("raise") && !s.contains("lower"));
    assert!(p(&with(make(), |d| d.vertical = LEVEL_M * 1.2)).contains("raise"));
}

#[test]
fn scenario_04_squareness() {
    let _g = metric();
    assert!(!p(&with(make(), |d| d.yaw = YAW_DEG * 0.5)).contains("face"));
    assert!(p(&with(make(), |d| d.yaw = YAW_DEG * 2.0)).contains("face"));
    assert!(p(&with(make(), |d| d.yaw = 20.0)).contains("degrees left"));
    assert!(p(&with(make(), |d| d.yaw = -20.0)).contains("degrees right"));
}

#[test]
fn scenario_05_band_naming() {
    assert_eq!(band_name("GAP", 1, 4), "underside");
    assert_eq!(band_name("GAP", 3, 4), "opening");
    assert_eq!(band_name("SOLID", 4, 4), "roof");
    assert_eq!(band_name("SOLID", 1, 4), "base");
    assert_eq!(band_name("SOLID", 2, 4), "body");
    assert_eq!(band_name("SOLID", 1, 1), "roof", "idx == count wins");
}

#[test]
fn scenario_06_manual_is_held() {
    let _g = metric();
    assert!(p(&with(make(), |d| d.manual = true)).contains("held"));
    assert!(!p(&with(make(), |d| d.manual = false)).contains("held"));
}

#[test]
fn scenario_07_negative_range_floored() {
    let _g = metric();
    let s = p(&with(make(), |d| d.range = -0.02));
    assert!(s.contains("range 0.0"), "{s}");
}

#[test]
fn scenario_08_entry_gate() {
    let _g = metric();
    assert!(!p(&with(make(), |d| d.entry_depth = Some(ENTRY_MIN_M * 1.5))).contains("too steep"));
    assert!(p(&with(make(), |d| d.entry_depth = Some(0.15))).contains("too steep"));
    assert!(p(&with(make(), |d| d.entry_depth = Some(0.15))).contains("tines enter 0.15"));
    assert!(!p(&with(make(), |d| d.entry_depth = Some(-1.0))).contains("too steep"));
    assert!(!p(&with(make(), |d| d.entry_depth = None)).contains("too steep"));
}

#[test]
fn scenario_09_parse_tolerates_older_mod() {
    let new_line = "DOCK:pallet,1.240,-0.120,0.220,2,5,GAP,0.350,0.520,-14.0,1,36.0,0.187";
    let old_line = "DOCK:pallet,1.240,-0.120,0.220,2,5,GAP,0.350,0.520,-14.0,1";
    let n = parse_dock_line(new_line, "Forks").expect("the long form parses");
    assert!((n.entry_theta.unwrap() - 36.0).abs() < 1e-9);
    assert!((n.entry_depth.unwrap() - 0.187).abs() < 1e-9);
    let o = parse_dock_line(old_line, "Forks").expect("the short form still parses");
    assert!(o.kind == "GAP" && (o.yaw + 14.0).abs() < 1e-9 && o.manual);
    assert!(o.entry_theta.is_none() && o.entry_depth.is_none());
    let q = parse_dock_line(
        "DOCK:pallet, stacked,1.240,-0.120,0.220,2,5,GAP,0.350,0.520,-14.0,1,36.0,0.187",
        "Forks",
    )
    .unwrap();
    assert!(q.idx == 2 && (q.entry_depth.unwrap() - 0.187).abs() < 1e-9);
}

#[test]
fn scenario_10_entry_earcon() {
    let _g = metric();
    let s = |x: &[f64]| x.iter().map(|v| Some(*v)).collect::<Vec<_>>();
    assert_eq!(
        entry_edges(&s(&[1.2, 1.2, 1.2])),
        0,
        "arriving enterable is not an event"
    );
    assert_eq!(entry_edges(&s(&[0.15, 0.15, 1.2, 1.2])), 1);
    assert_eq!(entry_edges(&s(&[0.15, 1.2, 0.15, 1.2])), 2);
    let mut seq = s(&[0.15, 1.2]);
    for i in 0..200 {
        seq.push(Some(
            (ENTRY_MIN_M + ENTRY_EXIT_M) / 2.0 + 0.002 * if i % 2 == 1 { 1.0 } else { -1.0 },
        ));
    }
    assert_eq!(
        entry_edges(&seq),
        1,
        "wobble between the thresholds fires nothing more"
    );
    assert_eq!(entry_edges(&[Some(0.15), Some(1.2), None, Some(1.2)]), 1);
}

#[test]
fn scenario_11_three_wire_shapes() {
    let p15 = parse_dock_line(
        "DOCK:large cannon,8.000,-0.400,0.000,0,0,RAMP,1.200,1.200,6.0,0,2.0,-1.000,RAMP,0.900",
        "Forks",
    )
    .unwrap();
    assert_eq!(p15.mode, "RAMP");
    assert!((p15.margin.unwrap() - 0.9).abs() < 1e-9);
    let p13 = parse_dock_line(
        "DOCK:pallet,1.000,0.100,0.220,2,5,GAP,0.350,0.520,-14.0,1,3.0,0.450",
        "Forks",
    )
    .unwrap();
    assert_eq!(p13.entry_depth, Some(0.45));
    let p11 = parse_dock_line(
        "DOCK:pallet,1.000,0.100,0.220,2,5,GAP,0.350,0.520,-14.0,1",
        "Forks",
    )
    .unwrap();
    assert!(p11.entry_depth.is_none());
    assert!(p13.mode == "IMPL" && p11.mode == "IMPL");
    assert!(p13.margin.is_none() && p11.margin.is_none());
    let comma = parse_dock_line(
        "DOCK:Large, Cannon,8.000,-0.400,0.000,0,0,RAMP,1.200,1.200,6.0,0,2.0,-1.000,RAMP,0.900",
        "Forks",
    )
    .unwrap();
    assert!(comma.name == "Large, Cannon" && (comma.margin.unwrap() - 0.9).abs() < 1e-9);
}

#[test]
fn scenario_12_ramp_signs() {
    let _g = metric();
    let left = p(&with(make_ramp(), |d| d.lateral = 0.40));
    let right = p(&with(make_ramp(), |d| d.lateral = -0.40));
    assert!(left.contains("left") && !left.contains("right"), "{left}");
    assert!(
        right.contains("right") && !right.contains("left"),
        "{right}"
    );
    assert!(p(&with(make_ramp(), |d| d.yaw = 12.0)).contains("heading 12 degrees left"));
    assert!(p(&with(make_ramp(), |d| d.yaw = -12.0)).contains("heading 12 degrees right"));
}

#[test]
fn scenario_13_no_implement_wording() {
    let _g = metric();
    let banned = [
        "raise",
        "lower",
        "band",
        "tall",
        "held",
        "tines",
        "opening",
        "underside",
        "roof",
        "base",
        "body",
        "level",
    ];
    for d in [
        make_ramp(),
        with(make_ramp(), |d| {
            d.vertical = 0.9;
            d.lateral = -2.0;
            d.yaw = -30.0;
            d.margin = Some(-0.4)
        }),
        with(make_ramp(), |d| {
            d.range = 0.0;
            d.margin = Some(0.05)
        }),
        with(make_ramp(), |d| d.entry_theta = Some(12.0)),
    ] {
        let out = p(&d).to_lowercase();
        let hit: Vec<_> = banned.iter().filter(|w| out.contains(*w)).collect();
        assert!(hit.is_empty(), "{out:?}: {hit:?}");
    }
}

#[test]
fn scenario_14_clearance_is_advice() {
    let _g = metric();
    let fine = p(&with(make_ramp(), |d| d.margin = Some(0.9)));
    assert!(
        !fine.contains("clearance") && !fine.contains("0.9"),
        "{fine}"
    );
    assert!(p(&with(make_ramp(), |d| d.margin = Some(TIGHT_M - 0.05))).contains("tight"));
    assert!(p(&with(make_ramp(), |d| d.margin = Some(-0.30))).contains("too narrow by"));
    let sentinel = p(&with(make_ramp(), |d| d.margin = Some(-1.0)));
    assert!(!sentinel.contains("clearance") && !sentinel.contains("narrow"));
    assert!(!p(&with(make_ramp(), |d| d.margin = None)).contains("clearance"));
}

#[test]
fn scenario_15_ramp_dead_zones() {
    let _g = metric();
    const { assert!(CENTRE_M > LEVEL_M) };
    assert!(p(&with(make_ramp(), |d| d.lateral = CENTRE_M - 0.01)).contains("centred"));
    assert!(p(&with(make_ramp(), |d| d.lateral = CENTRE_M)).contains("centred"));
    let out = p(&with(make_ramp(), |d| d.lateral = CENTRE_M + 0.05));
    assert!(out.starts_with("0.2 "), "{out}");
    assert!(p(&with(make_ramp(), |d| d.yaw = 0.0)).contains("heading zero degrees"));
    assert!(
        p(&with(make_ramp(), |d| d.yaw = HEADING_ZERO_DEG - 0.01)).contains("heading zero degrees")
    );
    assert!(p(&with(make_ramp(), |d| d.yaw = HEADING_ZERO_DEG)).contains("heading 1 degrees left"));
    assert!(p(&with(make_ramp(), |d| d.yaw = 2.0)).contains("heading 2 degrees left"));
    let plain = p(&with(make_ramp(), |d| {
        d.lateral = -0.4;
        d.yaw = 2.0;
        d.margin = Some(0.9);
        d.entry_theta = Some(1.0)
    }));
    assert!(
        plain.matches(". ").count() == 1 && plain.split_whitespace().count() <= 10,
        "{plain}"
    );
    assert!(!p(&with(make_ramp(), |d| d.entry_theta = Some(1.0))).contains("ramp up"));
    assert!(p(&with(make_ramp(), |d| d.entry_theta = Some(PITCH_DEG + 2.0))).contains("ramp up"));
}

#[test]
fn scenario_16_implement_unchanged_by_split() {
    let _g = metric();
    let cases = [
        make(),
        with(make(), |d| d.vertical = 0.22),
        with(make(), |d| d.vertical = -0.22),
        with(make(), |d| d.lateral = 0.12),
        with(make(), |d| d.lateral = -0.12),
        with(make(), |d| {
            d.yaw = -14.0;
            d.manual = true
        }),
        with(make(), |d| d.range = -0.5),
        with(make(), |d| d.entry_depth = Some(0.05)),
        with(make(), |d| d.entry_depth = Some(-1.0)),
        with(make(), |d| {
            d.kind = "SOLID".into();
            d.idx = 5;
            d.count = 5
        }),
    ];
    assert!(cases.iter().all(|c| p(c) == dock_phrase_impl(c)));
    assert!(cases.iter().all(|c| !p(c).contains("Cannon ramp")));
}

#[test]
fn scenario_17_hunting_is_a_different_question() {
    let _g = metric();
    let behind = with(make_ramp(), |d| {
        d.range = -9.0;
        d.lateral = 4.2;
        d.yaw = 123.0;
        d.margin = Some(-3.1);
        d.acquire = true
    });
    let said = p(&behind);
    assert!(!said.split(',').nth(1).unwrap().contains("0.0"), "{said}");
    assert!(said.contains("wrong side"), "{said}");

    let words = ["ahead", "behind", "left", "right", "front", "back"];
    let mut mixed = Vec::new();
    for along in [-40.0, -12.0, -3.0, -0.6, 0.0, 3.0, 12.0, 40.0] {
        for across in [-9.0, -4.0, 0.0, 4.0, 9.0] {
            for face in [-170.0, -95.0, -20.0, 0.0, 20.0, 95.0, 170.0] {
                let d = with(make_ramp(), |d| {
                    d.range = along;
                    d.lateral = across;
                    d.yaw = face;
                    d.acquire = ramp_acquire(along, across, true)
                });
                let out = p(&d);
                let hit: Vec<_> = words.iter().filter(|w| out.contains(*w)).collect();
                if d.acquire && hit.len() > 1 {
                    mixed.push(out);
                }
            }
        }
    }
    assert!(mixed.is_empty(), "{:?}", mixed.first());
    let old = "ahead, 73.9 feet. behind";
    assert!(words.iter().filter(|w| old.contains(*w)).count() > 1);
    assert!(said.starts_with("82 right"), "{said}");
    assert!(!said.contains("right 13") && !said.contains("left 4"));
    assert!(!said.contains("narrow") && !said.contains("clearance"));

    assert!(ramp_bearing_deg(10.0, 0.0, 0.0).abs() < 1e-9);
    assert!(ramp_bearing_deg(10.0, 5.0, 0.0) > 0.0);
    assert!(ramp_bearing_deg(10.0, -5.0, 0.0) < 0.0);
    assert!((ramp_bearing_deg(-9.0, 0.0, 0.0).abs() - 180.0).abs() < 1e-9);
    let (b0, b1) = (
        ramp_bearing_deg(10.0, 0.0, 0.0),
        ramp_bearing_deg(10.0, 0.0, 30.0),
    );
    assert!(((b1 - b0) - 30.0).abs() < 1e-9);

    const { assert!(CORRIDOR_ENTER_M >= 6.0) };
    const { assert!(CORRIDOR_EXIT_M >= CORRIDOR_ENTER_M * 1.5) };
    assert!(ramp_acquire(-0.2, 0.0, false));
    assert!(ramp_acquire(10.0, CORRIDOR_EXIT_M + 0.1, false));
    assert!(!ramp_acquire(10.0, CORRIDOR_ENTER_M + 0.1, false));
    assert!(
        ramp_acquire(10.0, CORRIDOR_ENTER_M + 0.1, true)
            && !ramp_acquire(10.0, CORRIDOR_ENTER_M - 0.1, true)
    );
    let (mut wobble, mut flips) = (false, 0);
    for i in 0..200 {
        let lat = CORRIDOR_ENTER_M + 0.02 * if i % 2 == 1 { 1.0 } else { -1.0 };
        let nxt = ramp_acquire(12.0, lat, wobble);
        if nxt != wobble {
            flips += 1;
        }
        wobble = nxt;
    }
    assert!(flips <= 1, "{flips} changes");
    let handoffs = approach_edges(&[
        Some((12.0, CORRIDOR_ENTER_M + 1.0)),
        Some((12.0, CORRIDOR_ENTER_M - 0.1)),
        Some((12.0, CORRIDOR_ENTER_M + 0.1)),
        Some((12.0, CORRIDOR_EXIT_M - 0.1)),
    ]);
    assert_eq!(handoffs, 1);
    assert_eq!(
        approach_edges(&[
            Some((12.0, CORRIDOR_ENTER_M - 0.1)),
            Some((12.0, CORRIDOR_EXIT_M + 0.1)),
            Some((12.0, CORRIDOR_ENTER_M - 0.1)),
        ]),
        2
    );
    assert_eq!(
        approach_edges(&[
            Some((12.0, CORRIDOR_ENTER_M - 0.1)),
            None,
            Some((12.0, CORRIDOR_ENTER_M - 0.1)),
        ]),
        2
    );
}

#[test]
fn scenario_18_too_far_for_tones() {
    let _g = metric();
    let out = p(&with(make_ramp(), |d| {
        d.range = -30.0;
        d.lateral = 6.0;
        d.yaw = 100.0;
        d.acquire = true
    }));
    assert!(out.starts_with(|c: char| ('1'..='9').contains(&c)), "{out}");
    assert!(out.contains("too far for tones"), "{out}");
    let near = p(&with(make_ramp(), |d| {
        d.range = 6.0;
        d.lateral = 0.3
    }));
    assert!(!near.contains("too far"), "{near}");
    const { assert!(TONE_RANGE_M < 45.0) };
}

#[test]
fn scenario_18b_ramp_align_readout() {
    let _g = metric();
    let ok_wide = ramp_align_phrase("OK,large cannon,6.10,0.85");
    assert!(
        !ok_wide.contains("fit") && !ok_wide.contains("tight"),
        "{ok_wide}"
    );
    assert!(
        ok_wide.contains("large cannon") && ok_wide.contains("6.1"),
        "{ok_wide}"
    );
    units::set_units_mode("imperial");
    assert!(
        ramp_align_phrase("OK,large cannon,6.10,0.85")
            .find("20.0 feet")
            .unwrap()
            > 0
    );
    let stowed = ramp_align_phrase("OK,us semi,6.10,0.85,1.30");
    let tilted = ramp_align_phrase("OK,us semi,6.10,0.85,0.95");
    let ready = ramp_align_phrase("OK,us semi,6.10,0.85,0.15");
    let both = ramp_align_phrase("OK,us semi,6.10,0.04,1.30");
    units::set_units_mode("metric");
    assert!(ramp_align_phrase("OK,tilt deck,6.10,-1.00").contains("you do not fit"));
    assert!(ramp_align_phrase("OK,dry van,6.10,0.00").contains("you do not fit"));
    assert!(ramp_align_phrase("OK,dry van,6.10,0.04").ends_with(", tight"));
    let na = ramp_align_phrase("OK,large cannon,6.10,NA");
    assert!(!na.contains("fit") && !na.contains("tight"), "{na}");
    assert!(stowed.contains("ramp not down"), "{stowed}");
    assert!(stowed.contains("4.3 feet up"), "{stowed}");
    assert!(tilted.contains("ramp not down"), "{tilted}");
    assert!(!ready.contains("lip"), "{ready}");
    assert!(ramp_align_phrase("OK,us semi,6.10,0.04,0.15").ends_with(", tight"));
    assert!(!ramp_align_phrase("OK,us semi,6.10,0.85,NA").contains("lip"));
    assert!(!ramp_align_phrase("OK,us semi,6.10,0.85").contains("lip"));
    assert!(
        both.find("not down").unwrap() < both.find("tight").unwrap(),
        "{both}"
    );
    assert_eq!(
        ramp_align_phrase("FAIL,nearest is Old Cannon, 5 metres, no ramp on it"),
        "nearest is Old Cannon, 5 metres, no ramp on it"
    );
    assert_eq!(ramp_align_phrase("WAT"), "");
}

#[test]
fn scenario_19_deck_readout() {
    let _g = metric();
    let home_s = "0.0,1.30;upfit_tilt:1:52;upfit_extendRetract:0:552;upfit_extendRetractFeet:0:22";
    let work_s =
        "10.1,0.15;upfit_tilt:100:52;upfit_extendRetract:100:552;upfit_extendRetractFeet:0:22";
    let rp = |s: &str| ramp_self_phrase(&parse_ramp_self(s).unwrap());
    units::set_units_mode("imperial");
    let home = rp(home_s);
    let work = rp(work_s);
    let tilt_only = rp("10.2,0.95;upfit_tilt:100:52;upfit_extendRetract:0:552");
    units::set_units_mode("metric");

    assert!(home.starts_with("Ramp level"), "{home}");
    assert!(work.starts_with("Ramp up 10 degrees"), "{work}");
    assert!(work.contains("Lip on the ground"), "{work}");
    assert!(home.contains("Lip 4.3 feet up"), "{home}");
    assert!(tilt_only.contains("Lip 3.1 feet up"), "{tilt_only}");
    assert!(!rp("0.0,-999.00;upfit_tilt:1:52").contains("Lip"));
    assert!(!rp("0.0;upfit_tilt:1:52").contains("Lip"));
    assert!(work.contains("extend retract 18.1 of 18.1 feet"), "{work}");
    assert!(work.contains("tilt 100 percent"), "{work}");
    assert!(home.contains("extend retract 0.0 of 18.1 feet"), "{home}");
    assert!(!work.contains("upfit"), "{work}");
    let lone = rp("0.0;upfit_tilt:34:52");
    assert_eq!(lone, "Ramp level. upfit tilt 34 percent");
    // The always-strip negative control: stripping a lone group loses the only descriptive
    // word, so the guarded form must not match it.
    assert_ne!(lone, "Ramp level. tilt 34 percent");
    assert!(rp("0.0;deck_tilt:5:52;bed_slide:5:52").contains("deck"));
    assert_eq!(ramp_hyd_words("extendRetractFeet"), "extend retract feet");
    let sentinel = rp("-999.0;upfit_tilt:1:52;upfit_extendRetract:0:552");
    assert!(sentinel.starts_with("Ramp angle unavailable"), "{sentinel}");
    assert!(!sentinel.contains("999"), "{sentinel}");
    assert!(sentinel.contains("extend retract"), "{sentinel}");
    assert_eq!(rp("-25.0;"), "Ramp down 25 degrees");
    assert!(parse_ramp_self("NONE").is_none());
    assert!(parse_ramp_self("").is_none());
    assert!(parse_ramp_self("wat;x:y:z").is_none());
    assert_eq!(rp("12.0;"), "Ramp up 12 degrees");
    assert_eq!(
        rp("0.0;broken;upfit_tilt:7:52"),
        "Ramp level. upfit tilt 7 percent"
    );
}
