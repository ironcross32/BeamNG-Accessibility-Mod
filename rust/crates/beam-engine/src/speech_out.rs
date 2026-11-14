//! Owner: EA. Speech output (`beamtel.py:153-170, 299-502`): `say()`, the replay buffer,
//! the speech tap, the speech log file and the loading lifecycle.
//!
//! None of this lives in `state::State`, because `say()` is called while `state_lock` is
//! held (`_report_gear_change`, `on_buffer_nav_press`, the F9 digit branch, the telemetry
//! loop) and must therefore never take it. The flags `say()` reads are atomics here, and the
//! Python's own separate locks (`_loading_lock`, `_speech_tap_lock`) are kept. All of them
//! are leaves: nothing here calls back into the engine while holding one.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use beam_base::pyfmt::{py_float_str, py_strip};
use parking_lot::{Mutex, RwLock};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::compat::beam_speech::speech;
use crate::timer::{Timer, local_hms_millis, monotonic, time_time};

/// `SPEECH_BUFFER = deque(maxlen=100)`: what the user can replay.
pub const SPEECH_BUFFER_MAXLEN: usize = 100;
/// `SPEECH_TAP = deque(maxlen=1000)`: every `say()` call, for the MCP server.
pub const SPEECH_TAP_MAXLEN: usize = 1000;
/// When a vehicle name is announced, background speech is demoted to interrupt=False for
/// this long so it queues behind the name rather than cutting it off.
pub const SPEECH_PROTECT_S: f64 = 3.0;

static SPEECH_BUFFER: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// One `SPEECH_TAP` record. Field order is the Python dict's, so JSON output matches.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TapEntry {
    pub seq: u64,
    pub t: f64,
    pub text: String,
    pub source: String,
    pub interrupt: bool,
    pub excluded: bool,
    pub spoken: bool,
    pub reason: Option<String>,
}

struct Tap {
    entries: VecDeque<TapEntry>,
    seq: u64,
}

/// `_speech_tap_lock` with `SPEECH_TAP` and `_speech_tap_seq` behind it.
static SPEECH_TAP: Mutex<Tap> = Mutex::new(Tap {
    entries: VecDeque::new(),
    seq: 0,
});

/// `_command_context`: True only while processing an F9/F10 command keystroke.
static COMMAND_CONTEXT: AtomicBool = AtomicBool::new(false);
/// `_speech_protected_until` (monotonic seconds, f64 bits).
static SPEECH_PROTECTED_UNTIL: AtomicU64 = AtomicU64::new(0);
/// `speech_logging_active`.
static SPEECH_LOGGING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The loading lifecycle block, behind `_loading_lock`.
#[derive(Debug, Default)]
struct Loading {
    loading_active: bool,
    loading_settling: bool,
    loading_focus_text: String,
    loading_pending_vehicle: String,
    loading_generation: u64,
    /// `_telemetry_baseline_pending`. Python writes it under `_loading_lock` and reads it
    /// in the telemetry loop under `state_lock`; here it lives with the writer.
    telemetry_baseline_pending: bool,
}

static LOADING: Mutex<Loading> = Mutex::new(Loading {
    loading_active: false,
    loading_settling: false,
    loading_focus_text: String::new(),
    loading_pending_vehicle: String::new(),
    loading_generation: 0,
    telemetry_baseline_pending: false,
});

type SpeakFn = Box<dyn Fn(&str, bool) + Send + Sync>;

/// Test seam: replaces `speech.speak` (sims patch it the same way in Python).
static SPEAK_OVERRIDE: RwLock<Option<SpeakFn>> = RwLock::new(None);

static TABLE_POINTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^table:\s*0x[0-9a-f]+$").expect("static regex")
});

/// `SPEECH_LOG_PATH`.
pub fn speech_log_path() -> PathBuf {
    beam_base::paths::config_dir().join("speech.log")
}

pub fn command_context() -> bool {
    COMMAND_CONTEXT.load(Ordering::SeqCst)
}

