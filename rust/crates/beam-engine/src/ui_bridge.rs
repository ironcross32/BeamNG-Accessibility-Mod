//! Owner: E9. `nvda_ws_speaker.py`: the WebSocket server on 8765, the GE-Lua TCP relay on
//! 8766, `broadcast`, the `register_*_callback` hooks and `bnvda_debug_enabled`.
//!
//! Embedded as `start_server_in_thread(speak)`. One current-thread tokio runtime on the
//! `bnvda-ws` thread plays the asyncio loop: every message from every client is handled on
//! it in arrival order, and the callbacks run on it synchronously, exactly as the Python
//! handlers block the loop. Messages are decoded with Python's `json` semantics
//! (`beam_road::pyjson`), so a malformed or oddly typed message fails where the Python's
//! fails.

use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use beam_base::pyfmt::py_strip;
use beam_road::pyjson::{self, DumpOpts, PyErr, Value as PyJson, py_eq, py_max, py_str_repr};
use parking_lot::{Mutex, RwLock};
use regex::Regex;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc, watch};

use crate::convert::{json_to_pyjson, pyjson_to_json};
use crate::timer::monotonic;

/// The speak function the bridge calls (`speak_func(text, interrupt=True)`).
pub type SpeakFunc = Arc<dyn Fn(&str, bool) + Send + Sync>;
/// `stop()` returned by `start_server_in_thread`.
pub type StopFn = Box<dyn FnOnce() + Send>;

pub type DomDumpCallback = Arc<dyn Fn(Vec<String>) + Send + Sync>;
/// `(active, focus_text)`.
pub type LoadingStateCallback = Arc<dyn Fn(bool, &str) + Send + Sync>;
pub type SettingsRequestCallback = Arc<dyn Fn() + Send + Sync>;
/// `(context, title)`.
pub type ScreenContextCallback = Arc<dyn Fn(&str, &str) + Send + Sync>;
/// `(lines, title, code, sentence)`.
pub type PageTextCallback = Arc<dyn Fn(Vec<String>, &str, &str, &str) + Send + Sync>;
/// An allowlisted controller action name.
pub type AccessibilityActionCallback = Arc<dyn Fn(&str) + Send + Sync>;
/// A JSON event payload.
pub type JsonCallback = Arc<dyn Fn(&Value) + Send + Sync>;

/// `os.getenv("BNVDA_HOST", "127.0.0.1")` and the two port defaults.
pub const HOST_DEFAULT: &str = "127.0.0.1";
pub const WS_PORT_DEFAULT: &str = "8765";
pub const TCP_PORT_DEFAULT: &str = "8766";

/// Suppress an identical interrupting repeat only inside this window. Matches the
/// same-text gate the UI runtime already applies (bnvdaRuntime.js scheduleSpeak).
/// Without an expiry, leaving a list row and coming back to it stayed silent forever,
/// because the text still equalled the last thing spoken.
pub const SPEAK_DEDUP_S: f64 = 0.4;

/// `asyncio.start_server(..., limit=16 * 1024 * 1024)`: the relay's line-length limit.
pub const TCP_LINE_LIMIT: usize = 16 * 1024 * 1024;

/// aiohttp `WebSocketResponse(heartbeat=30)`: ping after 30 s of silence, give up 15 s later.
const WS_HEARTBEAT_S: f64 = 30.0;
/// aiohttp's default `max_msg_size`.
const WS_MAX_MSG_SIZE: usize = 4 * 1024 * 1024;

/// The six controller-menu commands `accessibility_action` may carry.
pub const ACCESSIBILITY_ACTIONS: [&str; 6] = [
    "status_up",
    "status_down",
    "status_repeat",
    "next_menu",
    "previous_menu",
    "activate",
];

/// The `_*_CALLBACK` globals.
#[derive(Default, Clone)]
pub struct Callbacks {
    pub dom_dump: Option<DomDumpCallback>,
    pub loading_state: Option<LoadingStateCallback>,
    pub settings_request: Option<SettingsRequestCallback>,
    pub screen_context: Option<ScreenContextCallback>,
    pub page_text: Option<PageTextCallback>,
    pub accessibility_action: Option<AccessibilityActionCallback>,
    pub challenge_event: Option<JsonCallback>,
    pub drift_event: Option<JsonCallback>,
}

pub static CALLBACKS: RwLock<Callbacks> = RwLock::new(Callbacks {
    dom_dump: None,
    loading_state: None,
    settings_request: None,
    screen_context: None,
    page_text: None,
    accessibility_action: None,
    challenge_event: None,
    drift_event: None,
});

/// `_BNVDA_DEBUG` (window.BNVDA_DEBUG mirrored from the UI runtime). The UI runtime
/// reports it whenever it changes, so debug output anywhere can be switched on at runtime
/// from the accessible console rather than needing a relaunch.
pub static BNVDA_DEBUG: AtomicBool = AtomicBool::new(false);

