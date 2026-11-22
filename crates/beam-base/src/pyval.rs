//! Python values as `json.load` produces them, with Python's coercion rules.
//!
//! The config file is read with `json.load`, mutated as a `dict` and written with
//! `json.dump(cfg, f, indent=2, ensure_ascii=False)`. Three things about that pipeline
//! cannot be carried by `serde_json::Value`, and each one decides what a hand-edited
//! config does:
//!
//! - Python's `json` accepts `NaN`, `Infinity` and `-Infinity`, and an out-of-range
//!   float literal (`1e400`) becomes infinity rather than an error. `bool(nan)` is
//!   `True`, `int(inf)` raises `OverflowError` -- which the config loaders do *not*
//!   catch, so it takes the corruption path.
//! - Python's ints are unbounded: `int(1e300)` is a 301-digit integer that is kept
//!   and written back.
//! - `str(x)` of a list or dict is its `repr`, and that string is what lands in a
//!   string-typed setting.
//!
//! So the config is held as [`PyValue`] and converted to `serde_json` only at the
//! edges ([`to_json`], [`from_json`]).

use std::cmp::Ordering;
use std::fmt;

use indexmap::IndexMap;

use crate::pyfmt::{py_float_parse, py_float_str, py_strip};

/// A Python `dict` with string keys, insertion ordered.
pub type PyDict = IndexMap<String, PyValue>;

/// The exception a Python conversion raises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyExc {
    TypeError,
    ValueError,
    OverflowError,
    AttributeError,
}

impl fmt::Display for PyExc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PyExc::TypeError => "TypeError",
            PyExc::ValueError => "ValueError",
            PyExc::OverflowError => "OverflowError",
            PyExc::AttributeError => "AttributeError",
        })
    }
}

impl std::error::Error for PyExc {}

/// CPython's default `sys.int_info.str_digits_check_threshold`: `int()` of a decimal
/// string longer than this raises `ValueError`.
pub const INT_MAX_STR_DIGITS: usize = 4300;

/// A Python `int`: an `i64` when it fits, else its exact decimal spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PyInt {
    Small(i64),
    /// Canonical decimal (optional `-`, no leading zeros), always outside `i64`.
    Big(String),
}

impl PyInt {
    /// From a sign and ASCII decimal digits (leading zeros allowed).
    pub fn from_decimal(neg: bool, digits: &str) -> PyInt {
        let trimmed = digits.trim_start_matches('0');
        if trimmed.is_empty() {
            return PyInt::Small(0);
        }
        let signed = if neg {
            format!("-{trimmed}")
        } else {
            trimmed.to_string()
        };
        match signed.parse::<i64>() {
            Ok(v) => PyInt::Small(v),
            Err(_) => PyInt::Big(signed),
        }
    }

    /// `int(x)` for a float: truncate toward zero, exactly.
    pub fn from_f64_trunc(x: f64) -> Result<PyInt, PyExc> {
        if x.is_nan() {
            return Err(PyExc::ValueError);
        }
        if x.is_infinite() {
            return Err(PyExc::OverflowError);
        }
        let t = x.trunc();
        if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&t) {
            return Ok(PyInt::Small(t as i64));
        }
        // `{:.0}` of an integral double is its exact decimal expansion.
        let text = format!("{:.0}", t);
        let (neg, digits) = match text.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, text.as_str()),
        };
        Ok(PyInt::from_decimal(neg, digits))
    }

    /// `float(n)`: correctly rounded; `OverflowError` when too large.
    pub fn to_f64(&self) -> Result<f64, PyExc> {
        match self {
            PyInt::Small(v) => Ok(*v as f64),
            PyInt::Big(s) => {
                let v = s.parse::<f64>().map_err(|_| PyExc::OverflowError)?;
                if v.is_finite() {
                    Ok(v)
                } else {
                    Err(PyExc::OverflowError)
                }
            }
        }
    }

    pub fn to_i64(&self) -> Option<i64> {
        match self {
            PyInt::Small(v) => Some(*v),
            PyInt::Big(_) => None,
        }
    }

    pub fn is_zero(&self) -> bool {
        matches!(self, PyInt::Small(0))
    }

    fn is_negative(&self) -> bool {
        match self {
            PyInt::Small(v) => *v < 0,
            PyInt::Big(s) => s.starts_with('-'),
        }
    }

    /// Compare with an `i64` (a `Big` is always outside `i64`, so its sign decides).
    pub fn cmp_i64(&self, other: i64) -> Ordering {
        match self {
            PyInt::Small(v) => v.cmp(&other),
            PyInt::Big(_) => {
                if self.is_negative() {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
        }
    }

    /// Python's `max(lo, min(hi, n))` on ints.
    pub fn clamp_py(&self, lo: i64, hi: i64) -> i64 {
        let m = match self.to_i64() {
            Some(v) if v < hi => v,
            Some(_) => hi,
            // A negative Big is below any i64, so min() keeps it and max() takes lo.
            None if self.is_negative() => return lo,
            None => hi,
        };
        if m > lo { m } else { lo }
    }
}

impl fmt::Display for PyInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PyInt::Small(v) => write!(f, "{v}"),
            PyInt::Big(s) => f.write_str(s),
        }
    }
}

