//! diagnostic/speed_limit_sim.py: map limit validation, stable announcements, units and
//! stale-data checks.

use std::sync::{Arc, Mutex};

use beam_road::road_guidance::*;
use serde_json::{Value as J, json};

fn packet_text(value: J, extra: J) -> String {
    let mut v = json!({"state": "onRoad", "speedLimit": value, "signalContext": "map:1"});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    format!("R2|{v}")
}

fn packet(value: J) -> R2Packet {
    parse_r2_packet(&packet_text(value, json!({}))).unwrap()
}

#[test]
fn main_scenario() {
    assert_eq!(
        parse_r2_packet(r#"R2|{"state":"onRoad"}"#)
            .unwrap()
            .speed_limit,
        None
    );
    for value in [
        json!(true),
        json!(false),
        json!(0),
        json!(-1),
        json!("nan"),
        json!([]),
        json!({}),
    ] {
        assert!(
            parse_r2_packet(&packet_text(value.clone(), json!({}))).is_err(),
            "accepted invalid limit: {value}"
        );
    }
    // float("inf") is written by Python's json.dumps as Infinity.
    assert!(
        parse_r2_packet(r#"R2|{"state":"onRoad","speedLimit":Infinity,"signalContext":"map:1"}"#)
            .is_err()
    );
    assert!(
        parse_r2_packet(r#"R2|{"state":"offRoad","speedLimit":10}"#).is_err(),
        "accepted off-road limit"
    );
    assert_eq!(
        map_speed_limit_phrase(Some(8.333333333333), "metric", "Map speed limit"),
        "Map speed limit 30 kilometers per hour."
    );
    assert_eq!(
        map_speed_limit_phrase(Some(8.333333333333), "imperial", "Map speed limit"),
        "Map speed limit 19 mph."
    );
    assert!(map_speed_limit_phrase(None, "imperial", "Map speed limit").contains("unavailable"));
    // F9, S says "Speed limit"; the automatic announcement keeps "Map speed limit".
    assert_eq!(
        map_speed_limit_phrase(Some(8.333333333333), "imperial", "Speed limit"),
        "Speed limit 19 mph."
    );
    assert_eq!(
        map_speed_limit_phrase(None, "metric", "Speed limit"),
        "Speed limit unavailable."
    );

    let now = Arc::new(Mutex::new(0.0f64));
    let n2 = now.clone();
    let feed = RoadGuidanceFeed::with_clock(move || *n2.lock().unwrap());

    let sample = |value: J, enabled: bool| -> Option<SpeedLimitEvent> {
        *now.lock().unwrap() += 0.2;
        feed.accept_r2(&packet(value), None, true, true, enabled)
            .speed_limit
    };
    let hold = |value: J, count: usize, enabled: bool| -> Vec<Option<f64>> {
        (0..count)
            .filter_map(|_| sample(value.clone(), enabled))
            .map(|e| e.value)
            .collect()
    };
    let ten = json!(10);
    let twenty = json!(20);
    let none = J::Null;

    assert_eq!(hold(ten.clone(), 5, true), vec![Some(10.0)]);
    assert_eq!(
        hold(ten.clone(), 30, true),
        vec![],
        "same limit on successive edges is silent"
    );
    assert!(sample(twenty.clone(), true).is_none());
    assert_eq!(
        hold(ten.clone(), 5, true),
        vec![],
        "brief edge flicker does not announce"
    );
    assert!(sample(none.clone(), true).is_none());
    assert_eq!(
        hold(ten.clone(), 5, true),
        vec![],
        "brief data gap does not repeat the old limit"
    );
    assert_eq!(hold(twenty.clone(), 5, true), vec![Some(20.0)]);
    assert!(
        feed.speed_limit_phrase(true, "imperial", "Map speed limit")
            .contains("45 mph")
    );
    assert!(
        feed.speed_limit_phrase(false, "imperial", "Map speed limit")
            .contains("unavailable")
    );
    assert_eq!(hold(twenty.clone(), 5, false), vec![]);
    assert_eq!(
        hold(twenty.clone(), 5, true),
        vec![Some(20.0)],
        "reenabling announces the current limit"
    );
    assert_eq!(hold(none.clone(), 18, true), vec![None]);
    assert_eq!(
        hold(none.clone(), 20, true),
        vec![],
        "unknown is announced once, only after a known limit"
    );
    assert_eq!(hold(ten.clone(), 5, true), vec![Some(10.0)]);
    *now.lock().unwrap() += 2.0;
    assert!(
        feed.speed_limit_phrase(true, "imperial", "Map speed limit")
            .contains("unavailable")
    );
    assert_eq!(
        hold(ten.clone(), 5, true),
        vec![Some(10.0)],
        "reconnect must announce again"
    );
    feed.reset();
    assert_eq!(
        hold(none.clone(), 20, true),
        vec![],
        "unsupported maps do not chatter"
    );
    assert_eq!(hold(ten.clone(), 5, true), vec![Some(10.0)]);
    for _ in 0..20 {
        *now.lock().unwrap() += 0.2;
        let p = parse_r2_packet(r#"R2|{"state":"offRoad","signalContext":"map:1"}"#).unwrap();
        assert!(feed.accept_r2_default(&p).speed_limit.is_none());
    }
    assert_eq!(
        hold(ten, 5, true),
        vec![Some(10.0)],
        "rejoining after an extended departure reannounces"
    );
}
