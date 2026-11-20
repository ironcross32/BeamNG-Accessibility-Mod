//! Owner: E5. The low-level keyboard hook, the shared F9/F10 layer hook, the
//! command-dispatch worker, suppressed-key hook records, arrow ownership and
//! `install_hotkeys` (`beamtel.py:5046-5259, 5393-5408, 5978-6034, 9355-9363, 10481-10520`).
//!
//! The Python uses the `keyboard` package; `hook` and `names` reproduce the parts of it the
//! engine relies on (which keys a hook matches, the names events carry, the order hooks
//! run in, and which keys are eaten). The hook callback must only classify and enqueue
//! (the reason is at `beamtel.py:5066`): it takes no engine lock, which is why `_kb_layer`
//! and `_live_mods` are atomics here rather than `State` fields. `_capture_mods`,
//! `_input_help_mode` and `next_key_timer` are touched only off the hook and live in
//! `state::State::kb`.

mod canonical_names;
mod hook;
mod names;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, LazyLock};

use beam_audio::controller::AudioController;
use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use regex::Regex;

use crate::speech_out::{say, set_command_context};
use crate::state::{HookRecord, Mods, state};
use crate::timer::Timer;

/// `command_timeout_sec`.
pub const COMMAND_TIMEOUT_SEC: f64 = 4.0;

/// Key down or up (`keyboard.KEY_DOWN` / `KEY_UP`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventType {
    Down,
    Up,
}

/// The fields handlers read from a `keyboard.KeyboardEvent` (`_SynthKeyEvent`).
/// `name` is the `keyboard` library's key name ("f9", "up", "space", "a", "?", ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub name: String,
    pub event_type: KeyEventType,
}

impl KeyEvent {
    /// `_SynthKeyEvent(name)` / `SimpleNamespace(event_type="down", name=name)`.
    pub fn down(name: &str) -> KeyEvent {
        KeyEvent {
            name: name.to_string(),
            event_type: KeyEventType::Down,
        }
    }

    /// A key-up event.
    pub fn up(name: &str) -> KeyEvent {
        KeyEvent {
            name: name.to_string(),
            event_type: KeyEventType::Up,
        }
    }
}

/// A handler run on the command worker for a suppressed key.
pub type KeyHandler = Arc<dyn Fn(&KeyEvent) + Send + Sync>;
/// A raw hook callback run INSIDE the low-level hook: returns true to pass the key on.
pub type RawHookCallback = Arc<dyn Fn(&KeyEvent) -> bool + Send + Sync>;

/// Which layer owns the keyboard (`_kb_layer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    F9,
    F10,
}

impl Layer {
    fn key_name(self) -> &'static str {
        match self {
            Layer::F9 => "f9",
            Layer::F10 => "f10",
        }
    }
}

pub(crate) fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_string())
}

// ---- KEYBOARD_OK -------------------------------------------------------------------------

static KEYBOARD_OK: LazyLock<bool> = LazyLock::new(|| {
    let ok = cfg!(windows);
    if !ok {
        log::warn!(
            "keyboard module unavailable \u{2013} 'pip install keyboard' and run as Administrator for key suppression."
        );
    }
    ok
});

/// `KEYBOARD_OK`: the hook could be installed (elevated, not blocked).
pub fn keyboard_ok() -> bool {
    *KEYBOARD_OK
}

// ---- Command dispatch worker -------------------------------------------------------------

type Job = Box<dyn FnOnce() + Send + 'static>;

/// `_command_queue`.
static COMMAND_QUEUE: LazyLock<(Sender<Job>, Receiver<Job>)> = LazyLock::new(unbounded);
/// `_command_worker_thread is not None`.
static WORKER_STARTED: Mutex<bool> = Mutex::new(false);

/// `_command_dispatch(fn, *args)`: queue work on the serial `command-dispatch` worker.
/// Safe inside a hook or any callback.
pub fn command_dispatch(f: Box<dyn FnOnce() + Send + 'static>) {
    if COMMAND_QUEUE.0.send(f).is_err() {
        log::error!("Failed to queue command");
    }
}

