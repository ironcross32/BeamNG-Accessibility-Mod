//! Port of `diagnostic/vehicle_info_sim.py`: rendering, the failure code, the Lua wrapper
//! contract, and the absent-mod latch. Scenario numbers follow the Python.
//!
//! Scenario 1's segment join is a Lua rule the Python sim only demonstrates with its own
//! transcription, so it is not repeated here. Scenario 5's spawner checks belong to the
//! vehicle spawner port (`vehicle_spawner.rs`, E8); the beamtel half of it (the latch)
//! is tested behaviourally below rather than by grepping the Python source.

use std::sync::Once;

use beam_engine::browsers;
use beam_engine::listeners::vehicle_info::handle_vehicle_info_text;
use beam_engine::net::{VEHICLE_INFO_CMD_PORT, VEHICLE_INFO_LISTEN_PORT};
use beam_engine::speech_out;
use beam_engine::state::{State, VINFO_EVENT, state};
use beam_testutil::repo_root;
use parking_lot::Mutex;
use serde_json::{Value, json};

static REPLIES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ENV: Once = Once::new();

fn setup() -> parking_lot::MutexGuard<'static, ()> {
    let g = speech_out::TEST_SERIAL.lock();
    ENV.call_once(|| {
        let dir = std::env::temp_dir().join(format!("beam_vinfo_sim_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: inside `Once`, before any state static is touched, under TEST_SERIAL.
        unsafe { std::env::set_var("LOCALAPPDATA", &dir) };
    });
    beam_engine::listeners::scanner::set_send_hook_for_tests(Some(Box::new(
        |port: u16, _payload: &[u8]| {
            if port == VEHICLE_INFO_CMD_PORT {
                for r in REPLIES.lock().clone() {
                    handle_vehicle_info_text(&r);
                }
            }
        },
    )));
    *state() = State::default();
    VINFO_EVENT.clear();
    REPLIES.lock().clear();
    g
}

fn rows() -> Vec<Value> {
    vec![
        json!({"i": 0, "kind": "head", "label": "", "value": "Pessima Track (M)"}),
        json!({"i": 1, "kind": "head", "label": "Brand", "value": "Ibishu"}),
        json!({"i": 2, "kind": "group", "label": "Summary", "value": ""}),
        json!({"i": 3, "kind": "spec", "label": "Weight", "value": "3075 lb"}),
        json!({"i": 4, "kind": "group", "label": "Features", "value": ""}),
        json!({"i": 5, "kind": "spec", "label": "", "value": "Drivetrain: All Wheel Drive"}),
    ]
}

#[test]
fn s2_rows_render_with_headings_kept_as_headings() {
    let lines = browsers::vinfo_render(&rows());
    assert_eq!(lines[0], "Pessima Track (M)");
    assert!(lines.contains(&"Weight: 3075 lb".to_string()));
    assert!(lines.contains(&"Summary".to_string()) && lines.contains(&"Features".to_string()));
    assert!(!lines.iter().any(|l| l.ends_with(':')), "no trailing colon");
    assert!(
        lines.contains(&"Drivetrain: All Wheel Drive".to_string())
            && !lines.iter().any(|l| l.starts_with(':'))
    );
    assert_eq!(lines.len(), rows().len());
}

#[test]
fn s3_the_failure_wire_carries_a_code() {
    let _g = setup();
    for (wire, code, sentence) in [
        (
            "notselector;not on a vehicle selector screen",
            "notselector",
            "not on a vehicle selector screen",
        ),
        (
            "nofocus;no vehicle selected on this screen yet",
            "nofocus",
            "no vehicle selected on this screen yet",
        ),
        (
            "nomodel;no such vehicle or configuration",
            "nomodel",
            "no such vehicle or configuration",
        ),
        ("nomodel;a, b; and c", "nomodel", "a, b; and c"),
        // A legacy sentence has no separator and must not read as notselector.
        (
            "no such vehicle or configuration",
            "unknown",
            "no such vehicle or configuration",
        ),
    ] {
        *REPLIES.lock() = vec![format!("INFOFAIL:{wire}")];
        let (lines, failure) = browsers::request_vehicle_info(None, None, 0.6);
        assert!(lines.is_empty());
        assert_eq!(
            failure,
            Some((code.to_string(), sentence.to_string())),
            "{wire}"
        );
    }
    let lua = lua();
    assert!(
        lua.contains("\"notselector\""),
        "the Lua half sends that code"
    );
}

fn lua() -> String {
    std::fs::read_to_string(
        repo_root()
            .join("bng_mod")
            .join("lua")
            .join("ge")
            .join("extensions")
            .join("vehicleInfo.lua"),
    )
    .expect("vehicleInfo.lua")
}

#[test]
fn s4_the_lua_wrapper_contract() {
    let lua = lua();
    let body: String = lua
        .lines()
        .filter(|l| !l.trim().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(body.contains("ui_vehicleSelector_vehicleSpecifications"));
    assert!(!body.contains("ui_vehicleSelector_detailsInteraction"));
    assert!(body.contains("WRAP_VERSION") && body.contains("gen[WRAP_MARK] == WRAP_VERSION"));
    assert!(body.contains("ORIG_KEY") && body.contains("gen[ORIG_KEY] or gen.requestDetails"));
    assert!(body.contains("local rec = gen[RECORDER_KEY]"));
    assert!(body.contains("gen[RECORDER_KEY] == recordFromItem"));
    assert!(body.contains("local bound, berr = udpCmd:setsockname"));
    assert!(body.contains("function M.onExtensionUnloaded()"));
    assert!(body.contains("retryCmdBind(dtReal)"));
    for (lua_name, port) in [
        ("PYTHON_PORT_DATA", VEHICLE_INFO_LISTEN_PORT),
        ("CMD_LISTEN_PORT", VEHICLE_INFO_CMD_PORT),
    ] {
        let lua_port = lua
            .lines()
            .find(|l| l.trim().starts_with(&format!("local {lua_name}")))
            .map(|l| {
                l.split('=')
                    .nth(1)
                    .unwrap()
                    .split("--")
                    .next()
                    .unwrap()
                    .trim()
                    .to_string()
            });
        assert_eq!(lua_port, Some(port.to_string()), "{lua_name}");
    }
    for cmd in ["INFO_SELECTOR", "INFO"] {
        assert!(body.contains(cmd), "the mod handles {cmd}");
    }
}

#[test]
fn s5_the_absent_mod_latch() {
    let _g = setup();
    // No reply: the full timeout is paid once and the half is latched absent.
    let t0 = std::time::Instant::now();
    let (_, failure) = browsers::request_vehicle_info(None, None, 0.3);
    assert!(t0.elapsed().as_secs_f64() >= 0.25);
    assert_eq!(failure, Some(("timeout".into(), String::new())));
    assert!(state().vinfo.vinfo_absent);
    // ...so a later press asks but does not WAIT.
    let t1 = std::time::Instant::now();
    let (_, failure) = browsers::request_vehicle_info(None, None, 5.0);
    assert!(t1.elapsed().as_secs_f64() < 0.2);
    assert_eq!(failure, Some(("timeout".into(), String::new())));
    // ...and any arriving packet, a refusal included, clears it again.
    *REPLIES.lock() = vec!["INFOFAIL:nofocus;none".into()];
    let (_, failure) = browsers::request_vehicle_info(None, None, 0.6);
    assert_eq!(failure, Some(("nofocus".into(), "none".into())));
    assert!(!state().vinfo.vinfo_absent);
}
