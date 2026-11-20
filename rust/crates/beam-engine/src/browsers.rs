//! Owner: E4. The browsers built on listener data (`beamtel.py:7545-7570, 7603-8256`):
//! vehicle bindings, vehicle information and page text (with their bounded waits), the
//! environment browser (edits, timeout, refresh), the forced-induction and ambient
//! temperature readouts, the audio-debug engine and sound-library browsers, and the
//! clickspot browser.

use std::net::UdpSocket;
use std::sync::Arc;

use beam_base::pyfmt::{py_f, py_strip};
use beam_base::pyval::{py_max, py_min};
use beam_reports::audio_debug;
use beam_reports::pyops::PyDict;
use serde_json::{Value, json};

use crate::listeners::audio_debug::row_from_json;
use crate::listeners::environment::env_float;
use crate::listeners::scanner::e2_send;
use crate::listeners::vehicle_bindings::send_vehicle_bindings_cmd;
use crate::listeners::vehicle_info::send_vehicle_info_cmd;
use crate::net::{AUDIO_DEBUG_CMD_PORT, DUMP_RESP_PORT, ENV_CMD_PORT, SCANNER_CMD_PORT};
use crate::speech_out::say;
use crate::state::{EnvRow, OnAdjust, OnEnter, OnEscape, PAGE_TEXT_EVENT, VINFO_EVENT, state};
use crate::timer::{Timer, spawn_daemon};
use crate::units::{PyVal, fmt_pressure, fmt_temp_c_or_f, units_mode};
use crate::virtual_browser::{BrowserOpts, close_virtual_browser, open_virtual_browser};

/// A `(code, sentence)` failure, as `request_vehicle_info` / `request_page_text` return it.
pub type Failure = (String, String);

fn say_excluded(text: &str) {
    say(text, true, true, "python");
}

fn say_plain(text: &str) {
    say(text, true, false, "python");
}

fn plural(n: usize) -> &'static str {
    if n != 1 { "s" } else { "" }
}

/// Entry data as the int the Python stored, `None` for a missing/None entry.
fn data_index(data: &Option<Value>) -> Option<i64> {
    data.as_ref().and_then(Value::as_i64)
}

/// `f"{x}"` of a `fmt_temp_c_or_f` value (always an int).
fn pyval_int(v: &PyVal) -> i64 {
    match v {
        PyVal::Int(n) => *n,
        PyVal::Float(x) => *x as i64,
        PyVal::Str(s) => s.parse().unwrap_or(0),
    }
}

/// `round(x) >= 1`, raising where Python's `round` does.
fn rounds_to_at_least_one(x: f64) -> bool {
    if x.is_nan() {
        panic!("ValueError: cannot convert float NaN to integer");
    }
    if x.is_infinite() {
        panic!("OverflowError: cannot convert float infinity to integer");
    }
    x.round_ties_even() >= 1.0
}

// ---- Vehicle bindings ---------------------------------------------------------------------

/// `_vehicle_bindings_on_enter(idx, line, data)`: fire the highlighted vehicle action. No
/// delayed press/release pair is needed (unlike clickspots): Lua drives down+up itself.
#[doc(hidden)]
pub fn vehicle_bindings_on_enter(data: Option<Value>) {
    let Some(d) = data.filter(|d| !d.is_null()) else {
        return;
    };
    send_vehicle_bindings_cmd(&format!("EXEC:{d}"));
}

/// `open_vehicle_bindings_browser()` (F9 B).
pub fn open_vehicle_bindings_browser() {
    let (list, vehicle) = {
        let st = state();
        (
            st.bindings.vehicle_bindings_list.clone(),
            st.bindings.vehicle_bindings_vehicle.clone(),
        )
    };
    if list.is_empty() {
        // Nudge the mod in case the push was missed (beamtel started after the vehicle
        // spawned), then report rather than opening an empty list.
        send_vehicle_bindings_cmd("REQUEST");
        say_excluded("No vehicle bindings available");
        return;
    }
    let lines: Vec<String> = list.iter().map(|b| b.1.clone()).collect();
    let entry_data: Vec<Value> = list.iter().map(|b| Value::from(b.0)).collect();
    let vehicle = if vehicle.is_empty() {
        "Vehicle".to_string()
    } else {
        vehicle
    };
    let n = lines.len();
    let on_enter: OnEnter = Arc::new(|_i, _line, data| vehicle_bindings_on_enter(data));
    open_virtual_browser(
        lines,
        BrowserOpts {
            title: Some(format!("{vehicle}, {n} binding{}", plural(n))),
            on_enter: Some(on_enter),
            entry_data,
            ..Default::default()
        },
    );
}

