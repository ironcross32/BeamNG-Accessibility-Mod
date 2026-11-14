//! Owner: E5. Status metrics and rows, function items, the controller accessibility menu
//! navigator and status mode (`beamtel.py:5411-5974, 6036-6260`).
//!
//! `STATUS_METRICS` and `FUNCTION_ITEMS` are catalogs of closures. In Rust the closures take
//! the state explicitly, because the navigator evaluates them while holding `state_lock`:
//! `get_value(&State) -> (PyVal, unit)` and `is_available(&State) -> bool`. A function
//! item's handler runs AFTER the guard is released, as in the Python.
//!
//! STUB: signatures fixed by EA; bodies to be ported by E5.

use std::sync::Arc;

use crate::keyboard::KeyEvent;
use crate::not_ported;
use crate::state::State;
use crate::units::PyVal;

pub const ACCESSIBILITY_SCREENS: [&str; 3] = ["Status", "Functions", "Click spots"];
pub const ACCESSIBILITY_ACTIONS: [&str; 6] = [
    "status_up",
    "status_down",
    "status_repeat",
    "next_menu",
    "previous_menu",
    "activate",
];

pub type ValueGetter = Arc<dyn Fn(&State) -> (PyVal, String) + Send + Sync>;
pub type Availability = Arc<dyn Fn(&State) -> bool + Send + Sync>;

/// One status row (`{"label", "category"?, "getValue", "isAvailable"}`).
#[derive(Clone)]
pub struct StatusMetric {
    pub label: String,
    pub category: Option<String>,
    pub get_value: ValueGetter,
    pub is_available: Availability,
}

/// What a function item does: an F9 command `(name, ctrl, shift, alt)` or a handler.
#[derive(Clone)]
pub enum FunctionAction {
    Command(&'static str, bool, bool, bool),
    Handler(Arc<dyn Fn() + Send + Sync>),
}

/// One `_function_item(...)`.
#[derive(Clone)]
pub struct FunctionItem {
    pub label: String,
    pub category: String,
    pub is_available: Availability,
    pub action: FunctionAction,
}

/// `_status_rows()`: `STATUS_METRICS` with the wheel groups expanded.
pub fn status_rows(st: &State) -> Vec<StatusMetric> {
    let _ = st;
    not_ported!("menus::status_rows");
    Vec::new()
}

/// `FUNCTION_ITEMS`.
pub fn function_items() -> Vec<FunctionItem> {
    not_ported!("menus::function_items");
    Vec::new()
}

/// `_execute_function_item(item)`.
pub fn execute_function_item(item: &FunctionItem) {
    let _ = item;
    not_ported!("menus::execute_function_item");
}

/// `navigate_accessibility_menu(action)`.
pub fn navigate_accessibility_menu(action: &str) {
    let _ = action;
    not_ported!("menus::navigate_accessibility_menu");
}

/// `navigate_status(action)`.
pub fn navigate_status(action: &str) {
    navigate_accessibility_menu(action);
}

/// `_on_accessibility_action(action)`: the UI bridge's controller callback.
pub fn on_accessibility_action(action: &str) {
    let action = action.to_string();
    crate::keyboard::command_dispatch(Box::new(move || navigate_accessibility_menu(&action)));
}

/// `on_status_arrow_press(event)`.
pub fn on_status_arrow_press(event: &KeyEvent) {
    let _ = event;
    not_ported!("menus::on_status_arrow_press");
}

/// `toggle_status_mode()`.
pub fn toggle_status_mode() {
    not_ported!("menus::toggle_status_mode");
}
