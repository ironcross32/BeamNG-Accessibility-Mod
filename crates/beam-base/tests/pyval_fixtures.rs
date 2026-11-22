//! Golden cases from `fixtures/gen/pyval.py`.

use beam_base::pyval::{json_dumps_indent2, json_loads};
use beam_testutil::{f, load, s, same_f64};

#[test]
fn json_loads_and_dumps_match_python() {
    let fx = load("pyval");
    let cases = fx["loads"].as_array().unwrap();
    let mut failures = Vec::new();
    for case in cases {
        let text = s(&case["text"]);
        let got = json_loads(text);
        match (case.get("ok"), &got) {
            (Some(want), Ok(v)) if json_dumps_indent2(v) == s(want) => {}
            (None, Err(_)) => {}
            _ => failures.push(format!("{text:?}: got {got:?}, want {case}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn coercions_match_python() {
    let fx = load("pyval");
    let cases = fx["values"].as_array().unwrap();
    let mut failures = Vec::new();
    for case in cases {
        let text = s(&case["json"]);
        let v = json_loads(text).expect("fixture value parses");
        let short: String = text.chars().take(60).collect();
        if v.py_str() != s(&case["str"]) {
            failures.push(format!("str({short}) = {:?}", v.py_str()));
        }
        if v.py_repr() != s(&case["repr"]) {
            failures.push(format!("repr({short}) = {:?}", v.py_repr()));
        }
        if v.truthy() != case["bool"].as_bool().unwrap() {
            failures.push(format!("bool({short}) = {}", v.truthy()));
        }
        let int_ok = match (v.py_int(), case["int"].get("ok")) {
            (Ok(n), Some(want)) => n.to_string() == s(want),
            (Err(e), None) => e.to_string() == s(&case["int"]["err"]),
            _ => false,
        };
        if !int_ok {
            failures.push(format!(
                "int({short}) = {:?}, want {}",
                v.py_int(),
                case["int"]
            ));
        }
        let float_ok = match (v.py_float(), case["float"].get("ok")) {
            (Ok(x), Some(want)) => same_f64(x, f(want)),
            (Err(e), None) => e.to_string() == s(&case["float"]["err"]),
            _ => false,
        };
        if !float_ok {
            failures.push(format!(
                "float({short}) = {:?}, want {}",
                v.py_float(),
                case["float"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
