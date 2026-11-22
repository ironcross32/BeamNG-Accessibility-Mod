//! The window's state: what `BeamTelFrame` (or the configurator's frame) held, the event
//! buffering CONTRACT.md requires before `frontend_ready`, the `UiSink` the engine talks
//! to, and the close sequence.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use beam_base::logger::{LogSink, SinkId};
use beam_base::pyval::PyDict;
use beam_engine::ui_sink::{Answer, MessageBox, Progress, UiSink};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};

use crate::console_view::{Action, ConsoleView};

/// How long a close waits for the page's `close_ready` before closing anyway (a crashed
/// page would otherwise make the window impossible to close). Not in the Python.
pub const CLOSE_READY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Main,
    Configurator,
}

impl Mode {
    /// `--configurator` on the command line selects the standalone configurator.
    pub fn from_args<I: IntoIterator<Item = String>>(args: I) -> Mode {
        if args.into_iter().any(|a| a == "--configurator") {
            Mode::Configurator
        } else {
            Mode::Main
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Main => "main",
            Mode::Configurator => "configurator",
        }
    }

    /// The window title (`BeamTelFrame.__init__` / `configurator.main`).
    pub fn title(self) -> String {
        match self {
            Mode::Main => format!("BeamNG Accessibility {}", beam_engine::updater::APP_VERSION),
            Mode::Configurator => "BeamTel Configurator".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextItem {
    pub index: i64,
    pub label: String,
}

/// What arrives before the page has registered its listeners.
#[derive(Debug, Default)]
struct Buffered {
    ready: bool,
    log_lines: Vec<String>,
    console_lines: Vec<String>,
    console_contexts: Option<Vec<ContextItem>>,
}

pub struct Shell {
    pub mode: Mode,
    app: OnceLock<AppHandle>,
    hwnd: AtomicIsize,
    buffered: Mutex<Buffered>,
    /// `_shutting_down`: no more `log-line` / `console-*` events.
    shutting_down: AtomicBool,
    /// `close-requested` has been emitted.
    closing: AtomicBool,
    /// The close has gone past `close_ready` (or its timeout).
    finished: AtomicBool,
    update_flow_started: AtomicBool,
    console: Mutex<ConsoleView>,
    progress_cancel: AtomicBool,
    log_sink: Mutex<Option<SinkId>>,
    pub engine: Mutex<Option<JoinHandle<()>>>,
    /// `ConfigPanel.cur_cfg`.
    pub panel: Mutex<PyDict>,
    /// `AIDescriberPanel.cur_cfg`.
    pub describer: Mutex<PyDict>,
    /// `_console_history`.
    pub history: Mutex<Vec<String>>,
}

impl Shell {
    pub fn new(mode: Mode) -> Arc<Shell> {
        let history = if mode == Mode::Main {
            beam_engine::console::load_console_history()
        } else {
            Vec::new()
        };
        Arc::new(Shell {
            mode,
            app: OnceLock::new(),
            hwnd: AtomicIsize::new(0),
            buffered: Mutex::new(Buffered::default()),
            shutting_down: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            update_flow_started: AtomicBool::new(false),
            console: Mutex::new(ConsoleView::new()),
            progress_cancel: AtomicBool::new(false),
            log_sink: Mutex::new(None),
            engine: Mutex::new(None),
            panel: Mutex::new(PyDict::new()),
            describer: Mutex::new(PyDict::new()),
            history: Mutex::new(history),
        })
    }

    /// Called from `setup` once the window exists.
    pub fn attach(&self, app: AppHandle, hwnd: Option<isize>) {
        let _ = self.app.set(app);
        self.hwnd.store(hwnd.unwrap_or(0), Ordering::SeqCst);
    }

    pub fn hwnd(&self) -> Option<isize> {
        match self.hwnd.load(Ordering::SeqCst) {
            0 => None,
            h => Some(h),
        }
    }

    pub fn app(&self) -> Option<&AppHandle> {
        self.app.get()
    }

    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) {
        if let Some(app) = self.app.get() {
            if let Err(e) = app.emit(event, payload) {
                log::warn!("could not emit {event}: {e}");
            }
        }
    }

    /// Install the log pane's copy of the application log (`WxLogHandler`). Main mode.
    pub fn install_log_sink(self: &Arc<Self>) {
        let id = beam_base::logger::add_sink(Arc::new(PaneLog(Arc::clone(self))));
        *self.log_sink.lock() = Some(id);
    }

    #[doc(hidden)]
    pub fn push_log(&self, line: &str) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let mut b = self.buffered.lock();
        if b.ready {
            self.emit("log-line", json!({"text": line}));
        } else {
            b.log_lines.push(line.to_string());
        }
    }

    fn push_console(&self, action: Action) {
        let mut b = self.buffered.lock();
        match action {
            Action::Line(text) => {
                if b.ready {
                    self.emit("console-line", json!({"text": text}));
                } else {
                    b.console_lines.push(text);
                }
            }
            Action::Contexts(items) => {
                let items: Vec<ContextItem> = items
                    .into_iter()
                    .map(|(index, label)| ContextItem { index, label })
                    .collect();
                if b.ready {
                    self.emit("console-contexts", json!({"items": items}));
                } else {
                    b.console_contexts = Some(items);
                }
            }
            Action::Speak(text) => {
                drop(b);
                beam_engine::speech_out::say_plain(&text);
            }
        }
    }

    /// `frontend_ready`: hand over the buffer, switch to events, and in main mode start the
    /// startup update flow (once).
    pub fn frontend_ready(self: &Arc<Self>) -> Value {
        let out = {
            let mut b = self.buffered.lock();
            b.ready = true;
            json!({
                "log_lines": std::mem::take(&mut b.log_lines),
                "console_lines": std::mem::take(&mut b.console_lines),
                "console_contexts": b.console_contexts.take(),
            })
        };
        if self.mode == Mode::Main && !self.update_flow_started.swap(true, Ordering::SeqCst) {
            let me = Arc::clone(self);
            // The flow is modal and blocks: it runs on its own thread so the page keeps
            // processing the progress events it drives.
            let spawned = std::thread::Builder::new()
                .name("beamtel-update-flow".into())
                .spawn(move || beam_engine::engine::run_update_flow(&*me));
            if let Err(e) = spawned {
                log::error!("Update check failed to run: {e}");
                beam_engine::updater::Gate::allow(&beam_engine::engine::LaunchGate);
            }
        }
        out
    }

    /// Step 1 of the close (`_on_close` up to the panel flush): stop the event streams and
    /// ask the page to flush. Every way of closing comes here.
    pub fn begin_close(self: &Arc<Self>) {
        if self.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shutting_down.store(true, Ordering::SeqCst);
        if let Some(id) = self.log_sink.lock().take() {
            beam_base::logger::remove_sink(id);
        }
        // console_frame = None
        beam_engine::ui_sink::clear_ui_sink();
        self.emit("close-requested", json!({}));
        let me = Arc::clone(self);
        let _ = std::thread::Builder::new()
            .name("beamtel-close-watchdog".into())
            .spawn(move || {
                std::thread::sleep(CLOSE_READY_TIMEOUT);
                if !me.finished.load(Ordering::SeqCst) {
                    log::warn!("The page did not confirm its saves; closing anyway.");
                    me.finish_close();
                }
            });
    }

    /// Step 4 (`close_ready`): stop the engine, join it for at most 2 s, destroy the window.
    /// The event loop then ends and `run` finishes the rest of `main()`.
    pub fn finish_close(self: &Arc<Self>) {
        if self.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = Arc::clone(self);
        let _ = std::thread::Builder::new()
            .name("beamtel-close".into())
            .spawn(move || {
                if me.mode == Mode::Main {
                    let engine = me.engine.lock().take();
                    beam_engine::engine::close_sequence(engine);
                }
                if let Some(w) = me.app.get().and_then(|a| a.get_webview_window("main")) {
                    if let Err(e) = w.destroy() {
                        log::error!("could not destroy the window: {e}");
                        if let Some(a) = me.app.get() {
                            a.exit(0);
                        }
                    }
                } else if let Some(a) = me.app.get() {
                    a.exit(0);
                }
            });
    }

    /// Log lines waiting for `frontend_ready`.
    #[doc(hidden)]
    pub fn buffered_log_count(&self) -> usize {
        self.buffered.lock().log_lines.len()
    }

    pub fn cancel_progress(&self) {
        self.progress_cancel.store(true, Ordering::SeqCst);
    }
}

/// `WxLogHandler`: the Main tab's log pane.
struct PaneLog(Arc<Shell>);

impl LogSink for PaneLog {
    fn emit(&self, _level: log::Level, line: &str) {
        self.0.push_log(line);
    }
}

impl UiSink for Shell {
    fn append_log(&self, line: &str) {
        self.push_log(line);
    }

    fn console_message(&self, record: &str) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let actions = self.console.lock().handle(record);
        for a in actions {
            self.push_console(a);
        }
    }

    fn message_box(&self, msg: &MessageBox) -> Answer {
        crate::dialogs::show(self.hwnd(), msg)
    }

    fn progress_update(&self, progress: &Progress) -> bool {
        self.emit(
            "update-progress",
            json!({
                "title": progress.title,
                "text": progress.text,
                "fraction": progress.fraction,
                "cancellable": progress.cancellable,
            }),
        );
        !(progress.cancellable && self.progress_cancel.load(Ordering::SeqCst))
    }

    fn progress_close(&self) {
        self.emit("update-progress-close", json!({}));
        self.progress_cancel.store(false, Ordering::SeqCst);
    }

    fn window_handle(&self) -> Option<isize> {
        self.hwnd()
    }

    fn request_exit(&self) {
        // `frame.Close(True)`: the same path as the close button. The sink is only ever
        // installed as the Arc the app manages, so recover it from there.
        if let Some(app) = self.app.get() {
            let shell = app.state::<Arc<Shell>>();
            shell.begin_close();
        }
    }
}
