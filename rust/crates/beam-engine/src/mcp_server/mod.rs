//! Owner: E9. `mcp_server.py`: the MCP automation server over loopback HTTP, which lets
//! an AI agent drive and test the mod: execute Lua in the game, place vehicles into known
//! geometry, read live state, trigger any F9 command, and assert on exactly what the mod
//! announced.
//!
//! Off by default (`mcp_server_enabled`); binds loopback only. It takes everything it needs
//! through [`init`] (`Deps`, carrying exactly what `_run_engine` injects), never from the
//! engine directly.
//!
//! Transport is streamable-HTTP MCP (JSON-RPC 2.0 over POST) with five methods. Two
//! protocol details are load-bearing: a notification is answered with 202 and an empty
//! body (otherwise the client hangs on the handshake), and `GET /mcp` is 405 (no
//! server-initiated stream). No `Mcp-Session-Id` is issued. A tool failure is `isError`
//! content, never a JSON-RPC error: those are for protocol faults.
//!
//! Requests and arguments are handled as Python values (`beam_road::pyjson`), so an
//! argument of an unexpected type fails with the message the Python's would.

mod camera;
mod tools;

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use beam_base::pyfmt::py_f;
use beam_road::pyjson::{self, Dict, DumpOpts, PyErr, Value as PyJson};
use indexmap::IndexMap;
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tokio::sync::watch;

use crate::f9_commands::HelpKey;
use crate::speech_out::SpeechLog;
use crate::timer::{Event, STOP, monotonic, time_time};

pub use camera::{CAM_LIST_LUA, CAM_READ_LUA, cam_place_chunk};

pub const HOST: &str = "127.0.0.1";
/// Clear of the 4444-4489 mod block.
pub const DEFAULT_PORT: u16 = 4481;

/// The five methods implemented here are identical across all three revisions, so the
/// client's own is echoed back rather than forcing one.
pub const SUPPORTED_PROTOCOLS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];
pub const FALLBACK_PROTOCOL: &str = "2025-06-18";
pub const SERVER_NAME: &str = "beamtel";

/// consoleAccessible.lua's own datagram cap, mirrored here because reassembly needs it to
/// tell a continuation chunk from a fresh print line (see [`assemble`]).
pub const MAX_DATAGRAM_CONTENT: usize = 1400;
pub const MAX_RESULT_CHARS: usize = 100000;

/// GE Lua is synchronous; a vehicle context streams its result back cross-VM in 700-byte
/// pieces and its EXECEND lands one or more frames later.
pub const EXEC_TIMEOUT_GE: f64 = 5.0;
pub const EXEC_TIMEOUT_VEH: f64 = 15.0;

/// `send_command` takes a module name, not a bare port, so a typo is an error rather than
/// a datagram into the void. In the Python's insertion order.
pub const MODULE_PORTS: [(&str, u16); 14] = [
    ("vehicleScanner", 4448),
    ("beamtelAI", 4449),
    ("cameraInfo", 4451),
    ("obstacleDetector", 4453),
    ("nodeGrabber", 4455),
    ("clickspot", 4457),
    ("vehicleSlots", 4459),
    ("vehicleSpawner", 4461),
    ("roadDetector", 4463),
    ("uiToggle", 4464),
    ("console", 4465),
    ("vehicleBindings", 4468),
    ("implementProximity", 4470),
    ("terrainScanner", 4472),
];

/// Curated shortcuts over the Lua diagnostic entry points. Each value is a Lua expression
/// evaluated in the GE context; `{id}` is substituted with the `veh_id` argument.
pub const DIAGNOSTICS: [(&str, &str); 13] = [
    ("dock", "extensions.implementProximity.dockTruth()"),
    ("ramp", "extensions.implementProximity.rampTruth()"),
    ("ramp_states", "extensions.rampGeometry.diag()"),
    ("ramp_state_of", "extensions.rampGeometry.stateOf({id})"),
    ("ramp_retry", "extensions.rampGeometry.retry({id})"),
    ("mouth_frame", "dumps(extensions.rampGeometry.mouthFrame({id}))"),
    ("cannon", "extensions.cannonShot.diag()"),
    ("trailer", "extensions.trailerAngle.diag()"),
    (
        "implement_frame",
        "dumps(extensions.implementProximity.getImplementFrame())",
    ),
    (
        "implement_points",
        "dumps(extensions.implementProximity.getImplementPoints())",
    ),
    (
        "scanner_target",
        "extensions.vehicleScanner.getCurrentTargetID()",
    ),
    ("player_id", "be:getPlayerVehicleID(0)"),
    // terrainScanner.diag() logs via tsLog and returns nil, so this one is wrapped in a
    // LOGON window by t_diag rather than read from the return value.
    ("terrain", "extensions.terrainScanner.diag()"),
];

/// core_camera's free-camera yaw is `atan2(f.x, f.y)`, cameraInfo.lua's (the mod's) is
/// `atan2(-f.x, -f.y)`: the two differ by exactly 180 degrees with the same sign, and the
/// pitches by a sign alone. Derived from the game source, not measured.
pub const CAM_YAW_OFFSET_DEG: f64 = 180.0;
/// Let the camera settle before reading the pose back, so a set is self-verifying.
pub const CAM_SETTLE_MS_DEFAULT: i64 = 150;
pub const CAM_SETTLE_MS_MAX: f64 = 3000.0;
pub const CAMERA_ACTIONS: [&str; 6] = ["get", "list_modes", "set_mode", "cycle", "reset", "place"];
pub const SHOT_SETTLE_MS_MAX: f64 = 5000.0;

const LOG_RING_MAX: usize = 2000;
const CTX_CACHE_S: f64 = 10.0;

/// `say_fn(text, interrupt=True)`.
pub type SayFn = Arc<dyn Fn(&str, bool) + Send + Sync>;
/// `get_speech_log_fn(since_seq, last_n, source, contains, spoken_only)`.
pub type SpeechLogFn = Arc<
    dyn Fn(Option<i64>, Option<i64>, Option<&str>, Option<&str>, bool) -> SpeechLog + Send + Sync,
