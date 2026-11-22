//! Owner: E4. Vehicle-specific bindings listener and sender (`beamtel.py:3787-3857, 7359-7367`).

use beam_base::pyfmt::py_strip;
use beam_base::pyval::py_int_parse;

use crate::listeners::scanner::e2_send;
use crate::net::{
    Decode, Recv, UdpListener, VEHICLE_BINDINGS_CMD_PORT, VEHICLE_BINDINGS_LISTEN_PORT, decode,
};
use crate::speech_out::py_repr_str;
use crate::state::state;
use crate::timer::Event;

/// One decoded, stripped, non-empty datagram from vehicleBindings.lua.
#[doc(hidden)]
pub fn handle_vehicle_bindings_text(text: &str) {
    if let Some(rest) = text.strip_prefix("BINDINGS_BEGIN:") {
        // BINDINGS_BEGIN:<count>,<vehicleName>
        let parts: Vec<&str> = rest.splitn(2, ',').collect();
        let mut st = state();
        st.bindings.vehicle_bindings_building = Some(Vec::new());
        st.bindings.vehicle_bindings_building_name = if parts.len() == 2 {
            py_strip(parts[1]).to_string()
        } else {
            String::new()
        };
        return;
    }
    if let Some(rest) = text.strip_prefix("BINDING:") {
        // BINDING:<cacheIndex>,<line>  -- line may contain commas
        let parts: Vec<&str> = rest.splitn(2, ',').collect();
        if parts.len() == 2 {
            let mut st = state();
            if let Some(b) = st.bindings.vehicle_bindings_building.as_mut()
                && let Some(idx) = py_int_parse(parts[0]).and_then(|v| v.to_i64())
            {
                b.push((idx, parts[1].to_string()));
            }
        }
        return;
    }
    if text == "BINDINGS_END" {
        // Swap in whole, so a browser opened mid-rebuild never sees a torn list.
        let mut st = state();
        if let Some(list) = st.bindings.vehicle_bindings_building.take() {
            st.bindings.vehicle_bindings_list = list;
            st.bindings.vehicle_bindings_vehicle =
                st.bindings.vehicle_bindings_building_name.clone();
            log::info!(
                "Vehicle bindings updated: {} for {}",
                st.bindings.vehicle_bindings_list.len(),
                py_repr_str(&st.bindings.vehicle_bindings_vehicle)
            );
        }
    }
}

/// `vehicle_bindings_listener(stop_event)`. Runs until `stop_event` is set.
///
/// Deliberately silent: the mod pushes a fresh list on every vehicle load, and announcing
/// that would talk over the spawn. The list is only spoken when the browser is opened.
pub fn vehicle_bindings_listener(stop_event: &Event) {
    let mut sock = match UdpListener::bind(VEHICLE_BINDINGS_LISTEN_PORT, 0.2, 2048) {
        Ok(s) => s,
        Err(e) => {
            log::error!(
                "Vehicle bindings listener could not bind port {VEHICLE_BINDINGS_LISTEN_PORT}: {e}"
            );
            log::info!("Vehicle bindings listener stopped.");
            return;
        }
    };
    log::info!("Vehicle bindings listener started on port {VEHICLE_BINDINGS_LISTEN_PORT}");
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Vehicle bindings listener socket error.");
            }
            Recv::Data(data, _addr) => {
                let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                let text = py_strip(&text);
                if !text.is_empty() {
                    handle_vehicle_bindings_text(text);
                }
            }
        }
    }
    log::info!("Vehicle bindings listener stopped.");
}

/// `_send_vehicle_bindings_cmd(command)` (port VEHICLE_BINDINGS_CMD_PORT).
pub fn send_vehicle_bindings_cmd(command: &str) {
    e2_send(
        VEHICLE_BINDINGS_CMD_PORT,
        command,
        Some("vehicle bindings command"),
    );
}
