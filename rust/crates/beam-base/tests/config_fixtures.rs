//! Golden cases from `rust/fixtures/gen/config.py`: both Python `load_config`s against a
//! table of config files, compared on the returned dict and every byte left on disk.
//! Each case runs in its own temporary directory; the real config is never touched.

use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use base64::Engine as _;
use beam_base::config::{
    self, default_config, default_config_engine, load_config_configurator_at, load_config_engine_at,
};
use beam_base::pyval::{PyDict, json_dumps_indent2};
use beam_base::secretstore;
use beam_testutil::load;
use serde_json::{Map, Value, json};

/// `dpapi:v1:<b64>` -> `<sealed:PLAINTEXT>` when it decrypts, as the generator does.
fn norm(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(secretstore::PREFIX) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + secretstore::PREFIX.len()..];
        let len = after
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
            .count();
        if len == 0 {
            out.push_str(secretstore::PREFIX);
            rest = after;
            continue;
        }
        let blob = &rest[pos..pos + secretstore::PREFIX.len() + len];
        match secretstore::unprotect(blob) {
            Some(plain) => out.push_str(&format!("<sealed:{plain}>")),
            None => out.push_str(blob),
        }
        rest = &after[len..];
    }
    out.push_str(rest);
    out
}

fn encode_result(res: &PyDict, default: &PyDict) -> Value {
    let mut diff = Map::new();
    for (k, v) in res {
        let t = norm(&json_dumps_indent2(v));
        if default.get(k).map(json_dumps_indent2) != Some(t.clone()) {
            diff.insert(k.clone(), Value::String(t));
        }
    }
    let missing: Vec<&String> = default.keys().filter(|k| !res.contains_key(*k)).collect();
    let mut out = json!({"diff": diff, "missing": missing});
    let expected: Vec<&String> = default
        .keys()
        .filter(|k| res.contains_key(*k))
        .chain(res.keys().filter(|k| !default.contains_key(*k)))
        .collect();
    let actual: Vec<&String> = res.keys().collect();
    if actual != expected {
        out["order"] = json!(actual);
    }
    out
}

fn encode_file(data: &[u8], input: Option<&[u8]>, default_bytes: &[u8]) -> Value {
    if input == Some(data) {
        return json!("<input>");
    }
    if data == default_bytes {
        return json!("<defaults>");
    }
    match std::str::from_utf8(data) {
        Ok(t) => json!(norm(t)),
        Err(_) => json!({"b64": base64::engine::general_purpose::STANDARD.encode(data)}),
    }
}

fn run(loader: &str, input: Option<&[u8]>, locked: bool) -> Value {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("beamtel_config.json");
    if let Some(bytes) = input {
        fs::write(&path, bytes).expect("write input");
    }
    let lock = locked.then(|| {
        fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("lock")
    });
    let (result, default) = match loader {
        "engine" => (load_config_engine_at(&path), default_config_engine()),
        _ => (load_config_configurator_at(&path), default_config()),
    };
    drop(lock);
    let default_bytes = config::config_file_bytes(&default);
    let res = match result {
        Ok(cfg) => encode_result(&cfg, &default),
        Err(_) => json!({"err": "OSError"}),
    };
    let mut files = Map::new();
    let mut names: Vec<String> = fs::read_dir(dir.path())
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        let data = fs::read(Path::new(dir.path()).join(&name)).expect("read");
        files.insert(name, encode_file(&data, input, &default_bytes));
    }
    json!({"result": res, "files": files})
}

#[test]
fn default_file_bytes_match_python() {
    let fx = load("config");
    let want = &fx["default_file_bytes"];
    assert_eq!(
        String::from_utf8(config::config_file_bytes(&default_config_engine())).unwrap(),
        want["engine"].as_str().unwrap()
    );
    assert_eq!(
        String::from_utf8(config::config_file_bytes(&default_config())).unwrap(),
        want["configurator"].as_str().unwrap()
    );
}

