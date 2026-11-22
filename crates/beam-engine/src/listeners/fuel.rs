//! Owner: E2. Fuel stations: listener, timers, browser, events and the key (`beamtel.py:6497-6690`).
//!
//! F9 then Shift+F chooses, parks, or cancels parking; F9 then Ctrl+Shift+F always opens
//! the list. See docs/lua-fuel-stations.md. Rows are kept in `state().fuel` as the JSON
//! of `fuel_stations.parse_row`'s dict.

use std::sync::Arc;

use beam_base::pyfmt::py_strip;
use beam_reports::fuel_stations::{self, StationRow};
use beam_reports::pyops::{PyDict, PyErrKind, PyValue};
use serde_json::{Value, json};

use crate::convert::{json_to_pyops, pyops_to_json};
use crate::listeners::scanner::e2_send;
use crate::net::{Decode, FUEL_CMD_PORT, FUEL_LISTEN_PORT, Recv, UdpListener, decode};
use crate::speech_out::{say, say_value};
use crate::state::{state, world_is_active};
use crate::timer::{Event, Timer};
use crate::units::units_mode;
use crate::virtual_browser::{BrowserOpts, close_virtual_browser, open_virtual_browser};

/// Past the GE side's 1 s wait for the vehicle's tanks.
pub const FUEL_REPLY_TIMEOUT_S: f64 = 2.5;

/// `_send_fuel_cmd(command)` (port FUEL_CMD_PORT).
pub fn send_fuel_cmd(command: &str) {
    e2_send(FUEL_CMD_PORT, command, Some("fuel station command"));
}

/// A parsed row as the JSON kept in `state().fuel.fuel_rows`.
#[doc(hidden)]
pub fn row_to_json(row: &StationRow) -> Value {
    let pumps = i64::try_from(row.pumps).unwrap_or(if row.pumps < 0 { i64::MIN } else { i64::MAX });
    json!({
        "id": row.id,
        "name": row.name,
        "official": row.official,
        "energy": row.energy,
        "distance": crate::convert::json_float(row.distance),
        "bearing": crate::convert::json_float(row.bearing),
        "pumps": pumps,
        "compatible": pyops_to_json(&row.compatible),
        "target": row.target,
    })
}

/// The row back from its JSON.
#[doc(hidden)]
pub fn row_from_json(v: &Value) -> StationRow {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    StationRow {
        id: s("id"),
        name: s("name"),
        official: b("official"),
        energy: v
            .get("energy")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        distance: f("distance"),
        bearing: f("bearing"),
        pumps: v.get("pumps").and_then(Value::as_i64).unwrap_or(0) as i128,
        compatible: json_to_pyops(v.get("compatible").unwrap_or(&Value::Null)),
        target: b("target"),
    }
}

fn fuel_cancel_timer() {
    let timer = state().fuel.fuel_timer.take();
    if let Some(t) = timer {
        t.cancel();
    }
}

fn fuel_arm_timer(seconds: f64) {
    fuel_cancel_timer();
    let t = Timer::start(seconds, fuel_timed_out);
    state().fuel.fuel_timer = Some(t);
}

/// `_fuel_timed_out()`: the reply timer fired.
#[doc(hidden)]
pub fn fuel_timed_out() {
    {
        let mut st = state();
        if !st.fuel.fuel_pending {
            return;
        }
        st.fuel.fuel_pending = false;
    }
    say("No response from the game", true, true, "python");
}

fn fuel_request_list() {
    state().fuel.fuel_pending = true;
    fuel_arm_timer(FUEL_REPLY_TIMEOUT_S);
    send_fuel_cmd("LIST");
}

/// What `_fuel_open_browser` builds: lines, entry data (`Null` for the stop line), title.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct FuelBrowser {
    pub lines: Vec<String>,
    pub data: Vec<Value>,
    pub title: String,
    /// "No fuel stations found on this map" is spoken.
    pub none_found: bool,
}