/// `_kb_enqueue(handler)`: wrap a suppressed-key handler so its body runs on the worker.
/// The wrapper returns nothing, which `hook_suppressed` reads as "suppress this key".
pub fn kb_enqueue(handler: KeyHandler) -> KeyHandler {
    Arc::new(move |event: &KeyEvent| {
        let h = Arc::clone(&handler);
        let event = event.clone();
        command_dispatch(Box::new(move || h(&event)));
    })
}

fn run_job(job: Job) {
    // A handler that panicked must not take the worker down with it, or every later
    // command would be silently dropped.
    if let Err(p) = catch_unwind(AssertUnwindSafe(job)) {
        log::error!("Command handler raised: {}", panic_text(&*p));
    }
}

fn command_worker_loop() {
    let rx = COMMAND_QUEUE.1.clone();
    while let Ok(job) = rx.recv() {
        run_job(job);
    }
}

/// `_start_command_worker()`. Also lets the system-wide hook be installed: this is the
/// engine starting up, which is the only place a real hook belongs.
pub fn start_command_worker() {
    hook::allow_os_hook();
    let mut started = WORKER_STARTED.lock();
    if *started {
        return;
    }
    *started = true;
    crate::timer::spawn_daemon("command-dispatch", command_worker_loop);
}

/// Run every queued command on the calling thread, in order, as the worker would. For
/// tests, which drive handlers without starting the worker.
#[doc(hidden)]
pub fn run_pending_commands_for_tests() -> usize {
    let mut n = 0;
    while let Ok(job) = COMMAND_QUEUE.1.try_recv() {
        run_job(job);
        n += 1;
    }
    n
}

/// Feed one key event through the listener as the OS hook would (blocking hooks, key
/// hooks, then the processing thread's hotkeys and handlers, run inline), using the first
/// scan code `key` maps to. Returns whether the key would pass on to the game. For tests;
/// no system-wide hook is involved.
#[doc(hidden)]
pub fn deliver_for_tests(key: &str, event: &KeyEvent) -> bool {
    let code = names::key_to_scan_codes(key)
        .ok()
        .and_then(|c| c.first().copied())
        .unwrap_or(0);
    let raw = hook::RawEvent {
        event: event.clone(),
        scan_code: code,
        has_name: !event.name.is_empty(),
    };
    let (pass, queue) = hook::LISTENER.direct_callback(&raw);
    if queue {
        hook::LISTENER.process(&raw);
    }
    pass
}

// ---- Shared layer hook (F9 and F10) ------------------------------------------------------
//
// Both layers share one blocking hook rather than installing their own: opening F10 while
// F9 was still open used to leave F9's hook behind. The hook exists only while a layer is
// open, so with no layer open every key reaches BeamNG untouched.

const LAYER_NONE: u8 = 0;
const LAYER_F9: u8 = 1;
const LAYER_F10: u8 = 2;

/// `_kb_layer`.
static KB_LAYER: AtomicU8 = AtomicU8::new(LAYER_NONE);
/// `_kb_layer_hook` and `_kb_layer_release_hook`.
static LAYER_HOOKS: Mutex<Option<(u64, u64)>> = Mutex::new(None);

/// `_live_mods`: modifier state as the hook sees it, live. Snapshotted per key-down and
/// handed to the worker, because the modifier may be released before the command runs.
static LIVE_CTRL: AtomicBool = AtomicBool::new(false);
static LIVE_SHIFT: AtomicBool = AtomicBool::new(false);
static LIVE_ALT: AtomicBool = AtomicBool::new(false);

fn layer_from(v: u8) -> Option<Layer> {
    match v {
        LAYER_F9 => Some(Layer::F9),
        LAYER_F10 => Some(Layer::F10),
        _ => None,
    }
}

fn layer_to(l: Option<Layer>) -> u8 {
    match l {
        Some(Layer::F9) => LAYER_F9,
        Some(Layer::F10) => LAYER_F10,
        None => LAYER_NONE,
    }
}

