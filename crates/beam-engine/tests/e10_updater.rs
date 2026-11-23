//! E10 golden tests: `updater.py` and the mod installer half of `config_ui.py`, against
//! `fixtures/data/e10_updater.json` (written by `fixtures/gen/e10_updater.py`).
//!
//! Nothing here touches the network, the real config or game folders, or starts a
//! process: HTTP is a fake [`HttpClient`], the helper launch is recorded, and every
//! directory is a fresh temp directory under Cargo's target tmpdir. Paths in messages are
//! compared with that directory replaced by `<ROOT>`, as the generator does.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use beam_base::pyval::PyValue;
use beam_engine::mod_install::{self, ModInstallEnv};
use beam_engine::ui_sink::{Answer, Buttons, MessageKind, RecordingSink, UiEvent};
use beam_engine::updater::{
    self, Gate, HelperLaunch, HttpClient, HttpError, HttpResponse, UpdaterEnv,
};
use parking_lot::Mutex;
use serde_json::{Value, json};

fn fixture() -> &'static Value {
    static F: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    F.get_or_init(|| beam_testutil::load("e10_updater"))
}

fn b64d(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .expect("fixture base64")
}

fn b64e(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("e10-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("tempdir")
}

fn root_str(t: &tempfile::TempDir) -> String {
    t.path().to_string_lossy().into_owned()
}

fn scrub(text: &str, root: &str) -> String {
    static TS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\.pre-bnvda-removal-\d{8}-\d{6}-\d{6}\.bak").unwrap()
    });
    // An empty root (cases with no paths) would otherwise match between every character.
    let t = if root.is_empty() {
        text.to_string()
    } else {
        let t = text.replace(root, "<ROOT>");
        t.replace(&root.replace('\\', "\\\\"), "<ROOTR>")
    };
    TS.replace_all(&t, ".pre-bnvda-removal-<TS>.bak")
        .into_owned()
}

/// Every file under `dir` as `{relative/posix/path: base64}`, directories as null.
fn tree(dir: &Path) -> Value {
    if !dir.is_dir() {
        return Value::Null;
    }
    let mut out = BTreeMap::new();
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Value>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            let rel = p
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if p.is_dir() {
                out.insert(format!("{rel}/"), Value::Null);
                walk(base, &p, out);
            } else {
                out.insert(rel, Value::String(b64e(&std::fs::read(&p).unwrap())));
            }
        }
    }
    walk(dir, dir, &mut out);
    Value::Object(out.into_iter().collect())
}

fn level_name(l: log::Level) -> &'static str {
    match l {
        log::Level::Error => "ERROR",
        log::Level::Warn => "WARNING",
        log::Level::Info => "INFO",
        log::Level::Debug => "DEBUG",
        log::Level::Trace => "DEBUG",
    }
}

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Call {
    url: String,
    headers: Vec<(String, String)>,
    timeout: f64,
}

struct FakeHttp {
    routes: HashMap<String, Value>,
    calls: Arc<Mutex<Vec<Call>>>,
    /// Hold the first read of a body, so a polling progress dialog can cancel first.
    slow: bool,
}

struct SlowReader {
    inner: std::io::Cursor<Vec<u8>>,
    first: bool,
}

impl Read for SlowReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.first {
            self.first = false;
            std::thread::sleep(Duration::from_millis(300));
        }
        self.inner.read(buf)
    }
}