>;
/// `snapshot_state_fn(sections)`.
pub type SnapshotFn = Arc<dyn Fn(Option<&[&str]>) -> Value + Send + Sync>;
/// `load_config_fn()`.
pub type LoadConfigFn = Arc<dyn Fn() -> serde_json::Map<String, Value> + Send + Sync>;
/// `send_console_command_fn(msg)`.
pub type SendConsoleFn = Arc<dyn Fn(&str) + Send + Sync>;
/// `send_udp_fn(port, message)`; `Err` is the ToolError text.
pub type SendUdpFn = Arc<dyn Fn(u16, &str) -> Result<(), String> + Send + Sync>;
/// `press_command_fn(name, ctrl, shift, alt)`.
pub type PressFn = Arc<dyn Fn(&str, bool, bool, bool) -> Result<bool, String> + Send + Sync>;
/// `world_active_fn()`.
pub type WorldActiveFn = Arc<dyn Fn() -> bool + Send + Sync>;
/// `capture_png_fn(hide_ui, settle_s, focus_game)`.
pub type CaptureFn = Arc<dyn Fn(bool, f64, bool) -> Result<(Vec<u8>, Value), String> + Send + Sync>;
/// `road_diagnostic_fn(action, label, session, note, limit)`.
pub type RoadDiagFn = Arc<
    dyn Fn(&str, Option<&str>, Option<&str>, Option<&str>, i64) -> Result<Value, String>
        + Send
        + Sync,
>;
/// `assistant_diagnostic_fn(action, **kwargs)`.
pub type AssistantDiagFn = Arc<dyn Fn(&str, &Value) -> Result<Value, String> + Send + Sync>;

/// `mcp_server.Deps(...)`. `stop_event` is the process `timer::STOP`; `logger` is the log facade.
#[derive(Clone)]
pub struct Deps {
    pub say_fn: SayFn,
    pub get_speech_log_fn: SpeechLogFn,
    pub snapshot_state_fn: SnapshotFn,
    pub load_config_fn: LoadConfigFn,
    pub send_console_command_fn: SendConsoleFn,
    pub send_udp_fn: SendUdpFn,
    pub press_command_fn: PressFn,
    pub f9_help: IndexMap<HelpKey, &'static str>,
    pub world_active_fn: WorldActiveFn,
    pub capture_png_fn: CaptureFn,
    pub road_diagnostic_fn: RoadDiagFn,
    pub assistant_diagnostic_fn: AssistantDiagFn,
    pub version: String,
}

// ---- Errors ---------------------------------------------------------------------------------

/// How a tool failed. `Tool` is the Python's `ToolError` (the message is the whole text
/// the model sees); `Py` is any other exception, shown as `"<Type>: <message>"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolFail {
    Tool(String),
    Py(String, String),
}

impl ToolFail {
    /// `str(e)`.
    pub fn message(&self) -> &str {
        match self {
            ToolFail::Tool(m) | ToolFail::Py(_, m) => m,
        }
    }
    fn tool(msg: impl Into<String>) -> Self {
        ToolFail::Tool(msg.into())
    }
    fn py(type_name: &str, msg: impl Into<String>) -> Self {
        ToolFail::Py(type_name.to_string(), msg.into())
    }
}

impl From<PyErr> for ToolFail {
    fn from(e: PyErr) -> Self {
        ToolFail::Py(e.type_name().to_string(), e.message().to_string())
    }
}

pub type TResult<T> = Result<T, ToolFail>;

// ---- Global state ---------------------------------------------------------------------------

static DEPS: RwLock<Option<Deps>> = RwLock::new(None);
static STARTED_AT: Mutex<f64> = Mutex::new(0.0);

fn deps() -> TResult<Deps> {
    DEPS.read().clone().ok_or_else(|| {
        ToolFail::py(
            "AttributeError",
            "'NoneType' object has no attribute 'send_console_command_fn'",
        )
    })
}

// ---- Console tap / EXEC correlation ---------------------------------------------------------
//
// consoleAccessible.lua has no request IDs: correlation is ordering plus the EXECEND
// sentinel. Port 4466 is exclusively bound by the console listener, so a second process
// could never listen -- hence an in-process tap, and hence EXEC_LOCK, which keeps exactly
// one EXEC outstanding so ordering is a sound basis for correlation.

static EXEC_LOCK: Mutex<()> = Mutex::new(());
static INPUT_LOCK: Mutex<()> = Mutex::new(());

struct Capture {
    records: Mutex<Vec<String>>,
    done: Event,
    suppress: bool,
}

#[derive(Clone)]
struct LogRec {
    seq: i64,
    t: f64,
    raw: String,
}

struct TapState {
    capture: Option<Arc<Capture>>,
    ring: VecDeque<LogRec>,
    log_seq: i64,
    last_record_ts: f64,
}

static TAP: Mutex<TapState> = Mutex::new(TapState {
    capture: None,
    ring: VecDeque::new(),
    log_seq: 0,
    last_record_ts: 0.0,
});

/// `(monotonic_ts, [(index, label), ...])`.
static CTX_CACHE: Mutex<Option<(f64, Vec<(i64, String)>)>> = Mutex::new(None);

/// `console_tap(record)`: runs on the console listener thread. Returns true to consume.
///
/// Consuming matters beyond tidiness: the window speaks any single-line exec result
/// aloud, so without suppression every agent exec would talk over the user. LOG records
/// are copied but never consumed -- they arrive unsolicited and belong to the GUI's log
/// pane.
pub fn console_tap(record: &str) -> bool {
    let tag = record.split('|').next().unwrap_or("");
    let mut tap = TAP.lock();
    tap.last_record_ts = monotonic();
    if tag == "LOG" {
        tap.log_seq += 1;
        let seq = tap.log_seq;
        tap.ring.push_back(LogRec {
            seq,
            t: time_time(),
            raw: record.to_string(),
        });
        while tap.ring.len() > LOG_RING_MAX {
            tap.ring.pop_front();
        }
        if let Some(cap) = &tap.capture {
            cap.records.lock().push(record.to_string());
        }
        return false;
    }
    let Some(cap) = tap.capture.clone() else {
        return false;
    };
    cap.records.lock().push(record.to_string());
    if tag == "EXECEND" || tag == "CTXEND" {
        cap.done.set();
    }
    cap.suppress
}

/// Disarms the capture on the way out (`_run_console`'s `finally`).
struct Disarm;

impl Drop for Disarm {
    fn drop(&mut self) {
        TAP.lock().capture = None;
    }
}

