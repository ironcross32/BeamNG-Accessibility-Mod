//! The tool implementations (`_t_*`) other than the camera and the screenshot.

use std::panic::{AssertUnwindSafe, catch_unwind};

use beam_base::pyfmt::{py_round, py_strip};
use beam_base::secretstore;
use beam_road::pyjson::{self, Dict, DumpOpts, PyErr, Value as PyJson, py_max, py_min};

use super::{
    DIAGNOSTICS, Deps, INPUT_LOCK, MODULE_PORTS, STARTED_AT, TAP, TResult, ToolFail, contexts,
    deps, exec_console, exec_ge, require_world,
};
use crate::convert::json_to_pyjson;
use crate::speech_out::SpeechLog;
use crate::timer::{monotonic, sleep};

/// `args.get(key)` on the tool arguments (an `AttributeError` for a non-dict).
pub(super) fn aget<'a>(args: &'a PyJson, key: &str) -> TResult<Option<&'a PyJson>> {
    Ok(args.py_get(key)?)
}

/// `args.get(key, default)`.
pub(super) fn aget_or(args: &PyJson, key: &str, default: PyJson) -> TResult<PyJson> {
    Ok(aget(args, key)?.cloned().unwrap_or(default))
}

/// `args.get(key)` as a value, `None` -> `Null`.
pub(super) fn aget_val(args: &PyJson, key: &str) -> TResult<PyJson> {
    aget_or(args, key, PyJson::Null)
}

/// `(x or default).strip()`.
pub(super) fn strip_or(v: &PyJson, default: &str) -> TResult<String> {
    if !v.truthy() {
        return Ok(default.to_string());
    }
    match v {
        PyJson::Str(s) => Ok(py_strip(s).to_string()),
        other => Err(ToolFail::py(
            "AttributeError",
            format!("'{}' object has no attribute 'strip'", other.type_name()),
        )),
    }
}

fn dict(pairs: Vec<(&str, PyJson)>) -> PyJson {
    pyjson::dict(pairs)
}

/// One `send_udp_fn(port, payload)` call, failing as `make_udp_sender` does for what the
/// typed sender cannot be handed.
pub(super) fn send_udp(d: &Deps, port: i64, payload: &PyJson) -> TResult<()> {
    let text = match payload {
        PyJson::Str(s) => s.as_str(),
        other => {
            return Err(ToolFail::tool(format!(
                "failed to send UDP to port {port}: '{}' object has no attribute 'encode'",
                other.type_name()
            )));
        }
    };
    let Ok(p) = u16::try_from(port) else {
        return Err(ToolFail::tool(format!(
            "failed to send UDP to port {port}: getsockaddrarg: port must be 0-65535."
        )));
    };
    (d.send_udp_fn)(p, text).map_err(ToolFail::Tool)
}

fn send_str(d: &Deps, port: i64, payload: &str) -> TResult<()> {
    send_udp(d, port, &PyJson::Str(payload.to_string()))
}

fn speech_log_json(log: &SpeechLog) -> PyJson {
    serde_json::to_value(log)
        .map(|v| json_to_pyjson(&v))
        .unwrap_or(PyJson::Null)
}

/// Run a dependency the Python wraps in `try: ... except Exception: pass`.
fn quietly<T>(f: impl FnOnce() -> T) -> Option<T> {
    catch_unwind(AssertUnwindSafe(f)).ok()
}

/// `_t_health(args)`.
pub(super) fn t_health(_args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let (console_ok, console_detail) =
        match exec_console(&PyJson::from("ge"), "return 1", &PyJson::Float(2.0), false, true) {
            Ok(res) => (
                res.get("ok").is_some_and(PyJson::truthy)
                    && !res.get("timed_out").is_some_and(PyJson::truthy),
                res.get("result").cloned().unwrap_or(PyJson::from("")),
            ),
            Err(e) => (false, PyJson::Str(e.message().to_string())),
        };
    let mut cfg = Dict::new();
    if let Some(full) = quietly(|| (d.load_config_fn)()) {
        for k in [
            "units",
            "telemetry_protocol",
            "mcp_server_enabled",
            "mcp_server_port",
        ] {
            cfg.insert(
                k.into(),
                full.get(k).map(json_to_pyjson).unwrap_or(PyJson::Null),
            );
        }
    }
    let state = quietly(|| (d.snapshot_state_fn)(Some(&["liveness"])))
        .map(|v| json_to_pyjson(&v))
        .filter(PyJson::truthy)
        .unwrap_or_else(|| PyJson::Dict(Dict::new()));
    let liveness = state
        .py_get("liveness")?
        .cloned()
        .unwrap_or_else(|| PyJson::Dict(Dict::new()));
    let started_at = *STARTED_AT.lock();
    Ok(dict(vec![
        ("world_active", PyJson::Bool((d.world_active_fn)())),
        ("liveness", liveness),
        ("console_responsive", PyJson::Bool(console_ok)),
        ("console_detail", console_detail),
        (
            "mcp_uptime_s",
            PyJson::Float(py_round(monotonic() - started_at, 1)),
        ),
        ("config", PyJson::Dict(cfg)),
        ("version", PyJson::Str(d.version.clone())),
    ]))
}

