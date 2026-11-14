//! Golden cases from `rust/fixtures/gen/ai_registry.py`.

use beam_base::ai_registry::*;
use beam_base::pyval::{PyValue, json_loads};
use beam_testutil::{load, s};
use serde_json::Value;

fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| (s(&p[0]).to_string(), s(&p[1]).to_string()))
        .collect()
}

fn own(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn constants_match_python() {
    let fx = load("ai_registry");
    let c = &fx["constants"];
    assert_eq!(pairs(&c["PROVIDERS"]), own(&PROVIDERS));
    assert_eq!(DEFAULT_PROVIDER, s(&c["DEFAULT_PROVIDER"]));
    assert_eq!(PROVIDER_CFG_KEY, s(&c["PROVIDER_CFG_KEY"]));
    assert_eq!(GEMINI_BASE, s(&c["GEMINI_BASE"]));
    assert_eq!(GEMINI_DEFAULT_MODEL, s(&c["GEMINI_DEFAULT_MODEL"]));
    assert_eq!(OPENAI_BASE, s(&c["OPENAI_BASE"]));
    assert_eq!(OPENAI_DEFAULT_BASE_URL, s(&c["OPENAI_DEFAULT_BASE_URL"]));
    assert_eq!(OPENAI_DEFAULT_MODEL, s(&c["OPENAI_DEFAULT_MODEL"]));
    assert_eq!(
        pairs(&c["OPENAI_REASONING_EFFORTS"]),
        own(&OPENAI_REASONING_EFFORTS)
    );
    assert_eq!(
        pairs(&c["OPENAI_DETAIL_LEVELS"]),
        own(&OPENAI_DETAIL_LEVELS)
    );
    let keys: Vec<&str> = fx["all_config_keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(s)
        .collect();
    assert_eq!(all_config_keys(), keys);
}

#[test]
fn provider_lookups_match_python() {
    let fx = load("ai_registry");
    for case in fx["providers"].as_array().unwrap() {
        // Python's None id falls back like any unknown id.
        let pid = case["pid"].as_str().unwrap_or("\u{0}none");
        assert_eq!(provider_display_name(pid), s(&case["display"]), "{pid}");
        assert_eq!(own(vision_models_for(pid)), pairs(&case["models"]), "{pid}");
        let names: Vec<&str> = case["model_names"]
            .as_array()
            .unwrap()
            .iter()
            .map(s)
            .collect();
        assert_eq!(model_names_for(pid), names);
        assert_eq!(default_model_for(pid), s(&case["default_model"]));
        let (k, m) = config_keys_for(pid);
        assert_eq!(
            vec![k, m],
            vec![s(&case["config_keys"][0]), s(&case["config_keys"][1])]
        );
        assert_eq!(key_help_for(pid), s(&case["key_help"]));
        let extras = extras_for(pid);
        let want = case["extras"].as_array().unwrap();
        assert_eq!(extras.len(), want.len());
        for (ex, w) in extras.iter().zip(want) {
            assert_eq!(ex.key, s(&w["key"]));
            assert_eq!(ex.arg, s(&w["arg"]));
            assert_eq!(ex.kind.as_str(), s(&w["kind"]));
            assert_eq!(ex.label, s(&w["label"]));
            assert_eq!(ex.default, s(&w["default"]));
            assert_eq!(ex.help, s(&w["help"]));
            assert_eq!(own(ex.values), pairs(&w["values"]));
        }
    }
}

#[test]
fn extra_args_and_display_names_match_python() {
    let fx = load("ai_registry");
    for case in fx["extra_args"].as_array().unwrap() {
        let pid = s(&case["pid"]);
        let cfg = match json_loads(s(&case["cfg"])).unwrap() {
            PyValue::Dict(d) => Some(d),
            _ => None,
        };
        let got = extra_args_for(pid, cfg.as_ref());
        match (&got, case.get("ok")) {
            (Ok(opts), Some(want)) => {
                let got_pairs: Vec<(String, String)> = opts
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect();
                assert_eq!(got_pairs, pairs(want), "{case}");
            }
            (Err(e), None) => assert_eq!(e.to_string(), s(&case["err"]), "{case}"),
            _ => panic!("extra_args_for: got {got:?}, want {case}"),
        }
    }
    for case in fx["display_names"].as_array().unwrap() {
        assert_eq!(
            display_name_for(s(&case["model"]), s(&case["pid"])),
            s(&case["display"]),
            "{case}"
        );
    }
}
