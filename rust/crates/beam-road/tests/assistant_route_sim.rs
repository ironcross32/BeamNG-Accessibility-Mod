//! diagnostic/assistant_route_sim.py: route guidance wording and callout rules,
//! without the GUI or audio application.

use std::sync::{Arc, Mutex};

use beam_road::assistant_route::*;
use beam_road::driving_assistant::{DrivingAssistant, SpeakOpts};
use beam_road::pyjson::{Value, loads};
use beam_road::road_guidance::{R2Packet, RoadGuidanceFeed, Signal};
use serde_json::{Value as J, json};

const MILE: f64 = 1609.344;
const FOOT: f64 = 1.0 / 3.28084;

fn v(j: J) -> Value {
    loads(&j.to_string()).unwrap()
}

struct R {
    turn_distance: Option<f64>,
    turn: &'static str,
    turn_id: i64,
    dest: f64,
    speed: f64,
    off_road: bool,
}

impl Default for R {
    fn default() -> Self {
        R {
            turn_distance: None,
            turn: "left",
            turn_id: 1,
            dest: 5000.0,
            speed: 10.0,
            off_road: false,
        }
    }
}

fn report(r: R) -> Value {
    let mut value = json!({"destDistance": r.dest, "speed": r.speed});
    if let Some(d) = r.turn_distance {
        value["turnId"] = json!(r.turn_id);
        value["turn"] = json!(r.turn);
        value["turnDistance"] = json!(d);
    }
    if r.off_road {
        value["offRoad"] = json!(true);
    }
    v(value)
}

fn turn(d: f64) -> R {
    R {
        turn_distance: Some(d),
        ..R::default()
    }
}

// --- Wording ---------------------------------------------------------------------

#[test]
fn test_distances_are_fit_to_speak() {
    assert_eq!(spoken_distance(437.0 * FOOT, "imperial"), "450 feet");
    assert_eq!(spoken_distance(MILE, "imperial"), "1 mile");
    assert_eq!(spoken_distance(3.2 * MILE, "imperial"), "3.2 miles");
    assert_eq!(spoken_distance(2.0 * MILE, "imperial"), "2 miles");
    assert_eq!(spoken_distance(437.0, "metric"), "450 meters");
    assert_eq!(spoken_distance(1000.0, "metric"), "1 kilometer");
    assert_eq!(spoken_distance(3200.0, "metric"), "3.2 kilometers");
    assert_eq!(spoken_distance(2000.0, "metric"), "2 kilometers");
    assert_eq!(spoken_distance(3.0, "metric"), "50 meters");
}

#[test]
fn test_instructions() {
    assert_eq!(turn_instruction("left"), "turn left");
    assert_eq!(turn_instruction("sharp right"), "turn sharp right");
    assert_eq!(turn_instruction("keep left"), "keep left");
    assert_eq!(turn_instruction("roundabout"), "take the roundabout");
}

#[test]
fn test_bearing_sign_is_positive_left() {
    assert_eq!(relative_direction(40.0), "40 degrees to your left");
    assert_eq!(relative_direction(-40.0), "40 degrees to your right");
    assert_eq!(relative_direction(2.0), "straight ahead");
    assert_eq!(relative_direction(-178.0), "directly behind you");
}

#[test]
fn test_arrival() {
    assert_eq!(
        arrival_phrase(&v(json!({"offRoad": false, "speed": 0.5})), "imperial").unwrap(),
        "Destination reached. Assistant off"
    );
    assert_eq!(
        arrival_phrase(
            &v(json!({"offRoad": true, "speed": 0, "distance": 140.0 * FOOT, "bearing": 40})),
            "imperial"
        )
        .unwrap(),
        "Destination off road, 150 feet, 40 degrees to your left. Waypoint marked. Assistant off"
    );
    assert!(
        arrival_phrase(
            &v(json!({"offRoad": true, "speed": 0, "distance": 100, "bearing": -90})),
            "metric"
        )
        .unwrap()
        .contains("100 meters, 90 degrees to your right")
    );
}

#[test]
fn test_toasts() {
    let arrived = "You have reached your destination.";
    let u_turn = "Please make a U-turn when possible";
    assert!(suppressed_toast(arrived, false, Some(1.0)));
    assert!(
        !suppressed_toast(arrived, false, Some(10.0)),
        "Long after: it is the game's to say"
    );
    assert!(!suppressed_toast(arrived, false, None));
    assert!(suppressed_toast(u_turn, true, None));
    assert!(
        !suppressed_toast(u_turn, false, None),
        "Driving by hand it is still useful"
    );
    assert!(!suppressed_toast("Fuel low", true, Some(1.0)));
}

