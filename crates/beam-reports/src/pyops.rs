//! Python semantics the report modules share: a JSON value model that matches what
//! `json.loads` produces, truthiness, `float()` / `int()` / `str()` / `repr()` of those
//! values, Python's `min`/`max` argument order, `math.hypot`, `html.unescape`, and
//! Unicode decimal digits (`re`'s `\d`).
//!
//! Several of the Python modules take dicts straight from `json.loads` (induction
//! reports, fuel station rows and events, drift events, POI packets) and apply Python's
//! dynamic rules to whatever the game sent. [`PyValue`] keeps exactly the information
//! those rules look at: integers keep their exact decimal text (Python ints are
//! unbounded), NaN and the infinities survive (`json.loads` accepts `NaN`, `Infinity`
//! and `-Infinity`), and dicts keep insertion order. `beam_base::pyfmt` covers plain
//! number formatting; everything here is fixture-tested against CPython in
//! `fixtures/data/pyops.json`.
//!
//! The tables in `pytables.in.rs` are generated from CPython by
//! `fixtures/gen/pytables.py`.

use std::fmt;

use beam_base::pyfmt::{py_float_parse, py_float_str, py_is_space};
use indexmap::IndexMap;

include!("pytables.in.rs");

// ---------------------------------------------------------------------------------------
// Exceptions

/// The Python exception class a ported function would have raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyErrKind {
    TypeError,
    ValueError,
    OverflowError,
    ZeroDivisionError,
    KeyError,
    AttributeError,
    RecursionError,
}

impl PyErrKind {
    pub fn name(self) -> &'static str {
        match self {
            PyErrKind::TypeError => "TypeError",
            PyErrKind::ValueError => "ValueError",
            PyErrKind::OverflowError => "OverflowError",
            PyErrKind::ZeroDivisionError => "ZeroDivisionError",
            PyErrKind::KeyError => "KeyError",
            PyErrKind::AttributeError => "AttributeError",
            PyErrKind::RecursionError => "RecursionError",
        }
    }
}

/// A raised Python exception: its class and `str(exc)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyErr {
    pub kind: PyErrKind,
    pub msg: String,
}

impl PyErr {
    pub fn new(kind: PyErrKind, msg: impl Into<String>) -> Self {
        PyErr {
            kind,
            msg: msg.into(),
        }
    }
    pub fn value(msg: impl Into<String>) -> Self {
        Self::new(PyErrKind::ValueError, msg)
    }
    pub fn type_error(msg: impl Into<String>) -> Self {
        Self::new(PyErrKind::TypeError, msg)
    }
    pub fn overflow(msg: impl Into<String>) -> Self {
        Self::new(PyErrKind::OverflowError, msg)
    }
    /// True for the classes an `except (TypeError, ValueError)` catches.
    pub fn is_type_or_value(&self) -> bool {
        matches!(self.kind, PyErrKind::TypeError | PyErrKind::ValueError)
    }
}

impl fmt::Display for PyErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for PyErr {}

// ---------------------------------------------------------------------------------------
// Values

/// A dict as `json.loads` builds it: string keys, insertion order, last duplicate wins
/// in the first duplicate's position.
pub type PyDict = IndexMap<String, PyValue>;

/// One value as `json.loads` returns it.
#[derive(Debug, Clone, PartialEq)]
pub enum PyValue {
    None,
    Bool(bool),
    /// Exact decimal text, canonical (no leading zeros, no `-0`).
    Int(String),
    Float(f64),
    Str(String),
    List(Vec<PyValue>),
    Dict(PyDict),
}

impl PyValue {
    /// An int from a machine integer.
    pub fn int(v: i64) -> PyValue {
        PyValue::Int(v.to_string())
    }

    pub fn str(s: impl Into<String>) -> PyValue {
        PyValue::Str(s.into())
    }

    pub fn is_none(&self) -> bool {
        matches!(self, PyValue::None)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            PyValue::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&PyDict> {
        match self {
            PyValue::Dict(d) => Some(d),
            _ => None,
        }
    }

    /// `type(x).__name__`.
    pub fn type_name(&self) -> &'static str {
        match self {
            PyValue::None => "NoneType",
            PyValue::Bool(_) => "bool",
            PyValue::Int(_) => "int",
            PyValue::Float(_) => "float",
            PyValue::Str(_) => "str",
            PyValue::List(_) => "list",
            PyValue::Dict(_) => "dict",
        }
    }

