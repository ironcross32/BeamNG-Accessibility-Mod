//! The native View Logs menu (`BeamTelFrame._log_menu_entries`, `_on_view_logs`,
//! `_open_file_if_exists`, `_on_open_log_folder`).
//!
//! A real Win32 popup menu rather than an HTML one: Windows announces a menu with no
//! framing, a disabled item is still arrowed onto and read, and Right / Left Arrow open
//! and close a submenu natively. Nothing here handles a key.

use std::path::{Path, PathBuf};

use beam_engine::ui_sink::{Answer, Buttons, MessageBox, MessageKind};

/// One top-level row: a file, or a submenu of `(label, file)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    File(PathBuf),
    Sub(Vec<(String, PathBuf)>),
}

/// What the user picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    File(PathBuf),
    Folder,
}

/// The paths the menu lists, resolved at call time.
pub struct LogPaths {
    pub app_log: PathBuf,
    pub speech_log: PathBuf,
    pub ai_log: PathBuf,
    pub dom_dump: PathBuf,
    pub scanner_diag: PathBuf,
    pub update_helper_log: PathBuf,
}

impl LogPaths {
    pub fn current() -> Self {
        LogPaths {
            app_log: beam_base::logger::log_filename(),
            speech_log: beam_engine::speech_out::speech_log_path(),
            ai_log: beam_engine::ai_describer::log_path(),
            dom_dump: beam_engine::engine::dom_dump_path(),
            scanner_diag: beam_audio::scanner_hrtf_diag::default_path(),
            update_helper_log: beam_engine::updater::update_dir()
                .join(beam_engine::updater::HELPER_LOG),
        }
    }
}

