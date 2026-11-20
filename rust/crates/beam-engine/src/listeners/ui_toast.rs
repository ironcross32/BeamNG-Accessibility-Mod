//! Owner: E2. UI toast listener, port 4579 (`ui_listener`, `beamtel.py:738-793`).

use beam_base::pyfmt::py_strip;
use beam_road::assistant_route::suppressed_toast;
use beam_road::pyjson::{self, Value as PyJson};

use crate::convert::pyjson_to_json;
use crate::net::{Decode, Recv, UI_PORT, UdpListener, decode};
use crate::speech_out::{normalize_speech_value, say};
use crate::state::driving_assistant;
use crate::timer::Event;

const PREFIX: &str = "BEAMTEL_UI ";

/// A non-finite float or an int beyond `serde_json` as the `str()` Python would speak,
/// so the conversion below cannot turn them into `null` or a rounded float.
fn keep_python_text(value: &PyJson) -> PyJson {
    match value {
        PyJson::Float(f) if !f.is_finite() => PyJson::Str(value.py_str()),
        PyJson::BigInt(t) => PyJson::Str(t.clone()),
        PyJson::List(items) => PyJson::List(items.iter().map(keep_python_text).collect()),
        PyJson::Dict(d) => PyJson::Dict(
            d.iter()
                .map(|(k, v)| (k.clone(), keep_python_text(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `_normalize_speech_value(value, source)` for a decoded beam-road value.
fn normalize(value: &PyJson, source: &str) -> Option<String> {
    normalize_speech_value(&pyjson_to_json(&keep_python_text(value)), source)
}

/// `x or ""`, normalised and stripped (`None` becomes "").
fn field_text(d: &PyJson, key: &str, source: &str) -> String {
    let empty = PyJson::Str(String::new());
    let value = d.get(key).filter(|v| v.truthy()).unwrap_or(&empty);
    normalize(value, source)
        .map(|t| py_strip(&t).to_string())
        .unwrap_or_default()
}

/// The text a `BEAMTEL_UI` datagram asks to speak and its `kind` (as `str(kind)`), before
/// the driving assistant's toast filter. `None` for anything that is not one.
#[doc(hidden)]
pub fn ui_toast_text(line: &str) -> Option<(String, String)> {
    let body = line.strip_prefix(PREFIX)?;
    let payload = pyjson::loads(body).ok()?;
    if !matches!(payload, PyJson::Dict(_)) {
        normalize(&payload, "ui_udp.payload");
        return None;
    }
    let kind = payload.get("kind").cloned().unwrap_or(PyJson::Null);
    let kind_text = kind.py_str();
    let d = match payload.get("data").filter(|v| v.truthy()) {
        Some(d) => d.clone(),
        None => PyJson::Dict(Default::default()),
    };
    if !matches!(d, PyJson::Dict(_)) {
        normalize(&d, &format!("ui_udp.{kind_text}.data"));
        return None;
    }
    let text = if kind.eq_str("toastr") {
        let title = field_text(&d, "title", "ui_udp.toastr.title");
        let msg = field_text(&d, "msg", "ui_udp.toastr.msg");
        let joined = py_strip(&format!("{title} {msg}")).to_string();
        if !joined.is_empty() {
            joined
        } else if !msg.is_empty() {
            msg
        } else {
            title
        }
    } else if kind.eq_str("message") {
        field_text(&d, "msg", "ui_udp.message.msg")
    } else {
        String::new()
    };
    Some((kind_text, text))
}

/// One datagram from 4579.
#[doc(hidden)]
pub fn handle_ui_datagram(data: &[u8]) {
    let line = decode(data, Decode::Utf8Ignore).unwrap_or_default();
    let Some((kind, text)) = ui_toast_text(&line) else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let da = driving_assistant();
    if suppressed_toast(
        &text,
        da.snapshot().following_route,
        da.seconds_since_arrival(),
    ) {
        return;
    }
    say(&text, true, false, &format!("ui_udp.{kind}"));
}

/// `ui_listener(stop_event)`. Runs until `stop_event` is set.
///
/// Any socket error other than a timeout ends the thread, as Python's
/// `except OSError: break` does.
pub fn ui_listener(stop_event: &Event) {
    let mut sock = match UdpListener::bind(UI_PORT, 0.2, 8192) {
        Ok(s) => s,
        Err(e) => {
            log::error!("UI listener could not bind port {UI_PORT}: {e}");
            return;
        }
    };
    while !stop_event.is_set() {
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(_) => break,
            Recv::Data(data, _) => handle_ui_datagram(data),
        }
    }
}