#[test]
fn load_config_matches_python() {
    let fx = load("config");
    let cases = fx["cases"].as_array().unwrap();
    let mut failures = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let input: Option<Vec<u8>> = if let Some(b) = case.get("input_b64") {
            Some(
                base64::engine::general_purpose::STANDARD
                    .decode(b.as_str().unwrap())
                    .unwrap(),
            )
        } else {
            case["input"].as_str().map(|s| s.as_bytes().to_vec())
        };
        if name == "python-sealed secret" && !python_blob_unseals(&fx) {
            // DPAPI is per Windows user: the blob in the fixture only opens for the
            // account that generated it.
            eprintln!("skipping {name:?}: fixture sealed by another Windows account");
            continue;
        }
        let locked = case["locked"].as_bool().unwrap();
        for loader in ["engine", "configurator"] {
            let got = run(loader, input.as_deref(), locked);
            if is_overflow_departure(name, loader) {
                if let Err(why) = check_overflow_departure(name, &got) {
                    failures.push(format!("{name} [{loader}] (departure): {why}\n  got:  {got}"));
                }
                continue;
            }
            if got != case[loader] {
                failures.push(format!(
                    "{name} [{loader}]\n  got:  {got}\n  want: {}",
                    case[loader]
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} loads differ:\n{}",
        failures.len(),
        cases.len() * 2,
        failures
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Cases where the Rust departs from the fixture on purpose (python-bugs.md 1): the
/// Python sent an overflowing or non-finite value down the corruption path, renaming the
/// user's config to the backup and writing defaults. The Rust falls back for that one
/// key. The engine half of `gear_tone_octave = inf` is here too: the Python engine
/// handed the inf through raw, and every later `settings_from_config` raised on it.
fn is_overflow_departure(name: &str, loader: &str) -> bool {
    const BOTH: &[&str] = &["gear_tone_octave = variant 27 (inf)"];
    const CONFIGURATOR: &[&str] = &[
        "overflowing float literal",
        "all keys = variant 36 ('inf')",
        "all keys = variant 37 ('nan')",
        "all keys = variant 38 ('-inf')",
        "all keys = variant 39 ('1e400')",
        "speech_rate = variant 27 (inf)",
        "speech_volume = variant 27 (inf)",
        "compass_click_interval = variant 27 (inf)",
        "compass_highlight_nth_click = variant 27 (inf)",
        "speed_announce_interval = variant 27 (inf)",
        "scanner_distance_callout_interval = variant 27 (inf)",
        "scanner_pitch_offset_oct = variant 26 (nan)",
        "scanner_pitch_offset_oct = variant 27 (inf)",
        "mcp_server_port = variant 27 (inf)",
    ];
    BOTH.contains(&name) || (loader == "configurator" && CONFIGURATOR.contains(&name))
}

/// No backup file, the config loaded, and a single-key case got that key's default.
fn check_overflow_departure(name: &str, got: &Value) -> Result<(), String> {
    if got["result"].get("err").is_some() {
        return Err("load failed".into());
    }
    let files = got["files"].as_object().unwrap();
    if files.len() != 1 || !files.contains_key("beamtel_config.json") {
        return Err(format!("expected only the config file, found {:?}", files.keys()));
    }
    if let Some((key, _)) = name.split_once(" = variant ")
        && !name.starts_with("all keys")
        && got["result"]["diff"].get(key).is_some()
    {
        return Err(format!("{key} did not fall back to its default"));
    }
    Ok(())
}

/// Hold the config open with read sharing only: reads succeed, the rename onto it fails
/// with a sharing violation, as when another process has it open mid-read.
fn hold_for_reading(path: &Path) -> fs::File {
    fs::OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ
        .open(path)
        .expect("hold")
}

/// python-bugs.md 2: a migration whose write-back fails must not take the corruption
/// path. The Python renamed the good config to the backup and wrote defaults.
#[test]
fn failed_migration_write_keeps_the_config() {
    let cases: [(&str, &[u8]); 2] = [
        ("engine", br#"{"announce_gear": true, "units": "metric", "speech_rate": 70}"#),
        (
            "configurator",
            br#"{"ai_describer_api_key": "sk-plain", "units": "metric", "speech_rate": 70}"#,
        ),
    ];
    for (loader, input) in cases {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("beamtel_config.json");
        fs::write(&path, input).expect("write input");
        let hold = hold_for_reading(&path);
        let result = match loader {
            "engine" => load_config_engine_at(&path),
            _ => load_config_configurator_at(&path),
        };
        drop(hold);
        let cfg = result.unwrap_or_else(|e| panic!("{loader}: load failed: {e}"));
        assert_eq!(cfg["units"].py_str(), "metric", "{loader}: settings lost");
        assert_eq!(cfg["speech_rate"].py_str(), "70", "{loader}: settings lost");
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["beamtel_config.json"], "{loader}: backup or temp file left");
        assert_eq!(fs::read(&path).unwrap(), input, "{loader}: config rewritten");
        // With the file free again, the next load completes the migration.
        let cfg = match loader {
            "engine" => load_config_engine_at(&path),
            _ => load_config_configurator_at(&path),
        }
        .unwrap();
        assert_eq!(cfg["units"].py_str(), "metric");
        assert_ne!(fs::read(&path).unwrap(), input, "{loader}: migration not retried");
    }
}

/// The blob secretstore.py sealed when the fixture was generated.
fn python_blob(fx: &Value) -> String {
    let case = fx["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "python-sealed secret")
        .unwrap();
    let input: Value = serde_json::from_str(case["input"].as_str().unwrap()).unwrap();
    input["ai_describer_api_key"].as_str().unwrap().to_string()
}

fn python_blob_unseals(fx: &Value) -> bool {
    secretstore::unprotect(&python_blob(fx)).is_some()
}

#[test]
fn python_sealed_secret_unseals() {
    let fx = load("config");
    let plain = fx["py_sealed_plaintext"].as_str().unwrap();
    let blob = python_blob(&fx);
    match secretstore::unprotect(&blob) {
        Some(got) => assert_eq!(got, plain),
        None => eprintln!("skipped: fixture sealed by another Windows account"),
    }
}
