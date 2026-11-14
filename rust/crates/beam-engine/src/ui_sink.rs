//! Owner: EA. The window side of the engine, as a trait, so beam-engine never depends on
//! Tauri. `src-tauri` (U2) implements [`UiSink`] and installs it with [`set_ui_sink`];
//! headless runs and tests use [`NullSink`] or [`RecordingSink`].
//!
//! What it replaces in beamtel.py / updater.py / config_ui.py:
//!
//! | Python | UiSink |
//! |---|---|
//! | `WxLogHandler.emit` -> `wx.CallAfter(ctrl.AppendText)` | [`UiSink::append_log`] (event `log-line`) |
//! | `wx.CallAfter(console_frame.on_console_message, text)` | [`UiSink::console_message`] (U2 parses the record into `console-line` / `console-contexts`) |
//! | `wx.MessageBox(text, title, style, parent)` | [`UiSink::message_box`] |
//! | `wx.ProgressDialog` in `updater._run_with_progress` (`Update`, `Pulse`, `Destroy`) | [`UiSink::progress_update`], [`UiSink::progress_close`] (events `update-progress`, `update-progress-close`) |
//! | the frame passed as `parent` (window handle for owned dialogs, `_find_game_window`'s "not us") | [`UiSink::window_handle`] |
//! | `frame.Close()` after an update hands over to the helper | [`UiSink::request_exit`] |
//!
//! Every method may be called from any thread; implementations marshal to the UI thread.
//! `message_box` and `progress_update` block the calling thread until the UI answers, as
//! the modal wx calls did.

use std::sync::{Arc, LazyLock};

use parking_lot::{Mutex, RwLock};

/// The icon, i.e. `wx.ICON_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Warning,
    Error,
    Question,
}

/// The buttons, i.e. `wx.OK` / `wx.YES_NO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Buttons {
    Ok,
    YesNo,
}

/// The answer (`wx.OK`, `wx.YES`, `wx.NO`; closing the box is `No` for YES_NO, `Ok` for OK).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Ok,
    Yes,
    No,
}

/// One `wx.MessageBox` call. `default` is the focused button (`wx.YES_DEFAULT` /
/// `wx.NO_DEFAULT`; `Ok` for an OK box).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageBox {
    pub kind: MessageKind,
    pub title: String,
    pub text: String,
    pub buttons: Buttons,
    pub default: Answer,
}

/// One tick of a progress dialog. `fraction` is `None` for `Pulse` (indeterminate);
/// otherwise 0.0..=1.0 (the Python's `pct / 100`).
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub title: String,
    pub text: String,
    pub fraction: Option<f64>,
    pub cancellable: bool,
}

pub trait UiSink: Send + Sync {
    /// Append one formatted application-log line (`"%(asctime)s [%(levelname)s] %(message)s"`).
    fn append_log(&self, line: &str);
    /// Deliver one raw record from `consoleAccessible.lua` (the UDP datagram text).
    fn console_message(&self, record: &str);
    /// Show a modal message box and wait for the answer.
    fn message_box(&self, msg: &MessageBox) -> Answer;
    /// Show or update the progress dialog. Returns false once the user has cancelled
    /// (wx's `keep`); always true for a non-cancellable dialog.
    fn progress_update(&self, progress: &Progress) -> bool;
    /// Close the progress dialog (`dlg.Destroy()`).
    fn progress_close(&self);
    /// The main window's HWND, if there is one.
    fn window_handle(&self) -> Option<isize>;
    /// Close the application window (runs the normal close sequence).
    fn request_exit(&self);
}

/// Does nothing; message boxes answer with their default button.
#[derive(Debug, Default)]
pub struct NullSink;

impl UiSink for NullSink {
    fn append_log(&self, _line: &str) {}
    fn console_message(&self, _record: &str) {}
    fn message_box(&self, msg: &MessageBox) -> Answer {
        msg.default
    }
    fn progress_update(&self, _progress: &Progress) -> bool {
        true
    }
    fn progress_close(&self) {}
    fn window_handle(&self) -> Option<isize> {
        None
    }
    fn request_exit(&self) {}
}

/// Everything a [`RecordingSink`] saw, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    Log(String),
    Console(String),
    MessageBox(MessageBox),
    Progress(Progress),
    ProgressClose,
    RequestExit,
}

/// Records every call; message boxes answer from a scripted queue, else their default.
#[derive(Debug, Default)]
pub struct RecordingSink {
    pub events: Mutex<Vec<UiEvent>>,
    pub answers: Mutex<std::collections::VecDeque<Answer>>,
    /// When set, `progress_update` reports a cancel.
    pub cancel_progress: std::sync::atomic::AtomicBool,
}

impl RecordingSink {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push_answer(&self, a: Answer) {
        self.answers.lock().push_back(a);
    }
    pub fn events(&self) -> Vec<UiEvent> {
        self.events.lock().clone()
    }
}

impl UiSink for RecordingSink {
    fn append_log(&self, line: &str) {
        self.events.lock().push(UiEvent::Log(line.to_string()));
    }
    fn console_message(&self, record: &str) {
        self.events.lock().push(UiEvent::Console(record.to_string()));
    }
    fn message_box(&self, msg: &MessageBox) -> Answer {
        self.events.lock().push(UiEvent::MessageBox(msg.clone()));
        self.answers.lock().pop_front().unwrap_or(msg.default)
    }
    fn progress_update(&self, progress: &Progress) -> bool {
        self.events.lock().push(UiEvent::Progress(progress.clone()));
        !self
            .cancel_progress
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    fn progress_close(&self) {
        self.events.lock().push(UiEvent::ProgressClose);
    }
    fn window_handle(&self) -> Option<isize> {
        None
    }
    fn request_exit(&self) {
        self.events.lock().push(UiEvent::RequestExit);
    }
}

static SINK: LazyLock<RwLock<Arc<dyn UiSink>>> =
    LazyLock::new(|| RwLock::new(Arc::new(NullSink)));

/// Install the window's sink (U2, at setup). Replaces the previous one.
pub fn set_ui_sink(sink: Arc<dyn UiSink>) {
    *SINK.write() = sink;
}

/// The installed sink (a [`NullSink`] until one is set, and again after the window
/// closes: the Python sets `console_frame = None` first thing in `_on_close`).
pub fn ui_sink() -> Arc<dyn UiSink> {
    SINK.read().clone()
}

/// `console_frame = None`: drop the window's sink so late records go nowhere.
pub fn clear_ui_sink() {
    *SINK.write() = Arc::new(NullSink);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_sink_scripts_answers() {
        let s = RecordingSink::new();
        s.push_answer(Answer::No);
        let q = MessageBox {
            kind: MessageKind::Question,
            title: "T".into(),
            text: "x".into(),
            buttons: Buttons::YesNo,
            default: Answer::Yes,
        };
        assert_eq!(s.message_box(&q), Answer::No);
        assert_eq!(s.message_box(&q), Answer::Yes);
        assert_eq!(NullSink.message_box(&q), Answer::Yes);
        assert_eq!(s.events().len(), 2);
    }
}
