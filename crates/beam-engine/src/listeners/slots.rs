//! Owner: E4. Vehicle slot listener (`beamtel.py:4101-4145`); writes `state::slots()`.

use beam_base::pyfmt::py_strip;
use beam_base::pyval::py_int_parse;
use indexmap::IndexMap;

use crate::net::{Decode, Recv, SLOT_LISTEN_PORT, UdpListener, decode};
use crate::state::{SlotInfo, slots};
use crate::timer::Event;

/// One datagram. `Err` where the Python raises out of its loop: the strict UTF-8 decode
/// is not inside any `except`, so one bad byte ends the slot listener for the session.
#[doc(hidden)]
pub fn handle_slot_datagram(data: &[u8]) -> Result<(), String> {
    let Some(text) = decode(data, Decode::Utf8Strict) else {
        return Err("UnicodeDecodeError: 'utf-8' codec can't decode".to_string());
    };
    let text = py_strip(&text);
    let Some(payload) = text.strip_prefix("SLOTS:") else {
        return Ok(());
    };
    let mut new_slots: IndexMap<i64, SlotInfo> = IndexMap::new();
    if !payload.is_empty() {
        for part in payload.split('|') {
            let pieces: Vec<&str> = part.splitn(3, ',').collect();
            if pieces.len() == 3 {
                let slot = py_int_parse(pieces[0]).and_then(|v| v.to_i64());
                let vid = py_int_parse(pieces[1]).and_then(|v| v.to_i64());
                if let (Some(slot), Some(vid)) = (slot, vid) {
                    new_slots.insert(
                        slot,
                        SlotInfo {
                            id: vid,
                            name: pieces[2].to_string(),
                        },
                    );
                }
            }
        }
    }
    let mut s = slots();
    // Purge selections/target for slots that no longer exist.
    s.selected_slots.retain(|k| new_slots.contains_key(k));
    if let Some(t) = s.target_slot
        && !new_slots.contains_key(&t)
    {
        s.target_slot = None;
    }
    s.vehicle_slots = new_slots;
    Ok(())
}

/// `slot_listener(stop_event)`. Runs until `stop_event` is set.
pub fn slot_listener(stop_event: &Event) {
    let mut sock = match UdpListener::bind(SLOT_LISTEN_PORT, 0.2, 4096) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Vehicle slot listener could not bind port {SLOT_LISTEN_PORT}: {e}");
            log::info!("Slot listener stopped.");
            return;
        }
    };
    log::info!("Vehicle slot listener started on port {SLOT_LISTEN_PORT}");
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Slot listener socket error.");
            }
            Recv::Data(data, _addr) => {
                if let Err(e) = handle_slot_datagram(data) {
                    log::error!("Slot listener ended: {e}");
                    break;
                }
            }
        }
    }
    log::info!("Slot listener stopped.");
}
