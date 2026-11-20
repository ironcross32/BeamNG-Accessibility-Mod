//! Owner: E7. The AI Describer: `_ai_describe_worker` and `_trigger_ai_describe`
//! (`beamtel.py:9604-9701`), plus the non-registry half of `ai_describer.py` (capture,
//! the Gemini and OpenAI request builders, key validation, the description log). The
//! registry half is `beam_base::ai_registry` (B1).
//!
//! Every network call goes through [`urlopen`], which tests replace with
//! [`set_transport_for_tests`]; the screen grab goes through [`capture_region`], which
//! tests replace with [`set_capture_hook_for_tests`]. A Python exception that the Python
//! does not catch inside a function is that function's `Err(message)`.

use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use beam_audio::controller::AudioController;
use beam_base::ai_registry::{
    self, DEFAULT_PROVIDER, ExtraKind, GEMINI_BASE, GEMINI_DEFAULT_MODEL, OPENAI_BASE,
    OPENAI_DEFAULT_MODEL, OPENAI_DETAIL_LEVELS, OPENAI_REASONING_EFFORTS,
};
use beam_base::pyfmt::py_strip;
use beam_base::pyval::{PyDict, PyValue, from_json_map, json_loads, py_str_repr};
use parking_lot::RwLock;
use serde_json::{Value, json};

use crate::speech_out::say;
use crate::state::{DESCRIBER_BUSY, state};

/// Verbatim system prompt: the scene for a blind listener, as plain text for a screen
/// reader (no Markdown).
pub const SYSTEM_PROMPT: &str = "Describe this image succinctly, but in as much detail as possible as if to a \
blind person. Be sure to include details about the environment, such as time \
of day and lighting, as well as details about the scene overall. Include \
details regarding the positioning of objects relative to one another, starting \
from what's closest to the center, and working outward. The response you \
provide will be processed through a screen reader as synthetic speech. As \
such, please use plain text which is devoid of special formatting syntax such \
as Markdown. Line breaks are fine where appropriate.";

pub const MAX_OUTPUT_TOKENS: i64 = 1000;
/// Thinking tokens count against the output budget, so thinking-capable models get this
/// on top; otherwise the visible answer is truncated after a few words.
pub const THINKING_HEADROOM_TOKENS: i64 = 2048;
pub const REQUEST_TIMEOUT_SEC: f64 = 60.0;
/// Key validation is a one-off click, so it gets a generous budget.
pub const VALIDATE_TIMEOUT_SEC: f64 = 30.0;

/// `ai_describer.LOG_PATH` (`ai_descriptions.log` in the config directory).
pub fn log_path() -> PathBuf {
    beam_base::paths::config_dir().join("ai_descriptions.log")
}

// ---- Python value helpers ----------------------------------------------------------------

/// `type(x).__name__`.
pub fn py_type_name(v: &PyValue) -> &'static str {
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

fn no_attr(v: &PyValue, attr: &str) -> String {
    format!("'{}' object has no attribute '{attr}'", py_type_name(v))
}

/// `x.get(key)` (AttributeError for a non-dict).
fn pget<'a>(v: &'a PyValue, key: &str) -> Result<Option<&'a PyValue>, String> {
    match v {
        PyValue::Dict(d) => Ok(d.get(key)),
        other => Err(no_attr(other, "get")),
    }
}

/// `x.get(key) or {}` / `or []`: the value if truthy.
fn pget_truthy<'a>(v: &'a PyValue, key: &str) -> Result<Option<&'a PyValue>, String> {
    Ok(pget(v, key)?.filter(|x| x.truthy()))
}

/// `for item in x`.
fn py_iter(v: &PyValue) -> Result<Vec<PyValue>, String> {
    match v {
        PyValue::List(l) => Ok(l.clone()),
        PyValue::Dict(d) => Ok(d.keys().map(|k| PyValue::Str(k.clone())).collect()),
        PyValue::Str(s) => Ok(s.chars().map(|c| PyValue::Str(c.to_string())).collect()),
        other => Err(format!("'{}' object is not iterable", py_type_name(other))),
    }
}

/// `x[0]`.
fn py_first(v: &PyValue) -> Result<PyValue, String> {
    match v {
        PyValue::List(l) => l
            .first()
            .cloned()
            .ok_or_else(|| "list index out of range".into()),
        PyValue::Str(s) => s
            .chars()
            .next()
            .map(|c| PyValue::Str(c.to_string()))
            .ok_or_else(|| "string index out of range".into()),
        PyValue::Dict(_) => Err("0".into()),
        other => Err(format!(
            "'{}' object is not subscriptable",
            py_type_name(other)
        )),
    }
}

/// `"".join(parts)`.
fn py_join(parts: &[PyValue]) -> Result<String, String> {
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        match p {
            PyValue::Str(s) => out.push_str(s),
            other => {
                return Err(format!(
                    "sequence item {i}: expected str instance, {} found",
                    py_type_name(other)
                ));
            }
        }
    }
    Ok(out)
}

fn is_str(v: Option<&PyValue>, s: &str) -> bool {
    matches!(v, Some(PyValue::Str(x)) if x == s)
}

/// `bytes.decode("utf-8", errors="ignore")`.
fn decode_ignore(b: &[u8]) -> String {
    crate::net::decode(b, crate::net::Decode::Utf8Ignore).unwrap_or_default()
}

