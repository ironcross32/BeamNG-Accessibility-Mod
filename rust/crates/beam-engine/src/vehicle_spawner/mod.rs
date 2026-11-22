//! Accessible Vehicle Spawner (`vehicle_spawner.py`): a multi-screen virtual-browser
//! modal opened with F11 and driven by suppressing key hooks. Talks to
//! `vehicleSpawnerAccessible.lua` over UDP (data on 4460, commands on 4461) to fetch the
//! vehicle catalog and the active vehicle list, and to issue spawn batches, teleports and
//! arrangements.
//!
//! Keys while the modal is open (all suppressed): Tab / Shift+Tab cycle the pages (main ->
//! to-be-spawned -> manage), arrows navigate, Home/End and PageUp/PageDown jump, Left/Right
//! drill in and out of configurations, Enter confirms, Escape backs out or closes, Space
//! spawns the queue (or toggles a filter box), Delete removes, F/C filter, G arrangements,
//! W the placement editor, R reload/random, V ignition off, Ctrl+A select all,
//! Shift+Enter queue as a replacement, X replace/add toggle (and force mode in the editor),
//! I vehicle information, 1-4 the editor's translation step.
//!
//! The placement editor (`place3d`) edits a queued vehicle's offset and rotation relative
//! to its anchor, or, opened from the manage page, drives a teleport of an existing
//! vehicle (standard: set down exactly; force: launched on a ballistic arc).
//!
//! Locking: every module global lives in one [`Vs`] behind `VS`. Handlers compute under
//! the lock and queue their side effects (speech, UDP sends, pings) in `Vs::fx`, which are
//! run in order after the lock is released, so no injected callback ever runs while the
//! spawner's state is held (`close_modal` is called from other modules' threads).

mod arrange;
mod place3d;
mod pyjson;
mod screens;

use std::collections::BTreeSet;
use std::net::UdpSocket;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use beam_base::pyfmt::{py_f, py_float_parse, py_strip};
use beam_base::pyval::{PyDict, PyInt, PyValue, json_loads, py_int_parse};
use crossbeam_channel::{Receiver, Sender, unbounded};
use indexmap::IndexMap;
use parking_lot::{Mutex, RwLock};

use crate::keyboard::{self, KeyEvent, KeyEventType};
use crate::net::{Decode, Recv, UdpListener, decode, set_rcvbuf_before_bind};
use crate::state::{HookRecord, SlotInfo};
use crate::timer::{Event, STOP};

pub use arrange::{ARRANGE_TYPES, DigitField, compute_arrangement_offsets};
pub use place3d::{
    PLACE_AXIS_FIELDS, PLACE_CROSS_RUNGS, PLACE_DEFAULTS, PLACE_HOLD_DELAY_SEC,
    PLACE_IDLE_READOUT_SEC, PLACE_KEY_AXES, PLACE_PITCH_DOWN, PLACE_PITCH_UP, PLACE_REPEAT_MIN_SEC,
    PLACE_REPEAT_RAMP_SEC, PLACE_REPEAT_START_SEC, PLACE_ROT_STEP_COARSE_DEG, PLACE_ROT_STEP_DEG,
    PLACE_ROT_STEP_FINE_DEG, PLACE_ROT_STEP_RUNGS, PLACE_ROT_WORDS, PLACE_TICK_SEC,
    PLACE_TRANSLATE_AXES, PLACE_UNIT_STEPS, PlaceVals, place_cross_rung, place_repeat_interval,
};
pub use screens::FILTER_CATEGORIES;

/// Receive from Lua.
pub const DATA_PORT: u16 = 4460;
/// Send to Lua.
pub const CMD_PORT: u16 = 4461;

// =============================================================================
//  Public API (signatures fixed for the other workstreams)
// =============================================================================

/// `say_fn(text, interrupt=True, exclude_from_buffer=False, source="python")`.
pub type SayFn = Arc<dyn Fn(&str, bool, bool, &str) + Send + Sync>;

/// `request_info_fn(model, config, timeout) -> (lines, failure)`.
pub type RequestInfoFn = Arc<
    dyn Fn(Option<&str>, Option<&str>, f64) -> (Vec<String>, Option<(String, String)>)
        + Send
        + Sync,
>;

/// The arguments of `vehicle_spawner.init(...)`.
#[derive(Clone)]
pub struct InitArgs {
    pub say_fn: SayFn,
    pub is_focused_fn: Arc<dyn Fn() -> bool + Send + Sync>,
    pub get_slots_fn: Arc<dyn Fn() -> IndexMap<i64, SlotInfo> + Send + Sync>,
    pub close_others_fn: Arc<dyn Fn() + Send + Sync>,
    /// `audio_controller.trigger_placement_ping(rung, azimuth_deg, pitch_mult)`.
    pub ping_fn: Arc<dyn Fn(i64, f64, f64) + Send + Sync>,
    pub request_info_fn: RequestInfoFn,
}

/// `beamtel.request_vehicle_info`'s default timeout, which the Python gets by calling
/// `_request_info_fn(model, config)` with two arguments.
const REQUEST_INFO_TIMEOUT: f64 = 0.6;

static DEPS: RwLock<Option<InitArgs>> = RwLock::new(None);

/// `init(say_fn, is_focused_fn, logger, get_slots_fn, close_others_fn, ping_fn, request_info_fn)`.
pub fn init(args: InitArgs) {
    *DEPS.write() = Some(args);
}

