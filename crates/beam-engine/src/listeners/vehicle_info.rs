//! Owner: E4. Vehicle information listener and sender (`beamtel.py:4012-4098, 7594-7600`).
//!
//! Silent by construction: rows are only ever spoken because the user asked for them
//! (F9 then SPACE on the stock selector, `i` in the spawner).
//!
//! The reply is chunked, so a whole answer is accumulated and only published on
//! INFO_END: a browser opened against a half-arrived list would read a truncated spec
//! sheet as though it were the whole thing. INFOFAIL publishes a reason instead, and both
//! paths set the event so a waiting caller is never left sitting out its full timeout.

use beam_base::pyfmt::py_strip;

use crate::convert::pyval_to_json;
use crate::listeners::scanner::e2_send;
use crate::net::{
    Decode, Recv, UdpListener, VEHICLE_INFO_CMD_PORT, VEHICLE_INFO_LISTEN_PORT, decode,
};
use crate::state::{VINFO_EVENT, state};
use crate::timer::Event;

/// One decoded, stripped, non-empty datagram from vehicleInfo.lua.
#[doc(hidden)]
pub fn handle_vehicle_info_text(text: &str) {
    // Any packet at all proves the mod half is alive and speaking this protocol, which is
    // the only evidence needed to drop the latch.
    state().vinfo.vinfo_absent = false;

    if text.starts_with("INFO_BEGIN:") {
        {
            let mut st = state();
            st.vinfo.vinfo_building = Some(Vec::new());
            st.vinfo.vinfo_error = None;
        }
        VINFO_EVENT.clear();
        return;
    }
    if let Some(rest) = text.strip_prefix("INFO_ROW:") {
        let mut st = state();
        let Some(building) = st.vinfo.vinfo_building.as_mut() else {
            return;
        };
        match beam_base::pyval::json_loads(rest) {
            Ok(v) => building.push(pyval_to_json(&v)),
            Err(_) => log::warn!("Malformed INFO_ROW dropped."),
        }
        return;
    }
    if text == "INFO_END" {
        {
            let mut st = state();
            if let Some(rows) = st.vinfo.vinfo_building.take() {
                st.vinfo.vinfo_rows = rows;
                st.vinfo.vinfo_error = None;
            }
        }
        VINFO_EVENT.set();
        return;
    }
    if let Some(rest) = text.strip_prefix("INFOFAIL:") {
        // "<code>;<sentence>". The code exists because one cause, notselector, is the
        // answer on every screen in the game, including the one where F9 SPACE means
        // "scan the terrain"; it has to fall through silently while the others are
        // spoken. Prose cannot carry that distinction.
        let body = py_strip(rest);
        let (code, sentence) = match body.split_once(';') {
            Some((c, s)) => (c, s),
            // An older mod half sent the sentence alone. Treat it as a real refusal
            // rather than guessing it was notselector, which would silently swallow every
            // failure.
            None => ("unknown", body),
        };
        {
            let mut st = state();
            st.vinfo.vinfo_rows = Vec::new();
            st.vinfo.vinfo_building = None;
            st.vinfo.vinfo_error =
                Some((py_strip(code).to_string(), py_strip(sentence).to_string()));
        }
        VINFO_EVENT.set();
    }
}

/// `vehicle_info_listener(stop_event)`. Runs until `stop_event` is set.
pub fn vehicle_info_listener(stop_event: &Event) {
    let mut sock = match UdpListener::bind(VEHICLE_INFO_LISTEN_PORT, 0.2, 8192) {
        Ok(s) => s,
        Err(e) => {
            log::error!(
                "Vehicle info listener could not bind port {VEHICLE_INFO_LISTEN_PORT}: {e}"
            );
            log::info!("Vehicle info listener stopped.");
            return;
        }
    };
    log::info!("Vehicle info listener started on port {VEHICLE_INFO_LISTEN_PORT}");
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Vehicle info listener socket error.");
            }
            Recv::Data(data, _addr) => {
                let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                let text = py_strip(&text);
                if !text.is_empty() {
                    handle_vehicle_info_text(text);
                }
            }
        }
    }
    log::info!("Vehicle info listener stopped.");
}

/// `_send_vehicle_info_cmd(command)` (port VEHICLE_INFO_CMD_PORT).
pub fn send_vehicle_info_cmd(command: &str) {
    e2_send(VEHICLE_INFO_CMD_PORT, command, Some("vehicle info command"));
}
