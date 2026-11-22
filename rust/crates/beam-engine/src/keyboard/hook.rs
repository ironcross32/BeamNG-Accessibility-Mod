//! The low-level hook and the listener behind it: the parts of the `keyboard` package the
//! engine relies on, with the package's semantics.
//!
//! * Every key event first goes through the *blocking hooks* (`keyboard.hook(cb,
//!   suppress=True)`, the shared F9/F10 layer hook). If any returns false the key is eaten
//!   and nothing else sees it, not even the pressed-key table.
//! * Then the pressed-key table is updated, and the *blocking key hooks*
//!   (`keyboard.hook_key(key, cb, suppress=True)`) for that scan code run; one returning
//!   false eats the key, and it is not queued.
//! * Everything that got through is queued for the processing thread, which runs the
//!   non-blocking hotkeys (`add_hotkey(..., suppress=False)`, matched against the pressed
//!   set as it is when the event is processed) and then the non-blocking handlers
//!   (`keyboard.on_release`), stopping at the first handler that returns true.
//!
//! The hook callback runs inside the Win32 `WH_KEYBOARD_LL` callback, which Windows gives
//! `LowLevelHooksTimeout` milliseconds (see `beamtel.py:5066`). The registered callbacks
//! classify and enqueue only. The listener's own bookkeeping (a read lock on the hook
//! registry, the pressed-key set) is held for microseconds, never across a callback, and
//! no engine lock is taken here.

use std::collections::{BTreeSet, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::{Mutex, RwLock};

use super::{KeyEvent, KeyEventType};

/// A blocking callback: true passes the key on, false eats it.
pub(crate) type BlockingFn = Arc<dyn Fn(&KeyEvent) -> bool + Send + Sync>;
/// A non-blocking handler on the processing thread: true stops later handlers.
pub(crate) type HandlerFn = Arc<dyn Fn(&KeyEvent) -> bool + Send + Sync>;
/// A non-blocking hotkey callback on the processing thread.
pub(crate) type HotkeyFn = Arc<dyn Fn(&KeyEvent) + Send + Sync>;

/// One OS key event after naming: the handlers' view plus what the listener keys on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawEvent {
    pub event: KeyEvent,
    /// `scan_code or -vk`.
    pub scan_code: i32,
    /// False when Windows has no name for the key (`event.name` is None).
    pub has_name: bool,
}

/// A suppressing hotkey's callback, run INSIDE the hook on key-down: its result is the
/// truthiness of the Python callback's return value (None, i.e. false, eats the key).
pub(crate) type BlockingHotkeyFn = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Default)]
struct Registry {
    blocking_hooks: Vec<(u64, BlockingFn)>,
    blocking_keys: HashMap<i32, Vec<(u64, BlockingFn)>>,
    handlers: Vec<(u64, HandlerFn)>,
    nonblocking_hotkeys: HashMap<Vec<i32>, Vec<(u64, HotkeyFn)>>,
    /// `blocking_hotkeys`. Like the package's defaultdict, a combination stays as a key
    /// once added, so "any suppressing hotkey was ever added" is `!is_empty()`.
    blocking_hotkeys: HashMap<Vec<i32>, Vec<(u64, BlockingHotkeyFn)>>,
    /// The combinations each suppressing hotkey added, to undo `filtered_modifiers`.
    blocking_hotkey_combos: HashMap<u64, Vec<Vec<i32>>>,
    /// `filtered_modifiers` (a Counter).
    filtered_modifiers: HashMap<i32, i64>,
}

/// `modifier_states` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModState {
    Free,
    Pending,
    Suppressed,
    Allowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Modifier,
    Hotkey,
    Other,
}

/// `_KeyboardListener.transition_table`: (send a fake press, accept override, next state).
pub(crate) fn transition(
    state: ModState,
    down: bool,
    origin: Origin,
) -> (bool, Option<bool>, ModState) {
    use ModState::*;
    match (state, down, origin) {
        (Free, false, Origin::Modifier) => (false, Some(true), Free),
        (Free, true, Origin::Modifier) => (false, Some(false), Pending),
        (Pending, false, Origin::Modifier) => (true, Some(true), Free),
        (Pending, true, Origin::Modifier) => (false, Some(true), Allowed),
        (Suppressed, false, Origin::Modifier) => (false, Some(false), Free),
        (Suppressed, true, Origin::Modifier) => (false, Some(false), Suppressed),
        (Allowed, false, Origin::Modifier) => (false, Some(true), Free),
        (Allowed, true, Origin::Modifier) => (false, Some(true), Allowed),

        (Free, _, Origin::Hotkey) => (false, None, Free),
        (Pending, _, Origin::Hotkey) => (false, None, Suppressed),
        (Suppressed, _, Origin::Hotkey) => (false, None, Suppressed),
        (Allowed, _, Origin::Hotkey) => (false, None, Allowed),

        (Free, _, Origin::Other) => (false, Some(true), Free),
        (Pending, _, Origin::Other) => (true, Some(true), Allowed),
        (Suppressed, false, Origin::Other) => (false, Some(false), Allowed),
        (Suppressed, true, Origin::Other) => (true, Some(true), Allowed),
        (Allowed, _, Origin::Other) => (false, Some(true), Allowed),
    }
}

