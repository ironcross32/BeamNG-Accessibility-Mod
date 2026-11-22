//! Owner: EA. Units and formatting, gear-free bearing helpers (`beamtel.py:4148-4349`).
//!
//! Every `fmt_*` returns Python's `(value, unit)` pair. The value keeps its Python TYPE in
//! [`PyVal`], because callers splice it into speech with `f"{val} {unit}"` and Python's
//! `str()` of an int, a float and a str differ (`3` vs `3.0` vs `"3.0"`).
//!
//! Where the Python raises (`int(round(nan))`), these functions panic with the Python
//! exception's message; see the engine map, section 1.

use std::fmt;
use std::sync::LazyLock;

use beam_base::pyfmt::{py_f, py_float_str, py_mod, py_round};
use parking_lot::RwLock;

use crate::state::State;

pub const MPH_PER_MS: f64 = 2.2369362920544;
pub const KMH_PER_MS: f64 = 3.6;
pub const PSI_PER_BAR: f64 = 14.503773773;
pub const FEET_PER_M: f64 = 3.280839895;
pub const HP_PER_KW: f64 = 1.341022090;
pub const LBFT_PER_NM: f64 = 0.737562149;
pub const MILES_PER_M: f64 = 0.000621371192;

/// `COMPASS_NAMES`.
pub const COMPASS_NAMES: [&str; 8] = [
    "north",
    "northeast",
    "east",
    "southeast",
    "south",
    "southwest",
    "west",
    "northwest",
];

// Bit flags the has_* helpers test (beamtel.py:703-712). E1's telemetry module re-uses them.
pub const TELEMETRY_HAS_CLUTCH: i64 = 1 << 6;
pub const TELEMETRY_HAS_ENGINE_TEMP: i64 = 1 << 7;
pub const TELEMETRY_HAS_ELECTRIC: i64 = 1 << 8;
pub const TELEMETRY_HAS_AIR: i64 = 1 << 9;

static UNITS_MODE: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("imperial".to_string()));

/// The `UNITS_MODE` global ("imperial" or "metric"; whatever the config said, in fact).
pub fn units_mode() -> String {
    UNITS_MODE.read().clone()
}

/// `UNITS_MODE = value`.
pub fn set_units_mode(value: &str) {
    *UNITS_MODE.write() = value.to_string();
}

fn is_metric() -> bool {
    *UNITS_MODE.read() == "metric"
}

/// A Python scalar as `str()` prints it.
#[derive(Debug, Clone, PartialEq)]
pub enum PyVal {
    Int(i64),
    Float(f64),
    Str(String),
}

impl fmt::Display for PyVal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PyVal::Int(n) => write!(f, "{n}"),
            PyVal::Float(x) => f.write_str(&py_float_str(*x)),
            PyVal::Str(s) => f.write_str(s),
        }
    }
}

/// Python's `int(round(x))`. Panics where Python raises.
pub fn py_int_round(x: f64) -> i64 {
    py_int(x.round_ties_even())
}

/// Python's `int(x)` for a float. Panics where Python raises.
pub fn py_int(x: f64) -> i64 {
    if x.is_nan() {
        panic!("ValueError: cannot convert float NaN to integer");
    }
    if x.is_infinite() {
        panic!("OverflowError: cannot convert float infinity to integer");
    }
    x.trunc() as i64
}

/// Python's float `x // y` (CPython's `float_floor_div`).
pub fn py_floordiv(x: f64, y: f64) -> f64 {
    if y == 0.0 {
        panic!("ZeroDivisionError: float floor division by zero");
    }
    let m = x % y;
    let mut div = (x - m) / y;
    if m != 0.0 && ((y < 0.0) != (m < 0.0)) {
        div -= 1.0;
    }
    if div != 0.0 {
        let mut floordiv = div.floor();
        if div - floordiv > 0.5 {
            floordiv += 1.0;
        }
        floordiv
    } else {
        0.0f64.copysign(x / y)
    }
}

/// `f"{n:,}"`.
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

pub fn fmt_speed(speed_ms: f64) -> (PyVal, &'static str) {
    if is_metric() {
        (PyVal::Int(py_int_round(speed_ms * KMH_PER_MS)), "km/h")
    } else {
        (PyVal::Int(py_int_round(speed_ms * MPH_PER_MS)), "mph")
    }
}

pub fn fmt_pressure(psi_val: f64) -> (PyVal, &'static str) {
    if is_metric() {
        (PyVal::Float(py_round(psi_val / PSI_PER_BAR, 2)), "bar")
    } else {
        (PyVal::Float(py_round(psi_val, 1)), "psi")
    }
}

