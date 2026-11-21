//! The Configuration panel's backend (`config_ui.ConfigPanel`): `load_into_controls`
//! as the `config_load` values, `_auto_save` plus `controls_to_config` as
//! `config_save_panel`, and `on_reset`'s snapshot swap. See `rust/ui/CONTRACT.md`
//! ("Configuration panel") for the rules this follows line by line.
//!
//! The panel uses `configurator.load_config` (config_ui imports it from there), in both
//! modes.

use std::path::Path;

use beam_base::config;
use beam_base::pyval::{PyDict, PyValue, from_json, to_json};
use beam_base::{ai_registry, gear_report, hrtf_sets};
use serde_json::{Map, Value, json};

/// The JSON type each owned key is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Str,
    Int,
    Float,
    Bool,
}

use Kind::{Bool as B, Float as F, Int as I, Str as S};

/// Every key `controls_to_config` writes, with its type (CONTRACT.md's table, in order).
pub const PANEL_KEYS: [(&str, Kind); 79] = [
    ("speech_backend", S),
    ("speech_voice_name", S),
    ("speech_rate", I),
    ("speech_volume", I),
    ("units", S),
    ("telemetry_protocol", S),
    ("launch_beamng", B),
    ("beamng_renderer", S),
    ("update_check_enabled", B),
    ("announce_turn_signals", B),
    ("announce_speed", B),
    ("speed_announce_interval", I),
    ("drift_speech_enabled", B),
    ("gear_report_mode", S),
    ("gear_speech_sport_once", B),
    ("gear_speech_number_style", S),
    ("gear_tone_root", S),
    ("gear_tone_octave", I),
    ("gear_tone_scale", S),
    ("gear_tone_level_dbfs", F),
    ("announce_clickspot_actions", B),
    ("scanner_distance_callout_enabled", B),
    ("scanner_distance_callout_interval", I),
    ("ui_nav_hold_suppression", B),
    ("road_follow_guidance_enabled", B),
    ("road_junction_speech_enabled", B),
    ("road_stop_sign_speech_enabled", B),
    ("road_traffic_light_speech_enabled", B),
    ("road_speed_limit_speech_enabled", B),
    ("road_surface_speech_enabled", B),
    ("road_junction_proximity_enabled", B),
    ("road_junction_earcon_enabled", B),
    ("road_include_private", B),
    ("road_beep_volume_db", F),
    ("road_correction_volume_db", F),
    ("road_junction_volume_db", F),
    ("compass_click_interval", I),
    ("compass_highlight_enabled", B),
    ("compass_highlight_nth_click", I),
    ("compass_click_level_dbfs", F),
    ("lowspeed_click_level_dbfs", F),
    ("lowspeed_stop_tone_level_dbfs", F),
    ("slip_tone_level_dbfs", F),
    ("placement_ping_volume_db", F),
    ("hrtf_enabled", B),
    ("hrtf_set", S),
    ("hrtf_elevation_deg", F),
    ("hrtf_equalise", B),
    ("hrtf_front_emphasis_db", F),
    ("hrtf_distance_gain_db", F),
    ("scanner_steer_tone_enabled", B),
    ("scanner_base_freq_hz", F),
    ("scanner_pitch_offset_oct", F),
    ("pitch_roll_tones_enabled", B),
    ("pitch_roll_max_dbfs", F),
    ("pitch_roll_min_dbfs", F),
    ("shift_tone_frequency_hz", F),
    ("shift_tone_level_dbfs", F),
    ("oil_chime_enabled", B),
    ("tc_clicks_enabled", B),
    ("check_engine_buzzer_level_dbfs", F),
    ("oil_chime_level_dbfs", F),
    ("obstacle_buzz_volume_db", F),
    ("obstacle_warning_sensitivity", S),
    ("implement_tones_enabled", B),
    ("implement_proximity_speech", B),
    ("dock_tones_enabled", B),
    ("implement_ground_tone_dbfs", F),
    ("implement_tilt_tone_dbfs", F),
    ("dock_tone_dbfs", F),
    ("scan_tones_enabled", B),
    ("scan_tone_dbfs", F),
    ("cannon_shot_readout", B),
    ("binding_learn_speak_description", B),
    ("follow_default_audio_device", B),
    ("preferred_device_name", S),
    ("audio_poll_interval_sec", F),
    ("mcp_server_enabled", B),
    ("mcp_server_port", I),
];

pub const CALLOUT_INTERVALS: [i64; 7] = [5, 10, 15, 20, 30, 45, 60];
pub const SPEED_INTERVALS: [i64; 4] = [25, 50, 75, 100];

/// `v in [ints]` with Python equality (`10.0 == 10`, `True == 1`).
fn pick_listed(v: Option<&PyValue>, list: &[i64], fallback: i64) -> i64 {
    let n = match v {
        Some(PyValue::Bool(b)) => Some(*b as i64 as f64),
        Some(PyValue::Int(i)) => i.to_i64().map(|x| x as f64),
        Some(PyValue::Float(f)) => Some(*f),
        _ => None,
    };
    n.and_then(|n| list.iter().copied().find(|x| *x as f64 == n))
        .unwrap_or(fallback)
}

fn py_str_of(v: Option<&PyValue>, default: &str) -> String {
    v.map_or_else(|| default.to_string(), PyValue::py_str)
}

