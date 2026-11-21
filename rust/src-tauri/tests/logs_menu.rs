//! Moved out of `src/logs_menu.rs`: see build.rs for why this crate's tests are integration tests.

use std::path::{Path, PathBuf};

use beam_app::logs_menu::*;
use beam_engine::ui_sink::Answer;

fn paths(dir: &Path) -> LogPaths {
    LogPaths {
        app_log: dir.join("bnvdahook.log"),
        speech_log: dir.join("speech.log"),
        ai_log: dir.join("ai_descriptions.log"),
        dom_dump: dir.join("dom.txt"),
        scanner_diag: dir.join("scan.csv"),
        update_helper_log: dir.join("update").join("apply_update.log"),
    }
}

#[test]
fn flat_without_backups_submenu_with_them() {
    let d = tempfile::tempdir().unwrap();
    let p = paths(d.path());
    let e = entries(&p);
    assert_eq!(e.len(), 6);
    assert_eq!(e[0], ("Application Log".into(), Target::File(p.app_log.clone())));
    assert_eq!(e[5].0, "Update Helper Log");

    std::fs::write(d.path().join("bnvdahook.log.2"), "x").unwrap();
    let e = entries(&p);
    assert_eq!(
        e[0].1,
        Target::Sub(vec![
            ("Current".into(), p.app_log.clone()),
            ("Previous 2".into(), d.path().join("bnvdahook.log.2")),
        ])
    );
}

#[test]
fn missing_files_say_so() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("speech.log");
    assert_eq!(
        item_label("Speech Log", &f),
        ("Speech Log (not created yet)".into(), false)
    );
    std::fs::write(&f, "x").unwrap();
    assert_eq!(item_label("Speech Log", &f), ("Speech Log".into(), true));
}

#[test]
fn a_vanished_file_is_reported() {
    let seen = std::cell::RefCell::new(Vec::new());
    handle_choice(Choice::File(PathBuf::from(r"C:\no\such\file.log")), &|m| {
        seen.borrow_mut().push((m.title.clone(), m.text.clone()));
        Answer::Ok
    });
    assert_eq!(
        seen.into_inner(),
        vec![("Not Found".into(), "File not found:\nC:\\no\\such\\file.log".into())]
    );
}