/// `_t_lua_exec(args)`.
pub(super) fn t_lua_exec(args: &PyJson) -> TResult<PyJson> {
    let code = aget_val(args, "code")?;
    if !code.truthy() {
        return Err(ToolFail::tool("`code` is required"));
    }
    let context = aget_or(args, "context", PyJson::from("ge"))?;
    let timeout = aget_val(args, "timeout_s")?;
    let verbose = aget_val(args, "verbose")?.truthy();
    let suppress = aget_or(args, "suppress_gui", PyJson::Bool(true))?.truthy();
    Ok(PyJson::Dict(exec_console(
        &context,
        &code.py_str(),
        &timeout,
        verbose,
        suppress,
    )?))
}

/// `_t_lua_contexts(args)`.
pub(super) fn t_lua_contexts(args: &PyJson) -> TResult<PyJson> {
    let force = aget_or(args, "refresh", PyJson::Bool(true))?.truthy();
    let ctxs = contexts(force)?;
    Ok(dict(vec![(
        "contexts",
        PyJson::List(
            ctxs.into_iter()
                .map(|(i, label)| dict(vec![("index", PyJson::Int(i)), ("label", PyJson::Str(label))]))
                .collect(),
        ),
    )]))
}

/// `set(sections) if sections else None`, as the names a snapshot can match.
pub fn sections_arg(sections: &PyJson) -> TResult<Option<Vec<String>>> {
    if !sections.truthy() {
        return Ok(None);
    }
    match sections {
        PyJson::List(items) => {
            let mut out = Vec::new();
            for item in items {
                if !item.hashable() {
                    return Err(pyjson::unhashable(item).into());
                }
                if let PyJson::Str(s) = item {
                    out.push(s.clone());
                }
            }
            Ok(Some(out))
        }
        PyJson::Str(s) => Ok(Some(s.chars().map(String::from).collect())),
        PyJson::Dict(d) => Ok(Some(d.keys().cloned().collect())),
        other => Err(ToolFail::py(
            "TypeError",
            format!("'{}' object is not iterable", other.type_name()),
        )),
    }
}

/// `_t_get_state(args)`.
pub(super) fn t_get_state(args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let sections = sections_arg(&aget_val(args, "sections")?)?;
    let names: Option<Vec<&str>> = sections
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect());
    let snap = (d.snapshot_state_fn)(names.as_deref());
    Ok(json_to_pyjson(&snap))
}

/// `_REDACTED`: which settings are secrets is `secretstore`'s list, not a second copy.
pub const REDACTED: &str = "<redacted>";