/// `urllib.parse.quote(s)` (safe="/").
pub fn py_quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn dumps_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn dumps_into(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => out.push_str(&i.to_string()),
            (_, Some(u), _) => out.push_str(&u.to_string()),
            (_, _, Some(f)) => out.push_str(&beam_base::pyfmt::py_float_str(f)),
            _ => out.push_str(&n.to_string()),
        },
        Value::String(s) => dumps_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dumps_into(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dumps_str(k, out);
                out.push_str(": ");
                dumps_into(x, out);
            }
            out.push('}');
        }
    }
}

/// `json.dumps(v)` with Python's defaults (`", "`, `": "`, ensure_ascii).
pub fn py_json_dumps(v: &Value) -> String {
    let mut out = String::new();
    dumps_into(v, &mut out);
    out
}

// ---- Screen capture ----------------------------------------------------------------------

/// An mss-style region `{left, top, width, height}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub left: i64,
    pub top: i64,
    pub width: i64,
    pub height: i64,
}

impl Region {
    /// The dict, in the Python's key order.
    pub fn to_json(&self) -> Value {
        json!({"left": self.left, "top": self.top, "width": self.width, "height": self.height})
    }
}

/// Replaces the screen grab (tests only).
pub type CaptureHook = Box<dyn Fn(Option<Region>) -> Result<Vec<u8>, String> + Send + Sync>;
static CAPTURE_HOOK: RwLock<Option<CaptureHook>> = RwLock::new(None);

#[doc(hidden)]
pub fn set_capture_hook_for_tests(hook: Option<CaptureHook>) {
    *CAPTURE_HOOK.write() = hook;
}

/// `capture_region(region=None)`: grab a screen region as PNG bytes. None means the first
/// monitor mss enumerates (`monitors[1]`), which is what the AI Describer has always used:
/// it fires while the user is playing, so the game fills that display. `Err` is the
/// message of the exception mss would raise.
pub fn capture_region(region: Option<Region>) -> Result<Vec<u8>, String> {
    if let Some(h) = CAPTURE_HOOK.read().as_ref() {
        return h(region);
    }
    gdi::capture(region)
}

/// `capture_primary_monitor()`.
pub fn capture_primary_monitor() -> Result<Vec<u8>, String> {
    capture_region(None)
}

/// RGB bytes to PNG (8-bit RGB, no row filter, as `mss.tools.to_png` writes).
pub fn rgb_to_png(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_filter(png::Filter::NoFilter);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(rgb).map_err(|e| e.to_string())?;
        w.finish().map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(windows)]
mod gdi {
    use super::{Region, rgb_to_png};
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC,
        CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GdiFlush,
        GetWindowDC, HBITMAP, HDC, HGDIOBJ, HMONITOR, ReleaseDC, SRCCOPY, SelectObject,
    };
    use windows::Win32::UI::HiDpi::{PROCESS_PER_MONITOR_DPI_AWARE, SetProcessDpiAwareness};
    use windows::core::BOOL;

    fn fail(name: &str) -> String {
        let e = windows::core::Error::from_thread();
        if e.code().0 == 0 {
            format!("Windows graphics function failed (no error provided): {name}")
        } else {
            format!("Windows graphics function failed: {name}: {}", e.message())
        }
    }

    unsafe extern "system" fn monitor_cb(
        _h: HMONITOR,
        _dc: HDC,
        rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the `Vec<Region>` owned by `monitors` for the duration of the
        // EnumDisplayMonitors call, and `rect` is a valid RECT supplied by the system.
        unsafe {
            let out = &mut *(data.0 as *mut Vec<Region>);
            let r = &*rect;
            out.push(Region {
                left: r.left as i64,
                top: r.top as i64,
                width: (r.right - r.left) as i64,
                height: (r.bottom - r.top) as i64,
            });
        }
        BOOL(1)
    }

    fn monitors() -> Result<Vec<Region>, String> {
        let mut out: Vec<Region> = Vec::new();
        // SAFETY: the callback only writes into `out`, which outlives the call.
        let ok = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(monitor_cb),
                LPARAM(&mut out as *mut Vec<Region> as isize),
            )
        };
        if !ok.as_bool() {
            return Err(fail("EnumDisplayMonitors"));
        }
        Ok(out)
    }

    pub fn capture(region: Option<Region>) -> Result<Vec<u8>, String> {
        // mss makes the process per-monitor DPI aware on every instance it creates. When
        // the shell already set an awareness this fails, harmlessly.
        // SAFETY: no pointers; the call only changes process state.
        let _ = unsafe { SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE) };
        let mon = match region {
            Some(r) => r,
            None => *monitors()?.first().ok_or("list index out of range")?,
        };
        let (w, h) = (mon.width as i32, mon.height as i32);
        // SAFETY: every handle is created here, checked, and released below; the DIB
        // memory is read only after GdiFlush, while the bitmap is still alive.
        unsafe {
            let src = GetWindowDC(None);
            if src.is_invalid() {
                return Err(fail("GetWindowDC"));
            }
            let mem = CreateCompatibleDC(Some(src));
            if mem.is_invalid() {
                ReleaseDC(None, src);
                return Err(fail("CreateCompatibleDC"));
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let result = (|| -> Result<Vec<u8>, String> {
                let dib: HBITMAP =
                    CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
                        .map_err(|_| fail("CreateDIBSection"))?;
                let old = SelectObject(mem, HGDIOBJ(dib.0));
                let grab = (|| -> Result<Vec<u8>, String> {
                    if old.is_invalid() {
                        return Err(fail("SelectObject"));
                    }
                    BitBlt(
                        mem,
                        0,
                        0,
                        w,
                        h,
                        Some(src),
                        mon.left as i32,
                        mon.top as i32,
                        SRCCOPY | CAPTUREBLT,
                    )
                    .map_err(|_| fail("BitBlt"))?;
                    let _ = GdiFlush();
                    if bits.is_null() || w <= 0 || h <= 0 {
                        return Err(fail("CreateDIBSection"));
                    }
                    let n = (w as usize) * (h as usize);
                    let raw = std::slice::from_raw_parts(bits as *const u8, n * 4);
                    let mut rgb = Vec::with_capacity(n * 3);
                    for px in raw.chunks_exact(4) {
                        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
                    }
                    Ok(rgb)
                })();
                if !old.is_invalid() {
                    SelectObject(mem, old);
                }
                let _ = DeleteObject(HGDIOBJ(dib.0));
                grab
            })();
            let _ = DeleteDC(mem);
            ReleaseDC(None, src);
            let rgb = result?;
            rgb_to_png(&rgb, w as u32, h as u32)
        }
    }
}