/// `deadline = time.monotonic() + timeout` for a timeout of any Python type.
fn timeout_secs(timeout: &PyJson) -> TResult<f64> {
    match timeout {
        PyJson::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        PyJson::Int(i) => Ok(*i as f64),
        PyJson::Float(f) => Ok(*f),
        PyJson::BigInt(_) => Err(ToolFail::py(
            "OverflowError",
            "int too large to convert to float",
        )),
        other => Err(ToolFail::py(
            "TypeError",
            format!(
                "unsupported operand type(s) for +: 'float' and '{}'",
                other.type_name()
            ),
        )),
    }
}

/// `_run_console(cmd, timeout, suppress)`: send one console command and collect its
/// records up to the terminating sentinel. Returns `(records, timed_out)`. Always
/// releases the lock and disarms the capture.
fn run_console(cmd: &str, timeout: &PyJson, suppress: bool) -> TResult<(Vec<String>, bool)> {
    let Some(_guard) = EXEC_LOCK.try_lock_for(Duration::from_secs(2)) else {
        return Err(ToolFail::tool(
            "console busy -- another exec is still in flight",
        ));
    };
    let cap = Arc::new(Capture {
        records: Mutex::new(Vec::new()),
        done: Event::new(),
        suppress,
    });
    let _disarm = Disarm;
    TAP.lock().capture = Some(Arc::clone(&cap));
    let d = deps()?;
    (d.send_console_command_fn)(cmd);
    let deadline = monotonic() + timeout_secs(timeout)?;
    let mut timed_out = false;
    while !cap.done.wait_secs(0.2) {
        if STOP.is_set() {
            return Err(ToolFail::tool("beamtel is shutting down"));
        }
        if monotonic() > deadline {
            timed_out = true;
            break;
        }
    }
    let records = cap.records.lock().clone();
    Ok((records, timed_out))
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `_assemble(records)`: reassemble RESP/OUT records into a result value and print output.
///
/// emitChunks() repeats its prefix on *every* chunk, so a long value arrives as several
/// `OUT|= ` records that must be concatenated, not newline-joined. Bare `OUT|` records are
/// ambiguous -- a continuation of one long print line looks exactly like the next print
/// line -- so a chunk is treated as a continuation when the previous one filled the
/// datagram or already ended in a newline.
pub fn assemble(records: &[String]) -> Dict {
    let mut ok = true;
    let mut result_parts: Vec<String> = Vec::new();
    let mut out_lines: Vec<String> = Vec::new();
    let mut prev_out: Option<String> = None;
    let mut saw_resp = false;

    for rec in records {
        let parts: Vec<&str> = rec.split('|').collect();
        let tag = parts[0];
        if tag == "RESP" {
            saw_resp = true;
            let status = parts.get(1).copied().unwrap_or("");
            let body = if parts.len() > 2 {
                parts[2..].join("|")
            } else {
                String::new()
            };
            if status == "error" {
                ok = false;
                result_parts.push(if body.is_empty() {
                    "error".to_string()
                } else {
                    body
                });
            } else if status == "queued" {
                result_parts
                    .push("queued (fire-and-forget context; no value is returned)".to_string());
            } else if !body.is_empty() {
                result_parts.push(body);
            }
        } else if tag == "OUT" {
            let content = parts[1..].join("|");
            if let Some(value) = content.strip_prefix("= ") {
                result_parts.push(value.to_string());
                prev_out = None;
            } else {
                let cont = prev_out.as_ref().is_some_and(|p| {
                    char_len(p) >= MAX_DATAGRAM_CONTENT || p.ends_with('\n')
                });
                if cont && let Some(last) = out_lines.last_mut() {
                    last.push_str(&content);
                } else {
                    out_lines.push(content.clone());
                }
                prev_out = Some(content);
            }
        }
    }

    let mut d = Dict::new();
    d.insert("ok".into(), PyJson::Bool(ok));
    d.insert("saw_response".into(), PyJson::Bool(saw_resp));
    d.insert("result".into(), PyJson::Str(result_parts.concat()));
    d.insert("output".into(), PyJson::Str(out_lines.join("\n")));
    d
}

/// `_truncate(payload)`.
fn truncate(payload: &mut Dict) {
    for key in ["result", "output"] {
        let val = match payload.get(key) {
            Some(v) if v.truthy() => v.py_str(),
            _ => String::new(),
        };
        if char_len(&val) > MAX_RESULT_CHARS {
            payload.insert(
                key.into(),
                PyJson::Str(val.chars().take(MAX_RESULT_CHARS).collect()),
            );
            payload.insert("truncated".into(), PyJson::Bool(true));
        }
    }
}

fn console_unreachable(timeout: f64) -> ToolFail {
    ToolFail::tool(format!(
        "no response from consoleAccessible.lua on 127.0.0.1:4466 within {}s -- is BeamNG running with the mod loaded? Call `health`.",
        py_f(timeout, 1)
    ))
}

fn fetch_contexts(timeout: f64) -> TResult<Vec<(i64, String)>> {
    let (records, timed_out) = run_console("CTXLIST", &PyJson::Float(timeout), true)?;
    if timed_out && records.is_empty() {
        return Err(console_unreachable(timeout));
    }
    let mut out = Vec::new();
    for rec in &records {
        let parts: Vec<&str> = rec.split('|').collect();
        if parts[0] == "CTX" && parts.len() >= 3 {
            if let Ok(idx) = PyJson::Str(parts[1].to_string()).py_int() {
                out.push((idx, parts[2..].join("|")));
            }
        }
    }
    Ok(out)
}

fn contexts(force: bool) -> TResult<Vec<(i64, String)>> {
    let now = monotonic();
    if !force && let Some((ts, ctxs)) = CTX_CACHE.lock().as_ref() && now - ts < CTX_CACHE_S {
        return Ok(ctxs.clone());
    }
    let ctxs = fetch_contexts(5.0)?;
    *CTX_CACHE.lock() = Some((now, ctxs.clone()));
    Ok(ctxs)
}

fn match_vehicle_context(want: &str, force: bool) -> TResult<Option<i64>> {
    for (idx, label) in contexts(force)? {
        if idx < 3 {
            continue;
        }
        let low = label.to_lowercase();
        if want == "current" || want == "player" {
            if low.contains("(current)") {
                return Ok(Some(idx));
            }
        } else if low.contains(want) {
            return Ok(Some(idx));
        }
    }
    Ok(None)
}

/// `str.isdigit()` for a context spec.
fn is_digit_str(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// `_resolve_context(spec)`: map a friendly context name onto consoleAccessible's index.
pub fn resolve_context(spec: &PyJson) -> TResult<i64> {
    let s = if spec.truthy() {
        match spec {
            PyJson::Str(t) => beam_base::pyfmt::py_strip(t).to_lowercase(),
            other => {
                return Err(ToolFail::py(
                    "AttributeError",
                    format!("'{}' object has no attribute 'strip'", other.type_name()),
                ));
            }
        }
    } else {
        "ge".to_string()
    };
    match s.as_str() {
        "ge" | "lua" | "ge-lua" | "0" => return Ok(0),
        "ts" | "torque" | "torquescript" | "1" => return Ok(1),
        "ui" | "js" | "cef" | "2" => return Ok(2),
        _ => {}
    }
    if is_digit_str(&s) {
        return Ok(PyJson::Str(s).py_int()?);
    }
    if s.starts_with("veh") {
        let want = match s.split_once(':') {
            Some((_, rest)) => beam_base::pyfmt::py_strip(rest).to_string(),
            None => "current".to_string(),
        };
        for force in [false, true] {
            if let Some(idx) = match_vehicle_context(&want, force)? {
                return Ok(idx);
            }
        }
        return Err(ToolFail::tool(format!(
            "no vehicle context matching {}; call `lua_contexts` to see what is loaded",
            pyjson::py_str_repr(&want)
        )));
    }
    Err(ToolFail::tool(format!(
        "unknown context {} (use ge / ts / ui / veh:<id> / veh:current)",
        spec.py_repr()
    )))
}

/// `exec_console(context, code, timeout_s=None, verbose=False, suppress_gui=True)`.
/// `timeout_s` is `Null` for the context's default.
pub fn exec_console(
    context: &PyJson,
    code: &str,
    timeout_s: &PyJson,
    verbose: bool,
    suppress_gui: bool,
) -> TResult<Dict> {
    let idx = resolve_context(context)?;
    let timeout = if timeout_s.is_null() {
        PyJson::Float(if idx >= 3 {
            EXEC_TIMEOUT_VEH
        } else {
            EXEC_TIMEOUT_GE
        })
    } else {
        timeout_s.clone()
    };
    let started = monotonic();
    let (records, timed_out) = run_console(&format!("EXEC|{idx}|{code}"), &timeout, suppress_gui)?;
    let mut payload = assemble(&records);
    payload.insert("context".into(), context.clone());
    payload.insert("context_index".into(), PyJson::Int(idx));
    payload.insert(
        "elapsed_ms".into(),
        PyJson::Int(((monotonic() - started) * 1000.0).trunc() as i64),
    );
    if timed_out {
        payload.insert("timed_out".into(), PyJson::Bool(true));
        let secs = timeout_secs(&timeout)?;
        if records.is_empty() {
            return Err(console_unreachable(secs));
        }
        payload.insert("ok".into(), PyJson::Bool(false));
        payload.insert(
            "note".into(),
            PyJson::Str(format!(
                "no EXECEND within {}s; the records below are partial",
                py_f(secs, 1)
            )),
        );
    }
    if verbose {
        payload.insert(
            "records".into(),
            PyJson::List(records.into_iter().map(PyJson::Str).collect()),
        );
    }
    truncate(&mut payload);
    Ok(payload)
}

/// `exec_console("ge", code)` with every default.
fn exec_ge(code: &str) -> TResult<Dict> {
    exec_console(&PyJson::Str("ge".into()), code, &PyJson::Null, false, true)
}

fn require_world() -> TResult<()> {
    let d = deps()?;
    if !(d.world_active_fn)() {
        return Err(ToolFail::tool(
            "no live world: telemetry on 4444 has gone quiet. The game may be closed, at a menu, or paused. Call `health`.",
        ));
    }
    Ok(())
}

// ---- Tool registry --------------------------------------------------------------------------

type Handler = fn(&PyJson) -> TResult<PyJson>;

/// One `TOOLS` entry.
pub struct ToolDef {
    pub name: &'static str,
    pub description: String,
    pub input_schema: PyJson,
    handler: Handler,
}

fn schema(json: &str) -> PyJson {
    pyjson::loads(json).expect("tool input schema")
}

fn sorted_names(items: &[(&str, impl Copy)]) -> String {
    let mut names: Vec<&str> = items.iter().map(|(n, _)| *n).collect();
    names.sort();
    names.join(", ")
}

/// `TOOLS`, in the Python's order.
pub static TOOLS: LazyLock<Vec<ToolDef>> = LazyLock::new(|| {
    vec![
        ToolDef {
            name: "health",
            description: "Whether the game and the mod are reachable: live world, telemetry age, console round-trip, uptime. Call this first in any session; every other tool's failure message points back here.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {}}"#),
            handler: tools::t_health,
        },
        ToolDef {
            name: "lua_exec",
            description: "Execute Lua (or TorqueScript / UI JavaScript) inside BeamNG and return the value plus captured print output. context: 'ge' (Game Engine Lua, the only one that returns a value synchronously), 'ts', 'ui', 'veh:current' or 'veh:<id>' for a vehicle VM. This is the universal surface -- it reaches every mod diagnostic, including those with no UDP command.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {
                "code": {"type": "string", "description": "Lua source. Use 'return ...' to get a value."},
                "context": {"type": "string", "description": "ge | ts | ui | veh:current | veh:<id>", "default": "ge"},
                "timeout_s": {"type": "number", "description": "Default 5s for ge, 15s for a vehicle context."},
                "verbose": {"type": "boolean", "description": "Include the raw wire records."},
                "suppress_gui": {"type": "boolean", "description": "Keep the result out of beamtel's console pane and out of the user's ears. Default true.", "default": true}},
                "required": ["code"]}"#),
            handler: tools::t_lua_exec,
        },
        ToolDef {
            name: "lua_contexts",
            description: "List the execution contexts consoleAccessible offers, including one per loaded vehicle.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {"refresh": {"type": "boolean", "default": true}}}"#),
            handler: tools::t_lua_contexts,
        },
        ToolDef {
            name: "get_state",
            description: "Snapshot of beamtel's live state. Sections: telemetry, position, implement, trailer, dock, cannon, scanner, modes, slots, assistant, road, liveness. Omit `sections` for all.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {"sections": {"type": "array", "items": {"type": "string"}}}}"#),
            handler: tools::t_get_state,
        },
        ToolDef {
            name: "get_config",
            description: "The user's current beamtel configuration. Read-only by design -- the agent must not silently rewrite the user's settings. API keys and other secrets read back as `<redacted>` when set and empty when unset.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {}}"#),
            handler: tools::t_get_config,
        },
        ToolDef {
            name: "assistant_diagnostic",
            description: "Observe the driving assistant while the player drives. start records vehicle inputs, AI steering/braking, route and turn choices at 5 Hz plus announcements and disengagement reasons to NDJSON. It does not enable or drive the assistant. status gives latest state and sample age; read returns events after since_seq; mark adds a note; stop closes the recording; review summarizes a session; list returns saved session names.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {
                "action": {"type": "string", "default": "status", "enum": ["start", "stop", "status", "read", "mark", "review", "list"]},
                "label": {"type": "string"}, "session": {"type": "string"},
                "note": {"type": "string"}, "since_seq": {"type": "integer", "default": 0},
                "limit": {"type": "integer", "default": 100}}}"#),
            handler: tools::t_assistant_diagnostic,
        },
        ToolDef {
            name: "road_diagnostic",
            description: "Record and review a road-guidance driving attempt. `start` opens a durable NDJSON session and enables detailed Lua lane-correction fields; `stop` closes it and returns an analysis of settled-band occupancy, correction episodes, rapid settled-tone retriggers, steering/target oscillation, and wheel slip. `status`, `list`, and `review` are read-only; `mark` adds a timestamped note.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {
                "action": {"type": "string", "description": "start | stop | status | mark | review | list", "default": "status"},
                "label": {"type": "string", "description": "start: short session label"},
                "session": {"type": "string", "description": "review: bare .ndjson session file name; omit for latest"},
                "note": {"type": "string", "description": "mark: note about an observed event"},
                "limit": {"type": "integer", "description": "list: maximum sessions", "default": 20}}}"#),
            handler: tools::t_road_diagnostic,
        },
        ToolDef {
            name: "speech_log",
            description: "What the mod actually announced, as a sequence-numbered ring buffer. This is the assertion primitive: it records every say() call including the ones excluded from the user's replay buffer, and records calls that were suppressed during loading (spoken=false). Use `since_seq` as a cursor.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {
                "since_seq": {"type": "integer", "description": "Return only entries after this sequence number."},
                "last_n": {"type": "integer", "default": 50},
                "source": {"type": "string", "description": "Filter by the say() source tag."},
                "contains": {"type": "string", "description": "Case-insensitive substring filter."},
                "spoken_only": {"type": "boolean", "default": false}}}"#),
            handler: tools::t_speech_log,
        },
        ToolDef {
            name: "speak",
            description: "Say something to the user through the mod's own speech output -- how to give instructions mid-test.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {
                "text": {"type": "string"},
                "interrupt": {"type": "boolean", "default": true}},
                "required": ["text"]}"#),
            handler: tools::t_speak,
        },
        ToolDef {
            name: "list_commands",
            description: "Every F9 command the mod offers, with its modifiers and description. Works with no game running.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {}}"#),
            handler: tools::t_list_commands,
        },
        ToolDef {
            name: "press_command",
            description: "Trigger an F9 command in-process (no OS key injection, no window focus needed) and return the speech it produced. One round trip is act plus assert. Note that some commands open modal UI.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {
                "name": {"type": "string", "description": "The key pressed after F9, e.g. 's' for speed."},
                "ctrl": {"type": "boolean"},
                "shift": {"type": "boolean"},
                "alt": {"type": "boolean"},
                "settle_ms": {"type": "integer", "default": 300, "description": "How long to wait for speech before reading it back."}},
                "required": ["name"]}"#),
            handler: tools::t_press_command,
        },
        ToolDef {
            name: "send_command",
            description: format!(
                "Send a raw UDP command to one of the mod's Lua extensions. Fire-and-forget: verify the effect with get_state / speech_log / diag. Modules: {}",
                sorted_names(&MODULE_PORTS)
            ),
            input_schema: schema(r#"{"type": "object", "properties": {
                "module": {"type": "string", "description": "Extension name from the list above."},
                "port": {"type": "integer", "description": "Escape hatch for a port not in the map."},
                "payload": {"type": "string", "description": "The command verb, e.g. 'DOCK_ON', 'SCAN', 'MODE:chase'."}},
                "required": ["payload"]}"#),
            handler: tools::t_send_command,
        },
        ToolDef {
            name: "vehicle_control",
            description: "Spawn, teleport, remove or reload vehicles -- how to build known test geometry. Teleport offsets are in FEET, rotations in degrees.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {
                "action": {"type": "string", "description": "list_active | player_id | spawn | teleport | remove | reload | ignition_off"},
                "items": {"type": "array", "items": {"type": "object"}, "description": "spawn: [{model, config, refMode, offFwdFt, ...}]"},
                "arrangement": {"type": "object"},
                "vehicle_ids": {"type": "array", "items": {"type": "integer"}},
                "vehId": {"type": "integer"},
                "refVehId": {"type": "integer"},
                "refMode": {"type": "string"},
                "offFwdFt": {"type": "number"},
                "offRightFt": {"type": "number"},
                "offUpFt": {"type": "number"},
                "rotYawDeg": {"type": "number"},
                "rotPitchDeg": {"type": "number"},
                "rotRollDeg": {"type": "number"},
                "mode": {"type": "string"}},
                "required": ["action"]}"#),
            handler: tools::t_vehicle_control,
        },
        ToolDef {
            name: "diag",
            description: format!(
                "Run one of the mod's Lua ground-truth diagnostics. Names: {}. These print what the resolver actually chose, which is the only way to catch a confident wrong number.",
                sorted_names(&DIAGNOSTICS)
            ),
            input_schema: schema(r#"{"type": "object", "properties": {
                "name": {"type": "string"},
                "veh_id": {"type": "integer", "description": "For the diagnostics that take a vehicle id."}},
                "required": ["name"]}"#),
            handler: tools::t_diag,
        },
        ToolDef {
            name: "console_log",
            description: "Turn the game's Lua log stream on or off, and read what it captured. Log records still reach beamtel's own console pane.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {
                "action": {"type": "string", "description": "on | off | read", "default": "read"},
                "since_seq": {"type": "integer"},
                "last_n": {"type": "integer", "default": 200}}}"#),
            handler: tools::t_console_log,
        },
        ToolDef {
            name: "camera_control",
            description: "Move the eye. Switches camera mode (including free camera) and places the free camera anywhere, then returns the resulting camera state -- so a set is self-verifying in one round trip and pairs directly with `screenshot`. actions: get (default) | list_modes | set_mode | cycle | reset | place. `place` takes exactly one position form -- absolute (x/y/z), relative to a vehicle's own frame (rel_fwd_m/rel_right_m/rel_up_m), or orbit around one (orbit_distance_m + orbit_azimuth_deg/orbit_elevation_deg) -- plus an optional `look_at` that derives the orientation for you. Angles are in the mod's own convention: yaw 0-360 matching the 4450 feed and the Alt+H readout, pitch positive-UP. This never moves the vehicle.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {
                "action": {"type": "string", "description": "get | list_modes | set_mode | cycle | reset | place", "default": "get"},
                "mode": {"type": "string", "description": "set_mode: a camera name from list_modes, e.g. 'free', 'orbit', 'onboard.driver'."},
                "offset": {"type": "integer", "description": "cycle: +1 for the next vehicle camera, -1 for the previous.", "default": 1},
                "veh_id": {"type": "integer", "description": "place: the vehicle a relative or orbit placement is measured against. Defaults to the player's."},
                "x": {"type": "number", "description": "place, absolute: world X."},
                "y": {"type": "number", "description": "place, absolute: world Y."},
                "z": {"type": "number", "description": "place, absolute: world Z."},
                "rel_fwd_m": {"type": "number", "description": "place, relative: metres along the vehicle's forward. Negative is behind it."},
                "rel_right_m": {"type": "number", "description": "place, relative: metres to the vehicle's RIGHT. Note this is a placement offset, so it is the mirror of the mod's positive-is-LEFT bearing convention."},
                "rel_up_m": {"type": "number", "description": "place, relative: metres along the vehicle's up."},
                "orbit_distance_m": {"type": "number", "description": "place, orbit: radius from the vehicle. Implies looking at it."},
                "orbit_azimuth_deg": {"type": "number", "description": "place, orbit: 0 puts the camera directly ahead of the vehicle, positive swings to its right.", "default": 0},
                "orbit_elevation_deg": {"type": "number", "description": "place, orbit: degrees above the world horizontal.", "default": 20},
                "look_at": {"description": "place: a [x, y, z] point or a vehicle id to aim at. Overrides yaw_deg/pitch_deg."},
                "yaw_deg": {"type": "number", "description": "place: heading, 0-360, same convention as the 4450 camera feed.", "default": 0},
                "pitch_deg": {"type": "number", "description": "place: positive is looking UP, matching cameraInfo.lua.", "default": 0},
                "roll_deg": {"type": "number", "description": "place: camera roll.", "default": 0},
                "settle_ms": {"type": "integer", "description": "Wait before reading the pose back.", "default": 150}}}"#),
            handler: camera::t_camera_control,
        },
        ToolDef {
            name: "screenshot",
            description: "Capture the game and WRITE THE PNG TO A FILE for you to read -- the same capture path the AI Describer uses, with nothing sent to any model. Use it when a picture settles a question the numbers cannot: whether a car really is where a resolver says it is, what a mouth or an implement is actually pointing at, or what the mod's own UI is showing. Pass `dir` to put the file in your own working directory, then read that path. Pairs with `camera_control`: place the eye, then shoot.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {
                "dir": {"type": "string", "description": "Directory to write into. Defaults to %LOCALAPPDATA%/beamtel/screenshots."},
                "filename": {"type": "string", "description": "Bare file name, no path. Defaults to a millisecond-stamped name so successive shots never collide."},
                "hide_ui": {"type": "boolean", "description": "Hide the game HUD around the grab for a clean view of the world. Pass false to inspect the UI itself.", "default": true},
                "focus_game": {"type": "boolean", "description": "Raise the BeamNG window before the grab and put the previous window back afterwards. Needed whenever the game is minimized, which it usually is when an agent is driving -- a minimized BeamNG renders nothing and a plain grab returns the desktop. The result reports what was actually captured.", "default": true},
                "settle_ms": {"type": "integer", "description": "How long to let the game render a UI-free frame after hiding.", "default": 200}}}"#),
            handler: camera::t_screenshot,
        },
        ToolDef {
            name: "reset_test_state",
            description: "Drop the mod's cached geometry (implement rebuild + ramp resolve re-arm) so the next test starts clean.".into(),
            input_schema: schema(r#"{"type": "object", "properties": {}}"#),
            handler: tools::t_reset_test_state,
        },
    ]
});