/// `"%s.%d" % (LOG_FILENAME, n)`.
fn rolled_path(log: &Path, n: u32) -> PathBuf {
    let mut s = log.as_os_str().to_owned();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

/// `_log_menu_entries()`: every log BEAM writes, in menu order. A missing file is still
/// listed (the menu disables it); the rotated application-log backups only when present,
/// and they put the application log in a submenu with "Current" first.
pub fn entries(p: &LogPaths) -> Vec<(String, Target)> {
    let rolled: Vec<(String, PathBuf)> = (1..=3)
        .map(|n| (format!("Previous {n}"), rolled_path(&p.app_log, n)))
        .filter(|(_, path)| path.is_file())
        .collect();
    let mut out = Vec::new();
    if rolled.is_empty() {
        out.push((
            "Application Log".to_string(),
            Target::File(p.app_log.clone()),
        ));
    } else {
        let mut sub = vec![("Current".to_string(), p.app_log.clone())];
        sub.extend(rolled);
        out.push(("Application Log".to_string(), Target::Sub(sub)));
    }
    out.push(("Speech Log".into(), Target::File(p.speech_log.clone())));
    out.push(("AI Description Log".into(), Target::File(p.ai_log.clone())));
    out.push(("DOM Dump".into(), Target::File(p.dom_dump.clone())));
    out.push((
        "Scanner and HRTF Diagnostic".into(),
        Target::File(p.scanner_diag.clone()),
    ));
    out.push((
        "Update Helper Log".into(),
        Target::File(p.update_helper_log.clone()),
    ));
    out
}

/// A file item's label: `"<label> (not created yet)"` when it does not exist.
pub fn item_label(label: &str, path: &Path) -> (String, bool) {
    if path.is_file() {
        (label.to_string(), true)
    } else {
        (format!("{label} (not created yet)"), false)
    }
}

/// `os.startfile(path)`; `Err` is the OSError text.
pub fn shell_open(path: &Path) -> Result<(), String> {
    imp::shell_open(path)
}

/// Act on a choice. Message boxes go through `show_box` (owned by the window).
pub fn handle_choice(choice: Choice, show_box: &dyn Fn(&MessageBox) -> Answer) {
    match choice {
        Choice::File(path) => {
            if path.is_file() {
                if let Err(e) = shell_open(&path) {
                    log::error!("Could not open {}: {e}", path.display());
                }
            } else {
                show_box(&MessageBox {
                    kind: MessageKind::Warning,
                    title: "Not Found".into(),
                    text: format!("File not found:\n{}", path.display()),
                    buttons: Buttons::Ok,
                    default: Answer::Ok,
                });
            }
        }
        Choice::Folder => {
            let dir = beam_base::paths::config_dir();
            if let Err(e) = shell_open(&dir) {
                log::error!("Could not open the log folder: {e}");
                show_box(&MessageBox {
                    kind: MessageKind::Error,
                    title: "Open Failed".into(),
                    text: format!("The log folder could not be opened.\n\n{e}"),
                    buttons: Buttons::Ok,
                    default: Answer::Ok,
                });
            }
        }
    }
}

/// Show the menu with its top-left at client position `(x, y)` (physical pixels) of
/// `hwnd` and wait for a choice. Must run on the window's own thread.
pub fn popup(hwnd: isize, x: i32, y: i32, rows: &[(String, Target)]) -> Option<Choice> {
    imp::popup(hwnd, x, y, rows)
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, HMENU, MF_GRAYED, MF_POPUP, MF_SEPARATOR,
        MF_STRING, SW_SHOWNORMAL, SetForegroundWindow, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN,
        TrackPopupMenuEx,
    };
    use windows::core::{HSTRING, PCWSTR};

    use super::{Choice, Target, item_label};

    pub fn shell_open(path: &Path) -> Result<(), String> {
        let r = unsafe {
            ShellExecuteW(
                None,
                &HSTRING::from("open"),
                &HSTRING::from(path.as_os_str()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        let code = r.0 as isize;
        if code > 32 {
            return Ok(());
        }
        let e = std::io::Error::from_raw_os_error(code as i32);
        let msg = e.to_string();
        let msg = msg
            .split(" (os error")
            .next()
            .unwrap_or(&msg)
            .trim_end_matches(['.', ' '])
            .to_string();
        Err(format!(
            "[WinError {code}] {msg}: {}",
            beam_base::pyval::py_str_repr(&path.to_string_lossy())
        ))
    }

    fn add_file(menu: HMENU, label: &str, path: &Path, id: usize) -> bool {
        let (text, enabled) = item_label(label, path);
        let flags = if enabled {
            MF_STRING
        } else {
            MF_STRING | MF_GRAYED
        };
        unsafe { AppendMenuW(menu, flags, id, &HSTRING::from(text)) }.is_ok()
    }

    pub fn popup(hwnd: isize, x: i32, y: i32, rows: &[(String, Target)]) -> Option<Choice> {
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        let menu = unsafe { CreatePopupMenu() }.ok()?;
        let mut files = Vec::new();
        for (label, target) in rows {
            match target {
                Target::File(path) => {
                    files.push(path.clone());
                    add_file(menu, label, path, files.len());
                }
                Target::Sub(items) => {
                    let Ok(sub) = (unsafe { CreatePopupMenu() }) else {
                        continue;
                    };
                    for (sub_label, path) in items {
                        files.push(path.clone());
                        add_file(sub, sub_label, path, files.len());
                    }
                    let _ = unsafe {
                        AppendMenuW(
                            menu,
                            MF_POPUP,
                            sub.0 as usize,
                            &HSTRING::from(label.as_str()),
                        )
                    };
                }
            }
        }
        let folder_id = files.len() + 1;
        unsafe {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                folder_id,
                &HSTRING::from("Open Log Folder"),
            );
        }
        let mut pt = POINT { x, y };
        let cmd = unsafe {
            let _ = ClientToScreen(hwnd, &mut pt);
            // Without the window in front, the menu would not close when focus leaves it.
            let _ = SetForegroundWindow(hwnd);
            let r = TrackPopupMenuEx(
                menu,
                (TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN).0,
                pt.x,
                pt.y,
                hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            r.0 as usize
        };
        match cmd {
            0 => None,
            c if c == folder_id => Some(Choice::Folder),
            c => files.get(c - 1).cloned().map(Choice::File),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    use super::{Choice, Target};

    pub fn shell_open(_path: &Path) -> Result<(), String> {
        Err("not supported on this platform".into())
    }

    pub fn popup(_hwnd: isize, _x: i32, _y: i32, _rows: &[(String, Target)]) -> Option<Choice> {
        None
    }
}
