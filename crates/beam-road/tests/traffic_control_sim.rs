//! diagnostic/traffic_control_sim.py: signal event delivery, compatibility and
//! stale-feed behaviour.

use beam_road::road_guidance::*;
use serde_json::{Value as J, json};

fn signal_json(state: &str, distance: f64) -> J {
    json!({"id": "1", "kind": if state == "stop" {"stopSign"} else {"trafficLight"},
           "state": state, "distance": distance,
           "phase": if distance <= 12.0 {"near"} else {"approach"}})
}

fn packet_json(state: &str, distance: f64, overrides: J) -> J {
    let mut v = json!({"state": "onRoad", "signalStatus": "available", "signalContext": "italy:1",
                       "signal": signal_json(state, distance)});
    for (k, x) in overrides.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

fn try_packet(
    state: &str,
    distance: f64,
    overrides: J,
) -> Result<R2Packet, beam_road::pyjson::PyErr> {
    parse_r2_packet(&format!("R2|{}", packet_json(state, distance, overrides)))
}

fn packet(state: &str, distance: f64, overrides: J) -> R2Packet {
    try_packet(state, distance, overrides).unwrap()
}

fn at(feed: &RoadGuidanceFeed, p: &R2Packet, now: f64) -> Option<SignalEvent> {
    feed.accept_r2(p, Some(now), true, true, true).signal
}

#[test]
fn main_scenario() {
    let feed = RoadGuidanceFeed::new();
    let e = json!({});
    assert!(at(&feed, &packet("stop", 35.0, e.clone()), 0.0).is_some());
    for i in 1..=20 {
        assert!(
            at(
                &feed,
                &packet("stop", 35.0 - i as f64, e.clone()),
                i as f64 / 20.0
            )
            .is_none()
        );
    }
    assert!(at(&feed, &packet("stop", 10.0, e.clone()), 1.1).is_some());
    for i in 1..=30 {
        assert!(
            at(
                &feed,
                &packet("stop", 10.0, e.clone()),
                1.1 + i as f64 / 10.0
            )
            .is_none()
        );
    }
    assert!(
        feed.status_phrase(true, "imperial")
            .unwrap()
            .contains("Stop sign in 33 feet")
    );
    let gap = packet(
        "stop",
        35.0,
        json!({"signal": null, "signalStatus": "none"}),
    );
    at(&feed, &gap, 4.2);
    assert!(at(&feed, &packet("stop", 10.0, e.clone()), 4.3).is_none());
    // A later circuit of the same approach re-arms the sign.
    for i in 1..=60 {
        at(&feed, &gap, 4.3 + i as f64 / 10.0);
    }
    assert!(at(&feed, &packet("stop", 35.0, e.clone()), 10.4).is_some());
    assert!(
        at(
            &feed,
            &packet("stop", 35.0, json!({"signalContext": "westcoast:2"})),
            10.5
        )
        .is_some()
    );
    assert!(feed.check_timeout(Some(12.0)));
    assert!(at(&feed, &packet("stop", 35.0, e.clone()), 12.1).is_some());
    feed.reset();
    assert!(
        feed.accept_r2(
            &packet("stop", 35.0, e.clone()),
            Some(0.0),
            false,
            true,
            true
        )
        .signal
        .is_none()
    );
    assert!(
        feed.accept_r2(
            &packet("stop", 35.0, e.clone()),
            Some(0.1),
            true,
            true,
            true
        )
        .signal
        .is_some()
    );
    feed.reset();
    assert!(at(&feed, &packet("red", 8.0, e.clone()), 0.0).is_some());
    let event = at(&feed, &packet("green", 8.0, e.clone()), 0.1).unwrap();
    assert!(event.change && signal_phrase(&event.signal, "metric", true) == "Traffic light green.");
    assert!(at(&feed, &packet("green", 8.0, e.clone()), 0.2).is_none());
    assert!(
        at(&feed, &packet("yellow", 8.0, e.clone()), 0.3)
            .unwrap()
            .change
    );
    for state in [
        "flashingRed",
        "flashingYellow",
        "flashingGreen",
        "redYellow",
        "off",
        "unknown",
    ] {
        let p = packet(state, 35.0, e.clone());
        assert!(!signal_phrase(p.signal.as_ref().unwrap(), "metric", false).is_empty());
    }
    let old = parse_r2_packet(r#"R2|{"state":"onRoad"}"#).unwrap();
    assert!(old.signal_status == "unsupported" && old.signal.is_none());
    feed.reset();
    let mut preview = signal_json("stop", 35.0);
    preview["id"] = json!("preview:stop:1");
    preview["groupId"] = json!("stop:1");
    preview["preview"] = json!(true);
    let event = at(
        &feed,
        &packet("stop", 35.0, json!({"signal": preview})),
        0.0,
    )
    .unwrap();
    assert!(signal_phrase(&event.signal, "metric", false).contains("ahead, about"));
    let mut exact = preview.clone();
    exact["id"] = json!("42");
    exact["preview"] = json!(false);
    assert!(at(&feed, &packet("stop", 35.0, json!({"signal": exact})), 0.1).is_none());
    let mut light_preview = signal_json("unknown", 35.0);
    light_preview["groupId"] = json!("lights:2");
    light_preview["preview"] = json!(true);
    assert!(
        at(
            &feed,
            &packet("unknown", 35.0, json!({"signal": light_preview})),
            0.2
        )
        .is_some()
    );
    let mut red = light_preview.clone();
    red["preview"] = json!(false);
    red["state"] = json!("red");
    let event = at(&feed, &packet("red", 35.0, json!({"signal": red})), 0.3).unwrap();
    assert!(event.change && signal_phrase(&event.signal, "metric", true) == "Traffic light red.");
    // float("nan") is written by Python's json.dumps as NaN.
    for fields in [
        json!({"distance": -1}),
        json!({"distance": "NaN-literal"}),
        json!({"distance": true}),
        json!({"kind": "stopSign", "state": "red"}),
        json!({"id": []}),
        json!({"state": {}}),
        json!({"phase": []}),
    ] {
        let mut bad = signal_json("stop", 35.0);
        for (k, v) in fields.as_object().unwrap() {
            bad[k] = v.clone();
        }
        let text = format!("R2|{}", packet_json("stop", 35.0, json!({"signal": bad})))
            .replace("\"NaN-literal\"", "NaN");
        assert!(parse_r2_packet(&text).is_err(), "{fields}");
    }
}
