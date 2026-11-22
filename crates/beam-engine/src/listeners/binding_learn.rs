//! Owner: E4. Learn Bindings Mode: sender, toggle, keepalive, phrase, listener (`beamtel.py:7370-7542`).

use beam_base::pyfmt::py_strip;
use serde_json::{Map, Value, json};

use crate::convert::pyval_to_json;
use crate::listeners::scanner::e2_send;
use crate::net::{
    BINDING_LEARN_CMD_PORT, BINDING_LEARN_LISTEN_PORT, Decode, Recv, UdpListener, decode,
};
use crate::speech_out::say;
use crate::state::state;
use crate::timer::Event;

pub const BINDING_LEARN_KEEPALIVE_S: f64 = 1.0;

/// `_send_binding_learn_cmd(command)` (port BINDING_LEARN_CMD_PORT).
pub fn send_binding_learn_cmd(command: &str) {
    e2_send(
        BINDING_LEARN_CMD_PORT,
        command,
        Some("binding learn command"),
    );
}

/// `toggle_binding_learn_mode()` (F9 Shift+B).
///
/// Lua owns the authoritative state, and the confirmation is spoken from the LEARNMODE:
/// reply rather than from this request: entering the mode can fail on the Lua side, and
/// announcing "learn bindings on" here would claim a mode that is not running. If the mod
/// never answers, nothing is said and the log carries it.
pub fn toggle_binding_learn_mode() {
    let active = {
        let mut st = state();
        st.bindings.binding_learn_active = !st.bindings.binding_learn_active;
        st.bindings.binding_learn_active
    };
    send_binding_learn_cmd(if active { "LEARN_ON" } else { "LEARN_OFF" });
}

/// `binding_learn_keepalive_thread_fn(stop_event)`. Runs until `stop_event` is set.
///
/// This thread IS the safety property: with the mode on every binding points at the mod,
/// and if beamtel dies the mod tears the mode down after its heartbeat timeout, so simply
/// not sending is the recovery.
pub fn binding_learn_keepalive_thread_fn(stop_event: &Event) {
    while !stop_event.is_set() {
        stop_event.wait_secs(BINDING_LEARN_KEEPALIVE_S);
        if stop_event.is_set() {
            break;
        }
        if state().bindings.binding_learn_active {
            send_binding_learn_cmd("KEEPALIVE");
        }
    }
}

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// `(d.get(key) or "").strip()`: `Err` (AttributeError) for a truthy non-string.
fn stripped(d: &Map<String, Value>, key: &str) -> Result<String, String> {
    match d.get(key) {
        Some(v) if truthy(v) => match v.as_str() {
            Some(s) => Ok(py_strip(s).to_string()),
            None => Err("AttributeError: object has no attribute 'strip'".to_string()),
        },
        _ => Ok(String::new()),
    }
}

/// `a or b` over dict lookups.
fn or_get(d: &Map<String, Value>, keys: &[&str], last: Value) -> Value {
    for k in keys {
        if let Some(v) = d.get(*k)
            && truthy(v)
        {
            return v.clone();
        }
    }
    last
}

/// `_binding_learn_phrase(row)` with its exceptions as `Err`.
#[doc(hidden)]
pub fn try_binding_learn_phrase(
    row: &Map<String, Value>,
    describe: bool,
) -> Result<String, String> {
    let control = stripped(row, "control")?;

    // Holding a modifier over a control that carries no binding for it falls through in
    // the engine to the bare binding; the mod drops that and marks the report, because
    // naming the bare binding is a confident wrong answer. Reported rather than dropped:
    // silence is indistinguishable from the mode being broken.
    if row.get("unbound").is_some_and(truthy) {
        return Ok(if control.is_empty() {
            "Nothing bound".to_string()
        } else {
            format!("{control}. Nothing bound")
        });
    }

    // A press is reported as a GROUP: one control routinely carries several bindings,
    // and sent one at a time each announcement interrupted the last. `items` is optional
    // on the wire (the two halves go out of step).
    let items: Vec<Value> = match row.get("items") {
        Some(Value::Array(a)) if !a.is_empty() => a.clone(),
        _ => vec![json!({
            "title": or_get(row, &["title", "action"], json!("")),
            "desc": or_get(row, &["desc"], json!("")),
            "suppressed": row.get("suppressed").cloned().unwrap_or(json!(1)),
        })],
    };

    let is_axis = row.get("kind").and_then(Value::as_str) == Some("axis");
    let mut parts: Vec<String> = Vec::new();
    if !control.is_empty() {
        parts.push(control + if is_axis { ", axis" } else { "" });
    } else if is_axis {
        parts.push("axis".to_string());
    }
    if items.len() > 1 {
        parts.push(format!("{} actions", items.len()));
    }

    for item in &items {
        let Some(item) = item.as_object() else {
            continue;
        };
        let raw_title = stripped(item, "title")?;
        let mut title = if raw_title.is_empty() {
            "unnamed action".to_string()
        } else {
            raw_title.clone()
        };
        // An exempt binding fired as well as being named. Saying so stops "it did
        // something anyway" reading as a bug.
        if !item.get("suppressed").is_some_and(truthy) {
            title += ", still active";
        }
        if describe {
            let desc = stripped(item, "desc")?;
            if !desc.is_empty() && desc != raw_title {
                title += &format!(". {desc}");
            }
        }
        parts.push(title);
    }

    Ok(parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(". "))
}

