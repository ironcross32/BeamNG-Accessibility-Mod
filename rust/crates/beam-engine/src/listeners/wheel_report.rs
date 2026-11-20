//! Owner: E3. Wheel report listener (`beamtel.py:2444-2475`). Labels come from
//! `beam_reports::wheel_report`.

use beam_reports::wheel_report::{self, WheelRow};
use serde_json::{Value, json};

use crate::convert::json_float;
use crate::net::{Decode, Recv, UdpListener, WHEEL_REPORT_LISTEN_PORT, decode};
use crate::state::state;
use crate::timer::{Event, monotonic};

/// One `all_rows()` dict: `{"label", "psi", "brake_c", "broken"}`.
pub fn wheel_row_json(row: &WheelRow) -> Value {
    json!({
        "label": row.label,
        "psi": row.psi.map(json_float).unwrap_or(Value::Null),
        "brake_c": row.brake_c.map(json_float).unwrap_or(Value::Null),
        "broken": row.broken,
    })
}

/// One datagram's decoded text (not stripped: `parse_packet` strips), at `mono`.
pub fn handle_wheel_report_text(text: &str, mono: f64) {
    let Some(vehicles) = wheel_report::parse_packet(text) else {
        return;
    };
    let rows: Vec<Value> = wheel_report::all_rows(Some(&vehicles))
        .iter()
        .map(wheel_row_json)
        .collect();
    let mut st = state();
    st.wheels.last_wheel_rows = rows;
    st.wheels.last_wheel_stamp = mono;
}

/// `wheel_report_listener(stop_event)`. Runs until `stop_event` is set.
pub fn wheel_report_listener(stop_event: &Event) {
    match UdpListener::bind(WHEEL_REPORT_LISTEN_PORT, 0.2, 65535) {
        Ok(mut sock) => {
            log::info!("Wheel report listener started on port {WHEEL_REPORT_LISTEN_PORT}");
            while !stop_event.is_set() {
                match sock.recv() {
                    Recv::Timeout => continue,
                    Recv::Error(_) => {
                        if stop_event.is_set() {
                            break;
                        }
                        log::error!("Wheel report listener socket error.");
                    }
                    Recv::Data(data, _) => {
                        let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                        handle_wheel_report_text(&text, monotonic());
                    }
                }
            }
        }
        Err(e) => log::error!("Wheel report listener failed to bind: {e}"),
    }
    log::info!("Wheel report listener stopped.");
}