/// `start(stop_event)`: listener + worker threads, the F11 hotkey, the catalog request.
/// The stop event is the engine's `timer::STOP`.
pub fn start() {
    // Supervised (python-bugs.md 63): a CATALOG_ITEM that parses but is not an object
    // ended the Python listener, and F11 had no catalog updates until restart.
    crate::timer::spawn_supervised("vehicle-spawner-listener", &STOP, || listener_loop(&STOP));
    crate::timer::spawn_supervised("vehicle-spawner-worker", &STOP, worker_loop);
    install_f11_hotkey();
    // Kick off an initial catalog request (Lua may have already pre-built it).
    send_cmd("REQUEST_CATALOG");
}

/// `is_modal_open()`.
pub fn is_modal_open() -> bool {
    VS.lock().modal_open
}

/// `close_modal()`.
pub fn close_modal() {
    with_vs(|vs| vs.close_modal(false));
}

// =============================================================================
//  Module-level state
// =============================================================================

/// The spawner's screens (`_screen`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Main,
    Configs,
    ToSpawn,
    Manage,
    Filter,
    Place3d,
    Ref,
    MarkPicker,
    ReplaceSlot,
    Arrange,
    SpacingEdit,
    Info,
}

impl Screen {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Screen::Main => "main",
            Screen::Configs => "configs",
            Screen::ToSpawn => "to_spawn",
            Screen::Manage => "manage",
            Screen::Filter => "filter",
            Screen::Place3d => "place3d",
            Screen::Ref => "ref",
            Screen::MarkPicker => "mark_picker",
            Screen::ReplaceSlot => "replace_slot",
            Screen::Arrange => "arrange",
            Screen::SpacingEdit => "spacing_edit",
            Screen::Info => "info",
        }
    }
}

/// What the placement editor is driving (`_place_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaceMode {
    Spawn,
    Teleport,
}

/// How a teleport gets there (`_place_launch_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchMode {
    Standard,
    Force,
}

impl LaunchMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            LaunchMode::Standard => "standard",
            LaunchMode::Force => "force",
        }
    }
}

/// One `_to_spawn` item. Keys the Python sets to None and keys it never sets read the
/// same through `.get`, so both are `None` here.
#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub model: PyValue,
    pub config: PyValue,
    /// `str()` of the Python value (it is only ever formatted).
    pub display_name: String,
    pub ref_mode: String,
    pub ref_veh_id: Option<i64>,
    pub replace_slot: Option<i64>,
    pub replace_veh_id: Option<i64>,
    pub replace_veh_name: Option<String>,
    pub vals: PlaceVals,
}

/// `_place_held[name]`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Held {
    pub t: f64,
    pub next: f64,
    pub rot_step: i64,
}

/// A side effect queued under the lock and run after it is released.
pub(crate) enum Fx {
    Say(String),
    Send(String),
    Ping(i64, f64, f64),
}

pub(crate) type Filters = IndexMap<String, BTreeSet<String>>;

pub(crate) struct Vs {
    pub fx: Vec<Fx>,
    /// `_get_slots()` as fetched at the start of the running handler.
    pub slots: IndexMap<i64, SlotInfo>,

    pub catalog: Vec<Arc<PyDict>>,
    pub catalog_building: Vec<Arc<PyDict>>,
    pub catalog_expected: PyInt,
    pub active_vehicles: Vec<(i64, String)>,

    pub modal_open: bool,
    pub hook_handles: Vec<HookRecord>,

    pub filters: Filters,
    pub filter_draft: Option<Filters>,

    pub screen: Screen,
    pub idx_main: usize,
    pub idx_configs: usize,
    pub idx_to_spawn: usize,
    pub idx_filter: usize,
    pub idx_ref: usize,
    pub idx_mark: usize,
    pub idx_manage: usize,

    pub manage_vehicles: Vec<(i64, String)>,
    pub manage_selected: BTreeSet<i64>,

    pub drill_vehicle: Option<Arc<PyDict>>,

    pub info_lines: Vec<String>,
    pub idx_info: usize,
    pub info_return_screen: Screen,

    pub to_spawn: Vec<Item>,

    pub wizard_target_idx: Option<usize>,
    pub wizard_ref_mode: Option<String>,
    pub wizard_ref_veh_id: Option<i64>,
    pub wizard_ref_name: Option<String>,

    pub place_mode: PlaceMode,
    pub tp_veh_id: Option<i64>,
    pub tp_veh_name: Option<String>,
    pub place_launch_mode: LaunchMode,

    pub place_values: PlaceVals,
    pub place_snapshot: PlaceVals,
    pub place_held: IndexMap<String, Held>,
    pub place_unit_step: i64,
    pub place_last_step: f64,
    pub place_readout_pending: bool,

    pub mark_target_idx: Option<usize>,

    pub idx_replace_slot: usize,
    pub pending_replace_item: Option<Item>,
    pub replace_editing_idx: Option<usize>,

    pub pending_manage_announce: bool,

    pub arrange_type_idx: usize,
    pub arrange_variant_idx: usize,
    pub idx_arrange: usize,
    pub arrange_return_screen: Screen,
    pub player_veh_id: Option<PyInt>,

    pub spacing_field: DigitField,
    pub pending_prefix: Option<String>,
}