/// Which modifier `_MOD_ALIASES` maps a key name to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Modifier {
    Ctrl,
    Shift,
    Alt,
}

/// `_MOD_ALIASES.get(name)`.
pub(crate) fn mod_alias(name: &str) -> Option<Modifier> {
    match name {
        "ctrl" | "control" | "left ctrl" | "right ctrl" => Some(Modifier::Ctrl),
        "shift" | "left shift" | "right shift" => Some(Modifier::Shift),
        "alt" | "left alt" | "right alt" => Some(Modifier::Alt),
        _ => None,
    }
}

fn live_flag(m: Modifier) -> &'static AtomicBool {
    match m {
        Modifier::Ctrl => &LIVE_CTRL,
        Modifier::Shift => &LIVE_SHIFT,
        Modifier::Alt => &LIVE_ALT,
    }
}

fn reset_live_mods() {
    LIVE_CTRL.store(false, Ordering::SeqCst);
    LIVE_SHIFT.store(false, Ordering::SeqCst);
    LIVE_ALT.store(false, Ordering::SeqCst);
}

fn live_mods_snapshot() -> Mods {
    Mods {
        ctrl: LIVE_CTRL.load(Ordering::SeqCst),
        shift: LIVE_SHIFT.load(Ordering::SeqCst),
        alt: LIVE_ALT.load(Ordering::SeqCst),
    }
}

/// What the layer hook does with one key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LayerAction {
    /// Pass the key on (key-ups, or no layer open).
    Pass,
    /// A modifier went down: note it and eat the key.
    Modifier(Modifier),
    /// The F9/F10 that opened the layer, repeating: eat it.
    Repeat,
    /// Eat the key and run the layer body for `name` on the worker.
    Dispatch(Layer, String),
}

/// The classification `_kb_layer_press` makes, without its side effects.
pub(crate) fn classify_layer_key(layer: Option<Layer>, event: &KeyEvent) -> LayerAction {
    if event.event_type != KeyEventType::Down {
        return LayerAction::Pass; // key-ups reach the game, as they always have
    }
    let Some(layer) = layer else {
        // Closed between this key arriving and teardown finishing.
        return LayerAction::Pass;
    };
    let name = event.name.to_lowercase();
    if let Some(m) = mod_alias(&name) {
        return LayerAction::Modifier(m);
    }
    if name == layer.key_name() {
        return LayerAction::Repeat;
    }
    LayerAction::Dispatch(layer, name)
}

/// `_kb_layer_press(event)`: WH_KEYBOARD_LL context. True passes the key on.
/// Keep this cheap: no speech, no locks, no I/O, no unhooking.
pub(crate) fn kb_layer_press(event: &KeyEvent) -> bool {
    match classify_layer_key(kb_layer(), event) {
        LayerAction::Pass => true,
        LayerAction::Modifier(m) => {
            live_flag(m).store(true, Ordering::SeqCst);
            false
        }
        LayerAction::Repeat => false,
        LayerAction::Dispatch(layer, name) => {
            let mods = live_mods_snapshot();
            command_dispatch(Box::new(move || kb_run_layer_command(layer, &name, mods)));
            false
        }
    }
}

/// `_kb_layer_release(event)`: a non-suppressing handler, so it runs on the processing
/// thread and is not on the hook timeout clock.
pub(crate) fn kb_layer_release(event: &KeyEvent) {
    if event.event_type != KeyEventType::Up {
        return;
    }
    if let Some(m) = mod_alias(&event.name.to_lowercase()) {
        live_flag(m).store(false, Ordering::SeqCst);
    }
}

/// `_kb_run_layer_command(layer, name, mods)`: worker context; restore the modifier
/// snapshot, then run the layer body.
pub(crate) fn kb_run_layer_command(layer: Layer, name: &str, mods: Mods) {
    if Some(layer) != kb_layer() {
        return; // layer closed or switched while this sat in the queue
    }
    if layer == Layer::F9 {
        state().kb.capture_mods = mods;
        crate::f9_commands::on_next_key_press(
            &KeyEvent::down(name),
            crate::state::audio_controller_ref(),
        );
    } else {
        state().ai.ai_mods = mods;
        crate::ai_layer::on_ai_key_press(&KeyEvent::down(name));
    }
}

