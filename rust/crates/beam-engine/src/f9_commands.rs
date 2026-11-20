//! Owner: E6. The F9 command handler: `_F9_HELP`, `_invoke_f9_command`,
//! `_mcp_press_command`, `_on_next_key_press` (`beamtel.py:5260-5341, 8259-9352`).
//!
//! The branch order in [`on_next_key_press`] IS the behaviour: several branches test only
//! some modifiers, so an earlier, looser branch swallows combinations a later one names.
//! `rust_plans/f9-command-table.md` lists every key and modifier combination and the
//! branch that answers it.
//!
//! Every call this handler makes into another module, a socket, a thread or `sleep` goes
//! through one private wrapper, so a test can replace them all with a recorder
//! ([`set_external_override`]), as the Python generator replaces the module's functions.

use std::f64::consts::PI;
use std::io;
use std::net::UdpSocket;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use beam_base::gear_report::{DEFAULT_NUMBER_STYLE, extended_gear_to_phrase, gear_to_phrase};
use beam_base::pyfmt::{py_f, py_float_str, py_strip};
use beam_base::route_beacon::relative_bearing;
use indexmap::IndexMap;
use parking_lot::RwLock;
use serde_json::{Value, json};

use crate::keyboard::{KeyEvent, KeyEventType, clear_next_key_hook};
use crate::listeners::dock_phrases::{
    CANNON_READOUT_STALE_S, IMPL_DOCK_LEVEL_M, band_name, dock_phrase, ramp_self_phrase,
};
use crate::net::{DUMP_RESP_PORT, LOCALHOST, SCANNER_CMD_PORT};
use crate::speech_out::{
    command_context, say, set_command_context, set_speech_logging_active, speech_buffer_recent,
    speech_log_path, speech_logging_active,
};
use crate::state::{CannonAim, DockReading, Mods, RampSelf, audio_controller_ref, state};
use crate::timer::{monotonic, time_time};
use crate::units::{
    PyVal, flip_units, fmt_distance, fmt_heading, fmt_pressure, fmt_speed, fmt_temp_c_or_f,
    fmt_turbo, has_air_system, has_engine_temp, py_int_round, units_mode,
};
use crate::virtual_browser::BrowserOpts;
use beam_audio::controller::AudioController;

/// A help key: (name, ctrl, shift, alt).
pub type HelpKey = (&'static str, bool, bool, bool);

/// `_F9_HELP`: F9 command descriptions keyed by (name, ctrl, shift, alt), in the Python's
/// insertion order (the MCP server lists them in this order).
pub static F9_HELP: LazyLock<IndexMap<HelpKey, &'static str>> = LazyLock::new(|| {
    const F: bool = false;
    const T: bool = true;
    let entries: &[(HelpKey, &'static str)] = &[
        (("s", F, F, F), "Speak speed and map speed limit"),
        (("r", F, F, F), "Speak RPM"),
        (("r", F, T, F), "Speak redline RPM"),
        (("g", F, F, F), "Speak gear"),
        (("f", F, F, F), "Speak fuel"),
        (("e", F, F, F), "Speak engine temperature"),
        (("e", F, T, F), "Dump electrics to log"),
        (("p", F, T, F), "Dump powertrain to log"),
        (("h", F, T, F), "Dump hydros to log"),
        (("t", F, F, F), "Speak boost pressure"),
        (("t", F, T, F), "Speak max boost pressure"),
        (("t", F, F, T), "Speak forced induction details"),
        (("t", T, F, F), "Speak ambient temperature"),
        (("o", F, F, F), "Speak oil temperature"),
        (("a", F, F, F), "Speak attitude, roll and pitch"),
        (("c", F, F, F), "Speak coordinates"),
        (("c", F, T, F), "Mark waypoint at current position"),
        (("c", F, F, T), "Speak marked waypoint"),
        (("w", F, F, F), "Distance and bearing to waypoint"),
        (("w", T, F, F), "Toggle route beacon"),
        (("w", F, T, F), "Nearest point of interest"),
        (
            ("f", F, T, F),
            "Fuel stations: choose one, park at a pump once there, or cancel parking",
        ),
        (("f", T, T, F), "List fuel stations"),
        (("d", F, F, F), "Scanner distance and orientation"),
        (("d", F, T, F), "Scanner relative bearing to target"),
        (("d", T, F, F), "Toggle drift detection"),
        (("d", T, T, F), "Toggle coupler distance callouts"),
        (("m", F, F, F), "Damage report"),
        (("s", T, F, F), "Toggle status mode"),
        (("b", F, F, F), "Browse vehicle bindings"),
        (("n", F, F, F), "Browse environment settings"),
        (
            ("j", F, F, F),
            "Audio debug: engine sounds and sound library",
        ),
        (("b", T, F, F), "Toggle buffer mode"),
        (("c", T, F, F), "Toggle pedal tones"),
        (("v", T, F, F), "Toggle vehicle scanner"),
        (
            ("v", F, T, F),
            "Align to a ramp when the docking instrument is on, otherwise to a trailer \
             coupler, starting coupler tracking and the attach monitor",
        ),
        (("o", T, F, F), "Toggle obstacle detection"),
        (("r", T, F, F), "Toggle road detection"),
        (("r", T, T, F), "Read road guidance status"),
        (("tab", F, F, F), "Next scanner target"),
        (("tab", F, T, F), "Previous scanner target"),
        (("tab", T, F, F), "Closest scanner target"),
        (("h", T, F, F), "Toggle heading guidance"),
        (("g", T, F, F), "Toggle coordinate guidance"),
        (("l", T, F, F), "DOM dump"),
        (("l", T, T, F), "Toggle low speed detection"),
        (("k", T, F, F), "Toggle wheel slip detection"),
        (("s", T, T, F), "Toggle speech logger"),
        (("f", F, F, T), "Toggle camera info"),
        (
            ("g", F, F, T),
            "Toggle camera line-up guide: X mode, arrows distance, T target, M mute, \
             Escape releases the keys",
        ),
        (("h", F, F, T), "Camera heading"),
        (("a", F, F, T), "Camera altitude"),
        (("p", F, F, T), "Camera pitch"),
        (("v", F, F, T), "Vehicle bearing from camera"),
        (("d", F, F, T), "Vehicle distance from camera"),
        (("h", F, F, F), "Speak heading"),
        (("p", F, F, F), "Speak air pressure"),
        (("u", F, F, F), "Switch between imperial and metric"),
        (
            ("space", F, F, F),
            "Read vehicle information on the vehicle selector, scan terrain when driving, or activate the on-screen control in a menu",
        ),
        (("n", T, F, F), "Toggle accessible node grabber"),
        (("c", T, T, F), "Toggle clickspot detection"),
        (("c", T, T, T), "Browse clickspots"),
        (
            ("i", F, F, F),
            "Alignment readout, or cannon aim when in a cannon",
        ),
        (("i", F, T, F), "Cycle alignment reference band"),
        (
            ("i", T, F, F),
            "Toggle alignment instrument (implement or ramp)",
        ),
        (("b", F, T, F), "Toggle learn bindings mode"),
    ];
    entries.iter().copied().collect()
});

// ---- The external-call seam ------------------------------------------------------------

/// A replacement for every call this handler makes outside itself: `(name, args) -> return`.
/// Names and argument shapes are the Python's, so a recording from the Python handler can
/// be compared call for call. For tests only.
#[doc(hidden)]
pub type ExternalHook = Arc<dyn Fn(&str, &Value) -> Value + Send + Sync>;