impl Default for Vs {
    fn default() -> Self {
        Vs {
            fx: Vec::new(),
            slots: IndexMap::new(),
            catalog: Vec::new(),
            catalog_building: Vec::new(),
            catalog_expected: PyInt::Small(0),
            active_vehicles: Vec::new(),
            modal_open: false,
            hook_handles: Vec::new(),
            filters: IndexMap::new(),
            filter_draft: None,
            screen: Screen::Main,
            idx_main: 0,
            idx_configs: 0,
            idx_to_spawn: 0,
            idx_filter: 0,
            idx_ref: 0,
            idx_mark: 0,
            idx_manage: 0,
            manage_vehicles: Vec::new(),
            manage_selected: BTreeSet::new(),
            drill_vehicle: None,
            info_lines: Vec::new(),
            idx_info: 0,
            info_return_screen: Screen::Main,
            to_spawn: Vec::new(),
            wizard_target_idx: None,
            wizard_ref_mode: None,
            wizard_ref_veh_id: None,
            wizard_ref_name: None,
            place_mode: PlaceMode::Spawn,
            tp_veh_id: None,
            tp_veh_name: None,
            place_launch_mode: LaunchMode::Standard,
            place_values: PLACE_DEFAULTS,
            place_snapshot: PLACE_DEFAULTS,
            place_held: IndexMap::new(),
            place_unit_step: 1,
            place_last_step: 0.0,
            place_readout_pending: false,
            mark_target_idx: None,
            idx_replace_slot: 0,
            pending_replace_item: None,
            replace_editing_idx: None,
            pending_manage_announce: false,
            arrange_type_idx: 0,
            arrange_variant_idx: 0,
            idx_arrange: 0,
            arrange_return_screen: Screen::Main,
            player_veh_id: None,
            // The arrangement spacing persists across openings.
            spacing_field: DigitField::new(15),
            pending_prefix: None,
        }
    }
}

static VS: LazyLock<Mutex<Vs>> = LazyLock::new(|| Mutex::new(Vs::default()));

/// `_catalog_ready` and `_active_vehicles_event`: set by the listener, cleared by the
/// requesters, never waited on.
static CATALOG_READY: Event = Event::new();
static ACTIVE_VEHICLES_EVENT: Event = Event::new();
/// `_place_active`: set while the editor screen is up; the ticker parks on it.
pub(crate) static PLACE_ACTIVE: Event = Event::new();

/// Run `f` on the state, then the side effects it queued, in order, with the lock
/// released. A panic still runs the effects queued before it (the Python had already
/// spoken them) and then continues unwinding.
pub(crate) fn with_vs<R>(f: impl FnOnce(&mut Vs) -> R) -> R {
    let mut g = VS.lock();
    let r = catch_unwind(AssertUnwindSafe(|| f(&mut g)));
    let fx = std::mem::take(&mut g.fx);
    drop(g);
    flush(fx);
    match r {
        Ok(v) => v,
        Err(p) => resume_unwind(p),
    }
}

fn flush(fx: Vec<Fx>) {
    for f in fx {
        match f {
            Fx::Say(t) => say_now(&t),
            Fx::Send(c) => send_cmd(&c),
            Fx::Ping(rung, az, pitch) => ping_now(rung, az, pitch),
        }
    }
}

impl Vs {
    /// `_say_safe(text)`.
    pub(crate) fn say(&mut self, text: impl Into<String>) {
        self.fx.push(Fx::Say(text.into()));
    }

    /// `_send_cmd(cmd)`.
    pub(crate) fn send(&mut self, cmd: impl Into<String>) {
        self.fx.push(Fx::Send(cmd.into()));
    }
}

// =============================================================================
//  Utilities
// =============================================================================

fn deps() -> Option<InitArgs> {
    DEPS.read().clone()
}

fn panic_msg(p: &(dyn std::any::Any + Send)) -> String {
    keyboard::panic_text(p)
}

/// `_say_safe`: `_say(text, interrupt=True, exclude_from_buffer=True)`, errors swallowed.
fn say_now(text: &str) {
    if let Some(d) = deps() {
        let _ = catch_unwind(AssertUnwindSafe(|| (d.say_fn)(text, true, true, "python")));
    }
}

/// `_place_ping`.
fn ping_now(rung: i64, azimuth_deg: f64, pitch_mult: f64) {
    if let Some(d) = deps() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            (d.ping_fn)(rung, azimuth_deg, pitch_mult)
        }));
    }
}

/// `_get_slots()`.
fn get_slots() -> IndexMap<i64, SlotInfo> {
    if let Some(d) = deps()
        && let Ok(s) = catch_unwind(AssertUnwindSafe(|| (d.get_slots_fn)()))
    {
        return s;
    }
    IndexMap::new()
}

/// `_logi`: silent until `init` has supplied the logger.
pub(crate) fn logi(msg: &str) {
    if DEPS.read().is_some() {
        log::info!("{msg}");
    }
}

/// `_logw`.
pub(crate) fn logw(msg: &str) {
    if DEPS.read().is_some() {
        log::warn!("{msg}");
    }
}

/// Persistent UDP send socket, created once and reused, so the worker does not build a
/// socket per send.
static CMD_SOCK: Mutex<Option<UdpSocket>> = Mutex::new(None);