    /// `bool(x)`.
    pub fn truthy(&self) -> bool {
        match self {
            PyValue::None => false,
            PyValue::Bool(b) => *b,
            PyValue::Int(s) => s != "0",
            PyValue::Float(f) => *f != 0.0,
            PyValue::Str(s) => !s.is_empty(),
            PyValue::List(v) => !v.is_empty(),
            PyValue::Dict(d) => !d.is_empty(),
        }
    }

    /// `x == "text"`: only a str equals a str.
    pub fn eq_str(&self, text: &str) -> bool {
        matches!(self, PyValue::Str(s) if s == text)
    }

    /// `float(x)`.
    pub fn py_float(&self) -> Result<f64, PyErr> {
        match self {
            PyValue::Float(f) => Ok(*f),
            PyValue::Int(s) => int_text_to_f64(s),
            PyValue::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            PyValue::Str(s) => py_str_to_float(s),
            other => Err(PyErr::type_error(format!(
                "float() argument must be a string or a real number, not '{}'",
                other.type_name()
            ))),
        }
    }

    /// `int(x)`, as exact canonical decimal text.
    pub fn py_int(&self) -> Result<String, PyErr> {
        match self {
            PyValue::Int(s) => Ok(s.clone()),
            PyValue::Bool(b) => Ok(if *b { "1" } else { "0" }.to_string()),
            PyValue::Float(f) => py_float_to_int(*f),
            PyValue::Str(s) => py_str_to_int(s),
            other => Err(PyErr::type_error(format!(
                "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                other.type_name()
            ))),
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
            PyValue::Int(s) => s.clone(),
            PyValue::Float(f) => py_float_str(*f),
            PyValue::Str(s) => py_repr_str(s),
            PyValue::List(v) => {
                let parts: Vec<String> = v.iter().map(PyValue::py_repr).collect();
                format!("[{}]", parts.join(", "))
            }
            PyValue::Dict(d) => {
                let parts: Vec<String> = d
                    .iter()
                    .map(|(k, v)| format!("{}: {}", py_repr_str(k), v.py_repr()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
        }
    }

    /// `d.get(key)` on a dict; Python raises `AttributeError` for anything else.
    pub fn dict_get(&self, key: &str) -> Result<Option<&PyValue>, PyErr> {
        match self {
            PyValue::Dict(d) => Ok(d.get(key)),
            other => Err(PyErr::new(
                PyErrKind::AttributeError,
                format!("'{}' object has no attribute 'get'", other.type_name()),
            )),
        }
    }
}

impl From<&serde_json::Value> for PyValue {
    /// For callers holding `serde_json` values. serde_json cannot carry NaN or the
    /// infinities, and its integers above `u64` arrive as floats, so prefer
    /// [`json_loads`] on the original text where that matters.
    fn from(v: &serde_json::Value) -> Self {
        match v {
            serde_json::Value::Null => PyValue::None,
            serde_json::Value::Bool(b) => PyValue::Bool(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    PyValue::Int(i.to_string())
                } else if let Some(u) = n.as_u64() {
                    PyValue::Int(u.to_string())
                } else {
                    PyValue::Float(n.as_f64().unwrap_or(f64::NAN))
                }
            }
            serde_json::Value::String(s) => PyValue::Str(s.clone()),
            serde_json::Value::Array(a) => PyValue::List(a.iter().map(PyValue::from).collect()),
            serde_json::Value::Object(o) => PyValue::Dict(
                o.iter()
                    .map(|(k, v)| (k.clone(), PyValue::from(v)))
                    .collect(),
            ),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Numbers

/// Python's `min(a, b)`: `b` only when `b < a`, so NaN handling follows argument order.
pub fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// Python's `max(a, b)`: `b` only when `b > a`.
pub fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// `min(iterable)` over a non-empty slice: the first wins on ties.
pub fn py_min_iter(values: &[f64]) -> Option<f64> {
    let mut it = values.iter();
    let mut cur = *it.next()?;
    for &v in it {
        if v < cur {
            cur = v;
        }
    }
    Some(cur)
}

/// `max(iterable)` over a non-empty slice.
pub fn py_max_iter(values: &[f64]) -> Option<f64> {
    let mut it = values.iter();
    let mut cur = *it.next()?;
    for &v in it {
        if v > cur {
            cur = v;
        }
    }
    Some(cur)
}

/// CPython 3.13 `math.hypot(x, y)` (its own `vector_norm`, not the C library's
/// `hypot`, which can differ in the last bit).
pub fn py_hypot(x: f64, y: f64) -> f64 {
    let mut vec = [x.abs(), y.abs()];
    let found_nan = vec[0].is_nan() || vec[1].is_nan();
    let mut max = 0.0f64;
    for &v in &vec {
        if v > max {
            max = v;
        }
    }
    vector_norm(&mut vec, max, found_nan)
}

fn vector_norm(vec: &mut [f64], max: f64, found_nan: bool) -> f64 {
    if max.is_infinite() {
        return max;
    }
    if found_nan {
        return f64::NAN;
    }
    if max == 0.0 || vec.len() <= 1 {
        return max;
    }
    let max_e = frexp_exp(max);
    if max_e < -1023 {
        for v in vec.iter_mut() {
            *v /= f64::MIN_POSITIVE;
        }
        return f64::MIN_POSITIVE * vector_norm(vec, max / f64::MIN_POSITIVE, found_nan);
    }
    let scale = ldexp1(-max_e);
    let mut csum = 1.0f64;
    let mut frac1 = 0.0f64;
    let mut frac2 = 0.0f64;
    for &v in vec.iter() {
        let x = v * scale;
        let (pr_hi, pr_lo) = dl_mul(x, x);
        let (sm_hi, sm_lo) = dl_fast_sum(csum, pr_hi);
        csum = sm_hi;
        frac1 += pr_lo;
        frac2 += sm_lo;
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let (pr_hi, pr_lo) = dl_mul(-h, h);
    let (sm_hi, sm_lo) = dl_fast_sum(csum, pr_hi);
    csum = sm_hi;
    frac1 += pr_lo;
    frac2 += sm_lo;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h);
    h / scale
}

fn dl_mul(x: f64, y: f64) -> (f64, f64) {
    let z = x * y;
    let zz = x.mul_add(y, -z);
    (z, zz)
}

fn dl_fast_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let z = x - a;
    let y = b - z;
    (x, y)
}

/// C's `frexp(x, &e)` exponent for a finite, non-zero `x`: `x = m * 2**e`, `0.5 <= m < 1`.
fn frexp_exp(x: f64) -> i32 {
    let bits = x.abs().to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        // Subnormal: the leading set bit of the mantissa decides it.
        let mant = bits & ((1u64 << 52) - 1);
        let lz = mant.leading_zeros() as i32 - 12; // zeros inside the 52-bit field
        -1022 - lz
    } else {
        exp - 1022
    }
}

/// `ldexp(1.0, e)` for the range `vector_norm` uses.
fn ldexp1(e: i32) -> f64 {
    if e > 1023 {
        return f64::INFINITY;
    }
    if e >= -1022 {
        return f64::from_bits(((e + 1023) as u64) << 52);
    }
    if e >= -1074 {
        return f64::from_bits(1u64 << (e + 1074));
    }
    0.0
}

/// The decimal value of a Unicode category Nd character (`unicodedata.decimal`).
pub fn decimal_value(c: char) -> Option<u32> {
    let cp = c as u32;
    let idx = DECIMAL_ZEROS.partition_point(|&z| z <= cp);
    if idx == 0 {
        return None;
    }
    let zero = DECIMAL_ZEROS[idx - 1];
    if cp - zero < 10 {
        Some(cp - zero)
    } else {
        None
    }
}

/// The whitespace `int()` and `float()` strip: `str.isspace()` without U+001C..U+001F.
fn number_space(c: char) -> bool {
    py_is_space(c) && !('\u{1c}'..='\u{1f}').contains(&c)
}

/// What CPython's `_PyUnicode_TransformDecimalAndSpaceToASCII` hands the number
/// parsers: Unicode digits become ASCII digits, Unicode spaces become `' '`.
fn ascii_number_text(s: &str) -> String {
    s.chars()
        .map(|c| {
            if let Some(d) = decimal_value(c) {
                char::from(b'0' + d as u8)
            } else if number_space(c) && !c.is_ascii() {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// `float(text)`.
pub fn py_str_to_float(s: &str) -> Result<f64, PyErr> {
    py_float_parse(&ascii_number_text(s)).ok_or_else(|| {
        PyErr::value(format!(
            "could not convert string to float: {}",
            py_repr_str(s)
        ))
    })
}

/// `int(text)` (base 10), as canonical decimal text.
pub fn py_str_to_int(s: &str) -> Result<String, PyErr> {
    let err = || {
        PyErr::value(format!(
            "invalid literal for int() with base 10: {}",
            py_repr_str(s)
        ))
    };
    let t = ascii_number_text(s);
    let t = t.trim_matches(number_space);
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let b = rest.as_bytes();
    if b.is_empty() || !b[0].is_ascii_digit() {
        return Err(err());
    }
    let mut digits = String::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            digits.push(b[i] as char);
            i += 1;
        } else if b[i] == b'_' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            i += 1;
        } else {
            return Err(err());
        }
    }
    Ok(canonical_int(neg, &digits))
}

fn canonical_int(neg: bool, digits: &str) -> String {
    let d = digits.trim_start_matches('0');
    if d.is_empty() {
        "0".to_string()
    } else if neg {
        format!("-{d}")
    } else {
        d.to_string()
    }
}

/// `int(x)` for a float: exact truncation toward zero.
pub fn py_float_to_int(x: f64) -> Result<String, PyErr> {
    if x.is_nan() {
        return Err(PyErr::value("cannot convert float NaN to integer"));
    }
    if x.is_infinite() {
        return Err(PyErr::overflow("cannot convert float infinity to integer"));
    }
    // `{:.0}` of an integral double is its exact decimal value.
    let t = format!("{:.0}", x.trunc().abs());
    Ok(canonical_int(x < 0.0, &t))
}

/// `int(round(x))` for a float: ties to even, exact.
pub fn py_round_to_int(x: f64) -> Result<String, PyErr> {
    if x.is_nan() {
        return Err(PyErr::value("cannot convert float NaN to integer"));
    }
    if x.is_infinite() {
        return Err(PyErr::overflow("cannot convert float infinity to integer"));
    }
    py_float_to_int(x.round_ties_even())
}

/// `float(n)` for an int held as decimal text: correctly rounded, `OverflowError` past
/// the largest double.
pub fn int_text_to_f64(s: &str) -> Result<f64, PyErr> {
    let v: f64 = s.parse().unwrap_or(f64::NAN);
    if v.is_infinite() {
        return Err(PyErr::overflow("int too large to convert to float"));
    }
    Ok(v)
}

/// `a + b` for ints held as canonical decimal text, exact at any size.
pub fn int_text_add(a: &str, b: &str) -> String {
    let split = |t: &str| -> (bool, Vec<u8>) {
        let (neg, d) = match t.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, t),
        };
        (neg, d.bytes().rev().map(|c| c.wrapping_sub(b'0')).collect())
    };
    let (an, ad) = split(a);
    let (bn, bd) = split(b);
    // Magnitudes, little-endian digits.
    fn cmp(x: &[u8], y: &[u8]) -> std::cmp::Ordering {
        let tx = x.len() - x.iter().rev().take_while(|&&d| d == 0).count();
        let ty = y.len() - y.iter().rev().take_while(|&&d| d == 0).count();
        tx.cmp(&ty)
            .then_with(|| x[..tx].iter().rev().cmp(y[..ty].iter().rev()))
    }
    fn add(x: &[u8], y: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(x.len().max(y.len()) + 1);
        let mut carry = 0u8;
        for i in 0..x.len().max(y.len()) {
            let s = x.get(i).copied().unwrap_or(0) + y.get(i).copied().unwrap_or(0) + carry;
            out.push(s % 10);
            carry = s / 10;
        }
        if carry > 0 {
            out.push(carry);
        }
        out
    }
    // x >= y
    fn sub(x: &[u8], y: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(x.len());
        let mut borrow = 0i8;
        for (i, &xi) in x.iter().enumerate() {
            let mut d = xi as i8 - y.get(i).copied().unwrap_or(0) as i8 - borrow;
            borrow = 0;
            if d < 0 {
                d += 10;
                borrow = 1;
            }
            out.push(d as u8);
        }
        out
    }
    let (neg, mag) = if an == bn {
        (an, add(&ad, &bd))
    } else if cmp(&ad, &bd) != std::cmp::Ordering::Less {
        (an, sub(&ad, &bd))
    } else {
        (bn, sub(&bd, &ad))
    };
    let digits: String = mag.iter().rev().map(|d| char::from(b'0' + d)).collect();
    canonical_int(neg, &digits)
}

/// The sign of an int held as canonical decimal text: -1, 0 or 1.
pub fn int_text_sign(t: &str) -> i32 {
    if t == "0" {
        0
    } else if t.starts_with('-') {
        -1
    } else {
        1
    }
}

/// An int held as decimal text, if it fits `i128`.
pub fn int_text_to_i128(s: &str) -> Option<i128> {
    s.parse().ok()
}

/// `f"{n:,}"` for an int held as decimal text.
pub fn group_thousands(int_text: &str) -> String {
    let (sign, digits) = match int_text.strip_prefix('-') {
        Some(d) => ("-", d),
        None => ("", int_text),
    };
    let n = digits.len();
    let mut out = String::with_capacity(n + n / 3 + 1);
    out.push_str(sign);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------------------------
// Strings

/// `str.isprintable()` for one character.
pub fn is_printable(c: char) -> bool {
    let cp = c as u32;
    let idx = NON_PRINTABLE.partition_point(|&(a, _)| a <= cp);
    if idx == 0 {
        return true;
    }
    let (a, b) = NON_PRINTABLE[idx - 1];
    !(a <= cp && cp <= b)
}

/// `repr(s)` for a str.
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        let cp = c as u32;
        if c == quote || c == '\\' {
            out.push('\\');
            out.push(c);
        } else if c == '\t' {
            out.push_str("\\t");
        } else if c == '\n' {
            out.push_str("\\n");
        } else if c == '\r' {
            out.push_str("\\r");
        } else if cp < 0x20 || cp == 0x7f {
            out.push_str(&format!("\\x{cp:02x}"));
        } else if cp < 0x7f || is_printable(c) {
            out.push(c);
        } else if cp <= 0xff {
            out.push_str(&format!("\\x{cp:02x}"));
        } else if cp <= 0xffff {
            out.push_str(&format!("\\u{cp:04x}"));
        } else {
            out.push_str(&format!("\\U{cp:08x}"));
        }
    }
    out.push(quote);
    out
}

/// Python's `html.unescape` (HTML5 named and numeric character references).
pub fn html_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '&' {
            if let Some(end) = match_charref(&chars, i + 1) {
                let group: String = chars[i + 1..end].iter().collect();
                out.push_str(&replace_charref(&group));
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\t\n\f <&#;]{1,32};?` at `start`; the end of the
/// group on a match.
fn match_charref(c: &[char], start: usize) -> Option<usize> {
    let n = c.len();
    let opt_semi = |j: usize| if j < n && c[j] == ';' { j + 1 } else { j };
    if start < n && c[start] == '#' {
        let mut j = start + 1;
        while j < n && c[j].is_ascii_digit() {
            j += 1;
        }
        if j > start + 1 {
            return Some(opt_semi(j));
        }
        if start + 1 < n && (c[start + 1] == 'x' || c[start + 1] == 'X') {
            let mut j = start + 2;
            while j < n && c[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j > start + 2 {
                return Some(opt_semi(j));
            }
        }
        return None;
    }
    let mut j = start;
    while j < n
        && j - start < 32
        && !matches!(c[j], '\t' | '\n' | '\u{0c}' | ' ' | '<' | '&' | '#' | ';')
    {
        j += 1;
    }
    if j == start {
        return None;
    }
    Some(opt_semi(j))
}

fn html5_lookup(name: &str) -> Option<&'static str> {
    HTML5_ENTITIES
        .binary_search_by(|(k, _)| k.as_bytes().cmp(name.as_bytes()))
        .ok()
        .map(|i| HTML5_ENTITIES[i].1)
}

fn replace_charref(s: &str) -> String {
    if let Some(rest) = s.strip_prefix('#') {
        let (digits, radix) = match rest.strip_prefix(['x', 'X']) {
            Some(hex) => (hex.trim_end_matches(';'), 16),
            None => (rest.trim_end_matches(';'), 10),
        };
        // Python's int is unbounded; anything past u32 is simply "> 0x10FFFF".
        let num = u32::from_str_radix(digits.trim_start_matches('0'), radix)
            .ok()
            .or(if digits.trim_start_matches('0').is_empty() {
                Some(0)
            } else {
                None
            })
            .unwrap_or(u32::MAX);
        if let Some((_, v)) = INVALID_CHARREFS.iter().find(|(k, _)| *k == num) {
            return (*v).to_string();
        }
        if (0xD800..=0xDFFF).contains(&num) || num > 0x10FFFF {
            return "\u{FFFD}".to_string();
        }
        if INVALID_CODEPOINTS.contains(&num) {
            return String::new();
        }
        return char::from_u32(num).map(String::from).unwrap_or_default();
    }
    if let Some(v) = html5_lookup(s) {
        return v.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut x = chars.len().saturating_sub(1);
    while x > 1 {
        let prefix: String = chars[..x].iter().collect();
        if let Some(v) = html5_lookup(&prefix) {
            let tail: String = chars[x..].iter().collect();
            return format!("{v}{tail}");
        }
        x -= 1;
    }
    format!("&{s}")
}

/// Python's stable `list.sort` with a `<` predicate, as a binary insertion sort.
///
/// Identical to CPython's result whenever `lt` is a consistent order. With NaN keys
/// (where `<` is not an order) CPython's run detection can place items differently.
pub fn py_sort_by<T>(items: &mut [T], lt: impl Fn(&T, &T) -> bool) {
    for i in 1..items.len() {
        // The first position whose element is greater than items[i] (stable).
        let (mut lo, mut hi) = (0usize, i);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if lt(&items[i], &items[mid]) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        items[lo..=i].rotate_right(1);
    }
}

// ---------------------------------------------------------------------------------------
// json.loads

/// CPython's default recursion limit, a stand-in for where `json.loads` gives up on
/// deeply nested input (CPython's exact depth depends on the C stack).
const JSON_MAX_DEPTH: usize = 1000;

/// Python's `json.loads(text)` with the default (strict) decoder.
///
/// A decode error is `ValueError` (Python's `JSONDecodeError` subclasses it); the
/// message is not Python's, since every caller replaces it with its own. A lone
/// surrogate escape (`"\ud800"`), which Python keeps in the str, becomes U+FFFD.
pub fn json_loads(text: &str) -> Result<PyValue, PyErr> {
    if text.starts_with('\u{feff}') {
        return Err(PyErr::value(
            "Unexpected UTF-8 BOM (decode using utf-8-sig)",
        ));
    }
    let chars: Vec<char> = text.chars().collect();
    let mut p = JsonParser { c: &chars, i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != chars.len() {
        return Err(p.err("Extra data"));
    }
    Ok(v)
}

struct JsonParser<'a> {
    c: &'a [char],
    i: usize,
}

impl JsonParser<'_> {
    fn err(&self, what: &str) -> PyErr {
        PyErr::value(format!("{what}: char {}", self.i))
    }

    fn ws(&mut self) {
        while self.i < self.c.len() && matches!(self.c[self.i], ' ' | '\t' | '\n' | '\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    fn starts(&self, lit: &str) -> bool {
        lit.chars()
            .enumerate()
            .all(|(k, ch)| self.c.get(self.i + k) == Some(&ch))
    }

    fn value(&mut self, depth: usize) -> Result<PyValue, PyErr> {
        let Some(ch) = self.peek() else {
            return Err(self.err("Expecting value"));
        };
        match ch {
            '"' => {
                self.i += 1;
                Ok(PyValue::Str(self.string()?))
            }
            '{' => {
                if depth >= JSON_MAX_DEPTH {
                    return Err(PyErr::new(
                        PyErrKind::RecursionError,
                        "maximum recursion depth exceeded while decoding a JSON object from a unicode string",
                    ));
                }
                self.i += 1;
                self.object(depth + 1)
            }
            '[' => {
                if depth >= JSON_MAX_DEPTH {
                    return Err(PyErr::new(
                        PyErrKind::RecursionError,
                        "maximum recursion depth exceeded while decoding a JSON array from a unicode string",
                    ));
                }
                self.i += 1;
                self.array(depth + 1)
            }
            'n' if self.starts("null") => {
                self.i += 4;
                Ok(PyValue::None)
            }
            't' if self.starts("true") => {
                self.i += 4;
                Ok(PyValue::Bool(true))
            }
            'f' if self.starts("false") => {
                self.i += 5;
                Ok(PyValue::Bool(false))
            }
            'N' if self.starts("NaN") => {
                self.i += 3;
                Ok(PyValue::Float(f64::NAN))
            }
            'I' if self.starts("Infinity") => {
                self.i += 8;
                Ok(PyValue::Float(f64::INFINITY))
            }
            '-' if self.starts("-Infinity") => {
                self.i += 9;
                Ok(PyValue::Float(f64::NEG_INFINITY))
            }
            _ => self.number(),
        }
    }

    fn number(&mut self) -> Result<PyValue, PyErr> {
        let c = self.c;
        let start = self.i;
        let mut j = self.i;
        let neg = c.get(j) == Some(&'-');
        if neg {
            j += 1;
        }
        match c.get(j) {
            Some('0') => j += 1,
            Some(d) if ('1'..='9').contains(d) => {
                while j < c.len() && c[j].is_ascii_digit() {
                    j += 1;
                }
            }
            _ => return Err(self.err("Expecting value")),
        }
        let int_end = j;
        let mut is_float = false;
        if c.get(j) == Some(&'.') && c.get(j + 1).is_some_and(|d| d.is_ascii_digit()) {
            j += 2;
            while j < c.len() && c[j].is_ascii_digit() {
                j += 1;
            }
            is_float = true;
        }
        if matches!(c.get(j), Some('e') | Some('E')) {
            let mut k = j + 1;
            if matches!(c.get(k), Some('+') | Some('-')) {
                k += 1;
            }
            let digits_start = k;
            while k < c.len() && c[k].is_ascii_digit() {
                k += 1;
            }
            if k > digits_start {
                j = k;
                is_float = true;
            }
        }
        self.i = j;
        let text: String = c[start..j].iter().collect();
        if is_float {
            Ok(PyValue::Float(text.parse().unwrap_or(f64::NAN)))
        } else {
            let digits: String = c[if neg { start + 1 } else { start }..int_end]
                .iter()
                .collect();
            Ok(PyValue::Int(canonical_int(neg, &digits)))
        }
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        if at + 4 > self.c.len() {
            return None;
        }
        let mut v = 0u32;
        for k in 0..4 {
            v = v * 16 + self.c[at + k].to_digit(16)?;
        }
        Some(v)
    }

    fn string(&mut self) -> Result<String, PyErr> {
        let mut out = String::new();
        loop {
            let Some(ch) = self.peek() else {
                return Err(self.err("Unterminated string starting at"));
            };
            self.i += 1;
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let Some(e) = self.peek() else {
                        return Err(self.err("Unterminated string starting at"));
                    };
                    self.i += 1;
                    match e {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let Some(u) = self.hex4(self.i) else {
                                return Err(self.err("Invalid \\uXXXX escape"));
                            };
                            self.i += 4;
                            let mut cp = u;
                            if (0xd800..=0xdbff).contains(&u)
                                && self.c.get(self.i) == Some(&'\\')
                                && self.c.get(self.i + 1) == Some(&'u')
                            {
                                if let Some(lo) = self.hex4(self.i + 2) {
                                    if (0xdc00..=0xdfff).contains(&lo) {
                                        cp = 0x10000 + (((u - 0xd800) << 10) | (lo - 0xdc00));
                                        self.i += 6;
                                    }
                                }
                            }
                            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        }
                        _ => return Err(self.err("Invalid \\escape")),
                    }
                }
                c if (c as u32) < 0x20 => return Err(self.err("Invalid control character at")),
                c => out.push(c),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<PyValue, PyErr> {
        let mut d = PyDict::new();
        self.ws();
        if self.peek() == Some('}') {
            self.i += 1;
            return Ok(PyValue::Dict(d));
        }
        loop {
            if self.peek() != Some('"') {
                return Err(self.err("Expecting property name enclosed in double quotes"));
            }
            self.i += 1;
            let key = self.string()?;
            self.ws();
            if self.peek() != Some(':') {
                return Err(self.err("Expecting ':' delimiter"));
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth)?;
            d.insert(key, v);
            self.ws();
            match self.peek() {
                Some('}') => {
                    self.i += 1;
                    return Ok(PyValue::Dict(d));
                }
                Some(',') => {
                    self.i += 1;
                    self.ws();
                }
                _ => return Err(self.err("Expecting ',' delimiter")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<PyValue, PyErr> {
        let mut v = Vec::new();
        self.ws();
        if self.peek() == Some(']') {
            self.i += 1;
            return Ok(PyValue::List(v));
        }
        loop {
            v.push(self.value(depth)?);
            self.ws();
            match self.peek() {
                Some(']') => {
                    self.i += 1;
                    return Ok(PyValue::List(v));
                }
                Some(',') => {
                    self.i += 1;
                    self.ws();
                }
                _ => return Err(self.err("Expecting ',' delimiter")),
            }
        }
    }
}
