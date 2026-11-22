//! Golden cases from `fixtures/gen/gear_report.py`.

use beam_base::gear_report::*;
use beam_base::pyval::{PyValue, json_loads};
use beam_testutil::{f, load, s, same_f64};
use serde_json::Value;

/// Gear strings whose tail is a non-ASCII digit: Python's `isdigit()` accepts them,
/// this port does not (a documented difference; telemetry gear strings are ASCII).
const NON_ASCII_DIGITS: [&str; 1] = ["S²"];

fn opt_str(v: &Value) -> Option<&str> {
    v.as_str()
}

fn check(failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn constants_match_python() {
    let fx = load("gear_report");
    let c = &fx["constants"];
    let strs = |v: &Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| s(x).to_string())
            .collect()
    };
    assert_eq!(strs(&c["MODES"]), MODES);
    assert_eq!(strs(&c["NUMBER_STYLES"]), NUMBER_STYLES);
    for (i, (k, w)) in ORDINAL_WORDS.iter().enumerate() {
        assert_eq!(c["ORDINAL_WORDS"][i][0].as_i64(), Some(*k));
        assert_eq!(s(&c["ORDINAL_WORDS"][i][1]), *w);
    }
    assert_eq!(
        c["ORDINAL_WORDS"].as_array().unwrap().len(),
        ORDINAL_WORDS.len()
    );
    for (i, (k, l)) in ROOT_NOTES.iter().enumerate() {
        assert_eq!(s(&c["ROOT_NOTES"][i][0]), *k);
        assert_eq!(s(&c["ROOT_NOTES"][i][1]), *l);
    }
    let octaves: Vec<i64> = c["ROOT_OCTAVES"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_i64().unwrap())
        .collect();
    assert_eq!(octaves, ROOT_OCTAVES);
    assert_eq!(c["SCALES"].as_array().unwrap().len(), SCALES.len());
    for (i, sc) in SCALES.iter().enumerate() {
        let want = &c["SCALES"][i];
        assert_eq!(s(&want[0]), sc.key);
        assert_eq!(s(&want[1]), sc.label);
        let steps: Vec<f64> = want[2].as_array().unwrap().iter().map(f).collect();
        assert_eq!(steps, sc.steps, "{}", sc.key);
    }
    for (name, list) in [
        ("TONE_PARTIALS", &TONE_PARTIALS[..]),
        ("TONE_PARTIALS_SPORT", &TONE_PARTIALS_SPORT[..]),
    ] {
        let want = c[name].as_array().unwrap();
        assert_eq!(want.len(), list.len());
        for ((m, a), w) in list.iter().zip(want) {
            assert_eq!(w[0].as_i64(), Some(*m));
            assert!(same_f64(*a, f(&w[1])));
        }
    }
    for (name, v) in [
        ("DEFAULT_LEVEL_DB", DEFAULT_LEVEL_DB),
        ("TONE_ATTACK_MS", TONE_ATTACK_MS),
        ("TONE_HOLD_MS", TONE_HOLD_MS),
        ("TONE_DECAY_MS", TONE_DECAY_MS),
        ("TONE_MIN_HZ", TONE_MIN_HZ),
        ("TONE_MAX_HZ", TONE_MAX_HZ),
    ] {
        assert!(same_f64(v, f(&c["floats"][name])), "{name}");
    }
    for (name, v) in [
        ("DEFAULT_MODE", DEFAULT_MODE),
        ("DEFAULT_NUMBER_STYLE", DEFAULT_NUMBER_STYLE),
        ("DEFAULT_ROOT", DEFAULT_ROOT),
        ("DEFAULT_SCALE", DEFAULT_SCALE),
    ] {
        assert_eq!(s(&c["strs"][name]), v);
    }
    assert_eq!(c["DEFAULT_OCTAVE"].as_i64(), Some(DEFAULT_OCTAVE));
}