// ---- Vehicle information ------------------------------------------------------------------

/// `(r.get(key) or "").strip()` on a JSON row; panics (AttributeError) for a truthy
/// non-string, as the Python raises.
fn stripped_field(r: &serde_json::Map<String, Value>, key: &str) -> String {
    match r.get(key) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => String::new(),
        Some(Value::String(s)) => py_strip(s).to_string(),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => String::new(),
        Some(Value::Array(a)) if a.is_empty() => String::new(),
        Some(Value::Object(m)) if m.is_empty() => String::new(),
        Some(_) => panic!("AttributeError: object has no attribute 'strip'"),
    }
}

/// `_vinfo_render(rows)`: the mod's {kind,label,value} rows as browser lines.
///
/// A group row is a heading spoken as a bare name, so a listener hears the structure of
/// the page rather than a flat run of numbers. A spec with no label carries its whole
/// sentence in the value, so prefixing an empty label would leave a stray comma.
pub fn vinfo_render(rows: &[Value]) -> Vec<String> {
    let mut lines = Vec::new();
    for r in rows {
        let Some(r) = r.as_object() else {
            panic!("AttributeError: object has no attribute 'get'");
        };
        let is_group = match r.get("kind") {
            None => false, // default "spec"
            Some(k) => k.as_str() == Some("group"),
        };
        let label = stripped_field(r, "label");
        let value = stripped_field(r, "value");
        if is_group {
            lines.push(label);
        } else if !label.is_empty() && !value.is_empty() {
            lines.push(format!("{label}: {value}"));
        } else if !value.is_empty() {
            lines.push(value);
        }
    }
    lines
}

/// `request_vehicle_info(model=None, config=None, timeout=0.6)`: `(lines, failure)`.
/// Also the vehicle spawner's `request_info_fn`.
///
/// `model` None asks about whatever the stock selector's details page is showing;
/// otherwise that explicit pair, with an empty config meaning the model's default. The
/// bounded wait is safe from the command worker and the spawner's worker, never a hook.
pub fn request_vehicle_info(
    model: Option<&str>,
    config: Option<&str>,
    timeout: f64,
) -> (Vec<String>, Option<Failure>) {
    VINFO_EVENT.clear();
    {
        let mut st = state();
        st.vinfo.vinfo_rows = Vec::new();
        st.vinfo.vinfo_error = None;
    }
    match model.filter(|m| !m.is_empty()) {
        Some(m) => send_vehicle_info_cmd(&format!("INFO:{m},{}", config.unwrap_or(""))),
        None => send_vehicle_info_cmd("INFO_SELECTOR"),
    }

    // A mod half that has already failed to answer is asked again (a reply is how the
    // latch clears) but not WAITED for: this runs inline on the F9 SPACE path, where the
    // other answers are a terrain scan and a menu press.
    let absent = state().vinfo.vinfo_absent;
    if !VINFO_EVENT.wait_secs(if absent { 0.0 } else { timeout }) {
        // Silence is its own diagnosis and must not be reported as "no vehicle".
        state().vinfo.vinfo_absent = true;
        return (Vec::new(), Some(("timeout".to_string(), String::new())));
    }
    let (rows, error) = {
        let st = state();
        (st.vinfo.vinfo_rows.clone(), st.vinfo.vinfo_error.clone())
    };
    if let Some(e) = error {
        return (Vec::new(), Some(e));
    }
    (vinfo_render(&rows), None)
}

// ---- UI page text -------------------------------------------------------------------------

/// `_on_screen_context(context, title)`: which readable screen is up (empty when none).
pub fn on_screen_context(context: &str, title: &str) {
    let mut st = state();
    st.page.ui_screen_context = context.to_string();
    st.page.ui_screen_title = title.to_string();
}

/// `_on_page_text(lines, title, code, sentence)`: the UI bridge's page-text callback.
pub fn on_page_text(lines: Vec<String>, title: &str, code: &str, sentence: &str) {
    {
        let mut st = state();
        let p = &mut st.page;
        p.page_text_absent = false;
        if !code.is_empty() {
            p.page_text_lines = Vec::new();
            p.page_text_title = String::new();
            p.page_text_error = Some((code.to_string(), sentence.to_string()));
        } else {
            p.page_text_lines = lines;
            p.page_text_title = title.to_string();
            p.page_text_error = None;
        }
    }
    PAGE_TEXT_EVENT.set();
}

/// Where `request_page_text` broadcasts (`broadcast`). Replaced in tests.
type BroadcastFn = dyn Fn(&Value) + Send + Sync;
static BROADCAST_HOOK: parking_lot::RwLock<Option<Box<BroadcastFn>>> =
    parking_lot::RwLock::new(None);