pub fn fmt_turbo(bar_val: f64) -> (PyVal, &'static str) {
    if is_metric() {
        (PyVal::Float(py_round(bar_val, 2)), "bar")
    } else {
        (PyVal::Float(py_round(bar_val * PSI_PER_BAR, 1)), "psi")
    }
}

pub fn fmt_temp_c_or_f(celsius: f64) -> (PyVal, &'static str) {
    if is_metric() {
        (PyVal::Int(py_int_round(celsius)), "Celsius")
    } else {
        let f = (celsius * 9.0 / 5.0) + 32.0;
        (PyVal::Int(py_int_round(f)), "Fahrenheit")
    }
}

/// Short distances (implement height, clearance).
pub fn fmt_distance(metres: f64) -> (PyVal, &'static str) {
    if is_metric() {
        (PyVal::Float(py_round(metres, 2)), "meters")
    } else {
        (PyVal::Float(py_round(metres * FEET_PER_M, 1)), "feet")
    }
}

pub fn fmt_power_kw(kw: f64) -> (PyVal, &'static str) {
    if is_metric() {
        (PyVal::Int(py_int_round(kw)), "kilowatts")
    } else {
        (PyVal::Int(py_int_round(kw * HP_PER_KW)), "horsepower")
    }
}

pub fn fmt_torque_nm(nm: f64) -> (PyVal, &'static str) {
    if is_metric() {
        (PyVal::Int(py_int_round(nm)), "newton meters")
    } else {
        (PyVal::Int(py_int_round(nm * LBFT_PER_NM)), "pound feet")
    }
}

pub fn fmt_range(metres: f64) -> (PyVal, &'static str) {
    if is_metric() {
        (PyVal::Int(py_int_round(metres / 1000.0)), "kilometers")
    } else {
        (PyVal::Int(py_int_round(metres * MILES_PER_M)), "miles")
    }
}

/// Trip distance: tenths of a mile or kilometre, short distances in feet or meters.
pub fn fmt_trip(metres: f64) -> (PyVal, &'static str) {
    if is_metric() {
        if metres < 100.0 {
            return (PyVal::Int(py_int_round(metres)), "meters");
        }
        return (PyVal::Str(py_f(metres / 1000.0, 1)), "kilometers");
    }
    let miles = metres * MILES_PER_M;
    if miles < 0.1 {
        return (PyVal::Int(py_int_round(metres * FEET_PER_M)), "feet");
    }
    (PyVal::Str(py_f(miles, 1)), "miles")
}

/// Whole miles or kilometres, with tenths below ten.
pub fn fmt_odometer(metres: f64) -> (PyVal, &'static str) {
    let metric = is_metric();
    let dist = if metric {
        metres / 1000.0
    } else {
        metres * MILES_PER_M
    };
    let unit = if metric { "kilometers" } else { "miles" };
    if dist < 10.0 {
        return (PyVal::Str(py_f(dist, 1)), unit);
    }
    (PyVal::Str(thousands(py_int_round(dist))), unit)
}

pub fn has_air_system(st: &State) -> bool {
    st.tel.protocol_mode == "extended" && (st.tel.last_telemetry_presence & TELEMETRY_HAS_AIR) != 0
}

/// The odometer exceeds the trip only on a vehicle carrying mileage of its own (career).
pub fn has_stored_mileage(st: &State) -> bool {
    st.tel.last_trip_m >= 0.0 && st.tel.last_odometer_m - st.tel.last_trip_m > 100.0
}

/// OutGauge has no presence mask, so it keeps reading the temperatures as before.
pub fn has_engine_temp(st: &State) -> bool {
    st.tel.protocol_mode != "extended"
        || (st.tel.last_telemetry_presence & TELEMETRY_HAS_ENGINE_TEMP) != 0
}

pub fn has_electric_drive(st: &State) -> bool {
    st.tel.protocol_mode == "extended"
        && (st.tel.last_telemetry_presence & TELEMETRY_HAS_ELECTRIC) != 0
}

pub fn fmt_heading(st: &State) -> String {
    format!("{} degrees", py_f(st.tel.last_heading, 1))
}

pub fn flip_units() {
    let mut u = UNITS_MODE.write();
    *u = if *u == "imperial" {
        "metric".to_string()
    } else {
        "imperial".to_string()
    };
}

/// `get_speed_bucket(speed_ms)`, reading `speed_announce_interval` from the state.
pub fn get_speed_bucket(st: &State, speed_ms: f64) -> i64 {
    get_speed_bucket_with(speed_ms, st.cfg.speed_announce_interval)
}

