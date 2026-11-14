//! Port of `diagnostic/audio_debug_row_sim.py`: the F9-then-J audio debug browser's
//! parsing, wording and stepping, plus the checks on the Lua halves' source (the FMOD
//! frequency table against the stock Engine Audio Debug window when the game is
//! installed, and the GE and vehicle sides naming the same keys).

use std::collections::BTreeSet;

use beam_base::pyfmt::py_float_str;
use beam_reports::audio_debug as ad;
use beam_testutil::repo_root;

fn row(pairs: &[(&str, &str)]) -> ad::Row {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn with(r: &ad::Row, k: &str, v: &str) -> ad::Row {
    let mut r = r.clone();
    r.insert(k.into(), v.into());
    r
}

#[test]
fn parse() {
    let r = ad::parse_fields(
        "key=mainGain;kind=number;futureField=x;label=Main gain;unit=dB;value=-3.5;min=-30;max=10;step=0.5",
    );
    assert!(
        r.get("label").map(String::as_str) == Some("Main gain")
            && r.get("value").map(String::as_str) == Some("-3.5"),
        "unknown field ignored, later fields intact: {r:?}"
    );
    assert_eq!(
        ad::parse_fields("20;soundCount=2")
            .get("count")
            .map(String::as_str),
        Some("20"),
        "bare leading count kept"
    );
}

#[test]
fn wording() {
    let r = ad::parse_fields(
        "key=mainGain;kind=number;futureField=x;label=Main gain;unit=dB;value=-3.5;min=-30;max=10;step=0.5",
    );
    assert_eq!(
        ad::row_line(&r).unwrap(),
        "Main gain, minus 3.5 decibels",
        "negative decibels spelled out"
    );
    let zero = with(&r, "value", "-0.0001");
    assert_eq!(
        ad::row_line(&zero).unwrap(),
        "Main gain, 0 decibels",
        "near-zero is plain 0"
    );

    for (hz, want) in [
        ("120", "120 hertz"),
        ("1044", "1.04 kilohertz"),
        ("2500", "2.5 kilohertz"),
        ("22000", "22 kilohertz"),
        ("382.35", "382 hertz"),
    ] {
        let freq = row(&[
            ("kind", "number"),
            ("label", "Low shelf frequency"),
            ("unit", "Hz"),
            ("value", "500"),
            ("hz", hz),
        ]);
        let got = ad::row_line(&freq).unwrap();
        assert_eq!(
            got,
            format!("Low shelf frequency, {want}"),
            "{hz} Hz spoken as {want}"
        );
    }

    let sel = row(&[
        ("kind", "select"),
        ("index", "1"),
        ("count", "2"),
        ("ref", "exhaust"),
        ("device", "mainEngine"),
        ("blend", "V8_cross_exhaust"),
        ("modified", "1"),
    ]);
    assert_eq!(
        ad::row_line(&sel).unwrap(),
        "Sound 2 of 2: exhaust, mainEngine, V8_cross_exhaust, changed",
        "selector line"
    );
    assert_eq!(
        ad::row_line(&row(&[
            ("kind", "action"),
            ("key", "restore"),
            ("label", "Restore vehicle defaults"),
            ("editable", "0")
        ]))
        .unwrap(),
        "Restore vehicle defaults, nothing changed",
        "restore with nothing changed says so"
    );
    assert_eq!(
        ad::sfx_line(&row(&[("kind", "dir"), ("name", "UI"), ("count", "71")])),
        "UI, folder, 71 items",
        "folder line"
    );
    assert_eq!(
        ad::sfx_line(&row(&[("kind", "dir"), ("name", "DLC"), ("count", "1")])),
        "DLC, folder, 1 item",
        "one-item folder"
    );
}

#[test]
fn stepping() {
    let freq = row(&[
        ("kind", "number"),
        ("value", "382.3529"),
        ("min", "0"),
        ("max", "1000"),
        ("step", "10"),
    ]);
    let mut seen = Vec::new();
    let mut r = freq.clone();
    for _ in 0..5 {
        let v = ad::stepped_value(&r, 1).expect("can step");
        seen.push(v);
        r.insert("value".into(), py_float_str(v));
    }
    let distinct: BTreeSet<u64> = seen.iter().map(|v| v.to_bits()).collect();
    assert!(
        distinct.len() == 5 && seen.windows(2).all(|w| w[0] <= w[1]),
        "five presses, five distinct increasing values: {seen:?}"
    );
    let up = ad::stepped_value(&freq, 1).unwrap();
    let back = ad::stepped_value(&with(&freq, "value", &py_float_str(up)), -1);
    assert_eq!(
        back,
        Some(382.3529),
        "step up then down returns the vehicle's own value"
    );
    assert_eq!(
        ad::stepped_value(&freq, 10),
        Some(482.3529),
        "shift step is ten steps"
    );
    assert_eq!(
        ad::stepped_value(&with(&freq, "value", "1000"), 1),
        None,
        "at the top, cannot move"
    );
    assert_eq!(
        ad::stepped_value(&with(&freq, "value", "5"), -1),
        Some(0.0),
        "clamped at the bottom"
    );
    let ratio = row(&[
        ("kind", "number"),
        ("value", "0.65"),
        ("min", "0"),
        ("max", "1"),
        ("step", "0.01"),
    ]);
    assert_eq!(
        ad::stepped_value(&ratio, 1),
        Some(0.66),
        "ratio step free of float noise"
    );
    assert!(
        ad::wire_number(0.66) == "0.66" && ad::wire_number(-3.5) == "-3.5",
        "wire number"
    );
    assert_eq!(
        ad::select_target(&row(&[("index", "1"), ("count", "2")]), 1),
        Some(0),
        "select wraps forward"
    );
    assert_eq!(
        ad::select_target(&row(&[("index", "0"), ("count", "3")]), -10),
        Some(2),
        "select wraps back"
    );
    assert_eq!(
        ad::select_target(&row(&[("index", "0"), ("count", "1")]), 1),
        None,
        "single sound cannot select"
    );
}

fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

fn lua(parts: &[&str]) -> String {
    let mut p = repo_root();
    for part in parts {
        p = p.join(part);
    }
    read(&p)
}

fn float_list(text: &str) -> Vec<f64> {
    text.split(',')
        .map(|x| beam_reports::pyops::py_str_to_float(x).expect("number in table"))
        .collect()
}

#[test]
fn lua_source() {
    let ge = lua(&[
        "bng_mod",
        "lua",
        "ge",
        "extensions",
        "audioDebugAccessible.lua",
    ]);
    let veh = lua(&[
        "bng_mod",
        "lua",
        "vehicle",
        "extensions",
        "beamtelAudioDebug.lua",
    ]);

    let ge_keys: BTreeSet<String> = regex::Regex::new(r#"\{key = "(\w+)",\s+label"#)
        .unwrap()
        .captures_iter(&ge)
        .map(|c| c[1].to_string())
        .collect();
    let veh_keys: BTreeSet<String> = regex::Regex::new(r"\bout\.(\w+) =")
        .unwrap()
        .captures_iter(&veh)
        .map(|c| c[1].to_string())
        .collect();
    assert!(
        ge_keys == veh_keys && ge_keys.len() == 16,
        "GE controls and vehicle values name the same keys: GE only {:?}, vehicle only {:?}",
        ge_keys.difference(&veh_keys).collect::<Vec<_>>(),
        veh_keys.difference(&ge_keys).collect::<Vec<_>>()
    );

    // PARAM maps the plain keys; mainGain, muffling and cylinders are branches of their
    // own in set().
    let param_block = veh
        .split("local PARAM = {")
        .nth(1)
        .expect("PARAM block")
        .split('}')
        .next()
        .unwrap_or("");
    let mut veh_settable: BTreeSet<String> = regex::Regex::new(r#"(\w+) = "\w+""#)
        .unwrap()
        .captures_iter(param_block)
        .map(|c| c[1].to_string())
        .collect();
    for k in ["mainGain", "muffling", "cylinders"] {
        if veh.contains(&format!("key == \"{k}\"")) {
            veh_settable.insert(k.to_string());
        }
    }
    assert!(
        ge_keys.is_subset(&veh_settable),
        "every GE control is settable on the vehicle side: {:?}",
        ge_keys.difference(&veh_settable).collect::<Vec<_>>()
    );

    let ours = regex::Regex::new(r"local FMOD_HZ = \{([^}]*)\}")
        .unwrap()
        .captures(&ge)
        .map(|c| float_list(&c[1]))
        .expect("FMOD_HZ table");
    let pf =
        std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    let stock_path = std::path::PathBuf::from(pf)
        .join("Steam")
        .join("steamapps")
        .join("common")
        .join("BeamNG.drive")
        .join("lua")
        .join("ge")
        .join("extensions")
        .join("editor")
        .join("engineAudioDebug.lua");
    if stock_path.exists() {
        let stock = regex::Regex::new(r"local fmodtable = \{([^}]*)\}")
            .unwrap()
            .captures(&read(&stock_path))
            .map(|c| float_list(&c[1]));
        assert!(
            stock.as_ref() == Some(&ours) && !ours.is_empty(),
            "FMOD table matches the stock Engine Audio Debug window: ours {ours:?}, stock {stock:?}"
        );
    } else {
        println!(
            "SKIP FMOD table vs stock: game install not found at {}",
            stock_path.display()
        );
    }

    // The GE conversion, mirrored, against values measured in game.
    let fmod_to_hz = |f: f64| {
        let f = f.min(1000.0).max(0.0);
        let seg = ((f / 100.0).floor() as usize).min(9);
        ours[seg] + (f - seg as f64 * 100.0) / 100.0 * (ours[seg + 1] - ours[seg])
    };
    for (f, hz) in [
        (250.0, 120.0),
        (729.6296, 3500.0),
        (514.0625, 750.0),
        (560.0, 1044.0),
        (1000.0, 22000.0),
        (0.0, 20.0),
    ] {
        assert!(
            (fmod_to_hz(f) - hz).abs() < 0.5,
            "FMOD {f} is {hz} Hz: {}",
            fmod_to_hz(f)
        );
    }
}