#[cfg(not(windows))]
mod gdi {
    use super::Region;
    pub fn capture(_region: Option<Region>) -> Result<Vec<u8>, String> {
        Err("screen capture is only available on Windows".into())
    }
}

// ---- HTTP --------------------------------------------------------------------------------

/// One request as urllib would send it.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: f64,
    /// Whether the caller reads the body (`resp.read()`) of a 2xx response.
    pub read_body: bool,
}

/// What `urlopen` did, in the shape of urllib's exception ladder.
#[derive(Debug, Clone, PartialEq)]
pub enum HttpOutcome {
    /// A 2xx response (`resp.status`, and the body if it was read).
    Response { status: u16, body: Vec<u8> },
    /// `urllib.error.HTTPError` (`e.code`, `e.read()`; empty if reading failed).
    HttpError { code: u16, body: Vec<u8> },
    /// `urllib.error.URLError` with `str(e.reason)`.
    UrlError(String),
    /// Any other exception, `str(e)`.
    Other(String),
}

/// Replaces the network (tests only).
pub type TransportHook = Box<dyn Fn(&HttpRequest) -> HttpOutcome + Send + Sync>;
static TRANSPORT: RwLock<Option<TransportHook>> = RwLock::new(None);

#[doc(hidden)]
pub fn set_transport_for_tests(hook: Option<TransportHook>) {
    *TRANSPORT.write() = hook;
}

/// `urllib.request.Request(url)`'s parse: unwrap, drop the fragment, and require a
/// scheme. `Err` is the `ValueError` Request raises (outside every try in the Python).
fn request_url(url: &str) -> Result<String, String> {
    let mut u = py_strip(url).to_string();
    if u.starts_with('<') && u.ends_with('>') && u.len() >= 2 {
        u = py_strip(&u[1..u.len() - 1]).to_string();
    }
    if let Some(rest) = u.strip_prefix("URL:") {
        u = py_strip(rest).to_string();
    }
    if let Some(i) = u.find('#') {
        u.truncate(i);
    }
    match u.find(':') {
        Some(i) if i > 0 && !u[..i].contains('/') => Ok(u),
        _ => Err(format!("unknown url type: {}", py_str_repr(&u))),
    }
}

/// `urllib.request.urlopen(req, timeout=...)`.
pub fn urlopen(req: &HttpRequest) -> HttpOutcome {
    if let Some(h) = TRANSPORT.read().as_ref() {
        return h(req);
    }
    let i = req.url.find(':').unwrap_or(0);
    let scheme = req.url[..i].to_lowercase();
    if scheme != "http" && scheme != "https" {
        return HttpOutcome::UrlError(format!("unknown url type: {scheme}"));
    }
    let rest = &req.url[i + 1..];
    let host_ok = rest
        .strip_prefix("//")
        .map(|r| !r.split(['/', '?', '#']).next().unwrap_or("").is_empty())
        .unwrap_or(false);
    if !host_ok {
        return HttpOutcome::UrlError("no host given".into());
    }
    ureq_send(req)
}

