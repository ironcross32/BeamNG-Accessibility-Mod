//! Owner: E10. `updater.py`: the GitHub-only auto-updater.
//!
//! The whole product ships as ONE release asset (`BeamNG_accessibility_mod.zip`, holding
//! `beamtel.exe`, `README.md` and `bng_screenreader_mod.zip`), so an update is: fetch that
//! asset, put its contents where the running program lives, and offer the inner mod zip
//! to BeamNG afterwards.
//!
//! Two facts shape everything here.
//!
//! * The running exe cannot replace itself: Windows holds it open for the life of the
//!   process, so the swap happens after we exit, in a windowless `.cmd` helper that waits
//!   for the executable's file lock, copies the staged files over and starts the new exe.
//!   That restart is also why the mod install cannot happen in the same run.
//! * The exe and the Lua/JS mod go out of step silently, so the update is two-phase with a
//!   flag in the config file across the restart: phase one swaps the program directory,
//!   phase two offers the mod zip. The flag is written only AFTER the download has
//!   validated and staged, so a failed download never leaves a phase-two prompt behind.
//!
//! Nothing here launches BeamNG.drive; the launch is deferred until this flow answers
//! through the [`Gate`].
//!
//! Prompts and progress go through [`UiSink`]. Every outside effect (HTTP, the config
//! file, the directories, starting the helper) goes through an [`UpdaterEnv`], so tests
//! run the whole flow against temp directories without the network.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use beam_base::config::{RawRead, load_config_configurator_at, read_config_raw, write_config};
use beam_base::pyfmt::py_strip;
use beam_base::pyval::{PyDict, PyValue, json_loads, py_int_parse, py_str_repr};
use parking_lot::Mutex;
use regex::Regex;

use crate::mod_install::{self, ModInstallEnv};
use crate::ui_sink::{Answer, Buttons, MessageBox, MessageKind, Progress, UiSink};

/// Bumped by hand when cutting a tag; must equal the tag with the "v" stripped, or every
/// client concludes it is already up to date. Also read by the window title.
pub const APP_VERSION: &str = "0.2.1";

pub const REPO: &str = "ironcross32/BeamNG-Accessibility-Mod";
pub const LATEST_URL: &str =
    "https://api.github.com/repos/ironcross32/BeamNG-Accessibility-Mod/releases/latest";

/// Must equal the build's bundle name: the same contract seen from either end.
pub const ASSET_NAME: &str = "BeamNG_accessibility_mod.zip";

/// Files the bundle must contain to be a bundle at all, checked before anything is
/// staged: a truncated download or an HTML error page saved under a .zip name fails here
/// rather than halfway through overwriting the user's install.
pub const REQUIRED_MEMBERS: [&str; 2] = ["beamtel.exe", "bng_screenreader_mod.zip"];

pub const HELPER_NAME: &str = "apply_update.cmd";
pub const HELPER_LOG: &str = "apply_update.log";

/// GitHub rejects API requests with no User-Agent outright.
pub static USER_AGENT: LazyLock<String> =
    LazyLock::new(|| format!("BEAM-accessibility-mod-updater/{APP_VERSION}"));

pub const CHECK_TIMEOUT_S: f64 = 8.0;
pub const DOWNLOAD_TIMEOUT_S: f64 = 30.0;

pub const PENDING_KEY: &str = "pending_update_version";
pub const ENABLED_KEY: &str = "update_check_enabled";

/// `UPDATE_DIR`: `<config dir>\update`.
pub fn update_dir() -> PathBuf {
    beam_base::paths::config_dir().join("update")
}

/// `STAGED_DIR`: `<config dir>\update\staged`.
pub fn staged_dir() -> PathBuf {
    update_dir().join("staged")
}

/// `program_dir()`: the directory the user launched us from (beside
/// `bng_screenreader_mod.zip`), the same directory the mod installer reads the zip from.
pub fn program_dir() -> PathBuf {
    beam_base::paths::program_dir()
}

// =========================
//  Launch gate
// =========================

/// The updater's view of the deferred game launch (`LaunchGate` / `NullGate`).
pub trait Gate: Send + Sync {
    fn allow(&self);
    fn deny(&self);
}

/// `NullGate`: decides nothing. The manual "Check for updates" button runs the same flow
/// as startup, but the deferred launch has long since happened by then, so its gate must
/// be inert rather than merely already-set, or a manual check would fire a launch nobody
/// asked for.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullGate;

impl Gate for NullGate {
    fn allow(&self) {}
    fn deny(&self) {}
}

// =========================
//  Version comparison
// =========================

/// One component of a parsed version: Python's unbounded `int`, kept as its decimal
/// digits (no leading zeros) so that ordering by (length, digits) is numeric ordering.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VersionNum {
    len: usize,
    digits: String,
}

impl VersionNum {
    fn from_digits(ascii_digits: &str) -> Self {
        let trimmed = ascii_digits.trim_start_matches('0');
        let digits = if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        };
        VersionNum {
            len: digits.len(),
            digits,
        }
    }
}

impl std::fmt::Display for VersionNum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.digits)
    }
}

/// `parse_version`'s result: a 3-tuple compared element by element, like Python's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub [VersionNum; 3]);

impl std::fmt::Display for Version {
    /// Python's tuple repr: `(1, 2, 3)`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {}, {})", self.0[0], self.0[1], self.0[2])
    }
}

static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?").expect("version regex"));
static DIGIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d$").expect("digit regex"));

fn is_unicode_digit(c: char) -> bool {
    let mut buf = [0u8; 4];
    DIGIT_RE.is_match(c.encode_utf8(&mut buf))
}

/// The decimal value of a Unicode `Nd` character, as `int()` reads it. Unicode lays every
/// decimal digit set out as a contiguous run starting at its zero, so the value is the
/// distance from the start of the run, modulo 10 (some runs hold several sets).
fn digit_value(c: char) -> u32 {
    if let Some(d) = c.to_digit(10) {
        return d;
    }
    let mut start = c as u32;
    while start > 0 {
        match char::from_u32(start - 1) {
            Some(p) if is_unicode_digit(p) => start -= 1,
            _ => break,
        }
    }
    (c as u32 - start) % 10
}

fn version_num(group: Option<regex::Match<'_>>) -> VersionNum {
    match group {
        None => VersionNum::from_digits("0"),
        Some(m) => {
            let ascii: String = m
                .as_str()
                .chars()
                .map(|c| char::from_digit(digit_value(c), 10).unwrap_or('0'))
                .collect();
            VersionNum::from_digits(&ascii)
        }
    }
}

/// `parse_version`: `"v1.2.3"` -> (1, 2, 3). `None` when it cannot be read as a version.
///
/// A pre-release suffix ("v1.2.3-beta") keeps its numeric prefix; anything with no
/// leading number is `None`. An unreadable REMOTE tag is "no update" in [`is_newer`]:
/// guessing the other way would offer an update on every startup with no way to stop it.
pub fn parse_version(text: &str) -> Option<Version> {
    if text.is_empty() {
        return None;
    }
    let caps = VERSION_RE.captures(py_strip(text))?;
    Some(Version([
        version_num(caps.get(1)),
        version_num(caps.get(2)),
        version_num(caps.get(3)),
    ]))
}

/// `is_newer(remote_tag, local=APP_VERSION)`.
pub fn is_newer(remote_tag: &str, local: &str) -> bool {
    match (parse_version(remote_tag), parse_version(local)) {
        (Some(r), Some(l)) => r > l,
        _ => false,
    }
}