static EXTERNAL: RwLock<Option<ExternalHook>> = RwLock::new(None);

/// Replace (or with `None`, restore) the calls this handler makes into other modules, the
/// sockets, `threading.Thread` and `time.sleep`. For tests only.
#[doc(hidden)]
pub fn set_external_override(hook: Option<ExternalHook>) {
    *EXTERNAL.write() = hook;
}

fn intercept(name: &str, args: Value) -> Option<Value> {
    let hook = EXTERNAL.read().clone()?;
    Some(hook(name, &args))
}

/// The controller the toggles are handed. The Python passes `audio_controller` along even
/// when it is None and the callee fails on first use; the Rust callees take it by value,
/// so the failure is raised here instead.
fn need_ac(ac: &Option<Arc<AudioController>>) -> Arc<AudioController> {
    match ac {
        Some(a) => Arc::clone(a),
        None => panic!("AttributeError: 'NoneType' object has no attribute 'set_dock_mode'"),
    }
}

fn ext_unit(name: &str, args: Value, real: impl FnOnce()) {
    if intercept(name, args).is_none() {
        real();
    }
}

fn ext_ac(name: &str, ac: &Option<Arc<AudioController>>, real: impl FnOnce(Arc<AudioController>)) {
    if intercept(name, json!([])).is_none() {
        real(need_ac(ac));
    }
}

fn x_str(name: &str, args: Value, real: impl FnOnce() -> String) -> String {
    match intercept(name, args) {
        Some(v) => v.as_str().unwrap_or_default().to_string(),
        None => real(),
    }
}

fn x_bool(name: &str, real: impl FnOnce() -> bool) -> bool {
    match intercept(name, json!([])) {
        Some(v) => v.as_bool().unwrap_or(false),
        None => real(),
    }
}

fn x_camera_feed_problem() -> Option<String> {
    match intercept("_camera_feed_problem", json!([])) {
        Some(v) => v.as_str().map(str::to_string),
        None => crate::listeners::camera::camera_feed_problem(),
    }
}

type Failure = (String, String);

fn x_request_vehicle_info() -> (Vec<String>, Option<Failure>) {
    match intercept("request_vehicle_info", json!([])) {
        Some(v) => {
            let lines = v[0]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|l| l.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let fail = v[1].as_array().map(|f| {
                (
                    f.first()
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    f.get(1)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            });
            (lines, fail)
        }
        None => crate::browsers::request_vehicle_info(None, None, 0.6),
    }
}

fn x_open_virtual_browser(lines: Vec<String>, title: String) {
    if intercept("open_virtual_browser", json!([lines, title])).is_none() {
        crate::virtual_browser::open_virtual_browser(lines, BrowserOpts::titled(title));
    }
}

fn x_send_implement_cmd(cmd: &str) {
    ext_unit("_send_implement_cmd", json!([cmd]), || {
        crate::listeners::implement::send_implement_cmd(cmd)
    });
}

fn x_set_dock_mode(ac: &Option<Arc<AudioController>>, active: bool) {
    if intercept("audio_controller.set_dock_mode", json!([active])).is_none() {
        need_ac(ac).set_dock_mode(active);
    }
}

fn x_broadcast(msg: Value) {
    if intercept("broadcast", json!([msg])).is_none() {
        crate::ui_bridge::broadcast(&msg);
    }
}

fn x_sleep(seconds: f64) {
    if intercept("time.sleep", json!([py_float_str(seconds)])).is_none() {
        crate::timer::sleep(seconds);
    }
}

fn x_dock_phrase(snap: &DockReading) -> String {
    x_str("_dock_phrase", json!([snap.name]), || dock_phrase(snap))
}

fn x_ramp_self_phrase(deck: &RampSelf) -> String {
    x_str("_ramp_self_phrase", json!([]), || ramp_self_phrase(deck))
}

fn x_band_name(kind: &str, idx: i64, count: i64) -> String {
    x_str("_band_name", json!([kind, idx, count]), || {
        band_name(kind, idx, count)
    })
}

/// `cmd_sock = socket.socket(...); cmd_sock.sendto(payload, ("127.0.0.1", SCANNER_CMD_PORT))`.
fn scanner_sendto(payload: &[u8]) -> io::Result<()> {
    let args = json!([SCANNER_CMD_PORT, String::from_utf8_lossy(payload)]);
    if intercept("udp", args).is_some() {
        return Ok(());
    }
    crate::net::send_udp(SCANNER_CMD_PORT, payload)
}

// ---- The 4447 dumps ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DumpKind {
    Electrics,
    Powertrain,
    Hydros,
    Damage,
}

impl DumpKind {
    /// The nested function's name in the Python.
    fn target(self) -> &'static str {
        match self {
            DumpKind::Electrics => "_do_dump",
            DumpKind::Powertrain => "_do_pdump",
            DumpKind::Hydros => "_do_hdump",
            DumpKind::Damage => "_do_damage_report",
        }
    }

    fn command(self) -> &'static [u8] {
        match self {
            DumpKind::Electrics => b"DUMP",
            DumpKind::Powertrain => b"PDUMP",
            DumpKind::Hydros => b"HDUMP",
            DumpKind::Damage => b"DAMAGE",
        }
    }
}

enum DumpRecv {
    Data(Vec<u8>),
    Timeout,
}

/// The two sockets a dump uses: the receiver bound on 4447, and a throwaway sender.
trait DumpIo {
    fn bind(&mut self) -> io::Result<()>;
    fn sendto(&mut self, payload: &[u8]) -> io::Result<()>;
    fn settimeout(&mut self, seconds: f64) -> io::Result<()>;
    fn recvfrom(&mut self) -> io::Result<DumpRecv>;
}

struct UdpDumpIo {
    sock: Option<UdpSocket>,
    buf: Vec<u8>,
}

impl DumpIo for UdpDumpIo {
    fn bind(&mut self) -> io::Result<()> {
        self.sock = Some(UdpSocket::bind((LOCALHOST, DUMP_RESP_PORT))?);
        Ok(())
    }

    fn sendto(&mut self, payload: &[u8]) -> io::Result<()> {
        crate::net::send_udp(SCANNER_CMD_PORT, payload)
    }

    fn settimeout(&mut self, seconds: f64) -> io::Result<()> {
        if let Some(s) = &self.sock {
            s.set_read_timeout(Some(
                Duration::from_secs_f64(seconds).max(Duration::from_millis(1)),
            ))?;
        }
        Ok(())
    }

    fn recvfrom(&mut self) -> io::Result<DumpRecv> {
        let Some(s) = &self.sock else {
            return Err(io::Error::other("socket is not bound"));
        };
        match s.recv_from(&mut self.buf) {
            Ok((n, _)) => Ok(DumpRecv::Data(self.buf[..n].to_vec())),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(DumpRecv::Timeout)
            }
            Err(e) => Err(e),
        }
    }
}

/// Datagrams handed over by a test recorder; then a timeout. Socket calls are reported to
/// the recorder in the order the Python's fake socket sees them.
struct ScriptedDumpIo {
    datagrams: std::collections::VecDeque<Vec<u8>>,
}

impl DumpIo for ScriptedDumpIo {
    fn bind(&mut self) -> io::Result<()> {
        intercept("socket.bind", json!([DUMP_RESP_PORT]));
        Ok(())
    }