fn ureq_send(req: &HttpRequest) -> HttpOutcome {
    use ureq::Error as E;
    use ureq::Timeout as T;
    let t = Some(crate::timer::secs(req.timeout));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_resolve(t)
        .timeout_connect(t)
        .timeout_per_read(t)
        .timeout_per_write(t)
        .build()
        .into();
    let map_err = |e: E| -> HttpOutcome {
        match e {
            E::Timeout(T::Resolve | T::Connect | T::SendRequest | T::SendBody | T::PerWrite) => {
                HttpOutcome::UrlError("timed out".into())
            }
            E::Timeout(_) => HttpOutcome::Other("timed out".into()),
            E::HostNotFound => HttpOutcome::UrlError("[Errno 11001] getaddrinfo failed".into()),
            E::Io(e) => HttpOutcome::UrlError(e.to_string()),
            E::ConnectionFailed => HttpOutcome::UrlError(E::ConnectionFailed.to_string()),
            other => {
                let text = other.to_string();
                if text.to_lowercase().contains("tls")
                    || text.to_lowercase().contains("certificate")
                {
                    HttpOutcome::UrlError(text)
                } else {
                    HttpOutcome::Other(text)
                }
            }
        }
    };
    let result = if req.method == "POST" {
        let mut b = agent.post(&req.url);
        for (k, v) in &req.headers {
            b = b.header(k.as_str(), v.as_str());
        }
        b.send(req.body.as_deref().unwrap_or(&[]))
    } else {
        let mut b = agent.get(&req.url);
        for (k, v) in &req.headers {
            b = b.header(k.as_str(), v.as_str());
        }
        b.call()
    };
    let resp = match result {
        Ok(r) => r,
        Err(e) => return map_err(e),
    };
    let status = resp.status().as_u16();
    let body = resp.into_body().into_with_config().limit(u64::MAX);
    if (200..300).contains(&status) {
        if !req.read_body {
            return HttpOutcome::Response {
                status,
                body: Vec::new(),
            };
        }
        match body.read_to_vec() {
            Ok(b) => HttpOutcome::Response { status, body: b },
            Err(E::Timeout(_)) => HttpOutcome::Other("timed out".into()),
            Err(e) => HttpOutcome::Other(e.to_string()),
        }
    } else {
        HttpOutcome::HttpError {
            code: status,
            body: body.read_to_vec().unwrap_or_default(),
        }
    }
}

/// `_extract_error_message(body_bytes)`: Gemini and OpenAI both wrap failures as
/// `{"error": {"message": ...}}`.
pub fn extract_error_message(body: &[u8]) -> String {
    let text = decode_ignore(body);
    if let Ok(data) = json_loads(&text)
        && let Ok(err) = pget(&data, "error")
    {
        let err = err.filter(|e| e.truthy());
        let empty = PyValue::Dict(PyDict::new());
        let err = err.unwrap_or(&empty);
        if let Ok(Some(msg)) = pget(err, "message")
            && msg.truthy()
        {
            return msg.py_str();
        }
    }
    py_strip(&text).to_string()
}

fn or_http(msg: String, code: u16) -> String {
    if msg.is_empty() {
        format!("HTTP {code}.")
    } else {
        msg
    }
}

/// The shared `URLError` / `Exception` arms of the describe and OpenAI requests.
fn request_failure(o: HttpOutcome) -> String {
    match o {
        HttpOutcome::HttpError { code, body } => or_http(extract_error_message(&body), code),
        HttpOutcome::UrlError(reason) => {
            if reason.to_lowercase().contains("timed out") {
                "Request timed out.".into()
            } else {
                format!("Network error: {reason}")
            }
        }
        HttpOutcome::Other(e) => {
            if e.to_lowercase().contains("timed out") {
                "Request timed out.".into()
            } else {
                format!("Request failed: {e}")
            }
        }
        HttpOutcome::Response { .. } => String::new(),
    }
}

fn parse_body(body: &[u8]) -> Result<PyValue, String> {
    json_loads(&decode_ignore(body)).map_err(|e| format!("Could not parse API response: {e}"))
}

/// The extra request parameters (`**opts`). `None` is "not passed".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Opts {
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub reasoning_effort: Option<String>,
    pub detail: Option<String>,
}

impl Opts {
    /// From `extra_args_for`'s `arg -> value` map.
    pub fn from_extra_args(args: &indexmap::IndexMap<&'static str, String>) -> Opts {
        Opts {
            model: None,
            base_url: args.get("base_url").cloned(),
            reasoning_effort: args.get("reasoning_effort").cloned(),
            detail: args.get("detail").cloned(),
        }
    }
}

/// `(model or "").lower()` for a model that may not be a str.
fn model_lower(model: &PyValue) -> Result<String, String> {
    match model {
        PyValue::Str(s) => Ok(s.to_lowercase()),
        m if !m.truthy() => Ok(String::new()),
        m => Err(no_attr(m, "lower")),
    }
}

// ---- Gemini ------------------------------------------------------------------------------

/// `_gemini_thinking_config(model)`: minimise (or disable) thinking.
pub fn gemini_thinking_config(model_l: &str) -> Option<Value> {
    if model_l.contains("gemini-3") {
        return Some(json!({"thinkingLevel": "low"}));
    }
    if model_l.contains("gemini-2.5") && (model_l.contains("flash") || model_l.contains("lite")) {
        return Some(json!({"thinkingBudget": 0}));
    }
    None
}

/// `_gemini_is_thinking_model(model)`.
pub fn gemini_is_thinking_model(model_l: &str) -> bool {
    model_l.contains("gemini-3") || model_l.contains("gemini-2.5")
}

/// The `generateContent` request `_gemini_describe` sends.
pub fn gemini_describe_request(
    png: &[u8],
    model: &str,
    key: &str,
    timeout: f64,
) -> Result<HttpRequest, String> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let mut gen_config = serde_json::Map::new();
    gen_config.insert("maxOutputTokens".into(), json!(MAX_OUTPUT_TOKENS));
    let ml = model.to_lowercase();
    if let Some(t) = gemini_thinking_config(&ml) {
        gen_config.insert("thinkingConfig".into(), t);
    }
    if gemini_is_thinking_model(&ml) {
        gen_config.insert(
            "maxOutputTokens".into(),
            json!(MAX_OUTPUT_TOKENS + THINKING_HEADROOM_TOKENS),
        );
    }
    let payload = json!({
        "systemInstruction": {"parts": [{"text": SYSTEM_PROMPT}]},
        "contents": [{"parts": [{"inline_data": {"mime_type": "image/png", "data": b64}}]}],
        "generationConfig": Value::Object(gen_config),
    });
    let url = request_url(&format!(
        "{GEMINI_BASE}/{model}:generateContent?key={}",
        py_quote(key)
    ))?;
    Ok(HttpRequest {
        method: "POST",
        url,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: Some(py_json_dumps(&payload).into_bytes()),
        timeout,
        read_body: true,
    })
}