/// The browser `_fuel_open_browser` opens for these rows and this target, or `None` when
/// there is nothing to list (the "none found" line is still spoken).
#[doc(hidden)]
pub fn fuel_browser(
    rows: &[Value],
    target_id: Option<&Value>,
    target_name: Option<&str>,
    units: &str,
) -> FuelBrowser {
    let mut lines = Vec::new();
    let mut data = Vec::new();
    if target_id.is_some() {
        lines.push(format!(
            "Stop fuel station assistance for {}",
            target_name
                .filter(|n| !n.is_empty())
                .unwrap_or("the station")
        ));
        data.push(Value::Null);
    }
    for row in rows {
        lines.push(fuel_stations::row_line(&row_from_json(row), units));
        data.push(row.clone());
    }
    let count = rows.len();
    let title = if count != 1 {
        format!("Fuel stations, {count}")
    } else {
        "Fuel stations, 1".to_string()
    };
    FuelBrowser {
        lines,
        data,
        title,
        none_found: rows.is_empty(),
    }
}

/// The browser's `choose(_index, _line, row)`: Enter on a station targets it; Enter on
/// the stop line (`None`) cancels.
#[doc(hidden)]
pub fn fuel_choose(row: Option<Value>) {
    close_virtual_browser(false);
    match row.filter(|r| !r.is_null()) {
        None => send_fuel_cmd("CANCEL"),
        Some(r) => {
            let id = r.get("id").and_then(Value::as_str).unwrap_or("");
            send_fuel_cmd(&format!("TARGET:{id}"));
        }
    }
}

/// `_fuel_open_browser()`: a fresh row set has landed and a browser was asked for
/// (command worker).
fn fuel_open_browser() {
    let (rows, target_id, target_name) = {
        let mut st = state();
        if !st.fuel.fuel_pending {
            return;
        }
        st.fuel.fuel_pending = false;
        (
            st.fuel.fuel_rows.clone(),
            st.fuel.fuel_target_id.clone(),
            st.fuel.fuel_target_name.clone(),
        )
    };
    fuel_cancel_timer();
    let b = fuel_browser(
        &rows,
        target_id.as_ref(),
        target_name.as_deref(),
        &units_mode(),
    );
    if b.none_found {
        say("No fuel stations found on this map", true, true, "python");
        if b.lines.is_empty() {
            return;
        }
    }
    let choose = Arc::new(|_index: usize, _line: &str, row: Option<Value>| fuel_choose(row));
    open_virtual_browser(
        b.lines,
        BrowserOpts {
            title: Some(b.title),
            on_enter: Some(choose),
            entry_data: b.data,
            ..Default::default()
        },
    );
}

/// `open_fuel_stations(force_list=False)` (F9 Shift+F): park when at the chosen station,
/// cancel parking, or choose one.
pub fn open_fuel_stations(force_list: bool) {
    if !world_is_active() {
        say(
            "Fuel stations are only available while driving",
            true,
            true,
            "python",
        );
        return;
    }
    if !force_list {
        let (parking, has_target, in_range) = {
            let st = state();
            (
                st.fuel.fuel_parking,
                st.fuel.fuel_target_id.is_some(),
                st.fuel.fuel_in_range,
            )
        };
        if parking {
            send_fuel_cmd("PARK_CANCEL");
            return;
        }
        if has_target && in_range {
            send_fuel_cmd("PARK");
            return;
        }
    }
    fuel_request_list();
}

