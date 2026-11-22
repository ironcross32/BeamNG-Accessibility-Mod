//! `wx.MessageBox` as Win32 `MessageBoxW`, owned by the main window (modal to it).
//!
//! wx mapping: `wx.ICON_*` -> `MB_ICON*`; `wx.OK` / `wx.YES_NO` -> `MB_OK` / `MB_YESNO`;
//! `wx.NO_DEFAULT` -> `MB_DEFBUTTON2`, `wx.YES_DEFAULT` -> the first button.

use beam_engine::ui_sink::{Answer, Buttons, MessageBox, MessageKind};
use serde::Deserialize;

/// `message_box`'s argument, as the frontend sends it.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageBoxArgs {
    pub title: String,
    pub text: String,
    pub kind: String,
    pub buttons: String,
    pub default: String,
}

impl MessageBoxArgs {
    pub fn to_message_box(&self) -> MessageBox {
        let kind = match self.kind.as_str() {
            "warning" => MessageKind::Warning,
            "error" => MessageKind::Error,
            "question" => MessageKind::Question,
            _ => MessageKind::Info,
        };
        let buttons = if self.buttons == "yes_no" {
            Buttons::YesNo
        } else {
            Buttons::Ok
        };
        let default = match (buttons, self.default.as_str()) {
            (Buttons::Ok, _) => Answer::Ok,
            (Buttons::YesNo, "no") => Answer::No,
            (Buttons::YesNo, _) => Answer::Yes,
        };
        MessageBox {
            kind,
            title: self.title.clone(),
            text: self.text.clone(),
            buttons,
            default,
        }
    }
}

/// The answer as the frontend names it.
pub fn answer_name(a: Answer) -> &'static str {
    match a {
        Answer::Ok => "ok",
        Answer::Yes => "yes",
        Answer::No => "no",
    }
}

/// Show the box and wait for the answer. Blocks the calling thread; any thread may call
/// it (the owner is disabled while the box is up, as for a wx modal).
#[cfg(windows)]
pub fn show(owner: Option<isize>, msg: &MessageBox) -> Answer {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IDNO, IDOK, IDYES, MB_DEFBUTTON1, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONINFORMATION,
        MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_YESNO, MessageBoxW,
    };
    use windows::core::HSTRING;

    let icon = match msg.kind {
        MessageKind::Info => MB_ICONINFORMATION,
        MessageKind::Warning => MB_ICONWARNING,
        MessageKind::Error => MB_ICONERROR,
        MessageKind::Question => MB_ICONQUESTION,
    };
    let buttons = match msg.buttons {
        Buttons::Ok => MB_OK,
        Buttons::YesNo => MB_YESNO,
    };
    let def = if msg.buttons == Buttons::YesNo && msg.default == Answer::No {
        MB_DEFBUTTON2
    } else {
        MB_DEFBUTTON1
    };
    let hwnd = owner.map(|h| HWND(h as *mut core::ffi::c_void));
    let r = unsafe {
        MessageBoxW(
            hwnd,
            &HSTRING::from(msg.text.as_str()),
            &HSTRING::from(msg.title.as_str()),
            icon | buttons | def,
        )
    };
    if r == IDYES {
        Answer::Yes
    } else if r == IDNO {
        Answer::No
    } else if r == IDOK {
        Answer::Ok
    } else {
        // Closed without a button (or MessageBoxW failed): wx reports NO for a YES_NO
        // box and OK for an OK box.
        match msg.buttons {
            Buttons::YesNo => Answer::No,
            Buttons::Ok => Answer::Ok,
        }
    }
}

#[cfg(not(windows))]
pub fn show(_owner: Option<isize>, msg: &MessageBox) -> Answer {
    msg.default
}
