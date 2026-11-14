//! Python's `json` module, and the builtins the recorders apply to decoded JSON.
//!
//! The recorders in this crate write NDJSON and summary files that the Python build
//! must be able to reopen, and read files the Python build wrote. serde_json cannot do
//! that byte for byte: it has no NaN, prints `1e16` where Python prints `1e+16`, and
//! escapes differently. So this module carries its own [`Value`] (Python's view of a
//! decoded JSON document: `int` and `float` are different types, NaN is a float) with a
//! decoder that accepts what `json.loads` accepts and an encoder that writes what
//! `json.dumps` writes for each option set the Python uses.
//!
//! It also holds the Python semantics the recorders lean on when they poke at that
//! data: truthiness, `float()`, `int()`, `str()`, `==`, and `.get()` on something that
//! might not be a dict (an `AttributeError` in Python, a [`PyErr`] here).

use std::fmt;

use beam_base::pyfmt::{py_float_parse, py_float_str, py_strip};
use indexmap::IndexMap;

/// A Python `dict` with string keys, in insertion order.
pub type Dict = IndexMap<String, Value>;

/// A decoded JSON document as Python holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    /// An integer outside `i64`, kept as its decimal text (Python's int is unbounded).
    BigInt(String),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Dict(Dict),
}

/// An exception Python would raise, by type, with its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyErr {
    ValueError(String),
    TypeError(String),
    AttributeError(String),
    OverflowError(String),
    KeyError(String),
    RuntimeError(String),
    OSError(String),
}

impl PyErr {
    /// The Python exception class name.
    pub fn type_name(&self) -> &'static str {
        match self {
            PyErr::ValueError(_) => "ValueError",
            PyErr::TypeError(_) => "TypeError",
            PyErr::AttributeError(_) => "AttributeError",
            PyErr::OverflowError(_) => "OverflowError",
            PyErr::KeyError(_) => "KeyError",
            PyErr::RuntimeError(_) => "RuntimeError",
            PyErr::OSError(_) => "OSError",
        }
    }

    /// `str(exc)`.
    pub fn message(&self) -> &str {
        match self {
            PyErr::ValueError(m)
            | PyErr::TypeError(m)
            | PyErr::AttributeError(m)
            | PyErr::OverflowError(m)
            | PyErr::KeyError(m)
            | PyErr::RuntimeError(m)
            | PyErr::OSError(m) => m,
        }
    }
}

impl fmt::Display for PyErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for PyErr {}

impl From<std::io::Error> for PyErr {
    fn from(e: std::io::Error) -> Self {
        PyErr::OSError(e.to_string())
    }
}

impl From<beam_base::pyerr::ValueError> for PyErr {
    fn from(e: beam_base::pyerr::ValueError) -> Self {
        PyErr::ValueError(e.0)
    }
}

// ---------------------------------------------------------------------------------
// Construction helpers

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int(v as i64)
    }
}
impl From<usize> for Value {
    fn from(v: usize) -> Self {
        Value::Int(v as i64)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Str(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Str(v)
    }
}
impl From<Dict> for Value {
    fn from(v: Dict) -> Self {
        Value::Dict(v)
    }
}
impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::List(v)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(v) => v.into(),
            None => Value::Null,
        }
    }
}

/// Build a dict value from `(key, value)` pairs, in order (a later duplicate key
/// replaces the value in the first one's position, as a Python dict literal does).
pub fn dict<I, K>(pairs: I) -> Value
where
    I: IntoIterator<Item = (K, Value)>,
    K: Into<String>,
{
    Value::Dict(dict_of(pairs))
}

/// [`dict`] returning the map itself.
pub fn dict_of<I, K>(pairs: I) -> Dict
where
    I: IntoIterator<Item = (K, Value)>,
    K: Into<String>,
{
    let mut d = Dict::new();
    for (k, v) in pairs {
        d.insert(k.into(), v);
    }
    d
}

// ---------------------------------------------------------------------------------
// Python semantics

