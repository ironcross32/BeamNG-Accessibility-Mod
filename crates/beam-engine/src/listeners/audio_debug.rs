//! Owner: E4. Audio debug listener (`beamtel.py:3951-4009`).
//!
//! Both row sets are swapped in whole on their END line, then handed to the command
//! worker, which is the only thread that opens or rewrites the browser.

use beam_base::pyfmt::py_strip;
use beam_reports::audio_debug::{self, Row};
use serde_json::{Map, Value};

use crate::net::{AUDIO_DEBUG_LISTEN_PORT, Decode, Recv, UdpListener, decode};
use crate::state::state;
use crate::timer::Event;

/// A parsed row as kept in `state().aud` (an object of strings, wire order).
pub fn row_to_json(row: &Row) -> Value {
    let mut m = Map::new();
    for (k, v) in row {
        m.insert(k.clone(), Value::String(v.clone()));
    }
    Value::Object(m)
}

/// The row back from its JSON. Only string values exist (every row comes from
/// `parse_fields`).
pub fn row_from_json(v: &Value) -> Row {
    let mut row = Row::new();
    if let Some(m) = v.as_object() {
        for (k, val) in m {
            if let Some(s) = val.as_str() {
                row.insert(k.clone(), s.to_string());
            }
        }
    }
    row
}

type Job = Box<dyn FnOnce() + Send + 'static>;

/// One decoded, stripped, non-empty datagram. `dispatch` is `_command_dispatch`.
#[doc(hidden)]
pub fn handle_audio_debug_text(text: &str, dispatch: &dyn Fn(Job)) {
    if text.starts_with("AUD_BEGIN:") {
        state().aud.aud_building = Some(Vec::new());
    } else if let Some(rest) = text.strip_prefix("AUD:") {
        let mut st = state();
        if let Some(b) = st.aud.aud_building.as_mut() {
            b.push(row_to_json(&audio_debug::parse_fields(rest)));
        }
    } else if text == "AUD_END" {
        let landed = {
            let mut st = state();
            match st.aud.aud_building.take() {
                Some(rows) => {
                    st.aud.aud_rows = rows;
                    true
                }
                None => false,
            }
        };
        if landed {
            dispatch(Box::new(crate::browsers::aud_on_rows));
        }
    } else if let Some(rest) = text.strip_prefix("AUD_SFX_BEGIN:") {
        let mut st = state();
        st.aud.aud_sfx_building = Some(Vec::new());
        st.aud.aud_sfx_head = row_to_json(&audio_debug::parse_fields(rest));
    } else if let Some(rest) = text.strip_prefix("AUD_SFX:") {
        let mut st = state();
        if let Some(b) = st.aud.aud_sfx_building.as_mut() {
            b.push(row_to_json(&audio_debug::parse_fields(rest)));
        }
    } else if text == "AUD_SFX_END" {
        let landed = {
            let mut st = state();
            match st.aud.aud_sfx_building.take() {
                Some(rows) => {
                    st.aud.aud_sfx_rows = rows;
                    true
                }
                None => false,
            }
        };
        if landed {
            dispatch(Box::new(crate::browsers::aud_on_sfx_rows));
        }
    } else if let Some(rest) = text.strip_prefix("AUD_ERROR:") {
        let msg = py_strip(rest).to_string();
        dispatch(Box::new(move || {
            crate::browsers::aud_on_message(&msg, true)
        }));
    } else if let Some(rest) = text.strip_prefix("AUD_DONE:") {
        let msg = py_strip(rest).to_string();
        dispatch(Box::new(move || {
            crate::browsers::aud_on_message(&msg, false)
        }));
    }
}

fn command_dispatch(f: Job) {
    crate::keyboard::command_dispatch(f);
}

/// `audio_debug_listener(stop_event)`. Runs until `stop_event` is set.
pub fn audio_debug_listener(stop_event: &Event) {
    let mut sock = match UdpListener::bind(AUDIO_DEBUG_LISTEN_PORT, 0.2, 4096) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Audio debug listener could not bind port {AUDIO_DEBUG_LISTEN_PORT}: {e}");
            log::info!("Audio debug listener stopped.");
            return;
        }
    };
    log::info!("Audio debug listener started on port {AUDIO_DEBUG_LISTEN_PORT}");
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Audio debug listener socket error.");
            }
            Recv::Data(data, _addr) => {
                let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                let text = py_strip(&text);
                if !text.is_empty() {
                    handle_audio_debug_text(text, &command_dispatch);
                }
            }
        }
    }
    log::info!("Audio debug listener stopped.");
}