/// `_send_cmd(cmd)`.
fn send_cmd(cmd: &str) {
    if let Some(h) = test_hooks().send {
        h(cmd);
        return;
    }
    let mut g = CMD_SOCK.lock();
    let r = (|| -> std::io::Result<()> {
        let sock = match g.take() {
            Some(s) => s,
            None => UdpSocket::bind("0.0.0.0:0")?,
        };
        let r = sock.send_to(cmd.as_bytes(), ("127.0.0.1", CMD_PORT));
        *g = Some(sock);
        r.map(|_| ())
    })();
    if let Err(e) = r {
        logw(&format!("vehicle_spawner: failed to send command: {e}"));
        *g = None;
    }
}

// ---- Python value helpers ----------------------------------------------------------------

static NONE: PyValue = PyValue::None;

fn type_name(v: &PyValue) -> &'static str {
    match v {
        PyValue::None => "NoneType",
        PyValue::Bool(_) => "bool",
        PyValue::Int(_) => "int",
        PyValue::Float(_) => "float",
        PyValue::Str(_) => "str",
        PyValue::List(_) => "list",
        PyValue::Dict(_) => "dict",
    }
}

/// `d.get(key)` on a dict.
pub(crate) fn dget<'a>(d: &'a PyDict, key: &str) -> &'a PyValue {
    d.get(key).unwrap_or(&NONE)
}

/// `v.get(key)` where `v` came off the wire: AttributeError unless it is a dict.
pub(crate) fn vget<'a>(v: &'a PyValue, key: &str) -> &'a PyValue {
    match v {
        PyValue::Dict(d) => dget(d, key),
        other => panic!("'{}' object has no attribute 'get'", type_name(other)),
    }
}

/// `a or b or ...`: the first truthy value, else the last.
pub(crate) fn py_or<'a>(vals: &[&'a PyValue]) -> &'a PyValue {
    for v in vals {
        if v.truthy() {
            return v;
        }
    }
    vals.last().copied().unwrap_or(&NONE)
}

/// `s.lower()`: AttributeError unless it is a str.
pub(crate) fn py_lower(v: &PyValue) -> String {
    match v {
        PyValue::Str(s) => s.to_lowercase(),
        other => panic!("'{}' object has no attribute 'lower'", type_name(other)),
    }
}

/// `len(v)`.
pub(crate) fn py_len(v: &PyValue) -> usize {
    match v {
        PyValue::List(l) => l.len(),
        PyValue::Dict(d) => d.len(),
        PyValue::Str(s) => s.chars().count(),
        other => panic!("object of type '{}' has no len()", type_name(other)),
    }
}

/// The elements `for x in v` visits (a dict yields its keys, a str its characters).
pub(crate) fn py_seq(v: &PyValue) -> Vec<PyValue> {
    match v {
        PyValue::List(l) => l.clone(),
        PyValue::Dict(d) => d.keys().map(|k| PyValue::Str(k.clone())).collect(),
        PyValue::Str(s) => s.chars().map(|c| PyValue::Str(c.to_string())).collect(),
        other => panic!("'{}' object is not iterable", type_name(other)),
    }
}

/// `seq[i]` for an index that the Python does not clamp first.
pub(crate) fn py_index<T>(seq: &[T], i: usize) -> &T {
    match seq.get(i) {
        Some(v) => v,
        None => panic!("list index out of range"),
    }
}

/// `"vehicle" if n == 1 else "vehicles"`.
pub(crate) fn vehicles_word(n: usize) -> &'static str {
    if n == 1 { "vehicle" } else { "vehicles" }
}

/// `int(text)`, or 0 where Python raises ValueError (`except ValueError: n = 0`).
fn int_or_zero(text: &str) -> PyInt {
    py_int_parse(text).unwrap_or(PyInt::Small(0))
}

fn pyint_word(n: &PyInt) -> &'static str {
    if n.cmp_i64(1) == std::cmp::Ordering::Equal {
        "vehicle"
    } else {
        "vehicles"
    }
}

// ---- Modifier polls, clock, randomness -----------------------------------------------------

/// `keyboard.is_pressed(name)` behind `_KEYBOARD_OK`.
pub(crate) fn key_pressed(name: &str) -> bool {
    if let Some(h) = test_hooks().pressed {
        return h(name);
    }
    if !keyboard::keyboard_ok() {
        return false;
    }
    catch_unwind(|| keyboard::is_pressed(name)).unwrap_or(false)
}

/// `_shift_pressed()`.
pub(crate) fn shift_pressed() -> bool {
    key_pressed("shift")
}

/// `_ctrl_pressed()`.
pub(crate) fn ctrl_pressed() -> bool {
    key_pressed("ctrl")
}

/// `time.monotonic()`.
pub(crate) fn monotonic_now() -> f64 {
    if let Some(h) = test_hooks().clock {
        return h();
    }
    crate::timer::monotonic()
}

static RNG: AtomicU64 = AtomicU64::new(0);

/// `random.choice(seq)`'s index for a sequence of length `n` (`n > 0`). Python raises
/// IndexError on an empty sequence; no caller passes one.
pub(crate) fn choice_index(n: usize) -> usize {
    if let Some(h) = test_hooks().choice {
        return h(n) % n.max(1);
    }
    // xorshift64*, seeded from the clock: the Python seeds its Mersenne Twister from the
    // OS, so no particular sequence is part of the behaviour.
    let mut x = RNG.load(Ordering::Relaxed);
    if x == 0 {
        x = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
    }
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    RNG.store(x, Ordering::Relaxed);
    (x.wrapping_mul(0x2545_F491_4F6C_DD1D) % n.max(1) as u64) as usize
}

// =============================================================================
//  UDP listener
// =============================================================================