/// `_fuel_on_event(data)`: keep the little state the key needs, then say what happened
/// (command worker).
#[doc(hidden)]
pub fn fuel_on_event(data: &PyDict) {
    let kind = data.get("kind").cloned().unwrap_or(PyValue::None);
    {
        let mut st = state();
        let f = &mut st.fuel;
        match kind.as_str() {
            Some("target") => {
                f.fuel_target_id = match data.get("id") {
                    None | Some(PyValue::None) => None,
                    Some(v) => Some(pyops_to_json(v)),
                };
                f.fuel_target_name = data
                    .get("station")
                    .filter(|v| v.truthy())
                    .map(PyValue::py_str);
                f.fuel_in_range = data.get("here").is_some_and(PyValue::truthy);
            }
            Some("in_range") => f.fuel_in_range = true,
            Some("left_range") => f.fuel_in_range = false,
            Some("parking") => f.fuel_parking = true,
            Some("parked" | "park_failed" | "park_refused" | "park_cancelled") => {
                f.fuel_parking = false
            }
            // A new catalogue (scan_done) means a new level or a reload: the old target is
            // gone on that side.
            Some("cancelled" | "scan_done") => {
                f.fuel_target_id = None;
                f.fuel_target_name = None;
                f.fuel_in_range = false;
                f.fuel_parking = false;
            }
            _ => {}
        }
    }
    let phrase = fuel_stations::event_phrase(data);
    if let Some(phrase) = phrase.filter(PyValue::truthy) {
        log::info!("Fuel stations: {}: {}", kind.py_str(), phrase.py_str());
        say_value(&pyops_to_json(&phrase), true, false, "fuel_stations");
    }
}

/// One decoded, stripped, non-empty datagram from 4486. `dispatch` is
/// `_command_dispatch`: events and the browser run on the command worker.
#[doc(hidden)]
pub fn handle_fuel_text(text: &str, dispatch: &dyn Fn(Box<dyn FnOnce() + Send + 'static>)) {
    if text == "FS_WAIT" {
        // Still building the catalogue: say so once and keep waiting.
        if state().fuel.fuel_pending {
            fuel_arm_timer(8.0);
            say("Finding fuel stations", true, true, "python");
        }
        return;
    }
    if text.starts_with("FS_BEGIN:") {
        state().fuel.fuel_building = Some(Vec::new());
        return;
    }
    if text.starts_with("FS_EVENT:") {
        match fuel_stations::parse_event(text) {
            Ok(data) => dispatch(Box::new(move || fuel_on_event(&data))),
            Err(e) if e.kind == PyErrKind::ValueError => {
                log::warn!("Malformed fuel station event: {}", e.msg)
            }
            Err(e) => panic!("{}: {}", e.kind.name(), e.msg),
        }
        return;
    }
    if text.starts_with("FS:") {
        if state().fuel.fuel_building.is_none() {
            return;
        }
        match fuel_stations::parse_row(text) {
            Ok(row) => {
                if let Some(b) = state().fuel.fuel_building.as_mut() {
                    b.push(row_to_json(&row));
                }
            }
            Err(e) if e.kind == PyErrKind::ValueError => {
                log::warn!("Malformed fuel station row: {}", e.msg)
            }
            Err(e) => panic!("{}: {}", e.kind.name(), e.msg),
        }
        return;
    }
    if text == "FS_END" {
        let landed = {
            let mut st = state();
            match st.fuel.fuel_building.take() {
                Some(rows) => {
                    st.fuel.fuel_rows = rows;
                    true
                }
                None => false,
            }
        };
        if landed {
            dispatch(Box::new(fuel_open_browser));
        }
    }
}

fn command_dispatch(f: Box<dyn FnOnce() + Send + 'static>) {
    crate::keyboard::command_dispatch(f);
}

/// `fuel_station_listener(stop_event)`. Runs until `stop_event` is set.
///
/// Rows are swapped in whole on FS_END, like the environment browser's, so a browser
/// never sees a torn list. Events are spoken from the command worker, never from here.
pub fn fuel_station_listener(stop_event: &Event) {
    let mut sock = match UdpListener::bind(FUEL_LISTEN_PORT, 0.2, 4096) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Fuel station listener could not bind port {FUEL_LISTEN_PORT}: {e}");
            log::info!("Fuel station listener stopped.");
            return;
        }
    };
    log::info!("Fuel station listener started on port {FUEL_LISTEN_PORT}");
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Fuel station listener socket error.");
            }
            Recv::Data(data, _addr) => {
                let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                let text = py_strip(&text);
                if text.is_empty() {
                    continue;
                }
                handle_fuel_text(text, &command_dispatch);
            }
        }
    }
    log::info!("Fuel station listener stopped.");
}