/// A value `json.load` can produce.
#[derive(Debug, Clone, PartialEq)]
pub enum PyValue {
    None,
    Bool(bool),
    Int(PyInt),
    Float(f64),
    Str(String),
    List(Vec<PyValue>),
    Dict(PyDict),
}

impl From<bool> for PyValue {
    fn from(v: bool) -> Self {
        PyValue::Bool(v)
    }
}
impl From<i64> for PyValue {
    fn from(v: i64) -> Self {
        PyValue::Int(PyInt::Small(v))
    }
}
impl From<f64> for PyValue {
    fn from(v: f64) -> Self {
        PyValue::Float(v)
    }
}
impl From<&str> for PyValue {
    fn from(v: &str) -> Self {
        PyValue::Str(v.to_string())
    }
}
impl From<String> for PyValue {
    fn from(v: String) -> Self {
        PyValue::Str(v)
    }
}
impl From<PyInt> for PyValue {
    fn from(v: PyInt) -> Self {
        PyValue::Int(v)
    }
}

impl PyValue {
    /// `bool(x)`.
    pub fn truthy(&self) -> bool {
        match self {
            PyValue::None => false,
            PyValue::Bool(b) => *b,
            PyValue::Int(i) => !i.is_zero(),
            PyValue::Float(f) => *f != 0.0,
            PyValue::Str(s) => !s.is_empty(),
            PyValue::List(l) => !l.is_empty(),
            PyValue::Dict(d) => !d.is_empty(),
        }
    }

    /// `int(x)`.
    pub fn py_int(&self) -> Result<PyInt, PyExc> {
        match self {
            PyValue::Bool(b) => Ok(PyInt::Small(*b as i64)),
            PyValue::Int(i) => Ok(i.clone()),
            PyValue::Float(f) => PyInt::from_f64_trunc(*f),
            PyValue::Str(s) => py_int_parse(s).ok_or(PyExc::ValueError),
            PyValue::None | PyValue::List(_) | PyValue::Dict(_) => Err(PyExc::TypeError),
        }
    }

    /// `float(x)`.
    pub fn py_float(&self) -> Result<f64, PyExc> {
        match self {
            PyValue::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            PyValue::Int(i) => i.to_f64(),
            PyValue::Float(f) => Ok(*f),
            PyValue::Str(s) => py_float_parse(s).ok_or(PyExc::ValueError),
            PyValue::None | PyValue::List(_) | PyValue::Dict(_) => Err(PyExc::TypeError),
        }
    }

    /// `str(x)`.
    pub fn py_str(&self) -> String {
        match self {
            PyValue::Str(s) => s.clone(),
            other => other.py_repr(),
        }
    }

    /// `repr(x)`.
    pub fn py_repr(&self) -> String {
        match self {
            PyValue::None => "None".to_string(),
            PyValue::Bool(true) => "True".to_string(),
            PyValue::Bool(false) => "False".to_string(),
            PyValue::Int(i) => i.to_string(),
            PyValue::Float(f) => py_float_str(*f),
            PyValue::Str(s) => py_str_repr(s),
            PyValue::List(items) => {
                let inner: Vec<String> = items.iter().map(PyValue::py_repr).collect();
                format!("[{}]", inner.join(", "))
            }
            PyValue::Dict(d) => {
                let inner: Vec<String> = d
                    .iter()
                    .map(|(k, v)| format!("{}: {}", py_str_repr(k), v.py_repr()))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
        }
    }

    /// `isinstance(x, bool)` and its value.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            PyValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The string, if this is a `str`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PyValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The value as an `i64`, if it is an `int` (not a bool) that fits.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            PyValue::Int(i) => i.to_i64(),
            _ => None,
        }
    }

    /// The value as an `f64`, if it is a `float`, or an `int` (not a bool).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            PyValue::Float(f) => Some(*f),
            PyValue::Int(i) => i.to_f64().ok(),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&PyDict> {
        match self {
            PyValue::Dict(d) => Some(d),
            _ => None,
        }
    }
}

