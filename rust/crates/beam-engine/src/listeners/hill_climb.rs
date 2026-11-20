//! Owner: E2. Hill-climb challenge glue: report browsers and the challenge event callbacks (`beamtel.py:6462-6494, 6693-6725`).
//!
//! Summaries are beam-road's value type, as the recorder (`state::hill_climb_challenge()`)
//! produces and consumes them (`list_reports`, `report_lines`, the finalize callback).

use std::sync::Arc;

use beam_road::challenge_results::{HillClimbChallengeRecorder, completion_speech};
use beam_road::pyjson::{PyErr, Value as PyJson, get_or_empty, or_empty_dict};

use crate::convert::{json_to_pyjson, pyjson_to_json};
use crate::listeners::road::send_road_command;
use crate::speech_out::say;
use crate::state::{hill_climb_challenge, road_diagnostic_telemetry_snapshot};
use crate::units::units_mode;
use crate::virtual_browser::{BrowserOpts, open_virtual_browser};

/// A Python exception nothing in the call chain catches.
fn raise(e: PyErr) -> ! {
    panic!("{}: {}", e.type_name(), e.message())
}

/// The history browser's title.
#[doc(hidden)]
pub fn history_title(count: usize) -> String {
    format!("Hill-climb report history, {count} attempts. Press Enter for details")
}

/// `open_hill_climb_report(summary, return_to_history=None, announce_interrupt=True)`:
/// one persisted challenge summary in the existing virtual browser.
pub fn open_hill_climb_report(
    summary: PyJson,
    return_to_history: Option<i64>,
    announce_interrupt: bool,
) {
    let lines = HillClimbChallengeRecorder::report_lines(&summary, &units_mode())
        .unwrap_or_else(|e| raise(e));
    let on_escape: Option<crate::state::OnEscape> = return_to_history
        .map(|index| Arc::new(move || open_hill_climb_history(index)) as crate::state::OnEscape);
    open_virtual_browser(
        lines,
        BrowserOpts {
            title: Some("Hill-climb report".to_string()),
            on_escape,
            announce_interrupt,
            ..Default::default()
        },
    );
}

/// `open_hill_climb_history(start_index=0)`: retained attempts newest-first; Enter opens
/// one for its full report.
pub fn open_hill_climb_history(start_index: i64) {
    let reports = hill_climb_challenge().list_reports(100);
    if reports.is_empty() {
        say("No hill-climb reports available", true, true, "python");
        return;
    }
    let units = units_mode();
    let lines: Vec<String> = reports
        .iter()
        .map(|item| {
            HillClimbChallengeRecorder::history_line(item, &units).unwrap_or_else(|e| raise(e))
        })
        .collect();

    let open_selected = Arc::new(
        |index: usize, _line: &str, summary: Option<serde_json::Value>| {
            let summary = json_to_pyjson(&summary.unwrap_or(serde_json::Value::Null));
            open_hill_climb_report(summary, Some(index as i64), true);
        },
    );

    let title = history_title(lines.len());
    open_virtual_browser(
        lines,
        BrowserOpts {
            title: Some(title),
            on_enter: Some(open_selected),
            entry_data: reports.iter().map(pyjson_to_json).collect(),
            start_index,
            ..Default::default()
        },
    );
}

/// `_present_hill_climb_result(summary, auto_open)` (command worker).
fn present_hill_climb_result(summary: PyJson, auto_open: bool) {
    if !auto_open || !summary.get("status").is_some_and(|s| s.eq_str("completed")) {
        return;
    }
    say(
        &completion_speech(&summary).unwrap_or_else(|e| raise(e)),
        true,
        true,
        "hill_climb",
    );
    open_hill_climb_report(summary, None, false);
}

/// The data-quality log line `_hill_climb_finalized` writes.
#[doc(hidden)]
pub fn data_quality_line(summary: &PyJson) -> Result<String, PyErr> {
    let stats = or_empty_dict(summary.py_get("statistics")?);
    let get_or = |v: &PyJson, key: &str, default: PyJson| -> Result<String, PyErr> {
        Ok(v.py_get(key)?.cloned().unwrap_or(default).py_str())
    };
    let gaps = get_or_empty(stats.py_get("packet_gaps")?, "count")?
        .cloned()
        .unwrap_or(PyJson::Int(0))
        .py_str();
    Ok(format!(
        "Hill-climb data quality: attempt={}, status={}, samples={}, sample_rate_hz={}, packet_gaps={}",
        get_or(summary, "attempt_id", "unknown".into())?,
        get_or(summary, "status", "unknown".into())?,
        get_or(&stats, "samples", PyJson::Int(0))?,
        get_or(&stats, "sample_rate_hz", PyJson::Int(0))?,
        gaps,
    ))
}

/// `_hill_climb_finalized(summary, auto_open)`: the recorder's finalize callback.
pub fn hill_climb_finalized(summary: &PyJson, auto_open: bool) {
    let line = data_quality_line(summary).unwrap_or_else(|e| raise(e));
    log::info!("{line}");
    let summary = summary.clone();
    crate::keyboard::command_dispatch(Box::new(move || {
        present_hill_climb_result(summary, auto_open)
    }));
}

/// `_on_hill_climb_event(data)`: the UI bridge's challenge-event callback, on the Lua TCP
/// relay thread. The bridge logs anything raised here (`challenge_event callback error`).
pub fn on_hill_climb_event(data: &serde_json::Value) {
    let telemetry = json_to_pyjson(&road_diagnostic_telemetry_snapshot());
    let action = hill_climb_challenge()
        .handle_event(&json_to_pyjson(data), Some(&telemetry))
        .unwrap_or_else(|e| raise(e));
    let Some(action) = action.filter(PyJson::truthy) else {
        return;
    };
    let capture = match action.get("capture") {
        None | Some(PyJson::Null) => return,
        Some(c) => c.truthy(),
    };
    send_road_command(if capture { "CAPTURE_ON" } else { "CAPTURE_OFF" });
}