// --- Callouts --------------------------------------------------------------------

struct Callouts {
    units: Arc<Mutex<String>>,
    callouts: RouteCallouts,
}

impl Callouts {
    fn new() -> Self {
        let units = Arc::new(Mutex::new("imperial".to_string()));
        let u = units.clone();
        Callouts {
            units,
            callouts: RouteCallouts::new(move || u.lock().unwrap().clone()),
        }
    }

    fn spoken(&mut self, distances: &[f64], make: impl Fn(f64) -> R) -> Vec<String> {
        distances
            .iter()
            .filter_map(|d| self.callouts.update(&report(make(*d))).unwrap())
            .collect()
    }
}

#[test]
fn test_route_set() {
    let mut c = Callouts::new();
    let phrase = c
        .callouts
        .route_set(&report(R {
            dest: 3.2 * MILE,
            ..turn(1.4 * MILE)
        }))
        .unwrap();
    assert_eq!(
        phrase,
        "Route set. 3.2 miles by road. Continue 1.4 miles, then turn left"
    );
    assert!(c.callouts.following);
}

#[test]
fn test_route_set_to_off_road_destination() {
    let mut c = Callouts::new();
    let phrase = c
        .callouts
        .route_set(&report(R {
            dest: 2000.0,
            off_road: true,
            ..R::default()
        }))
        .unwrap();
    assert!(phrase.contains("Destination is off the road"));
    assert!(phrase.contains("to the closest road point"));
}

#[test]
fn test_each_threshold_once_and_only_ahead() {
    let mut c = Callouts::new();
    c.callouts.route_set(&report(turn(1.4 * MILE))).unwrap();
    let said = c.spoken(
        &[
            1.2 * MILE,
            0.99 * MILE,
            0.98 * MILE,
            0.49 * MILE,
            999.0 * FOOT,
            499.0 * FOOT,
            400.0 * FOOT,
        ],
        turn,
    );
    assert_eq!(
        said,
        [
            "In 1 mile, turn left",
            "In half a mile, turn left",
            "In 1000 feet, turn left",
            "In 500 feet, turn left"
        ]
    );
}

#[test]
fn test_metric() {
    let mut c = Callouts::new();
    *c.units.lock().unwrap() = "metric".into();
    c.callouts.route_set(&report(turn(3000.0))).unwrap();
    assert_eq!(
        c.spoken(&[1990.0, 990.0, 490.0, 190.0], turn),
        [
            "In 2 kilometers, turn left",
            "In 1 kilometer, turn left",
            "In 500 meters, turn left",
            "In 200 meters, turn left"
        ]
    );
}

#[test]
fn test_a_long_gap_speaks_only_the_nearest() {
    let mut c = Callouts::new();
    c.callouts.route_set(&report(turn(1.4 * MILE))).unwrap();
    assert_eq!(c.spoken(&[400.0 * FOOT], turn), ["In 500 feet, turn left"]);
}

#[test]
fn test_threshold_near_the_opening_sentence_is_not_repeated() {
    let mut c = Callouts::new();
    c.callouts.route_set(&report(turn(1050.0 * FOOT))).unwrap();
    assert!(
        c.spoken(&[990.0 * FOOT], turn).is_empty(),
        "1000 feet would only repeat 1050"
    );
}

#[test]
fn test_now_scales_with_speed_and_ends_the_count() {
    let mut c = Callouts::new();
    c.callouts.route_set(&report(turn(300.0))).unwrap();
    let fast = |d| R {
        speed: 30.0,
        ..turn(d)
    };
    assert_eq!(c.spoken(&[89.0], fast), ["Turn left now"]);
    assert!(c.spoken(&[80.0, 20.0], fast).is_empty(), "Said once");
    c.callouts
        .route_set(&report(R {
            turn_id: 2,
            ..turn(100.0)
        }))
        .unwrap();
    let slow = |d| R {
        speed: 5.0,
        turn_id: 2,
        ..turn(d)
    };
    assert!(c.spoken(&[60.0], slow).is_empty(), "40 m floor when slow");
    assert_eq!(c.spoken(&[39.0], slow), ["Turn left now"]);
}

