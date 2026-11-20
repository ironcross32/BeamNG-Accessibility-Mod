//! Owner: E5. Status metrics and rows, function items, the controller accessibility menu
//! navigator and status mode (`beamtel.py:5411-5974, 6036-6260`).
//!
//! `STATUS_METRICS` and `FUNCTION_ITEMS` are catalogs of closures. In Rust the closures take
//! the state explicitly, because the navigator evaluates them while holding `state_lock`:
//! `get_value(&State) -> (PyVal, unit)` and `is_available(&State) -> bool`. A function
//! item's handler runs AFTER the guard is released, as in the Python.
//!
//! A getter or availability test that raises in Python (the Gear row with no gear byte
//! yet, an `int()` of NaN) panics here with the Python message, which unwinds out of the
//! navigator to the command worker exactly as the exception does.

use std::sync::{Arc, LazyLock};

use beam_base::gear_report::{DEFAULT_NUMBER_STYLE, extended_gear_to_phrase, gear_to_phrase};
use beam_base::pyfmt::{py_f, py_strip};
use parking_lot::RwLock;
use serde_json::Value;

use crate::keyboard::{self, KeyEvent, KeyHandler};
use crate::speech_out::{py_repr_str, say};
use crate::state::{
    IMPL_FLAG_ARTIC_VALID, IMPL_FLAG_GROUND_VALID, IMPL_FLAG_PRESENT, State, TRAILER_STALE_SEC,
    WHEEL_REPORT_STALE_SEC, state, world_is_active_in,
};
use crate::timer::monotonic;
use crate::units::{
    PyVal, TELEMETRY_HAS_CLUTCH, fmt_distance, fmt_odometer, fmt_power_kw, fmt_pressure, fmt_range,
    fmt_speed, fmt_temp_c_or_f, fmt_torque_nm, fmt_trip, fmt_turbo, has_air_system,
    has_electric_drive, has_engine_temp, has_stored_mileage, py_int, py_int_round,
};

pub const ACCESSIBILITY_SCREENS: [&str; 3] = ["Status", "Functions", "Click spots"];
pub const ACCESSIBILITY_ACTIONS: [&str; 6] = [
    "status_up",
    "status_down",
    "status_repeat",
    "next_menu",
    "previous_menu",
    "activate",
];

/// `OG_TURBO`: the OutGauge flag saying this vehicle exposes turbo boost. (The telemetry
/// decoder's copy is the canonical one; this is the same value.)
const OG_TURBO: i64 = 1 << 13;
const WHEEL_POS_FL: i64 = 1 << 0;
const WHEEL_POS_FR: i64 = 1 << 1;
const WHEEL_POS_RL: i64 = 1 << 2;
const WHEEL_POS_RR: i64 = 1 << 3;
const WHEEL_POS_F: i64 = 1 << 4;
const WHEEL_POS_R: i64 = 1 << 5;

pub type ValueGetter = Arc<dyn Fn(&State) -> (PyVal, String) + Send + Sync>;
pub type Availability = Arc<dyn Fn(&State) -> bool + Send + Sync>;
/// A group of rows computed from the state (`_wheel_rows(...)["rows"]`).
pub type RowsFn = Arc<dyn Fn(&State) -> Vec<StatusMetric> + Send + Sync>;

/// One status row (`{"label", "category"?, "getValue", "isAvailable"}`).
#[derive(Clone)]
pub struct StatusMetric {
    pub label: String,
    pub category: Option<String>,
    pub get_value: ValueGetter,
    pub is_available: Availability,
}