impl Value {
    /// `type(x).__name__`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "NoneType",
            Value::Bool(_) => "bool",
            Value::Int(_) | Value::BigInt(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "str",
            Value::List(_) => "list",
            Value::Dict(_) => "dict",
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// `bool(x)`.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::BigInt(_) => true,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Dict(d) => !d.is_empty(),
        }
    }

    /// The dict, if this is one.
    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// `d.get(key)` on a value known to be a dict; `None` for a missing key or a
    /// non-dict.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_dict().and_then(|d| d.get(key))
    }

    /// `x.get(key)` with Python's failure for a non-dict.
    pub fn py_get(&self, key: &str) -> Result<Option<&Value>, PyErr> {
        match self {
            Value::Dict(d) => Ok(d.get(key)),
            other => Err(no_get(other)),
        }
    }

    /// `x.get(key)` returning Python's `None` (as [`Value::Null`]) for a missing key.
    pub fn py_get_or_null(&self, key: &str) -> Result<&Value, PyErr> {
        Ok(self.py_get(key)?.unwrap_or(&NULL))
    }

    /// `float(x)`.
    pub fn py_float(&self) -> Result<f64, PyErr> {
        match self {
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Int(i) => Ok(*i as f64),
            Value::BigInt(text) => {
                let v = text.parse::<f64>().unwrap_or(f64::INFINITY);
                if v.is_finite() {
                    Ok(v)
                } else {
                    Err(PyErr::OverflowError(
                        "int too large to convert to float".to_string(),
                    ))
                }
            }
            Value::Float(f) => Ok(*f),
            Value::Str(s) => py_float_parse(s).ok_or_else(|| {
                PyErr::ValueError(format!(
                    "could not convert string to float: {}",
                    py_str_repr(s)
                ))
            }),
            other => Err(PyErr::TypeError(format!(
                "float() argument must be a string or a real number, not '{}'",
                other.type_name()
            ))),
        }
    }

    /// `int(x)`. A value outside `i64` saturates (Python's int would not).
    pub fn py_int(&self) -> Result<i64, PyErr> {
        match self {
            Value::Bool(b) => Ok(*b as i64),
            Value::Int(i) => Ok(*i),
            Value::BigInt(text) => Ok(if text.starts_with('-') {
                i64::MIN
            } else {
                i64::MAX
            }),
            Value::Float(f) => float_to_int(*f),
            Value::Str(s) => parse_py_int(s).ok_or_else(|| {
                PyErr::ValueError(format!(
                    "invalid literal for int() with base 10: {}",
                    py_str_repr(s)
                ))
            }),
            other => Err(PyErr::TypeError(format!(
                "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                other.type_name()
            ))),
        }
    }

    /// `str(x)`.
    pub fn py_str(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            other => other.py_repr(),
        }
    }

    /// `repr(x)`.
    pub fn py_repr(&self) -> String {
        match self {
            Value::Null => "None".to_string(),
            Value::Bool(true) => "True".to_string(),
            Value::Bool(false) => "False".to_string(),
            Value::Int(i) => i.to_string(),
            Value::BigInt(t) => t.clone(),
            Value::Float(f) => py_float_str(*f),
            Value::Str(s) => py_str_repr(s),
            Value::List(l) => {
                let parts: Vec<String> = l.iter().map(Value::py_repr).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Dict(d) => {
                let parts: Vec<String> = d
                    .iter()
                    .map(|(k, v)| format!("{}: {}", py_str_repr(k), v.py_repr()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
        }
    }

    /// Whether Python could use this as a dict key or set member.
    pub fn hashable(&self) -> bool {
        !matches!(self, Value::List(_) | Value::Dict(_))
    }

    /// `x in {"a", "b"}`: `TypeError` for an unhashable `x`.
    pub fn in_str_set(&self, set: &[&str]) -> Result<bool, PyErr> {
        if !self.hashable() {
            return Err(unhashable(self));
        }
        Ok(matches!(self, Value::Str(s) if set.contains(&s.as_str())))
    }

    /// `x == "text"`.
    pub fn eq_str(&self, text: &str) -> bool {
        matches!(self, Value::Str(s) if s == text)
    }
}

static NULL: Value = Value::Null;

/// The `TypeError` for an unhashable value.
pub fn unhashable(v: &Value) -> PyErr {
    PyErr::TypeError(format!("unhashable type: '{}'", v.type_name()))
}

fn no_get(v: &Value) -> PyErr {
    PyErr::AttributeError(format!(
        "'{}' object has no attribute 'get'",
        v.type_name()
    ))
}

/// `x or {}` followed by `.get(key)`: the common recorder idiom. A falsy `x` gives
/// `None`; a truthy non-dict is Python's `AttributeError`.
pub fn get_or_empty<'a>(v: Option<&'a Value>, key: &str) -> Result<Option<&'a Value>, PyErr> {
    match v {
        Some(v) if v.truthy() => v.py_get(key),
        _ => Ok(None),
    }
}

/// `x or {}` as a value: the value itself if truthy, else an empty dict.
pub fn or_empty_dict(v: Option<&Value>) -> Value {
    match v {
        Some(v) if v.truthy() => v.clone(),
        _ => Value::Dict(Dict::new()),
    }
}

/// `float(x)` for a float, as `int()` does it.
pub fn float_to_int(f: f64) -> Result<i64, PyErr> {
    if f.is_nan() {
        return Err(PyErr::ValueError(
            "cannot convert float NaN to integer".to_string(),
        ));
    }
    if f.is_infinite() {
        return Err(PyErr::OverflowError(
            "cannot convert float infinity to integer".to_string(),
        ));
    }
    Ok(f.trunc() as i64)
}

/// `int(text)` in base 10: surrounding whitespace, a sign, digits with single
/// underscores between them.
fn parse_py_int(text: &str) -> Option<i64> {
    let t = py_strip(text);
    let (neg, body) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let digits: String = body.chars().filter(|c| *c != '_').collect();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Saturate rather than fail for magnitudes beyond i64.
    let mut v: i128 = 0;
    for b in digits.bytes() {
        v = (v * 10 + (b - b'0') as i128).min(i128::from(i64::MAX) + 1);
    }
    let v = if neg { -v } else { v };
    Some(v.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
}

/// `float(x)` with the failures Python's `_finite` helpers swallow (`TypeError`,
/// `ValueError`) mapped to `None`, and non-finite results to `None`.
///
/// An `int` too large for a float raises `OverflowError` in Python, which those
/// helpers do not catch; here it is `None` as well.
pub fn finite(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    match v.py_float() {
        Ok(f) if f.is_finite() => Some(f),
        _ => None,
    }
}

/// `repr()` of a str.
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
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if (0x80..0xa0).contains(&(c as u32)) => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `a == b` with Python's cross-type numeric equality (`1 == 1.0 == True`).
pub fn py_eq(a: &Value, b: &Value) -> bool {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Bool(b) => Some(*b as i64 as f64),
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Int(x), Value::Bool(y)) | (Value::Bool(y), Value::Int(x)) => *x == *y as i64,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::BigInt(x), Value::BigInt(y)) => x == y,
        (Value::Null, Value::Null) => true,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::List(x), Value::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Value::Dict(x), Value::Dict(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).map(|w| py_eq(v, w)).unwrap_or(false))
        }
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

