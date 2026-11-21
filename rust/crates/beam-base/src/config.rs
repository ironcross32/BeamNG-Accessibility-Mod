//! DEFAULT_CONFIG, load_config with every coercion and clamp, atomic write, from
//! configurator.py and beamtel.py.
//!
//! The Python has two loaders that share one file and are meant to agree, but do not:
//!
//! - **beamtel.py** (the engine, [`load_config_engine`]) validates only a handful of
//!   keys (units, the road speech toggles, five volumes, obstacle sensitivity) and
//!   hands everything else through raw. Its migrations run speech, secrets, gear and
//!   write back if *any* changed. A corrupt file is renamed to `.bak`.
//! - **configurator.py** ([`load_config_configurator`]) coerces and clamps every key.
//!   Its migrations run speech, gear, secrets and write back only if a *secret* was
//!   sealed. A corrupt file is renamed to `.bak.cfgtool`.
//!
//! Their DEFAULT_CONFIG dicts hold the same values but in a different key order, and
//! beamtel's lacks `beamng_renderer`. There is one table of values here,
//! [`DEFAULT_CONFIG`] in the configurator's order, and [`ENGINE_KEY_ORDER`] giving
//! beamtel's order, so each loader writes the bytes its Python counterpart writes.
//!
//! The config is held as a [`PyDict`]; see `pyval` for why `serde_json::Value` cannot
//! carry it. [`crate::pyval::to_json_map`] converts at the edges.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::ai_registry;
use crate::gear_report;
use crate::hrtf_sets;
use crate::pyfmt::py_strip;
use crate::pyval::{PyDict, PyExc, PyInt, PyValue, json_dumps_dict_indent2, json_loads, py_clamp};
use crate::secretstore;
use crate::speech_config;

/// beamtel.py's corruption backup: `beamtel_config.json.bak`.
pub const BACKUP_SUFFIX_ENGINE: &str = ".bak";
/// configurator.py's corruption backup: `beamtel_config.json.bak.cfgtool`.
pub const BACKUP_SUFFIX_CONFIGURATOR: &str = ".bak.cfgtool";

/// A default value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConfigDefault {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(&'static str),
}

impl ConfigDefault {
    pub fn to_value(self) -> PyValue {
        match self {
            ConfigDefault::Bool(b) => PyValue::Bool(b),
            ConfigDefault::Int(i) => PyValue::Int(PyInt::Small(i)),
            ConfigDefault::Float(f) => PyValue::Float(f),
            ConfigDefault::Str(s) => PyValue::Str(s.to_string()),
        }
    }
}

use ConfigDefault::{Bool as B, Float as F, Int as I, Str as S};