fn listener_loop(stop: &Event) {
    let mut sock = match UdpListener::bind(DATA_PORT, 0.2, 65536) {
        Ok(s) => s,
        Err(_) => {
            // The Python's bind raises out of the thread; its `finally` logs this.
            logi("Vehicle spawner listener stopped.");
            return;
        }
    };
    set_rcvbuf_before_bind(sock.socket(), 1024 * 1024);
    logi(&format!(
        "Vehicle spawner listener started on port {DATA_PORT}"
    ));
    while !stop.is_set() {
        let data = match sock.recv() {
            Recv::Data(d, _) => d.to_vec(),
            Recv::Timeout => continue,
            Recv::Error(_) => {
                if stop.is_set() {
                    break;
                }
                continue;
            }
        };
        // An exception out of `_handle_packet` is not caught in the Python: it ends the
        // listener thread for the session.
        if catch_unwind(AssertUnwindSafe(|| handle_datagram(&data))).is_err() {
            break;
        }
    }
    logi("Vehicle spawner listener stopped.");
}

/// One datagram as the listener loop handles it: UTF-8 with replacement, stripped.
#[doc(hidden)]
pub fn handle_datagram(data: &[u8]) {
    if let Some(text) = decode(data, Decode::Utf8Replace) {
        handle_packet(py_strip(&text));
    }
}

