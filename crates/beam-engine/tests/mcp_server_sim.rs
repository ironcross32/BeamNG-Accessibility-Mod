//! `diagnostic/mcp_server_sim.py`, ported: the MCP server stood up on a spare port with
//! stub deps and driven over real HTTP, plus the camera chunks against
//! `fixtures/data/mcp_camera.json` (written by `fixtures/gen/mcp_camera.py`).
//!
//! The things a client depends on and a hand-rolled transport gets wrong are asserted
//! over the wire: 202 with an empty body for a notification (otherwise the client hangs
//! on the handshake), 405 for `GET /mcp`, a plain application/json body, protocol-version
//! echo, and `isError` rather than a JSON-RPC error when a tool fails. The console tap is
//! driven directly, since EXEC correlation rests on ordering plus the EXECEND sentinel,
//! including that LOG records are copied but NOT consumed.
//!
//! The sim's section 13 and 19 grep `beamtel.py`, `configurator.py` and `config_ui.py`
//! for Python wiring and test the Python `secretstore`; only the checks with a Rust
//! counterpart are kept (13s, 13u, 19a). Wiring is I1's and U2's to verify.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use beam_engine::mcp_server::{self as ms, Deps, ToolFail};
use beam_engine::speech_out::{SpeechLog, TapEntry};
use beam_road::pyjson::{self, Value as PyJson};
use beam_testutil::{load, repo_root, s};
use indexmap::IndexMap;
use parking_lot::Mutex;
use serde_json::{Value, json};

// ---- camera chunks against the Python -------------------------------------------

fn outcome(r: Result<String, ToolFail>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(ToolFail::Tool(m)) => json!({"err": "ToolError", "msg": m}),
        Err(ToolFail::Py(t, m)) => json!({"err": t, "msg": m}),
    }
}