pub const INSTRUCTIONS: &str = "Drives the BEAM screen-reader mod for BeamNG.drive. Call `health` first -- it says whether a world is live and whether the game console answers. Every tool returns a clear error rather than hanging when the game is not running. `lua_exec` is the universal surface; `speech_log` is how you assert on what the mod announced; `speak` is how you give the user instructions mid-test.";

// ---- JSON-RPC / MCP protocol ----------------------------------------------------------------

fn rpc_result(id: PyJson, result: PyJson) -> PyJson {
    pyjson::dict([
        ("jsonrpc", PyJson::from("2.0")),
        ("id", id),
        ("result", result),
    ])
}

fn rpc_error(id: PyJson, code: i64, message: String) -> PyJson {
    pyjson::dict([
        ("jsonrpc", PyJson::from("2.0")),
        ("id", id),
        (
            "error",
            pyjson::dict([("code", PyJson::Int(code)), ("message", PyJson::Str(message))]),
        ),
    ])
}

fn text_content(text: String, is_error: bool) -> PyJson {
    pyjson::dict([
        (
            "content",
            PyJson::List(vec![pyjson::dict([
                ("type", PyJson::from("text")),
                ("text", PyJson::Str(text)),
            ])]),
        ),
        ("isError", PyJson::Bool(is_error)),
    ])
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_string())
}