/// `_handle_packet(text)`.
pub fn handle_packet(text: &str) {
    if text.is_empty() {
        return;
    }

    if let Some(rest) = text.strip_prefix("CATALOG_BEGIN:") {
        let n = int_or_zero(rest);
        let mut vs = VS.lock();
        vs.catalog_building = Vec::new();
        vs.catalog_expected = n;
        CATALOG_READY.clear();
        return;
    }

    if let Some(rest) = text.strip_prefix("CATALOG_ITEM:") {
        let obj = match json_loads(rest) {
            Ok(v) => v,
            Err(e) => {
                logw(&format!("vehicle_spawner: bad catalog item: {e}"));
                return;
            }
        };
        // Derive the synthetic "police" filter field: Lua sets hasPolice when any
        // config's "Config Type" is "Police", and populating the field only on police
        // vehicles makes the standard filter pipeline a Police-only filter.
        let mut d = match obj {
            PyValue::Dict(d) => d,
            other => panic!("'{}' object has no attribute 'get'", type_name(&other)),
        };
        if dget(&d, "hasPolice").truthy() {
            d.insert("police".to_string(), PyValue::from("Yes"));
        }
        VS.lock().catalog_building.push(Arc::new(d));
        return;
    }

    if text == "CATALOG_END" {
        let n = {
            let mut vs = VS.lock();
            // Vehicles (Type != "Prop") alphabetically first, then props, by the same
            // display name `_vehicle_summary` speaks, so ETK "800-Series" sorts under E.
            let building = std::mem::take(&mut vs.catalog_building);
            let keys: Vec<(u8, String)> = building.iter().map(|v| catalog_sort_key(v)).collect();
            let mut order: Vec<usize> = (0..building.len()).collect();
            order.sort_by(|&a, &b| keys[a].cmp(&keys[b]));
            vs.catalog = order
                .into_iter()
                .map(|i| Arc::clone(&building[i]))
                .collect();
            CATALOG_READY.set();
            vs.catalog.len()
        };
        logi(&format!("Vehicle spawner catalog received: {n} entries"));
        return;
    }

    if let Some(rest) = text.strip_prefix("CATALOG_ERR:") {
        logw(&format!("vehicle_spawner: catalog error: {rest}"));
        return;
    }

    if let Some(payload) = text.strip_prefix("ACTIVE_VEHICLES:") {
        let mut entries = Vec::new();
        if !payload.is_empty() {
            for part in payload.split(';') {
                if part.is_empty() {
                    continue;
                }
                if let Some((id, name)) = part.split_once(':')
                    && let Some(vid) = py_int_parse(id).and_then(|i| i.to_i64())
                {
                    entries.push((vid, name.to_string()));
                }
            }
        }
        let slots = get_slots();
        with_vs(|vs| {
            vs.slots = slots;
            vs.active_vehicles = entries.clone();
            // Refresh the manage snapshot too, dropping selections that are gone.
            vs.manage_vehicles = entries.clone();
            let present: BTreeSet<i64> = entries.iter().map(|(v, _)| *v).collect();
            vs.manage_selected.retain(|v| present.contains(v));
            if !vs.manage_vehicles.is_empty() {
                if vs.idx_manage >= vs.manage_vehicles.len() {
                    vs.idx_manage = vs.manage_vehicles.len() - 1;
                }
            } else {
                vs.idx_manage = 0;
            }
            ACTIVE_VEHICLES_EVENT.set();
            if vs.pending_manage_announce && vs.screen == Screen::Manage {
                vs.pending_manage_announce = false;
                vs.speak_current();
            }
        });
        return;
    }

    if let Some(body) = text.strip_prefix("RELOAD_DONE:") {
        let parts: Vec<&str> = body.split(',').collect();
        with_vs(|vs| {
            if parts.len() == 2 {
                vs.say(format!("Reloaded {}, failed {}.", parts[0], parts[1]));
            }
            vs.send("REQUEST_ACTIVE_VEHICLES");
        });
        return;
    }

    if let Some(body) = text.strip_prefix("REMOVE_DONE:") {
        let parts: Vec<&str> = body.split(',').collect();
        with_vs(|vs| {
            if parts.len() == 2 {
                vs.say(format!("Removed {}, failed {}.", parts[0], parts[1]));
            }
            vs.send("REQUEST_ACTIVE_VEHICLES");
        });
        return;
    }

    if let Some(rest) = text.strip_prefix("IGNITION_OFF_DONE:") {
        let n = int_or_zero(rest);
        say_now(&format!("Ignition off on {n} {}.", pyint_word(&n)));
        return;
    }

    if let Some(rest) = text.strip_prefix("SPAWN_OK:") {
        let n = int_or_zero(rest);
        say_now(&format!("Spawned {n} {}.", pyint_word(&n)));
        return;
    }

    if let Some(body) = text.strip_prefix("SPAWN_PARTIAL:") {
        // body: "<successes>,<fails>,<msg>"
        let parts: Vec<&str> = body.splitn(3, ',').collect();
        if parts.len() >= 2 {
            say_now(&format!("Spawned {}, failed {}.", parts[0], parts[1]));
        } else {
            say_now("Partial spawn.");
        }
        return;
    }

    if let Some(rest) = text.strip_prefix("SPAWN_ERR:") {
        say_now(&format!("Spawn error: {rest}"));
        return;
    }

    if text == "TELEPORT_OK" {
        say_now("Moved.");
        return;
    }

    if let Some(rest) = text.strip_prefix("TELEPORT_LAUNCHED:") {
        let parts: Vec<&str> = rest.split(',').collect();
        let mph = parts.first().and_then(|p| py_float_parse(p));
        let secs = parts.get(1).and_then(|p| py_float_parse(p));
        let (Some(mph), Some(secs)) = (mph, secs) else {
            say_now("Launched.");
            return;
        };
        let short = parts.len() > 2 && parts[2] == "1";
        let msg = format!(
            "Launched at {} miles per hour, landing in {} seconds.",
            py_f(mph, 0),
            py_f(secs, 1)
        );
        say_now(&if short {
            format!("Launching short. {msg}")
        } else {
            msg
        });
        return;
    }

    if let Some(rest) = text.strip_prefix("TELEPORT_LANDED:") {
        let parts: Vec<&str> = rest.split(',').collect();
        if parts.len() < 3 {
            return;
        }
        let mut vals = [0.0f64; 3];
        for (i, p) in parts[..3].iter().enumerate() {
            match py_float_parse(p) {
                Some(v) => vals[i] = v,
                None => return,
            }
        }
        let [miss_m, along_m, cross_m] = vals;
        const M_TO_FT: f64 = 3.28084;
        let miss_ft = miss_m * M_TO_FT;
        // A launch is only ever approximately on target, so a couple of feet is a hit.
        if miss_ft < 6.0 {
            say_now("Landed on target.");
            return;
        }
        // Along-track (the arc falling short or long) and cross-track (an aiming error)
        // are announced separately: knowing which one it was is the point of measuring.
        let mut bits = Vec::new();
        if along_m.abs() * M_TO_FT >= 3.0 {
            bits.push(format!(
                "{} feet {}",
                py_f(along_m.abs() * M_TO_FT, 0),
                if along_m > 0.0 { "long" } else { "short" }
            ));
        }
        if cross_m.abs() * M_TO_FT >= 3.0 {
            bits.push(format!(
                "{} feet {}",
                py_f(cross_m.abs() * M_TO_FT, 0),
                if cross_m > 0.0 { "left" } else { "right" }
            ));
        }
        say_now(&if !bits.is_empty() {
            format!("Landed {}.", bits.join(" and "))
        } else {
            format!("Landed {} feet off target.", py_f(miss_ft, 0))
        });
        return;
    }

    if let Some(rest) = text.strip_prefix("TELEPORT_ERR:") {
        say_now(&format!("Move failed: {rest}"));
        return;
    }

    if let Some(rest) = text.strip_prefix("ARRANGE_OK:") {
        let n = int_or_zero(rest);
        say_now(&format!("Arranged {n} {}.", pyint_word(&n)));
        return;
    }

    if let Some(rest) = text.strip_prefix("ARRANGE_ERR:") {
        say_now(&format!("Arrangement error: {rest}"));
        return;
    }

    if let Some(rest) = text.strip_prefix("PLAYER_VEH_ID:") {
        if let Some(vid) = py_int_parse(rest) {
            VS.lock().player_veh_id = if vid.cmp_i64(0) != std::cmp::Ordering::Less {
                Some(vid)
            } else {
                None
            };
        }
    }
}

/// The CATALOG_END sort key: `(is_prop, display.lower())`.
fn catalog_sort_key(v: &PyDict) -> (u8, String) {
    let ty = py_or(&[dget(v, "type"), &PyValue::from("")]).clone();
    let is_prop = u8::from(py_lower(&ty) == "prop");
    let empty = PyValue::from("");
    let name = py_or(&[dget(v, "name"), dget(v, "model"), &empty]);
    let brand = py_or(&[dget(v, "brand"), &empty]);
    let display = if brand.truthy() && !py_lower(name).contains(&py_lower(brand)) {
        format!("{} {}", brand.py_str(), name.py_str())
    } else {
        py_lower(name)
    };
    (is_prop, display.to_lowercase())
}

// =============================================================================
//  Modal open/close + key hook management
// =============================================================================