/// `{"values": {...}}` for `config_load` / `config_reset`: what `load_into_controls` puts
/// in each control. `Err` where the Python would raise out of it.
pub fn control_values(cfg: &PyDict) -> Result<Value, String> {
    let gear = gear_report::settings_from_config(cfg).map_err(|e| e.to_string())?;
    let mut values = Map::new();
    for (key, _kind) in PANEL_KEYS {
        let raw = cfg.get(key);
        let v = match key {
            "units" => {
                let s = py_str_of(raw, "imperial").to_lowercase();
                json!(if s.starts_with('m') { "metric" } else { "imperial" })
            }
            "telemetry_protocol" => {
                let s = py_str_of(raw, "extended").to_lowercase();
                json!(if s == "outgauge" { "outgauge" } else { "extended" })
            }
            "beamng_renderer" => {
                let s = py_str_of(raw, "d3d").to_lowercase();
                json!(if s == "vulkan" { "vulkan" } else { "d3d" })
            }
            "obstacle_warning_sensitivity" => {
                let s = py_str_of(raw, "normal").to_lowercase();
                json!(if ["early", "normal", "late"].contains(&s.as_str()) {
                    s
                } else {
                    "normal".to_string()
                })
            }
            "hrtf_set" => {
                let id = match raw {
                    Some(PyValue::Str(s)) => s.as_str(),
                    _ => "",
                };
                json!(hrtf_sets::resolve(id).set_id)
            }
            "hrtf_elevation_deg" => json!(hrtf_sets::clamp_elevation_value(
                &raw.map(to_json).unwrap_or(Value::Null)
            )),
            "hrtf_equalise" => json!(raw.map_or(hrtf_sets::DEFAULT_EQUALISE, PyValue::truthy)),
            "gear_report_mode" => json!(gear.mode),
            "gear_speech_sport_once" => json!(gear.collapse_sport),
            "gear_speech_number_style" => json!(gear.number_style),
            "gear_tone_root" => json!(gear.root),
            "gear_tone_octave" => json!(gear.octave),
            "gear_tone_scale" => json!(gear.scale),
            "gear_tone_level_dbfs" => json!(gear.level_db),
            "scanner_distance_callout_interval" => json!(pick_listed(raw, &CALLOUT_INTERVALS, 10)),
            "speed_announce_interval" => json!(pick_listed(raw, &SPEED_INTERVALS, 25)),
            "scanner_base_freq_hz" => {
                let f = raw
                    .and_then(|v| v.py_float().ok())
                    .unwrap_or(1000.0);
                json!(f.round_ties_even() as i64)
            }
            "mcp_server_port" => {
                let n = raw
                    .and_then(|v| v.py_int().ok())
                    .and_then(|n| n.to_i64())
                    .unwrap_or(4481);
                json!(n)
            }
            _ => match raw {
                Some(v) => to_json(v),
                None => config::default_for(key)
                    .map(|_| to_json(&config::default_config()[key]))
                    .unwrap_or(Value::Null),
            },
        };
        values.insert(key.to_string(), v);
    }
    Ok(json!({ "values": values }))
}

/// One control value coerced to the key's on-disk type.
fn coerce(kind: Kind, v: &Value) -> PyValue {
    match kind {
        Kind::Int => match v {
            Value::Number(n) => match n.as_i64() {
                Some(i) => PyValue::from(i),
                None => PyValue::from(n.as_f64().unwrap_or(0.0).trunc() as i64),
            },
            Value::Bool(b) => PyValue::from(*b as i64),
            other => from_json(other),
        },
        Kind::Float => match v {
            Value::Number(n) => PyValue::Float(n.as_f64().unwrap_or(0.0)),
            Value::Bool(b) => PyValue::Float(*b as i64 as f64),
            other => from_json(other),
        },
        Kind::Bool => match v {
            Value::Bool(b) => PyValue::Bool(*b),
            other => PyValue::Bool(from_json(other).truthy()),
        },
        Kind::Str => match v {
            Value::String(s) => PyValue::Str(s.clone()),
            other => from_json(other),
        },
    }
}

/// `config_save_panel`: build the config `_auto_save` writes from the snapshot and the
/// control values (steps 1-5 of the contract). `disk` is what `load_config()` returned,
/// or `None` where it raised.
pub fn build_saved_config(snapshot: &PyDict, args: &Map<String, Value>, disk: Option<&PyDict>) -> PyDict {
    let mut cfg = snapshot.clone();
    for (key, kind) in PANEL_KEYS {
        if let Some(v) = args.get(key) {
            cfg.insert(key.to_string(), coerce(kind, v));
        }
    }
    if let Some(v) = cfg.get("scanner_base_freq_hz").and_then(|v| v.py_float().ok()) {
        cfg.insert("scanner_base_freq_hz".into(), PyValue::Float(v));
    }
    if let Some(v) = cfg
        .get("scanner_pitch_offset_oct")
        .and_then(|v| v.py_float().ok())
    {
        cfg.insert(
            "scanner_pitch_offset_oct".into(),
            PyValue::Float((v * 12.0).round_ties_even() / 12.0),
        );
    }
    if let Some(disk) = disk {
        for k in ai_registry::all_config_keys() {
            if let Some(v) = disk.get(k) {
                cfg.insert(k.to_string(), v.clone());
            }
        }
        if let Some(v) = disk.get("pending_update_version") {
            cfg.insert("pending_update_version".into(), v.clone());
        }
    }
    cfg
}

/// The whole of `config_save_panel` against `path`: on success the snapshot becomes what
/// was written. Returns the command's `{ok, error}` value.
pub fn save_panel_at(path: &Path, snapshot: &mut PyDict, args: &Map<String, Value>) -> Value {
    let disk = config::load_config_configurator_at(path).ok();
    let cfg = build_saved_config(snapshot, args, disk.as_ref());
    match config::write_config(path, &cfg) {
        Ok(()) => {
            *snapshot = cfg;
            json!({"ok": true})
        }
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}