fn gemini_validate_key(api_key: &str, timeout: f64) -> Result<(bool, Option<String>), String> {
    let key = py_strip(api_key);
    if key.is_empty() {
        return Ok((false, Some("No API key provided.".into())));
    }
    let url = request_url(&format!("{GEMINI_BASE}/models?key={}", py_quote(key)))?;
    let req = HttpRequest {
        method: "GET",
        url,
        headers: Vec::new(),
        body: None,
        timeout,
        read_body: false,
    };
    Ok(match urlopen(&req) {
        HttpOutcome::Response { status, .. } => {
            if (200..300).contains(&status) {
                (true, None)
            } else {
                (false, Some(format!("Unexpected status {status}.")))
            }
        }
        HttpOutcome::HttpError { code, body } => {
            (false, Some(or_http(extract_error_message(&body), code)))
        }
        HttpOutcome::UrlError(reason) => (false, Some(format!("Network error: {reason}"))),
        HttpOutcome::Other(e) => (false, Some(format!("Validation failed: {e}"))),
    })
}

fn gemini_describe(
    png: &[u8],
    model: &PyValue,
    api_key: &str,
    timeout: f64,
) -> Result<(Option<String>, Option<String>), String> {
    let key = py_strip(api_key);
    if key.is_empty() {
        return Ok((None, Some("No API key set.".into())));
    }
    let model = if model.truthy() {
        model.clone()
    } else {
        PyValue::Str(GEMINI_DEFAULT_MODEL.into())
    };
    model_lower(&model)?;
    let model = model.py_str();
    let req = gemini_describe_request(png, &model, key, timeout)?;
    let body = match urlopen(&req) {
        HttpOutcome::Response { body, .. } => body,
        other => return Ok((None, Some(request_failure(other)))),
    };
    let result = match parse_body(&body) {
        Ok(r) => r,
        Err(e) => return Ok((None, Some(e))),
    };

    // A top-level promptFeedback block usually means the request was blocked.
    let empty = PyValue::Dict(PyDict::new());
    let feedback = pget_truthy(&result, "promptFeedback")?.unwrap_or(&empty);
    let block_reason = pget(feedback, "blockReason")?.cloned();

    let Some(candidates) = pget_truthy(&result, "candidates")? else {
        if let Some(b) = block_reason.filter(|b| b.truthy()) {
            return Ok((
                None,
                Some(format!(
                    "Request blocked by safety filters ({}).",
                    b.py_str()
                )),
            ));
        }
        return Ok((None, Some("API returned no candidates.".into())));
    };
    let cand = py_first(candidates)?;
    let content = pget_truthy(&cand, "content")?.unwrap_or(&empty);
    let parts = match pget_truthy(content, "parts")? {
        Some(p) => py_iter(p)?,
        None => Vec::new(),
    };
    // Skip "thought" parts (model reasoning): only the answer parts are spoken.
    let mut texts = Vec::new();
    for p in &parts {
        if pget(p, "thought")?.is_some_and(|t| t.truthy()) {
            continue;
        }
        texts.push(
            pget(p, "text")?
                .cloned()
                .unwrap_or(PyValue::Str(String::new())),
        );
    }
    let text = py_strip(&py_join(&texts)?).to_string();
    if text.is_empty() {
        let finish = pget_truthy(&cand, "finishReason")?
            .map(|f| f.py_str())
            .unwrap_or_else(|| "unknown".into());
        if finish == "MAX_TOKENS" {
            return Ok((
                None,
                Some(
                    "The model used its entire token budget before producing a \
                     description (finishReason: MAX_TOKENS)."
                        .into(),
                ),
            ));
        }
        return Ok((
            None,
            Some(format!(
                "API returned an empty description (finishReason: {finish})."
            )),
        ));
    }
    Ok((Some(text), None))
}

// ---- OpenAI ------------------------------------------------------------------------------

/// `_openai_base(base_url)`: a pasted URL often carries a trailing slash.
pub fn openai_base(base_url: Option<&str>) -> String {
    let base = py_strip(base_url.unwrap_or("")).trim_end_matches('/');
    if base.is_empty() {
        OPENAI_BASE.to_string()
    } else {
        base.to_string()
    }
}

/// `_openai_is_reasoning_model(model)`: gpt-4o/4.1 reject a `reasoning` block.
pub fn openai_is_reasoning_model(model_l: &str) -> bool {
    model_l.starts_with("gpt-5") || model_l.starts_with("o3")
}

/// `_openai_reasoning_config(model, effort)`.
pub fn openai_reasoning_config(model_l: &str, effort: Option<&str>) -> Option<Value> {
    if !openai_is_reasoning_model(model_l) {
        return None;
    }
    let e = effort.filter(|e| !e.is_empty()).unwrap_or("low");
    let mut eff = py_strip(e).to_lowercase();
    if !OPENAI_REASONING_EFFORTS.iter().any(|(v, _)| *v == eff) {
        eff = "low".into();
    }
    if eff == "none" {
        // "none" means "don't think": leave the block off entirely.
        return None;
    }
    Some(json!({"effort": eff}))
}