pub fn set_command_context(value: bool) {
    COMMAND_CONTEXT.store(value, Ordering::SeqCst);
}

pub fn speech_protected_until() -> f64 {
    f64::from_bits(SPEECH_PROTECTED_UNTIL.load(Ordering::SeqCst))
}

pub fn set_speech_protected_until(value: f64) {
    SPEECH_PROTECTED_UNTIL.store(value.to_bits(), Ordering::SeqCst);
}

/// `_speech_protected_until = time.monotonic() + SPEECH_PROTECT_S`.
pub fn protect_speech() {
    set_speech_protected_until(monotonic() + SPEECH_PROTECT_S);
}

pub fn speech_logging_active() -> bool {
    SPEECH_LOGGING_ACTIVE.load(Ordering::SeqCst)
}

pub fn set_speech_logging_active(value: bool) {
    SPEECH_LOGGING_ACTIVE.store(value, Ordering::SeqCst);
}

/// `with _loading_lock: _loading_active or _loading_settling`.
pub fn loading_or_settling() -> bool {
    let l = LOADING.lock();
    l.loading_active || l.loading_settling
}

/// The scanner listener's SWITCHED step: under `_loading_lock`, if loading or settling,
/// remember `name` as the vehicle to announce when ready. Returns whether it was queued.
pub fn queue_loading_pending_vehicle(name: &str) -> bool {
    let mut l = LOADING.lock();
    let loading_or_settling = l.loading_active || l.loading_settling;
    if loading_or_settling {
        l.loading_pending_vehicle = name.to_string();
    }
    loading_or_settling
}

/// The telemetry loop's `baseline_frame = _telemetry_baseline_pending; if it: clear it`.
pub fn take_telemetry_baseline_pending() -> bool {
    let mut l = LOADING.lock();
    std::mem::replace(&mut l.telemetry_baseline_pending, false)
}

/// Replace (or with `None`, restore) the speech backend. For tests and simulators only.
pub fn set_speak_override(f: Option<SpeakFn>) {
    *SPEAK_OVERRIDE.write() = f;
}

fn backend_speak(text: &str, interrupt: bool) {
    if let Some(f) = SPEAK_OVERRIDE.read().as_ref() {
        f(text, interrupt);
        return;
    }
    speech::speak(text, interrupt);
}

fn py_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Python's `repr()` of a str, for log lines.
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `str(value)` for a JSON scalar.
fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(if *b { "True" } else { "False" }.to_string()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i.to_string())
            } else if let Some(u) = n.as_u64() {
                Some(u.to_string())
            } else {
                n.as_f64().map(py_float_str)
            }
        }
        _ => None,
    }
}

/// `_normalize_speech_value`: a scalar speech value, or None for ambiguous containers.
///
/// The Python logs container contents with `%r`; this logs them as JSON (log text only).
pub fn normalize_speech_value(value: &Value, source: &str) -> Option<String> {
    let original = value;
    let mut value = value;
    loop {
        match value {
            Value::Array(items) => {
                log::warn!(
                    "[LUA_TABLE_SPEECH] source={source} count={} contents={value}",
                    items.len()
                );
                if items.len() != 1 {
                    return None;
                }
                value = &items[0];
            }
            Value::Object(map) => {
                log::warn!(
                    "[LUA_TABLE_SPEECH] source={source} count={} contents={value}",
                    map.len()
                );
                if map.len() != 1 {
                    return None;
                }
                value = map.values().next()?;
            }
            _ => break,
        }
    }
    let Some(text) = scalar_text(value) else {
        if !original.is_null() {
            log::warn!(
                "[LUA_TABLE_SPEECH] source={source} unsupported_type={}",
                py_type_name(value)
            );
        }
        return None;
    };
    if TABLE_POINTER.is_match(py_strip(&text)) {
        log::warn!(
            "[LUA_TABLE_SPEECH] source={source} collapsed_pointer={} contents_lost_upstream",
            py_repr_str(&text)
        );
        return None;
    }
    Some(text)
}