/// The body of `get_speed_bucket` with the interval passed in.
pub fn get_speed_bucket_with(speed_ms: f64, speed_announce_interval: f64) -> i64 {
    if speed_ms < 0.0 {
        return 0;
    }
    // max(1, interval): Python keeps the first argument on a tie, which is the same number.
    let interval = if 1.0 >= speed_announce_interval {
        1.0
    } else {
        speed_announce_interval
    };
    if is_metric() {
        py_int(py_floordiv(speed_ms * KMH_PER_MS, interval))
    } else {
        py_int(py_floordiv(speed_ms * MPH_PER_MS, interval))
    }
}

pub fn yaw_to_heading_deg(yaw_rad: f64) -> f64 {
    py_mod(yaw_rad.to_degrees(), 360.0)
}

/// The camera listener's 8-way zone test (`beamtel.py:3369-3387`): the index of the named
/// direction whose +/-3 degree window contains `heading`, or -1.
pub fn compass_zone_index(heading: f64) -> i64 {
    let half_width = 3.0;
    for i in 0..8 {
        let target = i as f64 * 45.0;
        let lower = py_mod(target - half_width + 360.0, 360.0);
        let upper = py_mod(target + half_width, 360.0);
        let inside = if lower < upper {
            lower <= heading && heading < upper
        } else {
            heading >= lower || heading < upper
        };
        if inside {
            return i;
        }
    }
    -1
}

/// The telemetry loop's snapped-heading match (`beamtel.py:10797-10806`): the index of the
/// named direction within 0.5 degrees of `snapped`, or -1.
pub fn compass_snap_match(snapped: f64) -> i64 {
    for i in 0..8 {
        let target_deg = i as f64 * 45.0;
        let diff = (py_mod(snapped - target_deg + 180.0, 360.0) - 180.0).abs();
        if diff < 0.5 {
            return i;
        }
    }
    -1
}

