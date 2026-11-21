//! Moved out of `src/console_view.rs`: see build.rs for why this crate's tests are integration tests.

use beam_app::console_view::*;

fn run(records: &[&str]) -> Vec<Action> {
    let mut v = ConsoleView::new();
    records.iter().flat_map(|r| v.handle(r)).collect()
}

#[test]
fn single_value_is_shown_and_spoken() {
    assert_eq!(
        run(&["RESP|ok", "OUT|= 42", "EXECEND"]),
        vec![Action::Line("  = 42".into()), Action::Speak("42".into())]
    );
}

#[test]
fn two_lines_are_not_spoken() {
    assert_eq!(
        run(&["RESP|ok|a", "OUT|b", "EXECEND"]),
        vec![Action::Line("  = a".into()), Action::Line("  b".into())]
    );
}

#[test]
fn error_and_queued() {
    assert_eq!(
        run(&[
            "RESP|error|bad|thing",
            "EXECEND",
            "RESP|queued",
            "EXECEND",
            "RESP|error",
            "EXECEND"
        ]),
        vec![
            Action::Line("  error: bad|thing".into()),
            Action::Speak("error: bad|thing".into()),
            Action::Line("  (queued)".into()),
            Action::Speak("queued".into()),
            Action::Line("  error: ".into()),
            Action::Speak("error".into()),
        ]
    );
}

#[test]
fn log_lines_are_never_captured() {
    assert_eq!(
        run(&["LOG|12:00|I|Test|a|b", "LOG|short", "EXECEND"]),
        vec![
            Action::Line("  [I Test] a|b".into()),
            Action::Line("  [ ] short".into()),
        ]
    );
}

#[test]
fn output_without_a_resp_is_shown_not_spoken() {
    assert_eq!(run(&["OUT|x", "EXECEND"]), vec![Action::Line("  x".into())]);
}

#[test]
fn contexts_accumulate_until_ctxend() {
    assert_eq!(
        run(&[
            "CTX|0|GE - Lua",
            "CTX|x|bad",
            "CTX|3|veh|1",
            "CTX|4",
            "CTXEND",
            "CTXEND"
        ]),
        vec![Action::Contexts(vec![
            (0, "GE - Lua".into()),
            (3, "veh|1".into())
        ])]
    );
}

#[test]
fn whitespace_only_lines_do_not_count() {
    assert_eq!(
        run(&["RESP|ok", "OUT|   ", "OUT|= 7 ", "EXECEND"]),
        vec![
            Action::Line("     ".into()),
            Action::Line("  = 7 ".into()),
            Action::Speak("7".into())
        ]
    );
}