/// `_call_tool(name, args)`: a known tool's result content. A `ToolError` is its message;
/// any other failure is logged and shown as `"<Type>: <message>"`.
pub fn call_tool(name: &str, args: &PyJson) -> PyJson {
    let Some(tool) = TOOLS.iter().find(|t| t.name == name) else {
        return text_content(format!("KeyError: {}", pyjson::py_str_repr(name)), true);
    };
    let empty = PyJson::Dict(Dict::new());
    let args = if args.truthy() { args } else { &empty };
    let handler = tool.handler;
    let outcome = catch_unwind(AssertUnwindSafe(|| handler(args)))
        .unwrap_or_else(|p| Err(ToolFail::py("Exception", panic_text(&*p))));
    match outcome {
        Ok(payload) => {
            let text = match &payload {
                PyJson::Str(s) => s.clone(),
                other => pyjson::dumps(other, &DumpOpts::indented(2)).unwrap_or_default(),
            };
            text_content(text, false)
        }
        Err(ToolFail::Tool(m)) => text_content(m, true),
        Err(ToolFail::Py(t, m)) => {
            log::error!("mcp_server: tool {name} failed: {t}: {m}");
            text_content(format!("{t}: {m}"), true)
        }
    }
}

/// The `tools/list` result.
pub fn tools_list() -> PyJson {
    PyJson::List(
        TOOLS
            .iter()
            .map(|t| {
                pyjson::dict([
                    ("name", PyJson::from(t.name)),
                    ("description", PyJson::Str(t.description.clone())),
                    ("inputSchema", t.input_schema.clone()),
                ])
            })
            .collect(),
    )
}