/// Route `request_page_text`'s broadcast to `hook` (tests only).
#[doc(hidden)]
pub fn set_broadcast_hook_for_tests(hook: Option<Box<BroadcastFn>>) {
    *BROADCAST_HOOK.write() = hook;
}

/// `request_page_text(timeout=0.6)`: `(lines, failure)`, the `request_vehicle_info`
/// contract. `notdetails` is the answer on every screen that is not a mod details page,
/// so the caller must fall through silently on it.
pub fn request_page_text(timeout: f64) -> (Vec<String>, Option<Failure>) {
    PAGE_TEXT_EVENT.clear();
    {
        let mut st = state();
        st.page.page_text_lines = Vec::new();
        st.page.page_text_title = String::new();
        st.page.page_text_error = None;
    }
    let msg = json!({"type": "page_text"});
    match BROADCAST_HOOK.read().as_ref() {
        Some(hook) => hook(&msg),
        None => crate::ui_bridge::broadcast(&msg),
    }

    // Asked again (a reply is how the latch clears) but not WAITED for.
    let absent = state().page.page_text_absent;
    if !PAGE_TEXT_EVENT.wait_secs(if absent { 0.0 } else { timeout }) {
        state().page.page_text_absent = true;
        return (Vec::new(), Some(("timeout".to_string(), String::new())));
    }
    let st = state();
    if let Some(e) = st.page.page_text_error.clone() {
        return (Vec::new(), Some(e));
    }
    (st.page.page_text_lines.clone(), None)
}

/// `_read_mod_details_page()` (F9 Space on a mod details page): true when handled.
///
/// Gated on the latch BEFORE the round trip: this key is the terrain scan and is pressed
/// while driving, so the ordinary case has to cost nothing. A failure is spoken only when
/// the screen really was up and still could not be read; a timeout says nothing about the
/// mod and stays silent, so the chain falls through to the scan.
pub fn read_mod_details_page() -> bool {
    if state().page.ui_screen_context != "mod_details" {
        return false;
    }
    let (lines, failure) = request_page_text(0.6);
    if !lines.is_empty() {
        let title = state().page.page_text_title.clone();
        let n = lines.len();
        open_virtual_browser(
            lines,
            BrowserOpts::titled(format!(
                "{}, {n} items",
                if title.is_empty() {
                    "Mod details"
                } else {
                    &title
                }
            )),
        );
        return true;
    }
    if let Some((code, sentence)) = failure
        && code != "notdetails"
        && code != "timeout"
    {
        say_excluded(if sentence.is_empty() {
            "No mod information"
        } else {
            &sentence
        });
        return true;
    }
    false
}

/// `open_page_text_browser()` (controller Functions menu): read the current readable
/// screen, or say why it cannot be read. The entry only appears while a readable screen
/// is latched, so every failure here IS news and is spoken.
pub fn open_page_text_browser() -> bool {
    let (lines, failure) = request_page_text(0.6);
    if !lines.is_empty() {
        let title = state().page.page_text_title.clone();
        let n = lines.len();
        open_virtual_browser(
            lines,
            BrowserOpts::titled(format!(
                "{}, {n} items",
                if title.is_empty() {
                    "Page text"
                } else {
                    &title
                }
            )),
        );
        return true;
    }
    match failure {
        Some((code, _)) if code == "timeout" => say_excluded("The mod interface did not answer"),
        Some((_, sentence)) if !sentence.is_empty() => say_excluded(&sentence),
        _ => say_excluded("Nothing to read here"),
    }
    false
}

// ---- Environment --------------------------------------------------------------------------

/// `_send_env_cmd(command)` (port 4475).
pub fn send_env_cmd(command: &str) {
    e2_send(ENV_CMD_PORT, command, Some("environment command"));
}

/// `_env_temp_from_display(display_value)`: inverse of `fmt_temp_c_or_f`; the wire is
/// always Celsius.
pub fn env_temp_from_display(display_value: f64) -> f64 {
    if units_mode() == "metric" {
        return display_value;
    }
    (display_value - 32.0) * 5.0 / 9.0
}

