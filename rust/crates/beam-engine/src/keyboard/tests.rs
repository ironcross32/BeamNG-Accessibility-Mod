//! The hook's classify/enqueue logic, without installing a real hook: every test drives a
//! private `Listener` (or the pure classifiers) with synthetic events.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;

use super::hook::{Listener, ModState, RawEvent, next_id, process_key};
use super::*;

fn raw(name: &str, code: i32, down: bool) -> RawEvent {
    RawEvent {
        event: KeyEvent {
            name: name.to_string(),
            event_type: if down {
                KeyEventType::Down
            } else {
                KeyEventType::Up
            },
        },
        scan_code: code,
        has_name: true,
    }
}

#[test]
fn layer_classification_matches_kb_layer_press() {
    use LayerAction::{Dispatch, Pass, Repeat};
    let down = |n: &str| KeyEvent::down(n);
    // Key-ups always pass, layer or not.
    assert_eq!(
        classify_layer_key(Some(Layer::F9), &KeyEvent::up("a")),
        Pass
    );
    // No layer open: everything passes.
    assert_eq!(classify_layer_key(None, &down("a")), Pass);
    // Modifiers are noted and eaten, under every alias.
    for (n, m) in [
        ("ctrl", Modifier::Ctrl),
        ("Control", Modifier::Ctrl),
        ("left ctrl", Modifier::Ctrl),
        ("right ctrl", Modifier::Ctrl),
        ("shift", Modifier::Shift),
        ("left shift", Modifier::Shift),
        ("right shift", Modifier::Shift),
        ("alt", Modifier::Alt),
        ("left alt", Modifier::Alt),
        ("right alt", Modifier::Alt),
    ] {
        assert_eq!(
            classify_layer_key(Some(Layer::F9), &down(n)),
            LayerAction::Modifier(m),
            "{n}"
        );
    }
    // "alt gr" and "windows" are not modifiers to the layer: they dispatch.
    assert_eq!(
        classify_layer_key(Some(Layer::F10), &down("alt gr")),
        Dispatch(Layer::F10, "alt gr".into())
    );
    // The layer's own key repeating is eaten; the OTHER layer's key dispatches.
    assert_eq!(classify_layer_key(Some(Layer::F9), &down("f9")), Repeat);
    assert_eq!(classify_layer_key(Some(Layer::F10), &down("f10")), Repeat);
    assert_eq!(
        classify_layer_key(Some(Layer::F10), &down("f9")),
        Dispatch(Layer::F10, "f9".into())
    );
    // Names are lowercased (a shifted letter arrives as "A").
    assert_eq!(
        classify_layer_key(Some(Layer::F9), &down("A")),
        Dispatch(Layer::F9, "a".into())
    );
    assert_eq!(
        classify_layer_key(Some(Layer::F10), &down("!")),
        Dispatch(Layer::F10, "!".into())
    );
    assert_eq!(
        classify_layer_key(Some(Layer::F9), &down("")),
        Dispatch(Layer::F9, String::new())
    );
}

#[test]
fn blocking_hook_eats_before_anything_else_sees_the_key() {
    let l = Listener::new();
    let seen_keys = Arc::new(AtomicUsize::new(0));
    let s2 = Arc::clone(&seen_keys);
    l.add_blocking_key(
        next_id(),
        &[30],
        Arc::new(move |_| {
            s2.fetch_add(1, Ordering::SeqCst);
            true
        }),
    );
    let layer_id = next_id();
    l.add_blocking_hook(
        layer_id,
        Arc::new(|e: &KeyEvent| e.event_type == KeyEventType::Up),
    );
    // Down eaten by the blocking hook: no key hook ran, not queued, not pressed.
    assert_eq!(l.direct_callback(&raw("a", 30, true)), (false, false));
    assert_eq!(seen_keys.load(Ordering::SeqCst), 0);
    assert!(!l.any_pressed(&[30]));
    // Up passes and is queued.
    assert_eq!(l.direct_callback(&raw("a", 30, false)), (true, true));
    assert_eq!(seen_keys.load(Ordering::SeqCst), 1);
    l.remove(layer_id);
    assert_eq!(l.direct_callback(&raw("a", 30, true)), (true, true));
    assert!(l.any_pressed(&[30]));
}