/// DEFAULT_CONFIG, in configurator.py's order (the superset: beamtel.py's has every
/// key but `beamng_renderer`, with the same values).
pub const DEFAULT_CONFIG: &[(&str, ConfigDefault)] = &[
    ("speech_backend", S(speech_config::AUTO_BACKEND)),
    ("speech_voice_name", S("")),
    ("speech_rate", I(50)),
    ("speech_volume", I(100)),
    ("units", S("imperial")),
    ("shift_tone_frequency_hz", F(880.0)),
    ("shift_tone_level_dbfs", F(-12.0)),
    ("check_engine_buzzer_level_dbfs", F(-12.0)),
    ("oil_chime_level_dbfs", F(-12.0)),
    ("oil_chime_enabled", B(true)),
    ("tc_clicks_enabled", B(true)),
    ("pitch_roll_tones_enabled", B(true)),
    ("pitch_roll_max_dbfs", F(-24.0)),
    ("pitch_roll_min_dbfs", F(-36.0)),
    ("compass_click_level_dbfs", F(-6.0)),
    ("lowspeed_click_level_dbfs", F(-14.0)),
    ("lowspeed_stop_tone_level_dbfs", F(-16.0)),
    ("slip_tone_level_dbfs", F(-18.0)),
    ("placement_ping_volume_db", F(-12.0)),
    ("obstacle_buzz_volume_db", F(-18.0)),
    ("obstacle_warning_sensitivity", S("normal")),
    ("road_follow_guidance_enabled", B(true)),
    ("road_junction_speech_enabled", B(true)),
    ("road_stop_sign_speech_enabled", B(true)),
    ("road_traffic_light_speech_enabled", B(true)),
    ("road_speed_limit_speech_enabled", B(true)),
    ("road_surface_speech_enabled", B(true)),
    ("drift_speech_enabled", B(true)),
    ("road_junction_earcon_enabled", B(true)),
    ("road_junction_proximity_enabled", B(true)),
    ("road_include_private", B(false)),
    ("road_beep_volume_db", F(-14.0)),
    ("route_beacon_volume_db", F(-16.0)),
    ("road_correction_volume_db", F(-24.0)),
    ("road_junction_volume_db", F(-14.0)),
    ("telemetry_protocol", S("extended")),
    ("compass_click_interval", I(15)),
    ("compass_highlight_enabled", B(true)),
    ("compass_highlight_nth_click", I(6)),
    ("hrtf_enabled", B(true)),
    ("hrtf_set", S(hrtf_sets::DEFAULT_HRTF_SET)),
    ("hrtf_elevation_deg", F(hrtf_sets::DEFAULT_ELEVATION_DEG)),
    ("hrtf_equalise", B(hrtf_sets::DEFAULT_EQUALISE)),
    ("hrtf_front_emphasis_db", F(-6.0)),
    ("hrtf_distance_gain_db", F(0.0)),
    ("follow_default_audio_device", B(true)),
    ("preferred_device_name", S("")),
    ("audio_poll_interval_sec", F(2.0)),
    ("launch_beamng", B(false)),
    ("beamng_renderer", S("d3d")),
    // Auto-updater. pending_update_version is written by the updater behind the
    // configuration panel's back; dropping it on save would lose the one fact that
    // survives the update restart.
    ("update_check_enabled", B(true)),
    ("pending_update_version", S("")),
    ("announce_turn_signals", B(true)),
    ("announce_speed", B(true)),
    ("speed_announce_interval", I(25)),
    // Replaces the old announce_gear boolean; gear_report::migrate_config carries it over.
    ("gear_report_mode", S(gear_report::DEFAULT_MODE)),
    ("gear_speech_sport_once", B(false)),
    (
        "gear_speech_number_style",
        S(gear_report::DEFAULT_NUMBER_STYLE),
    ),
    ("gear_tone_root", S(gear_report::DEFAULT_ROOT)),
    ("gear_tone_octave", I(gear_report::DEFAULT_OCTAVE)),
    ("gear_tone_scale", S(gear_report::DEFAULT_SCALE)),
    ("gear_tone_level_dbfs", F(gear_report::DEFAULT_LEVEL_DB)),
    ("announce_clickspot_actions", B(false)),
    ("scanner_distance_callout_enabled", B(false)),
    ("scanner_distance_callout_interval", I(10)),
    ("scanner_steer_tone_enabled", B(true)),
    ("scanner_base_freq_hz", F(1000.0)),
    ("scanner_pitch_offset_oct", F(1.0)),
    ("ui_nav_hold_suppression", B(true)),
    // Loader implement (WL-40 bucket / forks). Inert on every other vehicle.
    ("implement_tones_enabled", B(true)),
    ("implement_ground_tone_dbfs", F(-19.5)),
    ("implement_tilt_tone_dbfs", F(-20.0)),
    ("implement_proximity_speech", B(true)),
    ("dock_tones_enabled", B(true)),
    ("dock_tone_dbfs", F(-18.0)),
    ("scan_tones_enabled", B(true)),
    ("cannon_shot_readout", B(true)),
    ("binding_learn_speak_description", B(false)),
    ("scan_tone_dbfs", F(-20.0)),
    ("ai_describer_provider", S(ai_registry::DEFAULT_PROVIDER)),
    // Gemini's key/model keep their original names so existing configs migrate for
    // free; every other provider is namespaced.
    ("ai_describer_api_key", S("")),
    ("ai_describer_model", S(ai_registry::GEMINI_DEFAULT_MODEL)),
    ("ai_describer_openai_api_key", S("")),
    (
        "ai_describer_openai_model",
        S(ai_registry::OPENAI_DEFAULT_MODEL),
    ),
    (
        "ai_describer_openai_base_url",
        S(ai_registry::OPENAI_DEFAULT_BASE_URL),
    ),
    ("ai_describer_openai_reasoning_effort", S("low")),
    ("ai_describer_openai_detail", S("auto")),
    ("ai_describer_disable_ui_toggle", B(false)),
    // MCP automation server. Off by default: it executes arbitrary Lua in the game on
    // behalf of a local agent, so a shipped build listens on nothing unless asked.
    ("mcp_server_enabled", B(false)),
    ("mcp_server_port", I(4481)),
];