    fn sendto(&mut self, payload: &[u8]) -> io::Result<()> {
        intercept(
            "udp",
            json!([SCANNER_CMD_PORT, String::from_utf8_lossy(payload)]),
        );
        Ok(())
    }

    fn settimeout(&mut self, seconds: f64) -> io::Result<()> {
        intercept("socket.settimeout", json!([py_float_str(seconds)]));
        Ok(())
    }

    fn recvfrom(&mut self) -> io::Result<DumpRecv> {
        Ok(match self.datagrams.pop_front() {
            Some(d) => DumpRecv::Data(d),
            None => DumpRecv::Timeout,
        })
    }
}

/// `data.decode("utf-8", errors="replace").strip()`.
fn dump_text(data: &[u8]) -> String {
    py_strip(&String::from_utf8_lossy(data)).to_string()
}

/// The receive loop shared by the three log dumps. `Err` is an exception the Python thread
/// would die with (it catches only `socket.timeout`).
fn collect_dump_lines(io: &mut dyn DumpIo, kind: DumpKind) -> io::Result<Vec<String>> {
    // Open the receive socket first so the response is not missed.
    io.bind()?;
    io.sendto(kind.command())?;
    io.settimeout(5.0)?; // wait up to 5 s for the first packet
    let mut lines = Vec::new();
    for _ in 0..1000 {
        match io.recvfrom()? {
            DumpRecv::Timeout => break,
            DumpRecv::Data(data) => {
                let text = dump_text(&data);
                if text == "DONE" {
                    break;
                }
                lines.push(text);
                io.settimeout(0.5)?; // shorter timeout after the first
            }
        }
    }
    Ok(lines)
}

fn run_log_dump(io: &mut dyn DumpIo, kind: DumpKind) {
    let (title, word) = match kind {
        DumpKind::Electrics => ("ELECTRICS", "Electrics"),
        DumpKind::Powertrain => ("POWERTRAIN", "Powertrain"),
        DumpKind::Hydros => ("HYDROS", "Hydros"),
        DumpKind::Damage => return run_damage_report(io),
    };
    let mut lines = match collect_dump_lines(io, kind) {
        Ok(l) => l,
        Err(e) => {
            log::error!("Exception in thread {}: {e}", kind.target());
            return;
        }
    };
    if !lines.is_empty() {
        lines.sort();
        let content = lines.join("\n");
        let text = format!(
            "=== {title} DUMP ({} values) ===\n{content}\n=== END DUMP ===",
            lines.len()
        );
        log::info!("{text}");
        intercept("logger.info", json!([text]));
        say(
            &format!("{word} dumped, {} values logged", lines.len()),
            true,
            true,
            "python",
        );
    } else {
        say(
            &format!("{word} dump timed out \u{2014} is a vehicle spawned?"),
            true,
            true,
            "python",
        );
    }
}

fn collect_damage(io: &mut dyn DumpIo) -> io::Result<(String, Vec<String>)> {
    io.bind()?;
    io.sendto(DumpKind::Damage.command())?;
    io.settimeout(5.0)?;
    let mut veh_name = String::new();
    let mut items = Vec::new();
    for _ in 0..200 {
        match io.recvfrom()? {
            DumpRecv::Timeout => break,
            DumpRecv::Data(data) => {
                let text = dump_text(&data);
                if text == "DONE" {
                    break;
                }
                if text == "NONE" {
                    continue;
                }
                if let Some(rest) = text.strip_prefix("NAME:") {
                    veh_name = rest.to_string();
                    continue;
                }
                items.push(text);
                io.settimeout(0.5)?;
            }
        }
    }
    Ok((veh_name, items))
}

fn run_damage_report(io: &mut dyn DumpIo) {
    let (veh_name, items) = match collect_damage(io) {
        Ok(r) => r,
        Err(e) => {
            log::error!("Exception in thread {}: {e}", DumpKind::Damage.target());
            return;
        }
    };
    let preamble = if !veh_name.is_empty() {
        format!("Damage report for {veh_name}")
    } else {
        "Damage report".to_string()
    };
    if items.is_empty() {
        say(
            &format!("{preamble}: no damaged components"),
            true,
            true,
            "python",
        );
    } else {
        let sentence = match items.len() {
            1 => items[0].clone(),
            2 => format!("{} and {}", items[0], items[1]),
            n => format!("{}, and {}", items[..n - 1].join(", "), items[n - 1]),
        };
        say(&format!("{preamble}: {sentence}"), true, true, "python");
    }
}

/// `threading.Thread(target=_do_..., daemon=True).start()`.
fn start_dump_thread(kind: DumpKind) {
    match intercept("threading.Thread", json!([kind.target()])) {
        None => crate::timer::spawn_daemon(kind.target(), move || {
            let mut io = UdpDumpIo {
                sock: None,
                buf: vec![0u8; 4096],
            };
            run_log_dump(&mut io, kind);
        }),
        // A recorder that hands back datagrams runs the body now against them; one that
        // hands back nothing only records the thread.
        Some(Value::Array(datagrams)) => {
            let mut io = ScriptedDumpIo {
                datagrams: datagrams
                    .iter()
                    .map(|d| match d {
                        Value::String(s) => s.as_bytes().to_vec(),
                        Value::Array(b) => {
                            b.iter().map(|x| x.as_u64().unwrap_or(0) as u8).collect()
                        }
                        _ => Vec::new(),
                    })
                    .collect(),
            };
            run_log_dump(&mut io, kind);
        }
        Some(_) => {}
    }
}

// ---- Small Python semantics --------------------------------------------------------------

/// `say(text, exclude_from_buffer=True)`.
fn say_x(text: &str) {
    say(text, true, true, "python");
}

/// `say(text)`.
fn say_p(text: &str) {
    say(text, true, false, "python");
}

/// Python's `min(a, b)`: `a` unless `b < a`.
fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// Python's `max(a, b)`: `a` unless `b > a`.
fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// `math.degrees(x)`: CPython divides by `pi / 180`.
fn py_degrees(x: f64) -> f64 {
    x / (PI / 180.0)
}

/// `name.isdigit()` for the key names the layer delivers.
fn is_digit_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_digit())
}

/// Restores `_capture_mods` and `_command_context` on the way out, panic or not
/// (`_invoke_f9_command`'s `finally`).
struct RestoreCapture {
    saved: Mods,
    prev_ctx: bool,
}

impl Drop for RestoreCapture {
    fn drop(&mut self) {
        set_command_context(self.prev_ctx);
        state().kb.capture_mods = self.saved;
    }
}

/// `_invoke_f9_command(name, ctrl, shift, alt)`: invoke an F9 command in-process without
/// injecting an operating-system key.
///
/// The handler reads only the event's type and name, with the modifiers coming from
/// `_capture_mods`, so controller and MCP commands reuse the keyboard handlers without
/// BeamNG having focus. The modifiers are saved and restored because a real capture may be
/// in progress, and the command context is set so speech behaves as for a real keypress.
/// `Err` is the Python's `RuntimeError("audio controller is not up yet")`.
pub fn invoke_f9_command(name: &str, ctrl: bool, shift: bool, alt: bool) -> Result<bool, String> {
    let Some(ac) = audio_controller_ref() else {
        return Err("audio controller is not up yet".to_string());
    };
    let saved = state().kb.capture_mods;
    let _restore = RestoreCapture {
        saved,
        prev_ctx: command_context(),
    };
    state().kb.capture_mods = Mods { ctrl, shift, alt };
    set_command_context(true);
    let evt = KeyEvent::down(&name.to_lowercase());
    on_next_key_press(&evt, Some(ac));
    Ok(true)
}

