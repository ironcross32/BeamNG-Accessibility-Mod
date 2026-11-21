//! The F9/F10 layer lifecycle, status mode and arrow ownership, driven through the
//! listener with synthetic key events (`keyboard::deliver_for_tests`). No system-wide hook
//! is installed: nothing here starts the command worker or the hotkeys.

use std::sync::Arc;

use beam_audio::controller::AudioController;
use beam_engine::keyboard::{self, KeyEvent, Layer};
use beam_engine::speech_out::{self, get_speech_log};
use beam_engine::state::{Mods, State, state};
use beam_engine::{menus, virtual_browser};
use parking_lot::Mutex;

static SERIAL: Mutex<()> = Mutex::new(());

fn setup() -> parking_lot::MutexGuard<'static, ()> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("beam_e5_kb_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: set once, before anything in this test binary reads the environment.
        unsafe { std::env::set_var("LOCALAPPDATA", &dir) };
    });
    let g = speech_out::TEST_SERIAL.lock();
    speech_out::reset_for_tests();
    speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
    *state() = State::default();
    keyboard::run_pending_commands_for_tests();
    g
}

fn said() -> Vec<String> {
    get_speech_log(None, Some(0), None, None, false)
        .entries
        .iter()
        .map(|e| e.text.clone())
        .collect()
}

fn down(key: &str) -> bool {
    keyboard::deliver_for_tests(key, &KeyEvent::down(key))
}

fn up(key: &str) -> bool {
    keyboard::deliver_for_tests(key, &KeyEvent::up(key))
}

#[test]
fn f9_layer_eats_keys_until_cleared() {
    let _s = SERIAL.lock();
    let _g = setup();
    keyboard::start_next_key_capture(AudioController::new());
    assert_eq!(keyboard::kb_layer(), Some(Layer::F9));
    assert!(state().kb.next_key_timer.is_some());
    // Modifiers and command keys are eaten; key-ups reach the game.
    assert!(!down("ctrl"));
    assert!(!down("a"));
    assert!(!down("f9"), "the F9 that opened the layer, repeating");
    assert!(up("a"));
    assert!(up("ctrl"));
    // The command runs on the worker with the modifier snapshot taken at key-down: check
    // what the F9 body receives, not what it leaves behind.
    let received: Arc<Mutex<Vec<(Layer, String, Mods)>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = Arc::clone(&received);
    keyboard::set_layer_body_override_for_tests(Some(Arc::new(
        move |layer: Layer, e: &KeyEvent| {
            r2.lock()
                .push((layer, e.name.clone(), state().kb.capture_mods));
        },
    )));
    keyboard::run_pending_commands_for_tests();
    keyboard::set_layer_body_override_for_tests(None);
    assert_eq!(
        *received.lock(),
        vec![(
            Layer::F9,
            "a".to_string(),
            Mods {
                ctrl: true,
                shift: false,
                alt: false
            }
        )]
    );
    keyboard::clear_next_key_hook(true);
    assert_eq!(keyboard::kb_layer(), None);
    assert!(state().kb.next_key_timer.is_none());
    assert_eq!(state().kb.capture_mods, Mods::default());
    assert!(!speech_out::command_context());
    assert_eq!(said(), vec!["Exit"]);
    // With no layer open every key passes untouched.
    assert!(down("a"));
    assert!(up("a"));
    speech_out::set_speak_override(None);
}

#[test]
fn f10_layer_takes_f9_as_a_command_and_closing_drops_queued_keys() {
    let _s = SERIAL.lock();
    let _g = setup();
    keyboard::kb_open_layer(Layer::F10);
    assert!(!down("f10"));
    assert!(!down("f9"), "F9 inside the F10 layer is an F10 command key");
    assert!(!down("shift"));
    assert!(!down("1"));
    // clear_next_key_hook only closes the F9 layer.
    keyboard::clear_next_key_hook(false);
    assert_eq!(keyboard::kb_layer(), Some(Layer::F10));
    keyboard::kb_close_layer();
    assert_eq!(keyboard::kb_layer(), None);
    // The layer closed before the worker ran them: nothing reaches the AI layer.
    keyboard::run_pending_commands_for_tests();
    assert_eq!(state().ai.ai_mods, Mods::default());
    assert!(down("1"));
    assert!(up("shift"));
    speech_out::set_speak_override(None);
}

