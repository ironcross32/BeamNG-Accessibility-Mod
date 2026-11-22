//! Rotating log file (same name, size and backup count), from bnh_logger.py.
//!
//! `%LOCALAPPDATA%\beamtel\bnvdahook.log`, rolled at 1,024,000 bytes into `.1`..`.3`,
//! each line `YYYY-MM-DD HH:MM:SS [LEVEL] message` in local time, CRLF line endings
//! (Python's text-mode file on Windows). Records arrive through the `log` facade;
//! [`add_sink`] lets another crate take a copy of every formatted line (the GUI log
//! pane, tests).

use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

/// The log file's name in [`log_dir`].
pub const LOG_BASENAME: &str = "bnvdahook.log";
/// `maxBytes` of the Python RotatingFileHandler.
pub const MAX_BYTES: u64 = 1_024_000;
/// `backupCount` of the Python RotatingFileHandler.
pub const BACKUP_COUNT: u32 = 3;
/// The Python logger's name. Records whose `log` target starts with this, or with
/// `beam` (every crate in the workspace), are logged; dependencies' records are not,
/// as Python only logs its own `bnvdahook` logger tree.
pub const LOGGER_NAME: &str = "bnvdahook";

/// `%LOCALAPPDATA%\beamtel` (home fallback), created if missing.
pub fn log_dir() -> PathBuf {
    let dir = crate::paths::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// `LOG_FILENAME`: the current log file.
pub fn log_filename() -> PathBuf {
    log_dir().join(LOG_BASENAME)
}

/// Python's `levelname` for a `log` level. The Python logger is at INFO, so DEBUG
/// never reaches a handler; TRACE has no Python counterpart and is treated the same.
pub fn level_name(level: log::Level) -> &'static str {
    match level {
        log::Level::Error => "ERROR",
        log::Level::Warn => "WARNING",
        log::Level::Info => "INFO",
        log::Level::Debug | log::Level::Trace => "DEBUG",
    }
}

