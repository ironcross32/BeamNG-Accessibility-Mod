//! AI provider registry (providers, models, config keys, defaults), from ai_describer.py.
//!
//! Only the registry half lives here, so both config loaders can validate the AI
//! settings without the HTTP and capture code; the request builders, validation and
//! description logging are in `beam_engine::ai_describer`, which dispatches on
//! [`provider_info`].

use indexmap::IndexMap;

use crate::pyfmt::py_strip;
use crate::pyval::{PyDict, PyExc, PyValue};

pub const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const GEMINI_DEFAULT_MODEL: &str = "models/gemini-3-flash-preview";

/// Curated allowlist of vision-capable generateContent models (the API listing exposes
/// no "vision" capability flag, so this is maintained by hand from the published model
/// list). (model_name, display_name), newest first. TTS, image-generation, embeddings,
/// veo/imagen/lyria, robotics, computer-use, aqa, deep-research and antigravity models
/// are intentionally excluded.
pub const GEMINI_VISION_MODELS: [(&str, &str); 16] = [
    ("models/gemini-3-flash-preview", "Gemini 3 Flash Preview"),
    ("models/gemini-3-pro-preview", "Gemini 3 Pro Preview"),
    ("models/gemini-3.1-pro-preview", "Gemini 3.1 Pro Preview"),
    (
        "models/gemini-3.1-flash-lite-preview",
        "Gemini 3.1 Flash Lite Preview",
    ),
    ("models/gemini-3.1-flash-lite", "Gemini 3.1 Flash Lite"),
    ("models/gemini-3.5-flash", "Gemini 3.5 Flash"),
    ("models/gemini-2.5-flash", "Gemini 2.5 Flash"),
    ("models/gemini-2.5-pro", "Gemini 2.5 Pro"),
    ("models/gemini-2.5-flash-lite", "Gemini 2.5 Flash-Lite"),
    ("models/gemini-2.0-flash", "Gemini 2.0 Flash"),
    ("models/gemini-2.0-flash-001", "Gemini 2.0 Flash 001"),
    ("models/gemini-2.0-flash-lite", "Gemini 2.0 Flash-Lite"),
    ("models/gemini-2.0-flash-lite-001", "Gemini 2.0 Flash-Lite 001"),
    ("models/gemini-flash-latest", "Gemini Flash Latest"),
    ("models/gemini-flash-lite-latest", "Gemini Flash-Lite Latest"),
    ("models/gemini-pro-latest", "Gemini Pro Latest"),
];

pub const OPENAI_BASE: &str = "https://api.openai.com/v1";
pub const OPENAI_DEFAULT_BASE_URL: &str = OPENAI_BASE;
pub const OPENAI_DEFAULT_MODEL: &str = "gpt-5.6-terra";

/// Curated allowlist of vision-capable OpenAI models. GET /v1/models has no capability
/// flag either. o4-mini looks vision-capable by naming convention but is not, so it
/// is deliberately absent. (model_id, display_name), newest/best first.
pub const OPENAI_VISION_MODELS: [(&str, &str); 15] = [
    ("gpt-5.6-terra", "GPT-5.6 Terra (balanced)"),
    ("gpt-5.6-sol", "GPT-5.6 Sol (frontier)"),
    ("gpt-5.6-luna", "GPT-5.6 Luna (fast, low cost)"),
    ("gpt-5.5", "GPT-5.5"),
    ("gpt-5.4", "GPT-5.4"),
    ("gpt-5.4-mini", "GPT-5.4 Mini"),
    ("gpt-5.2", "GPT-5.2"),
    ("gpt-5.1", "GPT-5.1"),
    ("gpt-5-mini", "GPT-5 Mini"),
    ("gpt-5-nano", "GPT-5 Nano"),
    ("gpt-4.1", "GPT-4.1"),
    ("gpt-4.1-mini", "GPT-4.1 Mini"),
    ("gpt-4o", "GPT-4o"),
    ("gpt-4o-mini", "GPT-4o Mini"),
    ("o3", "o3"),
];

