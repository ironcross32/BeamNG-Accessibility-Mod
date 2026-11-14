//! The F9 Alt+T forced induction report: what is spoken for one reply from the game.
//!
//! vehicleScanner.lua sends raw figures as JSON and the engine owns the socket, so the
//! sim can assert the wording with no game.
//!
//! The report is the dict `json.loads` produced, and every rule here is Python's dynamic
//! one applied to whatever the game sent (truthiness, `float()`, `str()`), through
//! [`crate::pyops`].

use crate::pyops::{
    PyDict, PyErr, PyErrKind, PyValue, group_thousands, int_text_add, int_text_sign, json_loads,
    py_round_to_int,
};

pub const REPLY_PREFIX: &str = "FI:";

/// `TYPE_NAMES`: the supercharger types this file names.
pub const TYPE_NAMES: [(&str, &str); 3] = [
    ("roots", "Roots"),
    ("screws", "Screw"),
    ("centrifugal", "Centrifugal"),
];

/// The figures from one packet, or `None` when it is not a usable report.
///
/// Python catches only `ValueError` from `json.loads`; nesting too deep for the decoder
/// raises `RecursionError` there, which escaped to the report thread. That case is
/// returned as `Err`.
pub fn parse_reply(text: &str) -> Result<Option<PyDict>, PyErr> {
    let Some(body) = text.strip_prefix(REPLY_PREFIX) else {
        return Ok(None);
    };
    match json_loads(body) {
        Ok(PyValue::Dict(d)) => Ok(Some(d)),
        Ok(_) => Ok(None),
        Err(e) if e.kind == PyErrKind::ValueError => Ok(None),
        Err(e) => Err(e),
    }
}

/// `int(round(float(coef) * 100.0))`, as exact decimal text (Python's int is unbounded).
fn percent(coef: &PyValue) -> Result<String, PyErr> {
    py_round_to_int(coef.py_float()? * 100.0)
}

fn percent_f(value: f64) -> Result<String, PyErr> {
    percent(&PyValue::Float(value))
}

/// `_percent(x) - 100`.
fn gain(text: &str) -> String {
    int_text_add(text, "-100")
}

/// `f"{int(round(float(value))):,}"`, exact for any magnitude.
fn rpm(value: &PyValue) -> Result<String, PyErr> {
    Ok(group_thousands(&py_round_to_int(value.py_float()?)?))
}

fn get<'a>(charger: &'a PyValue, key: &str) -> Result<Option<&'a PyValue>, PyErr> {
    charger.dict_get(key)
}

fn get_or<'a>(charger: &'a PyValue, key: &str, default: &'a PyValue) -> Result<&'a PyValue, PyErr> {
    Ok(charger.dict_get(key)?.unwrap_or(default))
}

fn truthy(v: Option<&PyValue>) -> bool {
    v.is_some_and(PyValue::truthy)
}

/// The live multiplier as a change. Idling it is below 1: blower drag, or a turbo in
/// vacuum.
fn torque_phrase(coef: &PyValue) -> Result<String, PyErr> {
    let change = gain(&percent(coef)?);
    match int_text_sign(&change) {
        1 => return Ok(format!("Engine torque up {change} percent")),
        -1 => {
            return Ok(format!(
                "Engine torque down {} percent",
                change.trim_start_matches('-')
            ));
        }
        _ => {}
    }
    Ok("Engine torque unchanged".to_string())
}

/// Absent on an engine with no torque curve entries; never read out as a gain of
/// nothing.
fn best_gain(charger: &PyValue, about: &str) -> Result<Vec<String>, PyErr> {
    let peak_coef = get(charger, "peakCoef")?;
    let peak_rpm = get(charger, "peakRPM")?;
    let (Some(peak_coef), Some(peak_rpm)) = (peak_coef, peak_rpm) else {
        return Ok(Vec::new());
    };
    if peak_coef.is_none() || peak_rpm.is_none() {
        return Ok(Vec::new());
    }
    let gain = gain(&percent(peak_coef)?);
    Ok(vec![format!(
        "Best gain {about}{gain} percent at {} RPM",
        rpm(peak_rpm)?
    )])
}

/// `fmt_pressure(value)`: the caller's units. Python hands the raw JSON value to the
/// caller's function, whose first act is `float(value)`; here the `float()` is done
/// first and the closure gets the number. It returns `(str(value), unit)`.
type FmtPressure<'a> = &'a dyn Fn(f64) -> (String, String);

fn pressure(fmt_pressure: FmtPressure, v: &PyValue) -> Result<(String, String), PyErr> {
    Ok(fmt_pressure(v.py_float()?))
}

/// (what changes while driving, what describes the part), so torque can sit between.
fn supercharger_parts(
    charger: &PyValue,
    fmt_pressure: FmtPressure,
) -> Result<(Vec<String>, Vec<String>), PyErr> {
    let zero = PyValue::Int("0".into());
    let one = PyValue::Int("1".into());
    let empty = PyValue::Str(String::new());
    let type_text = get_or(charger, "type", &empty)?.py_str().to_lowercase();
    let kind = TYPE_NAMES
        .iter()
        .find(|(k, _)| *k == type_text)
        .map(|(_, v)| *v);
    let name = match kind {
        Some(kind) => {
            let twisted = if kind == "Roots" && truthy(get(charger, "twisted")?) {
                " with twisted lobes"
            } else {
                ""
            };
            format!("{kind} supercharger{twisted}")
        }
        None => "Supercharger".to_string(),
    };
    let (boost, unit) = pressure(fmt_pressure, get_or(charger, "boost", &zero)?)?;
    let (boost_max, _) = pressure(fmt_pressure, get_or(charger, "boostMax", &zero)?)?;
    let live = vec![
        name,
        format!("Boost {boost} of {boost_max} {unit}"),
        format!("Blower {} RPM", rpm(get_or(charger, "blowerRPM", &zero)?)?),
        format!(
            "Blower drag {} percent",
            percent(get_or(charger, "lossCoef", &zero)?)?
        ),
    ];
    let mut spec = best_gain(charger, "")?;
    spec.push(format!(
        "Condition {} percent",
        percent(get_or(charger, "condition", &one)?)?
    ));
    Ok((live, spec))
}