#[test]
fn phrases_match_python() {
    let fx = load("gear_report");
    let mut failures = Vec::new();
    for case in fx["phrases"].as_array().unwrap() {
        let g = s(&case["gear"]);
        if NON_ASCII_DIGITS.contains(&g) {
            continue;
        }
        let st = s(&case["style"]);
        let got = extended_gear_to_phrase(g, st);
        if got != s(&case["phrase"]) {
            failures.push(format!(
                "extended_gear_to_phrase({g:?}, {st}) = {got:?}, want {case}"
            ));
        }
    }
    for case in fx["singles"].as_array().unwrap() {
        let g = s(&case["gear"]);
        if NON_ASCII_DIGITS.contains(&g) {
            continue;
        }
        if is_sport(g) != case["is_sport"].as_bool().unwrap() {
            failures.push(format!("is_sport({g:?})"));
        }
        let got = extended_gear_degree(g);
        let want = &case["degree"];
        let ok = if let Some(big) = want.get("big") {
            // Beyond i64: saturates (documented).
            got == Some(if s(big).starts_with('-') {
                i64::MIN
            } else {
                i64::MAX
            })
        } else {
            got == want["ok"].as_i64()
        };
        if !ok {
            failures.push(format!(
                "extended_gear_degree({g:?}) = {got:?}, want {want}"
            ));
        }
    }
    for case in fx["bytes"].as_array().unwrap() {
        let b = case["byte"].as_i64().unwrap();
        let st = s(&case["style"]);
        if gear_to_phrase(b, st) != s(&case["phrase"])
            || outgauge_change_phrase(b, st).as_deref() != opt_str(&case["change"])
            || outgauge_gear_degree(b) != case["degree"].as_i64()
        {
            failures.push(format!("outgauge byte {b} {st}: want {case}"));
        }
    }
    for case in fx["pairs"].as_array().unwrap() {
        let g = s(&case["gear"]);
        let p = case["prev"].as_str();
        let collapse = case["collapse"].as_bool().unwrap();
        let st = s(&case["style"]);
        let got = extended_change_phrase(g, p, collapse, st);
        if got.as_deref() != opt_str(&case["phrase"]) {
            failures.push(format!("extended_change_phrase {case}: got {got:?}"));
        }
        // Python passes None through `(gear_str or "")`.
        let silenced = sport_step_silenced(g, p.unwrap_or(""), collapse);
        if silenced != case["silenced"].as_bool().unwrap() {
            failures.push(format!("sport_step_silenced {case}: got {silenced}"));
        }
    }
    check(failures);
}

#[test]
fn pitch_matches_python() {
    let fx = load("gear_report");
    let mut failures = Vec::new();
    for case in fx["notes"].as_array().unwrap() {
        let n = s(&case["name"]);
        if note_semitone(n) != case["semitone"].as_i64() {
            failures.push(format!(
                "note_semitone({n:?}) = {:?}, want {}",
                note_semitone(n),
                case
            ));
        }
    }
    for case in fx["semitones"].as_array().unwrap() {
        let d = case["degree"].as_i64().unwrap();
        let sc = s(&case["scale"]);
        let got = degree_semitones(d, sc);
        if !same_f64(got, f(&case["semitones"])) {
            failures.push(format!("degree_semitones({d}, {sc}) = {got}"));
        }
    }
    for case in fx["frequencies"].as_array().unwrap() {
        let root = s(&case["root"]);
        let octave = case["octave"].as_i64().unwrap();
        let got = root_frequency(root, octave);
        if !same_f64(got, f(&case["root_hz"])) {
            failures.push(format!(
                "root_frequency({root:?}, {octave}) = {got:?}, want {}",
                case["root_hz"]
            ));
        }
        let mut i = 0;
        for d in [-40, -2, -1, 0, 1, 5, 9, 40] {
            for sc in ["major", "quarter_tone", "rast"] {
                let got = degree_frequency(d, root, octave, sc);
                let want = f(&case["degrees"][i]);
                if !same_f64(got, want) {
                    failures.push(format!(
                        "degree_frequency({d}, {root:?}, {octave}, {sc}) = {got:?}, want {want:?}"
                    ));
                }
                i += 1;
            }
        }
    }
    check(failures);
}