/// Python's `max(a, b)` on floats: `a` unless `b > a` (so NaN placement matters).
pub fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// Python's `min(a, b)` on floats: `a` unless `b < a`.
pub fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// Python's `max(lo, min(hi, x))`, the clamp idiom the config uses throughout.
pub fn py_clamp(x: f64, lo: f64, hi: f64) -> f64 {
    py_max(lo, py_min(hi, x))
}

/// Python's `int(s)` for a string, base 10: surrounding whitespace, a sign, `_` between
/// digits. `None` where Python raises `ValueError`.
///
/// Known difference: Python also accepts non-ASCII decimal digits; this does not.
pub fn py_int_parse(s: &str) -> Option<PyInt> {
    let t = py_strip(s);
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let b = rest.as_bytes();
    if b.is_empty() || !b[0].is_ascii_digit() {
        return None;
    }
    let mut digits = String::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() {
            digits.push(c as char);
            i += 1;
        } else if c == b'_' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            i += 1;
        } else {
            return None;
        }
    }
    if digits.len() > INT_MAX_STR_DIGITS {
        return None;
    }
    Some(PyInt::from_decimal(neg, &digits))
}

/// Python's `str.isprintable()` for one character, as `repr` uses it.
///
/// Every category Python treats as non-printable is covered except Cn (unassigned
/// code points), which would need the Unicode database; an unassigned character is
/// printed as itself here where Python escapes it.
fn py_is_printable(c: char) -> bool {
    let u = c as u32;
    if u < 0x20 || (0x7f..=0x9f).contains(&u) {
        return false; // Cc
    }
    if u == 0x20 {
        return true;
    }
    // Zs other than space, Zl, Zp.
    if matches!(
        u,
        0xa0 | 0x1680 | 0x202f | 0x205f | 0x3000 | 0x2028 | 0x2029
    ) || (0x2000..=0x200a).contains(&u)
    {
        return false;
    }
    // Cf.
    const CF: &[(u32, u32)] = &[
        (0xad, 0xad),
        (0x600, 0x605),
        (0x61c, 0x61c),
        (0x6dd, 0x6dd),
        (0x70f, 0x70f),
        (0x890, 0x891),
        (0x8e2, 0x8e2),
        (0x180e, 0x180e),
        (0x200b, 0x200f),
        (0x202a, 0x202e),
        (0x2060, 0x2064),
        (0x2066, 0x206f),
        (0xfeff, 0xfeff),
        (0xfff9, 0xfffb),
        (0x110bd, 0x110bd),
        (0x110cd, 0x110cd),
        (0x13430, 0x1343f),
        (0x1bca0, 0x1bca3),
        (0x1d173, 0x1d17a),
        (0xe0001, 0xe0001),
        (0xe0020, 0xe007f),
    ];
    if CF.iter().any(|&(a, b)| (a..=b).contains(&u)) {
        return false;
    }
    // Co (private use). Cs cannot occur in a Rust char.
    if (0xe000..=0xf8ff).contains(&u)
        || (0xf0000..=0xffffd).contains(&u)
        || (0x100000..=0x10fffd).contains(&u)
    {
        return false;
    }
    // Noncharacters are Cn.
    if (0xfdd0..=0xfdef).contains(&u) || (u & 0xfffe) == 0xfffe {
        return false;
    }
    true
}

