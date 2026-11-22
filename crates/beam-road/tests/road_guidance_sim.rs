//! diagnostic/road_guidance_sim.py: deterministic R2 protocol, wording, fallback and
//! event checks.
//!
//! Not ported here: the first block, which compares the road_* keys of the two
//! DEFAULT_CONFIG dicts in beamtel.py and configurator.py; that belongs to the config
//! port (beam_base::config).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use beam_road::road_guidance::*;
use serde_json::{Value as J, json};

fn packet(overrides: J) -> R2Packet {
    let mut value = json!({
        "state": "onRoad",
        "oneWay": false,
        "roadDirections": [4, -176],
        "offRoad": null,
        "correction": {"active": false, "bearing": 0, "severity": 0},
        "junction": null,
    });
    for (k, v) in overrides.as_object().unwrap() {
        value[k] = v.clone();
    }
    parse_r2_packet(&format!("R2|{value}")).unwrap()
}

fn expect_bad(payload: &str) {
    assert!(
        parse_r2_packet(payload).is_err(),
        "accepted malformed packet: {payload}"
    );
}

struct Clock(Arc<AtomicU64>);
impl Clock {
    fn set(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::SeqCst);
    }
}

fn feed() -> (RoadGuidanceFeed, Clock) {
    let cell = Arc::new(AtomicU64::new(0f64.to_bits()));
    let c = cell.clone();
    (
        RoadGuidanceFeed::with_clock(move || f64::from_bits(c.load(Ordering::SeqCst))),
        Clock(cell),
    )
}

#[test]
fn one_way_phrases() {
    assert_eq!(one_way_phrase(0.0), "One-way road. Legal travel is ahead");
    assert_eq!(
        one_way_phrase(92.0),
        "One-way road. Legal travel is 92 degrees to your left"
    );
    assert_eq!(
        one_way_phrase(-175.0),
        "One-way road. Legal travel is 175 degrees to your right"
    );
}

