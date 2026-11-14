//! Port of `diagnostic/induction_report_sim.py`: the F9 Alt+T forced induction report,
//! and the Lua half's wire contract (scenarios 10 and 11 read the real Lua files).

mod common;

use beam_base::pyfmt::{py_float_str, py_round};
use beam_reports::induction_report as ir;
use beam_reports::pyops::{PyDict, PyValue, json_loads};
use beam_testutil::repo_root;

fn psi(v: f64) -> (String, String) {
    (py_float_str(py_round(v, 1)), "psi".into())
}

fn bar(v: f64) -> (String, String) {
    (py_float_str(py_round(v / 14.504, 2)), "bar".into())
}

fn obj(json: &str) -> PyDict {
    match json_loads(json).unwrap() {
        PyValue::Dict(d) => d,
        _ => panic!("not an object"),
    }
}

/// A roots-blown car at wide open throttle, near its torque peak.
const SUPERCHARGER: &str = r#"{"boost": 7.26, "boostMax": 9.633, "type": "roots", "twisted": false,
 "blowerRPM": 7220.4, "lossCoef": 0.1083, "peakCoef": 1.4729, "peakRPM": 4225, "condition": 1}"#;

/// A turbo car idling in Park.
const TURBO: &str = r#"{"boost": 0, "boostMax": 28.51, "wastegateTarget": 28.505, "turboRPM": 0,
 "maxTurboRPM": 250000, "peakCoef": 2.0786, "peakRPM": 5137, "heat": 320.7, "heatLimit": 650,
 "hot": false, "condition": 1, "blowOff": true, "antilag": false}"#;

fn with(base: &str, changes: &[(&str, PyValue)]) -> PyValue {
    let mut d = obj(base);
    for (k, v) in changes {
        d.insert(k.to_string(), v.clone());
    }
    PyValue::Dict(d)
}

fn turbo_report(changes: &[(&str, PyValue)]) -> PyDict {
    let mut r = PyDict::new();
    r.insert("torqueCoef".into(), PyValue::Float(0.974));
    r.insert("damaged".into(), PyValue::Bool(false));
    r.insert("turbo".into(), with(TURBO, changes));
    r
}

fn sc_report(changes: &[(&str, PyValue)]) -> PyDict {
    let mut r = PyDict::new();
    r.insert("torqueCoef".into(), PyValue::Float(1.473));
    r.insert("damaged".into(), PyValue::Bool(false));
    r.insert("supercharger".into(), with(SUPERCHARGER, changes));
    r
}

fn phrase(report: &PyDict) -> String {
    ir::report_phrase(report, &psi).unwrap()
}

fn set(mut r: PyDict, k: &str, v: PyValue) -> PyDict {
    r.insert(k.into(), v);
    r
}

#[test]
fn scenario_1_the_whole_sentence_for_each_charger() {
    let p = phrase(&sc_report(&[]));
    assert_eq!(
        p,
        "Roots supercharger, Boost 7.3 of 9.6 psi, Blower 7,220 RPM, Blower drag 11 percent, \
         Engine torque up 47 percent, Best gain 47 percent at 4,225 RPM, Condition 100 percent",
        "Full supercharger report"
    );
    let p = phrase(&turbo_report(&[]));
    assert_eq!(
        p,
        "Turbo with blow-off valve, Boost 0.0 of 28.5 psi, Wastegate target 28.5 psi in this gear, \
         Turbo speed 0 RPM, 0 percent of maximum, Engine torque down 3 percent, \
         Best gain about 108 percent at 5,137 RPM, Heat 49 percent of damage limit, Condition 100 percent",
        "Full turbo report"
    );
}

#[test]
fn scenario_2_pressure_goes_through_the_callers_units() {
    let p = ir::report_phrase(&turbo_report(&[]), &bar).unwrap();
    assert!(
        p.contains("Boost 0.0 of 1.97 bar") && p.contains("target 1.97 bar"),
        "Metric units: {p}"
    );
}