#[test]
fn camera_chunks_match_python() {
    let fx = load("mcp_camera");
    assert_eq!(ms::CAM_READ_LUA, s(&fx["read_lua"]), "_CAM_READ_LUA");
    assert_eq!(ms::CAM_LIST_LUA, s(&fx["list_lua"]), "_CAM_LIST_LUA");
    let mut failures = Vec::new();
    for case in fx["place"].as_array().unwrap() {
        let args = pyjson::loads(&case["args"].to_string()).unwrap();
        let got = outcome(ms::cam_place_chunk(&args));
        if got != case["result"] {
            failures.push(format!(
                "args {}:\n  got  {got}\n  want {}",
                case["args"], case["result"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---- the HTTP sim -----------------------------------------------------------------

struct Rec {
    spoken: Mutex<Vec<String>>,
    sent_console: Mutex<Vec<String>>,
    sent_udp: Mutex<Vec<(u16, String)>>,
    captures: Mutex<Vec<(bool, f64, bool)>>,
    capture_meta: Mutex<Value>,
    road: Mutex<Vec<String>>,
    assistant: Mutex<Vec<(String, Value)>>,
}

const FAKE_PNG_TAIL: usize = 512;

fn fake_png() -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.extend(std::iter::repeat_n(b'x', FAKE_PNG_TAIL));
    v
}

fn good_meta() -> Value {
    json!({"focused_game": true, "game_window_found": true,
           "region": {"left": 0, "top": 0, "width": 1920, "height": 1080}})
}

fn deps(rec: &Arc<Rec>) -> Deps {
    let (r1, r2, r3, r4, r5, r6, r7) = (
        rec.clone(),
        rec.clone(),
        rec.clone(),
        rec.clone(),
        rec.clone(),
        rec.clone(),
        rec.clone(),
    );
    let mut f9_help = IndexMap::new();
    f9_help.insert(("s", false, false, false), "Speak speed");
    Deps {
        say_fn: Arc::new(move |t, _i| r1.spoken.lock().push(t.to_string())),
        get_speech_log_fn: Arc::new(|_, _, _, _, _| SpeechLog {
            entries: vec![TapEntry {
                seq: 1,
                t: 0.0,
                text: "hello".into(),
                source: "test".into(),
                interrupt: false,
                excluded: false,
                spoken: true,
                reason: None,
            }],
            next_seq: 1,
            dropped: false,
        }),
        snapshot_state_fn: Arc::new(
            |_| json!({"liveness": {"world_active": false, "seconds_since_telemetry": 999.0}}),
        ),
        load_config_fn: Arc::new(|| {
            let Value::Object(m) = json!({
                "units": "imperial",
                "mcp_server_enabled": true,
                // A sealed key and an unset one, so section 18 can assert both answers.
                "ai_describer_api_key": "dpapi:v1:c2VhbGVk",
                "ai_describer_openai_api_key": "",
            }) else {
                unreachable!()
            };
            m
        }),
        // Nothing ever answers, which exercises the timeout path.
        send_console_command_fn: Arc::new(move |m| r2.sent_console.lock().push(m.to_string())),
        send_udp_fn: Arc::new(move |p, m| {
            r3.sent_udp.lock().push((p, m.to_string()));
            Ok(())
        }),
        press_command_fn: Arc::new(|_, _, _, _| Ok(true)),
        f9_help,
        world_active_fn: Arc::new(|| false),
        capture_png_fn: Arc::new(move |hide, settle, focus| {
            r4.captures.lock().push((hide, settle, focus));
            Ok((fake_png(), r5.capture_meta.lock().clone()))
        }),
        road_diagnostic_fn: Arc::new(move |action, _, _, _, _| {
            r6.road.lock().push(action.to_string());
            Ok(json!({"active": false}))
        }),
        assistant_diagnostic_fn: Arc::new(move |action, kwargs| {
            r7.assistant
                .lock()
                .push((action.to_string(), kwargs.clone()));
            Ok(json!({"recording": false}))
        }),
        version: "test".into(),
    }
}

struct Http {
    base: String,
    agent: ureq::Agent,
}

struct Reply {
    status: u16,
    content_type: String,
    session: Option<String>,
    body: Vec<u8>,
}

impl Http {
    fn reply(mut r: ureq::http::Response<ureq::Body>) -> Reply {
        let h = |k: &str| {
            r.headers()
                .get(k)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let content_type = h("content-type").unwrap_or_default();
        let session = h("mcp-session-id");
        Reply {
            status: r.status().as_u16(),
            content_type,
            session,
            body: r.body_mut().read_to_vec().unwrap(),
        }
    }

    fn post(&self, payload: &Value) -> Reply {
        let r = self
            .agent
            .post(format!("{}/mcp", self.base))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .send(payload.to_string())
            .unwrap();
        Self::reply(r)
    }

    fn json(&self, payload: &Value) -> Value {
        serde_json::from_slice(&self.post(payload).body).unwrap()
    }

    fn call(&self, name: &str, args: Value) -> Value {
        self.json(&json!({"jsonrpc": "2.0", "id": 99, "method": "tools/call",
                          "params": {"name": name, "arguments": args}}))["result"]
            .clone()
    }
}

fn text(r: &Value) -> String {
    r["content"][0]["text"].as_str().unwrap_or("").to_string()
}

fn is_error(r: &Value) -> bool {
    r["isError"].as_bool().unwrap_or(false)
}

/// The Python's free camera forward (core/camera.lua:1105): yaw measured as
/// atan2(f.x, f.y), pitch positive-DOWN.
fn game_forward(yaw_set_deg: f64, pitch_down_deg: f64) -> [f64; 3] {
    let (y, p) = (yaw_set_deg.to_radians(), pitch_down_deg.to_radians());
    let cp = p.cos();
    [y.sin() * cp, y.cos() * cp, -p.sin()]
}

/// cameraInfo.lua's readback: yaw atan2(-f.x, -f.y), pitch positive-UP.
fn mod_readback(f: [f64; 3]) -> (f64, f64) {
    let l = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
    (
        (-f[0]).atan2(-f[1]).to_degrees().rem_euclid(360.0),
        (f[2] / l).asin().to_degrees(),
    )
}

#[test]
fn mcp_server_over_http() {
    let mut failures: Vec<String> = Vec::new();
    let mut check = |label: &str, ok: bool, detail: String| {
        if !ok {
            failures.push(format!("{label}  -- {detail}"));
        }
    };

    let rec = Arc::new(Rec {
        spoken: Mutex::new(Vec::new()),
        sent_console: Mutex::new(Vec::new()),
        sent_udp: Mutex::new(Vec::new()),
        captures: Mutex::new(Vec::new()),
        capture_meta: Mutex::new(good_meta()),
        road: Mutex::new(Vec::new()),
        assistant: Mutex::new(Vec::new()),
    });
    ms::reset_for_tests();
    ms::init(deps(&rec));
    let server = ms::start_on(ms::HOST, 0);
    let addr = server.addr.expect("server bound");
    let http = Http {
        base: format!("http://{addr}"),
        agent: ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(90)))
            .build()
            .into(),
    };

    // ---- 1. initialize
    let r = http.post(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "smoke", "version": "0"}}}));
    let init: Value = serde_json::from_slice(&r.body).unwrap();
    check(
        "1a initialize returns 200",
        r.status == 200,
        r.status.to_string(),
    );
    check(
        "1b content-type is application/json",
        r.content_type.contains("application/json"),
        r.content_type.clone(),
    );
    check(
        "1c protocolVersion echoed",
        init["result"]["protocolVersion"] == "2025-06-18",
        init.to_string(),
    );
    check(
        "1d tools capability advertised",
        init["result"]["capabilities"].get("tools").is_some(),
        init.to_string(),
    );
    check(
        "1e serverInfo present",
        init["result"]["serverInfo"]["name"] == "beamtel",
        init.to_string(),
    );
    check(
        "1f no Mcp-Session-Id issued",
        r.session.is_none(),
        format!("{:?}", r.session),
    );
    check(
        "1g instructions present",
        init["result"]["instructions"]
            .as_str()
            .is_some_and(|t| !t.is_empty()),
        String::new(),
    );
    let v = http.json(&json!({"jsonrpc": "2.0", "id": 2, "method": "initialize",
                              "params": {"protocolVersion": "1999-01-01"}}));
    check(
        "1h unknown version falls back",
        v["result"]["protocolVersion"] == ms::FALLBACK_PROTOCOL,
        v.to_string(),
    );

    // ---- 2. notifications
    let r = http.post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    check(
        "2a notification returns 202",
        r.status == 202,
        r.status.to_string(),
    );
    check(
        "2b notification body is empty",
        r.body.is_empty(),
        String::from_utf8_lossy(&r.body).into_owned(),
    );

    // ---- 3. tools/list
    let tools =
        http.json(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}))["result"]["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default();
    check(
        "3a tools/list returns tools",
        tools.len() >= 10,
        tools.len().to_string(),
    );
    let names: HashSet<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expect in [
        "health",
        "lua_exec",
        "speech_log",
        "speak",
        "press_command",
        "diag",
        "camera_control",
        "screenshot",
        "road_diagnostic",
        "assistant_diagnostic",
    ] {
        check(
            &format!("3b tools/list contains {expect}"),
            names.contains(expect),
            String::new(),
        );
    }
    check(
        "3c every tool has an inputSchema object",
        tools.iter().all(|t| t["inputSchema"]["type"] == "object"),
        String::new(),
    );
    check(
        "3d every tool has a description",
        tools
            .iter()
            .all(|t| t["description"].as_str().is_some_and(|d| !d.is_empty())),
        String::new(),
    );

    // ---- 4. ping
    let v = http.json(&json!({"jsonrpc": "2.0", "id": 4, "method": "ping"}));
    check(
        "4a ping returns empty result",
        v["result"] == json!({}),
        v.to_string(),
    );

    // ---- 5. GET / DELETE / health
    let r = Http::reply(http.agent.get(format!("{}/mcp", http.base)).call().unwrap());
    check(
        "5a GET /mcp returns 405",
        r.status == 405,
        r.status.to_string(),
    );
    let r = Http::reply(
        http.agent
            .delete(format!("{}/mcp", http.base))
            .call()
            .unwrap(),
    );
    check(
        "5b DELETE /mcp returns 200",
        r.status == 200,
        r.status.to_string(),
    );
    let r = Http::reply(
        http.agent
            .get(format!("{}/health", http.base))
            .call()
            .unwrap(),
    );
    check("5c /health responds", r.status == 200, r.status.to_string());

    // ---- 6. unknown method
    let v = http.json(&json!({"jsonrpc": "2.0", "id": 6, "method": "resources/list"}));
    check(
        "6a unknown method is -32601",
        v["error"]["code"] == -32601,
        v.to_string(),
    );

    // ---- 7. tools that work with no game
    let r = http.call("list_commands", json!({}));
    check(
        "7a list_commands works with no game",
        !is_error(&r),
        r.to_string(),
    );
    check(
        "7b list_commands returned the F9 entry",
        text(&r).contains("Speak speed"),
        r.to_string(),
    );
    let r = http.call("speak", json!({"text": "test message"}));
    let spoken = rec.spoken.lock().clone();
    check(
        "7c speak works",
        !is_error(&r) && spoken == ["test message"],
        format!("{spoken:?}"),
    );
    let r = http.call("speech_log", json!({}));
    check(
        "7d speech_log works",
        !is_error(&r) && text(&r).contains("hello"),
        r.to_string(),
    );
    let r = http.call("get_state", json!({}));
    check("7e get_state works", !is_error(&r), r.to_string());
    let r = http.call("get_config", json!({}));
    check("7f get_config works", !is_error(&r), r.to_string());
    let r = http.call("road_diagnostic", json!({"action": "status"}));
    check(
        "7f2 road diagnostic status reaches its injected backend",
        !is_error(&r) && rec.road.lock().last().is_some_and(|a| a == "status"),
        r.to_string(),
    );
    let r = http.call(
        "assistant_diagnostic",
        json!({"action": "read", "since_seq": 12, "limit": 5}),
    );
    let last = rec.assistant.lock().last().cloned();
    check(
        "7f3 assistant diagnostic cursor reaches its backend",
        !is_error(&r)
            && last
                .as_ref()
                .is_some_and(|(_, k)| k["since_seq"] == 12 && k["limit"] == 5),
        format!("{r} {last:?}"),
    );
    let r = http.call("assistant_diagnostic", json!({"action": "start"}));
    check(
        "7f4 assistant recording requires a live world",
        is_error(&r),
        r.to_string(),
    );
    http.call(
        "send_command",
        json!({"module": "terrainScanner", "payload": "SCAN"}),
    );
    let sent = rec.sent_udp.lock().clone();
    check(
        "7g send_command routes to the right port",
        sent.contains(&(4472, "SCAN".to_string())),
        format!("{sent:?}"),
    );
    let r = http.call("send_command", json!({"module": "nope", "payload": "X"}));
    check(
        "7h unknown module is an isError, not a crash",
        is_error(&r) && text(&r).contains("unknown module"),
        r.to_string(),
    );

    // ---- 8. the no-game timeout path: nothing ever replies on 4466
    let t0 = Instant::now();
    let r = http.call("lua_exec", json!({"code": "return 1", "timeout_s": 2.0}));
    let elapsed = t0.elapsed().as_secs_f64();
    check(
        "8a lua_exec with no game is an isError",
        is_error(&r),
        r.to_string(),
    );
    check(
        "8b ... names the port and points at health",
        text(&r).contains("4466") && text(&r).contains("health"),
        text(&r),
    );
    check(
        "8c ... returns promptly rather than hanging",
        elapsed < 8.0,
        format!("{elapsed:.1}s"),
    );
    let t0 = Instant::now();
    let r2 = http.call("lua_exec", json!({"code": "return 2", "timeout_s": 2.0}));
    check(
        "8d exec lock was released (second call behaves the same)",
        is_error(&r2) && !text(&r2).contains("busy"),
        text(&r2),
    );
    check(
        "8e ... and is still prompt",
        t0.elapsed().as_secs_f64() < 8.0,
        String::new(),
    );
    let sent_console = rec.sent_console.lock().clone();
    check(
        "8x the EXEC went to the console sender",
        sent_console
            .iter()
            .any(|m| m.starts_with("EXEC|0|return 1")),
        format!("{sent_console:?}"),
    );
    let r = http.call("health", json!({}));
    check("8f health works with no game", !is_error(&r), r.to_string());
    check(
        "8g health reports console unresponsive",
        text(&r)
            .to_lowercase()
            .contains("\"console_responsive\": false"),
        text(&r).chars().take(400).collect(),
    );

    // ---- 9. unknown tool
    let v = http.json(&json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
                              "params": {"name": "nosuchtool", "arguments": {}}}));
    check(
        "9a unknown tool is -32602",
        v["error"]["code"] == -32602,
        v.to_string(),
    );

    // ---- 10. batch
    let v = http.json(&json!([{"jsonrpc": "2.0", "id": 10, "method": "ping"},
                              {"jsonrpc": "2.0", "id": 11, "method": "ping"}]));
    check(
        "10a batch returns an array of responses",
        v.as_array().is_some_and(|a| a.len() == 2),
        v.to_string(),
    );

    // ---- 10b. the DNS-rebinding guard (python-bugs.md 66): a look-alike origin is
    // refused over real HTTP, the loopback one still answered.
    let with_origin = |origin: &str| {
        let r = http
            .agent
            .post(format!("{}/mcp", http.base))
            .header("Content-Type", "application/json")
            .header("Origin", origin)
            .send(json!({"jsonrpc": "2.0", "id": 12, "method": "ping"}).to_string())
            .unwrap();
        r.status().as_u16()
    };
    let s = with_origin("http://localhost.example.com");
    check(
        "10b a look-alike origin is refused",
        s == 403,
        s.to_string(),
    );
    let s = with_origin("http://127.0.0.1:4481");
    check(
        "10c the loopback origin is answered",
        s == 200,
        s.to_string(),
    );

    // ---- 11. console tap correlation
    let ge = PyJson::from("ge");
    let feeder = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(400));
        [
            ("RESP", ms::console_tap("RESP|ok")),
            ("OUTVAL", ms::console_tap("OUT|= 42")),
            ("OUTLINE", ms::console_tap("OUT|printed line")),
            ("LOG", ms::console_tap("LOG|12:00|I|Test|a log line")),
            ("EXECEND", ms::console_tap("EXECEND")),
        ]
    });
    let res = ms::exec_console(&ge, "return 42", &PyJson::Float(5.0), false, true).unwrap();
    let tags: std::collections::HashMap<_, _> = feeder.join().unwrap().into_iter().collect();
    check(
        "11a exec assembled the return value",
        res.get("result").is_some_and(|v| v.eq_str("42")),
        format!("{res:?}"),
    );
    check(
        "11b exec assembled the print output",
        res.get("output").is_some_and(|v| v.eq_str("printed line")),
        format!("{res:?}"),
    );
    check(
        "11c exec reports ok",
        res.get("ok").is_some_and(PyJson::truthy)
            && !res.get("timed_out").is_some_and(PyJson::truthy),
        format!("{res:?}"),
    );
    check(
        "11d RESP/OUT/EXECEND are consumed (kept out of the GUI)",
        tags["RESP"] && tags["OUTVAL"] && tags["EXECEND"],
        format!("{tags:?}"),
    );
    check(
        "11e LOG is NOT consumed (still reaches the GUI)",
        !tags["LOG"],
        format!("{tags:?}"),
    );

    let feeder = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(400));
        ms::console_tap("RESP|ok");
        ms::console_tap(&format!("OUT|= {}", "A".repeat(ms::MAX_DATAGRAM_CONTENT)));
        ms::console_tap(&format!("OUT|= {}", "B".repeat(10)));
        ms::console_tap("EXECEND");
    });
    let res = ms::exec_console(&ge, "x", &PyJson::Float(5.0), false, true).unwrap();
    feeder.join().unwrap();
    let want = format!("{}{}", "A".repeat(ms::MAX_DATAGRAM_CONTENT), "B".repeat(10));
    check(
        "11f chunked value concatenates",
        res.get("result").is_some_and(|v| v.eq_str(&want)),
        format!("{:?}", res.get("result").map(|v| v.py_str().len())),
    );

    let feeder = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(400));
        ms::console_tap("RESP|error|attempt to index a nil value");
        ms::console_tap("EXECEND");
    });
    let res = ms::exec_console(&ge, "bad", &PyJson::Float(5.0), false, true).unwrap();
    feeder.join().unwrap();
    check(
        "11g a Lua error surfaces as ok=false with the message",
        !res.get("ok").is_some_and(PyJson::truthy)
            && res
                .get("result")
                .is_some_and(|v| v.py_str().contains("nil value")),
        format!("{res:?}"),
    );

    // ---- 12. context resolution
    check(
        "12a ge resolves to 0",
        ms::resolve_context(&PyJson::from("ge")) == Ok(0),
        String::new(),
    );
    check(
        "12b ts resolves to 1",
        ms::resolve_context(&PyJson::from("ts")) == Ok(1),
        String::new(),
    );
    check(
        "12c ui resolves to 2",
        ms::resolve_context(&PyJson::from("ui")) == Ok(2),
        String::new(),
    );
    check(
        "12d an unknown context is a ToolError",
        matches!(
            ms::resolve_context(&PyJson::from("garbage")),
            Err(ToolFail::Tool(_))
        ),
        String::new(),
    );

    // ---- 14. screenshot: what is asserted is the FILE
    let shot = tempfile::tempdir().unwrap();
    let shot_dir = shot.path().to_string_lossy().into_owned();
    let png_len = fake_png().len() as u64;
    let r = http.call("screenshot", json!({"dir": shot_dir}));
    check(
        "14a screenshot works with no game",
        !is_error(&r),
        r.to_string(),
    );
    let p: Value = serde_json::from_str(&text(&r)).unwrap_or(Value::Null);
    let path = std::path::PathBuf::from(p["path"].as_str().unwrap_or(""));
    check(
        "14b ... returns an absolute path",
        path.is_absolute(),
        p.to_string(),
    );
    check("14c ... wrote the file", path.is_file(), p.to_string());
    check(
        "14d ... and the reported byte count is the file's",
        p["bytes"] == png_len && std::fs::metadata(&path).map(|m| m.len()).ok() == Some(png_len),
        p.to_string(),
    );
    let last = *rec.captures.lock().last().unwrap();
    check(
        "14e ... hides the UI by default",
        last.0,
        format!("{last:?}"),
    );
    check(
        "14e2 ... and raises the game window by default",
        last.2,
        format!("{last:?}"),
    );
    http.call("screenshot", json!({"dir": shot_dir, "hide_ui": false}));
    let last = *rec.captures.lock().last().unwrap();
    check(
        "14f hide_ui=false is honoured",
        !last.0,
        format!("{last:?}"),
    );
    let path_of = |r: Value| -> String {
        serde_json::from_str::<Value>(&text(&r)).unwrap()["path"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let a = path_of(http.call("screenshot", json!({"dir": shot_dir})));
    let b = path_of(http.call("screenshot", json!({"dir": shot_dir})));
    check(
        "14g back-to-back shots land at different paths",
        a != b,
        format!("{a} {b}"),
    );
    let r = http.call(
        "screenshot",
        json!({"dir": shot_dir, "filename": "named.png"}),
    );
    check(
        "14h an explicit filename is used",
        std::path::Path::new(&path_of(r.clone())).file_name() == Some("named.png".as_ref()),
        r.to_string(),
    );
    let r = http.call(
        "screenshot",
        json!({"dir": shot_dir, "filename": "../escape.png"}),
    );
    check(
        "14i a filename containing a path is refused",
        is_error(&r),
        r.to_string(),
    );
    *rec.capture_meta.lock() =
        json!({"focused_game": false, "game_window_found": false, "region": null});
    let r = http.call("screenshot", json!({"dir": shot_dir}));
    let p: Value = serde_json::from_str(&text(&r)).unwrap_or(Value::Null);
    check(
        "14k a failed focus is reported, not silently believed",
        p.get("warning").is_some(),
        p.to_string(),
    );
    check(
        "14l ... and says the game window was not found",
        p["game_window_found"] == false,
        p.to_string(),
    );
    *rec.capture_meta.lock() = good_meta();
    http.call("screenshot", json!({"dir": shot_dir, "focus_game": false}));
    let last = *rec.captures.lock().last().unwrap();
    check(
        "14m focus_game=false is honoured",
        !last.2,
        format!("{last:?}"),
    );
    let sub = shot.path().join("sub").to_string_lossy().into_owned();
    let r = http.call("screenshot", json!({"dir": sub, "filename": "deep.png"}));
    check(
        "14j a missing directory is created",
        !is_error(&r),
        r.to_string(),
    );

    // ---- 15. camera_control: argument handling (no world in this harness)
    let r = http.call("camera_control", json!({"action": "get"}));
    check(
        "15a camera_control with no world is an isError",
        is_error(&r),
        r.to_string(),
    );
    check(
        "15b ... and says so rather than timing out on the console",
        text(&r).contains("no live world"),
        text(&r),
    );
    let r = http.call("camera_control", json!({"action": "nonsense"}));
    check(
        "15c an unknown action is an isError naming the valid ones",
        is_error(&r) && text(&r).contains("place"),
        text(&r),
    );

    // ---- 16. camera_control: the composed Lua
    for (name, args) in [
        (
            "absolute",
            json!({"x": 10, "y": 20, "z": 5, "yaw_deg": 90, "pitch_deg": -10}),
        ),
        (
            "relative",
            json!({"rel_fwd_m": -8, "rel_up_m": 3, "look_at": 1234}),
        ),
        (
            "relative+veh",
            json!({"veh_id": 7, "rel_fwd_m": 5, "rel_right_m": 2}),
        ),
        (
            "orbit",
            json!({"orbit_distance_m": 12, "orbit_azimuth_deg": 135,
                         "orbit_elevation_deg": 25}),
        ),
        (
            "abs+lookat_point",
            json!({"x": 1, "y": 2, "z": 3, "look_at": [4, 5, 6]}),
        ),
    ] {
        let chunk = ms::cam_place_chunk(&pyjson::loads(&args.to_string()).unwrap()).unwrap();
        check(
            &format!("16a {name} chunk fits one datagram"),
            chunk.chars().count() <= ms::MAX_DATAGRAM_CONTENT,
            chunk.len().to_string(),
        );
        check(
            &format!("16b {name} chunk carries no percent sign"),
            !chunk.contains('%'),
            String::new(),
        );
        check(
            &format!("16c {name} chunk enters the free camera first"),
            chunk.contains("isFreeCamera"),
            String::new(),
        );
    }
    for lua in [ms::CAM_READ_LUA, ms::CAM_LIST_LUA] {
        check(
            "16d the fixed chunks fit one datagram",
            lua.len() <= ms::MAX_DATAGRAM_CONTENT,
            lua.len().to_string(),
        );
        check(
            "16e the fixed chunks carry no percent sign",
            !lua.contains('%'),
            String::new(),
        );
    }

    // ---- 17. the angle conventions
    let (mut worst_y, mut worst_p) = (0.0f64, 0.0f64);
    for wy in [0.0, 45.0, 90.0, 137.5, 180.0, 270.0, 359.0] {
        for wp in [-80.0, -30.0, 0.0, 15.0, 60.0] {
            let (gy, gp) = mod_readback(game_forward(
                (wy - ms::CAM_YAW_OFFSET_DEG).rem_euclid(360.0),
                -wp,
            ));
            worst_y = worst_y.max(((gy - wy + 180.0).rem_euclid(360.0) - 180.0).abs());
            worst_p = worst_p.max((gp - wp).abs());
        }
    }
    check(
        "17a yaw/pitch round-trip over the whole sphere",
        worst_y < 1e-6 && worst_p < 1e-6,
        format!("{worst_y} {worst_p}"),
    );
    let (naive, _) = mod_readback(game_forward(90.0, 0.0));
    check(
        "17b ... and the un-offset form is wrong by 180",
        (naive - 90.0).abs() > 179.0,
        naive.to_string(),
    );
    check(
        "17c the yaw offset is the derived 180",
        ms::CAM_YAW_OFFSET_DEG == 180.0,
        String::new(),
    );
    check(
        "17d pitch_deg positive looks UP",
        game_forward((0.0 - ms::CAM_YAW_OFFSET_DEG).rem_euclid(360.0), -45.0)[2] > 0.7,
        String::new(),
    );
    check(
        "17e ... and the un-negated form would look DOWN",
        game_forward((0.0 - ms::CAM_YAW_OFFSET_DEG).rem_euclid(360.0), 45.0)[2] < -0.7,
        String::new(),
    );
    check(
        "17f the readback uses cameraInfo's own yaw formula",
        ms::CAM_READ_LUA.contains("math.atan2(-f.x,-f.y)"),
        String::new(),
    );
    let caminfo =
        std::fs::read_to_string(repo_root().join("bng_mod/lua/ge/extensions/cameraInfo.lua"))
            .unwrap_or_default();
    check(
        "17g ... and cameraInfo.lua still uses it too",
        caminfo.contains("atan2(-fwd.x, -fwd.y)") || caminfo.contains("atan2(-fwd.x,-fwd.y)"),
        "cameraInfo yaw changed".into(),
    );

    // ---- 18. get_config must not hand an agent a credential
    let raw = text(&http.call("get_config", json!({})));
    let cfg: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    check(
        "18a a set secret reads back redacted",
        cfg["ai_describer_api_key"] == "<redacted>",
        cfg.to_string(),
    );
    check(
        "18b ... and never leaks the stored value",
        !raw.contains("dpapi:v1:c2VhbGVk"),
        raw.clone(),
    );
    check(
        "18c an unset secret stays empty",
        cfg["ai_describer_openai_api_key"] == "",
        cfg.to_string(),
    );
    check(
        "18d ordinary settings are untouched",
        cfg["units"] == "imperial",
        cfg.to_string(),
    );
    check(
        "18e the tool description states the contract",
        tools
            .iter()
            .find(|t| t["name"] == "get_config")
            .and_then(|t| t["description"].as_str())
            .is_some_and(|d| d.contains("<redacted>")),
        String::new(),
    );

    (server.stop)();

    // ---- 13/19, the parts with a Rust counterpart
    let src = |rel: &str| {
        std::fs::read_to_string(repo_root().join("crates/beam-engine/src").join(rel))
            .unwrap_or_default()
    };
    let net = src("net.rs");
    let re = regex::Regex::new(r"_PORT\w*: u16 = (\d{4});").unwrap();
    let mod_ports: HashSet<u16> = re
        .captures_iter(&net)
        .map(|c| c[1].parse().unwrap())
        .collect();
    check(
        "13s the default MCP port is clear of every mod port",
        !mod_ports.is_empty()
            && !mod_ports.contains(&ms::DEFAULT_PORT)
            && ![8765, 8766].contains(&ms::DEFAULT_PORT),
        format!("{mod_ports:?}"),
    );
    check(
        "13u the shared capture helper is injected into the MCP deps",
        src("engine.rs").contains("capture_png_fn: Arc::new(crate::capture::capture_scene_png)"),
        String::new(),
    );
    let tools_src = src("mcp_server/tools.rs");
    check(
        "19a the mask reads secretstore's list, not a copy of it",
        tools_src.contains("secretstore::is_secret_setting(")
            && !tools_src.contains("SECRET_NAME_PARTS"),
        String::new(),
    );

    assert!(
        failures.is_empty(),
        "{} FAILURES:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
