//! Golden cases from `fixtures/gen/logger.py`: rotation and the bytes on disk,
//! plus the line format and the second sink.

use std::sync::Arc;

use base64::Engine as _;
use beam_base::logger::*;
use beam_testutil::{load, s};
use parking_lot::Mutex;

#[test]
fn rotation_matches_python() {
    let fx = load("logger");
    let messages: Vec<&str> = fx["messages"].as_array().unwrap().iter().map(s).collect();
    for run in fx["runs"].as_array().unwrap() {
        let max = run["max_bytes"].as_u64().unwrap();
        let backups = run["backups"].as_u64().unwrap() as u32;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOG_BASENAME);
        let mut h = RotatingFileHandler::new(&path, max, backups).unwrap();
        for m in &messages {
            h.emit(m).unwrap();
        }
        drop(h);
        let mut got = serde_json::Map::new();
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for n in names {
            let data = std::fs::read(dir.path().join(&n)).unwrap();
            got.insert(
                n,
                serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(data)),
            );
        }
        assert_eq!(
            serde_json::Value::Object(got),
            run["files"],
            "max_bytes {max}, backups {backups}"
        );
    }
}

#[test]
fn line_format_matches_python() {
    let fx = load("logger");
    assert_eq!(s(&fx["format"]), "%(asctime)s [%(levelname)s] %(message)s");
    assert_eq!(s(&fx["levels"]["20"]), level_name(log::Level::Info));
    assert_eq!(s(&fx["levels"]["30"]), level_name(log::Level::Warn));
    assert_eq!(s(&fx["levels"]["40"]), level_name(log::Level::Error));
    assert_eq!(s(&fx["levels"]["10"]), level_name(log::Level::Debug));
    assert_eq!(
        format_line("2026-11-14 09:05:01", log::Level::Warn, "hello"),
        "2026-11-14 09:05:01 [WARNING] hello"
    );
    let ts = local_timestamp();
    assert_eq!(ts.len(), 19, "{ts}");
    assert_eq!(&ts[4..5], "-");
    assert_eq!(&ts[10..11], " ");
}

struct Capture(Mutex<Vec<String>>);

impl LogSink for Capture {
    fn emit(&self, _level: log::Level, line: &str) {
        self.0.lock().push(line.to_string());
    }
}

/// The global logger, through the `log` facade, into a file and a second sink. The only
/// test in this binary that initialises the global logger.
#[test]
fn facade_reaches_file_and_sink() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(LOG_BASENAME);
    init_with(&path, false);
    let cap = Arc::new(Capture(Mutex::new(Vec::new())));
    let id = add_sink(cap.clone());
    log::info!(target: "beam_base::test", "from beam-base test {}", 42);
    log::debug!(target: "beam_base::test", "dropped at INFO");
    log::warn!(target: "some_dependency", "dropped: not ours");
    remove_sink(id);
    log::info!(target: "beam_base::test", "after removal");
    let lines = cap.0.lock().clone();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].ends_with(" [INFO] from beam-base test 42"),
        "{lines:?}"
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let file_lines: Vec<&str> = text.split("\r\n").collect();
    assert!(
        file_lines[0]
            .ends_with(" [INFO] ==================== Logger Initialized ====================")
    );
    assert!(file_lines[1].ends_with(" [INFO] from beam-base test 42"));
    assert!(file_lines[2].ends_with(" [INFO] after removal"));
    assert_eq!(file_lines.len(), 4, "{text:?}");
}