/// Serialises tests that flip the process-wide units mode.
#[cfg(test)]
pub(crate) static TEST_UNITS_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use beam_testutil::{f, load, same_f64};
    use serde_json::Value;

    fn check_val(case: &str, got: &PyVal, want: &Value) {
        if let Some(n) = want.get("int") {
            assert_eq!(got, &PyVal::Int(n.as_i64().unwrap()), "{case}");
        } else if let Some(x) = want.get("float") {
            match got {
                PyVal::Float(g) => assert!(same_f64(*g, f(x)), "{case}: {g} vs {}", f(x)),
                other => panic!("{case}: want float, got {other:?}"),
            }
        } else if let Some(s) = want.get("str") {
            assert_eq!(got, &PyVal::Str(s.as_str().unwrap().to_string()), "{case}");
        } else {
            panic!("{case}: bad fixture value");
        }
    }

    fn run_fmt(name: &str, x: f64) -> (PyVal, &'static str) {
        match name {
            "fmt_speed" => fmt_speed(x),
            "fmt_pressure" => fmt_pressure(x),
            "fmt_turbo" => fmt_turbo(x),
            "fmt_temp_c_or_f" => fmt_temp_c_or_f(x),
            "fmt_distance" => fmt_distance(x),
            "fmt_power_kw" => fmt_power_kw(x),
            "fmt_torque_nm" => fmt_torque_nm(x),
            "fmt_range" => fmt_range(x),
            "fmt_trip" => fmt_trip(x),
            "fmt_odometer" => fmt_odometer(x),
            other => panic!("unknown fn {other}"),
        }
    }

    #[test]
    fn fmt_functions_match_python() {
        let _g = TEST_UNITS_LOCK.lock();
        let fx = load("units");
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        for c in fx["fmt"].as_array().unwrap() {
            let name = c["fn"].as_str().unwrap();
            let units = c["units"].as_str().unwrap();
            let x = f(&c["x"]);
            let case = format!("{name}({x:?}) [{units}]");
            set_units_mode(units);
            let out = &c["out"];
            let got = std::panic::catch_unwind(|| run_fmt(name, x));
            if let Some(err) = out.get("err") {
                let msg = match got {
                    Ok(v) => panic!("{case}: Python raised {err}, Rust gave {v:?}"),
                    Err(p) => p
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_default(),
                };
                assert!(msg.starts_with(err.as_str().unwrap()), "{case}: {msg}");
            } else {
                let (val, unit) = got.unwrap_or_else(|_| panic!("{case}: Rust panicked"));
                assert_eq!(unit, out["unit"].as_str().unwrap(), "{case}");
                check_val(&case, &val, &out["val"]);
                assert_eq!(
                    format!("{val} {unit}"),
                    out["text"].as_str().unwrap(),
                    "{case}"
                );
            }
        }
        std::panic::set_hook(prev);
        set_units_mode("imperial");
    }

    #[test]
    fn speed_buckets_match_python() {
        let _g = TEST_UNITS_LOCK.lock();
        let fx = load("units");
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        for c in fx["buckets"].as_array().unwrap() {
            let units = c["units"].as_str().unwrap();
            let interval = c["interval"].as_f64().unwrap();
            let x = f(&c["x"]);
            set_units_mode(units);
            let got = std::panic::catch_unwind(|| get_speed_bucket_with(x, interval));
            let case = format!("bucket({x:?}, {interval}) [{units}]");
            match c["out"].get("ok") {
                Some(v) => assert_eq!(got.ok(), v.as_i64(), "{case}"),
                None => assert!(got.is_err(), "{case}: Python raised"),
            }
        }
        std::panic::set_hook(prev);
        set_units_mode("imperial");
    }

    #[test]
    fn yaw_heading_caps_and_flips_match_python() {
        let _g = TEST_UNITS_LOCK.lock();
        let fx = load("units");
        for c in fx["yaw"].as_array().unwrap() {
            let x = f(&c["x"]);
            let got = yaw_to_heading_deg(x);
            assert!(same_f64(got, f(&c["out"])), "yaw {x:?}: {got:?}");
        }
        let mut st = State::default();
        for c in fx["heading"].as_array().unwrap() {
            st.tel.last_heading = f(&c["x"]);
            assert_eq!(fmt_heading(&st), c["out"].as_str().unwrap());
        }
        for c in fx["caps"].as_array().unwrap() {
            st.tel.protocol_mode = c["protocol"].as_str().unwrap().to_string();
            st.tel.last_telemetry_presence = c["presence"].as_i64().unwrap();
            st.tel.last_trip_m = f(&c["trip"]);
            st.tel.last_odometer_m = f(&c["odo"]);
            let case = format!("{c}");
            assert_eq!(has_air_system(&st), c["air"].as_bool().unwrap(), "{case}");
            assert_eq!(
                has_stored_mileage(&st),
                c["mileage"].as_bool().unwrap(),
                "{case}"
            );
            assert_eq!(
                has_engine_temp(&st),
                c["engine_temp"].as_bool().unwrap(),
                "{case}"
            );
            assert_eq!(
                has_electric_drive(&st),
                c["electric"].as_bool().unwrap(),
                "{case}"
            );
        }
        set_units_mode("imperial");
        let flips: Vec<String> = (0..3)
            .map(|_| {
                flip_units();
                units_mode()
            })
            .collect();
        let want: Vec<String> = fx["flips"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(flips, want);
        set_units_mode("imperial");
        let names: Vec<&str> = fx["compass_names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(names, COMPASS_NAMES);
        let consts = &fx["consts"];
        assert!(same_f64(MPH_PER_MS, f(&consts["MPH_PER_MS"])));
        assert!(same_f64(KMH_PER_MS, f(&consts["KMH_PER_MS"])));
        assert!(same_f64(PSI_PER_BAR, f(&consts["PSI_PER_BAR"])));
        assert!(same_f64(FEET_PER_M, f(&consts["FEET_PER_M"])));
        assert!(same_f64(HP_PER_KW, f(&consts["HP_PER_KW"])));
        assert!(same_f64(LBFT_PER_NM, f(&consts["LBFT_PER_NM"])));
        assert!(same_f64(MILES_PER_M, f(&consts["MILES_PER_M"])));
    }

    #[test]
    fn compass_helpers() {
        assert_eq!(compass_zone_index(0.0), 0);
        assert_eq!(compass_zone_index(358.0), 0);
        assert_eq!(compass_zone_index(2.99), 0);
        assert_eq!(compass_zone_index(3.0), -1);
        assert_eq!(compass_zone_index(42.0), 1);
        assert_eq!(compass_zone_index(48.0), -1);
        assert_eq!(compass_zone_index(315.0), 7);
        assert_eq!(compass_snap_match(0.0), 0);
        assert_eq!(compass_snap_match(359.6), 0);
        assert_eq!(compass_snap_match(90.0), 2);
        assert_eq!(compass_snap_match(15.0), -1);
        assert_eq!(compass_snap_match(-45.0), 7);
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(10), "10");
        assert_eq!(thousands(-1000), "-1,000");
    }
}