/// `_env_row_line(row)`: one spoken line for an environment row. Panics where the Python
/// raises (a NaN or infinite temperature).
pub fn env_row_line(row: &EnvRow) -> String {
    let label = row
        .get("label")
        .or_else(|| row.get("key"))
        .cloned()
        .unwrap_or_else(|| "Setting".to_string());
    let kind = row.get("kind").map(String::as_str).unwrap_or("");
    let lo = env_float(row, "curveLo");
    let hi = env_float(row, "curveHi");

    if kind == "numberC" {
        let Some(value) = env_float(row, "value") else {
            return format!("{label}, not available");
        };
        let (shown, unit) = fmt_temp_c_or_f(value);
        let mut line = format!("{label}, {shown} {unit}");
        // A level whose curve is not flat has no single temperature: the figure is only
        // true for the current time of day, and editing it replaces the whole cycle with
        // one number. Saying so is the difference between an edit and a silent loss.
        if let (Some(lo), Some(hi)) = (lo, hi)
            && rounds_to_at_least_one(hi - lo)
        {
            let (lo_shown, _) = fmt_temp_c_or_f(lo);
            let (hi_shown, _) = fmt_temp_c_or_f(hi);
            line += &format!(", varies {lo_shown} to {hi_shown} through the day");
        }
        if row.get("editable").map(String::as_str).unwrap_or("1") != "1" {
            line += ", locked";
        }
        return line;
    }

    if kind == "action" {
        let Some(lo) = lo else {
            return format!("{label}, not available");
        };
        let (lo_shown, unit) = fmt_temp_c_or_f(lo);
        if let Some(hi) = hi
            && rounds_to_at_least_one(hi - lo)
        {
            let (hi_shown, _) = fmt_temp_c_or_f(hi);
            return format!("{label}, {lo_shown} to {hi_shown} {unit}");
        }
        return format!("{label}, {lo_shown} {unit}");
    }

    label
}

/// `speak_ambient_temperature()` (F9 Ctrl+T): the live outside temperature.
pub fn speak_ambient_temperature() {
    let (ambient, unavailable) = {
        let st = state();
        (st.env.env_ambient_c, st.env.env_unavailable.clone())
    };
    let Some(c) = ambient else {
        // Nudge the mod in case the push was missed, so the next press can answer.
        send_env_cmd("REQUEST");
        if !unavailable.is_empty() {
            say_plain(&format!("Ambient temperature unavailable, {unavailable}"));
        } else {
            say_plain("No ambient temperature");
        }
        return;
    };
    let (shown, unit) = fmt_temp_c_or_f(c);
    say_plain(&format!("Ambient temperature {shown} {unit}"));
}

/// `_env_change_timed_out()`: the edit's reply timer fired.
#[doc(hidden)]
pub fn env_change_timed_out() {
    {
        let mut st = state();
        if !st.env.env_awaiting_change {
            return;
        }
        st.env.env_awaiting_change = false;
    }
    // The alternative is a key that appears to do nothing.
    say_excluded("No response from the game");
}

/// `_env_notify_refresh()`: called by the environment listener on ENV_END.
pub fn env_notify_refresh() {
    let speech = {
        let mut st = state();
        if st.env.env_browser_open && st.vbrowser.vbrowser_active {
            let lines: Vec<String> = st.env.env_rows.iter().map(env_row_line).collect();
            // Replaced in place and only when the shape matches, so a push that arrives
            // while the list is being navigated cannot move the cursor onto a different
            // setting than the one the user last heard.
            if lines.len() == st.vbrowser.vbrowser_lines.len() {
                st.vbrowser.vbrowser_lines = lines;
            }
        }
        if st.env.env_awaiting_change {
            st.env.env_awaiting_change = false;
            if let Some(t) = st.env.env_change_timer.take() {
                t.cancel();
            }
            let idx = st.vbrowser.vbrowser_index;
            st.vbrowser.vbrowser_lines.get(idx).cloned()
        } else {
            None
        }
    };
    if let Some(s) = speech {
        say_excluded(&s);
    }
}

/// `_env_arm_change()`.
fn env_arm_change() {
    let mut st = state();
    st.env.env_awaiting_change = true;
    if let Some(t) = st.env.env_change_timer.take() {
        t.cancel();
    }
    st.env.env_change_timer = Some(Timer::start(1.0, env_change_timed_out));
}

/// `_env_on_adjust(idx, line, data, delta)`: left/right on an environment row.
#[doc(hidden)]
pub fn env_on_adjust(data: Option<Value>, delta: i64) {
    let row = {
        let st = state();
        let Some(i) = data_index(&data) else {
            return;
        };
        if i < 0 || i as usize >= st.env.env_rows.len() {
            return;
        }
        st.env.env_rows[i as usize].clone()
    };
    if row.get("kind").map(String::as_str) != Some("numberC") {
        return;
    }
    if row.get("editable").map(String::as_str).unwrap_or("1") != "1" {
        say_excluded("Locked");
        return;
    }
    let Some(current) = env_float(&row, "value") else {
        return;
    };
    // Stepped in the unit the user HEARS, not in Celsius: a one-degree press on an
    // imperial readout that moved the value by 1.8 F would skip numbers.
    let (shown, _unit) = fmt_temp_c_or_f(current);
    let mut target_c = env_temp_from_display((pyval_int(&shown) + delta) as f64);
    if let Some(lo) = env_float(&row, "min") {
        target_c = py_max(lo, target_c);
    }
    if let Some(hi) = env_float(&row, "max") {
        target_c = py_min(hi, target_c);
    }
    env_arm_change();
    let key = row.get("key").map(String::as_str).unwrap_or("None");
    send_env_cmd(&format!("SET:{key}={}", py_f(target_c, 2)));
}

