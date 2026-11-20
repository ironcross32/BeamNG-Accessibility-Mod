//! Port of `diagnostic/environment_row_sim.py`: the F9-then-N environment browser's
//! parsing, wording and stepping, and the checks on environmentAccessible.lua's source.
//! Scenario numbers follow the Python.

use std::sync::Once;

use beam_engine::browsers;
use beam_engine::listeners::environment::{env_float, parse_env_fields};
use beam_engine::net::{ENV_CMD_PORT, ENV_LISTEN_PORT};
use beam_engine::state::{EnvRow, State, state};
use beam_engine::{speech_out, units};
use beam_testutil::repo_root;
use parking_lot::Mutex;
use serde_json::Value;

static SENT: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ENV: Once = Once::new();

fn setup() -> parking_lot::MutexGuard<'static, ()> {
    let g = speech_out::TEST_SERIAL.lock();
    ENV.call_once(|| {
        let dir = std::env::temp_dir().join(format!("beam_env_sim_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: inside `Once`, before any state static is touched, under TEST_SERIAL.
        unsafe { std::env::set_var("LOCALAPPDATA", &dir) };
    });
    speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
    beam_engine::listeners::scanner::set_send_hook_for_tests(Some(Box::new(
        |_p: u16, payload: &[u8]| {
            SENT.lock()
                .push(String::from_utf8_lossy(payload).into_owned())
        },
    )));
    *state() = State::default();
    speech_out::reset_for_tests();
    SENT.lock().clear();
    units::set_units_mode("imperial");
    g
}

fn spoken() -> Vec<String> {
    speech_out::get_speech_log(None, Some(0), None, None, false)
        .entries
        .iter()
        .map(|e| e.text.clone())
        .collect()
}

fn temp_row(over: &[(&str, &str)]) -> EnvRow {
    let mut row: EnvRow = [
        ("key", "temperature"),
        ("kind", "numberC"),
        ("value", "15.00"),
        ("min", "-60.0"),
        ("max", "60.0"),
        ("step", "1"),
        ("label", "Temperature"),
        ("curveLo", "15.00"),
        ("curveHi", "15.00"),
        ("editable", "1"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    for (k, v) in over {
        row.insert(k.to_string(), v.to_string());
    }
    row
}

/// `_env_on_adjust(0, "", data, delta)` against `rows`, timer cancelled; the sends.
fn adjust(rows: Vec<EnvRow>, data: i64, delta: i64) -> Vec<String> {
    SENT.lock().clear();
    state().env.env_rows = rows;
    browsers::env_on_adjust(Some(Value::from(data)), delta);
    if let Some(t) = state().env.env_change_timer.take() {
        t.cancel();
    }
    state().env.env_awaiting_change = false;
    SENT.lock().clone()
}

fn sent_celsius(cmd: &str) -> f64 {
    cmd.split('=').nth(1).unwrap().parse().unwrap()
}

/// Feed a row through `_env_on_adjust` `times`, applying each result back.
fn press(row: EnvRow, delta: i64, times: usize) -> Vec<i64> {
    let mut seen = Vec::new();
    let mut cur = row;
    for _ in 0..times {
        let sent = adjust(vec![cur.clone()], 0, delta);
        let Some(last) = sent.last() else { break };
        let c = sent_celsius(last);
        cur.insert("value".into(), format!("{c:.2}"));
        match units::fmt_temp_c_or_f(c).0 {
            units::PyVal::Int(n) => seen.push(n),
            other => panic!("{other:?}"),
        }
    }
    seen
}

const LIVE: &str = "key=temperature;kind=numberC;value=15.00;min=-60.0;max=60.0;step=1;label=Temperature;curveLo=15.00;curveHi=15.00;editable=1";

#[test]
fn s1_named_fields_parse_and_unknown_fields_do_not_shift() {
    let got = parse_env_fields(LIVE);
    assert_eq!(got["key"], "temperature");
    assert_eq!(got["value"], "15.00");
    assert_eq!(got["editable"], "1");
    assert_eq!(
        parse_env_fields("key=restore;label=Restore level default")["label"],
        "Restore level default"
    );
    let future = parse_env_fields(&format!("{LIVE};humidityPct=41;windChillC=9.5"));
    assert!(
        future["value"] == "15.00" && future["editable"] == "1" && future["humidityPct"] == "41"
    );
    let head = parse_env_fields("2;level=smallgrid;canChange=1");
    assert_eq!(head["count"], "2");
    assert_eq!(head["level"], "smallgrid");
}

#[test]
fn s2_readout_speaks_the_users_unit() {
    let _g = setup();
    assert_eq!(
        browsers::env_row_line(&temp_row(&[])),
        "Temperature, 59 Fahrenheit"
    );
    units::set_units_mode("metric");
    assert_eq!(
        browsers::env_row_line(&temp_row(&[])),
        "Temperature, 15 Celsius"
    );
    units::set_units_mode("imperial");
}

#[test]
fn s3_a_curve_that_is_not_flat_says_so() {
    let _g = setup();
    let line = browsers::env_row_line(&temp_row(&[("curveLo", "5.00"), ("curveHi", "25.00")]));
    assert!(
        line.contains("varies") && line.contains("41") && line.contains("77"),
        "{line}"
    );
    assert!(line.starts_with("Temperature, 59 Fahrenheit"), "{line}");
    let flat = browsers::env_row_line(&temp_row(&[("curveHi", "15.40")]));
    assert!(!flat.contains("varies"), "{flat}");
    let locked = browsers::env_row_line(&temp_row(&[("editable", "0")]));
    assert!(locked.ends_with(", locked"), "{locked}");
    assert_eq!(
        browsers::env_row_line(&temp_row(&[("value", "")])),
        "Temperature, not available"
    );
}

#[test]
fn s4_the_restore_row() {
    let _g = setup();
    let act: EnvRow = [
        ("key", "restore"),
        ("kind", "action"),
        ("label", "Restore level default"),
        ("value", "15.00"),
        ("curveLo", "15.00"),
        ("curveHi", "15.00"),
        ("editable", "1"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(
        browsers::env_row_line(&act),
        "Restore level default, 59 Fahrenheit"
    );
    let mut cycle = act.clone();
    cycle.insert("curveLo".into(), "5.00".into());
    cycle.insert("curveHi".into(), "25.00".into());
    assert_eq!(
        browsers::env_row_line(&cycle),
        "Restore level default, 41 to 77 Fahrenheit"
    );
    let mut none = act;
    for k in ["value", "curveLo", "curveHi"] {
        none.insert(k.into(), String::new());
    }
    assert_eq!(
        browsers::env_row_line(&none),
        "Restore level default, not available"
    );
}

#[test]
fn s5_adjusting_steps_in_the_spoken_unit() {
    let _g = setup();
    assert_eq!(press(temp_row(&[]), 1, 5), vec![60, 61, 62, 63, 64]);
    assert_eq!(press(temp_row(&[]), -1, 3), vec![58, 57, 56]);
    units::set_units_mode("metric");
    assert_eq!(press(temp_row(&[]), 1, 3), vec![16, 17, 18]);
    units::set_units_mode("imperial");
    assert_eq!(press(temp_row(&[]), 10, 1), vec![69]);
}

#[test]
fn s6_the_adjust_respects_the_band_and_the_lock() {
    let _g = setup();
    let s = adjust(vec![temp_row(&[("value", "60.00")])], 0, 10);
    assert!(
        (sent_celsius(s.last().unwrap()) - 60.0).abs() < 1e-6,
        "{s:?}"
    );
    let s = adjust(vec![temp_row(&[("value", "-60.00")])], 0, -10);
    assert!(
        (sent_celsius(s.last().unwrap()) + 60.0).abs() < 1e-6,
        "{s:?}"
    );
    speech_out::reset_for_tests();
    assert!(adjust(vec![temp_row(&[("editable", "0")])], 0, 1).is_empty());
    assert_eq!(spoken(), vec!["Locked".to_string()]);
    assert!(adjust(vec![temp_row(&[("kind", "action")])], 0, 1).is_empty());
    assert!(adjust(vec![temp_row(&[])], 5, 1).is_empty());
}

fn lua() -> String {
    std::fs::read_to_string(
        repo_root()
            .join("bng_mod")
            .join("lua")
            .join("ge")
            .join("extensions")
            .join("environmentAccessible.lua"),
    )
    .expect("environmentAccessible.lua")
}

fn lua_port(lua: &str, name: &str) -> Option<String> {
    lua.lines()
        .find(|l| l.trim().starts_with(&format!("local {name}")))
        .map(|l| {
            l.split('=')
                .nth(1)
                .unwrap()
                .split("--")
                .next()
                .unwrap()
                .trim()
                .to_string()
        })
}

#[test]
fn s7_the_two_halves_agree_on_ports_and_commands() {
    let lua = lua();
    assert_eq!(
        lua_port(&lua, "PYTHON_PORT_DATA"),
        Some(ENV_LISTEN_PORT.to_string())
    );
    assert_eq!(
        lua_port(&lua, "CMD_LISTEN_PORT"),
        Some(ENV_CMD_PORT.to_string())
    );
    for cmd in ["REQUEST", "SET", "RESTORE"] {
        assert!(lua.contains(cmd), "the mod handles {cmd}");
    }
    assert!(
        lua.contains("{{0, celsius}, {1, celsius}}"),
        "a flat set writes TWO curve points"
    );
    assert!(
        lua.contains("core_environment.onInit()")
            && !lua.split("--").next().unwrap().contains("setEditorDirty"),
        "the refresh goes through core_environment.onInit"
    );
}

#[test]
fn s8_ambient_readout() {
    let _g = setup();
    state().env.env_ambient_c = Some(15.0);
    browsers::speak_ambient_temperature();
    assert_eq!(
        spoken(),
        vec!["Ambient temperature 59 Fahrenheit".to_string()]
    );

    units::set_units_mode("metric");
    speech_out::reset_for_tests();
    state().env.env_ambient_c = Some(-3.6);
    browsers::speak_ambient_temperature();
    assert_eq!(spoken(), vec!["Ambient temperature -4 Celsius".to_string()]);

    speech_out::reset_for_tests();
    SENT.lock().clear();
    state().env.env_ambient_c = None;
    browsers::speak_ambient_temperature();
    assert_eq!(spoken(), vec!["No ambient temperature".to_string()]);
    assert_eq!(*SENT.lock(), vec!["REQUEST".to_string()]);
    units::set_units_mode("imperial");

    let lua = lua();
    assert!(lua.contains("AMBIENT:valueC=") && lua.contains("\"AMBIENT:\""));
    assert_eq!(env_float(&parse_env_fields(""), "valueC"), None);
}