/// beamtel.py's DEFAULT_CONFIG key order (no `beamng_renderer`).
pub const ENGINE_KEY_ORDER: &[&str] = &[
    "speech_backend",
    "speech_voice_name",
    "speech_rate",
    "speech_volume",
    "units",
    "shift_tone_frequency_hz",
    "shift_tone_level_dbfs",
    "check_engine_buzzer_level_dbfs",
    "oil_chime_level_dbfs",
    "oil_chime_enabled",
    "tc_clicks_enabled",
    "pitch_roll_tones_enabled",
    "pitch_roll_max_dbfs",
    "pitch_roll_min_dbfs",
    "compass_click_level_dbfs",
    "lowspeed_click_level_dbfs",
    "lowspeed_stop_tone_level_dbfs",
    "slip_tone_level_dbfs",
    "telemetry_protocol",
    "compass_click_interval",
    "compass_highlight_enabled",
    "compass_highlight_nth_click",
    "hrtf_enabled",
    "hrtf_set",
    "hrtf_elevation_deg",
    "hrtf_equalise",
    "hrtf_front_emphasis_db",
    "hrtf_distance_gain_db",
    "follow_default_audio_device",
    "preferred_device_name",
    "audio_poll_interval_sec",
    "obstacle_buzz_volume_db",
    "obstacle_warning_sensitivity",
    "road_beep_volume_db",
    "route_beacon_volume_db",
    "road_correction_volume_db",
    "road_follow_guidance_enabled",
    "road_junction_speech_enabled",
    "road_stop_sign_speech_enabled",
    "road_traffic_light_speech_enabled",
    "road_speed_limit_speech_enabled",
    "road_surface_speech_enabled",
    "drift_speech_enabled",
    "road_junction_earcon_enabled",
    "road_junction_proximity_enabled",
    "road_include_private",
    "road_junction_volume_db",
    "placement_ping_volume_db",
    "launch_beamng",
    "update_check_enabled",
    "pending_update_version",
    "announce_turn_signals",
    "announce_speed",
    "speed_announce_interval",
    "gear_report_mode",
    "gear_speech_sport_once",
    "gear_speech_number_style",
    "gear_tone_root",
    "gear_tone_octave",
    "gear_tone_scale",
    "gear_tone_level_dbfs",
    "announce_clickspot_actions",
    "scanner_distance_callout_enabled",
    "scanner_distance_callout_interval",
    "scanner_steer_tone_enabled",
    "scanner_base_freq_hz",
    "scanner_pitch_offset_oct",
    "ui_nav_hold_suppression",
    "implement_tones_enabled",
    "implement_ground_tone_dbfs",
    "implement_tilt_tone_dbfs",
    "implement_proximity_speech",
    "dock_tones_enabled",
    "dock_tone_dbfs",
    "scan_tones_enabled",
    "scan_tone_dbfs",
    "cannon_shot_readout",
    "binding_learn_speak_description",
    "ai_describer_provider",
    "ai_describer_api_key",
    "ai_describer_model",
    "ai_describer_openai_api_key",
    "ai_describer_openai_model",
    "ai_describer_openai_base_url",
    "ai_describer_openai_reasoning_effort",
    "ai_describer_openai_detail",
    "ai_describer_disable_ui_toggle",
    "mcp_server_enabled",
    "mcp_server_port",
];

/// The default for one key, if it has one.
pub fn default_for(key: &str) -> Option<ConfigDefault> {
    DEFAULT_CONFIG
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, d)| *d)
}

/// configurator.py's `DEFAULT_CONFIG.copy()`.
pub fn default_config() -> PyDict {
    DEFAULT_CONFIG
        .iter()
        .map(|(k, d)| (k.to_string(), d.to_value()))
        .collect()
}

