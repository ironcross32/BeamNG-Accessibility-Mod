//! Moved out of `src/describer.rs`: see build.rs for why this crate's tests are integration tests.

use beam_app::describer::*;
use beam_base::config;
use beam_base::secretstore;
use serde_json::Map;

#[test]
fn state_hides_secrets() {
    let mut c = config::default_config();
    c.insert("ai_describer_openai_api_key".into(), "dpapi:v1:abc".into());
    let s = state(&c);
    assert_eq!(s["providers"]["openai"]["has_key"], true);
    assert_eq!(s["providers"]["gemini"]["has_key"], false);
    assert!(!s.to_string().contains("dpapi"));
}

#[test]
fn save_sets_clears_and_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("beamtel_config.json");
    let mut snap = config::load_config_configurator_at(&path).unwrap();
    let args = SaveArgs {
        provider: "openai".into(),
        model: "gpt-5.6-sol".into(),
        extras: Map::new(),
        disable_ui_toggle: true,
        key_action: "set".into(),
        key: Some("sk-test".into()),
    };
    let r = save_at(&path, &mut snap, &args);
    assert_eq!(r["ok"], true);
    assert_eq!(r["state"]["provider"], "openai");
    assert_eq!(r["state"]["providers"]["openai"]["has_key"], true);
    assert_eq!(r["state"]["providers"]["openai"]["model"], "gpt-5.6-sol");
    assert_eq!(r["state"]["disable_ui_toggle"], true);
    let disk = config::load_config_configurator_at(&path).unwrap();
    let stored = disk["ai_describer_openai_api_key"].py_str();
    assert!(secretstore::is_protected(&stored));
    assert_eq!(secretstore::unprotect(&stored).as_deref(), Some("sk-test"));
    let r = save_at(
        &path,
        &mut snap,
        &SaveArgs {
            key_action: "clear".into(),
            key: None,
            ..args
        },
    );
    assert_eq!(r["state"]["providers"]["openai"]["has_key"], false);
}
