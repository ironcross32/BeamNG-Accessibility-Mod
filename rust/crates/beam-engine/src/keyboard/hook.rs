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

#[derive(Default)]
struct Registry {
    blocking_hooks: Vec<(u64, BlockingFn)>,
    blocking_keys: HashMap<i32, Vec<(u64, BlockingFn)>>,
    handlers: Vec<(u64, HandlerFn)>,
    nonblocking_hotkeys: HashMap<Vec<i32>, Vec<(u64, HotkeyFn)>>,
}

/// `keyboard._listener`.
pub(crate) struct Listener {
    reg: RwLock<Registry>,
    /// `_pressed_events`, by scan code.
    pressed: Mutex<BTreeSet<i32>>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

impl Listener {
    pub(crate) fn new() -> Listener {
        Listener {
            reg: RwLock::new(Registry::default()),
            pressed: Mutex::new(BTreeSet::new()),
        }
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
        {
            let mut pressed = self.pressed.lock();
            match ev.event.event_type {
                KeyEventType::Down => {
                    pressed.insert(ev.scan_code);
                }
                KeyEventType::Up => {
                    pressed.remove(&ev.scan_code);
                }
            }
        }
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
        (true, true)
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
}

/// The process-wide listener.
pub(crate) static LISTENER: LazyLock<Listener> = LazyLock::new(Listener::new);

static QUEUE: LazyLock<(Sender<RawEvent>, Receiver<RawEvent>)> = LazyLock::new(unbounded);

static LISTENING: Mutex<bool> = Mutex::new(false);
/// Whether a real system-wide hook may be installed (set once the engine starts the
/// command worker or installs the hotkeys; never in a test that only drives handlers).
static OS_HOOK_ALLOWED: AtomicBool = AtomicBool::new(false);
static OS_HOOK_STARTED: AtomicBool = AtomicBool::new(false);

/// Allow the OS hook and install it if the listener is already running.
pub(crate) fn allow_os_hook() {
    OS_HOOK_ALLOWED.store(true, Ordering::SeqCst);
    if *LISTENING.lock() {
        start_os_hook();
    }
}

/// `_listener.start_if_necessary()`: build the name tables, start the processing thread,
/// and the hook thread.
pub(crate) fn start_if_necessary() {
    let mut listening = LISTENING.lock();
    if !*listening {
        super::names::setup_name_tables();
        *listening = true;
        crate::timer::spawn_daemon("keyboard-process", process_loop);
    }
    drop(listening);
    if OS_HOOK_ALLOWED.load(Ordering::SeqCst) {
        start_os_hook();
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

fn start_os_hook() {
    if OS_HOOK_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    #[cfg(windows)]
    crate::timer::spawn_daemon("keyboard-hook", win::listen);
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

    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, MSG, SetWindowsHookExW,
        TranslateMessage, WH_KEYBOARD_LL,
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

    /// `_winkeyboard.listen`: install the hook on this thread and pump its messages.
    pub(super) fn listen() {
        // SAFETY: GetModuleHandleW(None) returns this process's module and cannot fault.
        let hmod = unsafe { GetModuleHandleW(None) }
            .ok()
            .map(|h| HINSTANCE(h.0));
        // SAFETY: ll_proc has the HOOKPROC signature and lives for the whole process.
        if let Err(e) = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_proc), hmod, 0) } {
            log::error!("Failed to install the keyboard hook: {e}");
            return;
        }
        let mut msg = MSG::default();
        // `while not GetMessage(msg, 0, 0, 0)`, as the package writes it: the hook is
        // serviced inside the first, blocking GetMessage, and the loop would end the
        // thread if a message were ever posted to it (nothing posts one).
        // SAFETY: msg is a valid, owned MSG for each call.
        while !unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
            // SAFETY: as above.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