/// The hook-side bookkeeping only suppressing hotkeys use.
#[derive(Default)]
struct HotkeyBook {
    /// `active_modifiers`.
    active_modifiers: BTreeSet<i32>,
    /// `modifier_states` ("free" when absent).
    modifier_states: HashMap<i32, ModState>,
    /// `_logically_pressed_keys`.
    logically_pressed: BTreeSet<i32>,
}

type IsModifierFn = Arc<dyn Fn(i32) -> bool + Send + Sync>;
type PressFn = Arc<dyn Fn(i32) + Send + Sync>;

/// `keyboard._listener`.
pub(crate) struct Listener {
    reg: RwLock<Registry>,
    /// `_pressed_events`, by scan code.
    pressed: Mutex<BTreeSet<i32>>,
    book: Mutex<HotkeyBook>,
    /// `is_modifier(scan_code)`.
    is_modifier: IsModifierFn,
    /// `_os_keyboard.press(scan_code)`: the fake press the transition table asks for.
    press: PressFn,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

impl Listener {
    pub(crate) fn new() -> Listener {
        Listener::with_keys(
            Arc::new(super::names::is_modifier_code),
            Arc::new(super::names::press_code),
        )
    }

    /// A listener with its modifier test and fake-press output supplied (tests).
    pub(crate) fn with_keys(is_modifier: IsModifierFn, press: PressFn) -> Listener {
        Listener {
            reg: RwLock::new(Registry::default()),
            pressed: Mutex::new(BTreeSet::new()),
            book: Mutex::new(HotkeyBook::default()),
            is_modifier,
            press,
        }
    }

    /// `_add_hotkey_step(handler, combinations, suppress=True)`.
    pub(crate) fn add_blocking_hotkey(&self, id: u64, combos: &[Vec<i32>], cb: BlockingHotkeyFn) {
        let mut reg = self.reg.write();
        for combo in combos {
            for c in combo {
                if (self.is_modifier)(*c) {
                    *reg.filtered_modifiers.entry(*c).or_insert(0) += 1;
                }
            }
            reg.blocking_hotkeys
                .entry(combo.clone())
                .or_default()
                .push((id, Arc::clone(&cb)));
        }
        reg.blocking_hotkey_combos.insert(id, combos.to_vec());
    }

    pub(crate) fn add_blocking_hook(&self, id: u64, cb: BlockingFn) {
        self.reg.write().blocking_hooks.push((id, cb));
    }

    pub(crate) fn add_blocking_key(&self, id: u64, codes: &[i32], cb: BlockingFn) {
        let mut reg = self.reg.write();
        for c in codes {
            reg.blocking_keys
                .entry(*c)
                .or_default()
                .push((id, Arc::clone(&cb)));
        }
    }

    pub(crate) fn add_handler(&self, id: u64, cb: HandlerFn) {
        self.reg.write().handlers.push((id, cb));
    }

    pub(crate) fn add_nonblocking_hotkey(&self, id: u64, combos: &[Vec<i32>], cb: HotkeyFn) {
        let mut reg = self.reg.write();
        for combo in combos {
            reg.nonblocking_hotkeys
                .entry(combo.clone())
                .or_default()
                .push((id, Arc::clone(&cb)));
        }
    }