fn speech_tap_record(
    text: &str,
    source: &str,
    interrupt: bool,
    excluded: bool,
    spoken: bool,
    reason: Option<&str>,
) {
    let mut tap = SPEECH_TAP.lock();
    tap.seq += 1;
    let seq = tap.seq;
    tap.entries.push_back(TapEntry {
        seq,
        t: time_time(),
        text: text.to_string(),
        source: source.to_string(),
        interrupt,
        excluded,
        spoken,
        reason: reason.map(str::to_string),
    });
    while tap.entries.len() > SPEECH_TAP_MAXLEN {
        tap.entries.pop_front();
    }
}

/// `say(text, interrupt=True, exclude_from_buffer=False, source="python")` for a str.
pub fn say(text: &str, interrupt: bool, exclude_from_buffer: bool, source: &str) {
    if TABLE_POINTER.is_match(py_strip(text)) {
        log::warn!(
            "[LUA_TABLE_SPEECH] source={source} collapsed_pointer={} contents_lost_upstream",
            py_repr_str(text)
        );
        return;
    }
    say_normalized(text, interrupt, exclude_from_buffer, source);
}

/// `say(text)`: interrupt, kept in the replay buffer, source "python".
pub fn say_plain(text: &str) {
    say(text, true, false, "python");
}

/// `say(text, exclude_from_buffer=True)`: the most common call in beamtel.py.
pub fn say_excluded(text: &str) {
    say(text, true, true, "python");
}

/// `say(value, ...)` for a value that is not known to be a str (bridge and UDP payloads).
pub fn say_value(value: &Value, interrupt: bool, exclude_from_buffer: bool, source: &str) {
    if let Some(text) = normalize_speech_value(value, source) {
        say_normalized(&text, interrupt, exclude_from_buffer, source);
    }
}

fn say_normalized(normalized: &str, interrupt: bool, exclude_from_buffer: bool, source: &str) {
    let t = py_strip(normalized);
    if t.is_empty() {
        return;
    }
    let mut interrupt = interrupt;

    let suppress_for_loading = loading_or_settling();
    if suppress_for_loading && !command_context() && source != "loading_lifecycle" {
        log::info!(
            "Suppressed speech during loading: source={source} text={}",
            py_repr_str(t)
        );
        speech_tap_record(
            t,
            source,
            interrupt,
            exclude_from_buffer,
            false,
            Some("loading_suppressed"),
        );
        return;
    }

    if command_context() {
        log::info!("Speech output: '{t}'");
    }

    // Background threads must not cut through a protected vehicle-name announcement.
    // Command context always interrupts normally, as does UI navigation speech.
    if interrupt && !command_context() && source != "ui_bridge" && monotonic() < speech_protected_until()
    {
        interrupt = false;
    }

    if speech_logging_active() {
        let line = format!("[{}] {t}\n", local_hms_millis());
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(speech_log_path())
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    if !exclude_from_buffer {
        let mut buf = SPEECH_BUFFER.lock();
        buf.push_back(t.to_string());
        while buf.len() > SPEECH_BUFFER_MAXLEN {
            buf.pop_front();
        }
    }

    speech_tap_record(t, source, interrupt, exclude_from_buffer, true, None);
    backend_speak(t, interrupt);
}

/// `stop_speech()`.
pub fn stop_speech() {
    speech::stop();
}

/// `len(SPEECH_BUFFER)`.
pub fn speech_buffer_len() -> usize {
    SPEECH_BUFFER.lock().len()
}

/// `SPEECH_BUFFER[index]` (0 is the oldest).
pub fn speech_buffer_get(index: usize) -> Option<String> {
    SPEECH_BUFFER.lock().get(index).cloned()
}

/// `SPEECH_BUFFER[-recency]` (1 is the newest), or None outside `1..=len`.
pub fn speech_buffer_recent(recency: usize) -> Option<String> {
    let buf = SPEECH_BUFFER.lock();
    if recency == 0 || recency > buf.len() {
        return None;
    }
    buf.get(buf.len() - recency).cloned()
}

/// `list(SPEECH_BUFFER)`.
pub fn speech_buffer_snapshot() -> Vec<String> {
    SPEECH_BUFFER.lock().iter().cloned().collect()
}

/// `get_speech_log`'s result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpeechLog {
    pub entries: Vec<TapEntry>,
    pub next_seq: u64,
    pub dropped: bool,
}

