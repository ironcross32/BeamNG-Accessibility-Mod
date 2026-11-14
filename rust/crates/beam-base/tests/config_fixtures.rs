//! Golden cases from `rust/fixtures/gen/config.py`: both Python `load_config`s against a
//! table of config files, compared on the returned dict and every byte left on disk.
//! Each case runs in its own temporary directory; the real config is never touched.

use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use base64::Engine as _;
use beam_base::config::{
    self, default_config, default_config_engine, load_config_configurator_at,
    load_config_engine_at,
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