pub fn register_dom_dump_callback(cb: DomDumpCallback) {
    CALLBACKS.write().dom_dump = Some(cb);
}
/// Register the application-level loading lifecycle handler.
pub fn register_loading_state_callback(cb: LoadingStateCallback) {
    CALLBACKS.write().loading_state = Some(cb);
}
/// Register the handler that answers the UI runtime's settings pull.
///
/// The runtime asks on every transport activation rather than relying on a push: beamtel
/// broadcasts whenever the config changes, but the first such broadcast happens long
/// before the Lua bridge connects and would be dropped on the floor.
pub fn register_settings_request_callback(cb: SettingsRequestCallback) {
    CALLBACKS.write().settings_request = Some(cb);
}
/// Register the handler for the UI runtime's screen latch.
///
/// The runtime pushes on entering and on leaving a screen it can read on demand, and
/// again on every transport pong -- beamtel can restart while the UI stays up, and no
/// route change would follow to refill the latch.
pub fn register_screen_context_callback(cb: ScreenContextCallback) {
    CALLBACKS.write().screen_context = Some(cb);
}
/// Register the handler for a page-text reply (lines, or a failure code).
pub fn register_page_text_callback(cb: PageTextCallback) {
    CALLBACKS.write().page_text = Some(cb);
}
/// Register the application handler for allowlisted gameplay actions.
pub fn register_accessibility_action_callback(cb: AccessibilityActionCallback) {
    CALLBACKS.write().accessibility_action = Some(cb);
}
/// Register the application handler for native mission lifecycle events.
pub fn register_challenge_event_callback(cb: JsonCallback) {
    CALLBACKS.write().challenge_event = Some(cb);
}
/// Register the handler for free-roam drift scoring events (driftAccessible.lua).
pub fn register_drift_event_callback(cb: JsonCallback) {
    CALLBACKS.write().drift_event = Some(cb);
}

/// `bnvda_debug_enabled()`: true while window.BNVDA_DEBUG is set in the game's UI JS
/// context. Read it live at each use site rather than latching it -- the flag is flipped
/// from the accessible console long after startup, and a UI reload resets it.
pub fn bnvda_debug_enabled() -> bool {
    BNVDA_DEBUG.load(Ordering::SeqCst)
}

// ---- Global state ---------------------------------------------------------------------------

/// `_SPEAK_FUNC`, `_SPEAK_LAST`, `_SPEAK_LAST_TS`.
struct SpeakState {
    func: Option<SpeakFunc>,
    last: String,
    last_ts: f64,
}

static SPEAK: Mutex<SpeakState> = Mutex::new(SpeakState {
    func: None,
    last: String::new(),
    last_ts: 0.0,
});

/// `_HOVER_TASK`, `_HOVER_TOKEN`.
struct HoverState {
    task: Option<tokio::task::AbortHandle>,
    token: Option<PyJson>,
}

static HOVER: Mutex<HoverState> = Mutex::new(HoverState {
    task: None,
    token: None,
});

/// One connected client's outgoing queue.
enum Outgoing {
    Text(String),
}

/// The running server: `_LOOP`, `_CLIENTS`, `_TCP_CLIENTS`.
struct Shared {
    ws: Mutex<Vec<(u64, mpsc::UnboundedSender<Outgoing>)>>,
    tcp: Mutex<Vec<(u64, mpsc::UnboundedSender<Vec<u8>>)>>,
    shutdown: watch::Receiver<bool>,
    active: AtomicUsize,
    idle: Notify,
}

static BRIDGE: RwLock<Option<Arc<Shared>>> = RwLock::new(None);
/// `_LOOP` for `hover_on`: set when the thread starts, before binding, as in the Python.
static LOOP: RwLock<Option<tokio::runtime::Handle>> = RwLock::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl Shared {
    fn ws_count(&self) -> usize {
        self.ws.lock().len()
    }
    fn remove_ws(&self, id: u64) {
        self.ws.lock().retain(|(i, _)| *i != id);
    }
    fn remove_tcp(&self, id: u64) {
        self.tcp.lock().retain(|(i, _)| *i != id);
    }
    fn enter(&self) {
        self.active.fetch_add(1, Ordering::SeqCst);
    }
    fn leave(&self) {
        if self.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.idle.notify_waiters();
        }
    }
}

// Safety net for raw CSS that can leak from the UI hook when a handler reads the
// textContent of a non-rendered <style> element (happens when the game UI is toggled
// off). The app.js side filters these too; this is a backstop.
static CSS_LEAK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[#.\w-]+\s*\{[^}]*:[^}]*;|\}\s*[#.\w-]+\s*\{").expect("CSS leak pattern")
});

fn looks_like_css(t: &str) -> bool {
    CSS_LEAK_RE.is_match(t)
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_string())
}

/// Run a callback the way the Python's `try: ... except Exception as e:` does, logging
/// `"<name> callback error: <e>"` for whatever it raises.
fn guarded(name: &str, f: impl FnOnce()) {
    if let Err(p) = catch_unwind(AssertUnwindSafe(f)) {
        log::error!("{name} callback error: {}", panic_text(&*p));
    }
}

// ---- Core helpers ---------------------------------------------------------------------------

