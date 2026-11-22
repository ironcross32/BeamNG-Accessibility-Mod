//! Moved out of `src/panel.rs`: see build.rs for why this crate's tests are integration tests.

use beam_app::panel::*;
use beam_base::config;
use beam_base::hrtf_sets;
use beam_base::pyval::{PyDict, PyValue};
use serde_json::{Map, json};

fn defaults() -> PyDict {
    config::default_config()
}

#[test]
fn every_owned_key_has_a_default() {
    for (k, _) in PANEL_KEYS {
        assert!(config::default_for(k).is_some(), "{k}");
    }
}

#[test]
fn defaults_load_as_every_key() {
    let v = control_values(&defaults()).unwrap();
    let m = v["values"].as_object().unwrap();
    assert_eq!(m.len(), 79);
    assert_eq!(m["units"], "imperial");
    assert_eq!(m["hrtf_set"], hrtf_sets::DEFAULT_HRTF_SET);
    assert!(m["scanner_base_freq_hz"].is_i64());
}

#[test]
fn load_normalises_like_load_into_controls() {
    let mut c = defaults();
    c.insert("units".into(), "Metric".into());
    c.insert("telemetry_protocol".into(), "OUTGAUGE".into());
    c.insert("beamng_renderer".into(), "Vulkan".into());
    c.insert("obstacle_warning_sensitivity".into(), "weird".into());
    c.insert(
        "scanner_distance_callout_interval".into(),
        PyValue::Float(30.0),
    );
    c.insert("speed_announce_interval".into(), 33i64.into());
    c.insert("scanner_base_freq_hz".into(), PyValue::Float(1002.5));
    c.insert("hrtf_set".into(), "nope".into());
    let v = control_values(&c).unwrap();
    let m = &v["values"];
    assert_eq!(m["units"], "metric");
    assert_eq!(m["telemetry_protocol"], "outgauge");
    assert_eq!(m["beamng_renderer"], "vulkan");
    assert_eq!(m["obstacle_warning_sensitivity"], "normal");
    assert_eq!(m["scanner_distance_callout_interval"], 30);
    assert_eq!(m["speed_announce_interval"], 25);
    assert_eq!(m["scanner_base_freq_hz"], 1002);
    assert_eq!(m["hrtf_set"], hrtf_sets::DEFAULT_HRTF_SET);
}

#[test]
fn save_coerces_and_keeps_foreign_keys() {
    let snap = defaults();
    let mut args = Map::new();
    args.insert("shift_tone_frequency_hz".into(), json!(880));
    args.insert("speech_rate".into(), json!(60));
    args.insert("scanner_base_freq_hz".into(), json!(1200));
    args.insert("scanner_pitch_offset_oct".into(), json!(0.375));
    args.insert("launch_beamng".into(), json!(true));
    let mut disk = defaults();
    disk.insert("ai_describer_provider".into(), "openai".into());
    disk.insert("pending_update_version".into(), "9.9.9".into());
    disk.insert("route_beacon_volume_db".into(), PyValue::Float(-3.0));
    let cfg = build_saved_config(&snap, &args, Some(&disk));
    assert_eq!(cfg["shift_tone_frequency_hz"], PyValue::Float(880.0));
    assert_eq!(cfg["speech_rate"], PyValue::from(60i64));
    assert_eq!(cfg["scanner_base_freq_hz"], PyValue::Float(1200.0));
    // 0.375 * 12 = 4.5 exactly, which rounds half to even: 4/12.
    assert_eq!(cfg["scanner_pitch_offset_oct"], PyValue::Float(4.0 / 12.0));
    assert_eq!(cfg["route_beacon_volume_db"], PyValue::Float(-3.0));
    assert_eq!(cfg["ai_describer_provider"], PyValue::from("openai"));
    assert_eq!(cfg["pending_update_version"], PyValue::from("9.9.9"));
}

/// python-bugs.md 25: after Reset to Defaults the snapshot is DEFAULT_CONFIG. The save
/// that follows resets the panel's own keys and nothing else: a key the panel has no
/// control for keeps its value on disk, and an unknown key survives.
#[test]
fn save_after_reset_keeps_keys_the_panel_does_not_own() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("beamtel_config.json");
    std::fs::write(
        &path,
        r#"{"route_beacon_volume_db": -3.0, "units": "metric", "from_a_newer_build": [1, 2]}"#,
    )
    .unwrap();
    let mut snap = defaults(); // config_reset
    let values = control_values(&snap).unwrap();
    let args = values["values"].as_object().unwrap().clone();
    let r = save_panel_at(&path, &mut snap, &args);
    assert_eq!(r, json!({"ok": true}));
    let back = config::load_config_configurator_at(&path).unwrap();
    assert_eq!(
        back["units"],
        PyValue::from("imperial"),
        "the panel's own key resets"
    );
    assert_eq!(back["route_beacon_volume_db"], PyValue::Float(-3.0));
    assert!(back.contains_key("from_a_newer_build"));
}

#[test]
fn save_writes_and_updates_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("beamtel_config.json");
    let mut snap = config::load_config_configurator_at(&path).unwrap();
    let mut args = Map::new();
    args.insert("units".into(), json!("metric"));
    let r = save_panel_at(&path, &mut snap, &args);
    assert_eq!(r, json!({"ok": true}));
    assert_eq!(snap["units"], PyValue::from("metric"));
    let back = config::load_config_configurator_at(&path).unwrap();
    assert_eq!(back["units"], PyValue::from("metric"));
}