/// `_binding_learn_phrase(row)`: one learn-mode press as a spoken line, control first.
/// Panics where the Python raises (a non-string field).
pub fn binding_learn_phrase(row: &serde_json::Value) -> String {
    let describe = state().cfg.announce_binding_learn_description;
    let empty = Map::new();
    let map = row.as_object().unwrap_or(&empty);
    match try_binding_learn_phrase(map, describe) {
        Ok(s) => s,
        Err(e) => panic!("{e}"),
    }
}

/// One decoded, stripped, non-empty datagram from bindingLearn.lua. `Err` is an exception
/// the Python listener does not catch: the listener ends.
#[doc(hidden)]
pub fn handle_binding_learn_text(text: &str) -> Result<(), String> {
    if let Some(rest) = text.strip_prefix("LEARN:") {
        // JSON rather than a positional tail: the description is free text with commas
        // and quotes in it.
        let row = match beam_base::pyval::json_loads(rest) {
            Ok(v) => pyval_to_json(&v),
            Err(e) => {
                log::warn!("Malformed learn packet: {e}");
                return Ok(());
            }
        };
        let Some(map) = row.as_object() else {
            return Ok(());
        };
        let describe = state().cfg.announce_binding_learn_description;
        let phrase = try_binding_learn_phrase(map, describe)?;
        // interrupt=True: presses come as fast as the user can make them, and the answer
        // they want is about the button under their thumb now.
        say(&phrase, true, false, "python");
        return Ok(());
    }
    if let Some(body) = text.strip_prefix("LEARNMODE:") {
        let (st_text, reason) = body.split_once(';').unwrap_or((body, ""));
        let on = py_strip(st_text).to_uppercase() == "ON";
        state().bindings.binding_learn_active = on;
        log::info!(
            "Learn bindings mode {}: {reason}",
            if on { "on" } else { "off" }
        );
        say(
            &format!("Learn bindings {}", if on { "on" } else { "off" }),
            true,
            true,
            "python",
        );
        return Ok(());
    }
    if let Some(body) = text.strip_prefix("LEARNFAIL:") {
        let (code, sentence) = body.split_once(';').unwrap_or((body, ""));
        log::error!("Learn bindings failed ({code}): {sentence}");
        // Every current code means the mode is not running; leaving the flag set would
        // spend the next Shift+B turning off a mode that was never on.
        state().bindings.binding_learn_active = false;
        // A failure here is never silent.
        say(
            if sentence.is_empty() {
                "Learn bindings failed"
            } else {
                sentence
            },
            true,
            true,
            "python",
        );
    }
    Ok(())
}

/// `binding_learn_listener(stop_event)`. Runs until `stop_event` is set.
pub fn binding_learn_listener(stop_event: &Event) {
    let mut sock = match UdpListener::bind(BINDING_LEARN_LISTEN_PORT, 0.2, 4096) {
        Ok(s) => s,
        Err(e) => {
            log::error!(
                "Binding learn listener could not bind port {BINDING_LEARN_LISTEN_PORT}: {e}"
            );
            log::info!("Binding learn listener stopped.");
            return;
        }
    };
    log::info!("Binding learn listener started on port {BINDING_LEARN_LISTEN_PORT}");
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop_event.is_set() {
                    break;
                }
                log::error!("Binding learn listener socket error.");
            }
            Recv::Data(data, _addr) => {
                let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                let text = py_strip(&text);
                if text.is_empty() {
                    continue;
                }
                if let Err(e) = handle_binding_learn_text(text) {
                    log::error!("Binding learn listener ended: {e}");
                    break;
                }
            }
        }
    }
    log::info!("Binding learn listener stopped.");
}