#[test]
fn suppressed_key_hook_eats_downs_and_passes_ups() {
    let l = Listener::new();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let c2 = Arc::clone(&calls);
    // The `_hook_suppressed` callback shape.
    let id = next_id();
    l.add_blocking_key(
        id,
        &[72, 57416],
        Arc::new(move |e: &KeyEvent| {
            if e.event_type == KeyEventType::Up {
                return true;
            }
            c2.lock().push(e.name.clone());
            false
        }),
    );
    assert_eq!(l.direct_callback(&raw("up", 72, true)), (false, false));
    // Auto-repeat: every down reaches the handler.
    assert_eq!(l.direct_callback(&raw("up", 72, true)), (false, false));
    // The pressed table still saw the down (the table updates before key hooks run).
    assert!(l.any_pressed(&[72]));
    assert_eq!(l.direct_callback(&raw("up", 72, false)), (true, true));
    assert!(!l.any_pressed(&[72]));
    // Numpad 8 shares the scan code, so it is eaten too, carrying its own name.
    assert_eq!(l.direct_callback(&raw("8", 72, true)), (false, false));
    // Other keys are untouched.
    assert_eq!(l.direct_callback(&raw("down", 80, true)), (true, true));
    assert_eq!(*calls.lock(), vec!["up", "up", "8"]);
    l.remove(id);
    assert_eq!(l.direct_callback(&raw("up", 72, true)), (true, true));
}

#[test]
fn two_owners_on_one_key_release_independently() {
    let l = Listener::new();
    let a = next_id();
    let b = next_id();
    l.add_blocking_key(
        a,
        &[72],
        Arc::new(|e: &KeyEvent| e.event_type == KeyEventType::Up),
    );
    l.add_blocking_key(
        b,
        &[72],
        Arc::new(|e: &KeyEvent| e.event_type == KeyEventType::Up),
    );
    l.remove(a);
    assert_eq!(l.direct_callback(&raw("up", 72, true)), (false, false));
    l.remove(b);
    assert_eq!(l.direct_callback(&raw("up", 72, true)), (true, true));
}

#[test]
fn hotkey_fires_only_when_it_is_the_only_key_down() {
    let l = Listener::new();
    let fired = Arc::new(AtomicUsize::new(0));
    let f2 = Arc::clone(&fired);
    l.add_nonblocking_hotkey(
        next_id(),
        &[vec![67]],
        Arc::new(move |e: &KeyEvent| {
            if e.event_type == KeyEventType::Down {
                f2.fetch_add(1, Ordering::SeqCst);
            }
        }),
    );
    let feed = |ev: RawEvent| {
        let (pass, queue) = l.direct_callback(&ev);
        assert!(pass);
        if queue {
            assert!(l.process(&ev));
        }
    };
    feed(raw("f9", 67, true));
    assert_eq!(fired.load(Ordering::SeqCst), 1);
    feed(raw("f9", 67, true)); // auto-repeat fires again
    assert_eq!(fired.load(Ordering::SeqCst), 2);
    feed(raw("f9", 67, false));
    // Ctrl held first: Ctrl+F9 is not F9.
    feed(raw("ctrl", 29, true));
    feed(raw("f9", 67, true));
    assert_eq!(fired.load(Ordering::SeqCst), 2);
    feed(raw("f9", 67, false));
    feed(raw("ctrl", 29, false));
    feed(raw("f9", 67, true));
    assert_eq!(fired.load(Ordering::SeqCst), 3);
}

#[test]
fn on_release_handler_stops_later_handlers_on_key_down() {
    let l = Listener::new();
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let l1 = Arc::clone(&log);
    l.add_handler(
        next_id(),
        Arc::new(move |e: &KeyEvent| {
            if e.event_type == KeyEventType::Down {
                return true;
            }
            l1.lock().push(format!("release {}", e.name));
            false
        }),
    );
    let l2 = Arc::clone(&log);
    l.add_handler(
        next_id(),
        Arc::new(move |e: &KeyEvent| {
            l2.lock().push(format!("second {:?}", e.event_type));
            false
        }),
    );
    l.process(&raw("shift", 42, true));
    l.process(&raw("shift", 42, false));
    assert_eq!(*log.lock(), vec!["release shift", "second Up"]);
}

