//! Moved out of `src/shell.rs`: see build.rs for why this crate's tests are integration tests.

use beam_app::shell::*;
use beam_engine::ui_sink::UiSink;
use serde_json::json;

#[test]
fn mode_and_title() {
    assert_eq!(Mode::from_args(["x".to_string()]), Mode::Main);
    assert_eq!(
        Mode::from_args(["x".to_string(), "--configurator".to_string()]),
        Mode::Configurator
    );
    assert_eq!(Mode::Configurator.title(), "BeamTel Configurator");
    assert!(Mode::Main.title().starts_with("BeamNG Accessibility "));
}

#[test]
fn events_are_buffered_until_ready() {
    let s = Shell::new(Mode::Configurator);
    s.push_log("a");
    s.console_message("RESP|ok|1");
    s.console_message("CTX|0|GE - Lua");
    s.console_message("CTXEND");
    let r = s.frontend_ready();
    assert_eq!(r["log_lines"], json!(["a"]));
    assert_eq!(r["console_lines"], json!(["  = 1"]));
    assert_eq!(r["console_contexts"], json!([{"index": 0, "label": "GE - Lua"}]));
    // Ready now: nothing more is buffered (no app attached, so emits go nowhere).
    s.push_log("b");
    assert_eq!(s.buffered_log_count(), 0);
}

#[test]
fn nothing_after_close_starts() {
    let s = Shell::new(Mode::Configurator);
    s.begin_close();
    s.push_log("late");
    s.console_message("OUT|late");
    let r = s.frontend_ready();
    assert_eq!(r["log_lines"], json!([]));
    assert_eq!(r["console_lines"], json!([]));
}