/// `_dispatch(msg)`: one JSON-RPC message. `Ok(None)` for a notification; `Err` is an
/// exception the Python's handler does not catch (aiohttp answers it with a 500).
pub async fn dispatch(msg: &PyJson) -> Result<Option<PyJson>, PyErr> {
    let method = msg.py_get("method")?.cloned().unwrap_or(PyJson::Null);
    let id = msg.py_get("id")?.cloned().unwrap_or(PyJson::Null);
    let params = match msg.py_get("params")? {
        Some(p) if p.truthy() => p.clone(),
        _ => PyJson::Dict(Dict::new()),
    };

    if id.is_null() {
        return Ok(None); // notification: nothing to answer
    }

    if method.eq_str("initialize") {
        let want = params.py_get("protocolVersion")?;
        let version = match want {
            Some(PyJson::Str(v)) if SUPPORTED_PROTOCOLS.contains(&v.as_str()) => v.clone(),
            _ => FALLBACK_PROTOCOL.to_string(),
        };
        let server_version = DEPS
            .read()
            .as_ref()
            .map_or_else(|| "1.0".to_string(), |d| d.version.clone());
        return Ok(Some(rpc_result(
            id,
            pyjson::dict([
                ("protocolVersion", PyJson::Str(version)),
                (
                    "capabilities",
                    pyjson::dict([("tools", pyjson::dict([("listChanged", PyJson::Bool(false))]))]),
                ),
                (
                    "serverInfo",
                    pyjson::dict([
                        ("name", PyJson::from(SERVER_NAME)),
                        ("version", PyJson::Str(server_version)),
                    ]),
                ),
                ("instructions", PyJson::from(INSTRUCTIONS)),
            ]),
        )));
    }

    if method.eq_str("ping") {
        return Ok(Some(rpc_result(id, PyJson::Dict(Dict::new()))));
    }

    if method.eq_str("tools/list") {
        return Ok(Some(rpc_result(
            id,
            pyjson::dict([("tools", tools_list())]),
        )));
    }

    if method.eq_str("tools/call") {
        let name = params.py_get("name")?.cloned().unwrap_or(PyJson::Null);
        let args = match params.py_get("arguments")? {
            Some(a) if a.truthy() => a.clone(),
            _ => PyJson::Dict(Dict::new()),
        };
        if !name.hashable() {
            return Err(pyjson::unhashable(&name));
        }
        let known = name
            .as_str()
            .filter(|n| TOOLS.iter().any(|t| t.name == *n))
            .map(str::to_string);
        let Some(name) = known else {
            return Ok(Some(rpc_error(
                id,
                -32602,
                format!("Unknown tool: {}", name.py_str()),
            )));
        };
        // Tool bodies block (UDP round trips, sleeps), so they never run on the event
        // loop. The outer timeout is generous: the tool's own timeout should fire first
        // and report something useful.
        let job_name = name.clone();
        let job = tokio::task::spawn_blocking(move || call_tool(&job_name, &args));
        let result = match tokio::time::timeout(Duration::from_secs(60), job).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => text_content(format!("Exception: {e}"), true),
            Err(_) => text_content(format!("tool {name} exceeded 60s and was abandoned"), true),
        };
        return Ok(Some(rpc_result(id, result)));
    }

    Ok(Some(rpc_error(
        id,
        -32601,
        format!("Method not found: {}", method.py_str()),
    )))
}