impl HttpClient for FakeHttp {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        timeout_s: f64,
    ) -> Result<HttpResponse, HttpError> {
        self.calls.lock().push(Call {
            url: url.to_string(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            timeout: timeout_s,
        });
        let Some(spec) = self.routes.get(url) else {
            return Err(HttpError::Url(format!("no route for {url}")));
        };
        match spec["kind"].as_str().unwrap() {
            "body" => {
                let body = b64d(spec["body"].as_str().unwrap());
                let content_length = match spec.get("content_length") {
                    None => Some(body.len().to_string()),
                    Some(Value::Null) => None,
                    Some(v) => Some(v.as_str().unwrap().to_string()),
                };
                Ok(HttpResponse {
                    content_length,
                    body: Box::new(SlowReader {
                        inner: std::io::Cursor::new(body),
                        first: self.slow,
                    }),
                })
            }
            "status" => Err(HttpError::Status {
                code: spec["code"].as_u64().unwrap() as u16,
                reason: spec["reason"].as_str().unwrap().to_string(),
            }),
            "url" => Err(HttpError::Url(spec["reason"].as_str().unwrap().to_string())),
            "other" => Err(HttpError::Other(spec["msg"].as_str().unwrap().to_string())),
            k => panic!("unknown response kind {k}"),
        }
    }
}

type Launches = Arc<Mutex<Vec<HelperLaunch>>>;

struct TestEnv {
    env: UpdaterEnv,
    logs: Arc<Mutex<Vec<(log::Level, String)>>>,
    calls: Arc<Mutex<Vec<Call>>>,
    launches: Launches,
}

fn test_env(
    root: &Path,
    routes: HashMap<String, Value>,
    slow: bool,
    popen_raises: bool,
) -> TestEnv {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let launches: Launches = Arc::new(Mutex::new(Vec::new()));
    let rec = launches.clone();
    let update_dir = root.join("config").join("update");
    let env = UpdaterEnv {
        config_path: root.join("config").join("beamtel_config.json"),
        staged_dir: update_dir.join("staged"),
        update_dir,
        program_dir: root.join("prog"),
        system_root: Some(r"C:\Windows".to_string()),
        pid: 4242,
        http: Box::new(FakeHttp {
            routes,
            calls: calls.clone(),
            slow,
        }),
        spawn_helper: Box::new(move |l: &HelperLaunch| {
            rec.lock().push(l.clone());
            if popen_raises {
                Err(std::io::Error::other("boom"))
            } else {
                Ok(())
            }
        }),
        mod_install: ModInstallEnv {
            program_dir: root.join("prog"),
            local_appdata: root.join("lad"),
        },
        log_tap: Some(logs.clone()),
        progress_interval: Duration::from_millis(10),
    };
    TestEnv {
        env,
        logs,
        calls,
        launches,
    }
}

#[derive(Default)]
struct RecGate(Mutex<Vec<&'static str>>);

impl Gate for RecGate {
    fn allow(&self) {
        self.0.lock().push("allow");
    }
    fn deny(&self) {
        self.0.lock().push("deny");
    }
}

fn answer_of(s: &str) -> Answer {
    match s {
        "yes" => Answer::Yes,
        "no" => Answer::No,
        "ok" => Answer::Ok,
        _ => panic!("answer {s}"),
    }
}

fn answer_name(a: Answer) -> &'static str {
    match a {
        Answer::Yes => "yes",
        Answer::No => "no",
        Answer::Ok => "ok",
    }
}

fn sink_with(answers: &Value) -> RecordingSink {
    let sink = RecordingSink::new();
    if let Some(list) = answers.as_array() {
        for a in list {
            sink.push_answer(answer_of(a.as_str().unwrap()));
        }
    }
    sink
}

/// The recorded UI in the generator's shape: message boxes, each progress dialog once
/// (when it opens), and the exit request.
fn events_json(events: &[UiEvent], root: &str) -> Value {
    let mut out = Vec::new();
    let mut dialog_open = false;
    for e in events {
        match e {
            UiEvent::MessageBox(m) => {
                let kind = match m.kind {
                    MessageKind::Error => "error",
                    MessageKind::Warning => "warning",
                    MessageKind::Question => "question",
                    MessageKind::Info => "info",
                };
                let buttons = match m.buttons {
                    Buttons::Ok => "ok",
                    Buttons::YesNo => "yes_no",
                };
                out.push(json!({
                    "event": "message",
                    "kind": kind,
                    "title": m.title,
                    "text": scrub(&m.text, root),
                    "buttons": buttons,
                    "default": answer_name(m.default),
                }));
            }
            UiEvent::Progress(p) => {
                if !dialog_open {
                    dialog_open = true;
                    out.push(json!({
                        "event": "progress",
                        "title": p.title,
                        "text": p.text,
                        "cancellable": p.cancellable,
                    }));
                }
            }
            UiEvent::ProgressClose => dialog_open = false,
            UiEvent::RequestExit => out.push(json!({"event": "exit"})),
            UiEvent::Log(_) | UiEvent::Console(_) => {}
        }
    }
    Value::Array(out)
}