    /// Remove every callback registered under `id`, wherever it is.
    pub(crate) fn remove(&self, id: u64) {
        let mut reg = self.reg.write();
        reg.blocking_hooks.retain(|(i, _)| *i != id);
        for list in reg.blocking_keys.values_mut() {
            list.retain(|(i, _)| *i != id);
        }
        reg.handlers.retain(|(i, _)| *i != id);
        for list in reg.nonblocking_hotkeys.values_mut() {
            list.retain(|(i, _)| *i != id);
        }
        if let Some(combos) = reg.blocking_hotkey_combos.remove(&id) {
            for combo in &combos {
                for c in combo {
                    if (self.is_modifier)(*c) {
                        *reg.filtered_modifiers.entry(*c).or_insert(0) -= 1;
                    }
                }
            }
        }
        for list in reg.blocking_hotkeys.values_mut() {
            list.retain(|(i, _)| *i != id);
        }
    }

    /// `direct_callback(event)`: (pass the key on, queue it for the processing thread).
    pub(crate) fn direct_callback(&self, ev: &RawEvent) -> (bool, bool) {
        let hooks: Vec<BlockingFn> = self
            .reg
            .read()
            .blocking_hooks
            .iter()
            .map(|(_, f)| Arc::clone(f))
            .collect();
        for hook in hooks {
            if !hook(&ev.event) {
                return (false, false);
            }
        }
        let code = ev.scan_code;
        let down = ev.event.event_type == KeyEventType::Down;
        // Update tables of currently pressed keys and modifiers; the hotkey is the pressed
        // set with this key-down added, or before this key-up is removed.
        let hotkey: Vec<i32> = {
            let mut pressed = self.pressed.lock();
            let mut book = self.book.lock();
            if down {
                if (self.is_modifier)(code) {
                    book.active_modifiers.insert(code);
                }
                pressed.insert(code);
            }
            let hotkey = pressed.iter().copied().collect();
            if !down {
                book.active_modifiers.remove(&code);
                pressed.remove(&code);
            }
            hotkey
        };
        let key_hooks: Vec<BlockingFn> = self
            .reg
            .read()
            .blocking_keys
            .get(&ev.scan_code)
            .map(|l| l.iter().map(|(_, f)| Arc::clone(f)).collect())
            .unwrap_or_default();
        for hook in key_hooks {
            if !hook(&ev.event) {
                return (false, false);
            }
        }

        // Default accept.
        let mut accept = true;
        let (any_blocking, filtered, callbacks) = {
            let reg = self.reg.read();
            (
                !reg.blocking_hotkeys.is_empty(),
                reg.filtered_modifiers.get(&code).copied().unwrap_or(0) != 0,
                reg.blocking_hotkeys
                    .get(&hotkey)
                    .map(|l| l.iter().map(|(_, f)| Arc::clone(f)).collect::<Vec<_>>())
                    .unwrap_or_default(),
            )
        };
        if any_blocking {
            let (modifiers_to_update, origin) = if filtered {
                (BTreeSet::from([code]), Origin::Modifier)
            } else {
                let mut m = self.book.lock().active_modifiers.clone();
                if (self.is_modifier)(code) {
                    m.insert(code);
                }
                if callbacks.is_empty() {
                    (m, Origin::Other)
                } else {
                    // The single-step handler: `(e.event_type == KEY_UP and e.scan_code in
                    // _logically_pressed_keys) or (e.event_type == KEY_DOWN and callback())`,
                    // every handler called (a list comprehension, then all()).
                    let logically = self.book.lock().logically_pressed.contains(&code);
                    let results: Vec<bool> = callbacks
                        .iter()
                        .map(|cb| if down { cb() } else { logically })
                        .collect();
                    accept = results.iter().all(|r| *r);
                    (m, Origin::Hotkey)
                }
            };
            for key in modifiers_to_update {
                let state = self
                    .book
                    .lock()
                    .modifier_states
                    .get(&key)
                    .copied()
                    .unwrap_or(ModState::Free);
                let (should_press, new_accept, new_state) = transition(state, down, origin);
                if should_press {
                    (self.press)(key);
                }
                if let Some(a) = new_accept {
                    accept = a;
                }
                self.book.lock().modifier_states.insert(key, new_state);
            }
        }

        if accept {
            let mut book = self.book.lock();
            if down {
                book.logically_pressed.insert(code);
            } else {
                book.logically_pressed.remove(&code);
            }
        }
        // Queued for the non-blocking handlers even when a hotkey ate it.
        (accept, true)
    }

    #[cfg(test)]
    pub(crate) fn modifier_state(&self, code: i32) -> ModState {
        self.book
            .lock()
            .modifier_states
            .get(&code)
            .copied()
            .unwrap_or(ModState::Free)
    }