fn openai_request_for(
    url: &str,
    api_key: &str,
    payload: Option<&Value>,
    timeout: f64,
) -> HttpRequest {
    let mut headers = vec![("Authorization".to_string(), format!("Bearer {api_key}"))];
    let body = payload.map(|p| {
        headers.push(("Content-Type".into(), "application/json".into()));
        py_json_dumps(p).into_bytes()
    });
    HttpRequest {
        method: if body.as_ref().is_some_and(|b| !b.is_empty()) {
            "POST"
        } else {
            "GET"
        },
        url: url.to_string(),
        headers,
        body,
        timeout,
        read_body: true,
    }
}

/// `_openai_request(url, api_key, payload, timeout)`: `(parsed_body, error)`.
fn openai_request(
    url: &str,
    api_key: &str,
    payload: Option<&Value>,
    timeout: f64,
) -> Result<Result<PyValue, String>, String> {
    let url = request_url(url)?;
    let req = openai_request_for(&url, api_key, payload, timeout);
    Ok(match urlopen(&req) {
        HttpOutcome::Response { body, .. } => parse_body(&body),
        other => Err(request_failure(other)),
    })
}

/// The `/responses` payload `_openai_validate_key` sends.
pub fn openai_validate_payload(model: Option<&str>) -> Value {
    json!({
        "model": model.filter(|m| !m.is_empty()).unwrap_or(OPENAI_DEFAULT_MODEL),
        "input": "Say OK.",
        // 16 is the floor the API accepts.
        "max_output_tokens": 16,
    })
}

fn openai_validate_key(
    api_key: &str,
    timeout: f64,
    opts: &Opts,
) -> Result<(bool, Option<String>), String> {
    let key = py_strip(api_key);
    if key.is_empty() {
        return Ok((false, Some("No API key provided.".into())));
    }
    let payload = openai_validate_payload(opts.model.as_deref());
    let url = format!("{}/responses", openai_base(opts.base_url.as_deref()));
    Ok(match openai_request(&url, key, Some(&payload), timeout)? {
        Err(e) => (false, Some(e)),
        Ok(_) => (true, None),
    })
}

