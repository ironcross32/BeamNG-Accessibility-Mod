//! `json.dumps` with Python's defaults (`ensure_ascii=True`), in the two separator styles
//! the spawner sends on 4461: the default `", "`/`": "` and the compact `","`/`":"`.

use beam_base::pyfmt::py_float_str;
use beam_base::pyval::PyValue;

fn escape_into(out: &mut String, s: &str) {
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
            ' '..='~' => out.push(c),
            c => {
                // ensure_ascii: everything outside printable ASCII is \uXXXX, astral
                // characters as a UTF-16 surrogate pair.
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

fn float_into(out: &mut String, f: f64) {
    if f.is_nan() {
        out.push_str("NaN");
    } else if f == f64::INFINITY {
        out.push_str("Infinity");
    } else if f == f64::NEG_INFINITY {
        out.push_str("-Infinity");
    } else {
        out.push_str(&py_float_str(f));
    }
}

fn dump_into(out: &mut String, v: &PyValue, item_sep: &str, key_sep: &str) {
    match v {
        PyValue::None => out.push_str("null"),
        PyValue::Bool(true) => out.push_str("true"),
        PyValue::Bool(false) => out.push_str("false"),
        PyValue::Int(i) => out.push_str(&i.to_string()),
        PyValue::Float(f) => float_into(out, *f),
        PyValue::Str(s) => escape_into(out, s),
        PyValue::List(items) => {
            out.push('[');
            for (n, item) in items.iter().enumerate() {
                if n > 0 {
                    out.push_str(item_sep);
                }
                dump_into(out, item, item_sep, key_sep);
            }
            out.push(']');
        }
        PyValue::Dict(d) => {
            out.push('{');
            for (n, (k, item)) in d.iter().enumerate() {
                if n > 0 {
                    out.push_str(item_sep);
                }
                escape_into(out, k);
                out.push_str(key_sep);
                dump_into(out, item, item_sep, key_sep);
            }
            out.push('}');
        }
    }
}

/// `json.dumps(v)`.
pub(super) fn dumps(v: &PyValue) -> String {
    let mut out = String::new();
    dump_into(&mut out, v, ", ", ": ");
    out
}

/// `json.dumps(v, separators=(",", ":"))`.
pub(super) fn dumps_compact(v: &PyValue) -> String {
    let mut out = String::new();
    dump_into(&mut out, v, ",", ":");
    out
}