    /// One queued event on the processing thread: `pre_process_event` then
    /// `invoke_handlers`. Returns false when a hotkey callback panicked, which in Python
    /// is an exception out of `pre_process_event` that ends the processing thread.
    pub(crate) fn process(&self, ev: &RawEvent) -> bool {
        let hotkey: Vec<i32> = self.pressed.lock().iter().copied().collect();
        let hotkeys: Vec<HotkeyFn> = self
            .reg
            .read()
            .nonblocking_hotkeys
            .get(&hotkey)
            .map(|l| l.iter().map(|(_, f)| Arc::clone(f)).collect())
            .unwrap_or_default();
        for cb in hotkeys {
            if let Err(p) = catch_unwind(AssertUnwindSafe(|| cb(&ev.event))) {
                log::error!(
                    "Keyboard event processing stopped: {}",
                    super::panic_text(&*p)
                );
                return false;
            }
        }
        // `event.scan_code or (event.name and event.name != 'unknown')`
        let proceed = ev.scan_code != 0
            || (ev.has_name && !ev.event.name.is_empty() && ev.event.name != "unknown");
        if proceed {
            let handlers: Vec<HandlerFn> = self
                .reg
                .read()
                .handlers
                .iter()
                .map(|(_, f)| Arc::clone(f))
                .collect();
            for h in handlers {
                match catch_unwind(AssertUnwindSafe(|| h(&ev.event))) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(p) => log::error!("Keyboard handler raised: {}", super::panic_text(&*p)),
                }
            }
        }
        true
    }

    /// Is any of `codes` in the pressed-key table?
    pub(crate) fn any_pressed(&self, codes: &[i32]) -> bool {
        let pressed = self.pressed.lock();
        codes.iter().any(|c| pressed.contains(c))
    }

    /// Whether anything is registered that needs to see key events.
    pub(crate) fn has_work(&self) -> bool {
        let reg = self.reg.read();
        !reg.blocking_hooks.is_empty()
            || reg.blocking_keys.values().any(|l| !l.is_empty())
            || !reg.handlers.is_empty()
            || reg.nonblocking_hotkeys.values().any(|l| !l.is_empty())
            || reg.blocking_hotkeys.values().any(|l| !l.is_empty())
    }

    /// Forget every key and modifier state: the hook is coming down, so it will miss the
    /// key-ups that would have cleared them.
    pub(crate) fn reset_tracking(&self) {
        self.pressed.lock().clear();
        *self.book.lock() = HotkeyBook::default();
    }
}

/// The process-wide listener.
pub(crate) static LISTENER: LazyLock<Listener> = LazyLock::new(Listener::new);

static QUEUE: LazyLock<(Sender<RawEvent>, Receiver<RawEvent>)> = LazyLock::new(unbounded);

static LISTENING: Mutex<bool> = Mutex::new(false);
/// Whether real system-wide input hooks may be installed (set once the engine starts the
/// command worker; never in a test that only drives handlers).
static OS_HOOK_ALLOWED: AtomicBool = AtomicBool::new(false);
/// The thread running the installed `WH_KEYBOARD_LL` hook, if one is installed.
static HOOK_THREAD: Mutex<Option<u32>> = Mutex::new(None);

/// Allow the OS hooks, and install whatever is already registered.
pub(crate) fn allow_os_hook() {
    OS_HOOK_ALLOWED.store(true, Ordering::SeqCst);
    sync_os_hook();
}

pub(crate) fn os_hook_allowed() -> bool {
    OS_HOOK_ALLOWED.load(Ordering::SeqCst)
}

/// `_listener.start_if_necessary()`: build the name tables and start the processing
/// thread. The OS hook is [`sync_os_hook`]'s business.
pub(crate) fn start_if_necessary() {
    let mut listening = LISTENING.lock();
    if !*listening {
        super::names::setup_name_tables();
        *listening = true;
        crate::timer::spawn_daemon("keyboard-process", process_loop);
    }
}