/// Python's `max(a, b)` for floats: `a` unless `b > a`.
pub fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// Python's `min(a, b)` for floats: `a` unless `b < a`.
pub fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// Python's `str.isspace()`-free first-N-characters slice, `s[:n]`.
pub fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Python's `len(s)` for a str (code points).
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `re.sub(r"[^<allowed>]+", "-", text)` where `allowed(c)` is the character class.
pub fn sub_runs(text: &str, allowed: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for c in text.chars() {
        if allowed(c) {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    out
}

/// `s.strip(chars)`.
pub fn strip_chars<'a>(s: &'a str, chars: &[char]) -> &'a str {
    s.trim_matches(|c| chars.contains(&c))
}

// ---------------------------------------------------------------------------------
// Decoding (json.loads)

/// Why `json.loads` rejected a document (`json.JSONDecodeError`, a `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonDecodeError(pub String);

impl fmt::Display for JsonDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<JsonDecodeError> for PyErr {
    fn from(e: JsonDecodeError) -> Self {
        PyErr::ValueError(e.0)
    }
}

/// `json.loads(text)`: NaN, Infinity and -Infinity are accepted, a duplicate key keeps
/// its first position and takes the last value, integers and floats stay distinct.
///
/// Known differences: a lone surrogate escape (`"\ud800"`) becomes U+FFFD, since a
/// Rust string cannot hold one; nesting deeper than 2000 levels is a decode error
/// where Python raises `RecursionError` at around 1000.
pub fn loads(text: &str) -> Result<Value, JsonDecodeError> {
    let mut p = Parser {
        s: text.as_bytes(),
        text,
        i: 0,
        depth: 0,
    };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(p.err("Extra data"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, what: &str) -> JsonDecodeError {
        JsonDecodeError(format!("{what}: char {}", self.text[..self.i].chars().count()))
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn starts(&self, lit: &str) -> bool {
        self.s[self.i..].starts_with(lit.as_bytes())
    }

    fn value(&mut self) -> Result<Value, JsonDecodeError> {
        let Some(&c) = self.s.get(self.i) else {
            return Err(self.err("Expecting value"));
        };
        match c {
            b'"' => {
                self.i += 1;
                Ok(Value::Str(self.string()?))
            }
            b'{' => {
                self.i += 1;
                self.object()
            }
            b'[' => {
                self.i += 1;
                self.array()
            }
            b'n' if self.starts("null") => {
                self.i += 4;
                Ok(Value::Null)
            }
            b't' if self.starts("true") => {
                self.i += 4;
                Ok(Value::Bool(true))
            }
            b'f' if self.starts("false") => {
                self.i += 5;
                Ok(Value::Bool(false))
            }
            b'N' if self.starts("NaN") => {
                self.i += 3;
                Ok(Value::Float(f64::NAN))
            }
            b'I' if self.starts("Infinity") => {
                self.i += 8;
                Ok(Value::Float(f64::INFINITY))
            }
            b'-' if self.starts("-Infinity") => {
                self.i += 9;
                Ok(Value::Float(f64::NEG_INFINITY))
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(self.err("Expecting value")),
        }
    }

    fn number(&mut self) -> Result<Value, JsonDecodeError> {
        let start = self.i;
        let s = self.s;
        let mut i = self.i;
        if s[i] == b'-' {
            i += 1;
        }
        match s.get(i) {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                while i < s.len() && s[i].is_ascii_digit() {
                    i += 1;
                }
            }
            _ => return Err(self.err("Expecting value")),
        }
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
            if j < s.len() && s[j].is_ascii_digit() {
                while j < s.len() && s[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
                is_float = true;
            }
        }
        self.i = i;
        let lit = &self.text[start..i];
        if is_float {
            Ok(Value::Float(lit.parse::<f64>().unwrap_or(f64::NAN)))
        } else {
            match lit.parse::<i64>() {
                Ok(v) => Ok(Value::Int(v)),
                Err(_) => Ok(Value::BigInt(lit.to_string())),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonDecodeError> {
        if self.i + 4 > self.s.len() {
            return Err(self.err("Invalid \\uXXXX escape"));
        }
        let h = &self.text[self.i..self.i + 4];
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(self.err("Invalid \\uXXXX escape"));
        }
        self.i += 4;
        Ok(u32::from_str_radix(h, 16).unwrap_or(0))
    }

    fn string(&mut self) -> Result<String, JsonDecodeError> {
        let mut out = String::new();
        loop {
            let Some(&c) = self.s.get(self.i) else {
                return Err(self.err("Unterminated string starting at"));
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.i += 1;
                    let Some(&e) = self.s.get(self.i) else {
                        return Err(self.err("Unterminated string starting at"));
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
                            let mut cp = self.hex4()?;
                            if (0xd800..0xdc00).contains(&cp)
                                && self.s.get(self.i) == Some(&b'\\')
                                && self.s.get(self.i + 1) == Some(&b'u')
                            {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xdc00..0xe000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                                } else {
                                    self.i = save;
                                }
                            }
                            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                        }
                        _ => {
                            self.i -= 1;
                            return Err(self.err("Invalid \\escape"));
                        }
                    }
                }
                c if c < 0x20 => return Err(self.err("Invalid control character at")),
                _ => {
                    // Copy one UTF-8 character.
                    let rest = &self.text[self.i..];
                    let ch = rest.chars().next().unwrap_or('\u{fffd}');
                    out.push(ch);
                    self.i += ch.len_utf8();
                }
            }
        }
    }

    fn enter(&mut self) -> Result<(), JsonDecodeError> {
        self.depth += 1;
        if self.depth > 2000 {
            return Err(self.err("maximum recursion depth exceeded"));
        }
        Ok(())
    }

    fn object(&mut self) -> Result<Value, JsonDecodeError> {
        self.enter()?;
        let mut d = Dict::new();
        self.ws();
        if self.s.get(self.i) == Some(&b'}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::Dict(d));
        }
        loop {
            if self.s.get(self.i) != Some(&b'"') {
                return Err(self.err("Expecting property name enclosed in double quotes"));
            }
            self.i += 1;
            let key = self.string()?;
            self.ws();
            if self.s.get(self.i) != Some(&b':') {
                return Err(self.err("Expecting ':' delimiter"));
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
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
                    return Ok(Value::Dict(d));
                }
                _ => return Err(self.err("Expecting ',' delimiter")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, JsonDecodeError> {
        self.enter()?;
        let mut l = Vec::new();
        self.ws();
        if self.s.get(self.i) == Some(&b']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::List(l));
        }
        loop {
            l.push(self.value()?);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => {
                    self.i += 1;
                    self.ws();
                }
                Some(b']') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Value::List(l));
                }
                _ => return Err(self.err("Expecting ',' delimiter")),
            }
        }
    }
}

// ---------------------------------------------------------------------------------
// Encoding (json.dumps)

/// The `json.dumps` options the Python uses.
#[derive(Debug, Clone, Copy)]
pub struct DumpOpts {
    pub ensure_ascii: bool,
    pub allow_nan: bool,
    pub indent: Option<usize>,
    pub item_sep: &'static str,
    pub key_sep: &'static str,
}

impl DumpOpts {
    /// `json.dumps(x)`: `", "` and `": "`.
    pub const fn default_seps() -> Self {
        DumpOpts {
            ensure_ascii: true,
            allow_nan: true,
            indent: None,
            item_sep: ", ",
            key_sep: ": ",
        }
    }

    /// `separators=(",", ":")`.
    pub const fn compact() -> Self {
        DumpOpts {
            ensure_ascii: true,
            allow_nan: true,
            indent: None,
            item_sep: ",",
            key_sep: ":",
        }
    }

    /// `indent=n` (which makes the item separator `","`).
    pub const fn indented(n: usize) -> Self {
        DumpOpts {
            ensure_ascii: true,
            allow_nan: true,
            indent: Some(n),
            item_sep: ",",
            key_sep: ": ",
        }
    }

    pub const fn ensure_ascii(mut self, on: bool) -> Self {
        self.ensure_ascii = on;
        self
    }

    pub const fn allow_nan(mut self, on: bool) -> Self {
        self.allow_nan = on;
        self
    }
}

/// `json.dumps(value, ...)`. `Err(ValueError)` for a non-finite float when
/// `allow_nan` is off, raised before anything is produced, as in Python.
pub fn dumps(value: &Value, opts: &DumpOpts) -> Result<String, PyErr> {
    let mut out = String::new();
    encode(value, opts, 0, &mut out)?;
    Ok(out)
}

fn encode(v: &Value, o: &DumpOpts, level: usize, out: &mut String) -> Result<(), PyErr> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::BigInt(t) => out.push_str(t),
        Value::Float(f) => out.push_str(&float_text(*f, o.allow_nan)?),
        Value::Str(s) => encode_str(s, o.ensure_ascii, out),
        Value::List(l) => {
            if l.is_empty() {
                out.push_str("[]");
                return Ok(());
            }
            out.push('[');
            for (n, item) in l.iter().enumerate() {
                if n > 0 {
                    out.push_str(o.item_sep);
                }
                newline(o, level + 1, out);
                encode(item, o, level + 1, out)?;
            }
            newline(o, level, out);
            out.push(']');
        }
        Value::Dict(d) => {
            if d.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            out.push('{');
            for (n, (k, item)) in d.iter().enumerate() {
                if n > 0 {
                    out.push_str(o.item_sep);
                }
                newline(o, level + 1, out);
                encode_str(k, o.ensure_ascii, out);
                out.push_str(o.key_sep);
                encode(item, o, level + 1, out)?;
            }
            newline(o, level, out);
            out.push('}');
        }
    }
    Ok(())
}