#[test]
fn test_passing_a_turn_gives_the_next_stretch() {
    let mut c = Callouts::new();
    c.callouts.route_set(&report(turn(300.0))).unwrap();
    assert_eq!(
        c.callouts
            .update(&report(R {
                turn: "right",
                turn_id: 2,
                ..turn(600.0 * FOOT)
            }))
            .unwrap()
            .as_deref(),
        Some("Continue 600 feet, then turn right")
    );
    assert_eq!(
        c.callouts
            .update(&report(R {
                dest: 2000.0 * FOOT,
                ..R::default()
            }))
            .unwrap()
            .as_deref(),
        Some("Continue 0.4 miles to the destination")
    );
    assert_eq!(
        c.callouts
            .update(&report(R {
                dest: 490.0 * FOOT,
                ..R::default()
            }))
            .unwrap()
            .as_deref(),
        Some("Destination in 500 feet")
    );
}

#[test]
fn test_next_turn_right_after_the_last() {
    let mut c = Callouts::new();
    c.callouts.route_set(&report(turn(300.0))).unwrap();
    assert_eq!(
        c.callouts
            .update(&report(R {
                turn: "right",
                turn_id: 2,
                ..turn(20.0)
            }))
            .unwrap()
            .as_deref(),
        Some("Turn right now")
    );
    assert_eq!(
        c.callouts
            .update(&report(R {
                turn: "right",
                turn_id: 2,
                ..turn(10.0)
            }))
            .unwrap(),
        None
    );
}

#[test]
fn test_missed_route_set_counts_silently() {
    let mut c = Callouts::new();
    assert_eq!(c.callouts.update(&report(turn(1000.0))).unwrap(), None);
    assert!(c.callouts.following);
}

// --- Session ---------------------------------------------------------------------

type Spoken = Arc<Mutex<Vec<(String, SpeakOpts)>>>;

struct Session {
    spoken: Spoken,
    arrivals: Arc<Mutex<Vec<Value>>>,
    client: DrivingAssistant,
    sequence: i64,
}

impl Session {
    fn new() -> Self {
        let spoken: Spoken = Arc::new(Mutex::new(Vec::new()));
        let arrivals = Arc::new(Mutex::new(Vec::new()));
        let (sp, ar) = (spoken.clone(), arrivals.clone());
        let client = DrivingAssistant::new(
            |_: &str| {},
            move |text: &str, o: SpeakOpts| sp.lock().unwrap().push((text.to_string(), o)),
            Some(Arc::new(|| "metric".to_string())),
            Some(Box::new(move |a: &Value| {
                ar.lock().unwrap().push(a.clone())
            })),
        );
        client.toggle();
        Session {
            spoken,
            arrivals,
            client,
            sequence: 0,
        }
    }

    fn send(&mut self, kind: &str, message: &str, data: Option<Value>) {
        self.sequence += 1;
        let data_text = data
            .map(|d| {
                beam_road::pyjson::dumps(&d, &beam_road::pyjson::DumpOpts::default_seps()).unwrap()
            })
            .unwrap_or_else(|| "null".into());
        let payload = format!(
            "{{\"token\": {}, \"kind\": {}, \"sequence\": {}, \"vehicle\": 42, \"message\": {}, \"data\": {}}}",
            json!(self.client.token()),
            json!(kind),
            self.sequence,
            json!(message),
            data_text
        );
        self.client.receive(&payload).unwrap();
    }

    fn last(&self) -> (String, SpeakOpts) {
        self.spoken.lock().unwrap().last().cloned().unwrap()
    }
}

#[test]
fn test_route_flow() {
    let mut s = Session::new();
    s.send("on", "Driving assistant on", None);
    s.send(
        "route_set",
        "",
        Some(report(R {
            dest: 3000.0,
            ..turn(1400.0)
        })),
    );
    assert_eq!(
        s.last().0,
        "Route set. 3 kilometers by road. Continue 1.4 kilometers, then turn left"
    );
    assert_eq!(
        s.last().1.interrupt,
        Some(false),
        "It must not cut off 'Driving assistant on'"
    );
    assert!(s.client.snapshot().following_route);
    let count = s.spoken.lock().unwrap().len();
    s.send(
        "route",
        "",
        Some(report(R {
            dest: 2900.0,
            ..turn(1300.0)
        })),
    );
    assert_eq!(
        s.spoken.lock().unwrap().len(),
        count,
        "No threshold crossed: silence"
    );
    s.send(
        "route",
        "",
        Some(report(R {
            dest: 2790.0,
            ..turn(990.0)
        })),
    );
    assert_eq!(s.last().0, "In 1 kilometer, turn left");
}