/// Install the low-level hook when something needs keys, and remove it when nothing does.
/// Call after every registration change.
///
/// The hook is never left installed idle. NVDA has a low-level hook of its own, and the
/// two fight over their order in the chain: with ours resident and installed after NVDA's,
/// NVDA stopped interrupting itself and NVDA+Space did nothing in this app's window; with
/// NVDA restarted afterwards, our F9 went silent. Installing at the moment a layer or mode
/// takes the keyboard puts ours first in the chain exactly while it is needed, whatever
/// NVDA did in the meantime, and the rest of the time there is nothing of ours to collide
/// with. The always-available keys are `hotkeys`, which need no hook.
///
/// Installing waits (briefly) for the hook to be in, so a layer opened by a key press has
/// its hook before the user's next key arrives.
pub(crate) fn sync_os_hook() {
    if !os_hook_allowed() {
        return;
    }
    let want = LISTENER.has_work();
    let mut thread = HOOK_THREAD.lock();
    match (want, *thread) {
        (true, None) => {
            #[cfg(windows)]
            {
                *thread = win::install();
            }
        }
        (false, Some(tid)) => {
            *thread = None;
            #[cfg(windows)]
            win::uninstall(tid);
            #[cfg(not(windows))]
            let _ = tid;
            LISTENER.reset_tracking();
            SHIFT_IS_PRESSED.store(false, Ordering::Relaxed);
            ALTGR_IS_PRESSED.store(false, Ordering::Relaxed);
            IGNORE_NEXT_RIGHT_ALT.store(false, Ordering::Relaxed);
        }
        _ => {}
    }
}

fn process_loop() {
    let rx = QUEUE.1.clone();
    while let Ok(ev) = rx.recv() {
        if !LISTENER.process(&ev) {
            return;
        }
    }
}

// ---- process_key: the hook thread's state ------------------------------------------------

static SHIFT_IS_PRESSED: AtomicBool = AtomicBool::new(false);
static ALTGR_IS_PRESSED: AtomicBool = AtomicBool::new(false);
static IGNORE_NEXT_RIGHT_ALT: AtomicBool = AtomicBool::new(false);

/// `process_key(event_type, vk, scan_code, is_extended)` with the lock-key state already
/// read: names the event, tracks Shift and Alt Gr, and runs the listener. Returns whether
/// the key passes on, and the event to queue for the processing thread, if any.
pub(crate) fn process_key(
    listener: &Listener,
    event_type: KeyEventType,
    vk: u32,
    scan: u32,
    ext: u32,
    lock_mods: u8,
) -> (bool, Option<RawEvent>) {
    use super::names::{MOD_ALT_GR, MOD_SHIFT, event_name};
    // Pressing Alt Gr also generates an extra "right alt" event.
    if vk == 0xA5 && IGNORE_NEXT_RIGHT_ALT.load(Ordering::Relaxed) {
        IGNORE_NEXT_RIGHT_ALT.store(false, Ordering::Relaxed);
        return (true, None);
    }
    let mut mods = lock_mods;
    if SHIFT_IS_PRESSED.load(Ordering::Relaxed) {
        mods |= MOD_SHIFT;
    }
    if ALTGR_IS_PRESSED.load(Ordering::Relaxed) {
        mods |= MOD_ALT_GR;
    }
    let name = event_name(scan, vk, ext, mods);
    let down = event_type == KeyEventType::Down;
    if matches!(vk, 0x10 | 0xa0 | 0xa1) {
        SHIFT_IS_PRESSED.store(down, Ordering::Relaxed);
    }
    if scan == 541 && vk == 162 {
        IGNORE_NEXT_RIGHT_ALT.store(true, Ordering::Relaxed);
        ALTGR_IS_PRESSED.store(down, Ordering::Relaxed);
    }
    let ev = RawEvent {
        has_name: name.is_some(),
        event: KeyEvent {
            name: name.unwrap_or_default(),
            event_type,
        },
        scan_code: if scan != 0 { scan as i32 } else { -(vk as i32) },
    };
    let (pass, queue) = listener.direct_callback(&ev);
    (pass, queue.then_some(ev))
}

