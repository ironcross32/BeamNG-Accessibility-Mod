//! Golden cases from `fixtures/gen/road_json.py`: json.loads acceptance and the
//! exact bytes of json.dumps under each option set the recorders use.

use beam_road::pyjson::{DumpOpts, dumps, loads};
use beam_testutil::{cases, load, s};

fn opts(name: &str) -> DumpOpts {
    match name {
        "compact_ascii_nonan" => DumpOpts::compact().allow_nan(false),
        "compact_utf8" => DumpOpts::compact().ensure_ascii(false),
        "default_utf8" => DumpOpts::default_seps().ensure_ascii(false),
        "indent2" => DumpOpts::indented(2),
        "indent2_nonan" => DumpOpts::indented(2).allow_nan(false),
        other => panic!("unknown option set {other}"),
    }
}

#[test]
fn loads_and_dumps_match_python() {
    let fx = load("road_json");
    let all = cases(&fx, "cases");
    let mut failures = Vec::new();
    for case in all {
        let text = s(&case["text"]);
        let decoded = loads(text);
        let want = case["decodes"].as_bool().unwrap();
        match (&decoded, want) {
            (Ok(_), false) => failures.push(format!("{text:?}: accepted, Python rejects")),
            (Err(e), true) => failures.push(format!("{text:?}: rejected ({e}), Python accepts")),
            _ => {}
        }
        let (Ok(value), true) = (decoded, want) else {
            continue;
        };
        for (name, expect) in case["encoded"].as_object().unwrap() {
            let got = dumps(&value, &opts(name));
            match (got, expect.get("ok")) {
                (Ok(g), Some(w)) if g == s(w) => {}
                (Err(e), None) if e.message() == s(&expect["msg"]) => {}
                (got, _) => failures.push(format!("{text:?} [{name}]: got {got:?}, want {expect}")),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