/// Local time as `%Y-%m-%d %H:%M:%S`.
pub fn local_timestamp() -> String {
    // SAFETY: GetLocalTime only writes the SYSTEMTIME it returns.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// `"%(asctime)s [%(levelname)s] %(message)s"`.
pub fn format_line(timestamp: &str, level: log::Level, message: &str) -> String {
    format!("{timestamp} [{}] {message}", level_name(level))
}

/// Python's `logging.handlers.RotatingFileHandler` (append mode, UTF-8, text-mode
/// newlines), without the formatter.
pub struct RotatingFileHandler {
    path: PathBuf,
    max_bytes: u64,
    backup_count: u32,
    stream: Option<File>,
}

impl RotatingFileHandler {
    /// Opens the file for append at once, as Python does with `delay=False`.
    pub fn new(path: impl Into<PathBuf>, max_bytes: u64, backup_count: u32) -> io::Result<Self> {
        let path = path.into();
        let stream = Some(Self::open(&path)?);
        Ok(RotatingFileHandler {
            path,
            max_bytes,
            backup_count,
            stream,
        })
    }

    fn open(path: &Path) -> io::Result<File> {
        OpenOptions::new().create(true).append(true).open(path)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `shouldRollover`. The size test is the current byte offset plus the formatted
    /// message's length in *characters* plus one, as Python's `len(msg)` counts it; an
    /// empty file never rolls over.
    fn should_rollover(&mut self, line: &str) -> io::Result<bool> {
        if self.stream.is_none() {
            self.stream = Some(Self::open(&self.path)?);
        }
        if self.max_bytes == 0 {
            return Ok(false);
        }
        let Some(stream) = self.stream.as_mut() else {
            return Ok(false);
        };
        let pos = stream.seek(SeekFrom::End(0))?;
        if pos == 0 {
            return Ok(false);
        }
        let msg_len = line.chars().count() as u64 + 1;
        if pos + msg_len >= self.max_bytes {
            if self.path.exists() && !self.path.is_file() {
                return Ok(false);
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn numbered(&self, i: u32) -> PathBuf {
        let mut s = self.path.as_os_str().to_owned();
        s.push(format!(".{i}"));
        PathBuf::from(s)
    }

    /// `doRollover`: shift `.1`..`.N-1` up one, move the current file to `.1`, reopen.
    pub fn do_rollover(&mut self) -> io::Result<()> {
        self.stream = None;
        if self.backup_count > 0 {
            for i in (1..self.backup_count).rev() {
                let sfn = self.numbered(i);
                let dfn = self.numbered(i + 1);
                if sfn.exists() {
                    if dfn.exists() {
                        std::fs::remove_file(&dfn)?;
                    }
                    std::fs::rename(&sfn, &dfn)?;
                }
            }
            let dfn = self.numbered(1);
            if dfn.exists() {
                std::fs::remove_file(&dfn)?;
            }
            if self.path.exists() {
                std::fs::rename(&self.path, &dfn)?;
            }
        }
        self.stream = Some(Self::open(&self.path)?);
        Ok(())
    }

    /// Write one formatted line (no terminator), rolling over first if it would reach
    /// `max_bytes`. Newlines are written as CRLF, as Python's text-mode file does.
    pub fn emit(&mut self, line: &str) -> io::Result<()> {
        if self.should_rollover(line)? {
            self.do_rollover()?;
        }
        let Some(stream) = self.stream.as_mut() else {
            return Ok(());
        };
        let mut text = line.replace('\n', "\r\n");
        text.push_str("\r\n");
        stream.write_all(text.as_bytes())?;
        stream.flush()
    }
}

/// A second destination for formatted log lines (the GUI log pane, tests).
pub trait LogSink: Send + Sync {
    /// One formatted line, exactly as written to the file (without the newline).
    fn emit(&self, level: log::Level, line: &str);
}

/// Handle returned by [`add_sink`], for [`remove_sink`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkId(u64);

struct Inner {
    file: Option<RotatingFileHandler>,
    sinks: Vec<(SinkId, Arc<dyn LogSink>)>,
    next_id: u64,
    console: bool,
}

/// The process-wide logger (Python's `get_logger()` singleton).
pub struct Logger {
    inner: Mutex<Inner>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

fn wanted(target: &str) -> bool {
    target.starts_with("beam") || target.starts_with(LOGGER_NAME)
}

impl Logger {
    fn new(file: Option<RotatingFileHandler>, console: bool) -> Self {
        Logger {
            inner: Mutex::new(Inner {
                file,
                sinks: Vec::new(),
                next_id: 1,
                console,
            }),
        }
    }

    /// Format and send one record to the file, the console and every sink. Errors
    /// writing the file are reported on stderr and swallowed, as Python's
    /// `handleError` does.
    pub fn write(&self, level: log::Level, message: &str) {
        if level > log::Level::Info {
            return;
        }
        let line = format_line(&local_timestamp(), level, message);
        let sinks: Vec<Arc<dyn LogSink>> = {
            let mut inner = self.inner.lock();
            if let Some(file) = inner.file.as_mut()
                && let Err(e) = file.emit(&line)
            {
                eprintln!("--- Logging error ---\n{e}");
            }
            if inner.console {
                eprintln!("{line}");
            }
            inner.sinks.iter().map(|(_, s)| s.clone()).collect()
        };
        for sink in sinks {
            sink.emit(level, &line);
        }
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info && wanted(metadata.target())
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            self.write(record.level(), &record.args().to_string());
        }
    }

    fn flush(&self) {}
}

/// Set up and return the shared logger, writing to [`log_filename`] (Python's
/// `get_logger()`). Idempotent. Installs it behind the `log` facade at INFO and logs
/// the "Logger Initialized" banner the first time.
pub fn init() -> &'static Logger {
    init_with(log_filename(), true)
}

/// [`init`] with the log file elsewhere and the console copy optional (tests, tools).
/// Only the first call in a process takes effect.
pub fn init_with(path: impl Into<PathBuf>, console: bool) -> &'static Logger {
    let path = path.into();
    let mut fresh = false;
    let logger = LOGGER.get_or_init(|| {
        fresh = true;
        let file = match RotatingFileHandler::new(&path, MAX_BYTES, BACKUP_COUNT) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("cannot open log file {}: {e}", path.display());
                None
            }
        };
        Logger::new(file, console)
    });
    if fresh {
        if log::set_logger(logger).is_ok() {
            log::set_max_level(log::LevelFilter::Info);
        }
        logger.write(
            log::Level::Info,
            &format!("{} Logger Initialized {}", "=".repeat(20), "=".repeat(20)),
        );
    }
    logger
}

/// The shared logger, if [`init`] has run.
pub fn get() -> Option<&'static Logger> {
    LOGGER.get()
}

/// Add a sink that receives every formatted line from now on. Initialises the logger
/// first if nothing has.
pub fn add_sink(sink: Arc<dyn LogSink>) -> SinkId {
    let logger = LOGGER.get().unwrap_or_else(init);
    let mut inner = logger.inner.lock();
    let id = SinkId(inner.next_id);
    inner.next_id += 1;
    inner.sinks.push((id, sink));
    id
}

/// Remove a sink added with [`add_sink`]. Unknown ids are ignored.
pub fn remove_sink(id: SinkId) {
    if let Some(logger) = LOGGER.get() {
        logger.inner.lock().sinks.retain(|(i, _)| *i != id);
    }
}