fn newline(o: &DumpOpts, level: usize, out: &mut String) {
    if let Some(n) = o.indent {
        out.push('\n');
        for _ in 0..n * level {
            out.push(' ');
        }
    }
}

fn float_text(f: f64, allow_nan: bool) -> Result<String, PyErr> {
    if f.is_finite() {
        return Ok(py_float_str(f));
    }
    if !allow_nan {
        return Err(PyErr::ValueError(format!(
            "Out of range float values are not JSON compliant: {}",
            py_float_str(f)
        )));
    }
    Ok(if f.is_nan() {
        "NaN"
    } else if f > 0.0 {
        "Infinity"
    } else {
        "-Infinity"
    }
    .to_string())
}

fn encode_str(s: &str, ensure_ascii: bool, out: &mut String) {
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
            c if ensure_ascii && (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------------
// Text files as Python reads and writes them on Windows

/// The lines of a text file opened with `open(path, encoding="utf-8")`: universal
/// newlines (`\r\n`, `\r` and `\n` all end a line and read back as `\n`), strict
/// UTF-8 (a bad byte is Python's `UnicodeDecodeError`, a `ValueError`).
pub fn read_text_lines(path: &std::path::Path) -> Result<Vec<String>, PyErr> {
    let text = read_text(path)?;
    let mut lines = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        cur.push(c);
        if c == '\n' {
            lines.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    Ok(lines)
}

/// `Path.read_text(encoding="utf-8")`: strict UTF-8, universal newlines.
pub fn read_text(path: &std::path::Path) -> Result<String, PyErr> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8(bytes)
        .map_err(|e| PyErr::ValueError(format!("'utf-8' codec can't decode bytes: {e}")))?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// What a text-mode write does to `\n` on Windows (`newline=None`): `\r\n`.
pub fn to_windows_newlines(text: &str) -> String {
    text.replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_basics() {
        let v = loads(r#"{"a": 1, "b": 1.0, "c": [NaN, -Infinity], "a": 2}"#).unwrap();
        let d = v.as_dict().unwrap();
        assert_eq!(d.keys().collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(d["a"], Value::Int(2));
        assert_eq!(
            dumps(&v, &DumpOpts::compact()).unwrap(),
            r#"{"a":2,"b":1.0,"c":[NaN,-Infinity]}"#
        );
        assert!(dumps(&v, &DumpOpts::compact().allow_nan(false)).is_err());
        assert_eq!(
            dumps(&Value::from("é\u{1f600}"), &DumpOpts::compact()).unwrap(),
            r#""é😀""#
        );
        assert!(loads("1.").is_err());
        assert!(loads("[1,]").is_err());
    }
}
