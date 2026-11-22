//! The AI Describer panel's backend (`config_ui.AIDescriberPanel`): its state as the
//! frontend sees it (secrets reduced to `has_key`), `_save` with the key handling
//! `_clear_api_key` / `_on_validated` did first, and key validation. CONTRACT.md,
//! "AI Describer panel".

use std::path::Path;

use beam_base::ai_registry::{self, PROVIDERS};
use beam_base::config;
use beam_base::pyfmt::py_strip;
use beam_base::pyval::{PyDict, PyValue, to_json};
use beam_base::secretstore;
use beam_engine::ai_describer::{self, Opts};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// `describer_save`'s argument.
#[derive(Debug, Clone, Deserialize)]
pub struct SaveArgs {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub extras: Map<String, Value>,
    #[serde(default)]
    pub disable_ui_toggle: bool,
    pub key_action: String,
    #[serde(default)]
    pub key: Option<String>,
}

/// `describer_validate_key`'s argument.
#[derive(Debug, Clone, Deserialize)]
pub struct ValidateArgs {
    pub provider: String,
    pub key: String,
    pub model: String,
    #[serde(default)]
    pub extras: Map<String, Value>,
}

/// `bool((cfg.get(key, "") or "").strip())`.
fn has_key(snapshot: &PyDict, key_cfg: &str) -> bool {
    match snapshot.get(key_cfg) {
        Some(PyValue::Str(s)) => !py_strip(s).is_empty(),
        Some(v) => v.truthy(),
        None => false,
    }
}

/// The state object built from the snapshot.
pub fn state(snapshot: &PyDict) -> Value {
    let mut providers = Map::new();
    for (pid, _display) in PROVIDERS {
        let (key_cfg, model_cfg) = ai_registry::config_keys_for(pid);
        let mut p = Map::new();
        p.insert("has_key".into(), json!(has_key(snapshot, key_cfg)));
        if let Some(m) = snapshot.get(model_cfg) {
            p.insert("model".into(), to_json(m));
        }
        let mut extras = Map::new();
        for ex in ai_registry::extras_for(pid) {
            if let Some(v) = snapshot.get(ex.key) {
                extras.insert(ex.key.into(), to_json(v));
            }
        }
        p.insert("extras".into(), Value::Object(extras));
        providers.insert(pid.into(), Value::Object(p));
    }
    json!({
        "provider": snapshot.get(ai_registry::PROVIDER_CFG_KEY).map(to_json).unwrap_or(Value::Null),
        "disable_ui_toggle": snapshot.get("ai_describer_disable_ui_toggle").is_some_and(PyValue::truthy),
        "providers": providers,
    })
}

fn value_to_py(v: &Value) -> PyValue {
    match v {
        Value::String(s) => PyValue::Str(s.clone()),
        other => beam_base::pyval::from_json(other),
    }
}

/// `describer_save` against `path`. The snapshot is mutated by the key action even when
/// the write then fails, as `cur_cfg` was in the Python.
pub fn save_at(path: &Path, snapshot: &mut PyDict, args: &SaveArgs) -> Value {
    let (key_cfg, model_cfg) = ai_registry::config_keys_for(&args.provider);
    match args.key_action.as_str() {
        "clear" => {
            snapshot.insert(key_cfg.into(), PyValue::from(""));
        }
        "set" => {
            let key = args.key.clone().unwrap_or_default();
            snapshot.insert(key_cfg.into(), PyValue::Str(secretstore::protect(&key)));
        }
        _ => {}
    }
    let mut cfg = match config::load_config_configurator_at(path) {
        Ok(c) => c,
        Err(_) => snapshot.clone(),
    };
    cfg.insert(
        ai_registry::PROVIDER_CFG_KEY.into(),
        PyValue::Str(args.provider.clone()),
    );
    cfg.insert(model_cfg.into(), PyValue::Str(args.model.clone()));
    cfg.insert(
        key_cfg.into(),
        snapshot.get(key_cfg).cloned().unwrap_or(PyValue::from("")),
    );
    for ex in ai_registry::extras_for(&args.provider) {
        let v = args
            .extras
            .get(ex.key)
            .map(value_to_py)
            .unwrap_or_else(|| PyValue::from(ex.default));
        cfg.insert(ex.key.into(), v);
    }
    cfg.insert(
        "ai_describer_disable_ui_toggle".into(),
        PyValue::Bool(args.disable_ui_toggle),
    );
    let (ok, error) = match config::write_config(path, &cfg) {
        Ok(()) => {
            *snapshot = cfg;
            (true, Value::Null)
        }
        Err(e) => (false, json!(e.to_string())),
    };
    json!({"ok": ok, "error": error, "state": state(snapshot)})
}

fn extra_text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// `validate_api_key(key, provider=provider, model=model, **extras)`. Blocking; the
/// command runs it off the main thread.
///
/// Where the Python's worker thread would die on an escaped exception (a base URL with
/// no scheme) and leave the button disabled for good, this reports the exception as a
/// failed validation instead.
pub fn validate(args: &ValidateArgs) -> Value {
    let opts = Opts {
        model: Some(args.model.clone()),
        base_url: extra_text(args.extras.get("base_url")),
        reasoning_effort: extra_text(args.extras.get("reasoning_effort")),
        detail: extra_text(args.extras.get("detail")),
    };
    let r = std::panic::catch_unwind(|| {
        ai_describer::try_validate_api_key(
            &args.key,
            &args.provider,
            ai_describer::VALIDATE_TIMEOUT_SEC,
            &opts,
        )
    });
    let (ok, err) = match r {
        Ok(Ok((ok, err))) => (ok, err),
        Ok(Err(e)) => (false, Some(e)),
        Err(_) => (false, Some("validation failed unexpectedly".to_string())),
    };
    json!({"ok": ok, "error": err})
}