/// `_mcp_press_command(...)`: the MCP-facing wrapper.
pub fn mcp_press_command(name: &str, ctrl: bool, shift: bool, alt: bool) -> Result<bool, String> {
    invoke_f9_command(name, ctrl, shift, alt)
}

/// What the handler reads from the shared state before dispatching, under one lock.
struct Snap {
    spd_val: PyVal,
    spd_unit: &'static str,
    rpm: i64,
    fuel_pct: i64,
    turbo_val: PyVal,
    turbo_unit: &'static str,
    etemp_val: PyVal,
    etemp_unit: &'static str,
    otemp_val: PyVal,
    otemp_unit: &'static str,
    engine_temp: bool,
    gear_phrase: String,
    x: f64,
    y: f64,
    z: f64,
    roll_deg: f64,
    pitch_deg: f64,
    hdg: String,
    heading: f64,
    rpm_max: f64,
    turbo_max: f64,
    air_pressure: f64,
    air_pressure_max: f64,
    cam_yaw: f64,
    cam_pitch: f64,
    cam_agl: f64,
    cam_agl_valid: bool,
    cam_bearing: f64,
    cam_dist: f64,
}

fn snapshot() -> Snap {
    let st = state();
    let t = &st.tel;
    let (spd_val, spd_unit) = fmt_speed(t.last_speed_ms);
    let rpm = py_int_round(t.last_rpm);
    let fuel_pct = py_int_round(py_max(0.0, py_min(1.0, t.last_fuel)) * 100.0);
    let (turbo_val, turbo_unit) = fmt_turbo(t.last_turbo);
    let (etemp_val, etemp_unit) = fmt_temp_c_or_f(t.last_engtemp);
    let (otemp_val, otemp_unit) = fmt_temp_c_or_f(t.last_oiltemp);
    let engine_temp = has_engine_temp(&st);
    let gear_phrase = if t.protocol_mode == "outgauge" {
        gear_to_phrase(t.last_gear_byte.unwrap_or(1), DEFAULT_NUMBER_STYLE)
    } else {
        extended_gear_to_phrase(
            t.last_gear_str.as_deref().unwrap_or(""),
            DEFAULT_NUMBER_STYLE,
        )
    };
    Snap {
        spd_val,
        spd_unit,
        rpm,
        fuel_pct,
        turbo_val,
        turbo_unit,
        etemp_val,
        etemp_unit,
        otemp_val,
        otemp_unit,
        engine_temp,
        gear_phrase,
        x: t.last_pos_x,
        y: t.last_pos_y,
        z: t.last_pos_z,
        roll_deg: py_degrees(t.last_roll_rad),
        pitch_deg: py_degrees(t.last_pitch_rad),
        hdg: fmt_heading(&st),
        heading: t.last_heading,
        rpm_max: t.last_rpm_max,
        turbo_max: t.last_turbo_max,
        air_pressure: t.last_air_pressure,
        air_pressure_max: t.last_air_pressure_max,
        cam_yaw: st.camera.cam_yaw_deg,
        cam_pitch: st.camera.cam_pitch_deg,
        cam_agl: st.camera.cam_agl,
        cam_agl_valid: st.camera.cam_agl_valid,
        cam_bearing: st.camera.cam_veh_bearing,
        cam_dist: st.camera.cam_veh_distance,
    }
}

fn mods() -> Mods {
    state().kb.capture_mods
}