/// `_mask_secrets(cfg)`: a copy with secret-named values replaced. An empty value stays
/// empty rather than redacted: whether a key is SET is what an agent legitimately needs
/// to know, and it gives away nothing.
pub fn mask_secrets(cfg: &Dict) -> Dict {
    let mut out = Dict::new();
    for (k, v) in cfg {
        let unset = match v {
            PyJson::Null => true,
            PyJson::Str(s) => s.is_empty(),
            PyJson::List(l) => l.is_empty(),
            PyJson::Dict(d) => d.is_empty(),
            _ => false,
        };
        if secretstore::is_secret_setting(k) && !unset {
            out.insert(k.clone(), PyJson::from(REDACTED));
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// `_t_get_config(args)`.
pub(super) fn t_get_config(_args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let cfg = (d.load_config_fn)();
    let cfg = match json_to_pyjson(&serde_json::Value::Object(cfg)) {
        PyJson::Dict(m) => m,
        _ => Dict::new(),
    };
    Ok(PyJson::Dict(mask_secrets(&cfg)))
}

/// `TypeError` for an order comparison between an int and something that is not a number.
fn cmp_error(op: &str, left: &str, right: &str) -> ToolFail {
    ToolFail::py(
        "TypeError",
        format!("'{op}' not supported between instances of '{left}' and '{right}'"),
    )
}

/// A sequence-number cursor as the integer it compares like. `Err` carries the type name
/// of something that does not compare with an int at all.
fn cursor_value(v: &PyJson) -> Result<i64, &'static str> {
    match v {
        PyJson::Bool(b) => Ok(*b as i64),
        PyJson::Int(i) => Ok(*i),
        PyJson::BigInt(t) => Ok(if t.starts_with('-') { i64::MIN } else { i64::MAX }),
        PyJson::Float(f) if f.is_nan() => Ok(i64::MAX),
        PyJson::Float(f) => Ok(f.floor() as i64),
        other => Err(other.type_name()),
    }
}

/// `_t_speech_log(args)`.
pub(super) fn t_speech_log(args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let since = aget_val(args, "since_seq")?;
    let last_n = aget_or(args, "last_n", PyJson::Int(50))?;
    let source = aget_val(args, "source")?;
    let contains = aget_val(args, "contains")?;
    let spoken_only = aget_or(args, "spoken_only", PyJson::Bool(false))?.truthy();

    let since_seq = if since.is_null() {
        None
    } else {
        match cursor_value(&since) {
            Ok(v) => Some(v),
            Err(t) => {
                // Only compared (and so only raised) when the tap holds anything.
                let any = !(d.get_speech_log_fn)(None, None, None, None, false)
                    .entries
                    .is_empty();
                if any {
                    return Err(cmp_error("<", t, "int"));
                }
                None
            }
        }
    };
    let source_filter = if source.truthy() { Some(source.clone()) } else { None };
    let contains = if contains.truthy() {
        match &contains {
            PyJson::Str(s) => Some(s.clone()),
            other => {
                return Err(ToolFail::py(
                    "AttributeError",
                    format!("'{}' object has no attribute 'lower'", other.type_name()),
                ));
            }
        }
    } else {
        None
    };
    let last_n = if last_n.truthy() {
        Some(last_n.py_int()?)
    } else {
        None
    };
    let source_str = source_filter.as_ref().and_then(|s| s.as_str());
    let mut log = (d.get_speech_log_fn)(
        since_seq,
        last_n,
        source_str,
        contains.as_deref(),
        spoken_only,
    );
    // A source that is not a str never equals an entry's.
    if source_filter.is_some() && source_str.is_none() {
        log.entries.clear();
    }
    Ok(speech_log_json(&log))
}

/// `_t_speak(args)`.
pub(super) fn t_speak(args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let text = aget_val(args, "text")?;
    if !text.truthy() {
        return Err(ToolFail::tool("`text` is required"));
    }
    let interrupt = aget_or(args, "interrupt", PyJson::Bool(true))?.truthy();
    // The Python hands the raw value to say(), which normalises it first: a number is
    // spoken as its text, a one-item list or dict as its item, anything else not at all.
    // say_fn takes text, so normalise here rather than stringify (["a"] is "a", not "['a']").
    let value = crate::convert::pyjson_to_json(&text);
    if let Some(t) = crate::speech_out::normalize_speech_value(&value, "mcp") {
        (d.say_fn)(&t, interrupt);
    }
    Ok(dict(vec![("spoken", text)]))
}

/// `_t_list_commands(args)`.
pub(super) fn t_list_commands(_args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let mut out: Vec<(&str, bool, bool, bool, &str)> = d
        .f9_help
        .iter()
        .map(|((name, ctrl, shift, alt), desc)| (*name, *ctrl, *shift, *alt, *desc))
        .collect();
    out.sort_by(|a, b| (a.0, a.1, a.2, a.3).cmp(&(b.0, b.1, b.2, b.3)));
    Ok(dict(vec![(
        "commands",
        PyJson::List(
            out.into_iter()
                .map(|(name, ctrl, shift, alt, desc)| {
                    dict(vec![
                        ("name", PyJson::from(name)),
                        ("ctrl", PyJson::Bool(ctrl)),
                        ("shift", PyJson::Bool(shift)),
                        ("alt", PyJson::Bool(alt)),
                        ("description", PyJson::from(desc)),
                    ])
                })
                .collect(),
        ),
    )]))
}

/// `_t_press_command(args)`.
pub(super) fn t_press_command(args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let name = aget_val(args, "name")?;
    if !name.truthy() {
        return Err(ToolFail::tool("`name` is required (the key pressed after F9)"));
    }
    let settle = aget_or(args, "settle_ms", PyJson::Int(300))?.py_float()? / 1000.0;
    let ctrl = aget_val(args, "ctrl")?.truthy();
    let shift = aget_val(args, "shift")?.truthy();
    let alt = aget_val(args, "alt")?.truthy();
    let _input = INPUT_LOCK.lock();
    let before = (d.get_speech_log_fn)(None, Some(1), None, None, false);
    let cursor = before.next_seq as i64;
    if let Err(e) = (d.press_command_fn)(&name.py_str(), ctrl, shift, alt) {
        return Err(ToolFail::py("RuntimeError", e));
    }
    sleep(py_max(0.0, py_min(settle, 5.0)));
    let after = speech_log_json(&(d.get_speech_log_fn)(Some(cursor), Some(50), None, None, false));
    Ok(dict(vec![
        ("pressed", name),
        (
            "speech",
            after.get("entries").cloned().unwrap_or(PyJson::List(Vec::new())),
        ),
        ("next_seq", after.get("next_seq").cloned().unwrap_or(PyJson::Null)),
    ]))
}

/// `_t_send_command(args)`.
pub(super) fn t_send_command(args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let payload = aget_val(args, "payload")?;
    if !payload.truthy() {
        return Err(ToolFail::tool("`payload` is required"));
    }
    let module = aget_val(args, "module")?;
    let mut port = aget_val(args, "port")?;
    if module.truthy() {
        if !module.hashable() {
            return Err(pyjson::unhashable(&module).into());
        }
        let found = module
            .as_str()
            .and_then(|m| MODULE_PORTS.iter().find(|(n, _)| *n == m));
        let Some((_, p)) = found else {
            let mut names: Vec<&str> = MODULE_PORTS.iter().map(|(n, _)| *n).collect();
            names.sort();
            return Err(ToolFail::tool(format!(
                "unknown module {}; known: {}",
                module.py_repr(),
                names.join(", ")
            )));
        };
        port = PyJson::Int(i64::from(*p));
    }
    if !port.truthy() {
        return Err(ToolFail::tool("one of `module` or `port` is required"));
    }
    let port = port.py_int()?;
    send_udp(&d, port, &payload)?;
    Ok(dict(vec![
        ("sent", payload),
        ("port", PyJson::Int(port)),
        (
            "note",
            PyJson::from("fire-and-forget; verify the effect with get_state / speech_log / diag"),
        ),
    ]))
}

/// `str(i) for i in ids`, iterating however Python would.
fn iter_str(v: &PyJson) -> TResult<Vec<String>> {
    match v {
        PyJson::List(items) => Ok(items.iter().map(PyJson::py_str).collect()),
        PyJson::Str(s) => Ok(s.chars().map(String::from).collect()),
        PyJson::Dict(d) => Ok(d.keys().cloned().collect()),
        other => Err(ToolFail::py(
            "TypeError",
            format!("'{}' object is not iterable", other.type_name()),
        )),
    }
}

/// `json.dumps(x)` with Python's defaults.
pub(super) fn dumps_default(v: &PyJson) -> String {
    pyjson::dumps(v, &DumpOpts::default_seps()).unwrap_or_default()
}

/// `_t_vehicle_control(args)`.
pub(super) fn t_vehicle_control(args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let action = strip_or(&aget_val(args, "action")?, "")?;
    match action.as_str() {
        "list_active" => {
            send_str(&d, 4461, "REQUEST_ACTIVE_VEHICLES")?;
            Ok(dict(vec![
                ("sent", PyJson::from("REQUEST_ACTIVE_VEHICLES")),
                (
                    "note",
                    PyJson::from("read the result with lua_exec or diag player_id"),
                ),
            ]))
        }
        "player_id" => Ok(PyJson::Dict(exec_ge("return be:getPlayerVehicleID(0)")?)),
        "ignition_off" => {
            send_str(&d, 4461, "IGNITION_OFF")?;
            Ok(dict(vec![("sent", PyJson::from("IGNITION_OFF"))]))
        }
        "remove" | "reload" => {
            let ids = aget_val(args, "vehicle_ids")?;
            if !ids.truthy() {
                return Err(ToolFail::tool(format!(
                    "`vehicle_ids` is required for {action}"
                )));
            }
            let verb = if action == "remove" { "REMOVE" } else { "RELOAD" };
            let msg = format!("{verb}:{}", iter_str(&ids)?.join(","));
            send_str(&d, 4461, &msg)?;
            Ok(dict(vec![("sent", PyJson::Str(msg))]))
        }
        "spawn" => {
            let items = aget_val(args, "items")?;
            if !items.truthy() {
                return Err(ToolFail::tool("`items` is required for spawn"));
            }
            let mut body = Dict::new();
            body.insert("items".into(), items);
            let arrangement = aget_val(args, "arrangement")?;
            if arrangement.truthy() {
                body.insert("arrangement".into(), arrangement);
            }
            let msg = format!("SPAWN:{}", dumps_default(&PyJson::Dict(body)));
            send_str(&d, 4461, &msg)?;
            Ok(dict(vec![("sent", PyJson::Str(msg))]))
        }
        "teleport" => {
            let mut body = Dict::new();
            for k in [
                "vehId",
                "refVehId",
                "refMode",
                "offFwdFt",
                "offRightFt",
                "offUpFt",
                "rotYawDeg",
                "rotPitchDeg",
                "rotRollDeg",
                "mode",
            ] {
                if let Some(v) = args.get(k) {
                    body.insert(k.into(), v.clone());
                }
            }
            if !body.contains_key("vehId") {
                return Err(ToolFail::tool("`vehId` is required for teleport"));
            }
            let msg = format!("TELEPORT_PLACE:{}", dumps_default(&PyJson::Dict(body)));
            send_str(&d, 4461, &msg)?;
            Ok(dict(vec![
                ("sent", PyJson::Str(msg)),
                (
                    "note",
                    PyJson::from("the settle report lands on 4460 and is consumed by the spawner"),
                ),
            ]))
        }
        _ => Err(ToolFail::tool(format!(
            "unknown action {} (list_active, player_id, spawn, teleport, remove, reload, ignition_off)",
            pyjson::py_str_repr(&action)
        ))),
    }
}

/// `_t_diag(args)`.
pub(super) fn t_diag(args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let name = strip_or(&aget_val(args, "name")?, "")?;
    let Some((_, expr)) = DIAGNOSTICS.iter().find(|(n, _)| *n == name) else {
        let mut names: Vec<&str> = DIAGNOSTICS.iter().map(|(n, _)| *n).collect();
        names.sort();
        return Err(ToolFail::tool(format!(
            "unknown diagnostic {}; known: {}",
            pyjson::py_str_repr(&name),
            names.join(", ")
        )));
    };
    let mut expr = expr.to_string();
    if expr.contains("{id}") {
        let veh = aget_val(args, "veh_id")?;
        let sub = match &veh {
            PyJson::Null => "nil".to_string(),
            PyJson::Str(s) if s.is_empty() => "nil".to_string(),
            other => other.py_str(),
        };
        expr = expr.replace("{id}", &sub);
    }
    if name == "terrain" {
        // diag() logs rather than returning, so read it out of a LOGON window.
        send_str(&d, 4465, "LOGON")?;
        let res = (|| -> TResult<Vec<String>> {
            let before_seq = TAP.lock().log_seq;
            exec_ge(&expr)?;
            sleep(0.5);
            let ring: Vec<_> = TAP.lock().ring.iter().cloned().collect();
            Ok(ring
                .into_iter()
                .filter(|e| e.seq > before_seq)
                .map(|e| e.raw)
                .collect())
        })();
        send_str(&d, 4465, "LOGOFF")?;
        let lines = res?;
        return Ok(dict(vec![
            ("diagnostic", PyJson::Str(name)),
            (
                "log",
                PyJson::List(lines.into_iter().map(PyJson::Str).collect()),
            ),
        ]));
    }
    let mut res = exec_ge(&format!("return {expr}"))?;
    res.insert("diagnostic".into(), PyJson::Str(name));
    Ok(PyJson::Dict(res))
}

/// `_t_console_log(args)`.
pub(super) fn t_console_log(args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let action = strip_or(&aget_val(args, "action")?, "read")?;
    match action.as_str() {
        "on" => {
            send_str(&d, 4465, "LOGON")?;
            Ok(dict(vec![("log_streaming", PyJson::Bool(true))]))
        }
        "off" => {
            send_str(&d, 4465, "LOGOFF")?;
            Ok(dict(vec![("log_streaming", PyJson::Bool(false))]))
        }
        "read" => {
            let since = aget_val(args, "since_seq")?;
            let mut entries: Vec<_> = TAP.lock().ring.iter().cloned().collect();
            if !since.is_null() && !entries.is_empty() {
                match cursor_value(&since) {
                    Ok(v) => entries.retain(|e| e.seq > v),
                    Err(t) => return Err(cmp_error(">", "int", t)),
                }
            }
            let limit = aget_or(args, "last_n", PyJson::Int(200))?.py_int()?;
            // entries[-limit:]
            let len = entries.len() as i64;
            let start = if limit <= 0 {
                (-limit).min(len)
            } else {
                (len - limit).max(0)
            };
            let entries = entries.split_off(start as usize);
            let next_seq = match entries.last() {
                Some(e) => PyJson::Int(e.seq),
                None if since.truthy() => since.clone(),
                None => PyJson::Int(0),
            };
            Ok(dict(vec![
                (
                    "entries",
                    PyJson::List(
                        entries
                            .into_iter()
                            .map(|e| {
                                dict(vec![
                                    ("seq", PyJson::Int(e.seq)),
                                    ("t", PyJson::Float(e.t)),
                                    ("raw", PyJson::Str(e.raw)),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("next_seq", next_seq),
            ]))
        }
        _ => Err(ToolFail::tool(format!(
            "unknown action {} (on / off / read)",
            pyjson::py_str_repr(&action)
        ))),
    }
}

/// `_t_reset_test_state(args)`.
pub(super) fn t_reset_test_state(_args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    send_str(&d, 4470, "REBUILD")?;
    let mut out = Dict::new();
    out.insert(
        "sent".into(),
        PyJson::List(vec![PyJson::from("REBUILD -> implementProximity")]),
    );
    match exec_ge("return extensions.rampGeometry.retry(nil)") {
        Ok(res) => {
            out.insert(
                "ramp_retry".into(),
                res.get("result").cloned().unwrap_or(PyJson::from("")),
            );
        }
        Err(e) => {
            out.insert("ramp_retry_error".into(), PyJson::Str(e.message().to_string()));
        }
    }
    Ok(PyJson::Dict(out))
}

/// `_t_assistant_diagnostic(args)`.
pub(super) fn t_assistant_diagnostic(args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let action = aget_or(args, "action", PyJson::from("status"))?
        .py_str();
    let action = py_strip(&action).to_lowercase();
    if action == "start" {
        require_world()?;
    }
    let kwargs = pyjson::dict([
        ("label", aget_val(args, "label")?),
        ("session", aget_val(args, "session")?),
        ("note", aget_val(args, "note")?),
        ("limit", aget_or(args, "limit", PyJson::Int(100))?),
        ("since_seq", aget_or(args, "since_seq", PyJson::Int(0))?),
    ]);
    let kwargs = crate::convert::pyjson_to_json(&kwargs);
    match (d.assistant_diagnostic_fn)(&action, &kwargs) {
        Ok(v) => Ok(json_to_pyjson(&v)),
        Err(e) => Err(diag_error(e)),
    }
}

/// The exception a diagnostic callback raised, by its message: the action check is a
/// `ValueError`, everything else the callbacks raise is a `RuntimeError`.
fn diag_error(msg: String) -> ToolFail {
    let t = if msg.starts_with("action must be ") {
        "ValueError"
    } else {
        "RuntimeError"
    };
    ToolFail::py(t, msg)
}

/// `str(x)` for an optional value handed to a typed callback.
fn opt_str(v: &PyJson) -> Option<String> {
    match v {
        PyJson::Null => None,
        other => Some(other.py_str()),
    }
}

/// `_t_road_diagnostic(args)`.
pub(super) fn t_road_diagnostic(args: &PyJson) -> TResult<PyJson> {
    let d = deps()?;
    let action = aget_or(args, "action", PyJson::from("status"))?
        .py_str();
    let action = py_strip(&action).to_lowercase();
    if action == "start" {
        require_world()?;
    }
    let label = opt_str(&aget_val(args, "label")?);
    let session = opt_str(&aget_val(args, "session")?);
    let note = opt_str(&aget_val(args, "note")?);
    let limit_v = aget_or(args, "limit", PyJson::Int(20))?;
    // list_sessions(limit) is the only reader: int(limit) there.
    let normalised = if action.is_empty() { "status" } else { action.as_str() };
    let limit = if normalised == "list" {
        limit_v.py_int().map_err(|e: PyErr| ToolFail::from(e))?
    } else {
        20
    };
    match (d.road_diagnostic_fn)(
        &action,
        label.as_deref(),
        session.as_deref(),
        note.as_deref(),
        limit,
    ) {
        Ok(v) => Ok(json_to_pyjson(&v)),
        Err(e) => Err(diag_error(e)),
    }
}
