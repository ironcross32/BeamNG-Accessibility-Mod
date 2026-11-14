//! Golden cases from `rust/fixtures/gen/secretstore.py`, plus a live DPAPI round trip
//! (DPAPI is per Windows user, so sealing cannot come from a committed fixture).

use beam_base::pyval::{PyDict, PyValue};
use beam_base::secretstore::*;
use beam_testutil::{load, s};

#[test]
fn constants_and_names_match_python() {
    let fx = load("secretstore");
    assert_eq!(PREFIX, s(&fx["PREFIX"]));
    let parts: Vec<&str> = fx["SECRET_NAME_PARTS"]
        .as_array()
        .unwrap()
        .iter()
        .map(s)
        .collect();
    assert_eq!(parts, SECRET_NAME_PARTS);
    assert_eq!(
        fx["CRYPTPROTECT_UI_FORBIDDEN"].as_u64(),
        Some(CRYPTPROTECT_UI_FORBIDDEN as u64)
    );
    for case in fx["names"].as_array().unwrap() {
        let n = s(&case["name"]);
        assert_eq!(
            is_secret_setting(n),
            case["secret"].as_bool().unwrap(),
            "{n:?}"
        );
    }
    for case in fx["protected"].as_array().unwrap() {
        let v = s(&case["value"]);
        assert_eq!(
            is_protected(v),
            case["protected"].as_bool().unwrap(),
            "{v:?}"
        );
    }
    for case in fx["unprotect_plain"].as_array().unwrap() {
        let v = s(&case["value"]);
        assert_eq!(unprotect(v).as_deref(), case["out"].as_str(), "{v:?}");
    }
}

#[test]
fn b64decode_matches_python() {
    let fx = load("secretstore");
    for case in fx["b64decode"].as_array().unwrap() {
        let text = s(&case["text"]);
        let got =
            py_b64decode(text).map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>());
        assert_eq!(got.as_deref(), case["ok"].as_str(), "b64decode({text:?})");
    }
}

#[test]
fn live_round_trip() {
    let sealed = protect("  sk-test-é  ");
    assert!(is_protected(&sealed), "{sealed}");
    assert_eq!(unprotect(&sealed).as_deref(), Some("sk-test-é"));
    // Whitespace inside the stored blob is skipped by the base64 decode, as in Python.
    let spaced = format!("{} {}", &sealed[..20], &sealed[20..]);
    assert_eq!(unprotect(&spaced).as_deref(), Some("sk-test-é"));
    // Sealed values and blanks pass through protect unchanged (blank is stripped).
    assert_eq!(protect(&sealed), sealed);
    assert_eq!(protect("   "), "");
    // A tampered blob is None, not "".
    let mut tampered = sealed.clone();
    tampered.truncate(tampered.len() - 8);
    tampered.push_str("AAAAAAAA");
    assert_eq!(unprotect(&tampered), None);
}

#[test]
fn migrate_config_seals_secret_names_only() {
    let mut cfg: PyDict = [
        ("ai_describer_api_key", PyValue::from(" key ")),
        ("my_token", PyValue::from("")),
        ("password", PyValue::from(5i64)),
        ("units", PyValue::from("metric")),
        ("client_secret", PyValue::from("dpapi:v1:AAAA")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert!(migrate_config(&mut cfg));
    let key = cfg["ai_describer_api_key"].as_str().unwrap();
    assert!(is_protected(key));
    assert_eq!(unprotect(key).as_deref(), Some("key"));
    assert_eq!(cfg["my_token"], PyValue::from(""));
    assert_eq!(cfg["password"], PyValue::from(5i64));
    assert_eq!(cfg["units"], PyValue::from("metric"));
    assert_eq!(cfg["client_secret"], PyValue::from("dpapi:v1:AAAA"));
    assert!(!migrate_config(&mut cfg), "idempotent");
}