/// `_on_next_key_press(event, audio_controller)`: one F9-layer key on the worker.
pub fn on_next_key_press(event: &KeyEvent, audio_controller: Option<Arc<AudioController>>) {
    let ac = &audio_controller;
    if event.event_type != KeyEventType::Down {
        return;
    }
    let name = event.name.to_lowercase();
    let name = name.as_str();

    match name {
        "ctrl" | "control" | "left ctrl" | "right ctrl" => {
            state().kb.capture_mods.ctrl = true;
            return;
        }
        "shift" | "left shift" | "right shift" => {
            state().kb.capture_mods.shift = true;
            return;
        }
        "alt" | "left alt" | "right alt" => {
            state().kb.capture_mods.alt = true;
            return;
        }
        _ => {}
    }

    if name == "f9" {
        return;
    }

    // Toggle input help mode with ? (shift+/)
    if name == "/" || name == "?" {
        let on = {
            let mut st = state();
            st.kb.input_help_mode = !st.kb.input_help_mode;
            st.kb.input_help_mode
        };
        if on {
            say_x("Input help on");
            // Cancel the timeout so the layer stays open.
            let timer = state().kb.next_key_timer.clone();
            if let Some(t) = timer {
                t.cancel();
            }
        } else {
            say_x("Input help off");
            clear_next_key_hook(false);
        }
        return;
    }

    // In help mode: speak what the key does, don't execute.
    if state().kb.input_help_mode {
        let m = mods();
        let mut desc: Option<String> = F9_HELP
            .iter()
            .find(|((k, c, s, a), _)| *k == name && *c == m.ctrl && *s == m.shift && *a == m.alt)
            .map(|(_, v)| v.to_string());
        if is_digit_name(name) && !(m.ctrl || m.shift || m.alt) {
            desc = Some(if name != "0" {
                format!("Speak buffer message {name}")
            } else {
                "Speak buffer message 10".to_string()
            });
        }
        match desc.filter(|d| !d.is_empty()) {
            Some(d) => say_x(&d),
            None => say_x("No command"),
        }
        return;
    }

    let m = mods();
    if is_digit_name(name) && !(m.ctrl || m.shift || m.alt) {
        let target_recency: usize = if name == "0" {
            10
        } else {
            name.parse().unwrap_or(usize::MAX)
        };
        {
            let _st = state();
            match speech_buffer_recent(target_recency) {
                Some(message) => say_x(&message),
                None => say_x("No message"),
            }
        }
        clear_next_key_hook(false);
        return;
    }

    let s = snapshot();
    let (ctrl, shift, alt) = (m.ctrl, m.shift, m.alt);

    // ALT+ camera info commands (before the bare key handlers)
    if name == "g" && alt && !(ctrl || shift) {
        clear_next_key_hook(false);
        ext_unit("open_camera_lineup", json!([]), || {
            crate::listeners::lineup::open_camera_lineup()
        });
        return;
    } else if name == "f" && alt && !(ctrl || shift) {
        ext_ac("toggle_free_cam_info", ac, |a| {
            crate::listeners::camera::toggle_free_cam_info(a)
        });
        clear_next_key_hook(false);
        return;
    } else if name == "h" && alt && !(ctrl || shift) {
        match x_camera_feed_problem() {
            None => say_x(&format!("Camera heading {} degrees", py_f(s.cam_yaw, 0))),
            Some(p) => say_x(&p),
        }
        clear_next_key_hook(false);
        return;
    } else if name == "a" && alt && !(ctrl || shift) {
        match x_camera_feed_problem() {
            None => {
                let (alt_val, unit) = if units_mode() == "imperial" {
                    (s.cam_agl * 3.28084, "feet")
                } else {
                    (s.cam_agl, "meters")
                };
                // With no ground found under the camera the number is an absolute height,
                // so say so instead of calling sea level the ground.
                let suffix = if s.cam_agl_valid {
                    ""
                } else {
                    " above sea level, ground unknown"
                };
                say_x(&format!(
                    "Camera altitude {} {unit}{suffix}",
                    py_f(alt_val, 1)
                ));
            }
            Some(p) => say_x(&p),
        }
        clear_next_key_hook(false);
        return;
    } else if name == "a" && alt && shift && !ctrl {
        // Camera/ground diagnostic: a spoken summary, the detail in bnvdahook.log, so a
        // wrong altitude can be traced to the query behind it.
        ext_unit("send_camera_diag", json!([]), || {
            crate::listeners::camera::send_camera_diag()
        });
        clear_next_key_hook(false);
        return;
    } else if name == "p" && alt && !(ctrl || shift) {
        match x_camera_feed_problem() {
            None => {
                let direction = if s.cam_pitch >= 0.0 { "up" } else { "down" };
                say_x(&format!(
                    "Camera pitch {} degrees {direction}",
                    py_f(s.cam_pitch.abs(), 0)
                ));
            }
            Some(p) => say_x(&p),
        }
        clear_next_key_hook(false);
        return;
    } else if name == "v" && alt && !(ctrl || shift) {
        match x_camera_feed_problem() {
            None => {
                if s.cam_dist < 0.0 {
                    say_x("No vehicle");
                } else {
                    let direction = if s.cam_bearing > 0.0 { "left" } else { "right" };
                    say_x(&format!(
                        "Vehicle {} degrees {direction}",
                        py_f(s.cam_bearing.abs(), 0)
                    ));
                }
            }
            Some(p) => say_x(&p),
        }
        clear_next_key_hook(false);
        return;
    } else if name == "d" && alt && !(ctrl || shift) {
        match x_camera_feed_problem() {
            None => {
                if s.cam_dist < 0.0 {
                    say_x("No vehicle");
                } else if units_mode() == "imperial" {
                    let dist_val = s.cam_dist * 3.28084;
                    say_x(&format!("Vehicle {} feet away", py_f(dist_val, 0)));
                } else {
                    say_x(&format!("Vehicle {} meters away", py_f(s.cam_dist, 0)));
                }
            }
            Some(p) => say_x(&p),
        }
        clear_next_key_hook(false);
        return;
    }

    if name == "s" && !ctrl {
        let enabled = state().road_mode_active || crate::state::driving_assistant().active();
        let units = units_mode();
        let limit = x_str(
            "ROAD_GUIDANCE_FEED.speed_limit_phrase",
            json!([enabled, units, "Speed limit"]),
            || {
                crate::state::road_guidance_feed().speed_limit_phrase(
                    enabled,
                    &units,
                    "Speed limit",
                )
            },
        );
        say_p(&format!("{} {}. {limit}", s.spd_val, s.spd_unit));
    } else if name == "r" && shift && !ctrl {
        let extended = state().tel.protocol_mode == "extended";
        say_p(&if extended {
            format!("Redline {} RPM", py_int_round(s.rpm_max))
        } else {
            "Unavailable".to_string()
        });
    } else if name == "r" && ctrl && shift && !alt {
        ext_unit("speak_road_status", json!([]), || {
            crate::listeners::road::speak_road_status()
        });
    } else if name == "r" && ctrl && !(shift || alt) {
        ext_ac("toggle_road_mode", ac, |a| {
            crate::listeners::road::toggle_road_mode(a)
        });
    } else if name == "r" && !(ctrl || shift || alt) {
        say_p(&format!("{} rpm", s.rpm));
    } else if name == "h" && ctrl {
        let on = {
            let mut st = state();
            st.tel.heading_guidance_active = !st.tel.heading_guidance_active;
            if st.tel.heading_guidance_active {
                st.tel.heading_guidance_target = st.tel.last_heading;
            }
            st.tel.heading_guidance_active
        };
        if on {
            say_x("Heading guidance on");
        } else {
            say_x("Heading guidance off");
        }
    } else if name == "d" && ctrl && shift && !alt {
        let scanning = state().scanner.scan_mode_active;
        if !scanning {
            say_x("Vehicle scanner is not active");
        } else {
            {
                let mut st = state();
                st.scanner.coupler_dist_mode = !st.scanner.coupler_dist_mode;
            }
            if let Err(e) = scanner_sendto(b"COUPLER_DIST") {
                log::error!("Failed to send COUPLER_DIST command via UDP: {e}");
            }
        }
    } else if name == "d" && ctrl && !(shift || alt) {
        let on = {
            let mut st = state();
            st.tel.drift_mode_active = !st.tel.drift_mode_active;
            if st.tel.drift_mode_active {
                st.tel.last_drift_check_ts = time_time();
                st.tel.drift_baseline_heading = st.tel.last_heading;
            }
            st.tel.drift_alert_active = false;
            st.tel.drift_mode_active
        };
        if on {
            say_x("Drift detection on");
        } else {
            say_x("Drift detection off");
        }
    } else if name == "d" && shift && !(ctrl || alt) {
        let scanning = state().scanner.scan_mode_active;
        if !scanning {
            say_x("Scanner is not active");
        } else {
            let (brg, dist) = {
                let st = state();
                (
                    st.scanner.last_scanner_bearing,
                    st.scanner.last_scanner_distance,
                )
            };
            if dist == f64::INFINITY {
                say_x("No target");
            } else {
                let direction = if brg >= 0.0 { "left" } else { "right" };
                say_x(&format!("{} degrees {direction}", py_f(brg.abs(), 0)));
            }
        }
    } else if name == "d" && !(ctrl || shift || alt) {
        let scanning = state().scanner.scan_mode_active;
        if !scanning {
            say_x("Scanner is not active");
        } else {
            let (dist, brg, tname, approach) = {
                let st = state();
                (
                    st.scanner.last_scanner_distance,
                    st.scanner.last_scanner_bearing,
                    st.scanner.last_scanner_target_name.clone(),
                    st.scanner.last_scanner_approach_deg,
                )
            };
            if dist == f64::INFINITY {
                say_x("No target");
            } else {
                let a = brg.abs();
                let orientation = if a < 30.0 {
                    "approaching"
                } else if a < 60.0 {
                    "angling toward"
                } else if a < 120.0 {
                    "broadside"
                } else if a < 150.0 {
                    "angling away"
                } else {
                    "departing"
                };
                let ap = approach.abs();
                let side = if ap < 45.0 {
                    "front"
                } else if ap > 135.0 {
                    "rear"
                } else if approach > 0.0 {
                    // positive = the player is off the target's left
                    "left side"
                } else {
                    "right side"
                };
                let dist_str = if units_mode() == "imperial" {
                    format!("{} feet", py_f(dist * 3.28084, 0))
                } else {
                    format!("{} meters", py_f(dist, 0))
                };
                let prefix = if !tname.is_empty() {
                    format!("{tname}, ")
                } else {
                    String::new()
                };
                say_x(&format!("{prefix}{dist_str} {orientation} {side}"));
            }
        }
    } else if name == "i" && !(ctrl || shift || alt) {
        cane_tap();
    } else if name == "i" && shift && !(ctrl || alt) {
        cycle_reference_band();
    } else if name == "i" && ctrl && !(shift || alt) {
        toggle_docking_instrument(ac);
    } else if name == "w" && ctrl && !(shift || alt) {
        ext_ac("toggle_route_beacon", ac, |a| {
            crate::listeners::route_beacon::toggle_route_beacon(a)
        });
    } else if name == "w" && shift && !(ctrl || alt) {
        ext_unit("trigger_nearest_poi", json!([]), || {
            crate::listeners::terrain_scan::trigger_nearest_poi()
        });
    } else if name == "w" && !(ctrl || shift || alt) {
        let (mx, my) = {
            let st = state();
            (st.tel.marked_coord_x, st.tel.marked_coord_y)
        };
        match mx {
            None => say_x("No waypoint marked"),
            Some(mx) => {
                let my = my.unwrap_or_else(|| {
                    panic!("TypeError: unsupported operand type(s) for -: 'NoneType' and 'float'")
                });
                let (dist, err) = relative_bearing(s.x, s.y, s.heading, mx, my);
                if dist < 0.5 {
                    say_x("At waypoint");
                } else {
                    let turn_dir = if err > 0.0 { "left" } else { "right" };
                    let dist_str = if units_mode() == "imperial" {
                        format!("{} feet", py_f(dist * 3.28084, 0))
                    } else {
                        format!("{} meters", py_f(dist, 0))
                    };
                    say_x(&format!(
                        "{dist_str}, {} degrees {turn_dir}",
                        py_f(err.abs(), 0)
                    ));
                }
            }
        }
    } else if name == "s" && ctrl && shift && !alt {
        set_speech_logging_active(!speech_logging_active());
        if speech_logging_active() {
            // Clear the log on each new session.
            let _ = std::fs::File::create(speech_log_path());
            say_x("Speech logging on");
        } else {
            say_x(&format!(
                "Speech logging off, saved to {}",
                speech_log_path().display()
            ));
        }
    } else if name == "l" && ctrl && shift && !alt {
        let on = {
            let mut st = state();
            st.tel.low_speed_mode_active = !st.tel.low_speed_mode_active;
            st.tel.low_speed_mode_active
        };
        say_x(if on {
            "Low speed detection on"
        } else {
            "Low speed detection off"
        });
    } else if name == "k" && ctrl && !(shift || alt) {
        let on = {
            let mut st = state();
            st.tel.slip_mode_active = !st.tel.slip_mode_active;
            st.tel.slip_mode_active
        };
        say_x(if on {
            "Wheel slip detection on"
        } else {
            "Wheel slip detection off"
        });
    } else if name == "l" && ctrl && !(shift || alt) {
        say_x("Dumping DOM");
        x_broadcast(json!({"type": "dom_dump"}));
    } else if name == "g" && ctrl {
        let spoken = {
            let mut st = state();
            st.tel.coord_guidance_active = !st.tel.coord_guidance_active;
            if st.tel.coord_guidance_active {
                if st.tel.marked_coord_x.is_none() {
                    st.tel.coord_guidance_active = false;
                    "No waypoint marked"
                } else {
                    "Coordinate guidance on"
                }
            } else {
                "Coordinate guidance off"
            }
        };
        say_x(spoken);
    } else if name == "h" && shift {
        say_x("Requesting hydros dump");
        start_dump_thread(DumpKind::Hydros);
    } else if name == "h" {
        say_p(&s.hdg);
    } else if name == "f" && shift && !alt {
        // Ctrl+Shift+F always lists; Shift+F parks when at the chosen station.
        ext_unit("open_fuel_stations", json!([ctrl]), || {
            crate::listeners::fuel::open_fuel_stations(ctrl)
        });
    } else if name == "f" {
        say_p(&format!("Fuel {} percent", s.fuel_pct));
    } else if name == "g" {
        say_p(&s.gear_phrase);
    } else if name == "t" && ctrl && !shift && !alt {
        // Before the turbo branches, which test only shift and would otherwise swallow it.
        ext_unit("speak_ambient_temperature", json!([]), || {
            crate::browsers::speak_ambient_temperature()
        });
    } else if name == "t" && alt && !(ctrl || shift) {
        ext_unit("speak_induction_report", json!([]), || {
            crate::browsers::speak_induction_report()
        });
    } else if name == "t" && shift {
        let (val, unit) = fmt_turbo(s.turbo_max);
        let extended = state().tel.protocol_mode == "extended";
        say_p(&if extended {
            format!("Max boost {val} {unit}")
        } else {
            "Unavailable".to_string()
        });
    } else if name == "t" {
        say_p(&format!("Boost {} {}", s.turbo_val, s.turbo_unit));
    } else if name == "p" && shift {
        say_x("Requesting powertrain dump");
        start_dump_thread(DumpKind::Powertrain);
    } else if name == "p" && !shift {
        let (extended, air) = {
            let st = state();
            (st.tel.protocol_mode == "extended", has_air_system(&st))
        };
        if extended {
            if air || s.air_pressure > 0.0 {
                let (val, unit) = fmt_pressure(s.air_pressure);
                if s.air_pressure_max > 1.0 {
                    let (max_val, _) = fmt_pressure(s.air_pressure_max);
                    say_p(&format!("Air pressure {val} of {max_val} {unit}"));
                } else {
                    say_p(&format!("Air pressure {val} {unit}"));
                }
            } else {
                say_p("Pneumatic system not available");
            }
        } else {
            say_p("Pneumatic data unavailable");
        }
    } else if name == "u" {
        flip_units();
        say_x(&units_mode());
    } else if name == "e" && shift {
        say_x("Requesting electrics dump");
        start_dump_thread(DumpKind::Electrics);
    } else if (name == "e" || name == "engtemp") && !shift {
        if s.engine_temp {
            say_p(&format!(
                "Engine temperature {} {}",
                s.etemp_val, s.etemp_unit
            ));
        } else {
            say_p("No engine temperature");
        }
    } else if name == "m" && !(ctrl || shift || alt) {
        say_x("Checking damage");
        start_dump_thread(DumpKind::Damage);
    } else if name == "o" && !ctrl {
        say_p(&format!("Oil temperature {} {}", s.otemp_val, s.otemp_unit));
    } else if name == "c" && ctrl && shift && alt {
        ext_unit("open_clickspot_browser", json!([]), || {
            crate::browsers::open_clickspot_browser()
        });
    } else if name == "c" && ctrl && shift && !alt {
        ext_ac("toggle_clickspot_mode", ac, |a| {
            crate::listeners::clickspot::toggle_clickspot_mode(a)
        });
    } else if name == "c" && alt && !(ctrl || shift) {
        let (mx, my) = {
            let st = state();
            (st.tel.marked_coord_x, st.tel.marked_coord_y)
        };
        match mx {
            None => say_x("No waypoint marked"),
            Some(mx) => {
                let my = my.unwrap_or_else(|| {
                    panic!("TypeError: unsupported format string passed to NoneType.__format__")
                });
                say_x(&format!("Waypoint X {}, Y {}", py_f(mx, 1), py_f(my, 1)));
            }
        }
    } else if name == "c" && shift && !(ctrl || alt) {
        {
            let mut st = state();
            st.tel.marked_coord_x = Some(s.x);
            st.tel.marked_coord_y = Some(s.y);
            st.tel.last_coord_bearing_ts = 0.0; // force immediate recalculation
        }
        say_x(&format!(
            "Waypoint marked at X {}, Y {}",
            py_f(s.x, 1),
            py_f(s.y, 1)
        ));
    } else if name == "c" && !ctrl && !shift && !alt {
        say_p(&format!(
            "Coordinates X {}, Y {}, Z {}",
            py_f(s.x, 2),
            py_f(s.y, 2),
            py_f(s.z, 2)
        ));
    } else if name == "s" && ctrl {
        ext_unit(
            "toggle_status_mode",
            json!([]),
            crate::menus::toggle_status_mode,
        );
    } else if name == "b" && ctrl {
        ext_unit(
            "toggle_buffer_mode",
            json!([]),
            crate::virtual_browser::toggle_buffer_mode,
        );
    } else if name == "b" && !ctrl && !shift && !alt {
        ext_unit(
            "open_vehicle_bindings_browser",
            json!([]),
            crate::browsers::open_vehicle_bindings_browser,
        );
    } else if name == "b" && shift && !ctrl && !alt {
        // Shift+B sits next to plain B on purpose: the browser LISTS this vehicle's special
        // controls, and this names whatever control you press on any device.
        ext_unit(
            "toggle_binding_learn_mode",
            json!([]),
            crate::listeners::binding_learn::toggle_binding_learn_mode,
        );
    } else if name == "n" && !ctrl && !shift && !alt {
        // The environment values the pause screen has no control for at all. N rather than
        // E: plain E is "Speak engine temperature" earlier in this chain, so an E binding
        // here would be unreachable.
        ext_unit(
            "open_environment_browser",
            json!([]),
            crate::browsers::open_environment_browser,
        );
    } else if name == "j" && !ctrl && !shift && !alt {
        // The World Editor's Window > Audio tools, which are ImGui and so unreadable.
        ext_unit(
            "open_audio_debug_browser",
            json!([]),
            crate::browsers::open_audio_debug_browser,
        );
    } else if name == "c" && ctrl && !(shift || alt) {
        let on = {
            let mut st = state();
            st.tel.pedal_tones_active = !st.tel.pedal_tones_active;
            st.tel.pedal_tones_active
        };
        say_x(if on {
            "Pedal tones on"
        } else {
            "Pedal tones off"
        });
    } else if name == "v" && shift && !ctrl {
        // One key, two alignments, disambiguated by the docking instrument: coupling to a
        // trailer and driving up its ramp are the same act from the seat. Deliberately NOT
        // falling back to the coupler when no ramp is found: with the instrument on you asked
        // for a ramp, and a named reason tells you whether to keep looking.
        let (docking, scanning) = {
            let st = state();
            (st.dock.dock_mode_active, st.scanner.scan_mode_active)
        };
        if docking {
            say_x("Aligning to ramp");
            x_send_implement_cmd("RAMPALIGN");
        } else if !scanning {
            say_x("Vehicle scanner is not active");
        } else {
            say_x("Aligning to trailer");
            if let Err(e) = scanner_sendto(b"ALIGN") {
                log::error!("Failed to send ALIGN command via UDP: {e}");
            }
        }
    } else if name == "v" && ctrl {
        ext_ac("toggle_scan_mode", ac, |a| {
            crate::listeners::scanner::toggle_scan_mode(a)
        });
    } else if name == "o" && ctrl {
        ext_ac("toggle_obstacle_mode", ac, |a| {
            crate::listeners::obstacle::toggle_obstacle_mode(a)
        });
    } else if name == "n" && ctrl && !(shift || alt) {
        ext_ac("toggle_nodegrab_mode", ac, |a| {
            crate::listeners::nodegrab::toggle_nodegrab_mode(a)
        });
    } else if name == "tab" {
        let scanning = state().scanner.scan_mode_active;
        if !scanning {
            say_x("Turn on vehicle scanner first");
        } else {
            let direction: &str = if ctrl {
                "CLOSEST"
            } else if shift {
                "PREV"
            } else {
                "NEXT"
            };
            if let Err(e) = scanner_sendto(direction.as_bytes()) {
                // The Python formats the bytes object: b'NEXT'.
                log::error!("Failed to send b'{direction}' command: {e}");
            }
        }
    } else if name == "a" {
        say_p(&format!(
            "Roll {} degrees, pitch {} degrees",
            py_f(s.roll_deg, 1),
            py_f(s.pitch_deg, 1)
        ));
    } else if name == "space" && !(ctrl || shift || alt) {
        space_command();
    }

    clear_next_key_hook(false);
}