fn logs_json(logs: &[(log::Level, String)], root: &str) -> Value {
    Value::Array(
        logs.iter()
            .map(|(l, m)| json!([level_name(*l), scrub(m, root)]))
            .collect(),
    )
}

fn outcome_json(r: Result<Option<String>, String>, root: &str) -> Value {
    match r {
        Ok(v) => json!({"ok": v.map(|s| scrub(&s, root))}),
        Err(e) => json!({"err": scrub(&e, root)}),
    }
}

fn headers_json(calls: &[Call]) -> Value {
    // urllib stores header names capitalized ("User-agent"); compare that form.
    match calls.first() {
        None => Value::Null,
        Some(c) => {
            let mut h: Vec<(String, String)> = c
                .headers
                .iter()
                .map(|(k, v)| {
                    let mut name = k.to_lowercase();
                    if let Some(first) = name.get_mut(0..1) {
                        first.make_ascii_uppercase();
                    }
                    (name, v.clone())
                })
                .collect();
            h.sort();
            Value::Array(h.into_iter().map(|(k, v)| json!([k, v])).collect())
        }
    }
}

// ---------------------------------------------------------------------------
// Constants and versions
// ---------------------------------------------------------------------------

#[test]
fn constants_match_python() {
    let c = &fixture()["constants"];
    // The fixture's app_version is the Python's at recording time; APP_VERSION moves
    // with every release, so only its shape is checked.
    assert!(updater::parse_version(updater::APP_VERSION).is_some());
    assert_eq!(c["REPO"], updater::REPO);
    assert_eq!(c["LATEST_URL"], updater::LATEST_URL);
    assert_eq!(c["ASSET_NAME"], updater::ASSET_NAME);
    assert_eq!(c["REQUIRED_MEMBERS"], json!(updater::REQUIRED_MEMBERS));
    assert_eq!(c["HELPER_NAME"], updater::HELPER_NAME);
    assert_eq!(c["HELPER_LOG"], updater::HELPER_LOG);
    assert_eq!(c["USER_AGENT"], updater::USER_AGENT.as_str());
    assert_eq!(
        c["CHECK_TIMEOUT_S"].as_f64(),
        Some(updater::CHECK_TIMEOUT_S)
    );
    assert_eq!(
        c["DOWNLOAD_TIMEOUT_S"].as_f64(),
        Some(updater::DOWNLOAD_TIMEOUT_S)
    );
    assert_eq!(c["PENDING_KEY"], updater::PENDING_KEY);
    assert_eq!(c["ENABLED_KEY"], updater::ENABLED_KEY);
    assert_eq!(c["MOD_ZIP"], mod_install::MOD_ZIP);
    assert_eq!(c["MOD_VERSION_MEMBER"], mod_install::MOD_VERSION_MEMBER);
    assert_eq!(c["BNVDA_APP_NAME"], mod_install::BNVDA_APP_NAME);
    assert_eq!(
        c["freeroam_layout_path"],
        mod_install::freeroam_layout_path(Path::new(r"C:\LAD"))
            .to_string_lossy()
            .as_ref()
    );
}