/// `get_speech_log(since_seq=None, last_n=50, source=None, contains=None, spoken_only=False)`.
///
/// `last_n: None` and `Some(0)` both mean "no limit" (Python's `if last_n:`); pass
/// `Some(50)` for the Python default. Empty `source`/`contains` mean "no filter", as in
/// Python. `contains` compares lowercased (Rust's Unicode lowercasing; Python's differs
/// only for a handful of special-cased characters).
pub fn get_speech_log(
    since_seq: Option<i64>,
    last_n: Option<i64>,
    source: Option<&str>,
    contains: Option<&str>,
    spoken_only: bool,
) -> SpeechLog {
    let (mut entries, newest): (Vec<TapEntry>, u64) = {
        let tap = SPEECH_TAP.lock();
        (tap.entries.iter().cloned().collect(), tap.seq)
    };
    let oldest = entries.first().map(|e| e.seq as i64).unwrap_or(0);
    let dropped = match since_seq {
        Some(s) => !entries.is_empty() && s < oldest - 1,
        None => false,
    };
    if let Some(s) = since_seq {
        entries.retain(|e| (e.seq as i64) > s);
    }
    if let Some(src) = source.filter(|s| !s.is_empty()) {
        entries.retain(|e| e.source == src);
    }
    if let Some(needle) = contains.filter(|s| !s.is_empty()) {
        let needle = needle.to_lowercase();
        entries.retain(|e| e.text.to_lowercase().contains(&needle));
    }
    if spoken_only {
        entries.retain(|e| e.spoken);
    }
    if let Some(n) = last_n.filter(|n| *n != 0) {
        // entries[-n:]
        let len = entries.len() as i64;
        let start = if n > 0 { (len - n).max(0) } else { (-n).min(len) };
        entries = entries.split_off(start as usize);
    }
    SpeechLog {
        entries,
        next_seq: newest,
        dropped,
    }
}

/// `_finish_loading_settle(generation)`: announce readiness one second after loading ends.
pub fn finish_loading_settle(generation: u64) {
    let (vehicle, focus) = {
        let mut l = LOADING.lock();
        if generation != l.loading_generation || l.loading_active {
            return;
        }
        let vehicle = py_strip(&l.loading_pending_vehicle).to_string();
        let focus = py_strip(&l.loading_focus_text).to_string();
        l.loading_pending_vehicle.clear();
        l.loading_settling = false;
        (vehicle, focus)
    };
    let ready_text = if !vehicle.is_empty() {
        vehicle
    } else if !focus.is_empty() {
        focus
    } else {
        "Ready".to_string()
    };
    protect_speech();
    log::info!(
        "Loading lifecycle ready: generation={generation} cue={}",
        py_repr_str(&ready_text)
    );
    say(&ready_text, true, true, "loading_lifecycle");
}

/// `_on_loading_state_changed(active, focus_text="")`, the UI bridge's loading callback.
pub fn on_loading_state_changed(active: bool, focus_text: &str) {
    let generation = {
        let mut l = LOADING.lock();
        if active == l.loading_active && (active || l.loading_settling) {
            return;
        }
        l.loading_generation += 1;
        l.loading_focus_text = py_strip(focus_text).to_string();
        if active {
            l.loading_active = true;
            l.loading_settling = false;
            l.loading_pending_vehicle.clear();
            l.telemetry_baseline_pending = true;
        } else {
            l.loading_active = false;
            l.loading_settling = true;
        }
        l.loading_generation
    };
    log::info!(
        "Loading lifecycle changed: active={} generation={generation} focus={}",
        if active { "True" } else { "False" },
        py_repr_str(focus_text)
    );
    if active {
        stop_speech();
        say("Loading", true, true, "loading_lifecycle");
    } else {
        let _timer = Timer::start(1.0, move || finish_loading_settle(generation));
    }
}