/// `engine_offer(text, interrupt=True)`.
pub fn engine_offer(text: &str, interrupt: bool) {
    let t = py_strip(text);
    if t.is_empty() {
        return;
    }
    if looks_like_css(t) {
        log::info!("[bnvda] Suppressed CSS-like text from UI hook.");
        return;
    }
    // Only dedup against the last utterance for interrupting speech. A queued follow-up
    // (interrupt=False, e.g. a tuning hint) is intentionally a second utterance and must
    // not be swallowed by this guard.
    let now = monotonic();
    let func = {
        let mut sp = SPEAK.lock();
        if interrupt && t == sp.last && (now - sp.last_ts) < SPEAK_DEDUP_S {
            return;
        }
        sp.last = t.to_string();
        sp.last_ts = now;
        sp.func.clone()
    };
    match func {
        None => log::error!("speak error: 'NoneType' object is not callable"),
        Some(f) => {
            if let Err(p) = catch_unwind(AssertUnwindSafe(|| f(t, interrupt))) {
                log::error!("speak error: {}", panic_text(&*p));
            }
        }
    }
}

/// `hover_cancel()`.
pub fn hover_cancel() {
    let task = {
        let mut h = HOVER.lock();
        h.token = None;
        h.task.take()
    };
    if let Some(task) = task {
        task.abort();
    }
}

/// `hover_on(text, item_id=None, delay_ms=1000)`.
///
/// `hover_cancel()` clears `_HOVER_TOKEN` right after it is set, so the token the task
/// later compares against is always `None` and a hover never speaks. Ported as it is.
pub fn hover_on(text: &PyJson, item_id: &PyJson, delay_ms: &PyJson) -> Result<(), PyErr> {
    let delay_s = match delay_ms.py_float() {
        Ok(f) => py_max(0.05, f / 1000.0),
        Err(_) => 1.0,
    };
    let token = if !item_id.is_null() {
        item_id.clone()
    } else if text.truthy() {
        text.clone()
    } else {
        PyJson::Str(String::new())
    };
    HOVER.lock().token = Some(token.clone());
    hover_cancel();

    let payload = text_or_empty_strip(text)?;
    let Some(handle) = LOOP.read().clone() else {
        return Err(PyErr::AttributeError(
            "'NoneType' object has no attribute 'create_task'".to_string(),
        ));
    };
    let task = handle.spawn(async move {
        sleep_secs(delay_s).await;
        let same = matches!(&HOVER.lock().token, Some(cur) if py_eq(&token, cur));
        if same && !payload.is_empty() {
            engine_offer(py_strip(&payload), true);
        }
    });
    HOVER.lock().task = Some(task.abort_handle());
    Ok(())
}

/// `(text or "").strip()`.
fn text_or_empty_strip(text: &PyJson) -> Result<String, PyErr> {
    if !text.truthy() {
        return Ok(String::new());
    }
    match text {
        PyJson::Str(s) => Ok(py_strip(s).to_string()),
        other => Err(PyErr::AttributeError(format!(
            "'{}' object has no attribute 'strip'",
            other.type_name()
        ))),
    }
}

/// `await asyncio.sleep(seconds)`, including the infinite sleep a huge delay asks for.
async fn sleep_secs(seconds: f64) {
    match Duration::try_from_secs_f64(seconds) {
        Ok(d) => tokio::time::sleep(d).await,
        Err(_) if seconds.is_nan() || seconds <= 0.0 => {}
        Err(_) => std::future::pending::<()>().await,
    }
}

/// `str(x or "")`.
fn str_or_empty(v: Option<&PyJson>) -> String {
    match v {
        Some(v) if v.truthy() => v.py_str(),
        _ => String::new(),
    }
}