// =========================
//  HTTP (injected)
// =========================

/// A failed request, shaped like the urllib exceptions the Python catches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// `urllib.error.HTTPError`: the server answered with an error status.
    Status { code: u16, reason: String },
    /// `urllib.error.URLError`: the server could not be reached.
    Url(String),
    /// Anything else (a timeout mid-response, a protocol error).
    Other(String),
}

impl std::fmt::Display for HttpError {
    /// `str(e)` of the matching urllib exception.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Status { code, reason } => write!(f, "HTTP Error {code}: {reason}"),
            HttpError::Url(reason) => write!(f, "<urlopen error {reason}>"),
            HttpError::Other(m) => f.write_str(m),
        }
    }
}

/// An open response: the `Content-Length` header as sent, and the body.
pub struct HttpResponse {
    pub content_length: Option<String>,
    pub body: Box<dyn Read + Send>,
}

/// `urllib.request.urlopen(Request(url, method="GET", headers=...), timeout=...)`.
/// `timeout_s` is a per-operation socket timeout (connect, each read), as urllib's is.
pub trait HttpClient: Send + Sync {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        timeout_s: f64,
    ) -> Result<HttpResponse, HttpError>;
}

/// The real client: `ureq`, following redirects (GitHub's asset URL redirects to its CDN).
#[derive(Debug, Default)]
pub struct UreqClient;

impl HttpClient for UreqClient {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        timeout_s: f64,
    ) -> Result<HttpResponse, HttpError> {
        let t = Some(Duration::from_secs_f64(timeout_s));
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(t)
            .timeout_per_read(t)
            .timeout_per_write(t)
            .build()
            .into();
        let mut req = agent.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req.call().map_err(|e| match e {
            ureq::Error::StatusCode(code) => HttpError::Status {
                code,
                reason: http_reason(code),
            },
            ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
            | ureq::Error::Io(_)
            | ureq::Error::BadUri(_) => HttpError::Url(e.to_string()),
            ureq::Error::Timeout(_) => HttpError::Other("timed out".to_string()),
            other => HttpError::Other(other.to_string()),
        })?;
        let status = resp.status().as_u16();
        if status >= 400 {
            return Err(HttpError::Status {
                code: status,
                reason: resp.status().canonical_reason().unwrap_or("").to_string(),
            });
        }
        let content_length = resp
            .headers()
            .get("content-length")
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
        let body = resp.into_body().into_reader();
        Ok(HttpResponse {
            content_length,
            body: Box::new(body),
        })
    }
}

fn http_reason(code: u16) -> String {
    ureq::http::StatusCode::from_u16(code)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("")
        .to_string()
}

// =========================
//  Environment
// =========================

/// Where the update flow reads and writes, and how it reaches the outside world.
pub struct UpdaterEnv {
    /// `CONFIG_PATH`.
    pub config_path: PathBuf,
    /// `UPDATE_DIR`.
    pub update_dir: PathBuf,
    /// `STAGED_DIR`.
    pub staged_dir: PathBuf,
    /// `program_dir()`.
    pub program_dir: PathBuf,
    /// `os.environ.get("SystemRoot")`.
    pub system_root: Option<String>,
    /// `os.getpid()` as written into the helper.
    pub pid: u32,
    pub http: Box<dyn HttpClient>,
    /// Starts the helper (`subprocess.Popen`). Tests record the launch instead.
    pub spawn_helper: Box<dyn Fn(&HelperLaunch) -> io::Result<()> + Send + Sync>,
    /// Where phase two's mod install looks.
    pub mod_install: ModInstallEnv,
    /// Every log line the flow writes is also appended here, when set.
    pub log_tap: Option<Arc<Mutex<Vec<(log::Level, String)>>>>,
    /// The progress dialog's poll interval (`time.sleep(0.05)`).
    pub progress_interval: Duration,
}

impl UpdaterEnv {
    /// The real paths, network and process launch.
    pub fn real() -> Self {
        UpdaterEnv {
            config_path: beam_base::paths::config_path(),
            update_dir: update_dir(),
            staged_dir: staged_dir(),
            program_dir: program_dir(),
            system_root: std::env::var("SystemRoot").ok(),
            pid: std::process::id(),
            http: Box::new(UreqClient),
            spawn_helper: Box::new(spawn_helper_process),
            mod_install: ModInstallEnv::real(),
            log_tap: None,
            progress_interval: Duration::from_millis(50),
        }
    }

    fn log(&self, level: log::Level, msg: String) {
        log::log!(level, "{msg}");
        if let Some(tap) = &self.log_tap {
            tap.lock().push((level, msg));
        }
    }
}

// =========================
//  Python exception text
// =========================

/// `str(exc)` for the OSErrors the Python can raise here, so the failure dialogs read
/// the same. Win32 calls (`os.mkdir`, `os.replace`, `CreateProcess`) raise the
/// `[WinError N] <FormatMessage text>` form; the CRT's `open()` raises `[Errno N] <strerror>`
/// with the Win32 code mapped to an errno as CPython maps it.
pub(crate) mod pyos {
    use std::io;
    use std::path::Path;

    use beam_base::pyval::py_str_repr;

    fn win_message(e: &io::Error, code: i32) -> String {
        let text = e.to_string();
        let suffix = format!(" (os error {code})");
        let text = text.strip_suffix(&suffix).unwrap_or(&text);
        // CPython strips trailing whitespace and dots from FormatMessage's text.
        text.trim_end_matches(|c: char| c <= ' ' || c == '.')
            .to_string()
    }

    fn with_files(mut s: String, f1: Option<&Path>, f2: Option<&Path>) -> String {
        if let Some(a) = f1 {
            s.push_str(": ");
            s.push_str(&py_str_repr(&a.to_string_lossy()));
            if let Some(b) = f2 {
                s.push_str(" -> ");
                s.push_str(&py_str_repr(&b.to_string_lossy()));
            }
        }
        s
    }

    /// The `[WinError N] ...` form.
    pub fn win(e: &io::Error, f1: Option<&Path>, f2: Option<&Path>) -> String {
        match e.raw_os_error() {
            Some(code) => with_files(
                format!("[WinError {code}] {}", win_message(e, code)),
                f1,
                f2,
            ),
            None => e.to_string(),
        }
    }

    /// The `[Errno N] ...` form of a failed CRT call.
    pub fn errno(e: &io::Error, f1: Option<&Path>) -> String {
        match e.raw_os_error() {
            Some(code) => {
                let n = winerror_to_errno(code);
                with_files(format!("[Errno {n}] {}", strerror(n)), f1, None)
            }
            None => e.to_string(),
        }
    }

    /// CPython's `winerror_to_errno` (PC/errmap.h) for the Win32 range.
    pub fn winerror_to_errno(code: i32) -> i32 {
        match code {
            2 | 3 | 15 | 18 | 53 | 67 | 161 | 206 => 2,
            4 => 24,
            5 | 16 | 19..=36 | 65 | 82 | 83 | 108 | 132 | 158 | 167 => 13,
            6 | 114 | 130 => 9,
            7 | 8 | 9 | 1816 => 12,
            10 => 7,
            11 | 188..=202 => 8,
            17 => 18,
            80 | 183 => 17,
            89 | 164 | 215 => 11,
            109 | 232 => 32,
            112 => 28,
            128 | 129 => 10,
            145 => 41,
            258 => 138,
            267 => 20,
            1113 => 42,
            _ => 22,
        }
    }