/// Python's `repr(s)` for a `str`.
pub fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x7f && (c as u32) >= 0x20 => out.push(c),
            c if py_is_printable(c) => out.push(c),
            c => {
                let u = c as u32;
                if u <= 0xff {
                    out.push_str(&format!("\\x{u:02x}"));
                } else if u <= 0xffff {
                    out.push_str(&format!("\\u{u:04x}"));
                } else {
                    out.push_str(&format!("\\U{u:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

// ---------------------------------------------------------------------------
// json.loads
// ---------------------------------------------------------------------------

/// A `json.loads` failure (Python's `JSONDecodeError`, a `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError(pub String);

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for JsonError {}

/// Nesting deeper than this fails, where Python raises `RecursionError` at about 1000
/// (the exact Python limit depends on the caller's stack depth).
const MAX_DEPTH: usize = 900;

/// Python's `json.loads(text)` with the default (strict) decoder.
///
/// Known difference: a lone UTF-16 surrogate escape (`"\ud800"`) becomes U+FFFD, since
/// a Rust `String` cannot hold one; Python keeps the surrogate.
pub fn json_loads(text: &str) -> Result<PyValue, JsonError> {
    if text.starts_with('\u{feff}') {
        return Err(JsonError(
            "Unexpected UTF-8 BOM (decode using utf-8-sig)".into(),
        ));
    }
    let mut p = Parser {
        s: text.as_bytes(),
        src: text,
        i: 0,
        depth: 0,
    };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(JsonError(decode_error_text("Extra data", text, p.i)));
    }
    Ok(v)
}

/// `str(JSONDecodeError(msg, doc, pos))`: `"<msg>: line L column C (char P)"`, where
/// `P` counts characters and `byte_pos` is the parser's byte offset.
fn decode_error_text(msg: &str, doc: &str, byte_pos: usize) -> String {
    let mut b = byte_pos.min(doc.len());
    while !doc.is_char_boundary(b) {
        b -= 1;
    }
    let prefix = &doc[..b];
    let pos = prefix.chars().count();
    let lineno = prefix.matches('\n').count() + 1;
    let colno = match prefix.rfind('\n') {
        Some(nl) => pos - prefix[..nl].chars().count(),
        None => pos + 1,
    };
    format!("{msg}: line {lineno} column {colno} (char {pos})")
}

struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn err<T>(&self, what: &str) -> Result<T, JsonError> {
        Err(JsonError(decode_error_text(what, self.src, self.i)))
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn starts(&self, lit: &str) -> bool {
        self.s[self.i..].starts_with(lit.as_bytes())
    }

    fn value(&mut self) -> Result<PyValue, JsonError> {
        let Some(&c) = self.s.get(self.i) else {
            return self.err("Expecting value");
        };
        match c {
            b'"' => Ok(PyValue::Str(self.string()?)),
            b'{' => self.object(),
            b'[' => self.array(),
            b'n' if self.starts("null") => {
                self.i += 4;
                Ok(PyValue::None)
            }
            b't' if self.starts("true") => {
                self.i += 4;
                Ok(PyValue::Bool(true))
            }
            b'f' if self.starts("false") => {
                self.i += 5;
                Ok(PyValue::Bool(false))
            }
            b'N' if self.starts("NaN") => {
                self.i += 3;
                Ok(PyValue::Float(f64::NAN))
            }
            b'I' if self.starts("Infinity") => {
                self.i += 8;
                Ok(PyValue::Float(f64::INFINITY))
            }
            b'-' if self.starts("-Infinity") => {
                self.i += 9;
                Ok(PyValue::Float(f64::NEG_INFINITY))
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => self.err("Expecting value"),
        }
    }

    fn number(&mut self) -> Result<PyValue, JsonError> {
        let start = self.i;
        let s = self.s;
        let mut i = self.i;
        let neg = s[i] == b'-';
        if neg {
            i += 1;
        }
        let int_start = i;
        match s.get(i) {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                while i < s.len() && s[i].is_ascii_digit() {
                    i += 1;
                }
            }
            _ => return self.err("Expecting value"),
        }
        let int_end = i;
        let mut is_float = false;
        if i + 1 < s.len() && s[i] == b'.' && s[i + 1].is_ascii_digit() {
            i += 2;
            while i < s.len() && s[i].is_ascii_digit() {
                i += 1;
            }
            is_float = true;
        }
        if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
            let mut j = i + 1;
            if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
                j += 1;
            }
            let digits_start = j;
            while j < s.len() && s[j].is_ascii_digit() {
                j += 1;
            }
            if j > digits_start {
                i = j;
                is_float = true;
            }
        }
        self.i = i;
        if is_float {
            let text = &self.src[start..i];
            // Rust's parse saturates to infinity on overflow, as Python's float() does.
            let v = text
                .parse::<f64>()
                .map_err(|_| JsonError(format!("bad number at {start}")))?;
            Ok(PyValue::Float(v))
        } else {
            let digits = &self.src[int_start..int_end];
            if digits.len() > INT_MAX_STR_DIGITS {
                return Err(JsonError(
                    "Exceeds the limit for integer string conversion".into(),
                ));
            }
            Ok(PyValue::Int(PyInt::from_decimal(neg, digits)))
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        if self.i + 4 > self.s.len() {
            return self.err("Invalid \\uXXXX escape");
        }
        let h = &self.src[self.i..self.i + 4];
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return self.err("Invalid \\uXXXX escape");
        }
        self.i += 4;
        Ok(u32::from_str_radix(h, 16).unwrap_or(0))
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            let run_start = self.i;
            while self.i < self.s.len() {
                let c = self.s[self.i];
                if c == b'"' || c == b'\\' || c < 0x20 {
                    break;
                }
                self.i += 1;
            }
            out.push_str(&self.src[run_start..self.i]);
            let Some(&c) = self.s.get(self.i) else {
                return self.err("Unterminated string starting");
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.i += 1;
                    let Some(&e) = self.s.get(self.i) else {
                        return self.err("Unterminated string starting");
                    };
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut u = self.hex4()?;
                            if (0xd800..=0xdbff).contains(&u) && self.starts("\\u") {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xdc00..=0xdfff).contains(&lo) {
                                    u = 0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00);
                                } else {
                                    self.i = save;
                                }
                            }
                            out.push(char::from_u32(u).unwrap_or('\u{fffd}'));
                        }
                        _ => return self.err("Invalid \\escape"),
                    }
                }
                _ => return self.err("Invalid control character"),
            }
        }
    }

    fn enter(&mut self) -> Result<(), JsonError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return self.err("maximum recursion depth exceeded");
        }
        Ok(())
    }

    fn object(&mut self) -> Result<PyValue, JsonError> {
        self.enter()?;
        self.i += 1;
        let mut d = PyDict::new();
        self.ws();
        if self.s.get(self.i) == Some(&b'}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(PyValue::Dict(d));
        }
        loop {
            if self.s.get(self.i) != Some(&b'"') {
                return self.err("Expecting property name enclosed in double quotes");
            }
            let key = self.string()?;
            self.ws();
            if self.s.get(self.i) != Some(&b':') {
                return self.err("Expecting ':' delimiter");
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
            // dict semantics: a repeated key keeps its first position, takes the last value.
            d.insert(key, v);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => {
                    self.i += 1;
                    self.ws();
                }
                Some(b'}') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(PyValue::Dict(d));
                }
                _ => return self.err("Expecting ',' delimiter"),
            }
        }
    }

    fn array(&mut self) -> Result<PyValue, JsonError> {
        self.enter()?;
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.s.get(self.i) == Some(&b']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(PyValue::List(items));
        }
        loop {
            items.push(self.value()?);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => {
                    self.i += 1;
                    self.ws();
                }
                Some(b']') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(PyValue::List(items));
                }
                _ => return self.err("Expecting ',' delimiter"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// json.dumps
// ---------------------------------------------------------------------------

fn json_escape_into(out: &mut String, s: &str) {
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
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn json_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".to_string()
    } else if f == f64::INFINITY {
        "Infinity".to_string()
    } else if f == f64::NEG_INFINITY {
        "-Infinity".to_string()
    } else {
        py_float_str(f)
    }
}

fn dumps_indented(out: &mut String, v: &PyValue, level: usize) {
    match v {
        PyValue::None => out.push_str("null"),
        PyValue::Bool(true) => out.push_str("true"),
        PyValue::Bool(false) => out.push_str("false"),
        PyValue::Int(i) => out.push_str(&i.to_string()),
        PyValue::Float(f) => out.push_str(&json_float(*f)),
        PyValue::Str(s) => json_escape_into(out, s),
        PyValue::List(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (n, item) in items.iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                out.push('\n');
                out.push_str(&"  ".repeat(level + 1));
                dumps_indented(out, item, level + 1);
            }
            out.push('\n');
            out.push_str(&"  ".repeat(level));
            out.push(']');
        }
        PyValue::Dict(d) => dumps_dict(out, d, level),
    }
}