/// What a callback that iterates `lines` sees, for the dom-dump hand-off.
fn iter_strings(v: &PyJson) -> Vec<String> {
    match v {
        PyJson::List(items) => items.iter().map(PyJson::py_str).collect(),
        PyJson::Str(s) => s.chars().map(String::from).collect(),
        PyJson::Dict(d) => d.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// `getattr(logger, level, logger.info)(msg)` for the `log` message.
fn log_at(level: &str, msg: &str) -> Result<(), PyErr> {
    let not_callable =
        |t: &str| Err(PyErr::TypeError(format!("'{t}' object is not callable")));
    match level {
        "debug" => log::debug!("{msg}"),
        "info" => log::info!("{msg}"),
        "warning" | "warn" => log::warn!("{msg}"),
        "error" => log::error!("{msg}"),
        // `logger.exception` outside an except block appends the empty traceback.
        "exception" => log::error!("{msg}\nNoneType: None"),
        // CRITICAL has no `log` level of its own; it is written at ERROR.
        "critical" | "fatal" => log::error!("{msg}"),
        "log" => {
            return Err(PyErr::TypeError(
                "Logger.log() missing 1 required positional argument: 'msg'".to_string(),
            ));
        }
        "_log" => {
            return Err(PyErr::TypeError(
                "Logger._log() missing 1 required positional argument: 'args'".to_string(),
            ));
        }
        // Filterer.filter(record) with no filters installed: no output, no error.
        "filter" => {}
        "handle" => {
            return Err(PyErr::AttributeError(
                "'str' object has no attribute 'levelno'".to_string(),
            ));
        }
        "name" => return not_callable("str"),
        "level" => return not_callable("int"),
        "parent" | "root" => return not_callable("RootLogger"),
        "propagate" | "disabled" => return not_callable("bool"),
        "handlers" | "filters" => return not_callable("list"),
        "manager" => return not_callable("Manager"),
        "_cache" => return not_callable("dict"),
        _ => log::info!("{msg}"),
    }
    Ok(())
}

// ---- Message handlers (called by WebSocket and the TCP relay) -------------------------------

/// `handle_ws_message(data)`. `Err` is the exception the Python lets escape to its
/// caller (the WebSocket handler logs it; the TCP relay drops the connection).
pub fn handle_ws_message(data: &PyJson) -> Result<(), PyErr> {
    if data.as_dict().is_none() {
        return Ok(());
    }
    let msg_type = data.get("type");
    let is = |name: &str| msg_type.is_some_and(|t| t.eq_str(name));

    if is("speak") {
        let mut text_val = data.get("text").cloned().unwrap_or(PyJson::Str(String::new()));
        if text_val.as_str().is_none() {
            log::warn!(
                "[UNSPEAKABLE] speak text is {}, not str: {}",
                text_val.type_name(),
                text_val.py_repr()
            );
            text_val = PyJson::Str(if text_val.is_null() {
                String::new()
            } else {
                text_val.py_str()
            });
        }
        // interrupt defaults to True; a queued follow-up sets interrupt:false so it
        // plays after the current utterance instead of cutting it off.
        let interrupt = data.get("interrupt").is_none_or(PyJson::truthy);
        engine_offer(text_val.as_str().unwrap_or(""), interrupt);
    } else if is("log") {
        let level = data
            .get("level")
            .map(PyJson::py_str)
            .unwrap_or_else(|| "INFO".to_string())
            .to_lowercase();
        let msg = data.get("msg").map(PyJson::py_str).unwrap_or_default();
        log_at(&level, &msg)?;
    } else if is("debug_state") {
        let new_state = data.get("enabled").is_some_and(PyJson::truthy);
        if new_state != BNVDA_DEBUG.load(Ordering::SeqCst) {
            BNVDA_DEBUG.store(new_state, Ordering::SeqCst);
            log::info!(
                "[bnvda] BNVDA_DEBUG {}",
                if new_state { "enabled" } else { "disabled" }
            );
        }
    } else if is("hover") {
        let empty = PyJson::Str(String::new());
        hover_on(
            data.get("text").unwrap_or(&empty),
            data.get("id").unwrap_or(&PyJson::Null),
            data.get("delay_ms").unwrap_or(&PyJson::Null),
        )?;
    } else if is("hover_cancel") {
        hover_cancel();
    } else if is("dom_dump_result") {
        let cb = CALLBACKS.read().dom_dump.clone();
        if let Some(cb) = cb {
            let lines = data.get("lines").map(iter_strings).unwrap_or_default();
            guarded("dom_dump_result", || cb(lines));
        }
    } else if is("settings_request") {
        let cb = CALLBACKS.read().settings_request.clone();
        if let Some(cb) = cb {
            guarded("settings_request", || cb());
        }
    } else if is("loading_state") {
        let cb = CALLBACKS.read().loading_state.clone();
        if let Some(cb) = cb {
            let active = data.get("active").is_some_and(PyJson::truthy);
            let focus = str_or_empty(data.get("focusText"));
            guarded("loading_state", || cb(active, &focus));
        }
    } else if is("screen_context") {
        let cb = CALLBACKS.read().screen_context.clone();
        if let Some(cb) = cb {
            let context = str_or_empty(data.get("context"));
            let title = str_or_empty(data.get("title"));
            guarded("screen_context", || cb(&context, &title));
        }
    } else if is("page_text") {
        let cb = CALLBACKS.read().page_text.clone();
        if let Some(cb) = cb {
            let lines: Vec<String> = match data.get("lines") {
                Some(PyJson::List(items)) => items.iter().map(PyJson::py_str).collect(),
                _ => Vec::new(),
            };
            let title = str_or_empty(data.get("title"));
            let code = str_or_empty(data.get("code"));
            let sentence = str_or_empty(data.get("sentence"));
            guarded("page_text", || cb(lines, &title, &code, &sentence));
        }
    } else if is("accessibility_action") {
        let action = data.get("action").cloned().unwrap_or(PyJson::Null);
        if !action.in_str_set(&ACCESSIBILITY_ACTIONS)? {
            log::warn!(
                "[bnvda] Ignoring unknown accessibility action: {}",
                action.py_repr()
            );
            return Ok(());
        }
        let cb = CALLBACKS.read().accessibility_action.clone();
        if let Some(cb) = cb {
            let name = action.as_str().unwrap_or("").to_string();
            guarded("accessibility_action", || cb(&name));
        }
    } else if is("challenge_event") {
        let cb = CALLBACKS.read().challenge_event.clone();
        if let Some(cb) = cb {
            let payload = pyjson_to_json(data);
            guarded("challenge_event", || cb(&payload));
        }
    } else if is("drift_event") {
        let cb = CALLBACKS.read().drift_event.clone();
        if let Some(cb) = cb {
            let payload = pyjson_to_json(data);
            guarded("drift_event", || cb(&payload));
        }
    }
    Ok(())
}

/// The reply to a WebSocket `transport_ping` (`ws.send_json`, Python's default separators).
pub const WS_PONG: &str = r#"{"type": "transport_pong", "transport": "direct-websocket"}"#;
/// The reply to a relay `transport_ping`, written as literal bytes.
pub const TCP_PONG: &[u8] = b"{\"type\":\"transport_pong\",\"transport\":\"lua-tcp\"}\n";

/// The body of `ws_handler`'s `async for msg in ws` for one TEXT message: the reply to
/// send, if any. Every failure is logged and swallowed, as the Python's `try` does.
pub fn ws_process_text(text: &str) -> Option<String> {
    let data = match pyjson::loads(text) {
        Ok(d) => d,
        Err(e) => {
            log::warn!(
                "[bnvda] Invalid WebSocket message: {}",
                json_error_text(text, &e.0)
            );
            return None;
        }
    };
    match data.py_get("type") {
        Err(e) => {
            log::warn!("[bnvda] Invalid WebSocket message: {e}");
            None
        }
        Ok(Some(t)) if t.eq_str("transport_ping") => Some(WS_PONG.to_string()),
        Ok(_) => {
            if let Err(e) = handle_ws_message(&data) {
                log::warn!("[bnvda] Invalid WebSocket message: {e}");
            }
            None
        }
    }
}

/// One line read by the TCP relay. `Ok(Some(bytes))` is a reply to write; `Err` is an
/// exception the relay's inner `except` does not catch, which ends the connection.
pub fn tcp_process_line(line: &[u8]) -> Result<Option<Vec<u8>>, PyErr> {
    let text = match std::str::from_utf8(line) {
        Ok(t) => t,
        Err(e) => {
            log::warn!(
                "[bnvda] Invalid Lua TCP relay message: {}",
                utf8_error_text(line, &e)
            );
            return Ok(None);
        }
    };
    let data = match pyjson::loads(text) {
        Ok(d) => d,
        Err(e) => {
            log::warn!(
                "[bnvda] Invalid Lua TCP relay message: {}",
                json_error_text(text, &e.0)
            );
            return Ok(None);
        }
    };
    if data.py_get("type")?.is_some_and(|t| t.eq_str("transport_ping")) {
        return Ok(Some(TCP_PONG.to_vec()));
    }
    handle_ws_message(&data)?;
    Ok(None)
}

/// `str(JSONDecodeError)`: `"<msg>: line L column C (char N)"`.
fn json_error_text(doc: &str, raw: &str) -> String {
    let Some((what, pos)) = raw.rsplit_once(": char ") else {
        return raw.to_string();
    };
    let Ok(pos) = pos.parse::<usize>() else {
        return raw.to_string();
    };
    let before: Vec<char> = doc.chars().take(pos).collect();
    let lineno = before.iter().filter(|c| **c == '\n').count() + 1;
    let colno = match before.iter().rposition(|c| *c == '\n') {
        Some(nl) => pos - nl,
        None => pos + 1,
    };
    format!("{what}: line {lineno} column {colno} (char {pos})")
}

/// `str(UnicodeDecodeError)` for `bytes.decode("utf-8")`.
fn utf8_error_text(bytes: &[u8], e: &std::str::Utf8Error) -> String {
    let pos = e.valid_up_to();
    match e.error_len() {
        Some(_) => {
            let b = bytes[pos];
            let reason = if (0x80..0xc2).contains(&b) || b >= 0xf5 {
                "invalid start byte"
            } else {
                "invalid continuation byte"
            };
            format!("'utf-8' codec can't decode byte 0x{b:02x} in position {pos}: {reason}")
        }
        None => {
            let end = bytes.len() - 1;
            if end == pos {
                format!(
                    "'utf-8' codec can't decode byte 0x{:02x} in position {pos}: unexpected end of data",
                    bytes[pos]
                )
            } else {
                format!(
                    "'utf-8' codec can't decode bytes in position {pos}-{end}: unexpected end of data"
                )
            }
        }
    }
}

// ---- Broadcast to connected clients ---------------------------------------------------------

/// `json.dumps(msg)` with Python's defaults.
fn py_dumps(msg: &Value) -> String {
    pyjson::dumps(&json_to_pyjson(msg), &DumpOpts::default_seps()).unwrap_or_default()
}

/// `broadcast(msg_dict)`: send JSON to every WebSocket and TCP client (thread-safe; a
/// no-op with no clients).
pub fn broadcast(msg: &Value) {
    let Some(shared) = BRIDGE.read().clone() else {
        return;
    };
    if shared.ws.lock().is_empty() && shared.tcp.lock().is_empty() {
        return;
    }
    let payload = py_dumps(msg);
    for (_, tx) in shared.ws.lock().iter() {
        let _ = tx.send(Outgoing::Text(payload.clone()));
    }
    let mut wire = payload.into_bytes();
    wire.push(b'\n');
    let dead: Vec<u64> = shared
        .tcp
        .lock()
        .iter()
        .filter(|(_, tx)| tx.send(wire.clone()).is_err())
        .map(|(id, _)| *id)
        .collect();
    for id in dead {
        shared.remove_tcp(id);
    }
}

// ---- WebSocket server -----------------------------------------------------------------------

/// `%r` of an optional header.
fn header_repr(headers: &HeaderMap, name: header::HeaderName) -> String {
    match headers.get(name) {
        Some(v) => py_str_repr(&String::from_utf8_lossy(v.as_bytes())),
        None => "None".to_string(),
    }
}

fn header_text(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
}

fn text_response(status: StatusCode, body: String) -> Response {
    let mut resp = (status, body).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp
}

/// aiohttp's `WebSocketResponse._handshake` checks, in its order, with its texts.
fn handshake_error(headers: &HeaderMap) -> Option<String> {
    let upgrade = header_text(headers, header::UPGRADE);
    if py_strip(&upgrade.as_deref().unwrap_or("").to_lowercase()) != "websocket" {
        return Some(format!(
            "No WebSocket UPGRADE hdr: {}\n Can \"Upgrade\" only to \"WebSocket\".",
            upgrade.unwrap_or_else(|| "None".to_string())
        ));
    }
    let connection = header_text(headers, header::CONNECTION);
    if !connection
        .as_deref()
        .unwrap_or("")
        .to_lowercase()
        .contains("upgrade")
    {
        return Some(format!(
            "No CONNECTION upgrade hdr: {}",
            connection.unwrap_or_else(|| "None".to_string())
        ));
    }
    let version = header_text(headers, header::SEC_WEBSOCKET_VERSION).unwrap_or_default();
    if !matches!(version.as_str(), "13" | "8" | "7") {
        return Some(format!("Unsupported version: {version}"));
    }
    let key = header_text(headers, header::SEC_WEBSOCKET_KEY);
    let key_ok = key
        .as_deref()
        .filter(|k| !k.is_empty())
        .and_then(beam_base::secretstore::py_b64decode)
        .is_some_and(|raw| raw.len() == 16);
    if !key_ok {
        let repr = match &key {
            Some(k) => py_str_repr(k),
            None => "None".to_string(),
        };
        return Some(format!("Handshake error: {repr}"));
    }
    None
}

async fn ws_handler(
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    log::info!(
        "[bnvda] WebSocket handshake attempt from {} origin={}",
        remote.ip(),
        header_repr(&headers, header::ORIGIN)
    );
    if let Some(text) = handshake_error(&headers) {
        log::error!("[bnvda] WebSocket handshake failed: Bad Request");
        return text_response(StatusCode::BAD_REQUEST, text);
    }
    let ws = match upgrade {
        Ok(ws) => ws,
        Err(rejection) => {
            log::error!("[bnvda] WebSocket handshake failed: {rejection}");
            return rejection.into_response();
        }
    };
    let Some(shared) = BRIDGE.read().clone() else {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, String::new());
    };
    let mut resp = ws
        .max_message_size(WS_MAX_MSG_SIZE)
        .on_upgrade(move |socket| ws_session(socket, shared));
    let h = resp.headers_mut();
    h.insert("Access-Control-Allow-Origin", HeaderValue::from_static("*"));
    h.insert(
        "Access-Control-Allow-Private-Network",
        HeaderValue::from_static("true"),
    );
    resp
}

/// The connected half of `ws_handler`.
async fn ws_session(mut socket: WebSocket, shared: Arc<Shared>) {
    shared.enter();
    let (tx, mut rx) = mpsc::unbounded_channel::<Outgoing>();
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    shared.ws.lock().push((id, tx));
    log::info!("WebSocket client connected.");
    let mut shutdown = shared.shutdown.clone();
    let heartbeat = Duration::from_secs_f64(WS_HEARTBEAT_S);
    let mut ping_at = tokio::time::Instant::now() + heartbeat;
    let mut pong_deadline: Option<tokio::time::Instant> = None;
    loop {
        let pong_wait = async {
            match pong_deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            msg = socket.recv() => {
                let now = tokio::time::Instant::now();
                ping_at = now + heartbeat;
                pong_deadline = None;
                match msg {
                    None => break,
                    Some(Ok(Message::Text(text))) => {
                        if let Some(reply) = ws_process_text(text.as_str())
                            && let Err(e) = socket.send(Message::Text(reply.into())).await
                        {
                            log::warn!("[bnvda] Invalid WebSocket message: {e}");
                        }
                    }
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        log::error!("ws error: {e}");
                        break;
                    }
                }
            }
            out = rx.recv() => match out {
                Some(Outgoing::Text(payload)) => {
                    let _ = socket.send(Message::Text(payload.into())).await;
                }
                None => break,
            },
            _ = tokio::time::sleep_until(ping_at), if pong_deadline.is_none() => {
                let _ = socket.send(Message::Ping(Vec::new().into())).await;
                pong_deadline = Some(tokio::time::Instant::now() + heartbeat / 2);
            }
            _ = pong_wait => break,
            _ = shutdown.changed() => break,
        }
    }
    shared.remove_ws(id);
    log::info!("WebSocket client disconnected.");
    let _ = socket.send(Message::Close(None)).await;
    shared.leave();
}