#[test]
fn test_arrival_off_road_hands_over_the_position() {
    let mut s = Session::new();
    s.send("on", "", None);
    s.send(
        "route_set",
        "",
        Some(report(R {
            dest: 3000.0,
            off_road: true,
            ..R::default()
        })),
    );
    let arrival = v(
        json!({"offRoad": true, "speed": 0, "distance": 100, "bearing": -90, "x": 5.0, "y": 6.0}),
    );
    s.send("off", "Destination reached. Assistant off", Some(v(json!({"arrival": json!({"offRoad": true, "speed": 0, "distance": 100, "bearing": -90, "x": 5.0, "y": 6.0})}))));
    assert!(s.last().0.contains("100 meters, 90 degrees to your right"));
    assert_eq!(*s.arrivals.lock().unwrap(), vec![arrival]);
    assert!(!s.client.active());
    assert!(!s.client.snapshot().following_route);
    assert!(s.client.seconds_since_arrival().is_some());
}

#[test]
fn test_ordinary_off_is_no_arrival() {
    let mut s = Session::new();
    s.send("on", "", None);
    s.send("off", "Driving assistant off", None);
    assert_eq!(s.last().0, "Driving assistant off");
    assert!(s.arrivals.lock().unwrap().is_empty());
    assert!(s.client.seconds_since_arrival().is_none());
}

#[test]
fn test_route_cleared() {
    let mut s = Session::new();
    s.send("on", "", None);
    s.send("route_set", "", Some(report(turn(1000.0))));
    s.send("route_clear", "Route cleared. Turns are yours", None);
    assert_eq!(s.last().0, "Route cleared. Turns are yours");
    assert!(!s.client.snapshot().following_route);
}

#[test]
fn test_turn_instructions_protect_themselves() {
    let mut s = Session::new();
    let cues = Arc::new(Mutex::new(Vec::<String>::new()));
    let c = cues.clone();
    s.client.set_cue(Arc::new(move |kind: &str, _: &[String]| {
        c.lock().unwrap().push(kind.into())
    }));
    s.send(
        "say",
        "Three point turn. Forward",
        Some(v(json!({"protect": true}))),
    );
    assert_eq!(s.last().0, "Three point turn. Forward");
    assert_eq!(
        *cues.lock().unwrap(),
        ["protect"],
        "The compass must not talk over a gear change"
    );
    s.send("say", "Back on the road", None);
    assert_eq!(
        *cues.lock().unwrap(),
        ["protect"],
        "Ordinary speech opens no window"
    );
}

#[test]
fn test_malformed_reports_are_dropped() {
    let mut s = Session::new();
    s.send("on", "", None);
    for data in [
        None,
        Some(v(json!("x"))),
        Some(v(json!({"turnId": 1, "turnDistance": "far"}))),
    ] {
        s.send("route", "", data);
    }
    assert!(s.client.active());
}

// --- SignalsWithoutRoadGuidance --------------------------------------------------

#[test]
fn test_feed_reports_with_assistant_gate() {
    // beamtel passes `road_mode_active or driving_assistant.active`: road guidance off
    // with the assistant on is the True case, and the same event must come out.
    let feed = RoadGuidanceFeed::with_clock(|| 0.0);
    let packet = R2Packet {
        state: "onRoad".into(),
        signal_context: "c".into(),
        signal_status: "available".into(),
        signal: Some(Signal {
            id: "s".into(),
            kind: "stopSign".into(),
            state: "stop".into(),
            phase: "approach".into(),
            distance: 100.0,
            group_id: "s".into(),
            preview: false,
        }),
        ..R2Packet::default()
    };
    let ev = feed.accept_r2(&packet, Some(0.0), true, true, true);
    assert_eq!(ev.signal.unwrap().signal.kind, "stopSign");
    let source = std::fs::read_to_string(beam_testutil::repo_root().join("beamtel.py")).unwrap();
    assert!(source.contains(
        "awareness_active = controls_ready and (road_mode_active or driving_assistant.active)"
    ));
}