#[test]
fn main_scenario() {
    assert_eq!(
        parse_surface_packet("SURFACE|Dirt|1"),
        Some(("Dirt".into(), true))
    );
    assert_eq!(
        parse_surface_packet("SURFACE|Grass|0|future"),
        Some(("Grass".into(), false)),
        "tails are optional"
    );
    for bad in [
        "SURFACE|Dirt",
        "SURFACE||1",
        "SURFACE|Dirt road|1",
        "SURFACE|Dirt|2",
        "R2|x",
    ] {
        assert_eq!(parse_surface_packet(bad), None, "{bad}");
    }
    assert_eq!(surface_phrase("Dirt", true), "Dirt road");
    assert_eq!(surface_phrase("Grass", false), "Grass");

    let valid = packet(json!({
        "correction": {"active": true, "bearing": 22, "severity": 1.8, "phase": "correct",
                       "lateralRatio": 0.82, "headingError": 7, "timeToEdge": 1.4},
        "junction": {"id": "n1+n2", "phase": "approach", "kind": "crossroads",
                     "distance": 60.96, "exits": [90, 0, -90]},
    }));
    let c = valid.correction.as_ref().unwrap();
    assert_eq!(c.severity, 1.0);
    assert_eq!(c.phase, "correct");
    assert_eq!(c.time_to_edge, Some(1.4));
    let with_diagnostic = packet(json!({"diagnostic": {
        "edgeId": "road_1|road_2", "signedLateral": 3.1, "lateralDistance": 3.1,
        "lateralSpeed": -0.4, "targetError": 1.1, "withinLateral": false, "withinHeading": true,
        "withinLateralSpeed": false, "settledCandidate": false, "activeBefore": true,
        "correctionArmed": false, "rearmTicks": 0, "steeringInput": -0.25, "steering": -0.2,
        "contactMaterials": "10:ASPHALT,30:RUBBER",
    }}));
    let d = with_diagnostic.diagnostic.as_ref().unwrap();
    assert!(d["edgeId"].eq_str("road_1|road_2"));
    assert_eq!(d["lateralSpeed"], (-0.4).into());
    assert_eq!(d["steeringInput"], (-0.25).into());
    assert!(d["contactMaterials"].eq_str("10:ASPHALT,30:RUBBER"));
    expect_bad("R2|not json");
    expect_bad(r#"R2|{"state":"flying"}"#);
    expect_bad(r#"R2|{"state":"onRoad","roadDirections":[NaN]}"#);
    expect_bad(r#"R2|{"state":"onRoad","roadDirections":{"bad":1}}"#);
    expect_bad(r#"R2|{"state":"onRoad","correction":{"active":true,"phase":"hold"}}"#);
    expect_bad(r#"R2|{"state":"onRoad","junction":{"id":"x","phase":"later"}}"#);
    expect_bad(r#"R2|{"state":"onRoad","diagnostic":{"withinLateral":1}}"#);
    expect_bad(r#"R2|{"state":"onRoad","diagnostic":{"contactMaterials":[]}}"#);
    expect_bad(r#"R2|{"state":"onRoad","junction":{"id":"x","phase":"near","entered":1}}"#);
    // Lua's jsonEncode writes an empty table as {}, even where the R2 schema defines an
    // array.
    let empty_arrays = parse_r2_packet(
        r#"R2|{"state":"offRoad","roadDirections":{},"offRoad":null,"correction":null,"junction":null}"#,
    )
    .unwrap();
    assert!(empty_arrays.road_directions.is_empty());
    let empty_exits = packet(
        json!({"junction": {"id": "dead-end", "phase": "near", "distance": 5, "exits": {}}}),
    );
    assert!(empty_exits.junction.unwrap().exits.is_empty());

    assert_eq!(
        direction_list(&[90.0, 0.0, -90.0], "and").unwrap(),
        "left, straight, and right"
    );
    assert_eq!(
        direction_list(&[30.0, -30.0], "or").unwrap(),
        "slight left or slight right"
    );
    let vj = valid.junction.clone().unwrap();
    assert_eq!(
        junction_phrase(&vj, "imperial").unwrap(),
        "Crossroads in 200 feet: left, straight, and right."
    );
    let metric_t = Junction {
        id: "t".into(),
        phase: "approach".into(),
        entered: false,
        kind: "tJunction".into(),
        distance: 40.0,
        exits: vec![90.0, -90.0],
    };
    assert_eq!(
        junction_phrase(&metric_t, "metric").unwrap(),
        "T-junction in 40 meters: left or right."
    );
    let j = |kind: &str, d: f64| Junction {
        id: "d".into(),
        phase: "approach".into(),
        entered: false,
        kind: kind.into(),
        distance: d,
        exits: vec![],
    };
    assert_eq!(
        junction_phrase(&j("deadEnd", 9.1), "metric").unwrap(),
        "Road ends in 9 meters."
    );
    // The map's "gated" flag is a private road, almost never with a gate on it.
    assert_eq!(
        junction_phrase(&j("private", 30.0), "metric").unwrap(),
        "Private road in 30 meters."
    );
    assert_eq!(
        junction_phrase(&j("gate", 30.0), "metric").unwrap(),
        "Gate in 30 meters."
    );

    let (feed, clock) = feed();
    let first = feed.accept_r2_default(&packet(json!({})));
    assert!(first.orientation);
    assert!(!feed.accept_r2_default(&packet(json!({}))).orientation);
    let rearmed = packet(json!({"orientation": true, "oneWay": true, "roadDirections": [-12]}));
    assert!(feed.accept_r2_default(&rearmed).orientation);

    let junction_json = json!({"id": "n1+n2", "phase": "approach", "kind": "crossroads", "distance": 60.96, "exits": [90, 0, -90]});
    let event_packet = packet(json!({"junction": junction_json}));
    assert!(feed.accept_r2_default(&event_packet).junction.is_some());
    assert!(feed.accept_r2_default(&event_packet).junction.is_none());
    let mut near = junction_json.clone();
    near["phase"] = json!("near");
    near["distance"] = json!(20);
    assert_eq!(
        feed.accept_r2_default(&packet(json!({"junction": near})))
            .junction
            .unwrap()
            .phase,
        "near"
    );
    assert!(
        feed.accept_r2_default(&packet(json!({"junction": near})))
            .junction
            .is_none()
    );
    let mut entered = near.clone();
    entered["entered"] = json!(true);
    entered["distance"] = json!(0);
    assert_eq!(
        feed.accept_r2_default(&packet(json!({"junction": entered})))
            .junction
            .unwrap()
            .phase,
        "entered"
    );
    assert!(
        feed.accept_r2_default(&packet(json!({"junction": entered})))
            .junction
            .is_none()
    );
    for _ in 0..3 {
        feed.accept_r2_default(&packet(json!({"junction": null})));
    }
    assert!(feed.accept_r2_default(&event_packet).junction.is_some());

    clock.set(1.01);
    assert!(feed.check_timeout(None));
    assert_eq!(feed.snapshot().mode, "stale");
    assert!(
        feed.status_phrase(true, "metric")
            .unwrap()
            .to_lowercase()
            .contains("stale")
    );

    // A legacy datagram arriving after the grace period immediately enables fallback.
    assert!(feed.accept_legacy("OFF_ROAD", 45.0, 10.0, &[], None));
    assert_eq!(feed.snapshot().mode, "legacy");
    let legacy_status = feed.status_phrase(true, "imperial").unwrap();
    assert!(legacy_status.contains("Legacy guidance only"));
    assert!(legacy_status.contains("33 feet"));

    let (fresh, _c) = feed_at(1.01);
    assert_eq!(
        fresh.status_phrase(false, "metric").unwrap(),
        "Road guidance is off."
    );
    assert!(
        fresh
            .status_phrase(true, "metric")
            .unwrap()
            .contains("unavailable")
    );
    fresh.accept_legacy("ON_ROAD", 0.0, 0.0, &[], None);
    assert!(
        fresh
            .status_phrase(true, "metric")
            .unwrap()
            .contains("Legacy road detector only")
    );

    let (no_match, _c) = feed_at(1.01);
    no_match.accept_r2_default(&packet(
        json!({"state": "offRoad", "offRoad": null, "correction": null}),
    ));
    assert!(
        no_match
            .status_phrase(true, "metric")
            .unwrap()
            .contains("No vertically compatible road")
    );

    let (status_feed, _c) = feed_at(1.01);
    status_feed.accept_r2_default(&packet(json!({
        "oneWay": true, "roadDirections": [10],
        "correction": {"active": true, "bearing": -50, "severity": 0.4, "phase": "correct"},
        "junction": {"id": "t", "phase": "approach", "kind": "tJunction", "distance": 40, "exits": [90, -90]},
    })));
    let status = status_feed.status_phrase(true, "metric").unwrap();
    for phrase in ["One-way road", "moderate correction right", "T-junction"] {
        assert!(status.contains(phrase), "{phrase} in {status}");
    }
    status_feed.accept_r2_default(&packet(json!({
        "correction": {"active": true, "bearing": 0, "severity": 0.2, "phase": "unwind"},
    })));
    assert!(
        status_feed
            .status_phrase(true, "metric")
            .unwrap()
            .contains("Straighten steering now")
    );
}

fn feed_at(t: f64) -> (RoadGuidanceFeed, Clock) {
    let (f, c) = feed();
    c.set(t);
    (f, c)
}