/// The `/responses` payload `_openai_describe` sends.
pub fn openai_describe_payload(
    png: &[u8],
    model: &str,
    reasoning_effort: Option<&str>,
    detail: Option<&str>,
) -> Value {
    let d = detail.filter(|d| !d.is_empty()).unwrap_or("auto");
    let mut det = py_strip(d).to_lowercase();
    if !OPENAI_DETAIL_LEVELS.iter().any(|(v, _)| *v == det) {
        det = "auto".into();
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let mut payload = json!({
        "model": model,
        // The system prompt goes in the top-level `instructions`, which takes priority
        // over anything in `input`.
        "instructions": SYSTEM_PROMPT,
        "input": [{
            "role": "user",
            "content": [
                {"type": "input_text", "text": "Describe what is on screen."},
                // Responses takes image_url as a plain data-URL string, not an object.
                {"type": "input_image", "image_url": format!("data:image/png;base64,{b64}"), "detail": det},
            ],
        }],
        "max_output_tokens": MAX_OUTPUT_TOKENS,
    });
    if let Some(reasoning) = openai_reasoning_config(&model.to_lowercase(), reasoning_effort)
        && let Value::Object(m) = &mut payload
    {
        m.insert("reasoning".into(), reasoning);
        // Reasoning tokens count against max_output_tokens too.
        m.insert(
            "max_output_tokens".into(),
            json!(MAX_OUTPUT_TOKENS + THINKING_HEADROOM_TOKENS),
        );
    }
    payload
}

/// `_openai_text_from_response(result)`: only output_text parts of message items (a
/// reasoning model puts a `reasoning` item first).
fn openai_text_from_response(result: &PyValue) -> Result<String, String> {
    let mut chunks = Vec::new();
    let output = match pget_truthy(result, "output")? {
        Some(o) => py_iter(o)?,
        None => Vec::new(),
    };
    for item in &output {
        if !is_str(pget(item, "type")?, "message") {
            continue;
        }
        let content = match pget_truthy(item, "content")? {
            Some(c) => py_iter(c)?,
            None => Vec::new(),
        };
        for part in &content {
            if is_str(pget(part, "type")?, "output_text") {
                chunks.push(
                    pget(part, "text")?
                        .cloned()
                        .unwrap_or(PyValue::Str(String::new())),
                );
            }
        }
    }
    Ok(py_strip(&py_join(&chunks)?).to_string())
}

/// `_openai_refusal(result)`.
fn openai_refusal(result: &PyValue) -> Result<Option<String>, String> {
    let output = match pget_truthy(result, "output")? {
        Some(o) => py_iter(o)?,
        None => Vec::new(),
    };
    for item in &output {
        let content = match pget_truthy(item, "content")? {
            Some(c) => py_iter(c)?,
            None => Vec::new(),
        };
        for part in &content {
            if is_str(pget(part, "type")?, "refusal") {
                return Ok(Some(
                    pget_truthy(part, "refusal")?
                        .map(|r| r.py_str())
                        .unwrap_or_else(|| "no reason given".into()),
                ));
            }
        }
    }
    Ok(None)
}

fn openai_describe(
    png: &[u8],
    model: &PyValue,
    api_key: &str,
    timeout: f64,
    opts: &Opts,
) -> Result<(Option<String>, Option<String>), String> {
    let key = py_strip(api_key);
    if key.is_empty() {
        return Ok((None, Some("No API key set.".into())));
    }
    let model = if model.truthy() {
        model.clone()
    } else {
        PyValue::Str(OPENAI_DEFAULT_MODEL.into())
    };
    model_lower(&model)?;
    let model = model.py_str();
    let payload = openai_describe_payload(
        png,
        &model,
        Some(opts.reasoning_effort.as_deref().unwrap_or("low")),
        Some(opts.detail.as_deref().unwrap_or("auto")),
    );
    let url = format!("{}/responses", openai_base(opts.base_url.as_deref()));
    let result = match openai_request(&url, key, Some(&payload), timeout)? {
        Err(e) => return Ok((None, Some(e))),
        Ok(r) => r,
    };
    let text = openai_text_from_response(&result)?;
    if text.is_empty() {
        if is_str(pget(&result, "status")?, "incomplete") {
            let empty = PyValue::Dict(PyDict::new());
            let details = pget_truthy(&result, "incomplete_details")?.unwrap_or(&empty);
            let reason = pget_truthy(details, "reason")?
                .map(|r| r.py_str())
                .unwrap_or_else(|| "unknown".into());
            if reason == "max_output_tokens" {
                return Ok((
                    None,
                    Some(
                        "The model used its entire token budget before producing a \
                         description (incomplete: max_output_tokens)."
                            .into(),
                    ),
                ));
            }
            return Ok((
                None,
                Some(format!("API returned an incomplete response ({reason}).")),
            ));
        }
        if let Some(refusal) = openai_refusal(&result)? {
            return Ok((
                None,
                Some(format!(
                    "The model declined to describe the image: {refusal}"
                )),
            ));
        }
        return Ok((None, Some("API returned an empty description.".into())));
    }
    Ok((Some(text), None))
}

/// The OpenAI describe request, built exactly as `_openai_describe` builds it (for tests
/// and diagnostics).
pub fn openai_describe_request(
    png: &[u8],
    model: &str,
    key: &str,
    timeout: f64,
    opts: &Opts,
) -> Result<HttpRequest, String> {
    let payload = openai_describe_payload(
        png,
        model,
        Some(opts.reasoning_effort.as_deref().unwrap_or("low")),
        Some(opts.detail.as_deref().unwrap_or("auto")),
    );
    let url = request_url(&format!(
        "{}/responses",
        openai_base(opts.base_url.as_deref())
    ))?;
    Ok(openai_request_for(
        &url,
        py_strip(key),
        Some(&payload),
        timeout,
    ))
}

// ---- Dispatch ----------------------------------------------------------------------------

/// `validate_api_key(api_key, provider, timeout, **opts)`. `Err` is an exception the
/// Python lets escape (a base URL with no scheme makes `Request` raise `ValueError`).
pub fn try_validate_api_key(
    api_key: &str,
    provider: &str,
    timeout: f64,
    opts: &Opts,
) -> Result<(bool, Option<String>), String> {
    match ai_registry::provider_info(provider).id {
        "openai" => openai_validate_key(api_key, timeout, opts),
        _ => gemini_validate_key(api_key, timeout),
    }
}

/// `ai_describer.validate_api_key(...)` for the AI Describer tab (U2 calls it):
/// `(ok, error_message)`, with no model and no extras. U2 should call
/// [`try_validate_api_key`] with the selected model and the provider's extras, as
/// `config_ui.py` does; an escaped exception is reported here as a failed validation.
pub fn validate_api_key(provider: &str, api_key: &str) -> (bool, Option<String>) {
    match try_validate_api_key(api_key, provider, VALIDATE_TIMEOUT_SEC, &Opts::default()) {
        Ok(r) => r,
        Err(e) => (false, Some(e)),
    }
}

/// `describe_image(png, model, api_key, provider, timeout, **opts)`: `(text, error)`.
pub fn describe_image(
    png: &[u8],
    model: &PyValue,
    api_key: &str,
    provider: &str,
    timeout: f64,
    opts: &Opts,
) -> Result<(Option<String>, Option<String>), String> {
    match ai_registry::provider_info(provider).id {
        "openai" => openai_describe(png, model, api_key, timeout, opts),
        _ => gemini_describe(png, model, api_key, timeout),
    }
}

// ---- Log ---------------------------------------------------------------------------------

fn local_stamp() -> String {
    #[cfg(windows)]
    {
        // SAFETY: GetLocalTime fills a caller-owned SYSTEMTIME and cannot fail.
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
        )
    }
    #[cfg(not(windows))]
    {
        "1970-01-01 00:00:00".to_string()
    }
}

/// `_append_log(prefix, text)`. Python writes in text mode, so every `\n` is `\r\n`.
fn append_log(prefix: &str, text: &str) {
    let path = log_path();
    let _ = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let line = format!("[{}] {prefix}: {text}\n", local_stamp()).replace('\n', "\r\n");
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?
            .write_all(line.as_bytes())
    })();
}

/// `log_description(text)`.
pub fn log_description(text: &str) {
    append_log("DESCRIPTION", py_strip(&text.replace('\n', " ")));
}