/// `_origin_ok(request)`: guard against DNS-rebinding from a browser on this machine.
fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    origin_allowed(&String::from_utf8_lossy(origin.as_bytes()))
}

/// `_origin_ok` for one `Origin` value: none (empty) is allowed, as from a non-browser
/// client; otherwise it must be exactly `http://127.0.0.1` or `http://localhost`, with an
/// optional numeric port and nothing after it.
///
/// **Deliberate difference (python-bugs.md 66):** the Python accepted any origin that
/// merely began with one of those, so `http://localhost.example.com` or
/// `http://127.0.0.1.nip.io` passed, and a page served from such a name and rebound to
/// 127.0.0.1 could call every tool, `lua_exec` included.
#[doc(hidden)]
pub fn origin_allowed(origin: &str) -> bool {
    if origin.is_empty() {
        return true;
    }
    let Some(rest) = origin.strip_prefix("http://") else {
        return false;
    };
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()));
    port_ok && (host == "127.0.0.1" || host.eq_ignore_ascii_case("localhost"))
}

fn with_type(mut resp: Response, content_type: &'static str) -> Response {
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    resp
}

fn text_response(status: StatusCode, body: &str) -> Response {
    with_type(
        (status, body.to_string()).into_response(),
        "text/plain; charset=utf-8",
    )
}

/// `web.json_response(obj, status=...)`.
fn json_response(status: StatusCode, obj: &PyJson) -> Response {
    let body = pyjson::dumps(obj, &DumpOpts::default_seps()).unwrap_or_default();
    with_type((status, body).into_response(), "application/json; charset=utf-8")
}

/// `web.Response(status=202)`: no body.
fn accepted() -> Response {
    with_type(
        (StatusCode::ACCEPTED, Bytes::new()).into_response(),
        "application/octet-stream",
    )
}

/// aiohttp's answer to an exception the handler did not catch.
fn internal_error(e: &PyErr) -> Response {
    eprintln!("Error handling request: {}: {}", e.type_name(), e.message());
    text_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "500 Internal Server Error\n\nServer got itself in trouble",
    )
}

