//! Owner: E4. Environment listener and its row parsing (`beamtel.py:3860-3948, 7573-7591, 7772-7776`).

use std::panic::{AssertUnwindSafe, catch_unwind};

use beam_base::pyfmt::{py_float_parse, py_strip};

use crate::net::{Decode, ENV_LISTEN_PORT, Recv, UdpListener, decode};
use crate::speech_out::say;
use crate::state::{EnvRow, state};
use crate::timer::Event;

/// One decoded, stripped, non-empty datagram from environmentAccessible.lua. `Err` is an
/// exception the Python listener does not catch (from the browser refresh): it ends.
#[doc(hidden)]
pub fn handle_environment_text(text: &str) -> Result<(), String> {
    if let Some(rest) = text.strip_prefix("ENV_BEGIN:") {
        // ENV_BEGIN:<count>;level=<id>;canChange=<0|1>
        let head = parse_env_fields(rest);
        let mut st = state();
        st.env.env_building = Some(Vec::new());
        st.env.env_unavailable = String::new();
        st.env.env_level = head.get("level").cloned().unwrap_or_default();
        st.env.env_can_change = head.get("canChange").map(String::as_str).unwrap_or("1") == "1";
        return Ok(());
    }
    if let Some(rest) = text.strip_prefix("ENV:") {
        let mut st = state();
        let Some(building) = st.env.env_building.as_mut() else {
            return Ok(());
        };
        let row = parse_env_fields(rest);
        if row.get("key").is_some_and(|k| !k.is_empty()) {
            building.push(row);
        }
        return Ok(());
    }
    if let Some(rest) = text.strip_prefix("AMBIENT:") {
        // AMBIENT:valueC=<c>, or bare AMBIENT: when the level is gone.
        let fields = parse_env_fields(rest);
        state().env.env_ambient_c = env_float(&fields, "valueC");
        return Ok(());
    }
    if text == "ENV_END" {
        // Swapped in whole, so a browser opened mid-push never sees a torn list.
        let landed = {
            let mut st = state();
            match st.env.env_building.take() {
                Some(rows) => {
                    st.env.env_rows = rows;
                    let mut ambient = st.env.env_ambient_c;
                    for row in &st.env.env_rows {
                        if row.get("key").map(String::as_str) == Some("temperature")
                            && let Some(live) = env_float(row, "value")
                        {
                            ambient = Some(live);
                        }
                    }
                    st.env.env_ambient_c = ambient;
                    true
                }
                None => false,
            }
        };
        if landed
            && let Err(p) = catch_unwind(AssertUnwindSafe(crate::browsers::env_notify_refresh))
        {
            return Err(crate::keyboard::panic_text(&*p));
        }
        return Ok(());
    }
    if let Some(rest) = text.strip_prefix("ENV_UNAVAILABLE:") {
        let mut st = state();
        st.env.env_rows = Vec::new();
        st.env.env_building = None;
        st.env.env_ambient_c = None;
        st.env.env_unavailable = py_strip(rest).to_string();
        return Ok(());
    }
    if let Some(rest) = text.strip_prefix("ENV_ERROR:") {
        // A refusal is the one environment message that speaks on its own: it is always
        // the direct answer to a key the user just pressed, and staying quiet would read
        // as the key doing nothing.
        let reason = py_strip(rest);
        log::warn!("Environment change refused: {reason}");
        say(
            if reason.is_empty() {
                "Environment change refused"
            } else {
                reason
            },
            true,
            true,
            "python",
        );
    }
    Ok(())
}

/// `environment_listener(stop_event)`. Runs until `stop_event` is set.
///
/// Silent like the bindings listener: rows are only spoken when the user opens the
/// browser (F9 then N) or edits a value from inside it.
pub fn environment_listener(stop_event: &Event) {
    let mut sock = match UdpListener::bind(ENV_LISTEN_PORT, 0.2, 2048) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Environment listener could not bind port {ENV_LISTEN_PORT}: {e}");
            log::info!("Environment listener stopped.");
            return;
        }
    };
    log::info!("Environment listener started on port {ENV_LISTEN_PORT}");
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Environment listener socket error.");
            }
            Recv::Data(data, _addr) => {
                let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                let text = py_strip(&text);
                if text.is_empty() {
                    continue;
                }
                if let Err(e) = handle_environment_text(text) {
                    log::error!("Environment listener ended: {e}");
                    break;
                }
            }
        }
    }
    log::info!("Environment listener stopped.");
}

/// `_parse_env_fields(payload)`: the mod's `k=v;k=v` row body.
///
/// Split on the FIRST '=' per field, and keep unknown keys: the Lua side sanitises ';'
/// and '=' out of every value so this stays a two-line parser, and a field this build
/// does not know about must be ignored rather than shifting anything after it.
pub fn parse_env_fields(payload: &str) -> EnvRow {
    let mut out = EnvRow::new();
    for field in payload.split(';') {
        if field.is_empty() {
            continue;
        }
        if let Some((key, value)) = field.split_once('=') {
            out.insert(py_strip(key).to_string(), py_strip(value).to_string());
        } else if !out.contains_key("count") {
            // The bare leading number on ENV_BEGIN.
            out.insert("count".to_string(), py_strip(field).to_string());
        }
    }
    out
}

/// `_env_float(row, key)`: `float(row.get(key, ""))`, `None` where that raises.
pub fn env_float(row: &EnvRow, key: &str) -> Option<f64> {
    py_float_parse(row.get(key).map(String::as_str).unwrap_or(""))
}