/// `_kb_layer`.
pub fn kb_layer() -> Option<Layer> {
    layer_from(KB_LAYER.load(Ordering::SeqCst))
}

/// `_kb_open_layer(which)`: hand the keyboard to `which` layer, installing the hook if
/// needed.
pub fn kb_open_layer(which: Layer) {
    KB_LAYER.store(layer_to(Some(which)), Ordering::SeqCst);
    reset_live_mods();
    let mut hooks = LAYER_HOOKS.lock();
    if hooks.is_some() {
        return; // already installed; retargeted above
    }
    hook::start_if_necessary();
    let press_id = hook::next_id();
    hook::LISTENER.add_blocking_hook(press_id, Arc::new(kb_layer_press));
    // keyboard.on_release: true for key-downs, which stops later handlers.
    let release_id = hook::next_id();
    hook::LISTENER.add_handler(
        release_id,
        Arc::new(|e: &KeyEvent| {
            if e.event_type == KeyEventType::Down {
                return true;
            }
            kb_layer_release(e);
            false
        }),
    );
    *hooks = Some((press_id, release_id));
}

/// `_kb_close_layer()`: release the keyboard back to the game. Never called from inside
/// the hook.
pub fn kb_close_layer() {
    KB_LAYER.store(LAYER_NONE, Ordering::SeqCst);
    let ids = LAYER_HOOKS.lock().take();
    if let Some((press, release)) = ids {
        hook::LISTENER.remove(press);
        hook::LISTENER.remove(release);
    }
    reset_live_mods();
}

/// `_clear_next_key_hook(speak_exit)`.
pub fn clear_next_key_hook(speak_exit: bool) {
    set_command_context(false);
    state().kb.input_help_mode = false;
    if kb_layer() == Some(Layer::F9) {
        kb_close_layer();
    }
    let timer = {
        let mut st = state();
        let t = st.kb.next_key_timer.take();
        st.kb.capture_mods = Mods::default();
        t
    };
    if let Some(t) = timer {
        t.cancel();
    }
    if speak_exit {
        say("Exit", true, true, "python");
    }
}

/// `_start_next_key_capture(audio_controller)`.
pub fn start_next_key_capture(audio_controller: Arc<AudioController>) {
    let _ = audio_controller;
    clear_next_key_hook(false);
    kb_open_layer(Layer::F9);
    let timer = Timer::start(COMMAND_TIMEOUT_SEC, || clear_next_key_hook(true));
    state().kb.next_key_timer = Some(timer);
}

// ---- Suppressed key hooks ----------------------------------------------------------------

/// `_hook_suppressed(key, handler)` with the error the caller's `except` would log.
///
/// The Python keeps its own reference to each callback because the package indexes hooks
/// by key name too, so two suppressing hooks on one key clobbered each other's teardown
/// and left the key swallowed for the life of the process. Here every record is removed by
/// its own id, so teardown always releases the key.
pub fn try_hook_suppressed(key: &str, handler: KeyHandler) -> Result<HookRecord, String> {
    let cb: RawHookCallback = Arc::new(move |e: &KeyEvent| {
        if e.event_type == KeyEventType::Up {
            return true;
        }
        handler(e);
        false
    });
    try_hook_key_raw(key, cb)
}

/// `_hook_suppressed(key, handler)`: a suppressing key-down hook; key-ups pass. None when
/// the key name is not mapped (Python raises `ValueError`).
pub fn hook_suppressed(key: &str, handler: KeyHandler) -> Option<HookRecord> {
    try_hook_suppressed(key, handler).ok()
}