#[test]
fn scenario_3_spinning_up_and_vacuum() {
    let p = phrase(&turbo_report(&[
        ("turboRPM", PyValue::int(125000)),
        ("boost", PyValue::Float(-2.0)),
    ]));
    assert!(
        p.contains("Turbo speed 125,000 RPM, 50 percent of maximum"),
        "Turbo speed share: {p}"
    );
    assert!(p.contains("Boost -2.0 of"), "Vacuum reads negative: {p}");
    let p = phrase(&turbo_report(&[
        ("maxTurboRPM", PyValue::int(0)),
        ("turboRPM", PyValue::int(100)),
    ]));
    assert!(
        p.contains("Turbo speed 100 RPM,") && !p.contains("of maximum"),
        "No maximum, no share: {p}"
    );
}

#[test]
fn scenario_4_what_is_fitted() {
    assert!(
        phrase(&turbo_report(&[("antilag", PyValue::Bool(true))]))
            .starts_with("Turbo with blow-off valve and anti-lag,"),
        "Anti-lag named"
    );
    assert!(
        phrase(&turbo_report(&[
            ("blowOff", PyValue::Bool(false)),
            ("antilag", PyValue::Bool(true))
        ]))
        .starts_with("Turbo with anti-lag,"),
        "Anti-lag alone"
    );
    assert!(
        phrase(&turbo_report(&[("blowOff", PyValue::Bool(false))])).starts_with("Turbo,"),
        "Neither"
    );
    assert!(
        phrase(&sc_report(&[("twisted", PyValue::Bool(true))]))
            .starts_with("Roots supercharger with twisted lobes,"),
        "Twisted roots"
    );
    assert!(
        phrase(&sc_report(&[
            ("type", PyValue::str("centrifugal")),
            ("twisted", PyValue::Bool(true))
        ]))
        .starts_with("Centrifugal supercharger,"),
        "Twisted ignored off roots"
    );
    assert!(
        phrase(&sc_report(&[("type", PyValue::str("future"))])).starts_with("Supercharger,"),
        "A type this file has never heard of still reads"
    );
}

#[test]
fn scenario_5_torque_as_a_change() {
    assert!(
        phrase(&set(sc_report(&[]), "torqueCoef", PyValue::Float(1.0)))
            .contains("Engine torque unchanged"),
        "Exactly 1 reads unchanged"
    );
    assert!(
        phrase(&set(sc_report(&[]), "torqueCoef", PyValue::Float(0.983)))
            .contains("Engine torque down 2 percent"),
        "Idle reads as a loss"
    );
}

#[test]
fn scenario_6_heat_and_damage() {
    let r = set(
        turbo_report(&[
            ("heat", PyValue::Float(715.0)),
            ("hot", PyValue::Bool(true)),
            ("condition", PyValue::Float(0.4)),
        ]),
        "damaged",
        PyValue::Bool(true),
    );
    let p = phrase(&r);
    assert!(
        p.ends_with("Heat 110 percent of damage limit, Overheating, Condition 40 percent, Induction system damaged"),
        "Hot and damaged: {p}"
    );
    assert!(
        !phrase(&turbo_report(&[("heat", PyValue::None)])).contains("Heat"),
        "No heat, no claim"
    );
    let mut no_peak = obj(TURBO);
    no_peak.shift_remove("peakCoef");
    let mut r = PyDict::new();
    r.insert("torqueCoef".into(), PyValue::int(1));
    r.insert("turbo".into(), PyValue::Dict(no_peak));
    assert!(!phrase(&r).contains("Best gain"), "No peak, no claim");
}

#[test]
fn scenario_7_two_step() {
    assert!(
        phrase(&set(turbo_report(&[]), "twoStep", PyValue::Bool(true))).ends_with("Two-step on"),
        "Two-step on"
    );
    assert!(
        phrase(&set(turbo_report(&[]), "twoStep", PyValue::Bool(false))).ends_with("Two-step off"),
        "Two-step off"
    );
    assert!(
        !phrase(&turbo_report(&[])).contains("Two-step"),
        "No two-step, no claim"
    );
}

fn twin() -> PyDict {
    let mut r = PyDict::new();
    r.insert("torqueCoef".into(), PyValue::Float(1.9));
    r.insert("damaged".into(), PyValue::Bool(false));
    r.insert("twoStep".into(), PyValue::Bool(true));
    r.insert(
        "turbo".into(),
        with(TURBO, &[("boost", PyValue::Float(10.0))]),
    );
    r.insert("supercharger".into(), PyValue::Dict(obj(SUPERCHARGER)));
    r
}