/// F9 I: the cane tap. One deliberate press, one complete picture.
///
/// Deliberately does NOT short-circuit on `_implement_word_current`: that is this side's
/// guess at what the mod knows, and gating on it masked the mod's own, far more specific
/// answer. Ask, and report what comes back.
fn cane_tap() {
    let (in_cannon, cannon_kind, cannon_aim, rpm_now, gear_now, deck, dock_snap, dock_why) = {
        let st = state();
        (
            st.dock.cannon_active,
            st.dock.cannon_kind.clone(),
            st.dock.last_cannon_aim.clone(),
            st.tel.last_rpm,
            st.tel.last_gear_str.clone(),
            st.dock.last_ramp_self.clone(),
            st.dock.last_dock.clone(),
            st.dock.last_dock_fail.clone(),
        )
    };
    if cannon_kind == "OLD" {
        old_cannon_readout(cannon_aim);
    } else if in_cannon {
        // In the cannon the alignment task is over and the aiming task has begun. Both
        // numbers arrive on ordinary telemetry from large_cannon's controller: rpm is
        // inclination * 1000 and the gear string is the shoot strength as a percentage.
        let rpm = if rpm_now != 0.0 { rpm_now } else { 0.0 };
        let incl = py_max(0.0, py_min(1.0, rpm / 1000.0));
        let mut strength = py_strip(gear_now.as_deref().unwrap_or("")).to_string();
        if !strength.ends_with('%') {
            strength = "unknown".to_string();
        }
        say_x(&format!(
            "Inclination {} percent, strength {strength}",
            py_f(incl * 100.0, 0)
        ));
    } else if state().dock.dock_mode_active && dock_snap.is_some() {
        if let Some(snap) = &dock_snap {
            say_x(&x_dock_phrase(snap));
        }
    } else if let Some(deck) = &deck {
        // Sitting in a ramp machine with nothing to line up with: answer what your own deck
        // is doing. Ordered strictly BELOW a live docking reading, so an alignment readout
        // the driver switched on is never displaced by a fact about their own bed.
        say_x(&x_ramp_self_phrase(deck));
    } else if !state().dock.dock_mode_active {
        say_x("Docking instrument is off");
    } else if let Some(why) = dock_why.filter(|w| !w.is_empty()) {
        // The instrument shares its soundscape with the scanner, so a dead instrument sounds
        // exactly like a working one; the mod's reason is the only way to tell them apart.
        say_x(&format!("No reading. {why}"));
    } else {
        say_x("Nothing in range");
    }
}