/// Vocabularies for the two OpenAI dropdowns. (value, display_name).
pub const OPENAI_REASONING_EFFORTS: [(&str, &str); 5] = [
    ("none", "None (fastest)"),
    ("minimal", "Minimal"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
];
pub const OPENAI_DETAIL_LEVELS: [(&str, &str); 3] = [
    ("auto", "Auto (full resolution)"),
    ("high", "High"),
    ("low", "Low"),
];

/// (provider_id, display_name), in the order the configurator lists them.
pub const PROVIDERS: [(&str, &str); 2] = [("gemini", "Google Gemini"), ("openai", "OpenAI")];
pub const DEFAULT_PROVIDER: &str = "gemini";
/// The config key that is not provider-specific.
pub const PROVIDER_CFG_KEY: &str = "ai_describer_provider";

/// An extra request parameter's control type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtraKind {
    Text,
    Choice,
}

impl ExtraKind {
    /// The Python descriptor's `kind` string.
    pub fn as_str(self) -> &'static str {
        match self {
            ExtraKind::Text => "text",
            ExtraKind::Choice => "choice",
        }
    }
}

/// A provider-specific request parameter. The configurator builds one control per
/// entry and hides them all when another provider is active, so adding one here is
/// all it takes to surface it in the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extra {
    pub key: &'static str,
    pub arg: &'static str,
    pub kind: ExtraKind,
    pub label: &'static str,
    pub default: &'static str,
    /// `(value, display_name)`; empty for a text extra.
    pub values: &'static [(&'static str, &'static str)],
    pub help: &'static str,
}

/// One registry entry (Python's `_PROVIDER_INFO[pid]`, minus the two callables, which
/// the engine dispatches on `id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub display: &'static str,
    pub models: &'static [(&'static str, &'static str)],
    pub default_model: &'static str,
    pub key_cfg: &'static str,
    pub model_cfg: &'static str,
    pub key_help: &'static str,
    pub extras: &'static [Extra],
}

const OPENAI_EXTRAS: [Extra; 3] = [
    Extra {
        key: "ai_describer_openai_base_url",
        arg: "base_url",
        kind: ExtraKind::Text,
        label: "Base URL:",
        default: OPENAI_DEFAULT_BASE_URL,
        values: &[],
        help: "The API endpoint. Change this only to reach a proxy or an \
               OpenAI-compatible server.",
    },
    Extra {
        key: "ai_describer_openai_reasoning_effort",
        arg: "reasoning_effort",
        kind: ExtraKind::Choice,
        label: "Reasoning effort:",
        default: "low",
        values: &OPENAI_REASONING_EFFORTS,
        help: "How long the model thinks before answering. Higher settings \
               are slower and cost more. Ignored by models without reasoning.",
    },
    Extra {
        key: "ai_describer_openai_detail",
        arg: "detail",
        kind: ExtraKind::Choice,
        label: "Image detail:",
        default: "auto",
        values: &OPENAI_DETAIL_LEVELS,
        help: "How much image resolution is sent. Low costs less but loses \
               on-screen text.",
    },
];

pub const PROVIDER_INFO: [ProviderInfo; 2] = [
    ProviderInfo {
        id: "gemini",
        display: "Google Gemini",
        models: &GEMINI_VISION_MODELS,
        default_model: GEMINI_DEFAULT_MODEL,
        key_cfg: "ai_describer_api_key",
        model_cfg: "ai_describer_model",
        key_help: "Paste your Google Gemini API key:",
        extras: &[],
    },
    ProviderInfo {
        id: "openai",
        display: "OpenAI",
        models: &OPENAI_VISION_MODELS,
        default_model: OPENAI_DEFAULT_MODEL,
        key_cfg: "ai_describer_openai_api_key",
        model_cfg: "ai_describer_openai_model",
        key_help: "Paste your OpenAI API key:",
        extras: &OPENAI_EXTRAS,
    },
];