/// beamtel.py's `DEFAULT_CONFIG.copy()`.
pub fn default_config_engine() -> PyDict {
    ENGINE_KEY_ORDER
        .iter()
        .filter_map(|k| default_for(k).map(|d| (k.to_string(), d.to_value())))
        .collect()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Windows errors Python maps to `PermissionError` (EACCES), the ones `_write_config`
/// retries on: access denied, write protect, sharing and lock violations, and the
/// rest of CPython's EACCES table.
fn is_permission_error(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied
        || matches!(
            e.raw_os_error(),
            Some(5 | 19 | 32 | 33 | 65 | 82 | 83 | 108 | 132 | 158 | 167)
        )
}

/// The exact bytes `json.dump(cfg, f, indent=2, ensure_ascii=False)` leaves on disk
/// through a text-mode file on Windows (`\n` becomes CRLF, no trailing newline).
pub fn config_file_bytes(cfg: &PyDict) -> Vec<u8> {
    json_dumps_dict_indent2(cfg)
        .replace('\n', "\r\n")
        .into_bytes()
}

/// `_write_config`: write the config atomically.
///
/// beamtel and the configurator share this file and beamtel polls it once a second to
/// hot-reload. Truncating in place would let a reader land on a half-written file,
/// fail to parse it and take the corruption branch -- which renames the real config to
/// a backup and replaces it with defaults. Writing a temp file (`<path>.<pid>.tmp`)
/// and renaming means a reader always sees the whole old file or the whole new one.
/// Windows refuses a rename onto a file another process has open, so a
/// `PermissionError` is retried with backoff (10 attempts, 20 ms doubling to 250 ms)
/// rather than dropping the user's settings. The temp file is always cleaned up.
pub fn write_config(path: &Path, cfg: &PyDict) -> io::Result<()> {
    let directory = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let _ = fs::create_dir_all(&directory);
    let tmp = with_suffix(path, &format!(".{}.tmp", std::process::id()));
    let result = (|| -> io::Result<()> {
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&config_file_bytes(cfg))?;
            f.flush()?;
            f.sync_all()?;
        }
        let mut delay = 0.02f64;
        for attempt in 0..10 {
            match fs::rename(&tmp, path) {
                Ok(()) => return Ok(()),
                Err(e) if is_permission_error(&e) => {
                    if attempt == 9 {
                        return Err(e);
                    }
                    std::thread::sleep(Duration::from_secs_f64(delay));
                    delay = (delay * 2.0).min(0.25);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    })();
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// What `_read_config_raw` produced.
#[derive(Debug)]
pub enum RawRead {
    /// `(parsed, None)`.
    Parsed(PyValue),
    /// The file was read but is not UTF-8 JSON: Python raises out of `_read_config_raw`
    /// and the caller treats it as corruption.
    Invalid(String),
    /// `(None, os_error)` after five attempts: transient, NOT corruption.
    OsError(io::Error),
}

/// `_read_config_raw`: read and parse, retrying briefly on OS errors.
///
/// A sharing violation, or a read arriving mid-rename, is not corruption. The caller
/// must keep the two apart: the corruption path destroys user settings, so it must
/// never fire just because the file was momentarily unavailable.
pub fn read_config_raw(path: &Path) -> RawRead {
    let mut last = None;
    for _attempt in 0..5 {
        match fs::read(path) {
            Ok(bytes) => {
                let text = match String::from_utf8(bytes) {
                    Ok(t) => t,
                    Err(e) => return RawRead::Invalid(format!("UnicodeDecodeError: {e}")),
                };
                return match json_loads(&text) {
                    Ok(v) => RawRead::Parsed(v),
                    Err(e) => RawRead::Invalid(e.0),
                };
            }
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    RawRead::OsError(last.unwrap_or_else(|| io::Error::other("unreadable")))
}

/// Anything the Python loaders' `except Exception` would catch: takes the corruption
/// path.
#[derive(Debug)]
struct Corrupt(#[allow(dead_code)] String);

impl From<PyExc> for Corrupt {
    fn from(e: PyExc) -> Self {
        Corrupt(e.to_string())
    }
}

impl From<io::Error> for Corrupt {
    fn from(e: io::Error) -> Self {
        Corrupt(e.to_string())
    }
}

/// `merged = DEFAULT_CONFIG.copy(); merged.update(user)`.
fn merge(defaults: PyDict, user: &PyDict) -> PyDict {
    let mut merged = defaults;
    for (k, v) in user {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

fn get_str(merged: &PyDict, key: &str) -> String {
    merged.get(key).map(PyValue::py_str).unwrap_or_default()
}

/// Write a migrated config back. Departure (python-bugs.md 2): the Python lets a failed
/// write (the file still locked after every retry) reach the corruption path, which
/// renamed a perfectly good config to the backup and replaced it with defaults. The
/// file on disk is intact and the migration is still applied in memory, so log it and
/// carry on; the next load migrates again and retries the write.
fn write_back_migration(path: &Path, user: &PyDict) {
    if let Err(e) = write_config(path, user) {
        log::warn!("Could not write the migrated config back ({e}); will retry on the next load.");
    }
}

/// Departure (python-bugs.md 1): `gear_report.settings_from_config` raises
/// OverflowError on an infinite octave or a level too large for a float, and the Python
/// loaders sent that to the corruption path. Here the one key falls back to its default,
/// as any other unusable value does.
fn sanitise_gear_overflow(merged: &mut PyDict) {
    if matches!(
        merged.get("gear_tone_level_dbfs").map(PyValue::py_float),
        Some(Err(PyExc::OverflowError))
    ) {
        merged.insert(
            "gear_tone_level_dbfs".into(),
            PyValue::Float(gear_report::DEFAULT_LEVEL_DB),
        );
    }
    if matches!(
        merged.get("gear_tone_octave").map(PyValue::py_int),
        Some(Err(PyExc::OverflowError))
    ) {
        merged.insert(
            "gear_tone_octave".into(),
            PyValue::Int(PyInt::Small(gear_report::DEFAULT_OCTAVE)),
        );
    }
}

/// The corruption path shared by both loaders.
fn recover(path: &Path, suffix: &str, defaults: PyDict) -> io::Result<PyDict> {
    let _ = fs::rename(path, with_suffix(path, suffix));
    write_config(path, &defaults)?;
    Ok(defaults)
}

// ---------------------------------------------------------------------------
// beamtel.py
// ---------------------------------------------------------------------------

/// beamtel.py's `load_config()`, on the real config path.
pub fn load_config_engine() -> io::Result<PyDict> {
    load_config_engine_at(&crate::paths::config_path())
}

/// beamtel.py's `load_config()` on `path`.
///
/// `Err` only where the Python raises out of `load_config`: writing the defaults
/// failed (first run, or after a corrupt file).
pub fn load_config_engine_at(path: &Path) -> io::Result<PyDict> {
    if !path.is_file() {
        write_config(path, &default_config_engine())?;
        return Ok(default_config_engine());
    }
    match engine_body(path) {
        Ok(cfg) => Ok(cfg),
        Err(_) => recover(path, BACKUP_SUFFIX_ENGINE, default_config_engine()),
    }
}

const ENGINE_ROAD_BOOLS: [(&str, bool); 10] = [
    ("road_follow_guidance_enabled", true),
    ("road_junction_speech_enabled", true),
    ("road_stop_sign_speech_enabled", true),
    ("road_traffic_light_speech_enabled", true),
    ("road_speed_limit_speech_enabled", true),
    ("road_surface_speech_enabled", true),
    ("drift_speech_enabled", true),
    ("road_junction_earcon_enabled", true),
    ("road_junction_proximity_enabled", true),
    ("road_include_private", false),
];

const ENGINE_VOLUMES: [(&str, f64); 5] = [
    ("route_beacon_volume_db", -16.0),
    ("road_beep_volume_db", -14.0),
    ("road_correction_volume_db", -24.0),
    ("road_junction_volume_db", -14.0),
    ("obstacle_buzz_volume_db", -18.0),
];

fn engine_body(path: &Path) -> Result<PyDict, Corrupt> {
    let user = match read_config_raw(path) {
        RawRead::OsError(e) => {
            // Unreadable right now, most likely because the configurator is mid-save.
            // Defaults for this call only; the file is almost certainly intact.
            log::warn!(
                "Config temporarily unreadable ({e}); using defaults for this load without rewriting it."
            );
            return Ok(default_config_engine());
        }
        RawRead::Invalid(msg) => return Err(Corrupt(msg)),
        RawRead::Parsed(v) => v,
    };
    let PyValue::Dict(mut user) = user else {
        return Err(Corrupt("Config root is not an object".into()));
    };
    let mut migrated = false;
    if speech_config::migrate_config(&mut user) {
        log::info!("Migrated legacy SAPI speech keys to speech_* keys.");
        migrated = true;
    }
    // An API key written before at-rest protection existed is still in plaintext;
    // seal it on the first run that sees it.
    if secretstore::migrate_config(&mut user) {
        log::info!("Encrypted plaintext config secrets with DPAPI.");
        migrated = true;
    }
    if gear_report::migrate_config(&mut user) {
        log::info!("Migrated announce_gear to gear_report_mode.");
        migrated = true;
    }
    if migrated {
        write_back_migration(path, &user);
    }
    let mut merged = merge(default_config_engine(), &user);
    // Departure (python-bugs.md 1): the engine hands the gear keys through raw, and an
    // overflowing one would make every later `settings_from_config` raise.
    sanitise_gear_overflow(&mut merged);
    let units = get_str(&merged, "units").to_lowercase();
    merged.insert(
        "units".into(),
        PyValue::from(if units.starts_with('m') {
            "metric"
        } else {
            "imperial"
        }),
    );
    for (key, fallback) in ENGINE_ROAD_BOOLS {
        let value = match merged.get(key) {
            Some(PyValue::Bool(b)) => *b,
            _ => fallback,
        };
        merged.insert(key.into(), PyValue::Bool(value));
    }
    for (key, fallback) in ENGINE_VOLUMES {
        let value = match merged.get(key).map(PyValue::py_float) {
            None => py_clamp(fallback, -120.0, 0.0),
            Some(Ok(x)) => py_clamp(x, -120.0, 0.0),
            // Python lets OverflowError through to the corruption path (bug 1).
            Some(Err(_)) => fallback,
        };
        merged.insert(key.into(), PyValue::Float(value));
    }
    let sensitivity = get_str(&merged, "obstacle_warning_sensitivity").to_lowercase();
    let sensitivity = if ["early", "normal", "late"].contains(&sensitivity.as_str()) {
        sensitivity
    } else {
        "normal".to_string()
    };
    merged.insert(
        "obstacle_warning_sensitivity".into(),
        PyValue::Str(sensitivity),
    );
    Ok(merged)
}

// ---------------------------------------------------------------------------
// configurator.py
// ---------------------------------------------------------------------------

/// configurator.py's `load_config()`, on the real config path.
pub fn load_config_configurator() -> io::Result<PyDict> {
    load_config_configurator_at(&crate::paths::config_path())
}

/// configurator.py's `load_config()` on `path`. `Err` as for [`load_config_engine_at`].
pub fn load_config_configurator_at(path: &Path) -> io::Result<PyDict> {
    if !path.is_file() {
        write_config(path, &default_config())?;
        return Ok(default_config());
    }
    match configurator_body(path) {
        Ok(cfg) => Ok(cfg),
        Err(_) => recover(path, BACKUP_SUFFIX_CONFIGURATOR, default_config()),
    }
}

#[derive(Clone, Copy)]
enum Cast {
    Str,
    Int,
    Float,
    Bool,
}

/// `_coerce(key, caster, fallback)`: a value the cast rejects falls back. Departure
/// (python-bugs.md 1): the Python caught only ValueError and TypeError, so an
/// OverflowError (`1e400`, a huge int in a float key) sent the whole file down the
/// corruption path; here it falls back like the rest.
fn coerce(merged: &mut PyDict, key: &str, cast: Cast, fallback: ConfigDefault) {
    let current = merged
        .get(key)
        .cloned()
        .unwrap_or_else(|| fallback.to_value());
    let result = match cast {
        Cast::Str => Ok(PyValue::Str(current.py_str())),
        Cast::Bool => Ok(PyValue::Bool(current.truthy())),
        Cast::Int => current.py_int().map(PyValue::Int),
        Cast::Float => current.py_float().map(PyValue::Float),
    };
    let value = result.unwrap_or_else(|_| fallback.to_value());
    merged.insert(key.to_string(), value);
}

/// The `_coerce` calls in configurator.py's order (gear keys are handled separately).
const COERCIONS_BEFORE_GEAR: &[(&str, Cast, ConfigDefault)] = &[
    ("speech_backend", Cast::Str, S("auto")),
    ("speech_voice_name", Cast::Str, S("")),
    ("speech_rate", Cast::Int, I(50)),
    ("speech_volume", Cast::Int, I(100)),
    ("shift_tone_frequency_hz", Cast::Float, F(880.0)),
    ("shift_tone_level_dbfs", Cast::Float, F(-12.0)),
    ("check_engine_buzzer_level_dbfs", Cast::Float, F(-12.0)),
    ("oil_chime_level_dbfs", Cast::Float, F(-12.0)),
    ("oil_chime_enabled", Cast::Bool, B(true)),
    ("tc_clicks_enabled", Cast::Bool, B(true)),
    ("pitch_roll_tones_enabled", Cast::Bool, B(true)),
    ("pitch_roll_max_dbfs", Cast::Float, F(-24.0)),
    ("pitch_roll_min_dbfs", Cast::Float, F(-36.0)),
    ("compass_click_level_dbfs", Cast::Float, F(-6.0)),
    ("lowspeed_click_level_dbfs", Cast::Float, F(-14.0)),
    ("lowspeed_stop_tone_level_dbfs", Cast::Float, F(-16.0)),
    ("slip_tone_level_dbfs", Cast::Float, F(-18.0)),
    ("placement_ping_volume_db", Cast::Float, F(-12.0)),
    ("obstacle_buzz_volume_db", Cast::Float, F(-18.0)),
    ("obstacle_warning_sensitivity", Cast::Str, S("normal")),
    ("road_follow_guidance_enabled", Cast::Bool, B(true)),
    ("road_junction_speech_enabled", Cast::Bool, B(true)),
    ("road_stop_sign_speech_enabled", Cast::Bool, B(true)),
    ("road_traffic_light_speech_enabled", Cast::Bool, B(true)),
    ("road_speed_limit_speech_enabled", Cast::Bool, B(true)),
    ("road_surface_speech_enabled", Cast::Bool, B(true)),
    ("drift_speech_enabled", Cast::Bool, B(true)),
    ("road_junction_earcon_enabled", Cast::Bool, B(true)),
    ("road_junction_proximity_enabled", Cast::Bool, B(true)),
    ("road_include_private", Cast::Bool, B(false)),
    ("road_beep_volume_db", Cast::Float, F(-14.0)),
    ("route_beacon_volume_db", Cast::Float, F(-16.0)),
    ("road_correction_volume_db", Cast::Float, F(-24.0)),
    ("road_junction_volume_db", Cast::Float, F(-14.0)),
    ("compass_click_interval", Cast::Int, I(15)),
    // The fallback is False although the default is True; bool() never raises, so it
    // is never used.
    ("compass_highlight_enabled", Cast::Bool, B(false)),
    ("compass_highlight_nth_click", Cast::Int, I(6)),
    ("hrtf_enabled", Cast::Bool, B(true)),
    ("hrtf_set", Cast::Str, S(hrtf_sets::DEFAULT_HRTF_SET)),
    (
        "hrtf_elevation_deg",
        Cast::Float,
        F(hrtf_sets::DEFAULT_ELEVATION_DEG),
    ),
    ("hrtf_equalise", Cast::Bool, B(hrtf_sets::DEFAULT_EQUALISE)),
    ("hrtf_front_emphasis_db", Cast::Float, F(-6.0)),
    ("hrtf_distance_gain_db", Cast::Float, F(0.0)),
    ("launch_beamng", Cast::Bool, B(false)),
    ("update_check_enabled", Cast::Bool, B(true)),
    ("pending_update_version", Cast::Str, S("")),
    ("beamng_renderer", Cast::Str, S("d3d")),
    ("follow_default_audio_device", Cast::Bool, B(true)),
    ("announce_turn_signals", Cast::Bool, B(true)),
    ("announce_speed", Cast::Bool, B(true)),
    ("speed_announce_interval", Cast::Int, I(25)),
];

const COERCIONS_AFTER_GEAR: &[(&str, Cast, ConfigDefault)] = &[
    ("announce_clickspot_actions", Cast::Bool, B(false)),
    ("scanner_distance_callout_enabled", Cast::Bool, B(false)),
    ("scanner_distance_callout_interval", Cast::Int, I(10)),
    ("scanner_steer_tone_enabled", Cast::Bool, B(true)),
    ("scanner_base_freq_hz", Cast::Float, F(1000.0)),
    ("scanner_pitch_offset_oct", Cast::Float, F(1.0)),
    ("ui_nav_hold_suppression", Cast::Bool, B(true)),
    ("implement_tones_enabled", Cast::Bool, B(true)),
    ("implement_ground_tone_dbfs", Cast::Float, F(-19.5)),
    ("implement_tilt_tone_dbfs", Cast::Float, F(-20.0)),
    ("implement_proximity_speech", Cast::Bool, B(true)),
    ("dock_tones_enabled", Cast::Bool, B(true)),
    ("dock_tone_dbfs", Cast::Float, F(-18.0)),
    ("scan_tones_enabled", Cast::Bool, B(true)),
    ("cannon_shot_readout", Cast::Bool, B(true)),
    ("binding_learn_speak_description", Cast::Bool, B(false)),
    ("scan_tone_dbfs", Cast::Float, F(-20.0)),
    ("preferred_device_name", Cast::Str, S("")),
    ("audio_poll_interval_sec", Cast::Float, F(2.0)),
    ("ai_describer_provider", Cast::Str, S("gemini")),
    ("ai_describer_api_key", Cast::Str, S("")),
    (
        "ai_describer_model",
        Cast::Str,
        S("models/gemini-3-flash-preview"),
    ),
    ("ai_describer_openai_api_key", Cast::Str, S("")),
    ("ai_describer_openai_model", Cast::Str, S("gpt-5.6-terra")),
    (
        "ai_describer_openai_base_url",
        Cast::Str,
        S("https://api.openai.com/v1"),
    ),
    ("ai_describer_openai_reasoning_effort", Cast::Str, S("low")),
    ("ai_describer_openai_detail", Cast::Str, S("auto")),
    ("ai_describer_disable_ui_toggle", Cast::Bool, B(false)),
    ("mcp_server_enabled", Cast::Bool, B(false)),
    ("mcp_server_port", Cast::Int, I(4481)),
];

/// The dB settings clamped to [-120, 0].
const DB_KEYS: [&str; 20] = [
    "shift_tone_level_dbfs",
    "check_engine_buzzer_level_dbfs",
    "oil_chime_level_dbfs",
    "pitch_roll_max_dbfs",
    "pitch_roll_min_dbfs",
    "compass_click_level_dbfs",
    "lowspeed_click_level_dbfs",
    "lowspeed_stop_tone_level_dbfs",
    "slip_tone_level_dbfs",
    "placement_ping_volume_db",
    "obstacle_buzz_volume_db",
    "road_beep_volume_db",
    "road_correction_volume_db",
    "road_junction_volume_db",
    "route_beacon_volume_db",
    "hrtf_front_emphasis_db",
    "implement_ground_tone_dbfs",
    "implement_tilt_tone_dbfs",
    "dock_tone_dbfs",
    "scan_tone_dbfs",
];

/// An int-coerced key (always an `Int` once `coerce` has run).
fn int_at(merged: &PyDict, key: &str) -> PyInt {
    match merged.get(key) {
        Some(PyValue::Int(i)) => i.clone(),
        _ => PyInt::Small(0),
    }
}

/// A float-coerced key (always a `Float` once `coerce` has run).
fn float_at(merged: &PyDict, key: &str) -> f64 {
    match merged.get(key) {
        Some(PyValue::Float(f)) => *f,
        Some(other) => other.py_float().unwrap_or(f64::NAN),
        None => f64::NAN,
    }
}

fn set_int(merged: &mut PyDict, key: &str, v: i64) {
    merged.insert(key.to_string(), PyValue::Int(PyInt::Small(v)));
}

fn set_float(merged: &mut PyDict, key: &str, v: f64) {
    merged.insert(key.to_string(), PyValue::Float(v));
}

/// Every provider's model and extras, not just the active one's -- the inactive
/// provider's settings are still on disk and are used the moment the user switches.
/// Runs after the `str` coercions, so every value it reads is a string.
fn validate_ai(merged: &mut PyDict) {
    let provider = get_str(merged, ai_registry::PROVIDER_CFG_KEY);
    if !ai_registry::PROVIDERS.iter().any(|(id, _)| *id == provider) {
        merged.insert(
            "ai_describer_provider".into(),
            PyValue::from(ai_registry::DEFAULT_PROVIDER),
        );
    }
    for (pid, _disp) in ai_registry::PROVIDERS {
        let (_key_cfg, model_cfg) = ai_registry::config_keys_for(pid);
        let model_ok = matches!(merged.get(model_cfg), Some(PyValue::Str(m))
            if ai_registry::model_names_for(pid).contains(&m.as_str()));
        if !model_ok {
            merged.insert(
                model_cfg.into(),
                PyValue::from(ai_registry::default_model_for(pid)),
            );
        }
        for ex in ai_registry::extras_for(pid) {
            let ok = match (ex.kind, merged.get(ex.key)) {
                (ai_registry::ExtraKind::Choice, Some(PyValue::Str(v))) => {
                    ex.values.iter().any(|(c, _)| c == v)
                }
                (ai_registry::ExtraKind::Choice, _) => false,
                (ai_registry::ExtraKind::Text, Some(PyValue::Str(v))) => !py_strip(v).is_empty(),
                (ai_registry::ExtraKind::Text, _) => false,
            };
            if !ok {
                merged.insert(ex.key.into(), PyValue::from(ex.default));
            }
        }
    }
}

fn configurator_body(path: &Path) -> Result<PyDict, Corrupt> {
    let user = match read_config_raw(path) {
        // Unreadable right now, most likely because beamtel is mid-save. Defaults
        // for this call only; do not rewrite the file, it is almost certainly intact.
        RawRead::OsError(_) => return Ok(default_config()),
        RawRead::Invalid(msg) => return Err(Corrupt(msg)),
        RawRead::Parsed(v) => v,
    };
    let PyValue::Dict(mut user) = user else {
        return Err(Corrupt("Config root is not an object".into()));
    };
    speech_config::migrate_config(&mut user);
    gear_report::migrate_config(&mut user);
    // Seal any plaintext secret the moment it is read, and write it back here rather
    // than waiting for the next edit -- a config that is only ever opened and closed
    // would otherwise stay in the clear forever.
    if secretstore::migrate_config(&mut user) {
        write_back_migration(path, &user);
    }

    let mut merged = merge(default_config(), &user);
    for &(key, cast, fallback) in COERCIONS_BEFORE_GEAR {
        coerce(&mut merged, key, cast, fallback);
    }
    // Every gear key, validated against the lists the dropdowns are built from.
    sanitise_gear_overflow(&mut merged);
    let gear = gear_report::settings_from_config(&merged)?;
    merged.insert("gear_report_mode".into(), PyValue::Str(gear.mode));
    merged.insert(
        "gear_speech_sport_once".into(),
        PyValue::Bool(gear.collapse_sport),
    );
    merged.insert(
        "gear_speech_number_style".into(),
        PyValue::Str(gear.number_style),
    );
    merged.insert("gear_tone_root".into(), PyValue::Str(gear.root));
    set_int(&mut merged, "gear_tone_octave", gear.octave);
    merged.insert("gear_tone_scale".into(), PyValue::Str(gear.scale));
    set_float(&mut merged, "gear_tone_level_dbfs", gear.level_db);
    for &(key, cast, fallback) in COERCIONS_AFTER_GEAR {
        coerce(&mut merged, key, cast, fallback);
    }

    validate_ai(&mut merged);

    let v = int_at(&merged, "speech_rate").clamp_py(0, 100);
    set_int(&mut merged, "speech_rate", v);
    let v = int_at(&merged, "speech_volume").clamp_py(0, 100);
    set_int(&mut merged, "speech_volume", v);
    let v = py_clamp(float_at(&merged, "shift_tone_frequency_hz"), 20.0, 20000.0);
    set_float(&mut merged, "shift_tone_frequency_hz", v);

    let v = int_at(&merged, "compass_click_interval").clamp_py(1, 90);
    set_int(&mut merged, "compass_click_interval", v);
    let v = int_at(&merged, "compass_highlight_nth_click").clamp_py(2, 100);
    set_int(&mut merged, "compass_highlight_nth_click", v);
    let v = py_clamp(float_at(&merged, "audio_poll_interval_sec"), 0.1, 10.0);
    set_float(&mut merged, "audio_poll_interval_sec", v);
    let interval = int_at(&merged, "speed_announce_interval");
    if ![25, 50, 75, 100]
        .iter()
        .any(|n| interval == PyInt::Small(*n))
    {
        set_int(&mut merged, "speed_announce_interval", 25);
    }
    let interval = int_at(&merged, "scanner_distance_callout_interval");
    if ![5, 10, 15, 20, 30, 45, 60]
        .iter()
        .any(|n| interval == PyInt::Small(*n))
    {
        set_int(&mut merged, "scanner_distance_callout_interval", 10);
    }
    let v = py_clamp(float_at(&merged, "scanner_base_freq_hz"), 100.0, 8000.0);
    set_float(&mut merged, "scanner_base_freq_hz", v);
    // Offset is in octaves, quantised to whole semitones (1/12 oct), clamped to
    // [0.5, 2.0]. Python's round() raises on NaN and infinity, which took the
    // corruption path; departure (python-bugs.md 1): the key falls back to its default.
    let mut scaled = float_at(&merged, "scanner_pitch_offset_oct") * 12.0;
    if !scaled.is_finite() {
        scaled = 12.0;
    }
    let semis = scaled.round_ties_even();
    let semis = if semis < 24.0 { semis } else { 24.0 };
    let semis = if semis > 6.0 { semis } else { 6.0 };
    set_float(&mut merged, "scanner_pitch_offset_oct", semis / 12.0);
    let sensitivity = get_str(&merged, "obstacle_warning_sensitivity").to_lowercase();
    let sensitivity = if ["early", "normal", "late"].contains(&sensitivity.as_str()) {
        sensitivity
    } else {
        "normal".to_string()
    };
    merged.insert(
        "obstacle_warning_sensitivity".into(),
        PyValue::Str(sensitivity),
    );

    for key in DB_KEYS {
        let v = py_clamp(float_at(&merged, key), -120.0, 0.0);
        set_float(&mut merged, key, v);
    }
    let v = py_clamp(float_at(&merged, "hrtf_distance_gain_db"), -24.0, 6.0);
    set_float(&mut merged, "hrtf_distance_gain_db", v);
    let set_id = hrtf_sets::resolve(&get_str(&merged, "hrtf_set")).set_id;
    merged.insert("hrtf_set".into(), PyValue::from(set_id));
    let v = hrtf_sets::clamp_elevation(float_at(&merged, "hrtf_elevation_deg"));
    set_float(&mut merged, "hrtf_elevation_deg", v);

    let units = get_str(&merged, "units").to_lowercase();
    merged.insert(
        "units".into(),
        PyValue::from(if units.starts_with('m') {
            "metric"
        } else {
            "imperial"
        }),
    );
    let proto = get_str(&merged, "telemetry_protocol").to_lowercase();
    merged.insert(
        "telemetry_protocol".into(),
        PyValue::from(if proto == "outgauge" {
            "outgauge"
        } else {
            "extended"
        }),
    );
    let renderer = get_str(&merged, "beamng_renderer").to_lowercase();
    merged.insert(
        "beamng_renderer".into(),
        PyValue::from(if renderer == "vulkan" {
            "vulkan"
        } else {
            "d3d"
        }),
    );
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_order_is_the_table_without_the_renderer() {
        let mut engine: Vec<&str> = ENGINE_KEY_ORDER.to_vec();
        engine.sort_unstable();
        let mut table: Vec<&str> = DEFAULT_CONFIG
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| *k != "beamng_renderer")
            .collect();
        table.sort_unstable();
        assert_eq!(engine, table);
        assert_eq!(default_config_engine().len(), DEFAULT_CONFIG.len() - 1);
    }
}
