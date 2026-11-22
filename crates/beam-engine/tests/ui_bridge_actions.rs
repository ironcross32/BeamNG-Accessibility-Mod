//! The E5 carry-over from `diagnostic/controller_menu_sim.py`: "Python bridge allows
//! exactly the six controller actions". Every action the controller menu handles reaches
//! the registered callback through `accessibility_action`; anything else is dropped.

use std::sync::Arc;

use beam_engine::{menus, ui_bridge};
use beam_road::pyjson::{self, Value as PyJson};
use parking_lot::Mutex;

fn action_msg(action: &str) -> PyJson {
    pyjson::dict(vec![
        ("type", PyJson::from("accessibility_action")),
        ("action", PyJson::from(action)),
    ])
}

#[test]
fn bridge_allows_exactly_the_six_controller_actions() {
    ui_bridge::reset_for_tests();
    let received = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&received);
    ui_bridge::register_accessibility_action_callback(Arc::new(move |a| {
        sink.lock().push(a.to_string())
    }));
    for action in menus::ACCESSIBILITY_ACTIONS {
        ui_bridge::handle_ws_message(&action_msg(action)).unwrap();
    }
    ui_bridge::handle_ws_message(&action_msg("not_a_command")).unwrap();

    let got = received.lock().clone();
    assert_eq!(got, menus::ACCESSIBILITY_ACTIONS, "actions delivered");
    assert_eq!(
        ui_bridge::ACCESSIBILITY_ACTIONS,
        menus::ACCESSIBILITY_ACTIONS,
        "the bridge's allowlist is the controller menu's"
    );
}