#[test]
fn layer_hook_notes_modifiers_and_dispatches_with_a_snapshot() {
    let _g = crate::speech_out::TEST_SERIAL.lock();
    // Drive the real layer statics through a private listener: install by hand.
    let l = Listener::new();
    KB_LAYER.store(LAYER_F10, Ordering::SeqCst);
    reset_live_mods();
    l.add_blocking_hook(next_id(), Arc::new(kb_layer_press));
    let release_id = next_id();
    l.add_handler(
        release_id,
        Arc::new(|e: &KeyEvent| {
            if e.event_type == KeyEventType::Down {
                return true;
            }
            kb_layer_release(e);
            false
        }),
    );
    run_pending_commands_for_tests();
    assert_eq!(l.direct_callback(&raw("shift", 42, true)), (false, false));
    assert!(LIVE_SHIFT.load(Ordering::SeqCst));
    // Shift's key-up passes, and the release handler clears the live flag.
    let up = raw("shift", 42, false);
    assert_eq!(l.direct_callback(&up), (true, true));
    l.process(&up);
    assert!(!LIVE_SHIFT.load(Ordering::SeqCst));
    // A command key is eaten and queued, not run inline.
    assert_eq!(l.direct_callback(&raw("1", 2, true)), (false, false));
    assert!(!COMMAND_QUEUE.1.is_empty());
    // Closing the layer before the worker gets to it drops the command (the F10 body
    // would otherwise run here).
    KB_LAYER.store(LAYER_NONE, Ordering::SeqCst);
    assert!(run_pending_commands_for_tests() >= 1);
    assert_eq!(l.direct_callback(&raw("1", 2, true)), (true, true));
}

#[test]
fn kb_enqueue_defers_the_handler_to_the_worker() {
    let _g = crate::speech_out::TEST_SERIAL.lock();
    run_pending_commands_for_tests();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = Arc::clone(&seen);
    let h = kb_enqueue(Arc::new(move |e: &KeyEvent| s2.lock().push(e.name.clone())));
    h(&KeyEvent::down("up"));
    h(&KeyEvent::down("down"));
    assert!(seen.lock().is_empty());
    // A panicking job does not stop the ones after it.
    command_dispatch(Box::new(|| panic!("boom")));
    h(&KeyEvent::down("escape"));
    assert!(run_pending_commands_for_tests() >= 4);
    assert_eq!(*seen.lock(), vec!["up", "down", "escape"]);
}

#[test]
fn process_key_names_events_and_tracks_shift() {
    let l = Listener::new();
    let got = Arc::new(Mutex::new(Vec::<(String, i32)>::new()));
    let g2 = Arc::clone(&got);
    l.add_blocking_hook(
        next_id(),
        Arc::new(move |e: &KeyEvent| {
            g2.lock().push((e.name.clone(), 0));
            true
        }),
    );
    // A key with scan code 0 is keyed by -vk.
    let (pass, queued) = process_key(&l, KeyEventType::Down, 0xFF, 0, 0, 0);
    assert!(pass);
    assert_eq!(queued.map(|e| e.scan_code), Some(-0xFF));
    // Left shift down, then "a" is named with Shift held, then shift up.
    process_key(&l, KeyEventType::Down, 0xA0, 42, 0, 0);
    let (_, q) = process_key(&l, KeyEventType::Down, 0x41, 30, 0, 0);
    process_key(&l, KeyEventType::Up, 0xA0, 42, 0, 0);
    let (_, q2) = process_key(&l, KeyEventType::Down, 0x41, 30, 0, 0);
    if cfg!(windows) {
        assert_eq!(q.map(|e| e.event.name), Some("A".to_string()));
        assert_eq!(q2.map(|e| e.event.name), Some("a".to_string()));
    }
}