fn old_cannon_readout(cannon_aim: Option<CannonAim>) {
    let stale = |a: &CannonAim| monotonic() - a.stamp > CANNON_READOUT_STALE_S;
    let aim = match cannon_aim {
        Some(a) if !stale(&a) => a,
        _ => {
            say_x("Old Cannon elevation unavailable");
            return;
        }
    };
    let elevation = aim.elevation;
    let mut bits = vec![format!("Elevation {} degrees", py_f(elevation, 1))];
    if aim.range < 0.0 {
        bits.push("select a scanner target".to_string());
    } else {
        let bearing = aim.bearing;
        if bearing.abs() <= 0.5 {
            bits.push("bearing aligned".to_string());
        } else {
            bits.push(format!(
                "{} degrees {}",
                py_f(bearing.abs(), 1),
                if bearing > 0.0 { "left" } else { "right" }
            ));
        }
        let (rv, ru) = fmt_distance(aim.range);
        if !aim.solution_available {
            bits.push(format!("line of sight {} degrees", py_f(aim.line_angle, 1)));
            bits.push("ballistic solution unavailable".to_string());
        } else if !aim.reachable {
            bits.push("target out of ballistic range".to_string());
        } else {
            let target_angle = aim.target_angle;
            let error = target_angle - elevation;
            if error.abs() <= 0.5 {
                bits.push(format!(
                    "elevation aligned at {} degrees",
                    py_f(target_angle, 1)
                ));
            } else {
                bits.push(format!(
                    "{} {} degrees to {}",
                    if error > 0.0 { "raise" } else { "lower" },
                    py_f(error.abs(), 1),
                    py_f(target_angle, 1)
                ));
            }
        }
        bits.push(format!("{rv} {ru}"));
    }
    say_x(&bits.join(". "));
}