/// `log_error(text)`.
pub fn log_error(text: &str) {
    append_log("ERROR", py_strip(text));
}

// ---- The worker --------------------------------------------------------------------------

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_string())
}

/// `extra_args_for` with the message of the AttributeError it can raise.
fn extra_args(provider: &str, settings: &PyDict) -> Result<Opts, String> {
    match ai_registry::extra_args_for(provider, Some(settings)) {
        Ok(a) => Ok(Opts::from_extra_args(&a)),
        Err(_) => {
            let bad = ai_registry::extras_for(provider)
                .iter()
                .filter(|ex| ex.kind == ExtraKind::Text)
                .filter_map(|ex| settings.get(ex.key))
                .find(|v| v.truthy() && v.as_str().is_none());
            Err(bad.map_or_else(|| "AttributeError".to_string(), |v| no_attr(v, "strip")))
        }
    }
}

fn worker_body() -> Result<(), String> {
    let (settings, provider_cfg, disable_toggle) = {
        let st = state();
        (
            from_json_map(&st.cfg.ai_describer_settings),
            st.cfg.ai_describer_provider.clone(),
            st.cfg.ai_describer_disable_ui_toggle,
        )
    };
    let provider = if provider_cfg.is_empty() {
        DEFAULT_PROVIDER.to_string()
    } else {
        provider_cfg
    };
    let (key_cfg, model_cfg) = ai_registry::config_keys_for(&provider);
    // Stored DPAPI-sealed; unsealed here so the plaintext lives only in this frame and
    // never in `ai_describer_settings`.
    let stored = settings
        .get(key_cfg)
        .cloned()
        .unwrap_or(PyValue::Str(String::new()));
    let api_key: PyValue = if !stored.truthy() {
        PyValue::Str(String::new())
    } else if let PyValue::Str(s) = &stored {
        match beam_base::secretstore::unprotect(s) {
            Some(k) => PyValue::Str(k),
            None => {
                // Sealed, but not by this Windows account: distinct from "no key set".
                let msg = "The stored API key could not be decrypted on this Windows account. \
                           Set it again in the AI Describer tab.";
                log_error(msg);
                say(msg, true, false, "python");
                return Ok(());
            }
        }
    } else {
        stored.clone()
    };
    let model = settings
        .get(model_cfg)
        .filter(|m| m.truthy())
        .cloned()
        .unwrap_or_else(|| PyValue::Str(ai_registry::default_model_for(&provider).into()));
    let opts = extra_args(&provider, &settings)?;
    let toggle_ui = !disable_toggle;

    let key_text = match &api_key {
        PyValue::Str(s) => s.clone(),
        other => return Err(no_attr(other, "strip")),
    };
    if py_strip(&key_text).is_empty() {
        let display = ai_registry::provider_display_name(&provider);
        let msg = format!("No {display} API key set. Configure it in the AI Describer tab.");
        log_error(&msg);
        say(&msg, true, false, "python");
        return Ok(());
    }

    // Capture the scene with the game UI hidden around the grab; the UI is always restored.
    let captured = catch_unwind(AssertUnwindSafe(|| {
        crate::capture::capture_scene_png(toggle_ui, crate::capture::UI_HIDE_SETTLE_S, false)
    }));
    let png = match captured {
        Ok(Ok((png, _meta))) => png,
        Ok(Err(e)) => {
            let m = format!("Screenshot failed: {e}");
            log_error(&m);
            say(&m, true, false, "python");
            return Ok(());
        }
        Err(p) => {
            let m = format!("Screenshot failed: {}", panic_message(&*p));
            log_error(&m);
            say(&m, true, false, "python");
            return Ok(());
        }
    };

    say("Description in progress.", true, true, "python");
    let (text, err) = describe_image(&png, &model, &key_text, &provider, 60.0, &opts)?;
    match text.filter(|t| !t.is_empty()) {
        Some(t) => {
            log_description(&t);
            say(&t, true, false, "python");
        }
        None => {
            let err = err
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "Description failed.".into());
            log_error(&err);
            say(&format!("Description failed. {err}"), true, false, "python");
        }
    }
    Ok(())
}

/// `_ai_describe_worker(audio_controller)`: hide the UI, screenshot, describe, speak.
/// Runs on its own thread so the keyboard worker returns immediately.
pub fn ai_describe_worker(audio_controller: Option<Arc<AudioController>>) {
    let _ = audio_controller;
    let failure = match catch_unwind(AssertUnwindSafe(worker_body)) {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e),
        Err(p) => Some(panic_message(&*p)),
    };
    if let Some(e) = failure {
        log_error(&format!("Unexpected error: {e}"));
        say("Description failed.", true, false, "python");
    }
    *DESCRIBER_BUSY.lock() = false;
}

/// `_trigger_ai_describe(audio_controller)`: a double-press while a request is in flight
/// plays a low FM error buzz and is otherwise ignored (no speech), per design.
pub fn trigger_ai_describe(audio_controller: Option<Arc<AudioController>>) {
    {
        let mut busy = DESCRIBER_BUSY.lock();
        if *busy {
            if let Some(ac) = &audio_controller {
                ac.trigger_describe_error_buzz();
            }
            return;
        }
        *busy = true;
    }
    crate::timer::spawn_daemon("Thread", move || ai_describe_worker(audio_controller));
}