/// A CEF-readable probe which is deliberately independent of WebSockets.
async fn health_handler(
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    log::info!(
        "[bnvda] HTTP health probe from {} origin={}",
        remote.ip(),
        header_repr(&headers, header::ORIGIN)
    );
    let clients = BRIDGE.read().as_ref().map_or(0, |s| s.ws_count());
    let body = format!(
        "{{\"ok\": true, \"service\": \"bnvda\", \"websocket_clients\": {clients}}}"
    );
    let mut resp = (StatusCode::OK, body).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    h.insert("Access-Control-Allow-Origin", HeaderValue::from_static("*"));
    h.insert(
        "Access-Control-Allow-Private-Network",
        HeaderValue::from_static("true"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// aiohttp's answer for a method a GET route lacks.
async fn get_only() -> Response {
    let mut resp = text_response(
        StatusCode::METHOD_NOT_ALLOWED,
        "405: Method Not Allowed".to_string(),
    );
    resp.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("GET,HEAD"));
    resp
}

/// aiohttp's answer for a path with no route.
async fn not_found() -> Response {
    text_response(StatusCode::NOT_FOUND, "404: Not Found".to_string())
}

// ---- Lua TCP relay --------------------------------------------------------------------------

/// `StreamReader.readline()` with the relay's limit.
struct LineReader {
    buf: Vec<u8>,
    eof: bool,
}

impl LineReader {
    /// `Ok(None)` at a clean EOF (`readline()` returning `b""`); `Err` is the ValueError
    /// or the socket error.
    async fn read_line(&mut self, rd: &mut tokio::net::tcp::OwnedReadHalf) -> Result<Option<Vec<u8>>, String> {
        loop {
            if let Some(isep) = self.buf.iter().position(|b| *b == b'\n') {
                if isep > TCP_LINE_LIMIT {
                    self.buf.drain(..=isep);
                    return Err("Separator is found, but chunk is longer than limit".into());
                }
                return Ok(Some(self.buf.drain(..=isep).collect()));
            }
            if self.buf.len() > TCP_LINE_LIMIT {
                self.buf.clear();
                return Err("Separator is not found, and chunk exceed the limit".into());
            }
            if self.eof {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(std::mem::take(&mut self.buf)));
            }
            let mut chunk = [0u8; 65536];
            match rd.read(&mut chunk).await {
                Ok(0) => self.eof = true,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

/// `f"{peer}"` for an IPv4 peername tuple.
fn peer_text(peer: &SocketAddr) -> String {
    match peer {
        SocketAddr::V4(a) => format!("('{}', {})", a.ip(), a.port()),
        SocketAddr::V6(a) => format!("('{}', {}, {}, {})", a.ip(), a.port(), a.flowinfo(), a.scope_id()),
    }
}

async fn tcp_session(stream: TcpStream, peer: SocketAddr, shared: Arc<Shared>) {
    shared.enter();
    let peer = peer_text(&peer);
    let (mut rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    shared.tcp.lock().push((id, tx.clone()));
    log::info!("[bnvda] Lua TCP relay connected from {peer}.");

    let writer_shared = Arc::clone(&shared);
    let writer = tokio::spawn(async move {
        while let Some(buf) = rx.recv().await {
            if wr.write_all(&buf).await.is_err() || wr.flush().await.is_err() {
                writer_shared.remove_tcp(id);
                break;
            }
        }
        let _ = wr.shutdown().await;
    });

    let mut shutdown = shared.shutdown.clone();
    let mut reader = LineReader {
        buf: Vec::new(),
        eof: false,
    };
    loop {
        tokio::select! {
            line = reader.read_line(&mut rd) => match line {
                Ok(None) => break,
                Ok(Some(line)) => match tcp_process_line(&line) {
                    Ok(Some(reply)) => {
                        let _ = tx.send(reply);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::error!("[bnvda] Lua TCP relay error: {e}");
                        break;
                    }
                },
                Err(e) => {
                    log::error!("[bnvda] Lua TCP relay error: {e}");
                    break;
                }
            },
            _ = shutdown.changed() => break,
        }
    }
    shared.remove_tcp(id);
    drop(tx);
    let _ = writer.await;
    log::info!("[bnvda] Lua TCP relay disconnected from {peer}.");
    shared.leave();
}

async fn tcp_accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    let mut shutdown = shared.shutdown.clone();
    loop {
        tokio::select! {
            r = listener.accept() => {
                if let Ok((stream, peer)) = r {
                    tokio::spawn(tcp_session(stream, peer, Arc::clone(&shared)));
                }
            }
            _ = shutdown.changed() => break,
        }
    }
}

// ---- Boot / Thread --------------------------------------------------------------------------

/// A started bridge: its stop function and the addresses it bound (`None` if a bind
/// failed, in which case nothing is served, as in the Python).
pub struct BridgeServer {
    pub stop: StopFn,
    pub ws_addr: Option<SocketAddr>,
    pub tcp_addr: Option<SocketAddr>,
}

/// `int(os.getenv(name, default))`.
fn env_port(name: &str, default: &str) -> Result<i64, String> {
    let text = std::env::var(name).unwrap_or_else(|_| default.to_string());
    PyJson::Str(text).py_int().map_err(|e| e.to_string())
}

/// `start_server_in_thread(speak_func)`: returns the stop function. The host and ports
/// come from `BNVDA_HOST`, `BNVDA_WS_PORT` and `BNVDA_TCP_PORT`; a port that is not an
/// integer is the import-time `ValueError` the Python would raise.
pub fn start_server_in_thread(speak_func: SpeakFunc) -> Result<StopFn, String> {
    let host = std::env::var("BNVDA_HOST").unwrap_or_else(|_| HOST_DEFAULT.to_string());
    let ws_port = env_port("BNVDA_WS_PORT", WS_PORT_DEFAULT)?;
    let tcp_port = env_port("BNVDA_TCP_PORT", TCP_PORT_DEFAULT)?;
    let (Ok(ws), Ok(tcp)) = (u16::try_from(ws_port), u16::try_from(tcp_port)) else {
        SPEAK.lock().func = Some(speak_func);
        eprintln!("Exception in thread bnvda-ws: OverflowError: port must be 0-65535.");
        return Ok(Box::new(|| {}));
    };
    Ok(start_server_on(speak_func, &host, ws, tcp).stop)
}

/// [`start_server_in_thread`] on an explicit host and ports (0 picks a free port).
/// Waits for the binds so the bound addresses can be reported.
pub fn start_server_on(speak_func: SpeakFunc, host: &str, ws_port: u16, tcp_port: u16) -> BridgeServer {
    SPEAK.lock().func = Some(speak_func);
    let host = host.to_string();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Option<(SocketAddr, SocketAddr)>>();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let spawned = std::thread::Builder::new()
        .name("bnvda-ws".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("Exception in thread bnvda-ws: {e}");
                    let _ = ready_tx.send(None);
                    return;
                }
            };
            rt.block_on(run_bridge(host, ws_port, tcp_port, ready_tx, shutdown_rx));
            drop(rt);
            let _ = done_tx.send(());
        });
    if let Err(e) = spawned {
        eprintln!("Exception in thread bnvda-ws: {e}");
    }
    let bound = ready_rx.recv().ok().flatten();
    let stop: StopFn = Box::new(move || {
        let _ = shutdown_tx.send(true);
        let _ = done_rx.recv_timeout(Duration::from_millis(1500));
    });
    BridgeServer {
        stop,
        ws_addr: bound.map(|b| b.0),
        tcp_addr: bound.map(|b| b.1),
    }
}

async fn run_bridge(
    host: String,
    ws_port: u16,
    tcp_port: u16,
    ready: std::sync::mpsc::Sender<Option<(SocketAddr, SocketAddr)>>,
    mut shutdown: watch::Receiver<bool>,
) {
    *LOOP.write() = Some(tokio::runtime::Handle::current());
    let ws_listener = match TcpListener::bind((host.as_str(), ws_port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Exception in thread bnvda-ws: OSError: {e}");
            let _ = ready.send(None);
            return;
        }
    };
    log::info!("BNVDA WebSocket server listening on ws://{host}:{ws_port}/");
    let tcp_listener = match TcpListener::bind((host.as_str(), tcp_port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Exception in thread bnvda-ws: OSError: {e}");
            let _ = ready.send(None);
            return;
        }
    };
    log::info!("BNVDA Lua TCP relay listening on {host}:{tcp_port}");
    let (Ok(ws_addr), Ok(tcp_addr)) = (ws_listener.local_addr(), tcp_listener.local_addr()) else {
        let _ = ready.send(None);
        return;
    };

    let shared = Arc::new(Shared {
        ws: Mutex::new(Vec::new()),
        tcp: Mutex::new(Vec::new()),
        shutdown: shutdown.clone(),
        active: AtomicUsize::new(0),
        idle: Notify::new(),
    });
    *BRIDGE.write() = Some(Arc::clone(&shared));
    let _ = ready.send(Some((ws_addr, tcp_addr)));

    let app = Router::new()
        .route("/", get(ws_handler).fallback(get_only))
        .route("/health", get(health_handler).fallback(get_only))
        .fallback(not_found);
    let mut ws_shutdown = shutdown.clone();
    let ws_server = axum::serve(
        ws_listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = ws_shutdown.changed().await;
    });
    let ws_task = tokio::spawn(async move {
        let _ = ws_server.await;
    });
    let tcp_task = tokio::spawn(tcp_accept_loop(tcp_listener, Arc::clone(&shared)));

    let _ = shutdown.changed().await;
    // _shutdown(): close every client, then the servers.
    let wait_idle = async {
        while shared.active.load(Ordering::SeqCst) > 0 {
            let notified = shared.idle.notified();
            if shared.active.load(Ordering::SeqCst) == 0 {
                break;
            }
            notified.await;
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(1), wait_idle).await;
    let _ = tokio::time::timeout(Duration::from_millis(500), tcp_task).await;
    let _ = tokio::time::timeout(Duration::from_millis(500), ws_task).await;
    let mut slot = BRIDGE.write();
    if slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, &shared)) {
        *slot = None;
    }
}

/// Reset the dedup memory, the hover task and the debug flag between test cases.
#[doc(hidden)]
pub fn reset_for_tests() {
    {
        let mut sp = SPEAK.lock();
        sp.last.clear();
        sp.last_ts = 0.0;
    }
    hover_cancel();
    BNVDA_DEBUG.store(false, Ordering::SeqCst);
}

/// Install the speak function without starting a server (tests).
#[doc(hidden)]
pub fn set_speak_func_for_tests(func: Option<SpeakFunc>) {
    SPEAK.lock().func = func;
}

/// Give `hover_on` a loop to schedule on without starting a server (tests).
#[doc(hidden)]
pub fn set_loop_for_tests(handle: Option<tokio::runtime::Handle>) {
    *LOOP.write() = handle;
}