/// `_MODAL_KEYS`, in install order. The digits pick the editor's translation step; they
/// are swallowed on the other screens too, the same trade the editor's letters make.
pub const MODAL_KEYS: [&str; 30] = [
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "page up",
    "page down",
    "tab",
    "enter",
    "esc",
    "space",
    "delete",
    "f",
    "c",
    "g",
    "w",
    "r",
    "v",
    "a",
    "x",
    "s",
    "d",
    "q",
    "e",
    "i",
    "1",
    "2",
    "3",
    "4",
];

type Job = Box<dyn FnOnce() + Send + 'static>;

/// `_event_queue`: every hook callback enqueues here and returns at once, so no UDP,
/// speech or wait ever runs on the low-level hook path (where it would push the hook past
/// Windows' LowLevelHooksTimeout and leak the suppressed key to the game).
static EVENT_QUEUE: LazyLock<(Sender<Job>, Receiver<Job>)> = LazyLock::new(unbounded);

fn enqueue(job: Job) {
    let _ = EVENT_QUEUE.0.send(job);
}

fn run_job(job: Job) {
    if let Err(p) = catch_unwind(AssertUnwindSafe(job)) {
        logw(&format!(
            "vehicle_spawner: handler error: {}",
            panic_msg(&*p)
        ));
    }
}

/// `_worker_loop`.
fn worker_loop() {
    let rx = EVENT_QUEUE.1.clone();
    while let Ok(job) = rx.recv() {
        run_job(job);
    }
}

/// Run every queued job on the calling thread, as the worker would. For tests.
#[doc(hidden)]
pub fn run_pending_for_tests() -> usize {
    let mut n = 0;
    while let Ok(job) = EVENT_QUEUE.1.try_recv() {
        run_job(job);
        n += 1;
    }
    n
}

/// The handler `_MODAL_KEYS` maps `name` to, run as the worker runs it.
pub fn dispatch_key(name: &str, event: &KeyEvent) {
    if name == "i" {
        screens::on_info();
        return;
    }
    let slots = get_slots();
    with_vs(|vs| {
        vs.slots = slots;
        vs.on_key(name, event);
    });
}

/// `_on_place_release(event)`.
pub fn on_place_release(event: &KeyEvent) {
    if !event.name.is_empty() {
        with_vs(|vs| vs.place_key_up(&event.name));
    }
}

/// `_hook_suppressed(key, handler, on_release)`: the hook passes the other direction and
/// eats this one, enqueueing the handler.
fn hook_modal_key(key: &str, on_release: bool) -> Result<HookRecord, String> {
    let owned = key.to_string();
    let cb: keyboard::RawHookCallback = Arc::new(move |e: &KeyEvent| {
        let pass_type = if on_release {
            KeyEventType::Down
        } else {
            KeyEventType::Up
        };
        if e.event_type == pass_type {
            return true;
        }
        let e = e.clone();
        if on_release {
            enqueue(Box::new(move || on_place_release(&e)));
        } else {
            let name = owned.clone();
            enqueue(Box::new(move || dispatch_key(&name, &e)));
        }
        false
    });
    keyboard::try_hook_key_raw(key, cb)
}

impl Vs {
    /// `_install_modal_hooks()`.
    pub(crate) fn install_modal_hooks(&mut self) {
        // Never drop the handles while hooks are live: they are the only way to release
        // the keys again.
        if !self.hook_handles.is_empty() {
            self.uninstall_modal_hooks();
        }
        self.hook_handles = Vec::new();
        if !keyboard::keyboard_ok() {
            return;
        }
        for key in MODAL_KEYS {
            match hook_modal_key(key, false) {
                Ok(h) => self.hook_handles.push(h),
                Err(e) => logw(&format!("vehicle_spawner: failed to hook {key}: {e}")),
            }
        }
        // The editor cares when a key comes back up (hold-to-repeat); suppressing the
        // release too keeps the game from seeing a key-up for a key-down it never got.
        for (key, _, _) in PLACE_KEY_AXES {
            match hook_modal_key(key, true) {
                Ok(h) => self.hook_handles.push(h),
                Err(e) => logw(&format!(
                    "vehicle_spawner: failed to hook release of {key}: {e}"
                )),
            }
        }
    }

    /// `_uninstall_modal_hooks()`. Each record is removed by its own id, so teardown
    /// always releases the key (the Python purges its callback from the package's
    /// blocking table for the same reason).
    pub(crate) fn uninstall_modal_hooks(&mut self) {
        if !keyboard::keyboard_ok() {
            self.hook_handles = Vec::new();
            return;
        }
        keyboard::unhook_suppressed(&mut self.hook_handles);
    }

    /// The part of `_open_modal()` after `_close_others()`.
    fn open_modal_rest(&mut self) {
        // Catalog readiness is the Lua side's job (it pre-builds on world ready). If it is
        // not ready, nudge it and say so; never block waiting for it.
        if self.catalog.is_empty() {
            self.send("REQUEST_CATALOG");
            self.say("Catalog not ready yet. Try again shortly.");
            return;
        }
        self.modal_open = true;
        self.drill_vehicle = None;
        self.install_modal_hooks();
        let start = if screens::PAGES.contains(&self.screen) {
            self.screen
        } else {
            Screen::Main
        };
        if start == Screen::Manage {
            self.pending_manage_announce = true;
            ACTIVE_VEHICLES_EVENT.clear();
            self.send("REQUEST_ACTIVE_VEHICLES");
        }
        self.enter_screen(start, true, Some("Vehicle spawner".to_string()));
    }