/// The name tables against the real `keyboard` package on this machine's layout.
#[cfg(windows)]
#[test]
fn names_match_the_keyboard_package() {
    use beam_testutil::load;
    let fx = load("keyboard_names");
    // SAFETY: GetKeyboardLayout takes a thread id (0: this thread) and cannot fault.
    let layout = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout(0) };
    if fx["layout"].as_u64() != Some(layout.0 as u64 & 0xFFFF_FFFF) {
        eprintln!("skipping: keyboard layout differs from the fixture's");
        return;
    }
    names::setup_name_tables();
    for (key, want) in fx["key_to_scan_codes"].as_object().unwrap() {
        let got = names::key_to_scan_codes(key);
        match (want.get("ok"), want.get("err")) {
            (Some(ok), _) => {
                let want: Vec<i32> = ok
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_i64().unwrap() as i32)
                    .collect();
                assert_eq!(got, Ok(want), "key_to_scan_codes({key:?})");
            }
            (_, Some(err)) => assert_eq!(got, Err(err.as_str().unwrap().to_string()), "{key:?}"),
            _ => panic!("bad fixture row for {key:?}"),
        }
    }
    for e in fx["events"].as_array().unwrap() {
        let a = e.as_array().unwrap();
        let (scan, vk, ext, mods) = (
            a[0].as_u64().unwrap() as u32,
            a[1].as_u64().unwrap() as u32,
            a[2].as_u64().unwrap() as u32,
            a[3].as_u64().unwrap() as u8,
        );
        let want = a[4].as_str().map(str::to_string);
        assert_eq!(
            names::event_name(scan, vk, ext, mods),
            want,
            "event name for scan={scan} vk={vk} ext={ext} mods={mods}"
        );
    }
    for pair in fx["normalize"].as_array().unwrap() {
        let p = pair.as_array().unwrap();
        assert_eq!(
            names::normalize_name(p[0].as_str().unwrap()).as_deref(),
            p[1].as_str(),
            "normalize_name({})",
            p[0]
        );
    }
}

fn hotkey_listener(presses: &Arc<Mutex<Vec<i32>>>) -> Listener {
    let p = Arc::clone(presses);
    Listener::with_keys(
        Arc::new(|c| c == 29 || c == 42),
        Arc::new(move |c| p.lock().push(c)),
    )
}

#[test]
fn suppressed_single_key_hotkey_eats_down_and_up_when_callback_returns_none() {
    let presses = Arc::new(Mutex::new(Vec::new()));
    let l = hotkey_listener(&presses);
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::clone(&calls);
    // vehicle_spawner's F11: the callback queues and returns None.
    let id = next_id();
    l.add_blocking_hotkey(
        id,
        &[vec![87]],
        Arc::new(move || {
            c2.fetch_add(1, Ordering::SeqCst);
            false
        }),
    );
    // Eaten, but still queued for the non-blocking handlers.
    assert_eq!(l.direct_callback(&raw("f11", 87, true)), (false, true));
    assert_eq!(l.direct_callback(&raw("f11", 87, true)), (false, true));
    assert_eq!(calls.load(Ordering::SeqCst), 2, "auto-repeat fires again");
    // Never logically pressed, so its key-up is eaten too.
    assert_eq!(l.direct_callback(&raw("f11", 87, false)), (false, true));
    assert_eq!(calls.load(Ordering::SeqCst), 2, "key-up does not call back");
    // Other keys pass, and Ctrl+F11 is not F11.
    assert_eq!(l.direct_callback(&raw("a", 30, true)), (true, true));
    assert_eq!(l.direct_callback(&raw("a", 30, false)), (true, true));
    assert_eq!(l.direct_callback(&raw("ctrl", 29, true)), (true, true));
    assert_eq!(l.direct_callback(&raw("f11", 87, true)), (true, true));
    assert_eq!(l.direct_callback(&raw("f11", 87, false)), (true, true));
    assert_eq!(l.direct_callback(&raw("ctrl", 29, false)), (true, true));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(l.modifier_state(29), ModState::Free);
    // Removed: F11 passes again.
    l.remove(id);
    assert_eq!(l.direct_callback(&raw("f11", 87, true)), (true, true));
    assert_eq!(l.direct_callback(&raw("f11", 87, false)), (true, true));
    assert!(presses.lock().is_empty());
}

#[test]
fn suppressed_hotkey_whose_callback_accepts_lets_the_key_through() {
    let presses = Arc::new(Mutex::new(Vec::new()));
    let l = hotkey_listener(&presses);
    l.add_blocking_hotkey(next_id(), &[vec![87]], Arc::new(|| true));
    assert_eq!(l.direct_callback(&raw("f11", 87, true)), (true, true));
    // Logically pressed, so its key-up is allowed.
    assert_eq!(l.direct_callback(&raw("f11", 87, false)), (true, true));
}