    /// The MSVC CRT's `strerror` for the errnos [`winerror_to_errno`] produces.
    pub fn strerror(n: i32) -> &'static str {
        match n {
            2 => "No such file or directory",
            7 => "Arg list too long",
            8 => "Exec format error",
            9 => "Bad file descriptor",
            10 => "No child processes",
            11 => "Resource temporarily unavailable",
            12 => "Not enough space",
            13 => "Permission denied",
            17 => "File exists",
            18 => "Improper link",
            20 => "Not a directory",
            24 => "Too many open files",
            28 => "No space left on device",
            32 => "Broken pipe",
            41 => "Directory not empty",
            42 => "Illegal byte sequence",
            138 => "timed out",
            _ => "Invalid argument",
        }
    }

    /// `str(UnicodeDecodeError)` for a strict UTF-8 decode of `bytes`.
    pub fn utf8_decode_error(bytes: &[u8], e: &std::str::Utf8Error) -> String {
        let pos = e.valid_up_to();
        let (len, reason) = match e.error_len() {
            None => (bytes.len() - pos, "unexpected end of data"),
            Some(n) => {
                let b = bytes[pos];
                let valid_lead = (0xC2..=0xF4).contains(&b);
                if n == 1 && !valid_lead {
                    (1, "invalid start byte")
                } else {
                    (n, "invalid continuation byte")
                }
            }
        };
        if len == 1 {
            format!(
                "'utf-8' codec can't decode byte 0x{:02x} in position {pos}: {reason}",
                bytes[pos]
            )
        } else {
            format!(
                "'utf-8' codec can't decode bytes in position {pos}-{}: {reason}",
                pos + len - 1
            )
        }
    }

    /// `type(x).__name__` for a JSON value.
    pub fn type_name(v: &beam_base::pyval::PyValue) -> &'static str {
        use beam_base::pyval::PyValue;
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
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_string())
}

/// `bool(error)` on the Python's `error` (None or a message).
fn err_truthy(e: &Option<String>) -> bool {
    e.as_deref().is_some_and(|s| !s.is_empty())
}

/// `path + suffix`, as string concatenation on a path does.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

// =========================
//  GitHub release query
// =========================

/// One release: the tag, the bundle asset's download URL and size, exactly as the JSON
/// carried them (the Python does not check their types).
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: PyValue,
    pub url: PyValue,
    pub size: PyValue,
}

impl Release {
    pub fn new(tag: PyValue, url: PyValue, size: PyValue) -> Self {
        let size = if size.truthy() {
            size
        } else {
            PyValue::from(0i64)
        };
        Release { tag, url, size }
    }

    /// `self.tag.lstrip("vV")`; `Err` is the `AttributeError` a non-string tag raises.
    pub fn version(&self) -> Result<String, String> {
        match &self.tag {
            PyValue::Str(s) => Ok(s.trim_start_matches(['v', 'V']).to_string()),
            other => Err(format!(
                "'{}' object has no attribute 'lstrip'",
                pyos::type_name(other)
            )),
        }
    }
}

/// `json.load(resp)` on the response bytes: UTF-8, with a UTF-8 BOM accepted.
fn json_load_bytes(bytes: &[u8]) -> Result<PyValue, String> {
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(body).map_err(|e| pyos::utf8_decode_error(body, &e))?;
    json_loads(text).map_err(|e| e.0)
}

/// `check_latest(timeout)`: the newest published release, or `None`.
///
/// Every failure the Python catches (offline, rate limited, malformed JSON, no matching
/// asset) is `Ok(None)` after logging: a check that cannot complete must never block
/// startup or the game launch behind it. `Err` is an exception the Python does not catch
/// (an `assets` value that cannot be iterated).
pub fn check_latest(env: &UpdaterEnv, timeout: f64) -> Result<Option<Release>, String> {
    use log::Level::{Info, Warn};
    let headers = [
        ("User-Agent", USER_AGENT.as_str()),
        ("Accept", "application/vnd.github+json"),
    ];
    let data = match env.http.get(LATEST_URL, &headers, timeout) {
        Err(HttpError::Status { code, .. }) => {
            env.log(Warn, format!("Update check: GitHub returned HTTP {code}"));
            return Ok(None);
        }
        Err(HttpError::Url(reason)) => {
            env.log(
                Info,
                format!("Update check: could not reach GitHub ({reason})"),
            );
            return Ok(None);
        }
        Err(HttpError::Other(e)) => {
            env.log(Warn, format!("Update check failed: {e}"));
            return Ok(None);
        }
        Ok(mut resp) => {
            let mut bytes = Vec::new();
            let parsed = match resp.body.read_to_end(&mut bytes) {
                Err(e) => Err(e.to_string()),
                Ok(_) => json_load_bytes(&bytes),
            };
            match parsed {
                Ok(v) => v,
                Err(e) => {
                    env.log(Warn, format!("Update check failed: {e}"));
                    return Ok(None);
                }
            }
        }
    };

    let PyValue::Dict(data) = data else {
        env.log(Warn, "Update check: unexpected response shape".to_string());
        return Ok(None);
    };

    let tag = match data.get("tag_name") {
        Some(v) if v.truthy() => v.clone(),
        _ => PyValue::from(""),
    };
    let mut asset_url = PyValue::None;
    let mut asset_size = PyValue::from(0i64);
    let assets: Vec<PyValue> = match data.get("assets") {
        Some(v) if v.truthy() => match v {
            PyValue::List(items) => items.clone(),
            // Iterating a dict or a string yields strings, which are never asset dicts.
            PyValue::Dict(_) | PyValue::Str(_) => Vec::new(),
            other => {
                return Err(format!(
                    "'{}' object is not iterable",
                    pyos::type_name(other)
                ));
            }
        },
        _ => Vec::new(),
    };
    for asset in &assets {
        if let PyValue::Dict(a) = asset
            && a.get("name") == Some(&PyValue::from(ASSET_NAME))
        {
            asset_url = a
                .get("browser_download_url")
                .cloned()
                .unwrap_or(PyValue::None);
            asset_size = match a.get("size") {
                Some(v) if v.truthy() => v.clone(),
                _ => PyValue::from(0i64),
            };
            break;
        }
    }
    if !tag.truthy() || !asset_url.truthy() {
        env.log(
            Warn,
            format!(
                "Update check: release {} carries no {ASSET_NAME} asset",
                tag.py_repr()
            ),
        );
        return Ok(None);
    }
    Ok(Some(Release::new(tag, asset_url, asset_size)))
}

// =========================
//  Download / stage
// =========================

/// `urllib.parse.unwrap(str(url))`, then `Request`'s fragment split.
fn request_url(url: &PyValue) -> Result<String, String> {
    let s = url.py_str();
    let mut u = py_strip(&s).to_string();
    if u.starts_with('<') && u.ends_with('>') && !u.is_empty() {
        u = py_strip(&u[1..u.len() - 1]).to_string();
    }
    if let Some(rest) = u.strip_prefix("URL:") {
        u = py_strip(rest).to_string();
    }
    let full = u.clone();
    // `_splittype`: `([^/:]+):(.*)`.
    let scheme_ok = match u.find(':') {
        Some(i) if i > 0 => !u[..i].contains('/'),
        _ => false,
    };
    if !scheme_ok {
        return Err(format!("unknown url type: {}", py_str_repr(&full)));
    }
    Ok(full)
}

