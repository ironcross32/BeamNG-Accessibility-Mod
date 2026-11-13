//! Golden cases from `rust/fixtures/gen/pyfmt.py`.

use beam_base::pyfmt::*;
use beam_testutil::{cases, f, load, s, same_f64};
use serde_json::Value;

fn expect_float(v: &Value) -> Result<f64, String> {
    match v {
        Value::Number(n) => Ok(f64::from_bits(n.as_u64().expect("bits"))),
        Value::String(e) => Err(e.clone()),
        other => panic!("bad fixture value {other}"),
    }
}

#[test]
fn round_repr_g_f_mod_match_python() {
    let fx = load("pyfmt");
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for case in cases(&fx, "cases") {
        let x = f(&case["x"]);
        let label = s(&case["x"]["repr"]);

        let mut check_round = |key: &str| {
            if let Some(map) = case.get(key).and_then(Value::as_object) {
                for (n, want) in map {
                    let n: i32 = n.parse().unwrap();
                    let got = try_py_round(x, n);
                    match expect_float(want) {
                        Ok(w) => {
                            if !matches!(got, Ok(g) if same_f64(g, w)) {
                                failures
                                    .push(format!("round({label}, {n}): got {got:?}, want {w:?}"));
                            }
                        }
                        Err(e) => {
                            if got.is_ok() {
                                failures.push(format!(
                                    "round({label}, {n}): got {got:?}, Python raised {e}"
                                ));
                            }
                            if !same_f64(py_round(x, n).abs(), f64::INFINITY) {
                                failures
                                    .push(format!("py_round({label}, {n}) should saturate to inf"));
                            }
                        }
                    }
                    checked += 1;
                }
            }
        };
        check_round("round");
        check_round("round_extra");

        let r0 = s(&case["round0"]);
        match try_py_round0(x) {
            Ok(v) => {
                if v.to_string() != r0 {
                    failures.push(format!("round({label}): got {v}, want {r0}"));
                }
            }
            Err(e) => {
                // Out of i64 range is fine; a Python exception must be the same one.
                let python_raised = r0 == "ValueError" || r0 == "OverflowError";
                if python_raised && e != r0 {
                    failures.push(format!("round({label}): got Err({e}), want {r0}"));
                }
                if !python_raised && e != "out of i64 range" {
                    failures.push(format!("round({label}): got Err({e}), want {r0}"));
                }
            }
        }
        let i = s(&case["int"]);
        match try_py_int(x) {
            Ok(v) if v.to_string() == i => {}
            Err(e)
                if e == i
                    || (e == "out of i64 range"
                        && i.trim_start_matches('-')
                            .bytes()
                            .all(|b| b.is_ascii_digit())
                        && i.parse::<i64>().is_err()) => {}
            other => failures.push(format!("int({label}): got {other:?}, want {i}")),
        }

        for key in ["repr", "str", "fstr"] {
            let want = s(&case[key]);
            let got = py_float_str(x);
            if got != want {
                failures.push(format!("{key}({label}): got {got:?}, want {want:?}"));
            }
        }

        for (p, want) in case["g"].as_object().unwrap() {
            let got = py_g(x, p.parse().unwrap());
            if got != s(want) {
                failures.push(format!(
                    "format({label}, '.{p}g'): got {got:?}, want {want}"
                ));
            }
        }
        for key in ["g_default", "pct_g"] {
            let got = py_g(x, 6);
            if got != s(&case[key]) {
                failures.push(format!("{key}({label}): got {got:?}, want {}", case[key]));
            }
        }
        for (p, want) in case["f"].as_object().unwrap() {
            let got = py_f(x, p.parse().unwrap());
            if got != s(want) {
                failures.push(format!(
                    "format({label}, '.{p}f'): got {got:?}, want {want}"
                ));
            }
        }
        for (m, want) in case["mod"].as_object().unwrap() {
            let y: f64 = m.parse().unwrap();
            let got = py_mod(x, y);
            match expect_float(want) {
                Ok(w) if same_f64(got, w) => {}
                Ok(w) => failures.push(format!("{label} % {m}: got {got:?}, want {w:?}")),
                Err(e) => failures.push(format!("{label} % {m}: Python raised {e}")),
            }
        }
        checked += 1;
    }
    assert!(
        failures.is_empty(),
        "{} of {} checks failed:\n{}",
        failures.len(),
        checked,
        failures
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(checked > 10_000, "only {checked} checks ran");
}

/// Python accepts non-ASCII decimal digits in float(); the helper does not (documented).
const FLOAT_PARSE_KNOWN_DIFFERENCES: &[&str] = &["١٢"];

#[test]
fn float_parse_matches_python() {
    let fx = load("pyfmt");
    let mut failures = Vec::new();
    for case in cases(&fx, "float_parse") {
        let text = s(&case["s"]);
        if FLOAT_PARSE_KNOWN_DIFFERENCES.contains(&text) {
            continue;
        }
        let got = py_float_parse(text);
        match case.get("ok") {
            Some(want) => {
                let w = f(want);
                if !matches!(got, Some(g) if same_f64(g, w)) {
                    failures.push(format!("float({text:?}): got {got:?}, want {w:?}"));
                }
            }
            None => {
                if got.is_some() {
                    failures.push(format!("float({text:?}): got {got:?}, Python raised"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn isspace_matches_python() {
    let fx = load("pyfmt");
    let want: Vec<u32> = cases(&fx, "isspace")
        .iter()
        .map(|v| v.as_u64().unwrap() as u32)
        .collect();
    let got: Vec<u32> = (0u32..0x110000)
        .filter_map(char::from_u32)
        .filter(|&c| py_is_space(c))
        .map(|c| c as u32)
        .collect();
    assert_eq!(got, want);
}