#[test]
fn status_mode_owns_the_arrows_until_released() {
    let _s = SERIAL.lock();
    let _g = setup();
    menus::toggle_status_mode();
    assert!(state().menu.status_keyboard_mode_active);
    assert_eq!(state().menu.status_arrow_hooks.len(), 4);
    for key in ["up", "down", "left", "right"] {
        assert!(!down(key), "{key} is eaten in status mode");
        assert!(up(key), "{key} up passes");
    }
    // Other keys pass.
    assert!(down("a"));
    // The queued arrows navigate the Status screen on the worker.
    keyboard::run_pending_commands_for_tests();
    let spoken = said();
    assert_eq!(spoken[0], "Status mode on");
    assert!(spoken.len() > 1, "{spoken:?}");
    // A browser opened on top: status mode's hook was registered first on the arrows, so it
    // eats the key before the browser's hook runs (the package stops at the first key hook
    // that returns False). In the Python on_status_arrow_press then returned because a
    // browser was up, and the arrows were dead. Departure (python-bugs.md 42): status mode
    // hands the key to the browser, which moves once.
    virtual_browser::open_virtual_browser(
        vec!["One".into(), "Two".into()],
        virtual_browser::BrowserOpts::default(),
    );
    speech_out::reset_for_tests();
    assert!(!down("down"));
    keyboard::run_pending_commands_for_tests();
    assert_eq!(said(), vec!["Two"]);
    assert_eq!(state().vbrowser.vbrowser_index, 1);
    // release_arrow_owners closes the browser and status mode both.
    speech_out::reset_for_tests();
    keyboard::release_arrow_owners("test");
    assert!(!state().vbrowser.vbrowser_active);
    assert!(!state().menu.status_keyboard_mode_active);
    assert!(state().menu.status_arrow_hooks.is_empty());
    assert_eq!(said(), vec!["Status mode off"]);
    for key in ["up", "down", "left", "right", "escape"] {
        assert!(down(key), "{key} is back with the game");
        assert!(up(key));
    }
    speech_out::set_speak_override(None);
}

#[test]
fn orphaned_status_hooks_are_released_without_speech() {
    let _s = SERIAL.lock();
    let _g = setup();
    let rec = keyboard::hook_suppressed("up", keyboard::kb_enqueue(Arc::new(|_e: &KeyEvent| {})))
        .unwrap();
    state().menu.status_arrow_hooks.push(rec);
    assert!(!down("up"));
    keyboard::release_arrow_owners("vehicle spawner opened");
    assert!(state().menu.status_arrow_hooks.is_empty());
    assert!(down("up"));
    keyboard::run_pending_commands_for_tests();
    assert!(said().is_empty());
    speech_out::set_speak_override(None);
}

#[test]
fn browser_keys_follow_its_callbacks() {
    let _s = SERIAL.lock();
    let _g = setup();
    // Read-only: Enter and the side arrows stay with the game.
    virtual_browser::open_virtual_browser(
        vec!["A".into()],
        virtual_browser::BrowserOpts::titled("T"),
    );
    assert!(!down("up"));
    assert!(!down("escape"));
    assert!(down("enter"));
    assert!(down("left"));
    keyboard::run_pending_commands_for_tests();
    assert!(!state().vbrowser.vbrowser_active, "Escape closed it");
    assert_eq!(said(), vec!["T. A", "Top: A", "Exit"]);
    assert!(down("escape"));
    // Editable: Enter and left/right are taken.
    let hits = Arc::new(Mutex::new(0));
    let h2 = Arc::clone(&hits);
    let h3 = Arc::clone(&hits);
    virtual_browser::open_virtual_browser(
        vec!["A".into()],
        virtual_browser::BrowserOpts {
            on_enter: Some(Arc::new(move |_, _, _| *h2.lock() += 1)),
            on_adjust: Some(Arc::new(move |_, _, _, _| *h3.lock() += 10)),
            ..Default::default()
        },
    );
    assert!(!down("enter"));
    assert!(!down("left"));
    assert!(!down("right"));
    keyboard::run_pending_commands_for_tests();
    assert_eq!(*hits.lock(), 21);
    virtual_browser::close_virtual_browser(false);
    for key in ["up", "down", "left", "right", "enter", "escape"] {
        assert!(down(key), "{key}");
    }
    speech_out::set_speak_override(None);
}

#[test]
fn accessibility_actions_are_queued_not_run_inline() {
    let _s = SERIAL.lock();
    let _g = setup();
    menus::on_accessibility_action("status_repeat");
    assert!(said().is_empty());
    keyboard::run_pending_commands_for_tests();
    assert_eq!(said(), vec!["0.0 degrees"]);
    menus::on_accessibility_action("nonsense");
    keyboard::run_pending_commands_for_tests();
    assert_eq!(said().len(), 1);
    speech_out::set_speak_override(None);
}

/// python-bugs.md 55 (departure): a controller or MCP command run while the user has a
/// real F9 layer open leaves that layer open, its timeout running and its modifiers as
/// they were. The Python closed it.
#[test]
fn an_invoked_command_leaves_a_real_f9_layer_open() {
    let _s = SERIAL.lock();
    let _g = setup();
    let ac = AudioController::new();
    beam_engine::state::set_audio_controller_ref(Some(Arc::clone(&ac)));
    keyboard::start_next_key_capture(Arc::clone(&ac));
    state().kb.capture_mods = Mods {
        ctrl: true,
        shift: false,
        alt: false,
    };
    assert_eq!(
        beam_engine::f9_commands::invoke_f9_command("f", false, false, false),
        Ok(true)
    );
    assert_eq!(keyboard::kb_layer(), Some(Layer::F9));
    assert!(state().kb.next_key_timer.is_some());
    assert!(state().kb.capture_mods.ctrl);
    assert!(said().iter().any(|t| t.starts_with("Fuel")), "{:?}", said());
    // The real layer still closes normally.
    keyboard::clear_next_key_hook(false);
    assert_eq!(keyboard::kb_layer(), None);
    beam_engine::state::set_audio_controller_ref(None);
}