/// One `STATUS_METRICS` entry: a row, or a group expanded by `_status_rows()`.
#[derive(Clone)]
pub enum StatusEntry {
    Row(StatusMetric),
    Rows(RowsFn),
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

// ---- Test seams (the Python sims patch these module attributes) -------------------------

type F9Invoker = Arc<dyn Fn(&str, bool, bool, bool) + Send + Sync>;
type ClickspotActivator = Arc<dyn Fn(i64) + Send + Sync>;

static STATUS_OVERRIDE: RwLock<Option<Vec<StatusEntry>>> = RwLock::new(None);
static FUNCTIONS_OVERRIDE: RwLock<Option<Vec<FunctionItem>>> = RwLock::new(None);
static F9_INVOKER_OVERRIDE: RwLock<Option<F9Invoker>> = RwLock::new(None);
static CLICKSPOT_OVERRIDE: RwLock<Option<ClickspotActivator>> = RwLock::new(None);

/// `beamtel.STATUS_METRICS = ...` in a sim. `None` restores the real catalog.
#[doc(hidden)]
pub fn set_status_metrics_override(v: Option<Vec<StatusEntry>>) {
    *STATUS_OVERRIDE.write() = v;
}

/// `beamtel.FUNCTION_ITEMS = ...` in a sim. `None` restores the real catalog.
#[doc(hidden)]
pub fn set_function_items_override(v: Option<Vec<FunctionItem>>) {
    *FUNCTIONS_OVERRIDE.write() = v;
}

/// `beamtel._invoke_f9_command = ...` in a sim.
#[doc(hidden)]
pub fn set_f9_invoker_override(v: Option<F9Invoker>) {
    *F9_INVOKER_OVERRIDE.write() = v;
}

/// `beamtel._activate_clickspot = ...` in a sim.
#[doc(hidden)]
pub fn set_clickspot_activator_override(v: Option<ClickspotActivator>) {
    *CLICKSPOT_OVERRIDE.write() = v;
}

/// `_invoke_f9_command(...)` as the menus call it: its `RuntimeError` is not caught here.
fn invoke_f9(name: &str, ctrl: bool, shift: bool, alt: bool) {
    let over = F9_INVOKER_OVERRIDE.read().clone();
    if let Some(f) = over {
        f(name, ctrl, shift, alt);
        return;
    }
    if let Err(e) = crate::f9_commands::invoke_f9_command(name, ctrl, shift, alt) {
        panic!("RuntimeError: {e}");
    }
}

fn activate_clickspot(cache_idx: i64) {
    let over = CLICKSPOT_OVERRIDE.read().clone();
    if let Some(f) = over {
        f(cache_idx);
        return;
    }
    crate::listeners::clickspot::activate_clickspot(cache_idx);
}

// ---- Status rows -------------------------------------------------------------------------

fn unit(t: (PyVal, &'static str)) -> (PyVal, String) {
    (t.0, t.1.to_string())
}

fn s(text: impl Into<String>) -> PyVal {
    PyVal::Str(text.into())
}

fn always() -> Availability {
    Arc::new(|_| true)
}

fn row(
    label: &str,
    get_value: impl Fn(&State) -> (PyVal, String) + Send + Sync + 'static,
    is_available: impl Fn(&State) -> bool + Send + Sync + 'static,
) -> StatusEntry {
    StatusEntry::Row(StatusMetric {
        label: label.to_string(),
        category: None,
        get_value: Arc::new(get_value),
        is_available: Arc::new(is_available),
    })
}

/// `_wheel_status_metric(label, category, value_getter, position_flag)`.
fn wheel_status_metric(
    label: &str,
    category: &str,
    value_getter: impl Fn(&State) -> (PyVal, String) + Send + Sync + 'static,
    position_flag: i64,
) -> StatusMetric {
    StatusMetric {
        label: label.to_string(),
        category: Some(category.to_string()),
        get_value: Arc::new(value_getter),
        is_available: Arc::new(move |st: &State| {
            st.tel.protocol_mode == "extended"
                && (st.tel.last_telemetry_presence & position_flag) != 0
        }),
    }
}

/// `_wheel_report_live()`.
fn wheel_report_live(st: &State) -> bool {
    st.wheels.last_wheel_stamp > 0.0
        && (monotonic() - st.wheels.last_wheel_stamp) <= WHEEL_REPORT_STALE_SEC
}

/// Python truthiness of a JSON value (`if w["broken"]:`).
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(t) => !t.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `_wheel_rows(category, field, fmt, slot_rows)`: one row per wheel.
///
/// From wheelReport.lua's list while it is live, which knows every axle and anything
/// coupled; otherwise the struct's fixed slots. A wheel the game has no reading for gets
/// no row, and one that has broken off says so rather than reading a pressure of zero.
fn wheel_rows(
    category: &'static str,
    field: &'static str,
    fmt: fn(f64) -> (PyVal, &'static str),
    slot_rows: Vec<StatusMetric>,
) -> StatusEntry {
    StatusEntry::Rows(Arc::new(move |st: &State| {
        if !wheel_report_live(st) {
            return slot_rows.clone();
        }
        let mut out = Vec::new();
        for w in &st.wheels.last_wheel_rows {
            let get_value: ValueGetter = if truthy(&w["broken"]) {
                Arc::new(|_| (s("broken off"), String::new()))
            } else if w[field].is_null() {
                continue;
            } else {
                let value = w[field].as_f64().unwrap_or(f64::NAN);
                Arc::new(move |_| unit(fmt(value)))
            };
            let label = match &w["label"] {
                Value::String(t) => t.clone(),
                other => other.to_string(),
            };
            out.push(StatusMetric {
                label,
                category: Some(category.to_string()),
                get_value,
                is_available: always(),
            });
        }
        out
    }))
}

fn tire_pressure_slot_rows() -> Vec<StatusMetric> {
    let c = "Tire pressures";
    vec![
        wheel_status_metric(
            "front",
            c,
            |st| unit(fmt_pressure(st.tel.last_tire_pressure_f)),
            WHEEL_POS_F,
        ),
        wheel_status_metric(
            "front left",
            c,
            |st| unit(fmt_pressure(st.tel.last_tire_pressure_fl)),
            WHEEL_POS_FL,
        ),
        wheel_status_metric(
            "front right",
            c,
            |st| unit(fmt_pressure(st.tel.last_tire_pressure_fr)),
            WHEEL_POS_FR,
        ),
        wheel_status_metric(
            "rear",
            c,
            |st| unit(fmt_pressure(st.tel.last_tire_pressure_r)),
            WHEEL_POS_R,
        ),
        wheel_status_metric(
            "rear left",
            c,
            |st| unit(fmt_pressure(st.tel.last_tire_pressure_rl)),
            WHEEL_POS_RL,
        ),
        wheel_status_metric(
            "rear right",
            c,
            |st| unit(fmt_pressure(st.tel.last_tire_pressure_rr)),
            WHEEL_POS_RR,
        ),
    ]
}

fn brake_temp_slot_rows() -> Vec<StatusMetric> {
    let c = "Brake temperatures";
    vec![
        wheel_status_metric(
            "front",
            c,
            |st| unit(fmt_temp_c_or_f(st.tel.last_brake_temp_f)),
            WHEEL_POS_F,
        ),
        wheel_status_metric(
            "front left",
            c,
            |st| unit(fmt_temp_c_or_f(st.tel.last_brake_temp_fl)),
            WHEEL_POS_FL,
        ),
        wheel_status_metric(
            "front right",
            c,
            |st| unit(fmt_temp_c_or_f(st.tel.last_brake_temp_fr)),
            WHEEL_POS_FR,
        ),
        wheel_status_metric(
            "rear",
            c,
            |st| unit(fmt_temp_c_or_f(st.tel.last_brake_temp_r)),
            WHEEL_POS_R,
        ),
        wheel_status_metric(
            "rear left",
            c,
            |st| unit(fmt_temp_c_or_f(st.tel.last_brake_temp_rl)),
            WHEEL_POS_RL,
        ),
        wheel_status_metric(
            "rear right",
            c,
            |st| unit(fmt_temp_c_or_f(st.tel.last_brake_temp_rr)),
            WHEEL_POS_RR,
        ),
    ]
}

/// `_current_speed_limit()`.
fn current_speed_limit(st: &State) -> Option<f64> {
    let enabled = st.road_mode_active || crate::state::driving_assistant().active();
    crate::state::road_guidance_feed().speed_limit(enabled)
}

/// `_air_pressure_status()`.
fn air_pressure_status(st: &State) -> (PyVal, String) {
    let (val, u) = fmt_pressure(st.tel.last_air_pressure);
    if st.tel.last_air_pressure_max > 1.0 {
        let (max_val, _) = fmt_pressure(st.tel.last_air_pressure_max);
        return (s(format!("{val} of {max_val}")), u.to_string());
    }
    (val, u.to_string())
}

/// `int(last_implement_flags) & flag`.
fn impl_flag(st: &State, flag: i64) -> bool {
    (py_int(st.tel.last_implement_flags) & flag) != 0
}

/// `"degrees " + (near if abs(x) < 1.0 else (pos if x > 0 else neg))`.
fn degrees_word(x: f64, near: &str, pos: &str, neg: &str) -> String {
    let word = if x.abs() < 1.0 {
        near
    } else if x > 0.0 {
        pos
    } else {
        neg
    };
    format!("degrees {word}")
}

fn gear_value(st: &State) -> (PyVal, String) {
    let phrase = if st.tel.protocol_mode == "outgauge" {
        match st.tel.last_gear_byte {
            Some(b) => gear_to_phrase(b, DEFAULT_NUMBER_STYLE),
            // gear_to_phrase(None) compares None >= 2.
            None => {
                panic!("TypeError: '>=' not supported between instances of 'NoneType' and 'int'")
            }
        }
    } else {
        extended_gear_to_phrase(
            st.tel.last_gear_str.as_deref().unwrap_or(""),
            DEFAULT_NUMBER_STYLE,
        )
    };
    (s(phrase), String::new())
}

/// `STATUS_METRICS`.
static STATUS_METRICS: LazyLock<Vec<StatusEntry>> = LazyLock::new(|| {
    vec![
        row(
            "Heading",
            |st| (s(py_f(st.tel.last_heading, 1)), "degrees".to_string()),
            |_| true,
        ),
        row(
            "Speed",
            |st| unit(fmt_speed(st.tel.last_speed_ms)),
            |_| true,
        ),
        // Only while road data is flowing and the map gives this road a limit.
        row(
            "Speed limit",
            |st| {
                let limit = current_speed_limit(st).filter(|v| *v != 0.0).unwrap_or(0.0);
                unit(fmt_speed(limit))
            },
            |st| current_speed_limit(st).is_some(),
        ),
        row(
            "RPM",
            |st| (PyVal::Int(py_int_round(st.tel.last_rpm)), "RPM".to_string()),
            |_| true,
        ),
        row("Gear", gear_value, |_| true),
        // electrics.values.fuel is every energy storage combined, so on a pure EV it is the
        // battery charge. A hybrid keeps "Fuel", since there it mixes tank and battery.
        row(
            "Fuel",
            |st| {
                (
                    s(py_int_round(st.tel.last_fuel * 100.0).to_string()),
                    "percent".to_string(),
                )
            },
            |st| has_engine_temp(st) || !has_electric_drive(st),
        ),
        row(
            "Battery",
            |st| {
                (
                    s(py_int_round(st.tel.last_fuel * 100.0).to_string()),
                    "percent".to_string(),
                )
            },
            |st| has_electric_drive(st) && !has_engine_temp(st),
        ),
        // Availability must not follow the reading: rows are addressed by index, so a row
        // that vanished whenever the car stopped would shift every row below it under the
        // cursor, and the repeat button would read a different metric.
        row(
            "Range",
            |st| {
                if st.tel.last_ev_range_m >= 0.0 {
                    unit(fmt_range(st.tel.last_ev_range_m))
                } else {
                    (s("unknown"), String::new())
                }
            },
            has_electric_drive,
        ),
        row(
            "Motor Power",
            |st| unit(fmt_power_kw(st.tel.last_motor_power_kw)),
            has_electric_drive,
        ),
        row(
            "Motor Torque",
            |st| unit(fmt_torque_nm(st.tel.last_motor_torque_nm)),
            has_electric_drive,
        ),
        row(
            "Engine Temperature",
            |st| unit(fmt_temp_c_or_f(st.tel.last_engtemp)),
            has_engine_temp,
        ),
        row(
            "Oil Temperature",
            |st| unit(fmt_temp_c_or_f(st.tel.last_oiltemp)),
            has_engine_temp,
        ),
        row(
            "Clutch Temperature",
            |st| unit(fmt_temp_c_or_f(st.tel.last_clutch_temp)),
            |st| {
                st.tel.protocol_mode == "extended"
                    && (st.tel.last_telemetry_presence & TELEMETRY_HAS_CLUTCH) != 0
            },
        ),
        // The level's, not the vehicle's: present whenever a level is loaded.
        row(
            "Ambient Temperature",
            |st| match st.env.env_ambient_c {
                Some(c) => unit(fmt_temp_c_or_f(c)),
                None => {
                    panic!("TypeError: unsupported operand type(s) for *: 'NoneType' and 'int'")
                }
            },
            |st| st.env.env_ambient_c.is_some(),
        ),
        row(
            "Boost Pressure",
            |st| unit(fmt_turbo(st.tel.last_turbo)),
            |st| (st.tel.last_protocol_flags & OG_TURBO) != 0,
        ),
        // The same reading as F9 P, gated on the tank existing rather than on what it holds.
        row("Air Pressure", air_pressure_status, has_air_system),
        row(
            "Trip",
            |st| unit(fmt_trip(st.tel.last_trip_m)),
            |st| st.tel.protocol_mode == "extended" && st.tel.last_trip_m >= 0.0,
        ),
        row(
            "Odometer",
            |st| unit(fmt_odometer(st.tel.last_odometer_m)),
            |st| st.tel.protocol_mode == "extended" && has_stored_mileage(st),
        ),
        // The game's own statistic, kept per vehicle MODEL across sessions and counting
        // only player driving -- hence the label: two Pickups share one figure.
        row(
            "Model lifetime",
            |st| unit(fmt_odometer(st.tel.last_lifetime_m)),
            |st| st.tel.protocol_mode == "extended" && st.tel.last_lifetime_m >= 0.0,
        ),
        wheel_rows(
            "Tire pressures",
            "psi",
            fmt_pressure,
            tire_pressure_slot_rows(),
        ),
        // Loader implement (WL-40 bucket / forks). These three only appear when the mod
        // resolved an implement or hydraulic steering; implementFlags is 0 on every
        // ordinary vehicle.
        row(
            "Implement Height",
            |st| {
                let h = st.tel.last_implement_edge_height;
                // max(0.0, h)
                unit(fmt_distance(if h > 0.0 { h } else { 0.0 }))
            },
            |st| impl_flag(st, IMPL_FLAG_PRESENT) && impl_flag(st, IMPL_FLAG_GROUND_VALID),
        ),
        row(
            "Implement Tilt",
            |st| {
                let t = st.tel.last_implement_tilt_deg;
                (
                    s(py_f(t.abs(), 0)),
                    degrees_word(t, "level", "back", "forward"),
                )
            },
            |st| impl_flag(st, IMPL_FLAG_PRESENT),
        ),
        row(
            "Frame Articulation",
            |st| {
                let a = st.tel.last_articulation_deg;
                (
                    s(py_f(a.abs(), 0)),
                    degrees_word(a, "straight", "left", "right"),
                )
            },
            |st| impl_flag(st, IMPL_FLAG_ARTIC_VALID),
        ),
        // The same quantity as Frame Articulation with the hinge at the coupler; confirming
        // WHICH trailer the mod picked is a question only a name answers. Availability is
        // the live trailer id rather than the angle, since 0 degrees means in line.
        row(
            "Trailer Angle",
            |st| {
                let d = st.trailer.last_trailer_deg;
                let name = &st.trailer.last_trailer_name;
                let suffix = if name.is_empty() {
                    String::new()
                } else {
                    format!(", {name}")
                };
                (
                    s(py_f(d.abs(), 0)),
                    degrees_word(d, "in line", "left", "right") + &suffix,
                )
            },
            |st| {
                st.trailer.last_trailer_id.is_some()
                    && st.trailer.last_trailer_stamp > 0.0
                    && (monotonic() - st.trailer.last_trailer_stamp) <= TRAILER_STALE_SEC
            },
        ),
        wheel_rows(
            "Brake temperatures",
            "brake_c",
            fmt_temp_c_or_f,
            brake_temp_slot_rows(),
        ),
    ]
});

/// `STATUS_METRICS` (or a sim's replacement).
pub fn status_metrics() -> Vec<StatusEntry> {
    if let Some(v) = STATUS_OVERRIDE.read().as_ref() {
        return v.clone();
    }
    STATUS_METRICS.clone()
}

/// `_status_rows()`: `STATUS_METRICS` with the wheel groups expanded.
pub fn status_rows(st: &State) -> Vec<StatusMetric> {
    let mut out = Vec::new();
    for metric in status_metrics() {
        match metric {
            StatusEntry::Row(m) => out.push(m),
            StatusEntry::Rows(f) => out.extend(f(st)),
        }
    }
    out
}

// ---- Function items ----------------------------------------------------------------------

/// `_function_item(label, category, name, ctrl, shift, alt, is_available)` for a command.
fn cmd(
    label: &str,
    category: &str,
    name: &'static str,
    (ctrl, shift, alt): (bool, bool, bool),
    is_available: impl Fn(&State) -> bool + Send + Sync + 'static,
) -> FunctionItem {
    FunctionItem {
        label: label.to_string(),
        category: category.to_string(),
        is_available: Arc::new(is_available),
        action: FunctionAction::Command(name, ctrl, shift, alt),
    }
}

/// `_function_item(label, category, handler=..., is_available=...)`.
fn handler(
    label: &str,
    category: &str,
    h: Arc<dyn Fn() + Send + Sync>,
    is_available: impl Fn(&State) -> bool + Send + Sync + 'static,
) -> FunctionItem {
    FunctionItem {
        label: label.to_string(),
        category: category.to_string(),
        is_available: Arc::new(is_available),
        action: FunctionAction::Handler(h),
    }
}

const NONE: (bool, bool, bool) = (false, false, false);
const CTRL: (bool, bool, bool) = (true, false, false);
const SHIFT: (bool, bool, bool) = (false, true, false);
const ALT: (bool, bool, bool) = (false, false, true);
const CTRL_SHIFT: (bool, bool, bool) = (true, true, false);

fn yes(_: &State) -> bool {
    true
}

static TERRAIN_SCAN_HANDLER: LazyLock<Arc<dyn Fn() + Send + Sync>> =
    LazyLock::new(|| Arc::new(crate::listeners::terrain_scan::trigger_terrain_scan_driving_only));

/// The Terrain scan item's handler: `trigger_terrain_scan_driving_only` itself.
pub fn terrain_scan_handler() -> Arc<dyn Fn() + Send + Sync> {
    Arc::clone(&TERRAIN_SCAN_HANDLER)
}

/// `_fuel_target_id is not None`.
fn fuel_target_set(st: &State) -> bool {
    st.fuel
        .fuel_target_id
        .as_ref()
        .is_some_and(|v| !v.is_null())
}

/// `FUNCTION_ITEMS`: a flat controller catalog over the existing F9 command layer.
/// Categories are spoken boundaries, not nested menus. Availability is deliberately live:
/// modes toggled by one item can expose or remove dependent items before the next press.
static FUNCTION_ITEMS: LazyLock<Vec<FunctionItem>> = LazyLock::new(|| {
    let scan = |st: &State| st.scanner.scan_mode_active;
    let marked = |st: &State| st.tel.marked_coord_x.is_some();
    let extended = |st: &State| st.tel.protocol_mode == "extended";
    let turbo = |st: &State| {
        st.tel.protocol_mode == "extended" && (st.tel.last_protocol_flags & OG_TURBO) != 0
    };
    let free_cam = |st: &State| st.camera.free_cam_active;
    let vs = "Vehicle scanner";
    let al = "Alignment";
    let da = "Driving assistance";
    let wp = "Waypoints";
    let vi = "Vehicle information";
    let cam = "Camera";
    let ia = "Interaction";
    vec![
        cmd("Toggle vehicle scanner", vs, "v", CTRL, yes),
        cmd("Next vehicle scanner target", vs, "tab", NONE, yes),
        cmd("Previous vehicle scanner target", vs, "tab", SHIFT, yes),
        cmd("Lock onto nearest vehicle", vs, "tab", CTRL, yes),
        cmd("Distance and orientation", vs, "d", NONE, scan),
        cmd("Relative bearing", vs, "d", SHIFT, scan),
        cmd("Trailer or ramp alignment", vs, "v", SHIFT, |st: &State| {
            st.scanner.scan_mode_active || st.dock.dock_mode_active
        }),
        cmd("Coupler distance callouts", vs, "d", CTRL_SHIFT, scan),
        cmd("Toggle alignment instrument", al, "i", CTRL, yes),
        cmd("Alignment or cannon readout", al, "i", NONE, yes),
        cmd("Cycle reference band", al, "i", SHIFT, yes),
        cmd("Pedal tones", da, "c", CTRL, yes),
        cmd("Heading guidance", da, "h", CTRL, yes),
        cmd("Coordinate guidance", da, "g", CTRL, marked),
        cmd("Drift detection", da, "d", CTRL, yes),
        cmd("Low-speed detection", da, "l", CTRL_SHIFT, yes),
        cmd("Wheel-slip detection", da, "k", CTRL, yes),
        cmd("Obstacle detection", da, "o", CTRL, yes),
        cmd("Road detection", da, "r", CTRL, yes),
        cmd("Road-status readout", da, "r", CTRL_SHIFT, yes),
        cmd("Mark waypoint", wp, "c", SHIFT, yes),
        cmd("Speak marked coordinates", wp, "c", ALT, marked),
        cmd("Distance and bearing", wp, "w", NONE, marked),
        cmd(
            "Toggle route beacon",
            wp,
            "w",
            CTRL,
            crate::listeners::route_beacon::route_is_set,
        ),
        cmd(
            "Nearest point of interest",
            wp,
            "w",
            SHIFT,
            world_is_active_in,
        ),
        handler(
            "Fuel stations",
            wp,
            Arc::new(|| crate::listeners::fuel::open_fuel_stations(true)),
            world_is_active_in,
        ),
        handler(
            "Park at a pump",
            wp,
            Arc::new(|| crate::listeners::fuel::open_fuel_stations(false)),
            |st: &State| {
                world_is_active_in(st)
                    && fuel_target_set(st)
                    && st.fuel.fuel_in_range
                    && !st.fuel.fuel_parking
            },
        ),
        handler(
            "Cancel parking",
            wp,
            Arc::new(|| crate::listeners::fuel::open_fuel_stations(false)),
            |st: &State| st.fuel.fuel_parking,
        ),
        cmd("Redline RPM", vi, "r", SHIFT, extended),
        cmd("Maximum boost pressure", vi, "t", SHIFT, turbo),
        cmd("Forced induction details", vi, "t", ALT, turbo),
        cmd("Air pressure", vi, "p", NONE, has_air_system),
        cmd("Attitude", vi, "a", NONE, yes),
        cmd("Coordinates", vi, "c", NONE, yes),
        cmd("Damage report", vi, "m", NONE, yes),
        cmd("Toggle camera information", cam, "f", ALT, yes),
        cmd("Camera heading", cam, "h", ALT, free_cam),
        cmd("Camera altitude", cam, "a", ALT, free_cam),
        cmd("Camera pitch", cam, "p", ALT, free_cam),
        cmd("Vehicle bearing", cam, "v", ALT, free_cam),
        cmd("Vehicle distance", cam, "d", ALT, free_cam),
        cmd("Accessible node grabber", ia, "n", CTRL, yes),
        // Reachable from the controller as well as from F9: this mode exists to be used by
        // somebody holding a pad they do not yet understand. The mod's own six
        // accessibility actions keep firing while it is on, so the menu that armed it can
        // still disarm it.
        cmd("Learn bindings mode", ia, "b", SHIFT, yes),
        cmd("Clickspot detection", ia, "c", CTRL_SHIFT, yes),
        // Available only while the UI runtime has a readable screen latched, so the entry
        // is absent everywhere else rather than present and answering "nothing to read".
        handler(
            "Read page text",
            ia,
            Arc::new(|| {
                crate::browsers::open_page_text_browser();
            }),
            |st: &State| st.page.ui_screen_context == "mod_details",
        ),
        handler(
            "Hill-climb reports",
            "Challenge reports",
            Arc::new(|| crate::listeners::hill_climb::open_hill_climb_history(0)),
            |_| {
                !crate::state::hill_climb_challenge()
                    .list_reports(1)
                    .is_empty()
            },
        ),
        cmd("Switch units", ia, "u", NONE, yes),
        handler(
            "Terrain scan",
            "Environment",
            terrain_scan_handler(),
            world_is_active_in,
        ),
    ]
});

/// `FUNCTION_ITEMS` (or a sim's replacement).
pub fn function_items() -> Vec<FunctionItem> {
    if let Some(v) = FUNCTIONS_OVERRIDE.read().as_ref() {
        return v.clone();
    }
    FUNCTION_ITEMS.clone()
}

/// `_execute_function_item(item)`.
pub fn execute_function_item(item: &FunctionItem) {
    match &item.action {
        FunctionAction::Handler(h) => h(),
        FunctionAction::Command(name, ctrl, shift, alt) => invoke_f9(name, *ctrl, *shift, *alt),
    }
}

// ---- The navigator -----------------------------------------------------------------------

enum ClickspotAction {
    Enable,
    Activate(i64),
}

/// `navigate_accessibility_menu(action)`: navigate or activate the controller-facing
/// accessibility screens.
pub fn navigate_accessibility_menu(action: &str) {
    if !ACCESSIBILITY_ACTIONS.contains(&action) {
        log::warn!(
            "Ignoring unknown accessibility action: {}",
            py_repr_str(action)
        );
        return;
    }

    // A virtual browser wins over the three accessibility screens, which is the rule
    // on_status_arrow_press already applies on the keyboard. Without it the pad could OPEN
    // a browser from the Functions menu and then have no way to move through it, because
    // open_virtual_browser hooks the keyboard arrows and nothing else. next_menu /
    // previous_menu is the way out.
    if state().vbrowser.vbrowser_active {
        crate::virtual_browser::drive_virtual_browser(action);
        return;
    }

    let mut execute_item: Option<FunctionItem> = None;
    let mut clickspot_action: Option<ClickspotAction> = None;
    let mut speech: Option<String> = None;
    {
        let mut st = state();
        let n_screens = ACCESSIBILITY_SCREENS.len() as i64;
        if action == "next_menu" {
            st.menu.current_accessibility_screen_index =
                (st.menu.current_accessibility_screen_index + 1).rem_euclid(n_screens);
        } else if action == "previous_menu" {
            st.menu.current_accessibility_screen_index =
                (st.menu.current_accessibility_screen_index - 1).rem_euclid(n_screens);
        }

        let switched_screen = action == "next_menu" || action == "previous_menu";
        if st.menu.current_accessibility_screen_index == 0 {
            let available: Vec<StatusMetric> = status_rows(&st)
                .into_iter()
                .filter(|m| (m.is_available)(&st))
                .collect();
            if available.is_empty() {
                speech = Some("No status metrics available".to_string());
            } else {
                let n = available.len() as i64;
                st.menu.current_status_metric_index =
                    st.menu.current_status_metric_index.rem_euclid(n);
                let previous_category = available[st.menu.current_status_metric_index as usize]
                    .category
                    .clone();
                if !switched_screen && action == "status_down" {
                    st.menu.current_status_metric_index =
                        (st.menu.current_status_metric_index + 1).rem_euclid(n);
                } else if !switched_screen && action == "status_up" {
                    st.menu.current_status_metric_index =
                        (st.menu.current_status_metric_index - 1).rem_euclid(n);
                }

                let metric = &available[st.menu.current_status_metric_index as usize];
                let (value, unit) = (metric.get_value)(&st);
                let value_text = py_strip(&format!("{value} {unit}")).to_string();
                let category = metric.category.clone().filter(|c| !c.is_empty());
                let mut label = metric.label.clone();
                if switched_screen {
                    if let Some(c) = &category {
                        label = format!("{c}: {label}");
                    }
                    speech = Some(format!("Status: {label}, {value_text}"));
                } else if action == "status_repeat" || action == "activate" {
                    speech = Some(value_text);
                } else {
                    if let Some(c) = &category {
                        if Some(c) != previous_category.as_ref() {
                            label = format!("{c}: {label}");
                        }
                    }
                    speech = Some(format!("{label}, {value_text}"));
                }
            }
        } else if st.menu.current_accessibility_screen_index == 1 {
            let available: Vec<FunctionItem> = function_items()
                .into_iter()
                .filter(|i| (i.is_available)(&st))
                .collect();
            if available.is_empty() {
                speech = Some("No functions available".to_string());
            } else {
                let n = available.len() as i64;
                st.menu.current_functions_item_index =
                    st.menu.current_functions_item_index.rem_euclid(n);
                let previous_category = available[st.menu.current_functions_item_index as usize]
                    .category
                    .clone();
                if !switched_screen && action == "status_down" {
                    st.menu.current_functions_item_index =
                        (st.menu.current_functions_item_index + 1).rem_euclid(n);
                } else if !switched_screen && action == "status_up" {
                    st.menu.current_functions_item_index =
                        (st.menu.current_functions_item_index - 1).rem_euclid(n);
                }

                let item = &available[st.menu.current_functions_item_index as usize];
                let mut label = item.label.clone();
                let category = &item.category;
                if switched_screen {
                    speech = Some(format!("Functions: {category}: {label}"));
                } else if action == "activate" {
                    execute_item = Some(item.clone());
                } else if action == "status_repeat" {
                    speech = Some(label);
                } else {
                    if *category != previous_category {
                        label = format!("{category}: {label}");
                    }
                    speech = Some(label);
                }
            }
        } else {
            // (kind, cache_idx, label)
            let items: Vec<(&str, Option<i64>, String)> = if !st.clickspot.clickspot_mode_active {
                vec![("enable", None, "Turn on clickspot detection".to_string())]
            } else if st.clickspot.clickspot_list_loading {
                vec![("status", None, "Detecting click spots".to_string())]
            } else if st.clickspot.clickspot_trigger_list.is_empty() {
                vec![("status", None, "No click spots found".to_string())]
            } else {
                st.clickspot
                    .clickspot_trigger_list
                    .iter()
                    .map(|(cache_idx, _trigger_id, display_name)| {
                        ("clickspot", Some(*cache_idx), display_name.clone())
                    })
                    .collect()
            };

            let n = items.len() as i64;
            st.menu.current_clickspot_index = st.menu.current_clickspot_index.rem_euclid(n);
            if !switched_screen && action == "status_down" {
                st.menu.current_clickspot_index =
                    (st.menu.current_clickspot_index + 1).rem_euclid(n);
            } else if !switched_screen && action == "status_up" {
                st.menu.current_clickspot_index =
                    (st.menu.current_clickspot_index - 1).rem_euclid(n);
            }

            let (kind, cache_idx, label) = &items[st.menu.current_clickspot_index as usize];
            if switched_screen {
                speech = Some(format!("Click spots: {label}"));
            } else if action == "activate" {
                if *kind == "enable" {
                    clickspot_action = Some(ClickspotAction::Enable);
                } else if *kind == "clickspot" {
                    clickspot_action = cache_idx.map(ClickspotAction::Activate);
                } else {
                    speech = Some(label.clone());
                }
            } else {
                speech = Some(label.clone());
            }
        }
    }

    if let Some(text) = speech {
        say(&text, true, true, "python");
    }
    if let Some(item) = execute_item {
        execute_function_item(&item);
    }
    match clickspot_action {
        Some(ClickspotAction::Enable) => invoke_f9("c", true, true, false),
        Some(ClickspotAction::Activate(idx)) => activate_clickspot(idx),
        None => {}
    }
}

/// `navigate_status(action)`: compatibility entry point for Ctrl+S arrow navigation.
pub fn navigate_status(action: &str) {
    navigate_accessibility_menu(action);
}

/// `_on_accessibility_action(action)`: the UI bridge's controller callback. It runs on the
/// bridge's thread; menu reads and speech stay serialised with keyboard commands on the
/// command worker.
pub fn on_accessibility_action(action: &str) {
    let action = action.to_string();
    keyboard::command_dispatch(Box::new(move || navigate_accessibility_menu(&action)));
}

/// `on_status_arrow_press(event)`.
pub fn on_status_arrow_press(event: &KeyEvent) {
    // Don't steal the arrows while a virtual browser (vehicle selector, clickspot list,
    // bindings list) owns them.
    {
        let st = state();
        if st.vbrowser.vbrowser_active {
            return;
        }
        if !st.menu.status_keyboard_mode_active {
            return;
        }
    }
    let action = match event.name.as_str() {
        "up" => Some("status_up"),
        "down" => Some("status_down"),
        "left" | "right" => Some("status_repeat"),
        _ => None,
    };
    if let Some(a) = action {
        navigate_status(a);
    }
}

/// `toggle_status_mode()`.
pub fn toggle_status_mode() {
    let active = {
        let mut st = state();
        st.menu.status_keyboard_mode_active = !st.menu.status_keyboard_mode_active;
        if st.menu.status_keyboard_mode_active {
            st.menu.current_status_metric_index = 0;
        }
        st.menu.status_keyboard_mode_active
    };
    if active {
        crate::listeners::lineup::lineup_close_layer(false);
        say("Status mode on", true, true, "python");
        let h: KeyHandler = Arc::new(on_status_arrow_press);
        for key in ["up", "down", "left", "right"] {
            match keyboard::try_hook_suppressed(key, keyboard::kb_enqueue(Arc::clone(&h))) {
                Ok(rec) => state().menu.status_arrow_hooks.push(rec),
                Err(e) => {
                    log::error!("Failed to hook status mode keys: {e}");
                    break;
                }
            }
        }
    } else {
        say("Status mode off", true, true, "python");
        let mut hooks = std::mem::take(&mut state().menu.status_arrow_hooks);
        keyboard::unhook_suppressed(&mut hooks);
    }
}