#[cfg(windows)]
mod win {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::time::Duration;

    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE,
        PeekMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
        WH_KEYBOARD_LL, WM_QUIT,
    };

    use super::super::KeyEventType;
    use super::super::names::{MOD_CAPS_LOCK, MOD_NUM_LOCK, MOD_SCROLL_LOCK};
    use super::{LISTENER, QUEUE, process_key};

    const VK_PACKET: u32 = 0xE7;
    const LLKHF_INJECTED: u32 = 0x10;
    const WM_KEYDOWN: usize = 0x0100;
    const WM_KEYUP: usize = 0x0101;
    const WM_SYSKEYDOWN: usize = 0x0104;
    const WM_SYSKEYUP: usize = 0x0105;

    fn toggled(vk: i32) -> bool {
        // SAFETY: GetKeyState takes a plain integer and cannot fault.
        (unsafe { GetKeyState(vk) } & 1) != 0
    }

    /// The body of `low_level_keyboard_handler`. Returns whether the key passes on.
    fn handle(vk: u32, scan: u32, flags: u32, wparam: usize) -> bool {
        // Ignore the second `alt` DOWN observed in some cases, and Unicode SendInput.
        let fake_alt = LLKHF_INJECTED | 0x20;
        if vk == VK_PACKET || flags & fake_alt == fake_alt {
            return true;
        }
        let event_type = match wparam {
            WM_KEYDOWN | WM_SYSKEYDOWN => KeyEventType::Down,
            WM_KEYUP | WM_SYSKEYUP => KeyEventType::Up,
            // A KeyError in the package, which its handler prints and passes the key on.
            _ => return true,
        };
        let ext = flags & 1;
        let mut lock_mods = 0u8;
        if toggled(0x90) {
            lock_mods |= MOD_NUM_LOCK;
        }
        if toggled(0x14) {
            lock_mods |= MOD_CAPS_LOCK;
        }
        if toggled(0x91) {
            lock_mods |= MOD_SCROLL_LOCK;
        }
        let (pass, queued) = process_key(&LISTENER, event_type, vk, scan, ext, lock_mods);
        if let Some(ev) = queued {
            let _ = QUEUE.0.send(ev);
        }
        pass
    }

    unsafe extern "system" fn ll_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: for WH_KEYBOARD_LL, lparam points to a KBDLLHOOKSTRUCT that is valid for
        // the duration of this call.
        let kb: KBDLLHOOKSTRUCT = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let pass = match catch_unwind(AssertUnwindSafe(|| {
            handle(kb.vkCode, kb.scanCode, kb.flags.0, wparam.0)
        })) {
            Ok(p) => p,
            Err(p) => {
                log::error!("Error in keyboard hook: {}", super::super::panic_text(&*p));
                true
            }
        };
        if !pass {
            return LRESULT(-1);
        }
        // SAFETY: passes the hook's own arguments on unchanged.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    /// How long [`install`] waits for the hook thread to report.
    const INSTALL_WAIT: Duration = Duration::from_millis(500);

    /// Start a thread that installs the hook and pumps its messages. Returns its id once
    /// the hook is in, or None if it failed or did not report in time.
    pub(super) fn install() -> Option<u32> {
        let (tx, rx) = crossbeam_channel::bounded::<Option<u32>>(1);
        crate::timer::spawn_daemon("keyboard-hook", move || listen(tx));
        match rx.recv_timeout(INSTALL_WAIT) {
            Ok(tid) => tid,
            Err(_) => {
                log::error!("The keyboard hook did not start in time.");
                None
            }
        }
    }

    /// End the hook thread `tid`; it removes its hook on the way out.
    pub(super) fn uninstall(tid: u32) {
        // SAFETY: a plain message post; a thread that already exited just fails it.
        if let Err(e) = unsafe { PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0)) } {
            log::warn!("Could not stop the keyboard hook thread: {e}");
        }
    }

    /// `_winkeyboard.listen`: install the hook on this thread, report, and pump its
    /// messages until WM_QUIT.
    fn listen(ready: crossbeam_channel::Sender<Option<u32>>) {
        let mut msg = MSG::default();
        // Create the message queue before reporting, so a quit posted at once is kept.
        // SAFETY: msg is a valid, owned MSG; PM_NOREMOVE leaves the queue as it is.
        let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
        // SAFETY: GetModuleHandleW(None) returns this process's module and cannot fault.
        let hmod = unsafe { GetModuleHandleW(None) }
            .ok()
            .map(|h| HINSTANCE(h.0));
        // SAFETY: ll_proc has the HOOKPROC signature and lives for the whole process.
        let hook = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_proc), hmod, 0) } {
            Ok(h) => h,
            Err(e) => {
                log::error!("Failed to install the keyboard hook: {e}");
                let _ = ready.send(None);
                return;
            }
        };
        // SAFETY: no arguments.
        let tid = unsafe { GetCurrentThreadId() };
        // Nobody waiting any more (install timed out): nothing tracks this thread, so it
        // must not stay installed.
        let abandoned = ready.send(Some(tid)).is_err();
        // The hook is serviced inside the blocking GetMessage.
        // SAFETY: msg is a valid, owned MSG for each call.
        while !abandoned && unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            // SAFETY: as above.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // SAFETY: the hook this thread installed.
        if let Err(e) = unsafe { UnhookWindowsHookEx(hook) } {
            log::warn!("Failed to remove the keyboard hook: {e}");
        }
    }
}