/// `download(url, dest, progress=None, cancelled=None, timeout=DOWNLOAD_TIMEOUT_S)`.
///
/// Streams the asset to `dest`, then validates it as one of our bundles. `Ok(None)` on
/// success, `Ok(Some(message))` describing what went wrong (`"cancelled"` when the user
/// cancelled). The validation is the point: everything after this overwrites the user's
/// install. `Err` is an exception the Python lets escape (creating the directory, an
/// unparseable URL, the final rename).
pub fn download(
    env: &UpdaterEnv,
    url: &PyValue,
    dest: &Path,
    progress: Option<&(dyn Fn(u64, f64) + Sync)>,
    cancelled: Option<&(dyn Fn() -> bool + Sync)>,
    timeout: f64,
) -> Result<Option<String>, String> {
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| pyos::win(&e, Some(parent), None))?;
    }
    let tmp = with_suffix(dest, ".part");
    let full_url = request_url(url)?;

    let fetched: Result<Option<&'static str>, String> = (|| {
        let scheme = full_url[..full_url.find(':').unwrap_or(0)].to_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(format!("<urlopen error unknown url type: {scheme}>"));
        }
        let headers = [("User-Agent", USER_AGENT.as_str())];
        let mut resp = env
            .http
            .get(&full_url, &headers, timeout)
            .map_err(|e| e.to_string())?;
        let total = match resp.content_length.as_deref() {
            None | Some("") => 0.0,
            Some(h) => match py_int_parse(h) {
                Some(n) => n.to_f64().unwrap_or(f64::INFINITY),
                None => {
                    return Err(format!(
                        "invalid literal for int() with base 10: {}",
                        py_str_repr(h)
                    ));
                }
            },
        };
        let mut done: u64 = 0;
        let mut f = fs::File::create(&tmp).map_err(|e| pyos::errno(&e, Some(&tmp)))?;
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            if let Some(c) = cancelled
                && c()
            {
                return Ok(Some("cancelled"));
            }
            let n = read_full(&mut resp.body, &mut chunk).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            f.write_all(&chunk[..n])
                .map_err(|e| pyos::errno(&e, None))?;
            done += n as u64;
            if let Some(p) = progress {
                p(done, total);
            }
        }
        Ok(None)
    })();
    match fetched {
        Ok(Some(c)) => return Ok(Some(c.to_string())),
        Ok(None) => {}
        Err(e) => {
            quiet_remove(&tmp);
            return Ok(Some(format!("Download failed: {e}")));
        }
    }

    if let Err(e) = validate_bundle(&tmp) {
        quiet_remove(&tmp);
        return Ok(Some(format!(
            "The downloaded file is not a valid BEAM update ({e})."
        )));
    }

    quiet_remove(dest);
    fs::rename(&tmp, dest).map_err(|e| pyos::win(&e, Some(&tmp), Some(dest)))?;
    Ok(None)
}