/// `_env_on_enter(idx, line, data)`: Enter on an environment row.
#[doc(hidden)]
pub fn env_on_enter(data: Option<Value>) {
    let row = {
        let st = state();
        let Some(i) = data_index(&data) else {
            return;
        };
        if i < 0 || i as usize >= st.env.env_rows.len() {
            return;
        }
        st.env.env_rows[i as usize].clone()
    };
    let kind = row.get("kind").map(String::as_str);
    if kind == Some("action") && row.get("key").map(String::as_str) == Some("restore") {
        if row.get("editable").map(String::as_str).unwrap_or("1") != "1" {
            say_excluded("No level default to restore");
            return;
        }
        env_arm_change();
        send_env_cmd("RESTORE");
        return;
    }
    if kind == Some("numberC") {
        say_excluded("Use left and right to adjust, hold shift for ten at a time");
    }
}

/// `open_environment_browser()` (F9 N): the environment values the pause UI omits.
pub fn open_environment_browser() {
    let (rows, unavailable) = {
        let st = state();
        (st.env.env_rows.clone(), st.env.env_unavailable.clone())
    };
    if rows.is_empty() {
        // Nudge the mod in case the push was missed, then report rather than opening an
        // empty list.
        send_env_cmd("REQUEST");
        if !unavailable.is_empty() {
            say_excluded(&format!("Environment unavailable, {unavailable}"));
        } else {
            say_excluded("No environment settings available");
        }
        return;
    }
    let lines: Vec<String> = rows.iter().map(env_row_line).collect();
    let entry_data: Vec<Value> = (0..rows.len()).map(Value::from).collect();
    let n = lines.len();
    let on_enter: OnEnter = Arc::new(|_i, _line, data| env_on_enter(data));
    let on_adjust: OnAdjust = Arc::new(|_i, _line, data, delta| env_on_adjust(data, delta));
    open_virtual_browser(
        lines,
        BrowserOpts {
            title: Some(format!("Environment, {n} setting{}", plural(n))),
            on_enter: Some(on_enter),
            on_adjust: Some(on_adjust),
            entry_data,
            ..Default::default()
        },
    );
    // Set AFTER the open, never before: open_virtual_browser closes whatever browser was
    // up, and close_virtual_browser clears this flag.
    let mut st = state();
    st.env.env_browser_open = st.vbrowser.vbrowser_active;
}

// ---- Forced induction ---------------------------------------------------------------------

/// What the induction report heard: the last usable report and whether anything arrived.
/// `Err` is an exception that ended the Python report thread (nothing is spoken).
#[doc(hidden)]
pub fn induction_collect<I: IntoIterator<Item = String>>(
    replies: I,
) -> Result<(Option<PyDict>, bool), String> {
    let mut report: Option<PyDict> = None;
    let mut answered = false;
    for raw in replies {
        let text = py_strip(&raw);
        answered = true;
        if text == "DONE" {
            break;
        }
        if let Some(rest) = text.strip_prefix("ERROR:") {
            log::warn!("Forced induction report failed: {rest}");
        }
        match beam_reports::induction_report::parse_reply(text) {
            // `parse_reply(text) or report`: an empty dict is falsy.
            Ok(Some(d)) if !d.is_empty() => report = Some(d),
            Ok(_) => {}
            Err(e) => return Err(format!("{}: {}", e.kind.name(), e.msg)),
        }
    }
    Ok((report, answered))
}

/// The speech that ends the report. `Err` where `report_phrase` raises (the Python thread
/// dies and says nothing).
#[doc(hidden)]
pub fn induction_speak(report: Option<&PyDict>, answered: bool) -> Result<(), String> {
    if let Some(r) = report {
        let fp = |psi: f64| {
            let (v, u) = fmt_pressure(psi);
            (v.to_string(), u.to_string())
        };
        let phrase = beam_reports::induction_report::report_phrase(r, &fp)
            .map_err(|e| format!("{}: {}", e.kind.name(), e.msg))?;
        say_plain(&phrase);
    } else if answered {
        say_plain("No forced induction");
    } else {
        say_excluded("No response from the game");
    }
    Ok(())
}

