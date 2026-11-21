//! Moved out of `src/dialogs.rs`: see build.rs for why this crate's tests are integration tests.

use beam_app::dialogs::*;
use beam_engine::ui_sink::{Answer, Buttons, MessageKind};

#[test]
fn maps_the_frontend_names() {
    let a = MessageBoxArgs {
        title: "Reset to Defaults".into(),
        text: "?".into(),
        kind: "question".into(),
        buttons: "yes_no".into(),
        default: "no".into(),
    };
    let m = a.to_message_box();
    assert_eq!(m.kind, MessageKind::Question);
    assert_eq!(m.buttons, Buttons::YesNo);
    assert_eq!(m.default, Answer::No);
    assert_eq!(answer_name(Answer::Yes), "yes");
}