fn dumps_dict(out: &mut String, d: &PyDict, level: usize) {
    if d.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push('{');
    for (n, (k, item)) in d.iter().enumerate() {
        if n > 0 {
            out.push(',');
        }
        out.push('\n');
        out.push_str(&"  ".repeat(level + 1));
        json_escape_into(out, k);
        out.push_str(": ");
        dumps_indented(out, item, level + 1);
    }
    out.push('\n');
    out.push_str(&"  ".repeat(level));
    out.push('}');
}

/// `json.dumps(v, indent=2, ensure_ascii=False)`: `\n` line breaks, no trailing newline.
pub fn json_dumps_indent2(v: &PyValue) -> String {
    let mut out = String::new();
    dumps_indented(&mut out, v, 0);
    out
}

/// [`json_dumps_indent2`] for a dict.
pub fn json_dumps_dict_indent2(d: &PyDict) -> String {
    let mut out = String::new();
    dumps_dict(&mut out, d, 0);
    out
}

// ---------------------------------------------------------------------------
// serde_json at the edges
// ---------------------------------------------------------------------------

/// Convert to `serde_json`. Lossy only where JSON-the-standard cannot hold the value:
/// NaN and the infinities become `null`, an int outside `i64`/`u64` becomes the
/// nearest float (or `null` past the float range).
pub fn to_json(v: &PyValue) -> serde_json::Value {
    use serde_json::Value;
    match v {
        PyValue::None => Value::Null,
        PyValue::Bool(b) => Value::Bool(*b),
        PyValue::Int(PyInt::Small(i)) => Value::from(*i),
        PyValue::Int(PyInt::Big(s)) => {
            if let Ok(u) = s.parse::<u64>() {
                return Value::from(u);
            }
            s.parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or(Value::Null)
        }
        PyValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        PyValue::Str(s) => Value::String(s.clone()),
        PyValue::List(items) => Value::Array(items.iter().map(to_json).collect()),
        PyValue::Dict(d) => Value::Object(to_json_map(d)),
    }
}