#[test]
fn parse_version_matches_python() {
    for case in beam_testutil::cases(fixture(), "versions") {
        let text = case["text"].as_str().unwrap();
        let got = updater::parse_version(text)
            .map(|v| json!(v.0.iter().map(|n| n.to_string()).collect::<Vec<_>>()));
        assert_eq!(
            got.unwrap_or(Value::Null),
            case["parsed"],
            "parse_version({text:?})"
        );
    }
}

#[test]
fn is_newer_matches_python() {
    for case in beam_testutil::cases(fixture(), "newer") {
        let r = case["remote"].as_str().unwrap();
        let l = case["local"].as_str().unwrap();
        assert_eq!(
            Value::Bool(updater::is_newer(r, l)),
            case["newer"],
            "is_newer({r:?}, {l:?})"
        );
    }
}

// ---------------------------------------------------------------------------
// check_latest
// ---------------------------------------------------------------------------

#[test]
fn check_latest_matches_python() {
    for case in beam_testutil::cases(fixture(), "check_latest") {
        let name = case["name"].as_str().unwrap();
        let t = tempdir();
        let routes = HashMap::from([(updater::LATEST_URL.to_string(), case["response"].clone())]);
        let te = test_env(t.path(), routes, false, false);
        let result = match updater::check_latest(&te.env, 8.0) {
            Ok(None) => json!({"ok": null}),
            Ok(Some(rel)) => json!({"ok": {
                "tag": rel.tag.py_repr(),
                "url": rel.url.py_repr(),
                "size": rel.size.py_repr(),
                "version": match rel.version() {
                    Ok(v) => json!({"ok": v}),
                    Err(e) => json!({"err": e}),
                },
            }}),
            Err(e) => json!({"err": e}),
        };
        assert_eq!(result, case["result"], "check_latest case {name}");
        assert_eq!(
            logs_json(&te.logs.lock(), ""),
            case["logs"],
            "check_latest logs, case {name}"
        );
        let calls = te.calls.lock();
        assert_eq!(
            headers_json(&calls),
            case["request_headers"],
            "headers, case {name}"
        );
        assert_eq!(calls[0].url, updater::LATEST_URL);
        assert_eq!(
            Some(calls[0].timeout),
            case["timeout"].as_f64(),
            "timeout, case {name}"
        );
    }
}

// ---------------------------------------------------------------------------
// download / stage
// ---------------------------------------------------------------------------

#[test]
fn stage_matches_python() {
    for case in beam_testutil::cases(fixture(), "stage") {
        let name = case["name"].as_str().unwrap();
        let t = tempdir();
        let root = root_str(&t);
        let zip_path = t.path().join("bundle.zip");
        std::fs::write(&zip_path, b64d(case["zip"].as_str().unwrap())).unwrap();
        let staged = t.path().join("staged");
        let res = updater::stage(&zip_path, &staged);
        assert_eq!(
            outcome_json(res, &root),
            case["result"],
            "stage case {name}"
        );
        assert_eq!(tree(&staged), case["tree"], "staged tree, case {name}");
    }
}