#[test]
fn scenario_8_twin_charged() {
    let p = phrase(&twin());
    assert!(
        p.starts_with("Turbo with blow-off valve, Boost 10.0"),
        "Twin leads with the turbo: {p}"
    );
    assert!(
        p.contains(". Roots supercharger, Boost 7.3 of 9.6 psi"),
        "Twin has the supercharger: {p}"
    );
    assert_eq!(
        p.matches("Engine torque").count(),
        1,
        "Twin says torque once: {p}"
    );
    assert!(
        p.ends_with(". Engine torque up 90 percent, Two-step on"),
        "Twin ends on the engine: {p}"
    );
}

/// `json.dumps(twin)`, as the sim builds the reply.
const TWIN_JSON: &str = r#"{"torqueCoef": 1.9, "damaged": false, "twoStep": true, "turbo": {"boost": 10.0, "boostMax": 28.51, "wastegateTarget": 28.505, "turboRPM": 0, "maxTurboRPM": 250000, "peakCoef": 2.0786, "peakRPM": 5137, "heat": 320.7, "heatLimit": 650, "hot": false, "condition": 1, "blowOff": true, "antilag": false}, "supercharger": {"boost": 7.26, "boostMax": 9.633, "type": "roots", "twisted": false, "blowerRPM": 7220.4, "lossCoef": 0.1083, "peakCoef": 1.4729, "peakRPM": 4225, "condition": 1}}"#;

#[test]
fn scenario_9_parsing_refuses_what_is_not_a_report() {
    let parsed = ir::parse_reply(&format!("FI:{TWIN_JSON}"))
        .unwrap()
        .expect("parses a reply");
    assert!(
        common::same_pv(&PyValue::Dict(parsed), &PyValue::Dict(twin())),
        "Parses a reply"
    );
    for bad in [
        "NONE",
        "DONE",
        "ERROR:boom",
        "FI:",
        "FI:{truncated",
        "FI:[1,2]",
        "SC:{}",
    ] {
        assert_eq!(ir::parse_reply(bad), Ok(None), "Rejects {bad:?}");
    }
}

fn read_lua(parts: &[&str]) -> String {
    let mut p = repo_root();
    for part in parts {
        p = p.join(part);
    }
    // Python reads in text mode: CRLF becomes LF.
    std::fs::read_to_string(&p)
        .expect("read Lua source")
        .replace("\r\n", "\n")
}

#[test]
fn scenario_10_the_lua_half_chunk() {
    let scanner = read_lua(&["bng_mod", "lua", "ge", "extensions", "vehicleScanner.lua"]);
    let re =
        regex::Regex::new(r#"(?s)cmd == "INDUCTION".*?queueLuaCommand\(\[\[(.*?)\]\]\)"#).unwrap();
    let m = re.captures(&scanner);
    assert!(m.is_some(), "Chunk found");
    let chunk = m.unwrap()[1].to_string();
    assert!(!chunk.contains('%'), "Chunk has no percent sign");
    assert!(chunk.contains("_ds:close()"), "Chunk reaches its own end");
    assert!(
        chunk.contains(&format!("\"{}\"", ir::REPLY_PREFIX)),
        "Chunk sends the report prefix"
    );
    assert!(
        chunk.contains("_ds:send(\"DONE\")"),
        "Chunk always terminates the reply"
    );
    let mut keys: Vec<String> = Vec::new();
    for src in [SUPERCHARGER, TURBO] {
        keys.extend(obj(src).keys().cloned());
    }
    keys.extend(["torqueCoef", "damaged", "twoStep"].map(String::from));
    for key in keys {
        let re = regex::Regex::new(&format!(r"\b{key} = ")).unwrap();
        assert!(re.is_match(&chunk), "Chunk sends {key}");
    }
    assert!(
        chunk.matches("supercharger.isExisting").count() >= 2
            && chunk.matches("turbocharger.isExisting").count() >= 2,
        "Both chargers are gated on isExisting, not presence"
    );
}

#[test]
fn scenario_11_the_telemetry_half() {
    let protocol = read_lua(&[
        "bng_mod",
        "lua",
        "vehicle",
        "protocols",
        "796F6C6F313035.lua",
    ]);
    assert!(
        protocol.contains("o.turbo = (electrics.values.boost or 0)"),
        "Current boost is the combined electric"
    );
    assert!(
        protocol.contains("electrics.values.superchargerBoost\n"),
        "Capability bit covers a supercharger"
    );
}