#[test]
fn config_matches_python() {
    let fx = load("gear_report");
    let mut failures = Vec::new();
    for case in fx["settings"].as_array().unwrap() {
        let cfg = match json_loads(s(&case["cfg"])).unwrap() {
            PyValue::Dict(d) => d,
            _ => unreachable!(),
        };
        let got = settings_from_config(&cfg);
        let ok = match (&got, case.get("ok")) {
            (Ok(g), Some(w)) => {
                g.mode == s(&w["mode"])
                    && g.collapse_sport == w["collapse_sport"].as_bool().unwrap()
                    && g.number_style == s(&w["number_style"])
                    && g.root == s(&w["root"])
                    && Some(g.octave) == w["octave"].as_i64()
                    && g.scale == s(&w["scale"])
                    && same_f64(g.level_db, f(&w["level_db"]))
            }
            (Err(e), None) => e.to_string() == s(&case["err"]),
            _ => false,
        };
        if !ok {
            failures.push(format!(
                "settings_from_config({}) = {got:?}, want {case}",
                case["cfg"]
            ));
        }
    }
    for case in fx["migrations"].as_array().unwrap() {
        let mut cfg = match json_loads(s(&case["before"])).unwrap() {
            PyValue::Dict(d) => d,
            _ => unreachable!(),
        };
        let changed = migrate_config(&mut cfg);
        let want = match json_loads(s(&case["after"])).unwrap() {
            PyValue::Dict(d) => d,
            _ => unreachable!(),
        };
        let same_order = cfg.keys().eq(want.keys());
        if changed != case["changed"].as_bool().unwrap() || cfg != want || !same_order {
            failures.push(format!(
                "migrate_config {case}: got {cfg:?} changed {changed}"
            ));
        }
    }
    check(failures);
}

fn hex_f32(hex: &str) -> Vec<f32> {
    (0..hex.len())
        .step_by(8)
        .map(|i| {
            let b = u32::from_str_radix(&hex[i..i + 8], 16)
                .unwrap()
                .to_be_bytes();
            f32::from_le_bytes(b)
        })
        .collect()
}

fn hex_f64(hex: &str) -> Vec<f64> {
    (0..hex.len())
        .step_by(16)
        .map(|i| {
            let b = u64::from_str_radix(&hex[i..i + 16], 16)
                .unwrap()
                .to_be_bytes();
            f64::from_le_bytes(b)
        })
        .collect()
}

/// On the generating machine both are bit-identical. The tolerances (1e-15 for the
/// float64 envelope, 1e-6 of full scale for the float32 tone, about 8 ulps at the peak)
/// only leave room for a different libm or numpy build rounding sin/cos/pow differently.
#[test]
fn tone_and_envelope_match_python() {
    let fx = load("gear_report");
    let mut failures = Vec::new();
    for case in fx["envelopes"].as_array().unwrap() {
        let sr = case["sr"].as_u64().unwrap() as u32;
        let want = hex_f64(s(&case["f64_hex"]));
        let got = tone_envelope(sr);
        if got.len() != want.len() {
            failures.push(format!(
                "envelope {sr}: len {} want {}",
                got.len(),
                want.len()
            ));
            continue;
        }
        let worst = got
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f64, f64::max);
        if worst > 1e-15 {
            failures.push(format!("envelope {sr}: max error {worst:e}"));
        }
    }
    for case in fx["tones"].as_array().unwrap() {
        let freq = f(&case["freq"]);
        let db = f(&case["db"]);
        let sr = case["sr"].as_u64().unwrap() as u32;
        let sport = case["sport"].as_bool().unwrap();
        let want = hex_f32(s(&case["f32_hex"]));
        let got = render_tone(freq, db, sr, sport);
        if got.len() != want.len() {
            failures.push(format!(
                "tone {freq} {db} {sr} {sport}: len {} want {}",
                got.len(),
                want.len()
            ));
            continue;
        }
        let worst = got
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        if worst > 1e-6 {
            failures.push(format!(
                "tone {freq} {db} {sr} {sport}: max error {worst:e}"
            ));
        }
    }
    check(failures);
}