#[test]
fn download_matches_python() {
    for case in beam_testutil::cases(fixture(), "download") {
        let name = case["name"].as_str().unwrap();
        let t = tempdir();
        let root = root_str(&t);
        let url = case["url"].as_str().unwrap();
        let key = url.trim().trim_matches(['<', '>']).replace("URL:", "");
        let routes = HashMap::from([(key, case["response"].clone())]);
        let te = test_env(t.path(), routes, false, false);
        let dest = t.path().join("update").join(updater::ASSET_NAME);
        let progress_log: Mutex<Vec<Value>> = Mutex::new(Vec::new());
        let progress = |d: u64, total: f64| progress_log.lock().push(json!([d, total]));
        let cancel = case["cancel"].as_bool().unwrap();
        let cancelled = || cancel;
        let res = updater::download(
            &te.env,
            &PyValue::from(url),
            &dest,
            Some(&progress),
            Some(&cancelled),
            updater::DOWNLOAD_TIMEOUT_S,
        );
        assert_eq!(
            outcome_json(res, &root),
            case["result"],
            "download case {name}"
        );
        let want_progress: Vec<Value> = case["progress"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| json!([p[0].as_u64().unwrap(), p[1].as_f64().unwrap()]))
            .collect();
        assert_eq!(*progress_log.lock(), want_progress, "progress, case {name}");
        assert_eq!(
            Value::Bool(dest.is_file()),
            case["dest_exists"],
            "dest, case {name}"
        );
        let mut part = dest.as_os_str().to_os_string();
        part.push(".part");
        assert_eq!(
            Value::Bool(PathBuf::from(part).is_file()),
            case["part_exists"],
            ".part, case {name}"
        );
        let calls = te.calls.lock();
        assert_eq!(
            headers_json(&calls),
            case["request_headers"],
            "headers, case {name}"
        );
        if let Some(c) = calls.first() {
            assert_eq!(
                Some(c.timeout),
                case["timeout"].as_f64(),
                "timeout, case {name}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The helper
// ---------------------------------------------------------------------------

/// The one deliberate change to the Python's helper (python-bugs.md 67): its three
/// `set` lines with the paths written in become these, reading the environment.
const ENV_PATHS_BLOCK: [&str; 9] = [
    "rem The three paths arrive in the environment, not in this file: a path can hold any",
    "rem character (a non-ASCII user name, a percent sign), and a batch file is read in the",
    "rem console code page. Cleared at once so the restarted program does not inherit them.",
    r#"set "PROG=%BEAM_UPDATE_PROG%""#,
    r#"set "STAGED=%BEAM_UPDATE_STAGED%""#,
    r#"set "LOGFILE=%BEAM_UPDATE_LOG%""#,
    r#"set "BEAM_UPDATE_PROG=""#,
    r#"set "BEAM_UPDATE_STAGED=""#,
    r#"set "BEAM_UPDATE_LOG=""#,
];

/// The Python's helper text with its path lines replaced by [`ENV_PATHS_BLOCK`]; the
/// rest must match byte for byte.
fn with_env_paths(python: &str) -> String {
    let nl = if python.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let re = regex::Regex::new(
        r#"set "PROG=[^"\r\n]*"\r?\nset "STAGED=[^"\r\n]*"\r?\nset "LOGFILE=[^"\r\n]*""#,
    )
    .unwrap();
    assert!(re.is_match(python), "the Python helper's path lines moved");
    re.replace(python, regex::NoExpand(&ENV_PATHS_BLOCK.join(nl)))
        .into_owned()
}

#[test]
fn helper_template_is_the_python_one_but_for_the_paths() {
    let h = &fixture()["helper"];
    let body = updater::helper_body(4242);
    assert_eq!(body, with_env_paths(h["template_body"].as_str().unwrap()));
    assert!(body.is_ascii());
}

#[test]
fn write_helper_writes_python_bytes_but_for_the_paths() {
    let h = &fixture()["helper"]["written"];
    let t = tempdir();
    let root = root_str(&t);
    let update_dir = t.path().join("update");
    let path = updater::write_helper(&update_dir, 4242).unwrap();
    assert_eq!(
        scrub(&path.to_string_lossy(), &root),
        h["path"].as_str().unwrap()
    );
    let bytes = std::fs::read(&path).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(
        scrub(&text, &root),
        with_env_paths(h["bytes"].as_str().unwrap())
    );
}

/// python-bugs.md 67: a non-ASCII (or percent-sign) path used to make the helper
/// unwritable after the launch had been denied. Now the file is ASCII and the paths reach
/// the helper intact through its environment.
#[test]
fn non_ascii_paths_reach_the_helper() {
    let t = tempdir();
    let base = t.path().join("Zoë 漢字 50%");
    std::fs::create_dir_all(&base).unwrap();
    let mut te = test_env(&base, HashMap::new(), false, false);
    te.env.program_dir = base.join("prøg");
    let staged = te.env.update_dir.join("staged");
    let out = updater::apply_and_restart(&te.env, &staged).unwrap();
    assert_eq!(out, None);
    let text = std::fs::read(te.env.update_dir.join(updater::HELPER_NAME)).unwrap();
    assert!(text.is_ascii(), "the helper file is ASCII");
    let launches = te.launches.lock();
    assert_eq!(launches.len(), 1);
    let env: HashMap<&str, &str> = launches[0]
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        env["BEAM_UPDATE_PROG"],
        std::path::absolute(&te.env.program_dir)
            .unwrap()
            .to_string_lossy()
    );
    assert_eq!(
        env["BEAM_UPDATE_STAGED"],
        std::path::absolute(&staged).unwrap().to_string_lossy()
    );
    assert!(env["BEAM_UPDATE_LOG"].ends_with(r"update\apply_update.log"));
    assert!(env["BEAM_UPDATE_LOG"].contains("Zoë 漢字 50%"));
}

#[test]
fn helper_encoding_matches_python() {
    for case in fixture()["helper"]["encode"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        match updater::helper_file_bytes(text) {
            Ok(b) => assert_eq!(b64e(&b), case["ok"].as_str().unwrap(), "encode {text:?}"),
            Err(e) => assert_eq!(e, case["err"].as_str().unwrap(), "encode {text:?}"),
        }
    }
}

#[test]
fn list2cmdline_matches_python() {
    for case in fixture()["helper"]["list2cmdline"].as_array().unwrap() {
        let seq: Vec<&str> = case["seq"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            updater::list2cmdline(&seq),
            case["line"].as_str().unwrap(),
            "{seq:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Mod installer
// ---------------------------------------------------------------------------

#[test]
fn mod_zip_version_matches_python() {
    for case in beam_testutil::cases(fixture(), "mod_version") {
        let name = case["name"].as_str().unwrap();
        let t = tempdir();
        let p = t.path().join("m.zip");
        std::fs::write(&p, b64d(case["zip"].as_str().unwrap())).unwrap();
        let got = mod_install::mod_zip_version(&p)
            .map(|(v, raw)| json!([v.0.iter().map(|n| n.to_string()).collect::<Vec<_>>(), raw]));
        assert_eq!(
            got.unwrap_or(Value::Null),
            case["result"],
            "mod_zip_version case {name}"
        );
    }
}

#[test]
fn install_mod_matches_python() {
    for case in beam_testutil::cases(fixture(), "mod_install") {
        let name = case["name"].as_str().unwrap();
        let t = tempdir();
        let root = root_str(&t);
        let prog = t.path().join("prog");
        let lad = t.path().join("lad");
        std::fs::create_dir_all(&prog).unwrap();
        std::fs::create_dir_all(&lad).unwrap();
        if let Some(src) = case["src"].as_str() {
            std::fs::write(prog.join(mod_install::MOD_ZIP), b64d(src)).unwrap();
        }
        let mods = lad
            .join("BeamNG")
            .join("BeamNG.drive")
            .join("current")
            .join("mods");
        if case["mods_exists"].as_bool().unwrap() {
            std::fs::create_dir_all(&mods).unwrap();
            match case["dst"].as_str() {
                Some("DIR") => std::fs::create_dir_all(mods.join(mod_install::MOD_ZIP)).unwrap(),
                Some(b) => std::fs::write(mods.join(mod_install::MOD_ZIP), b64d(b)).unwrap(),
                None => {}
            }
        }
        let layout_path = mod_install::freeroam_layout_path(&lad);
        if let Some(layout) = case["layout"].as_str() {
            std::fs::create_dir_all(layout_path.parent().unwrap()).unwrap();
            std::fs::write(&layout_path, b64d(layout)).unwrap();
        }
        let sink = sink_with(&case["answers"]);
        let result = mod_install::install_mod_interactive_in(
            &sink,
            &ModInstallEnv {
                program_dir: prog.clone(),
                local_appdata: lad.clone(),
            },
        );
        assert_eq!(
            Value::Bool(result),
            case["result"],
            "install result, case {name}"
        );
        assert_eq!(
            events_json(&sink.events(), &root),
            case["events"],
            "install dialogs, case {name}"
        );
        assert_eq!(tree(&mods), case["mods_tree"], "mods dir, case {name}");
        if !case["layout"].is_null() {
            let lt = match tree(layout_path.parent().unwrap()) {
                Value::Object(m) => {
                    Value::Object(m.into_iter().map(|(k, v)| (scrub(&k, &root), v)).collect())
                }
                other => other,
            };
            assert_eq!(lt, case["layout_tree"], "layout dir, case {name}");
        }
    }
}

// ---------------------------------------------------------------------------
// The flow
// ---------------------------------------------------------------------------

#[test]
fn startup_flow_matches_python() {
    for case in beam_testutil::cases(fixture(), "flow") {
        let name = case["name"].as_str().unwrap();
        let spec = &case["case"];
        let t = tempdir();
        let root = root_str(&t);
        let base = t.path();
        std::fs::create_dir_all(base.join("config")).unwrap();
        std::fs::create_dir_all(base.join("prog")).unwrap();
        std::fs::create_dir_all(base.join("lad")).unwrap();
        if let Some(cfg) = spec.get("config").and_then(Value::as_object)
            && !cfg.is_empty()
        {
            std::fs::write(
                base.join("config").join("beamtel_config.json"),
                serde_json::to_string(cfg).unwrap(),
            )
            .unwrap();
        }
        let mods = base
            .join("lad")
            .join("BeamNG")
            .join("BeamNG.drive")
            .join("current")
            .join("mods");
        if spec.get("mods").and_then(Value::as_str) == Some("ready") {
            // The generator's SRC_NEW: the same bytes the mod-install fixture calls "fresh".
            let src = beam_testutil::cases(fixture(), "mod_install")
                .iter()
                .find(|c| c["name"] == "fresh")
                .unwrap()["src"]
                .as_str()
                .unwrap()
                .to_string();
            std::fs::write(base.join("prog").join(mod_install::MOD_ZIP), b64d(&src)).unwrap();
            std::fs::create_dir_all(&mods).unwrap();
        }
        let mut routes = HashMap::new();
        if let Some(c) = spec.get("check") {
            routes.insert(updater::LATEST_URL.to_string(), c.clone());
        }
        if let Some(d) = spec.get("download") {
            routes.insert("https://example.invalid/b.zip".to_string(), d.clone());
        }
        let cancel = spec.get("cancel").and_then(Value::as_bool).unwrap_or(false);
        let popen_raises = spec.get("popen").and_then(Value::as_str) == Some("raise");
        let te = test_env(base, routes, cancel, popen_raises);
        let sink = sink_with(spec.get("answers").unwrap_or(&Value::Null));
        if cancel {
            sink.cancel_progress
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let gate = RecGate::default();
        let manual = spec.get("manual").and_then(Value::as_bool).unwrap_or(false);

        updater::run_startup_flow_with(&sink, &gate, manual, &te.env);

        assert_eq!(
            events_json(&sink.events(), &root),
            case["events"],
            "dialogs, flow {name}"
        );
        assert_eq!(
            logs_json(&te.logs.lock(), &root),
            case["logs"],
            "logs, flow {name}"
        );
        assert_eq!(json!(*gate.0.lock()), case["gate"], "gate, flow {name}");

        let popen: Vec<Value> = te
            .launches
            .lock()
            .iter()
            .map(|l| {
                let mut args = vec![scrub(&l.program.to_string_lossy(), &root)];
                args.extend(l.args.iter().map(|a| scrub(a, &root)));
                json!({
                    "args": args,
                    "line": scrub(&l.command_line, &root),
                    "cwd": scrub(&l.cwd.to_string_lossy(), &root),
                    "creationflags": l.creation_flags,
                })
            })
            .collect();
        let want_popen: Vec<Value> = case["popen"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                json!({
                    "args": p["args"],
                    "line": p["line"],
                    "cwd": p["cwd"],
                    "creationflags": p["creationflags"],
                })
            })
            .collect();
        assert_eq!(popen, want_popen, "helper launch, flow {name}");

        let cfg: Value = serde_json::from_str(
            &std::fs::read_to_string(&te.env.config_path).expect("config written"),
        )
        .unwrap();
        assert_eq!(
            cfg.get("pending_update_version")
                .cloned()
                .unwrap_or(Value::Null),
            case["pending_after"],
            "pending flag, flow {name}"
        );
        let helper_path = te.env.update_dir.join(updater::HELPER_NAME);
        let helper = if helper_path.is_file() {
            Value::String(scrub(
                &String::from_utf8(std::fs::read(&helper_path).unwrap()).unwrap(),
                &root,
            ))
        } else {
            Value::Null
        };
        let want_helper = match case["helper"].as_str() {
            Some(text) => Value::String(with_env_paths(text)),
            None => Value::Null,
        };
        assert_eq!(helper, want_helper, "helper file, flow {name}");
        assert_eq!(
            tree(&te.env.staged_dir),
            case["staged_tree"],
            "staged, flow {name}"
        );
        assert_eq!(
            Value::Bool(te.env.update_dir.join(updater::ASSET_NAME).is_file()),
            case["zip_exists"],
            "downloaded zip, flow {name}"
        );
        assert_eq!(tree(&mods), case["mods_tree"], "mods dir, flow {name}");
    }
}

/// The real launcher is never run by a test; this checks the launch description only.
#[test]
fn helper_launch_falls_back_to_c_windows() {
    let t = tempdir();
    let mut te = test_env(t.path(), HashMap::new(), false, false);
    te.env.system_root = Some(String::new());
    let l = updater::helper_launch(&te.env, Path::new(r"C:\x y\apply_update.cmd"));
    assert_eq!(l.program, PathBuf::from(r"C:\Windows\System32\cmd.exe"));
    assert_eq!(
        l.command_line,
        r#"C:\Windows\System32\cmd.exe /d /c "C:\x y\apply_update.cmd""#
    );
    assert_eq!(l.creation_flags, 0x0800_0200);
    te.env.system_root = None;
    let l = updater::helper_launch(&te.env, Path::new(r"C:\h.cmd"));
    assert_eq!(l.program, PathBuf::from(r"C:\Windows\System32\cmd.exe"));
    assert_eq!(l.args, vec!["/d", "/c", r"C:\h.cmd"]);
    assert_eq!(l.cwd, te.env.update_dir);
}

/// A body larger than one 64 KiB chunk reports progress per chunk, as `resp.read(65536)`.
#[test]
fn download_reports_each_chunk() {
    let t = tempdir();
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let routes = HashMap::from([(
        "https://example.invalid/big".to_string(),
        json!({"kind": "body", "body": b64e(&big)}),
    )]);
    let te = test_env(t.path(), routes, false, false);
    let dest = t.path().join("big.zip");
    let seen: Mutex<Vec<(u64, f64)>> = Mutex::new(Vec::new());
    let progress = |d: u64, total: f64| seen.lock().push((d, total));
    let cancelled = || false;
    let r = updater::download(
        &te.env,
        &PyValue::from("https://example.invalid/big"),
        &dest,
        Some(&progress),
        Some(&cancelled),
        30.0,
    )
    .unwrap();
    assert_eq!(
        r.as_deref(),
        Some("The downloaded file is not a valid BEAM update (File is not a zip file).")
    );
    assert_eq!(
        *seen.lock(),
        vec![
            (65536, 200_000.0),
            (131_072, 200_000.0),
            (196_608, 200_000.0),
            (200_000, 200_000.0)
        ]
    );
    assert!(!dest.exists());
}