/// `request.text()`: the body in the request's charset (UTF-8 unless it names another).
fn body_text(headers: &HeaderMap, body: &[u8]) -> Option<String> {
    let charset = headers
        .get(header::CONTENT_TYPE)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).to_lowercase())
        .and_then(|ct| {
            ct.split(';')
                .skip(1)
                .find_map(|p| p.trim().strip_prefix("charset=").map(|c| c.trim_matches('"').to_string()))
        });
    match charset.as_deref() {
        None | Some("utf-8") | Some("utf8") => String::from_utf8(body.to_vec()).ok(),
        Some("latin-1") | Some("latin1") | Some("iso-8859-1") => {
            Some(body.iter().map(|b| *b as char).collect())
        }
        Some("ascii") | Some("us-ascii") => {
            if body.is_ascii() {
                String::from_utf8(body.to_vec()).ok()
            } else {
                None
            }
        }
        Some(_) => None,
    }
}

async fn mcp_post(headers: HeaderMap, body: Bytes) -> Response {
    if !origin_ok(&headers) {
        return text_response(StatusCode::FORBIDDEN, "forbidden origin");
    }
    let parsed = body_text(&headers, &body).and_then(|t| pyjson::loads(&t).ok());
    let Some(body) = parsed else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &rpc_error(PyJson::Null, -32700, "Parse error".to_string()),
        );
    };

    if let PyJson::List(items) = &body {
        let mut responses = Vec::new();
        for msg in items {
            match dispatch(msg).await {
                Ok(Some(resp)) => responses.push(resp),
                Ok(None) => {}
                Err(e) => return internal_error(&e),
            }
        }
        if responses.is_empty() {
            return accepted();
        }
        return json_response(StatusCode::OK, &PyJson::List(responses));
    }

    match dispatch(&body).await {
        // A notification (notifications/initialized and friends) must be answered with
        // 202 and an empty body, or the client hangs waiting on the handshake.
        Ok(None) => accepted(),
        Ok(Some(resp)) => json_response(StatusCode::OK, &resp),
        Err(e) => internal_error(&e),
    }
}

/// No server-initiated SSE stream: nothing here pushes to the client. 405 is the spec's
/// way of saying so, and clients treat it as "no stream" rather than an error.
async fn mcp_get() -> Response {
    text_response(StatusCode::METHOD_NOT_ALLOWED, "no server-initiated stream")
}

async fn mcp_delete() -> Response {
    text_response(StatusCode::OK, "")
}

async fn health_handler() -> Response {
    text_response(StatusCode::OK, "beamtel mcp server ok\n")
}

fn method_not_allowed(allow: &'static str) -> Response {
    let mut resp = text_response(StatusCode::METHOD_NOT_ALLOWED, "405: Method Not Allowed");
    resp.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static(allow));
    resp
}

async fn mcp_other_method() -> Response {
    method_not_allowed("DELETE,GET,HEAD,POST")
}

async fn health_other_method() -> Response {
    method_not_allowed("GET,HEAD")
}

async fn not_found() -> Response {
    text_response(StatusCode::NOT_FOUND, "404: Not Found")
}

// ---- Boot -----------------------------------------------------------------------------------

/// `init(deps)`.
pub fn init(deps: Deps) {
    *DEPS.write() = Some(deps);
}

/// A started server: its stop function and the address it bound (`None` if the bind
/// failed, which is logged and otherwise ignored, as in the Python).
pub struct McpServer {
    pub stop: Box<dyn FnOnce() + Send>,
    pub addr: Option<SocketAddr>,
}

/// `start(stop_event, port)`: returns the stop function. A failure to bind is logged
/// (`failed to bind ...`) on the server thread and is not an error here.
pub fn start(port: u16) -> Result<Box<dyn FnOnce() + Send>, String> {
    Ok(start_on(HOST, port).stop)
}

/// [`start`] on an explicit host (port 0 picks a free port).
pub fn start_on(host: &str, port: u16) -> McpServer {
    *STARTED_AT.lock() = monotonic();
    let host = host.to_string();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Option<SocketAddr>>();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let spawned = std::thread::Builder::new()
        .name("beamtel-mcp".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("mcp_server: failed to bind {host}:{port} -- {e}");
                    let _ = ready_tx.send(None);
                    return;
                }
            };
            rt.block_on(serve(host, port, ready_tx, shutdown_rx));
            drop(rt);
            let _ = done_tx.send(());
        });
    if let Err(e) = spawned {
        log::error!("mcp_server: failed to start the server thread: {e}");
    }
    let addr = ready_rx.recv().ok().flatten();
    let stop: Box<dyn FnOnce() + Send> = Box::new(move || {
        let _ = shutdown_tx.send(true);
        let _ = done_rx.recv_timeout(Duration::from_millis(1500));
    });
    McpServer { stop, addr }
}

async fn serve(
    host: String,
    port: u16,
    ready: std::sync::mpsc::Sender<Option<SocketAddr>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let listener = match tokio::net::TcpListener::bind((host.as_str(), port)).await {
        Ok(l) => l,
        Err(e) => {
            log::error!("mcp_server: failed to bind {host}:{port} -- {e}");
            let _ = ready.send(None);
            return;
        }
    };
    log::info!("mcp_server: MCP server listening on http://{host}:{port}/mcp");
    let _ = ready.send(listener.local_addr().ok());
    let app = Router::new()
        .route(
            "/mcp",
            post(mcp_post)
                .get(mcp_get)
                .delete(mcp_delete)
                .fallback(mcp_other_method),
        )
        .route("/health", get(health_handler).fallback(health_other_method))
        .fallback(not_found);
    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
        })
        .await;
}

/// `make_udp_sender(logger)`: the default `send_udp_fn`, one throwaway datagram per call.
pub fn make_udp_sender() -> SendUdpFn {
    Arc::new(|port, message| {
        crate::net::send_udp(port, message.as_bytes())
            .map_err(|e| format!("failed to send UDP to port {port}: {e}"))
    })
}

/// Clear the context cache, the log ring and the capture between test cases.
#[doc(hidden)]
pub fn reset_for_tests() {
    *CTX_CACHE.lock() = None;
    let mut tap = TAP.lock();
    tap.ring.clear();
    tap.log_seq = 0;
    tap.capture = None;
}
