//! Port of `diagnostic/drift_speech_sim.py`: phrases, interrupt choice, the quiet window
//! and its timeout. The Python sim is one `main()`; its checks keep their order here.

use std::sync::{Arc, Mutex};

use beam_reports::drift_speech::{DriftAnnouncer, QUIET_TIMEOUT_S, drift_phrase};
use beam_reports::pyops::{PyValue, json_loads};

fn ev(json: &str) -> PyValue {
    json_loads(json).expect("event JSON")
}

fn phrase(json: &str) -> Option<(String, bool)> {
    drift_phrase(&ev(json)).expect("no exception")
}

fn some(p: &str, i: bool) -> Option<(String, bool)> {
    Some((p.to_string(), i))
}

#[test]
fn main() {
    // Phrases mirror the display, with "+80" read as "plus 80".
    assert_eq!(
        phrase(r#"{"event": "shown", "points": 450, "combo": 1.62}"#),
        some("Drift. 450 points, combo 1.6", false)
    );
    assert_eq!(
        phrase(r#"{"event": "shown", "points": 0, "combo": 1}"#),
        some("Drift", false)
    );
    assert_eq!(phrase(r#"{"event": "hidden"}"#), some("Drift over", false));
    assert_eq!(
        phrase(r#"{"event": "slide", "direction": "left"}"#),
        some("left", false)
    );
    assert_eq!(phrase(r#"{"event": "slide", "direction": "up"}"#), None);
    assert_eq!(
        phrase(r#"{"event": "combo", "combo": 2}"#),
        some("Combo 2", false)
    );
    assert_eq!(
        phrase(r#"{"event": "tier", "text": "Great Drift!"}"#),
        some("Great Drift!", false)
    );
    assert_eq!(
        phrase(r#"{"event": "quick", "text": "Close call! +80"}"#),
        some("Close call! plus 80", false)
    );
    assert_eq!(
        phrase(r#"{"event": "fail", "text": "Spinout!", "lost": 640}"#),
        some("Spinout! 640 points lost", true)
    );
    assert_eq!(
        phrase(r#"{"event": "fail", "text": "Crashed!", "lost": 0}"#),
        some("Crashed!", true)
    );
    assert_eq!(
        phrase(r#"{"event": "fail", "text": "Spinout!", "lost": 640, "total": 18916}"#),
        some("Spinout! 640 points lost. Total 18,916", true)
    );
    assert_eq!(
        phrase(r#"{"event": "fail", "text": "Spinout!", "total": 2000}"#),
        some("Spinout! Total 2,000", true)
    );
    assert_eq!(
        phrase(r#"{"event": "fail", "text": "Spinout!", "lost": 0, "total": 0}"#),
        some("Spinout!", true)
    );
    assert_eq!(
        phrase(r#"{"event": "scored", "points": 1240, "total": 5300}"#),
        some("Plus 1,240 points. Total 5,300", false)
    );
    assert_eq!(phrase(r#"{"event": "scored", "points": null}"#), None);
    assert_eq!(
        phrase(r#"{"event": "wrapup", "points": 300}"#),
        some("Plus 300 points for this drift", false)
    );
    assert_eq!(
        phrase(r#"{"event": "spot_done", "text": "New record! Score: 12000"}"#),
        some("New record! Score: 12000", false)
    );
    assert_eq!(
        phrase(r#"{"event": "spot_failed", "text": "Drift zone exited"}"#),
        some("Drift zone exited", true)
    );
    assert_eq!(
        phrase(r#"{"event": "out_of_bounds", "text": "Out of bounds!"}"#),
        some("Out of bounds!", true)
    );
    let start = phrase(
        r#"{"event": "spot_start", "name": "Docks", "highScore": 3200,
            "goals": [{"points": 1000, "done": true}, {"points": 5000, "done": false}]}"#,
    );
    assert_eq!(
        start,
        some(
            "Drift zone Docks. High score 3,200. Goal 5,000 points",
            false
        ),
        "{start:?}"
    );
    assert_eq!(
        phrase(r#"{"event": "spot_start"}"#),
        some("Drift zone started", false)
    );
    assert_eq!(
        phrase(r#"{"event": "spot_start", "highScore": 0}"#),
        some("Drift zone started. No high score yet", false)
    );
    for bad in [
        "null",
        "\"x\"",
        "{}",
        r#"{"event": "alive"}"#,
        r#"{"event": "nope"}"#,
        r#"{"event": "quick", "text": 5}"#,
    ] {
        assert_eq!(phrase(bad), None, "{bad}");
    }

    let now = Arc::new(Mutex::new(100.0f64));
    let spoken: Arc<Mutex<Vec<(String, bool, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let (n2, s2) = (now.clone(), spoken.clone());
    let announcer = DriftAnnouncer::new(
        Box::new(move |t, i, src| s2.lock().unwrap().push((t.to_string(), i, src.to_string()))),
        true,
        Box::new(move || *n2.lock().unwrap()),
    );
    let advance = |d: f64| *now.lock().unwrap() += d;
    let last = || spoken.lock().unwrap().last().cloned();

    assert!(!announcer.quiet());
    announcer
        .handle(&ev(r#"{"event": "shown", "points": 0, "combo": 1}"#))
        .unwrap();
    assert!(
        announcer.quiet(),
        "the display mutes speed and compass speech"
    );
    assert_eq!(
        last(),
        Some(("Drift".to_string(), false, "drift".to_string()))
    );
    advance(QUIET_TIMEOUT_S - 1.0);
    announcer.handle(&ev(r#"{"event": "alive"}"#)).unwrap();
    advance(QUIET_TIMEOUT_S - 1.0);
    assert!(
        announcer.quiet(),
        "keepalives hold the window through a long slide"
    );
    assert_eq!(spoken.lock().unwrap().len(), 1, "keepalives are silent");
    announcer.handle(&ev(r#"{"event": "hidden"}"#)).unwrap();
    assert!(!announcer.quiet() && last().unwrap().0 == "Drift over");

    announcer.handle(&ev(r#"{"event": "shown"}"#)).unwrap();
    advance(QUIET_TIMEOUT_S + 0.1);
    assert!(
        !announcer.quiet(),
        "a lost 'hidden' cannot mute speed and compass for good"
    );

    // A receiver restarted mid-drift missed "shown"; the keepalive restores the window.
    let n3 = now.clone();
    let late = DriftAnnouncer::new(
        Box::new(|_, _, _| {}),
        true,
        Box::new(move || *n3.lock().unwrap()),
    );
    late.handle(&ev(r#"{"event": "alive"}"#)).unwrap();
    assert!(late.quiet());

    // Off means silent and nothing muted.
    spoken.lock().unwrap().clear();
    announcer.set_enabled(false);
    announcer
        .handle(&ev(r#"{"event": "shown", "points": 10, "combo": 1}"#))
        .unwrap();
    announcer
        .handle(&ev(r#"{"event": "tier", "text": "Drift!"}"#))
        .unwrap();
    assert!(spoken.lock().unwrap().is_empty() && !announcer.quiet());
    announcer.set_enabled(true);
    assert!(
        announcer.quiet(),
        "turning it back on mid-display restores the window"
    );
    announcer.handle(&PyValue::str("garbage")).unwrap();
    announcer.handle(&ev(r#"{"event": 3}"#)).unwrap();
}