/// `keyboard.hook_key(key, cb, suppress=True)`, with the error text.
pub fn try_hook_key_raw(key: &str, cb: RawHookCallback) -> Result<HookRecord, String> {
    hook::start_if_necessary();
    let codes = names::key_to_scan_codes(key)?;
    let id = hook::next_id();
    hook::LISTENER.add_blocking_key(id, &codes, cb);
    Ok(HookRecord(id))
}

/// `keyboard.hook_key(key, cb, suppress=True)` with the callback deciding per event
/// (`_install_nodegrab_hooks`). Same record shape as [`hook_suppressed`].
pub fn hook_key_raw(key: &str, cb: RawHookCallback) -> Option<HookRecord> {
    try_hook_key_raw(key, cb).ok()
}

/// `_unhook_suppressed(records)`: undo the records and clear the list.
pub fn unhook_suppressed(records: &mut Vec<HookRecord>) {
    for rec in records.iter() {
        hook::LISTENER.remove(rec.0);
    }
    records.clear();
}

static STEP_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r",\s?").expect("static regex"));
static KEY_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s?\+\s?").expect("static regex"));

/// `keyboard.parse_hotkey(hotkey)` for a name: steps of keys of scan codes.
fn parse_hotkey(hotkey: &str) -> Result<Vec<Vec<Vec<i32>>>, String> {
    if hotkey.chars().count() == 1 {
        return Ok(vec![vec![names::key_to_scan_codes(hotkey)?]]);
    }
    let mut steps = Vec::new();
    for step in STEP_SPLIT.split(hotkey) {
        let mut keys = Vec::new();
        for key in KEY_SPLIT.split(step) {
            keys.push(names::key_to_scan_codes(key)?);
        }
        steps.push(keys);
    }
    Ok(steps)
}

/// `keyboard.is_pressed(name)` ("shift", "ctrl"). Where the package raises (an unmapped
/// name, a multi-step hotkey) this answers false.
pub fn is_pressed(name: &str) -> bool {
    hook::start_if_necessary();
    let Ok(steps) = parse_hotkey(name) else {
        return false;
    };
    if steps.len() > 1 {
        return false;
    }
    steps[0]
        .iter()
        .all(|codes| hook::LISTENER.any_pressed(codes))
}

/// `parse_hotkey_combinations(hotkey)` for a single-step hotkey: every combination of the
/// keys' scan codes (`itertools.product`), each sorted.
fn hotkey_combinations(hotkey: &str) -> Result<Vec<Vec<i32>>, String> {
    let steps = parse_hotkey(hotkey)?;
    if steps.len() != 1 {
        return Err(format!("multi-step hotkeys are not supported: {hotkey:?}"));
    }
    let mut combos: Vec<Vec<i32>> = vec![Vec::new()];
    for codes in &steps[0] {
        let mut next = Vec::new();
        for prefix in &combos {
            for c in codes {
                let mut v = prefix.clone();
                v.push(*c);
                next.push(v);
            }
        }
        combos = next;
    }
    for c in &mut combos {
        c.sort();
    }
    Ok(combos)
}