/// Serialises tests (here and in wave-2 test files) that use the process-wide speech state.
#[doc(hidden)]
pub static TEST_SERIAL: Mutex<()> = Mutex::new(());

/// Resets every piece of speech state to its import-time value. For tests only.
#[doc(hidden)]
pub fn reset_for_tests() {
    SPEECH_BUFFER.lock().clear();
    {
        let mut tap = SPEECH_TAP.lock();
        tap.entries.clear();
        tap.seq = 0;
    }
    set_command_context(false);
    set_speech_protected_until(0.0);
    set_speech_logging_active(false);
    *LOADING.lock() = Loading::default();
}

/// Sets the loading flags directly. For tests only.
#[doc(hidden)]
pub fn set_loading_for_tests(active: bool, settling: bool) {
    let mut l = LOADING.lock();
    l.loading_active = active;
    l.loading_settling = settling;
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_testutil::load;
    use std::sync::Arc;

    fn recorder() -> Arc<Mutex<Vec<(String, bool)>>> {
        let spoken = Arc::new(Mutex::new(Vec::new()));
        let s2 = Arc::clone(&spoken);
        set_speak_override(Some(Box::new(move |t: &str, i: bool| {
            s2.lock().push((t.to_string(), i));
        })));
        spoken
    }

    fn tap_view() -> Vec<Value> {
        SPEECH_TAP
            .lock()
            .entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "text": e.text, "source": e.source, "interrupt": e.interrupt,
                    "excluded": e.excluded, "spoken": e.spoken, "reason": e.reason,
                })
            })
            .collect()
    }

    #[test]
    fn normalize_matches_python() {
        let fx = load("speech_out");
        for c in fx["normalize"].as_array().unwrap() {
            let got = normalize_speech_value(&c["in"], "fixture");
            let want = c["out"].as_str().map(str::to_string);
            assert_eq!(got, want, "normalize({})", c["in"]);
        }
    }

    #[test]
    fn say_matches_python() {
        let _g = TEST_SERIAL.lock();
        let fx = load("speech_out");
        let spoken = recorder();
        for c in fx["say"].as_array().unwrap() {
            reset_for_tests();
            spoken.lock().clear();
            let name = c["name"].as_str().unwrap();
            set_loading_for_tests(
                c["loading"].as_bool().unwrap_or(false),
                c["settling"].as_bool().unwrap_or(false),
            );
            set_command_context(c["command_context"].as_bool().unwrap_or(false));
            if c["protected"].as_bool().unwrap_or(false) {
                set_speech_protected_until(monotonic() + 1000.0);
            }
            say_value(
                &c["text"],
                c["interrupt"].as_bool().unwrap_or(true),
                c["exclude"].as_bool().unwrap_or(false),
                c["source"].as_str().unwrap_or("python"),
            );
            let got_spoken: Vec<Value> = spoken
                .lock()
                .iter()
                .map(|(t, i)| serde_json::json!([t, i]))
                .collect();
            assert_eq!(Value::Array(got_spoken), c["spoken"], "{name}: spoken");
            assert_eq!(Value::Array(tap_view()), c["tap"], "{name}: tap");
            let buf: Vec<Value> = speech_buffer_snapshot().into_iter().map(Value::from).collect();
            assert_eq!(Value::Array(buf), c["buffer"], "{name}: buffer");
        }
        reset_for_tests();
        for i in 0..105 {
            say_plain(&format!("line {i}"));
        }
        let buf: Vec<Value> = speech_buffer_snapshot().into_iter().map(Value::from).collect();
        assert_eq!(Value::Array(buf), fx["buffer_overflow"]);
        assert_eq!(speech_buffer_recent(1).as_deref(), Some("line 104"));
        assert_eq!(speech_buffer_recent(101), None);
        set_speak_override(None);
        reset_for_tests();
    }

    fn query(q: &Value) -> SpeechLog {
        let last_n = match q.get("last_n") {
            None => Some(50),
            Some(Value::Null) => None,
            Some(v) => v.as_i64(),
        };
        get_speech_log(
            q.get("since_seq").and_then(Value::as_i64),
            last_n,
            q.get("source").and_then(Value::as_str),
            q.get("contains").and_then(Value::as_str),
            q.get("spoken_only").and_then(Value::as_bool).unwrap_or(false),
        )
    }

    fn check(out: &SpeechLog, want: &Value, label: &str) {
        let seqs: Vec<u64> = out.entries.iter().map(|e| e.seq).collect();
        let want_seqs: Vec<u64> = want["seqs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        assert_eq!(seqs, want_seqs, "{label}");
        assert_eq!(out.next_seq, want["next_seq"].as_u64().unwrap(), "{label}");
        assert_eq!(out.dropped, want["dropped"].as_bool().unwrap(), "{label}");
    }

    #[test]
    fn speech_log_matches_python() {
        let _g = TEST_SERIAL.lock();
        let fx = load("speech_out");
        reset_for_tests();
        for i in 0..30u64 {
            let text = format!("Entry {i} {}", if i % 3 == 0 { "Alpha" } else { "beta" });
            let source = if i % 2 == 0 { "python" } else { "ui_bridge" };
            let reason = if i % 7 != 0 { None } else { Some("loading_suppressed") };
            speech_tap_record(&text, source, i % 4 != 0, i % 5 == 0, i % 7 != 0, reason);
        }
        let all = get_speech_log(None, Some(0), None, None, false);
        let got: Vec<Value> = all
            .entries
            .iter()
            .map(|e| {
                serde_json::json!({"seq": e.seq, "text": e.text, "source": e.source,
                    "interrupt": e.interrupt, "excluded": e.excluded, "spoken": e.spoken,
                    "reason": e.reason})
            })
            .collect();
        assert_eq!(Value::Array(got), fx["log_entries"]);
        for c in fx["log_queries"].as_array().unwrap() {
            check(&query(&c["q"]), &c["out"], &c["q"].to_string());
        }
        reset_for_tests();
        for i in 0..1005 {
            speech_tap_record(&format!("o{i}"), "python", true, false, true, None);
        }
        for c in fx["log_overflow"].as_array().unwrap() {
            check(&query(&c["q"]), &c["out"], &c["q"].to_string());
        }
        reset_for_tests();
    }

    #[test]
    fn loading_lifecycle_settles_and_announces() {
        let _g = TEST_SERIAL.lock();
        reset_for_tests();
        let spoken = recorder();
        on_loading_state_changed(true, " Main menu ");
        assert!(loading_or_settling());
        assert!(take_telemetry_baseline_pending());
        assert!(!take_telemetry_baseline_pending());
        // A second "active" is ignored.
        on_loading_state_changed(true, "");
        say_plain("suppressed");
        assert!(queue_loading_pending_vehicle("Pickup"));
        on_loading_state_changed(false, "Garage");
        assert!(loading_or_settling());
        // An "inactive" while settling is ignored too.
        on_loading_state_changed(false, "Other");
        std::thread::sleep(std::time::Duration::from_millis(1400));
        assert!(!loading_or_settling());
        let got = spoken.lock().clone();
        assert_eq!(
            got,
            vec![("Loading".to_string(), true), ("Pickup".to_string(), true)]
        );
        assert!(speech_protected_until() > monotonic());
        let log = get_speech_log(None, Some(0), None, None, false);
        let reasons: Vec<Option<String>> = log.entries.iter().map(|e| e.reason.clone()).collect();
        assert_eq!(reasons, vec![None, Some("loading_suppressed".into()), None]);
        set_speak_override(None);
        reset_for_tests();
    }

    #[test]
    fn repr_matches_python_for_log_lines() {
        assert_eq!(py_repr_str("x"), "'x'");
        assert_eq!(py_repr_str("it's"), "\"it's\"");
        assert_eq!(py_repr_str("a\nb"), "'a\\nb'");
    }
}