/// The registry entry for a provider id, falling back to the default.
pub fn provider_info(provider: &str) -> &'static ProviderInfo {
    PROVIDER_INFO
        .iter()
        .find(|p| p.id == provider)
        .unwrap_or_else(|| {
            PROVIDER_INFO
                .iter()
                .find(|p| p.id == DEFAULT_PROVIDER)
                .unwrap_or(&PROVIDER_INFO[0])
        })
}

pub fn provider_display_name(provider: &str) -> &'static str {
    provider_info(provider).display
}

/// `[(model_name, display_name)]` for a provider.
pub fn vision_models_for(provider: &str) -> &'static [(&'static str, &'static str)] {
    provider_info(provider).models
}

pub fn model_names_for(provider: &str) -> Vec<&'static str> {
    vision_models_for(provider).iter().map(|(n, _)| *n).collect()
}

pub fn default_model_for(provider: &str) -> &'static str {
    provider_info(provider).default_model
}

/// `(api_key_config_name, model_config_name)` for a provider.
pub fn config_keys_for(provider: &str) -> (&'static str, &'static str) {
    let info = provider_info(provider);
    (info.key_cfg, info.model_cfg)
}

/// The provider's extra-parameter descriptors (possibly empty).
pub fn extras_for(provider: &str) -> &'static [Extra] {
    provider_info(provider).extras
}

/// The key-entry prompt for a provider.
pub fn key_help_for(provider: &str) -> &'static str {
    provider_info(provider).key_help
}

/// Every config key the AI Describer owns, across all providers. Both configurator
/// panels use this to avoid clobbering each other's writes.
pub fn all_config_keys() -> Vec<&'static str> {
    let mut keys = vec![PROVIDER_CFG_KEY, "ai_describer_disable_ui_toggle"];
    for (pid, _disp) in PROVIDERS {
        let info = provider_info(pid);
        keys.push(info.key_cfg);
        keys.push(info.model_cfg);
        keys.extend(info.extras.iter().map(|ex| ex.key));
    }
    keys
}

/// Build the `**opts` for a provider from a config mapping: `arg -> value`.
///
/// Missing or invalid values fall back to the descriptor's default, so a hand-edited
/// config can't produce a malformed request. `Err(AttributeError)` where Python's
/// `(val or "").strip()` raises: a text extra holding a truthy non-string.
pub fn extra_args_for(
    provider: &str,
    cfg: Option<&PyDict>,
) -> Result<IndexMap<&'static str, String>, PyExc> {
    let mut opts = IndexMap::new();
    for ex in extras_for(provider) {
        let val = cfg.and_then(|c| c.get(ex.key));
        let out = match ex.kind {
            ExtraKind::Choice => match val {
                None => ex.default.to_string(),
                Some(PyValue::Str(s)) if ex.values.iter().any(|(v, _)| v == s) => s.clone(),
                Some(_) => ex.default.to_string(),
            },
            ExtraKind::Text => {
                let text = match val {
                    None => ex.default.to_string(),
                    Some(PyValue::Str(s)) => s.clone(),
                    Some(v) if !v.truthy() => String::new(),
                    Some(_) => return Err(PyExc::AttributeError),
                };
                let stripped = py_strip(&text);
                if stripped.is_empty() {
                    ex.default.to_string()
                } else {
                    stripped.to_string()
                }
            }
        };
        opts.insert(ex.arg, out);
    }
    Ok(opts)
}

/// The friendly display name for a model name, or the name itself.
pub fn display_name_for<'a>(model_name: &'a str, provider: &str) -> &'a str {
    for (name, disp) in vision_models_for(provider) {
        if *name == model_name {
            return disp;
        }
    }
    model_name
}