/// `resp.read(n)`: up to `buf.len()` bytes, short only at the end of the body.
fn read_full(r: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// A member name as `zipfile` reports it: cut at a NUL, backslashes turned into slashes.
fn zip_member_name(raw: &str) -> String {
    let cut = raw.split('\0').next().unwrap_or("");
    cut.replace('\\', "/")
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<fs::File>, String> {
    let f = fs::File::open(path).map_err(|e| pyos::errno(&e, Some(path)))?;
    zip::ZipArchive::new(f).map_err(|_| "File is not a zip file".to_string())
}

fn zip_names(zf: &zip::ZipArchive<fs::File>) -> Vec<String> {
    zf.file_names()
        .zip(zf.file_names_raw())
        .map(|(name, raw)| match name {
            Ok(n) => zip_member_name(&n),
            Err(_) => zip_member_name(&String::from_utf8_lossy(raw)),
        })
        .collect()
}

/// `zipfile.ZipFile(tmp)` + `testzip()` + the required-members check.
fn validate_bundle(tmp: &Path) -> Result<(), String> {
    let mut zf = open_zip(tmp)?;
    for i in 0..zf.len() {
        let ok = match zf.by_index(i) {
            Ok(mut member) => io::copy(&mut member, &mut io::sink()).is_ok(),
            Err(_) => false,
        };
        if !ok {
            return Err("archive contains a corrupt entry".to_string());
        }
    }
    let names = zip_names(&zf);
    let missing: Vec<&str> = REQUIRED_MEMBERS
        .iter()
        .copied()
        .filter(|m| !names.iter().any(|n| n == m))
        .collect();
    if !missing.is_empty() {
        return Err(format!("archive does not contain {}", missing.join(", ")));
    }
    Ok(())
}

/// `ntpath.isabs` (3.13) on a name already using forward slashes.
fn nt_isabs(norm: &str) -> bool {
    let head: String = norm.chars().take(3).collect::<String>().replace('/', "\\");
    head.starts_with('\\') || head.chars().skip(1).take(2).collect::<String>() == ":\\"
}

/// The path `ZipFile.extract` writes a member to, relative to the target directory:
/// drive and `.`/`..`/empty components dropped, Windows-illegal characters replaced,
/// trailing dots and spaces stripped from each component.
fn sanitized_arcname(name: &str) -> String {
    let arc = name.replace('/', "\\");
    // splitdrive: a second-character colon is a drive (the UNC case cannot reach here).
    let arc: &str = if arc.chars().nth(1) == Some(':') {
        let skip: usize = arc.chars().take(2).map(char::len_utf8).sum();
        &arc[skip..]
    } else {
        &arc
    };
    let joined: Vec<&str> = arc
        .split('\\')
        .filter(|x| !x.is_empty() && *x != "." && *x != "..")
        .collect();
    let translated: String = joined
        .join("\\")
        .chars()
        .map(|c| if ":<>|\"?*".contains(c) { '_' } else { c })
        .collect();
    translated
        .split('\\')
        .map(|x| x.trim_end_matches([' ', '.']))
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join("\\")
}

/// `zf.extractall(staged_dir)`.
fn extract_all(zf: &mut zip::ZipArchive<fs::File>, target: &Path) -> Result<(), String> {
    for i in 0..zf.len() {
        let mut member = zf.by_index(i).map_err(|e| e.to_string())?;
        let name = zip_member_name(&String::from_utf8_lossy(member.name_raw()));
        let name = match member.name() {
            Ok(n) => zip_member_name(&n),
            Err(_) => name,
        };
        let is_dir = name.ends_with('/');
        let arcname = sanitized_arcname(&name);
        if arcname.is_empty() && !is_dir {
            return Err("Empty filename.".to_string());
        }
        let targetpath = if arcname.is_empty() {
            target.to_path_buf()
        } else {
            target.join(&arcname)
        };
        if let Some(upper) = targetpath.parent()
            && !upper.as_os_str().is_empty()
            && !upper.exists()
        {
            fs::create_dir_all(upper).map_err(|e| pyos::win(&e, Some(upper), None))?;
        }
        if is_dir {
            if !targetpath.is_dir()
                && let Err(e) = fs::create_dir(&targetpath)
                && !(e.kind() == io::ErrorKind::AlreadyExists && targetpath.is_dir())
            {
                return Err(pyos::win(&e, Some(&targetpath), None));
            }
            continue;
        }
        let mut out =
            fs::File::create(&targetpath).map_err(|e| pyos::errno(&e, Some(&targetpath)))?;
        let mut buf = Vec::new();
        member
            .read_to_end(&mut buf)
            .map_err(|_| format!("Bad CRC-32 for file {}", py_str_repr(&name)))?;
        out.write_all(&buf).map_err(|e| pyos::errno(&e, None))?;
    }
    Ok(())
}

/// `stage(zip_path, staged_dir=STAGED_DIR)`: extract the validated bundle into a scratch
/// directory. `Ok(None)` on success or `Ok(Some(message))`.
///
/// The zip-slip guard is here even though we build the archive ourselves: this is the one
/// place that writes remote-named paths into the user's program directory, and the check
/// costs nothing.
pub fn stage(zip_path: &Path, staged_dir: &Path) -> Result<Option<String>, String> {
    let _ = fs::remove_dir_all(staged_dir);
    fs::create_dir_all(staged_dir).map_err(|e| pyos::win(&e, Some(staged_dir), None))?;
    let unpacked: Result<(), String> = (|| {
        let mut zf = open_zip(zip_path)?;
        for name in zip_names(&zf) {
            let norm = name.replace('\\', "/");
            if norm.starts_with('/') || nt_isabs(&norm) {
                return Err(format!("absolute path in archive: {name}"));
            }
            if norm.split('/').any(|p| p == "..") {
                return Err(format!("parent-directory path in archive: {name}"));
            }
        }
        extract_all(&mut zf, staged_dir)
    })();
    if let Err(e) = unpacked {
        let _ = fs::remove_dir_all(staged_dir);
        return Ok(Some(format!("Could not unpack the update: {e}")));
    }
    for member in REQUIRED_MEMBERS {
        if !staged_dir.join(member).is_file() {
            let _ = fs::remove_dir_all(staged_dir);
            return Ok(Some(format!("The unpacked update is missing {member}.")));
        }
    }
    Ok(None)
}

/// `_quiet_remove(path)`.
fn quiet_remove(path: &Path) {
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}

// =========================
//  Apply (after we exit)
// =========================

/// The helper `.cmd`. `@PLACEHOLDERS@` rather than format strings: this is a batch file,
/// where `%` is the variable syntax and doubling it is exactly the sort of quiet breakage a
/// helper that runs after we have exited gives no chance to see.
///
/// It asks the FILE, not the process table, whether the exe can be overwritten yet (open
/// for append fails while Windows holds the image), so a second install elsewhere cannot
/// confuse it and a slow or failing `tasklist` cannot lie to it. A timeout abandons the
/// update without touching the live exe or its `.appdata` onefile cache. Every step after
/// the copy is logged and verified, because `start` reports almost nothing.
///
/// **Deliberate difference from the Python (python-bugs.md 67):** the three paths come
/// from the environment ([`helper_env`], set on the `cmd` process by
/// [`spawn_helper_process`]) instead of being written into the file. The Python wrote
/// them in and encoded the file as ASCII, so a non-ASCII user name (the update folder is
/// under the profile) raised UnicodeEncodeError after the launch had been denied, and the
/// update could never apply. Environment variables are Unicode and `cmd` expands them as
/// such, whatever code page it reads the file in; a `%` in a path is no longer re-read
/// either. The file stays pure ASCII. Everything else is the Python's template.
pub const HELPER_TEMPLATE: &str = r##"@echo off
setlocal
set "PID=@PID@"
rem The three paths arrive in the environment, not in this file: a path can hold any
rem character (a non-ASCII user name, a percent sign), and a batch file is read in the
rem console code page. Cleared at once so the restarted program does not inherit them.
set "PROG=%BEAM_UPDATE_PROG%"
set "STAGED=%BEAM_UPDATE_STAGED%"
set "LOGFILE=%BEAM_UPDATE_LOG%"
set "BEAM_UPDATE_PROG="
set "BEAM_UPDATE_STAGED="
set "BEAM_UPDATE_LOG="
set "SYS=%SystemRoot%\System32"
set "EXE=%PROG%\beamtel.exe"

echo [%DATE% %TIME%] waiting for "%EXE%" to be released (we were PID %PID%)> "%LOGFILE%"
set /a TRIES=0
:wait
rem The question is not "is a process called beamtel.exe running", it is "can
rem I overwrite this file yet" -- so ASK THE FILE. Opening the exe for append
rem fails with a sharing violation for as long as Windows holds the image open,
rem and succeeds (writing nothing) the moment the last handle goes. It costs one
rem file open, enumerates nothing, and cannot be confused by a second install
rem running elsewhere. The tasklist/find pipeline it replaces could be WRONG IN
rem BOTH DIRECTIONS: its stderr went to nul, so a failure of tasklist itself was
rem indistinguishable from "no such process". Observed 2026-08-28: one call took
rem 103 seconds and then answered "gone" while beamtel was still up, and the
rem post-start check answered "did not appear" about a process that had started.
rem Both are that same false negative, and the second one made the log lie about
rem the outcome of the update.
if not exist "%EXE%" goto gone
2>nul ( >>"%EXE%" (call ) ) && goto gone
set /a TRIES+=1
if %TRIES% GEQ 120 goto timeout
"%SYS%\ping.exe" -n 2 127.0.0.1 >nul
goto wait

:timeout
rem A locked executable cannot be replaced safely. In particular, do not remove
rem the onefile cache or fall through to robocopy: either can damage the still
rem running installation. Leave the staged update in place for diagnosis and
rem stop this helper; a later update check can stage a fresh copy.
echo [%DATE% %TIME%] ERROR: "%EXE%" still locked after %TRIES% tries; update abandoned>> "%LOGFILE%"
goto finish

:gone
rem Give Windows a moment to release the executable image itself.
"%SYS%\ping.exe" -n 3 127.0.0.1 >nul
echo [%DATE% %TIME%] "%EXE%" released after %TRIES% tries>> "%LOGFILE%"

:ready

rem The Nuitka onefile cache holds the OLD exe's unpacked payload under a fixed
rem name beside the exe (--onefile-tempdir-spec={PROGRAM_DIR}/.appdata with
rem --onefile-cache-mode=cached). It is only safe to remove once the process is
rem gone, which is precisely here. A HALF-removed cache is worse than a stale
rem one -- the new bootstrap can sit on it -- so the outcome is recorded.
if exist "%PROG%\.appdata" rd /s /q "%PROG%\.appdata"
if exist "%PROG%\.appdata" echo [%DATE% %TIME%] WARNING: .appdata survived the delete>> "%LOGFILE%"

echo [%DATE% %TIME%] copying "%STAGED%" -^> "%PROG%">> "%LOGFILE%"
"%SYS%\robocopy.exe" "%STAGED%" "%PROG%" /E /IS /IT /R:5 /W:2 >> "%LOGFILE%" 2>&1
set RC=%ERRORLEVEL%
echo [%DATE% %TIME%] robocopy exit %RC% >> "%LOGFILE%"
if %RC% GEQ 8 echo [%DATE% %TIME%] copy FAILED; not restarting>> "%LOGFILE%"
if %RC% GEQ 8 goto done

if not exist "%EXE%" echo [%DATE% %TIME%] ERROR: "%EXE%" missing after copy>> "%LOGFILE%"
if not exist "%EXE%" goto done

rem /D sets the new process's working directory: the helper's own is the update
rem folder, and handing that to the program we are restarting is not what it
rem would have had if the user had launched it themselves.
echo [%DATE% %TIME%] starting "%EXE%">> "%LOGFILE%"
start "BEAM" /B /D "%PROG%" "%EXE%"
echo [%DATE% %TIME%] start returned %ERRORLEVEL% >> "%LOGFILE%"

rem "start" is asynchronous and reports almost nothing, so the launch is
rem VERIFIED rather than assumed. Without this the log's last line is written
rem before the attempt and a silent failure to restart leaves no evidence at
rem all -- which is exactly how this went unexplained once already.
"%SYS%\ping.exe" -n 6 127.0.0.1 >nul
rem The same lock test read the other way round: a running beamtel holds its
rem own image open, so an exe we can still write to is one nothing is running.
2>nul ( >>"%EXE%" (call ) ) && (echo [%DATE% %TIME%] ERROR: "%EXE%" is still writable; it did not start>> "%LOGFILE%") || (echo [%DATE% %TIME%] beamtel.exe is running>> "%LOGFILE%")

:done
rd /s /q "%STAGED%"
:finish
echo [%DATE% %TIME%] helper finished>> "%LOGFILE%"
(goto) 2>nul & del "%~f0"
"##;

/// The helper's text: the template with the process id filled in. The paths are not in
/// the text (see [`HELPER_TEMPLATE`]), so the body is always ASCII.
pub fn helper_body(pid: u32) -> String {
    HELPER_TEMPLATE.replace("@PID@", &pid.to_string())
}

/// The environment the helper reads its paths from: the values the Python wrote into the
/// file (`abspath(prog)`, `abspath(staged_dir)`, `join(update_dir, HELPER_LOG)`).
pub fn helper_env(prog: &Path, staged_dir: &Path, update_dir: &Path) -> Vec<(String, String)> {
    vec![
        ("BEAM_UPDATE_PROG".to_string(), abspath(prog)),
        ("BEAM_UPDATE_STAGED".to_string(), abspath(staged_dir)),
        (
            "BEAM_UPDATE_LOG".to_string(),
            update_dir.join(HELPER_LOG).to_string_lossy().into_owned(),
        ),
    ]
}

/// The bytes `open(path, "w", encoding="ascii", newline="\r\n").write(body)` leaves on
/// disk; `Err` is the `UnicodeEncodeError` message for a non-ASCII body.
pub fn helper_file_bytes(body: &str) -> Result<Vec<u8>, String> {
    let text = body.replace('\n', "\r\n");
    let chars: Vec<char> = text.chars().collect();
    if let Some(start) = chars.iter().position(|c| !c.is_ascii()) {
        let end = chars[start..]
            .iter()
            .position(|c| c.is_ascii())
            .map_or(chars.len(), |n| start + n);
        return Err(if end - start == 1 {
            let c = chars[start] as u32;
            let esc = if c <= 0xff {
                format!("\\x{c:02x}")
            } else if c <= 0xffff {
                format!("\\u{c:04x}")
            } else {
                format!("\\U{c:08x}")
            };
            format!(
                "'ascii' codec can't encode character '{esc}' in position {start}: ordinal not in range(128)"
            )
        } else {
            format!(
                "'ascii' codec can't encode characters in position {start}-{}: ordinal not in range(128)",
                end - 1
            )
        });
    }
    Ok(text.into_bytes())
}

fn abspath(p: &Path) -> String {
    std::path::absolute(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Why `write_helper` raised: an `OSError` (caught by `apply_and_restart`) or anything
/// else (the `UnicodeEncodeError`, which escapes the whole flow).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteHelperError {
    Os(String),
    Other(String),
}

/// `write_helper(staged_dir, prog, update_dir)`: write `apply_update.cmd` and return its
/// path. The paths go to the helper through [`helper_env`], not the file.
pub fn write_helper(update_dir: &Path, pid: u32) -> Result<PathBuf, WriteHelperError> {
    fs::create_dir_all(update_dir)
        .map_err(|e| WriteHelperError::Os(pyos::win(&e, Some(update_dir), None)))?;
    let path = update_dir.join(HELPER_NAME);
    let body = helper_body(pid);
    // open() truncates before the encoder runs, so a non-ASCII body leaves an empty file.
    let mut f =
        fs::File::create(&path).map_err(|e| WriteHelperError::Os(pyos::errno(&e, Some(&path))))?;
    let bytes = helper_file_bytes(&body).map_err(WriteHelperError::Other)?;
    f.write_all(&bytes)
        .map_err(|e| WriteHelperError::Os(pyos::errno(&e, None)))?;
    Ok(path)
}

/// `subprocess.list2cmdline(seq)`.
pub fn list2cmdline(seq: &[&str]) -> String {
    let mut result = String::new();
    for arg in seq {
        let mut bs_buf = String::new();
        if !result.is_empty() {
            result.push(' ');
        }
        let needquote = arg.contains(' ') || arg.contains('\t') || arg.is_empty();
        if needquote {
            result.push('"');
        }
        for c in arg.chars() {
            if c == '\\' {
                bs_buf.push(c);
            } else if c == '"' {
                result.push_str(&"\\".repeat(bs_buf.len() * 2));
                bs_buf.clear();
                result.push_str("\\\"");
            } else {
                if !bs_buf.is_empty() {
                    result.push_str(&bs_buf);
                    bs_buf.clear();
                }
                result.push(c);
            }
        }
        if !bs_buf.is_empty() {
            result.push_str(&bs_buf);
        }
        if needquote {
            result.push_str(&bs_buf);
            result.push('"');
        }
    }
    result
}

pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// One `subprocess.Popen` of the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperLaunch {
    /// `%SystemRoot%\System32\cmd.exe`, fully qualified.
    pub program: PathBuf,
    /// `["/d", "/c", <helper path>]`.
    pub args: Vec<String>,
    /// `list2cmdline([cmd_exe, "/d", "/c", helper])`: the command line Python builds.
    pub command_line: String,
    pub cwd: PathBuf,
    pub creation_flags: u32,
    /// Added to the inherited environment: the helper's paths ([`helper_env`]).
    pub env: Vec<(String, String)>,
}

/// The real launch: no window, a new process group, every standard handle on NUL (the app
/// has no console, so the helper must not inherit invalid handles or allocate one).
pub fn spawn_helper_process(launch: &HelperLaunch) -> io::Result<()> {
    let mut cmd = std::process::Command::new(&launch.program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let args: Vec<&str> = launch.args.iter().map(String::as_str).collect();
        cmd.raw_arg(list2cmdline(&args));
        cmd.creation_flags(launch.creation_flags);
    }
    #[cfg(not(windows))]
    {
        cmd.args(&launch.args);
    }
    cmd.current_dir(&launch.cwd)
        .envs(launch.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_child| ())
}

/// The launch `apply_and_restart` would make for this helper.
pub fn helper_launch(env: &UpdaterEnv, helper: &Path) -> HelperLaunch {
    let system_root = match env.system_root.as_deref() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => r"C:\Windows".to_string(),
    };
    let cmd_exe = PathBuf::from(system_root).join("System32").join("cmd.exe");
    let helper_s = helper.to_string_lossy().into_owned();
    let cmd_s = cmd_exe.to_string_lossy().into_owned();
    HelperLaunch {
        command_line: list2cmdline(&[&cmd_s, "/d", "/c", &helper_s]),
        program: cmd_exe,
        args: vec!["/d".into(), "/c".into(), helper_s],
        cwd: env.update_dir.clone(),
        creation_flags: CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP,
        env: Vec::new(),
    }
}

/// `apply_and_restart(staged_dir=STAGED_DIR)`: spawn the windowless helper. The caller
/// must then close the app promptly. `Ok(None)` on success or `Ok(Some(message))`; `Err`
/// is an exception the Python does not catch here.
///
/// `CREATE_NO_WINDOW`, not `DETACHED_PROCESS`: redirecting the standard handles does not
/// stop console allocation, and `DETACHED_PROCESS` makes `CREATE_NO_WINDOW` inapplicable
/// (the v0.1.5 helper's children opened visible windows that way). A new process group
/// keeps it outside console-control events aimed at beamtel; an ordinary child process
/// already outlives its parent on Windows.
///
/// **Deliberate difference (python-bugs.md 67):** any failure to write the helper is
/// reported like the others, so the caller clears the pending flag, shows the message and
/// allows the launch. The Python let a non-OSError escape the whole flow after the launch
/// had been denied.
pub fn apply_and_restart(env: &UpdaterEnv, staged_dir: &Path) -> Result<Option<String>, String> {
    let helper = match write_helper(&env.update_dir, env.pid) {
        Ok(p) => p,
        Err(WriteHelperError::Os(e) | WriteHelperError::Other(e)) => {
            return Ok(Some(format!("Could not write the update helper: {e}")));
        }
    };
    let mut launch = helper_launch(env, &helper);
    launch.env = helper_env(&env.program_dir, staged_dir, &env.update_dir);
    if let Err(e) = (env.spawn_helper)(&launch) {
        return Ok(Some(format!(
            "Could not start the update helper: {}",
            pyos::win(&e, None, None)
        )));
    }
    Ok(None)
}

// =========================
//  Pending-update flag
// =========================

/// `_patch_config(key, value)`: set one key in the config file, preserving everything
/// else verbatim. A read-modify-write of the RAW file, not load/save: that path merges
/// defaults and rewrites the whole document, which is wrong for a single flag written
/// from a background flow. `Err` is the raised exception's text.
fn patch_config(env: &UpdaterEnv, key: &str, value: PyValue) -> Result<(), String> {
    let mut raw: PyDict = match read_config_raw(&env.config_path) {
        RawRead::Parsed(PyValue::Dict(d)) => d,
        RawRead::Invalid(msg) => return Err(msg),
        RawRead::Parsed(_) | RawRead::OsError(_) => {
            load_config_configurator_at(&env.config_path).map_err(|e| pyos::win(&e, None, None))?
        }
    };
    raw.insert(key.to_string(), value);
    write_config(&env.config_path, &raw).map_err(|e| pyos::win(&e, None, None))
}

/// `set_pending_update(tag)`.
pub fn set_pending_update(env: &UpdaterEnv, tag: &str) -> Result<(), String> {
    patch_config(env, PENDING_KEY, PyValue::from(tag))
}

/// `clear_pending_update()`.
pub fn clear_pending_update(env: &UpdaterEnv) -> Result<(), String> {
    patch_config(env, PENDING_KEY, PyValue::from(""))
}

// =========================
//  Progress plumbing
// =========================

/// What a progress worker sees: `report(done, total)` and `cancelled()`.
pub struct Reporter {
    done: AtomicU64,
    total: Mutex<f64>,
    cancel: AtomicBool,
}

impl Reporter {
    pub fn report(&self, done: u64, total: f64) {
        self.done.store(done, Ordering::SeqCst);
        *self.total.lock() = total;
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// `_run_with_progress(parent, title, message, work, cancellable)`: run `work` off the
/// calling thread behind a progress dialog. `work` returns `(result, error)`, or `Err`
/// for an exception, which becomes the error. The error is `"cancelled"` if the user
/// cancelled and the work reported nothing worse.
fn run_with_progress<T: Send>(
    sink: &dyn UiSink,
    env: &UpdaterEnv,
    title: &str,
    message: &str,
    work: impl FnOnce(&Reporter) -> Result<(Option<T>, Option<String>), String> + Send,
    cancellable: bool,
) -> (Option<T>, Option<String>) {
    let reporter = Reporter {
        done: AtomicU64::new(0),
        total: Mutex::new(0.0),
        cancel: AtomicBool::new(false),
    };
    // The dialog appears on construction, at zero.
    sink.progress_update(&Progress {
        title: title.to_string(),
        text: message.to_string(),
        fraction: Some(0.0),
        cancellable,
    });
    let (result, mut error) = std::thread::scope(|scope| {
        let rep = &reporter;
        let handle = std::thread::Builder::new()
            .name("beamtel-updater".to_string())
            .spawn_scoped(scope, move || work(rep));
        let handle = match handle {
            Ok(h) => h,
            Err(e) => return (None, Some(e.to_string())),
        };
        while !handle.is_finished() {
            let total = *reporter.total.lock();
            let keep = if total > 0.0 {
                let done = reporter.done.load(Ordering::SeqCst);
                let pct = ((done as f64 * 100.0 / total).trunc()).min(100.0);
                sink.progress_update(&Progress {
                    title: title.to_string(),
                    text: format!(
                        "{message}\n{:.1} of {:.1} MB",
                        done as f64 / 1048576.0,
                        total / 1048576.0
                    ),
                    fraction: Some(pct / 100.0),
                    cancellable,
                })
            } else {
                sink.progress_update(&Progress {
                    title: title.to_string(),
                    text: message.to_string(),
                    fraction: None,
                    cancellable,
                })
            };
            if cancellable && !keep {
                reporter.cancel.store(true, Ordering::SeqCst);
            }
            std::thread::sleep(env.progress_interval);
        }
        match handle.join() {
            Ok(Ok((r, e))) => (r, e),
            Ok(Err(e)) => (None, Some(e)),
            Err(p) => (None, Some(panic_text(&p))),
        }
    });
    sink.progress_close();
    if reporter.cancelled() && !err_truthy(&error) {
        error = Some("cancelled".to_string());
    }
    (result, error)
}

// =========================
//  The flow
// =========================

fn message(
    sink: &dyn UiSink,
    kind: MessageKind,
    title: &str,
    text: String,
    buttons: Buttons,
    default: Answer,
) -> Answer {
    sink.message_box(&MessageBox {
        kind,
        title: title.to_string(),
        text,
        buttons,
        default,
    })
}

/// `run_startup_flow(frame, gate, manual=False)` on the real paths and network.
///
/// Decides about updates, then releases the deferred game launch. Exactly one of
/// `allow`/`deny` fires on every path: allow is in a finally, so a crash in here cannot
/// leave the engine waiting out its whole timeout before starting the game.
pub fn run_startup_flow(sink: &dyn UiSink, gate: &dyn Gate, manual: bool) {
    run_startup_flow_with(sink, gate, manual, &UpdaterEnv::real());
}

/// [`run_startup_flow`] against an explicit environment.
pub fn run_startup_flow_with(sink: &dyn UiSink, gate: &dyn Gate, manual: bool, env: &UpdaterEnv) {
    let decided = AtomicBool::new(false);
    let allow = || {
        if !decided.swap(true, Ordering::SeqCst) {
            gate.allow();
        }
    };
    let deny = || {
        decided.store(true, Ordering::SeqCst);
        gate.deny();
    };

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        startup_body(sink, env, manual, &allow, &deny)
    }));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(e)) => env.log(log::Level::Error, format!("Updater flow failed: {e}")),
        Err(p) => env.log(
            log::Level::Error,
            format!("Updater flow failed: {}", panic_text(&p)),
        ),
    }
    allow();
}