#[test]
fn modifier_hotkey_follows_the_transition_table() {
    let presses = Arc::new(Mutex::new(Vec::new()));
    let l = hotkey_listener(&presses);
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::clone(&calls);
    l.add_blocking_hotkey(
        next_id(),
        &[vec![29, 87]],
        Arc::new(move || {
            c2.fetch_add(1, Ordering::SeqCst);
            false
        }),
    );
    // Ctrl is held back (pending) until it is known whether the hotkey follows.
    assert_eq!(l.direct_callback(&raw("ctrl", 29, true)), (false, true));
    assert_eq!(l.modifier_state(29), ModState::Pending);
    // Ctrl released alone: the held-back press is replayed, and the release passes.
    assert_eq!(l.direct_callback(&raw("ctrl", 29, false)), (true, true));
    assert_eq!(*presses.lock(), vec![29]);
    assert_eq!(l.modifier_state(29), ModState::Free);
    // Ctrl, then F11: the hotkey fires and eats F11; Ctrl becomes suppressed.
    assert_eq!(l.direct_callback(&raw("ctrl", 29, true)), (false, true));
    assert_eq!(l.direct_callback(&raw("f11", 87, true)), (false, true));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(l.modifier_state(29), ModState::Suppressed);
    assert_eq!(l.direct_callback(&raw("f11", 87, false)), (false, true));
    // Ctrl's release is eaten too (its press never reached the game).
    assert_eq!(l.direct_callback(&raw("ctrl", 29, false)), (false, true));
    assert_eq!(l.modifier_state(29), ModState::Free);
    assert_eq!(*presses.lock(), vec![29]);
}

#[test]
fn transition_table_matches_the_package() {
    use super::hook::{Origin, transition};
    use ModState::*;
    let n = None;
    // Every row of `_KeyboardListener.transition_table`, in its order.
    let rows = [
        (Free, false, Origin::Modifier, (false, Some(true), Free)),
        (Free, true, Origin::Modifier, (false, Some(false), Pending)),
        (Pending, false, Origin::Modifier, (true, Some(true), Free)),
        (
            Pending,
            true,
            Origin::Modifier,
            (false, Some(true), Allowed),
        ),
        (
            Suppressed,
            false,
            Origin::Modifier,
            (false, Some(false), Free),
        ),
        (
            Suppressed,
            true,
            Origin::Modifier,
            (false, Some(false), Suppressed),
        ),
        (Allowed, false, Origin::Modifier, (false, Some(true), Free)),
        (
            Allowed,
            true,
            Origin::Modifier,
            (false, Some(true), Allowed),
        ),
        (Free, false, Origin::Hotkey, (false, n, Free)),
        (Free, true, Origin::Hotkey, (false, n, Free)),
        (Pending, false, Origin::Hotkey, (false, n, Suppressed)),
        (Pending, true, Origin::Hotkey, (false, n, Suppressed)),
        (Suppressed, false, Origin::Hotkey, (false, n, Suppressed)),
        (Suppressed, true, Origin::Hotkey, (false, n, Suppressed)),
        (Allowed, false, Origin::Hotkey, (false, n, Allowed)),
        (Allowed, true, Origin::Hotkey, (false, n, Allowed)),
        (Free, false, Origin::Other, (false, Some(true), Free)),
        (Free, true, Origin::Other, (false, Some(true), Free)),
        (Pending, false, Origin::Other, (true, Some(true), Allowed)),
        (Pending, true, Origin::Other, (true, Some(true), Allowed)),
        (
            Suppressed,
            false,
            Origin::Other,
            (false, Some(false), Allowed),
        ),
        (Suppressed, true, Origin::Other, (true, Some(true), Allowed)),
        (Allowed, false, Origin::Other, (false, Some(true), Allowed)),
        (Allowed, true, Origin::Other, (false, Some(true), Allowed)),
    ];
    for (state, down, origin, want) in rows {
        assert_eq!(
            transition(state, down, origin),
            want,
            "{state:?} {down} {origin:?}"
        );
    }
}
