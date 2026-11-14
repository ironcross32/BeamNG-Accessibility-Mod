//! Golden cases from `rust/fixtures/gen/speech_config.py`.

use beam_base::pyval::{PyValue, json_dumps_indent2, json_loads};
use beam_base::speech_config::{AUTO_BACKEND, migrate_config};
use beam_testutil::{load, s};

#[test]
fn migrate_config_matches_python() {
    let fx = load("speech_config");
    assert_eq!(AUTO_BACKEND, s(&fx["AUTO_BACKEND"]));
    let mut failures = Vec::new();
    for case in fx["cases"].as_array().unwrap() {
        let before = s(&case["before"]);
        let PyValue::Dict(mut cfg) = json_loads(before).unwrap() else {
            panic!("not a dict")
        };
        let changed = migrate_config(&mut cfg);
        let want = json_loads(s(&case["after"])).unwrap();
        let got = json_dumps_indent2(&PyValue::Dict(cfg));
        if changed != case["changed"].as_bool().unwrap() || got != json_dumps_indent2(&want) {
            failures.push(format!("{before}: got {got} changed {changed}, want {case}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