fn induction_thread() {
    let mut replies: Vec<String> = Vec::new();
    // 4447 is shared with the damage report and the dumps, so it can be busy.
    let mut run = || -> std::io::Result<()> {
        let recv_sock = UdpSocket::bind(("127.0.0.1", DUMP_RESP_PORT))?;
        // A send failure is an OSError, caught by the same handler as the bind.
        crate::net::send_udp(SCANNER_CMD_PORT, b"INDUCTION")?;
        recv_sock.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        let mut buf = vec![0u8; 4096];
        loop {
            match recv_sock.recv_from(&mut buf) {
                Ok((n, _)) => {
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let done = py_strip(&text) == "DONE";
                    replies.push(text);
                    if done {
                        return Ok(());
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
    };
    if let Err(e) = run() {
        log::error!("Forced induction report could not use its port: {e}");
    }
    let outcome = induction_collect(replies).and_then(|(r, a)| induction_speak(r.as_ref(), a));
    if let Err(e) = outcome {
        log::error!("Forced induction report thread ended: {e}");
    }
}

/// `speak_induction_report()` (F9 Alt+T): the turbo's and supercharger's live figures,
/// asked for over the dump ports.
pub fn speak_induction_report() {
    spawn_daemon("induction-report", induction_thread);
}

// ---- Audio debug --------------------------------------------------------------------------

/// `_send_aud_cmd(command)` (port 4484).
pub fn send_aud_cmd(command: &str) {
    e2_send(AUDIO_DEBUG_CMD_PORT, command, Some("audio debug command"));
}

/// `_aud_cancel_timer()`.
#[doc(hidden)]
pub fn aud_cancel_timer() {
    let t = state().aud.aud_timer.take();
    if let Some(t) = t {
        t.cancel();
    }
}

/// `_aud_timed_out()`.
#[doc(hidden)]
pub fn aud_timed_out() {
    {
        let mut st = state();
        if st.aud.aud_pending.is_none() && !st.aud.aud_awaiting {
            return;
        }
        st.aud.aud_pending = None;
        st.aud.aud_awaiting = false;
    }
    // Same reasoning as the environment browser: silence would read as a dead key.
    say_excluded("No response from the game");
}

/// `_aud_send(command, pending=None, start=0, awaiting=False)`: a command the game
/// answers, with the one-shot no-answer fallback.
fn aud_send(command: &str, pending: Option<&str>, start: i64, awaiting: bool) {
    aud_cancel_timer();
    {
        let mut st = state();
        st.aud.aud_pending = pending.map(str::to_string);
        st.aud.aud_pending_start = start;
        st.aud.aud_awaiting = awaiting;
        // Longer than the GE side's own 1 s vehicle timeout, so a vehicle that does not
        // answer is reported by the game rather than as a dead game.
        st.aud.aud_timer = Some(Timer::start(1.5, aud_timed_out));
    }
    send_aud_cmd(command);
}

/// `_aud_on_message(text, is_error)`: AUD_ERROR / AUD_DONE, always the direct answer to
/// a key the user just pressed.
pub fn aud_on_message(text: &str, is_error: bool) {
    aud_cancel_timer();
    // An error is followed by a fresh row set; reading the current line on top of it
    // would cut the reason off mid-word.
    state().aud.aud_awaiting = false;
    if is_error {
        log::warn!("Audio debug: {text}");
    }
    let msg = if !text.is_empty() {
        text
    } else if is_error {
        "Audio debug command failed"
    } else {
        "Done"
    };
    say_excluded(msg);
}

fn aud_line(row: &Value) -> String {
    match audio_debug::row_line(&row_from_json(row)) {
        Ok(s) => s,
        Err(e) => panic!("{}: {}", e.kind.name(), e.msg),
    }
}

/// `_aud_engine_lines()`.
fn aud_engine_lines(rows: &[Value]) -> Vec<String> {
    rows.iter().map(aud_line).collect()
}

/// `_aud_open_engine_browser(start_index=0, title=True)`.
fn aud_open_engine_browser(start_index: i64, title: bool) {
    let rows = state().aud.aud_rows.clone();
    let lines = aud_engine_lines(&rows);
    let entry_data: Vec<Value> = (0..lines.len()).map(Value::from).collect();
    let on_enter: OnEnter = Arc::new(aud_engine_enter);
    let on_adjust: OnAdjust = Arc::new(aud_engine_adjust);
    open_virtual_browser(
        lines,
        BrowserOpts {
            title: if title {
                Some("Audio debug".to_string())
            } else {
                None
            },
            on_enter: Some(on_enter),
            on_adjust: Some(on_adjust),
            entry_data,
            start_index,
            ..Default::default()
        },
    );
    // After the open, never before: open_virtual_browser closes whatever was up, and
    // close_virtual_browser clears this.
    let mut st = state();
    st.aud.aud_browser = if st.vbrowser.vbrowser_active {
        Some("engine".to_string())
    } else {
        None
    };
}

/// `_aud_on_rows()`: a fresh engine-sound row set has landed (command worker).
pub fn aud_on_rows() {
    let (pending, start, browser, active, awaiting, rows) = {
        let st = state();
        (
            st.aud.aud_pending.clone(),
            st.aud.aud_pending_start,
            st.aud.aud_browser.clone(),
            st.vbrowser.vbrowser_active,
            st.aud.aud_awaiting,
            st.aud.aud_rows.clone(),
        )
    };
    if pending.as_deref() == Some("engine") {
        aud_cancel_timer();
        {
            let mut st = state();
            st.aud.aud_pending = None;
            st.aud.aud_awaiting = false;
        }
        aud_open_engine_browser(start, true);
        return;
    }
    if browser.as_deref() != Some("engine") || !active {
        return;
    }
    let speak = awaiting;
    if speak {
        aud_cancel_timer();
        state().aud.aud_awaiting = false;
    }
    let lines = aud_engine_lines(&rows);
    let (same_shape, index) = {
        let mut st = state();
        let index = st.vbrowser.vbrowser_index;
        if lines.len() == st.vbrowser.vbrowser_lines.len() {
            // In place, so the cursor stays on the setting the user last heard.
            st.vbrowser.vbrowser_lines = lines;
            (true, index)
        } else {
            (false, index)
        }
    };
    if same_shape {
        if speak {
            let line = state().vbrowser.vbrowser_lines.get(index).cloned();
            match line {
                Some(l) => say_excluded(&l),
                None => panic!("IndexError: list index out of range"),
            }
        }
    } else {
        // A different sound with a different set of controls (an EV motor has fewer).
        aud_open_engine_browser(index as i64, false);
    }
}

fn aud_row_at(data: &Option<Value>) -> Option<Value> {
    let i = data_index(data)?;
    let st = state();
    if i < 0 || i as usize >= st.aud.aud_rows.len() {
        return None;
    }
    Some(st.aud.aud_rows[i as usize].clone())
}

fn str_field<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

/// `_aud_engine_adjust(idx, line, data, delta)`.
#[doc(hidden)]
pub fn aud_engine_adjust(_idx: usize, line: &str, data: Option<Value>, delta: i64) {
    let Some(row) = aud_row_at(&data) else {
        return;
    };
    let kind = str_field(&row, "kind");
    if kind == Some("select") {
        match audio_debug::select_target(&row_from_json(&row), delta) {
            None => say_excluded("Only one sound"),
            Some(target) => aud_send(&format!("SELECT:{target}"), None, 0, true),
        }
        return;
    }
    if kind != Some("number") {
        return;
    }
    match audio_debug::stepped_value(&row_from_json(&row), delta) {
        // Already at the end of the range: re-read it rather than stay silent.
        None => say_excluded(line),
        Some(target) => {
            let key = str_field(&row, "key").unwrap_or("None");
            aud_send(
                &format!("SET:{key}={}", audio_debug::wire_number(target)),
                None,
                0,
                true,
            );
        }
    }
}

/// `_aud_engine_enter(idx, line, data)`.
#[doc(hidden)]
pub fn aud_engine_enter(_idx: usize, line: &str, data: Option<Value>) {
    let Some(row) = aud_row_at(&data) else {
        return;
    };
    let kind = str_field(&row, "kind");
    let key = str_field(&row, "key");
    if kind == Some("number") {
        say_excluded("Use left and right to adjust, hold shift for ten at a time");
    } else if kind == Some("select") {
        say_excluded("Use left and right to choose a sound");
    } else if key == Some("copy") {
        aud_send("COPY", None, 0, false);
    } else if key == Some("restore") {
        if str_field(&row, "editable") == Some("0") {
            say_excluded("Nothing to restore");
            return;
        }
        aud_send("RESTORE", None, 0, true);
    } else if key == Some("library") {
        state().aud.aud_sfx_stack = Vec::new();
        aud_open_sfx(0);
    } else {
        say_excluded(line);
    }
}

/// `_aud_open_sfx(start_index=0)`.
fn aud_open_sfx(start_index: i64) {
    let path = state()
        .aud
        .aud_sfx_stack
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(">");
    aud_send(&format!("SFX_LIST:{path}"), Some("sfx"), start_index, false);
}

/// `_aud_on_sfx_rows()`: a sound-library folder listing has landed (command worker).
pub fn aud_on_sfx_rows() {
    let (rows, head, start) = {
        let st = state();
        if st.aud.aud_pending.as_deref() != Some("sfx") {
            return;
        }
        (
            st.aud.aud_sfx_rows.clone(),
            st.aud.aud_sfx_head.clone(),
            st.aud.aud_pending_start,
        )
    };
    aud_cancel_timer();
    state().aud.aud_pending = None;
    // "Stop sound" leads every folder: playOnce of a looping event plays until stopped,
    // and the row has to be reachable from wherever the user is when they want silence.
    let mut lines = vec!["Stop sound".to_string()];
    lines.extend(
        rows.iter()
            .map(|r| audio_debug::sfx_line(&row_from_json(r))),
    );
    let mut entry_data = vec![Value::Null];
    entry_data.extend(rows.iter().cloned());
    let path = str_field(&head, "path").unwrap_or("");
    let n = rows.len();
    let title = format!(
        "{}, {n} item{}",
        if path.is_empty() {
            "Sound library"
        } else {
            path
        },
        plural(n)
    );
    let on_enter: OnEnter = Arc::new(aud_sfx_enter);
    let on_escape: OnEscape = Arc::new(aud_sfx_escape);
    open_virtual_browser(
        lines,
        BrowserOpts {
            title: Some(title),
            on_enter: Some(on_enter),
            on_escape: Some(on_escape),
            entry_data,
            // +1 for the Stop row: a fresh folder opens on its first item, and returning
            // from a sub-folder lands on that sub-folder.
            start_index: start + 1,
            ..Default::default()
        },
    );
    let mut st = state();
    st.aud.aud_browser = if st.vbrowser.vbrowser_active {
        Some("sfx".to_string())
    } else {
        None
    };
}

/// `_aud_sfx_enter(idx, line, data)`.
#[doc(hidden)]
pub fn aud_sfx_enter(idx: usize, _line: &str, data: Option<Value>) {
    let Some(data) = data.filter(|d| !d.is_null()) else {
        send_aud_cmd("SFX_STOP");
        say_excluded("Stopped");
        return;
    };
    if str_field(&data, "kind") == Some("dir") {
        state().aud.aud_sfx_stack.push(idx as i64 - 1);
        aud_open_sfx(0);
        return;
    }
    if let Some(sound_id) = str_field(&data, "id").filter(|s| !s.is_empty()) {
        // No speech: the sound itself is the answer, and speech would talk over it.
        send_aud_cmd(&format!("SFX_PLAY:{sound_id}"));
    }
}

/// `_aud_sfx_escape()`.
#[doc(hidden)]
pub fn aud_sfx_escape() {
    let child = state().aud.aud_sfx_stack.pop();
    if let Some(child) = child {
        aud_open_sfx(child);
        return;
    }
    send_aud_cmd("SFX_STOP");
    // Back to the engine list, on its Sound library row, from the rows already held:
    // they were current when the library was opened from them.
    let n = state().aud.aud_rows.len();
    if n > 0 {
        aud_open_engine_browser(n as i64 - 1, false);
    } else {
        close_virtual_browser(true);
    }
}

/// `open_audio_debug_browser()` (F9 J): the World Editor's audio tools, which are ImGui
/// and unreadable.
pub fn open_audio_debug_browser() {
    aud_send("REQUEST", Some("engine"), 0, false);
}

// ---- Clickspots ---------------------------------------------------------------------------

/// `open_clickspot_browser()` (F9 Ctrl+Shift+Alt+C): every detected clickspot.
pub fn open_clickspot_browser() {
    let (active, list) = {
        let st = state();
        (
            st.clickspot.clickspot_mode_active,
            st.clickspot.clickspot_trigger_list.clone(),
        )
    };
    if !active {
        say_excluded("Clickspot detection is not active");
        return;
    }
    if list.is_empty() {
        say_excluded("No clickspots available");
        return;
    }
    let lines: Vec<String> = list.iter().map(|t| t.2.clone()).collect(); // display names
    let entry_data: Vec<Value> = list.iter().map(|t| Value::from(t.0)).collect(); // cache indices
    let n = lines.len();
    let on_enter: OnEnter = Arc::new(|_i, _line, data| {
        crate::listeners::clickspot::clickspot_browser_on_enter(data.as_ref())
    });
    open_virtual_browser(
        lines,
        BrowserOpts {
            title: Some(format!("{n} clickspots")),
            on_enter: Some(on_enter),
            entry_data,
            ..Default::default()
        },
    );
}