/// [`to_json`] for a dict.
pub fn to_json_map(d: &PyDict) -> serde_json::Map<String, serde_json::Value> {
    d.iter().map(|(k, v)| (k.clone(), to_json(v))).collect()
}

/// Convert from `serde_json`: an integer literal stays an `int`, anything with a
/// fraction or exponent is a `float`, as `json.loads` would decide.
pub fn from_json(v: &serde_json::Value) -> PyValue {
    use serde_json::Value;
    match v {
        Value::Null => PyValue::None,
        Value::Bool(b) => PyValue::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                PyValue::Int(PyInt::Small(i))
            } else if let Some(u) = n.as_u64() {
                PyValue::Int(PyInt::from_decimal(false, &u.to_string()))
            } else {
                PyValue::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(s) => PyValue::Str(s.clone()),
        Value::Array(a) => PyValue::List(a.iter().map(from_json).collect()),
        Value::Object(o) => PyValue::Dict(from_json_map(o)),
    }
}

/// [`from_json`] for an object.
pub fn from_json_map(m: &serde_json::Map<String, serde_json::Value>) -> PyDict {
    m.iter().map(|(k, v)| (k.clone(), from_json(v))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_checks() {
        assert_eq!(
            PyInt::from_f64_trunc(1e30).unwrap().to_string(),
            "1000000000000000019884624838656"
        );
        assert_eq!(py_int_parse(" -1_0 "), Some(PyInt::Small(-10)));
        assert_eq!(py_int_parse("1__0"), None);
        assert_eq!(py_str_repr("it's"), "\"it's\"");
        assert_eq!(
            PyValue::List(vec![PyValue::from("a"), PyValue::from(1.0)]).py_str(),
            "['a', 1.0]"
        );
        let v = json_loads(r#"{"a": [1, 2.5e0, NaN], "b": {}, "a": -0}"#).unwrap();
        assert_eq!(json_dumps_indent2(&v), "{\n  \"a\": 0,\n  \"b\": {}\n}");
        assert!(
            json_loads("1e400")
                .unwrap()
                .py_float()
                .unwrap()
                .is_infinite()
        );
        assert!(json_loads("01").is_err());
        assert_eq!(py_clamp(f64::NAN, -120.0, 0.0), 0.0);
    }
}
