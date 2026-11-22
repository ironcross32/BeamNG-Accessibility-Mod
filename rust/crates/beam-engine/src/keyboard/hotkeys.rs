//! The always-available keys (F9, F10, F11) as `RegisterHotKey` hotkeys, registered only
//! while the game's window is in front.
//!
//! These used to be hotkeys on the low-level hook, which kept a `WH_KEYBOARD_LL` hook
//! installed for the life of the app. That hook and NVDA's own fight over their order in
//! the chain: whichever was installed second worked and the other broke. With beamtel
//! started after NVDA, NVDA stopped interrupting itself and NVDA+Space did nothing in this
//! app's window (quitting beamtel fixed it at once); with NVDA restarted afterwards, F9
//! went silent. A registered hotkey needs no hook, so nothing of ours sits in front of
//! NVDA until a layer actually takes the keyboard (see `hook::sync_os_hook`).
//!
//! Registering only while the game is foreground keeps the keys working normally
//! everywhere else: a registered hotkey is eaten system-wide, and F11 used to be eaten in
//! every program. Foreground changes arrive as `EVENT_SYSTEM_FOREGROUND` on this module's
//! own thread, which also receives `WM_HOTKEY`. A callback runs on that thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use parking_lot::Mutex;

/// A registered key's callback.
pub(crate) type HotkeyCb = Arc<dyn Fn() + Send + Sync>;

struct Entry {
    id: i32,
    name: String,
    vk: u32,
    cb: HotkeyCb,
}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicI32 = AtomicI32::new(1);

/// The virtual-key code for a key name this module accepts (function keys only).
fn vk_for(name: &str) -> Option<u32> {
    let n: u32 = name.strip_prefix('f')?.parse().ok()?;
    (1..=24).contains(&n).then(|| 0x70 + n - 1)
}

/// Register `name` (an unmodified function key, "f9") to run `cb` when pressed in the
/// game, for the life of the process. Returns its id.
pub(crate) fn register(name: &str, cb: HotkeyCb) -> Result<i32, String> {
    let vk = vk_for(name).ok_or_else(|| format!("not a function key: {name:?}"))?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    ENTRIES.lock().push(Entry {
        id,
        name: name.to_string(),
        vk,
        cb,
    });
    imp::resync();
    Ok(id)
}

/// Bring Windows in line with the registered keys (the engine calls this once OS hooks
/// are allowed, for keys registered before then).
pub(crate) fn resync() {
    imp::resync();
}

fn callback_for_id(id: i32) -> Option<HotkeyCb> {
    ENTRIES
        .lock()
        .iter()
        .find(|e| e.id == id)
        .map(|e| Arc::clone(&e.cb))
}

/// Run the callbacks registered for `name`, as a press in the game would. Returns
/// whether any was registered. For tests; nothing is registered with Windows.
pub(crate) fn fire_for_tests(name: &str) -> bool {
    let cbs: Vec<HotkeyCb> = ENTRIES
        .lock()
        .iter()
        .filter(|e| e.name == name)
        .map(|e| Arc::clone(&e.cb))
        .collect();
    for cb in &cbs {
        cb();
    }
    !cbs.is_empty()
}