fn startup_body(
    sink: &dyn UiSink,
    env: &UpdaterEnv,
    manual: bool,
    allow: &dyn Fn(),
    deny: &dyn Fn(),
) -> Result<(), String> {
    let cfg =
        load_config_configurator_at(&env.config_path).map_err(|e| pyos::win(&e, None, None))?;
    let pending = match cfg.get(PENDING_KEY) {
        Some(v) if v.truthy() => py_strip(&v.py_str()).to_string(),
        _ => String::new(),
    };
    if !pending.is_empty() && !manual {
        // A helper failure happens after the old GUI has exited, so it cannot clear this
        // flag. The running version is the durable success check: after a successful copy
        // it equals the pending release; after a timeout/copy/restart failure the user can
        // only relaunch the old version and the two differ. Clear the stale promise and
        // let the ordinary check offer the update again.
        if parse_version(&pending) == parse_version(APP_VERSION) {
            phase_two(sink, env, &pending);
            return Ok(());
        }
        env.log(
            log::Level::Error,
            format!("Discarding pending update {pending}: running version is still {APP_VERSION}"),
        );
        if let Err(e) = clear_pending_update(env) {
            env.log(
                log::Level::Error,
                format!("Could not clear the stale pending update flag: {e}"),
            );
        }
    }
    check_and_offer(sink, env, &cfg, manual, allow, deny)
}