/// `keyboard.add_hotkey(hotkey, callback, suppress=True)` for a single-step hotkey, as
/// `vehicle_spawner.start` installs F11.
///
/// The callback runs INSIDE the low-level hook on each key-down while exactly the
/// hotkey's keys are pressed, so it must only classify and enqueue. It returns the
/// truthiness of the Python callback's return value: false (Python's None) eats the key,
/// and as that key-down was never logically pressed, its key-up is eaten too. Modifiers
/// in the hotkey go through the package's modifier transition table, which can replay a
/// held-back modifier press. Suppressed events still reach the non-blocking handlers.
/// Remove it with [`remove_hotkey`].
pub fn add_hotkey_suppressed(
    hotkey: &str,
    callback: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<HookRecord, String> {
    hook::start_if_necessary();
    let combos = hotkey_combinations(hotkey)?;
    let id = hook::next_id();
    hook::LISTENER.add_blocking_hotkey(id, &combos, callback);
    Ok(HookRecord(id))
}

/// `keyboard.remove_hotkey(...)` for a record from [`add_hotkey`] or
/// [`add_hotkey_suppressed`].
pub fn remove_hotkey(record: HookRecord) {
    hook::LISTENER.remove(record.0);
}

/// `keyboard.add_hotkey(hotkey, callback, suppress=False)` for a single-step hotkey: the
/// callback runs on the processing thread on each key-down while exactly the hotkey's keys
/// are pressed (auto-repeat included).
pub fn add_hotkey(
    hotkey: &str,
    callback: Arc<dyn Fn() + Send + Sync>,
) -> Result<HookRecord, String> {
    hook::start_if_necessary();
    let combos = hotkey_combinations(hotkey)?;
    let id = hook::next_id();
    hook::LISTENER.add_nonblocking_hotkey(
        id,
        &combos,
        Arc::new(move |e: &KeyEvent| {
            if e.event_type == KeyEventType::Down {
                callback();
            }
        }),
    );
    Ok(HookRecord(id))
}

/// `release_arrow_owners(reason="")`: hand the arrow keys back, whoever holds them.
///
/// Status mode and the virtual browser both take the arrows exclusively, and the vehicle
/// spawner takes them too. Without one place that releases all of them, opening one on
/// top of another leaves two owners on the same key.
pub fn release_arrow_owners(reason: &str) {
    crate::virtual_browser::close_virtual_browser(false);
    crate::listeners::lineup::lineup_close_layer(false);
    let (active, orphans) = {
        let mut st = state();
        if st.menu.status_keyboard_mode_active {
            (true, Vec::new())
        } else {
            (false, std::mem::take(&mut st.menu.status_arrow_hooks))
        }
    };
    if active {
        crate::menus::toggle_status_mode();
    } else if !orphans.is_empty() {
        // Hooks outliving the mode flag: release them anyway rather than leaving the
        // arrows eaten with no mode to toggle off.
        let mut orphans = orphans;
        unhook_suppressed(&mut orphans);
        if !reason.is_empty() {
            log::warn!("Released orphaned status-mode arrow hooks ({reason})");
        }
    }
}

/// `install_hotkeys`' `on_f9`, run on the processing thread.
fn on_f9(audio_controller: &Arc<AudioController>) {
    if !crate::win_util::is_beamng_focused() {
        return;
    }
    set_command_context(true);
    say("Command?", true, true, "python");
    start_next_key_capture(Arc::clone(audio_controller));
}

/// `install_hotkeys`' `on_f10`.
fn on_f10() {
    if !crate::win_util::is_beamng_focused() {
        return;
    }
    set_command_context(true);
    // Revalidate slots so deleted/renamed vehicles don't linger in the list the user is
    // about to address with Ctrl+/Shift+digit.
    crate::ai_layer::send_slot_command("SLOT_VALIDATE");
    say("AI?", true, true, "python");
    crate::ai_layer::start_ai_capture();
}

/// `install_hotkeys(audio_controller)`: F9 and F10 (also forces the speech library load).
pub fn install_hotkeys(audio_controller: Arc<AudioController>) {
    if !keyboard_ok() {
        log::info!("Command mode disabled (keyboard module not available / not elevated).");
        return;
    }

    start_command_worker();

    // Force the speech library load now. on_f9/on_f10 speak the layer prompt before
    // installing the layer hook, and they run on the key processing thread; if the first
    // utterance of the session had to load the native library there, the hook would go in
    // late and the user's next key would reach the game instead of the layer.
    if !beam_speech::speech::init(None) {
        log::warn!("Speech unavailable at hotkey install; speech may be degraded.");
    }

    let ac = Arc::clone(&audio_controller);
    if let Err(e) = add_hotkey("f9", Arc::new(move || on_f9(&ac))) {
        log::error!("Failed to add F9 hotkey: {e}");
    }
    if let Err(e) = add_hotkey("f10", Arc::new(on_f10)) {
        log::error!("Failed to add F10 hotkey: {e}");
    }
}

#[cfg(test)]
mod tests;
