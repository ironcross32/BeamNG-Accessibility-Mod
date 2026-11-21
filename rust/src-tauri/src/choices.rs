//! `ui_choices`: every list a panel shows, so no table is duplicated in JavaScript.
//! Labels are verbatim from `config_ui.py`; values from the registries in beam-base.

use beam_base::ai_registry::{self, PROVIDERS};
use beam_base::{gear_report, hrtf_sets};
use serde_json::{Value, json};

use crate::panel::{CALLOUT_INTERVALS, SPEED_INTERVALS};

fn items<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Value {
    Value::Array(
        pairs
            .into_iter()
            .map(|(v, l)| json!({"value": v, "label": l}))
            .collect(),
    )
}

fn provider(pid: &str) -> Value {
    let info = ai_registry::provider_info(pid);
    let extras: Vec<Value> = info
        .extras
        .iter()
        .map(|ex| {
            let mut e = json!({
                "key": ex.key,
                "arg": ex.arg,
                "kind": ex.kind.as_str(),
                "label": ex.label,
                "default": ex.default,
                "help": ex.help,
            });
            if ex.kind == ai_registry::ExtraKind::Choice {
                e["values"] = items(ex.values.iter().copied());
            }
            e
        })
        .collect();
    json!({
        "id": info.id,
        "display": info.display,
        "key_help": info.key_help,
        "key_cfg": info.key_cfg,
        "model_cfg": info.model_cfg,
        "default_model": info.default_model,
        "models": items(info.models.iter().copied()),
        "extras": extras,
    })
}

/// The `ui_choices` value.
pub fn ui_choices() -> Value {
    let speed_labels = |unit: &str| -> Vec<String> {
        SPEED_INTERVALS
            .iter()
            .map(|v| format!("Every {v} {unit}"))
            .collect()
    };
    json!({
        // The audio stack is compiled in, so tone playback is always available.
        "audio_test_ok": true,
        "speech_auto_label": "Auto (best available)",
        "units": items([("imperial", "Imperial (mph, °F, psi)"), ("metric", "Metric (km/h, °C, bar)")]),
        "telemetry_protocols": items([("extended", "Extended"), ("outgauge", "OutGauge")]),
        "renderers": items([("d3d", "Direct3D 11"), ("vulkan", "Vulkan")]),
        "speed_intervals": {
            "values": SPEED_INTERVALS,
            "labels": { "imperial": speed_labels("mph"), "metric": speed_labels("km/h") },
        },
        "callout_intervals": CALLOUT_INTERVALS
            .iter()
            .map(|v| json!({"value": v, "label": format!("{v} seconds")}))
            .collect::<Vec<_>>(),
        "gear_modes": items(gear_report::MODES.into_iter().zip(["Off", "As speech", "As audio"])),
        "gear_number_styles": items(gear_report::NUMBER_STYLES.into_iter()
            .zip(["Ordinals (first, second)", "Numbers (1, 2)"])),
        "gear_roots": items(gear_report::ROOT_NOTES),
        "gear_octaves": gear_report::ROOT_OCTAVES
            .iter()
            .map(|o| json!({"value": o, "label": o.to_string()}))
            .collect::<Vec<_>>(),
        "gear_scales": items(gear_report::SCALES.iter().map(|s| (s.key, s.label))),
        "hrtf_sets": items(hrtf_sets::available_sets().iter().map(|s| (s.set_id, s.label))),
        "hrtf_elevation": { "min": hrtf_sets::ELEVATION_MIN_DEG, "max": hrtf_sets::ELEVATION_MAX_DEG },
        "obstacle_sensitivities": items([("early", "Early"), ("normal", "Normal"), ("late", "Late")]),
        "console_contexts": [
            {"index": 0, "label": "GE - Lua"},
            {"index": 1, "label": "GE - TorqueScript"},
            {"index": 2, "label": "CEF/UI - JS"},
        ],
        "ai_default_provider": ai_registry::DEFAULT_PROVIDER,
        "ai_providers": PROVIDERS.iter().map(|(pid, _)| provider(pid)).collect::<Vec<_>>(),
    })
}