/// As [`supercharger_parts`]. The best gain is the game's estimate, hence "about".
fn turbo_parts(
    charger: &PyValue,
    fmt_pressure: FmtPressure,
) -> Result<(Vec<String>, Vec<String>), PyErr> {
    let zero = PyValue::Int("0".into());
    let one = PyValue::Int("1".into());
    let mut fitted = Vec::new();
    for (key, label) in [("blowOff", "blow-off valve"), ("antilag", "anti-lag")] {
        if truthy(get(charger, key)?) {
            fitted.push(label);
        }
    }
    let name = if fitted.is_empty() {
        "Turbo".to_string()
    } else {
        format!("Turbo with {}", fitted.join(" and "))
    };
    let (boost, unit) = pressure(fmt_pressure, get_or(charger, "boost", &zero)?)?;
    let (boost_max, _) = pressure(fmt_pressure, get_or(charger, "boostMax", &zero)?)?;
    let (target, _) = pressure(fmt_pressure, get_or(charger, "wastegateTarget", &zero)?)?;
    let mut speed = format!(
        "Turbo speed {} RPM",
        rpm(get_or(charger, "turboRPM", &zero)?)?
    );
    if let Some(max_rpm) = get(charger, "maxTurboRPM")?.filter(|v| v.truthy()) {
        let share = div(
            get_or(charger, "turboRPM", &zero)?.py_float()?,
            max_rpm.py_float()?,
        )?;
        speed.push_str(&format!(", {} percent of maximum", percent_f(share)?));
    }
    let live = vec![
        name,
        format!("Boost {boost} of {boost_max} {unit}"),
        format!("Wastegate target {target} {unit} in this gear"),
        speed,
    ];
    let mut spec = best_gain(charger, "about ")?;
    let heat = get(charger, "heat")?;
    if let Some(heat) = heat.filter(|h| !h.is_none()) {
        if let Some(limit) = get(charger, "heatLimit")?.filter(|v| v.truthy()) {
            let share = div(heat.py_float()?, limit.py_float()?)?;
            spec.push(format!(
                "Heat {} percent of damage limit",
                percent_f(share)?
            ));
        }
    }
    if truthy(get(charger, "hot")?) {
        spec.push("Overheating".to_string());
    }
    spec.push(format!(
        "Condition {} percent",
        percent(get_or(charger, "condition", &one)?)?
    ));
    Ok((live, spec))
}

/// Float `/`: Python raises `ZeroDivisionError` for a zero divisor.
fn div(a: f64, b: f64) -> Result<f64, PyErr> {
    if b == 0.0 {
        return Err(PyErr::new(
            PyErrKind::ZeroDivisionError,
            "float division by zero",
        ));
    }
    Ok(a / b)
}

/// One spoken report. `fmt_pressure` takes PSI and returns `(value, unit)` in the user's
/// units, the value already formatted as the engine's `fmt_pressure` result would print.
///
/// `Err` where Python raises (a field that is not a number, a zero heat limit given as
/// text, a charger that is not a dict). In the Python that exception ended the report
/// thread, so nothing was spoken; the engine should do the same and log it.
pub fn report_phrase(
    report: &PyDict,
    fmt_pressure: &dyn Fn(f64) -> (String, String),
) -> Result<String, PyErr> {
    let mut chargers = Vec::new();
    if let Some(turbo) = report.get("turbo").filter(|v| v.truthy()) {
        chargers.push(turbo_parts(turbo, fmt_pressure)?);
    }
    if let Some(sc) = report.get("supercharger").filter(|v| v.truthy()) {
        chargers.push(supercharger_parts(sc, fmt_pressure)?);
    }

    let mut engine = Vec::new();
    if truthy(report.get("damaged")) {
        engine.push("Induction system damaged".to_string());
    }
    if let Some(two) = report.get("twoStep").filter(|v| !v.is_none()) {
        engine.push(format!(
            "Two-step {}",
            if two.truthy() { "on" } else { "off" }
        ));
    }
    let one = PyValue::Int("1".into());
    let torque = torque_phrase(report.get("torqueCoef").unwrap_or(&one))?;

    if chargers.len() == 1 {
        let (live, spec) = &chargers[0];
        let mut all = live.clone();
        all.push(torque);
        all.extend(spec.iter().cloned());
        all.extend(engine);
        return Ok(all.join(", "));
    }
    // The torque multiplier is the whole engine's, so a twin-charged car hears it once,
    // after both.
    let mut sections: Vec<String> = chargers
        .iter()
        .map(|(live, spec)| {
            let mut all = live.clone();
            all.extend(spec.iter().cloned());
            all.join(", ")
        })
        .collect();
    let mut tail = vec![torque];
    tail.extend(engine);
    sections.push(tail.join(", "));
    Ok(sections.join(". "))
}