/// `_phase_two(frame, pending)`: we have just restarted into a freshly copied build, so
/// offer the mod zip. The flag is cleared whichever way the user answers: it records "an
/// update was applied and the mod has not been offered yet", and it has been offered now.
fn phase_two(sink: &dyn UiSink, env: &UpdaterEnv, pending: &str) {
    env.log(
        log::Level::Info,
        format!("Pending update {pending}: offering the mod install"),
    );
    let answer = message(
        sink,
        MessageKind::Question,
        "Install the updated mod",
        format!(
            "BEAM has been updated to version {pending}.\n\n\
             Install the updated BeamNG.drive mod now?"
        ),
        Buttons::YesNo,
        Answer::Yes,
    );
    let mut installed = false;
    if answer == Answer::Yes {
        installed = mod_install::install_mod_interactive_in(sink, &env.mod_install);
    } else {
        env.log(
            log::Level::Info,
            format!("User declined the mod install for update {pending}"),
        );
    }
    if let Err(e) = clear_pending_update(env) {
        env.log(
            log::Level::Error,
            format!("Could not clear the pending update flag: {e}"),
        );
    }
    env.log(
        log::Level::Info,
        format!(
            "Pending update {pending} resolved (mod installed: {})",
            if installed { "True" } else { "False" }
        ),
    );
}