    /// `_close_modal(silent)`.
    pub(crate) fn close_modal(&mut self, silent: bool) {
        if !self.modal_open {
            return;
        }
        self.modal_open = false;
        // Park the ticker and drop held keys, or a key held as the modal closes would keep
        // stepping with no way to release it.
        self.leave_place3d();
        self.uninstall_modal_hooks();
        if !silent {
            self.say("Vehicle spawner closed.");
        }
    }
}

/// `_open_modal()`.
fn open_modal() {
    if VS.lock().modal_open {
        return;
    }
    // Close any other exclusive UI session before taking over the keyboard.
    if let Some(d) = deps() {
        let _ = catch_unwind(AssertUnwindSafe(|| (d.close_others_fn)()));
    }
    let slots = get_slots();
    with_vs(|vs| {
        vs.slots = slots;
        vs.open_modal_rest();
    });
}

/// `_toggle_modal()`.
pub fn toggle_modal() {
    if let Some(d) = deps()
        && !(d.is_focused_fn)()
    {
        return;
    }
    if VS.lock().modal_open {
        with_vs(|vs| vs.close_modal(false));
    } else {
        open_modal();
    }
}

/// `_toggle_modal_async()`: the F11 entry. Returns at once; the worker does the toggle.
fn toggle_modal_async() {
    enqueue(Box::new(toggle_modal));
}

static F11_INSTALLED: AtomicBool = AtomicBool::new(false);

/// `keyboard.add_hotkey("f11", _toggle_modal_async, suppress=True)` and its log lines.
/// A registered game hotkey rather than a hook hotkey: it is eaten only while the game is
/// in front (the hook version ate F11 in every program) and needs no resident hook.
#[doc(hidden)]
pub fn install_f11_hotkey() {
    if !keyboard::keyboard_ok() {
        return;
    }
    match keyboard::register_game_hotkey("f11", Arc::new(toggle_modal_async)) {
        Ok(_) => {
            F11_INSTALLED.store(true, Ordering::SeqCst);
            logi("Vehicle spawner F11 hotkey installed.");
        }
        Err(e) => logw(&format!("vehicle_spawner: failed to add F11 hotkey: {e}")),
    }
}

// =============================================================================
//  Test support
// =============================================================================

#[doc(hidden)]
pub type TestSend = Arc<dyn Fn(&str) + Send + Sync>;
#[doc(hidden)]
pub type TestPressed = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Stand-ins for the things a test must not touch: the 4461 socket, the clock, the live
/// modifier state, the random choice and the ticker thread.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct TestHooks {
    pub send: Option<TestSend>,
    pub clock: Option<Arc<dyn Fn() -> f64 + Send + Sync>>,
    pub pressed: Option<TestPressed>,
    pub choice: Option<Arc<dyn Fn(usize) -> usize + Send + Sync>>,
    pub no_ticker: bool,
}

static TEST_HOOKS: RwLock<Option<TestHooks>> = RwLock::new(None);

fn test_hooks() -> TestHooks {
    TEST_HOOKS.read().clone().unwrap_or_default()
}

#[doc(hidden)]
pub fn set_test_hooks(hooks: Option<TestHooks>) {
    *TEST_HOOKS.write() = hooks;
}

/// Back to the import-time state (hooks released, queue drained).
#[doc(hidden)]
pub fn reset_for_tests() {
    with_vs(|vs| {
        vs.uninstall_modal_hooks();
    });
    while EVENT_QUEUE.1.try_recv().is_ok() {}
    *VS.lock() = Vs::default();
    CATALOG_READY.clear();
    ACTIVE_VEHICLES_EVENT.clear();
    PLACE_ACTIVE.clear();
}

/// One pass of the ticker loop's body at time `now`, if the editor is up.
#[doc(hidden)]
pub fn tick_for_tests(now: f64) {
    if PLACE_ACTIVE.is_set() {
        with_vs(|vs| vs.place_tick(now));
    }
}

/// The parts of the module state a golden test compares.
#[doc(hidden)]
pub fn snapshot_for_tests() -> serde_json::Value {
    use serde_json::json;
    let vs = VS.lock();
    let item = |it: &Item| {
        json!({
            "model": beam_base::pyval::to_json(&it.model),
            "config": beam_base::pyval::to_json(&it.config),
            "displayName": it.display_name,
            "refMode": it.ref_mode,
            "refVehId": it.ref_veh_id,
            "replaceSlot": it.replace_slot,
            "replaceVehId": it.replace_veh_id,
            "replaceVehName": it.replace_veh_name,
            "vals": it.vals.to_vec(),
        })
    };
    json!({
        "screen": vs.screen.as_str(),
        "modal": vs.modal_open,
        "queue": vs.to_spawn.iter().map(item).collect::<Vec<_>>(),
        "place": vs.place_values.to_vec(),
        "unit_step": vs.place_unit_step,
        "launch": vs.place_launch_mode.as_str(),
        "place_mode": match vs.place_mode { PlaceMode::Spawn => "spawn", PlaceMode::Teleport => "teleport" },
        "spacing": vs.spacing_field.value(),
        "readout_pending": vs.place_readout_pending,
        "selected": vs.manage_selected.iter().copied().collect::<Vec<_>>(),
        "place_active": PLACE_ACTIVE.is_set(),
        "catalog_len": vs.catalog.len(),
        "hooks": vs.hook_handles.len(),
    })
}
