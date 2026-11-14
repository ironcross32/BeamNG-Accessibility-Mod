//! Owner: E7. The AI Describer: `_ai_describe_worker` and `_trigger_ai_describe`
//! (`beamtel.py:9604-9701`), plus the non-registry half of `ai_describer.py` (capture,
//! the Gemini and OpenAI request builders, key validation, the description log). The
//! registry half is `beam_base::ai_registry` (B1).
//!
//! STUB: signatures fixed by EA; bodies to be ported by E7.

use std::path::PathBuf;
use std::sync::Arc;

use crate::compat::beam_audio::controller::AudioController;
use crate::not_ported;

/// `ai_describer.LOG_PATH` (`ai_descriptions.log` in the config directory).
pub fn log_path() -> PathBuf {
    beam_base::paths::config_dir().join("ai_descriptions.log")
}

/// `_trigger_ai_describe(audio_controller)`: buzz if busy, else start the worker thread.
pub fn trigger_ai_describe(audio_controller: Option<Arc<AudioController>>) {
    let _ = audio_controller;
    not_ported!("ai_describer::trigger_ai_describe");
}

/// `ai_describer.validate_api_key(...)` for the AI Describer tab (U2 calls it):
/// `(ok, error_message)`.
pub fn validate_api_key(provider: &str, api_key: &str) -> (bool, Option<String>) {
    let _ = (provider, api_key);
    not_ported!("ai_describer::validate_api_key");
    (false, Some("not yet ported".to_string()))
}