fn check_and_offer(
    sink: &dyn UiSink,
    env: &UpdaterEnv,
    cfg: &PyDict,
    manual: bool,
    allow: &dyn Fn(),
    deny: &dyn Fn(),
) -> Result<(), String> {
    use log::Level::Info;
    if !manual && !cfg.get(ENABLED_KEY).is_none_or(PyValue::truthy) {
        return Ok(());
    }

    let (release, error) = run_with_progress(
        sink,
        env,
        "Checking for updates",
        "Contacting GitHub...",
        |_rep| check_latest(env, CHECK_TIMEOUT_S).map(|r| (r, None)),
        false,
    );
    let release = match release {
        Some(r) if !err_truthy(&error) => r,
        _ => {
            env.log(Info, "Update check found nothing to install".to_string());
            if manual {
                message(
                    sink,
                    MessageKind::Warning,
                    "Update Check Failed",
                    "Could not check for updates right now.".to_string(),
                    Buttons::Ok,
                    Answer::Ok,
                );
            }
            return Ok(());
        }
    };

    let tag = release.tag.py_str();
    if !is_newer(&tag, APP_VERSION) {
        env.log(
            Info,
            format!("Up to date: running {APP_VERSION}, latest release is {tag}"),
        );
        if manual {
            message(
                sink,
                MessageKind::Info,
                "No Update Available",
                format!("You are running the latest version ({APP_VERSION})."),
                Buttons::Ok,
                Answer::Ok,
            );
        }
        return Ok(());
    }

    // The offer is the question and nothing else: the release notes are commit messages,
    // and a link is painful to get out of a message box with a screen reader.
    let version = release.version()?;
    let answer = message(
        sink,
        MessageKind::Question,
        "Update Available",
        format!(
            "You have version {APP_VERSION}. Version {version} is available.\n\nInstall it now?"
        ),
        Buttons::YesNo,
        Answer::Yes,
    );
    if answer != Answer::Yes {
        env.log(Info, format!("User declined update {tag}"));
        return Ok(());
    }

    fs::create_dir_all(&env.update_dir).map_err(|e| pyos::win(&e, Some(&env.update_dir), None))?;
    let zip_path = env.update_dir.join(ASSET_NAME);

    let (_, error) = run_with_progress::<()>(
        sink,
        env,
        "Downloading update",
        &format!("Downloading {ASSET_NAME}..."),
        |rep| {
            let progress = |done: u64, total: f64| rep.report(done, total);
            let cancelled = || rep.cancelled();
            download(
                env,
                &release.url,
                &zip_path,
                Some(&progress),
                Some(&cancelled),
                DOWNLOAD_TIMEOUT_S,
            )
            .map(|e| (None, e))
        },
        true,
    );
    if error.as_deref() == Some("cancelled") {
        env.log(Info, format!("User cancelled the download of {tag}"));
        quiet_remove(&zip_path);
        allow();
        return Ok(());
    }
    if let Some(e) = error.filter(|e| !e.is_empty()) {
        fail(sink, env, &e, allow);
        return Ok(());
    }

    if let Some(e) = stage(&zip_path, &env.staged_dir)?.filter(|e| !e.is_empty()) {
        fail(sink, env, &e, allow);
        return Ok(());
    }

    // Only now, with a validated and unpacked update on disk, does the pending flag go
    // in: a promise made before staging could not be kept.
    if let Err(e) = set_pending_update(env, &version) {
        fail(
            sink,
            env,
            &format!("Could not record the pending update: {e}"),
            allow,
        );
        return Ok(());
    }

    // Only at the point of no return is the launch denied. Denying it at the "yes"
    // would strand the user with no game if the download then failed, because the engine
    // reads the decision the moment the gate opens and never looks again.
    deny();
    if let Some(e) = apply_and_restart(env, &env.staged_dir)?.filter(|e| !e.is_empty()) {
        let _ = clear_pending_update(env);
        fail(sink, env, &e, allow);
        return Ok(());
    }

    env.log(Info, format!("Update {tag} staged; restarting to apply it"));
    message(
        sink,
        MessageKind::Info,
        "Restarting to Update",
        "BEAM will now close and restart on the new version.".to_string(),
        Buttons::Ok,
        Answer::Ok,
    );
    sink.request_exit();
    Ok(())
}

/// `_fail(frame, message, allow)`: report a failed update and fall back to a completely
/// normal startup.
fn fail(sink: &dyn UiSink, env: &UpdaterEnv, msg: &str, allow: &dyn Fn()) {
    env.log(log::Level::Error, format!("Update failed: {msg}"));
    message(
        sink,
        MessageKind::Error,
        "Update Failed",
        format!(
            "The update could not be installed.\n\n{msg}\n\n\
             BEAM will carry on running the version you already have."
        ),
        Buttons::Ok,
        Answer::Ok,
    );
    allow();
}