#[cfg(windows)]
mod imp {
    use std::collections::HashSet;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW,
        PostThreadMessageW, TranslateMessage, WINEVENT_OUTOFCONTEXT, WM_APP, WM_HOTKEY,
    };

    use super::{ENTRIES, callback_for_id};

    const WM_RESYNC: u32 = WM_APP + 1;

    /// The hotkey thread's id, once its message queue exists.
    static THREAD_ID: AtomicU32 = AtomicU32::new(0);
    static STARTED: AtomicBool = AtomicBool::new(false);

    thread_local! {
        /// The ids currently registered with Windows (hotkey thread only).
        static REGISTERED: std::cell::RefCell<HashSet<i32>> =
            std::cell::RefCell::new(HashSet::new());
    }

    pub(super) fn resync() {
        // Only once the engine has started: a test registers keys but never with Windows.
        if !super::super::hook::os_hook_allowed() {
            return;
        }
        if !STARTED.swap(true, Ordering::SeqCst) {
            crate::timer::spawn_daemon("keyboard-hotkeys", run);
            return; // the thread syncs once its queue is up
        }
        let tid = THREAD_ID.load(Ordering::SeqCst);
        if tid != 0 {
            // SAFETY: plain message post to our own thread.
            let _ = unsafe { PostThreadMessageW(tid, WM_RESYNC, WPARAM(0), LPARAM(0)) };
        }
    }

    /// Register every entry while the game is in front, and nothing otherwise.
    fn sync() {
        let focused = crate::win_util::is_beamng_focused();
        let wanted: Vec<(i32, u32, String)> = if focused {
            ENTRIES
                .lock()
                .iter()
                .map(|e| (e.id, e.vk, e.name.clone()))
                .collect()
        } else {
            Vec::new()
        };
        REGISTERED.with_borrow_mut(|reg| {
            let keep: HashSet<i32> = wanted.iter().map(|w| w.0).collect();
            for id in reg
                .iter()
                .copied()
                .filter(|id| !keep.contains(id))
                .collect::<Vec<_>>()
            {
                // SAFETY: unregisters a thread hotkey this thread registered.
                let _ = unsafe { UnregisterHotKey(None, id) };
                reg.remove(&id);
            }
            for (id, vk, name) in wanted {
                if reg.contains(&id) {
                    continue;
                }
                // SAFETY: a thread hotkey (no window); plain integers.
                match unsafe { RegisterHotKey(None, id, HOT_KEY_MODIFIERS(MOD_NOREPEAT.0), vk) } {
                    Ok(()) => {
                        reg.insert(id);
                    }
                    Err(e) => log::warn!("Could not register {name} as a hotkey: {e}"),
                }
            }
        });
    }

    unsafe extern "system" fn on_foreground(
        _hook: HWINEVENTHOOK,
        _event: u32,
        _hwnd: HWND,
        _id_object: i32,
        _id_child: i32,
        _thread: u32,
        _time: u32,
    ) {
        if let Err(p) = catch_unwind(sync) {
            log::error!("Hotkey sync raised: {}", super::super::panic_text(&*p));
        }
    }

    fn run() {
        let mut msg = MSG::default();
        // SAFETY: msg is a valid, owned MSG; PM_NOREMOVE only creates the queue.
        let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
        // SAFETY: no arguments.
        THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
        // SAFETY: an out-of-context hook delivered to this thread's message loop;
        // on_foreground has the WINEVENTPROC signature and lives for the process.
        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_foreground),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if hook.is_invalid() {
            log::error!("Could not watch the foreground window; F9, F10 and F11 are off.");
            return;
        }
        sync();
        // SAFETY: msg is a valid, owned MSG for each call.
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            match msg.message {
                WM_HOTKEY => {
                    if let Some(cb) = callback_for_id(msg.wParam.0 as i32)
                        && let Err(p) = catch_unwind(AssertUnwindSafe(|| cb()))
                    {
                        log::error!("Hotkey handler raised: {}", super::super::panic_text(&*p));
                    }
                }
                WM_RESYNC => sync(),
                _ => {
                    // SAFETY: as above.
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn resync() {}
}

#[cfg(test)]
mod tests {
    use super::vk_for;

    #[test]
    fn function_key_names_map_to_their_virtual_keys() {
        assert_eq!(vk_for("f1"), Some(0x70));
        assert_eq!(vk_for("f9"), Some(0x78));
        assert_eq!(vk_for("f10"), Some(0x79));
        assert_eq!(vk_for("f11"), Some(0x7A));
        assert_eq!(vk_for("f24"), Some(0x87));
        assert_eq!(vk_for("f0"), None);
        assert_eq!(vk_for("f25"), None);
        assert_eq!(vk_for("a"), None);
        assert_eq!(vk_for("ctrl+f9"), None);
    }
}