/// F9 Shift+I: retarget the reference band. The mod auto-selects on lock; this overrides it
/// when you want a different part of the same object.
fn cycle_reference_band() {
    let (snap_mode, word) = {
        let st = state();
        (
            st.dock.last_dock.as_ref().map(|d| d.mode.clone()),
            st.dock.implement_word_current.clone(),
        )
    };
    if snap_mode.as_deref() == Some("RAMP") {
        // A ramp has one reference, its mouth, so there is nothing to cycle.
        say_x("No reference bands in ramp alignment");
    } else if word.is_empty() {
        say_x("No implement fitted");
    } else {
        x_send_implement_cmd("BANDNEXT");
        // The mod recomputes on its next 10 Hz tick, so read back rather than predicting.
        x_sleep(0.15);
        let snap = state().dock.last_dock.clone();
        match snap {
            None => say_x("Nothing in range"),
            Some(snap) => {
                let band = x_band_name(&snap.kind, snap.idx, snap.count);
                let vert = snap.vertical;
                let place = if vert.abs() < IMPL_DOCK_LEVEL_M {
                    "level".to_string()
                } else {
                    let (vv, vu) = fmt_distance(vert.abs());
                    format!("{} {vv} {vu}", if vert > 0.0 { "raise" } else { "lower" })
                };
                say_x(&format!("{band} {} of {}, {place}", snap.idx, snap.count));
            }
        }
    }
}

/// F9 Ctrl+I: the docking instrument on or off.
fn toggle_docking_instrument(ac: &Option<Arc<AudioController>>) {
    let active = {
        let mut st = state();
        st.dock.dock_mode_active = !st.dock.dock_mode_active;
        st.dock.dock_mode_active
    };
    // Re-announce on the way in, so toggling is a genuine recovery path if the mod and this
    // side have drifted apart about what is fitted.
    if active {
        x_send_implement_cmd("ON");
    }
    x_send_implement_cmd(if active { "DOCK_ON" } else { "DOCK_OFF" });
    x_set_dock_mode(ac, active);
    if !active {
        {
            let mut st = state();
            st.dock.last_dock = None;
            st.dock.last_dock_fail = None;
            st.dock.last_dock_mode = None;
            st.dock.last_dock_name = None;
        }
        say_x("Docking instrument off");
    } else {
        // The re-announce travels to the mod and back at 10 Hz, so read the answer rather
        // than the stale value.
        x_sleep(0.3);
        // Auto-select is per target, so a fresh start should not inherit an old band pick.
        x_send_implement_cmd("BANDAUTO");
        let (why, mode, tgt) = {
            let mut st = state();
            let why = st.dock.last_dock_fail.clone();
            let mode = st.dock.last_dock.as_ref().map(|d| d.mode.clone());
            let tgt = st.dock.last_dock.as_ref().map(|d| d.name.clone());
            // Claim both latches, so the listener does not also announce the mode and the
            // machine named here.
            st.dock.last_dock_mode = mode.clone();
            st.dock.last_dock_name = tgt.clone();
            (why, mode, tgt)
        };
        // Name the mode on the way in, and in ramp mode the machine with it: the instrument
        // auto-selects the mode, so otherwise the tones' meaning must be inferred.
        let tgt = tgt.filter(|t| !t.is_empty());
        let which = match mode.as_deref() {
            Some("RAMP") => Some(match &tgt {
                Some(t) => format!("ramp alignment, {t}"),
                None => "ramp alignment".to_string(),
            }),
            Some("IMPL") => Some("implement alignment".to_string()),
            _ => None,
        };
        let head = match which {
            Some(w) => format!("Docking instrument on, {w}"),
            None => "Docking instrument on".to_string(),
        };
        // Report the MOD's reason, not a guess from this side's own state.
        match why.filter(|w| !w.is_empty()) {
            Some(w) => say_x(&format!("{head}. {w}")),
            None => say_x(&head),
        }
    }
}

/// F9 Space: one key, four answers. On the stock vehicle selector it reads the details
/// page's specifications; on a mod details page that mod's sheet; driving, the terrain
/// scan; in a menu it activates the on-screen control.
///
/// The selector is asked FIRST: it is a positive fact about an open screen, where
/// `_world_is_active()` is a recency heuristic that also goes false while paused. A refusal
/// or a silent mod falls straight through, so nothing that worked before can regress. The
/// scan says nothing on success on purpose: its reference ping is the acknowledgement.
fn space_command() {
    let (info_lines, info_fail) = x_request_vehicle_info();
    let info_code = info_fail.as_ref().map(|f| f.0.clone());
    if !info_lines.is_empty() {
        let title = format!("Vehicle information, {} items", info_lines.len());
        x_open_virtual_browser(info_lines, title);
    } else if !matches!(
        info_code.as_deref(),
        None | Some("notselector") | Some("timeout")
    ) {
        // Spoken only when the mod was ON the selector and still could not answer.
        // "notselector" is every other screen's answer, and a timeout says nothing about the
        // vehicle at all.
        let sentence = info_fail.map(|f| f.1).unwrap_or_default();
        say_x(if sentence.is_empty() {
            "No vehicle information"
        } else {
            &sentence
        });
    } else if x_bool(
        "_read_mod_details_page",
        crate::browsers::read_mod_details_page,
    ) {
    } else if x_bool("_world_is_active", crate::state::world_is_active) {
        ext_unit("trigger_terrain_scan_driving_only", json!([]), || {
            crate::listeners::terrain_scan::trigger_terrain_scan_driving_only()
        });
    } else {
        x_broadcast(json!({"type": "context_action", "action": "activate"}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_table_starts_and_ends_as_the_python() {
        assert_eq!(
            F9_HELP.get(&("s", false, false, false)).copied(),
            Some("Speak speed and map speed limit")
        );
        assert_eq!(
            F9_HELP.last().map(|(k, v)| (*k, *v)),
            Some((("b", false, true, false), "Toggle learn bindings mode"))
        );
    }

    #[test]
    fn python_min_max_keep_the_first_on_nan() {
        assert_eq!(py_min(1.0, f64::NAN), 1.0);
        assert_eq!(py_max(0.0, 1.0), 1.0);
        assert!(py_max(f64::NAN, 0.0).is_nan());
    }
}
