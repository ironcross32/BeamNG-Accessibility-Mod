//! E1: the telemetry formats and `telemetry_loop`, against `rust/fixtures/gen/telemetry.py`.
//!
//! * `constants_match_python`: every format string, size, mask and padding tuple.
//! * `decode_matches_python`: packets packed by Python's `struct`, classified and unpacked
//!   (with the loop's padding) by both sides.
//! * `loop_scenarios_match_python`: the real `beamtel.telemetry_loop` was driven packet by
//!   packet with recorders for speech, audio, the drift announcer and the UDP pushers; the
//!   same packets go through `TelemetryLoop::handle_datagram` here, and every call it makes
//!   and every global it writes must match after every packet (including where Python
//!   raised out of the loop).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};

use beam_audio::controller::TelemetryState;
use beam_engine::state::{ROUTE_STALE_SEC, State};
use beam_engine::telemetry::*;
use beam_engine::units;
use beam_reports::wheel_slip;
use beam_testutil::{f, load, same_f64};
use parking_lot::{Mutex, MutexGuard};
use serde_json::{Map, Value, json};

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("fixture hex"))
        .collect()
}

fn fb(x: f64) -> Value {
    json!({ "bits": x.to_bits() })
}

/// Fixture equality: floats bitwise (NaN == NaN), an int equal to a float of the same
/// value, objects by key set.
fn same(a: &Value, b: &Value) -> bool {
    let num = |v: &Value| -> Option<f64> {
        if let Some(bits) = v.get("bits") {
            return bits.as_u64().map(f64::from_bits);
        }
        if v.is_i64() || v.is_u64() {
            return v.as_f64();
        }
        None
    };
    let is_float = |v: &Value| v.get("bits").is_some();
    if is_float(a) || is_float(b) {
        return match (num(a), num(b)) {
            (Some(x), Some(y)) => same_f64(x, y) || (x == y && !(is_float(a) && is_float(b))),
            _ => false,
        };
    }
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w))
        }
        _ => a == b,
    }
}

// ---- constants ---------------------------------------------------------------------------

#[test]
fn constants_match_python() {
    let fx = load("telemetry");
    let c = &fx["consts"];
    let strs = [
        ("OG_FORMAT", OG_FORMAT),
        ("EXT_FORMAT_V1", EXT_FORMAT_V1),
        ("EXT_FORMAT_V2", EXT_FORMAT_V2),
        ("EXT_FORMAT_V3", EXT_FORMAT_V3),
        ("EXT_FORMAT_V4", EXT_FORMAT_V4),
        ("EXT_FORMAT_V5", EXT_FORMAT_V5),
        ("EXT_FORMAT", EXT_FORMAT),
        ("MS_FORMAT", MS_FORMAT),
    ];
    for (name, v) in strs {
        assert_eq!(c[name].as_str(), Some(v), "{name}");
    }
    let sizes = [
        ("OG_SIZE", OG_SIZE),
        ("EXT_SIZE_V1", EXT_SIZE_V1),
        ("EXT_SIZE_V2", EXT_SIZE_V2),
        ("EXT_SIZE_V3", EXT_SIZE_V3),
        ("EXT_SIZE_V4", EXT_SIZE_V4),
        ("EXT_SIZE_V5", EXT_SIZE_V5),
        ("EXT_SIZE", EXT_SIZE),
        ("MS_SIZE", MS_SIZE),
    ];
    for (name, v) in sizes {
        assert_eq!(c[name].as_u64(), Some(v as u64), "{name}");
    }
    assert_eq!(hex(c["MS_MAGIC"].as_str().unwrap()), MS_MAGIC.to_vec());
    let ints = [
        ("OG_TURBO", OG_TURBO),
        ("WHEEL_POS_FL", WHEEL_POS_FL),
        ("WHEEL_POS_FR", WHEEL_POS_FR),
        ("WHEEL_POS_RL", WHEEL_POS_RL),
        ("WHEEL_POS_RR", WHEEL_POS_RR),
        ("WHEEL_POS_F", WHEEL_POS_F),
        ("WHEEL_POS_R", WHEEL_POS_R),
        ("WHEEL_POS_CORNERS", WHEEL_POS_CORNERS),
        ("TELEMETRY_HAS_CLUTCH", TELEMETRY_HAS_CLUTCH),
        ("TELEMETRY_HAS_ENGINE_TEMP", TELEMETRY_HAS_ENGINE_TEMP),
        ("TELEMETRY_HAS_ELECTRIC", TELEMETRY_HAS_ELECTRIC),
        ("TELEMETRY_HAS_AIR", TELEMETRY_HAS_AIR),
        ("DL_SHIFT", DL_SHIFT),
        ("DL_FULLBEAM", DL_FULLBEAM),
        ("DL_HANDBRAKE", DL_HANDBRAKE),
        ("DL_TC", DL_TC),
        ("DL_SIGNAL_L", DL_SIGNAL_L),
        ("DL_SIGNAL_R", DL_SIGNAL_R),
        ("DL_CHECK", DL_CHECK),
        ("DL_OILWARN", DL_OILWARN),
        ("DL_BATTERY", DL_BATTERY),
        ("DL_ABS", DL_ABS),
        ("DL_LOWBEAM", DL_LOWBEAM),
    ];
    for (name, v) in ints {
        assert_eq!(c[name].as_i64(), Some(v), "{name}");
    }
    let floats = |name: &str| -> Vec<Value> { c[name].as_array().unwrap().clone() };
    let check = |name: &str, got: Vec<Value>| {
        assert!(
            same(&Value::Array(floats(name)), &Value::Array(got)),
            "{name}"
        );
    };
    check(
        "EXT_V3_SLIP_DEFAULTS",
        EXT_V3_SLIP_DEFAULTS.map(fb).to_vec(),
    );
    check(
        "EXT_V4_ELECTRIC_DEFAULTS",
        EXT_V4_ELECTRIC_DEFAULTS.map(fb).to_vec(),
    );
    check(
        "EXT_V5_DISTANCE_DEFAULTS",
        EXT_V5_DISTANCE_DEFAULTS.map(fb).to_vec(),
    );
    check(
        "EXT_V1_IMPLEMENT_DEFAULTS",
        EXT_V1_IMPLEMENT_DEFAULTS.map(fb).to_vec(),
    );
    let (fs, mask) = EXT_V2_WHEEL_DEFAULTS;
    let mut v2: Vec<Value> = fs.map(fb).to_vec();
    v2.push(json!(mask));
    // The mask must be an int on both sides, not merely an equal float.
    assert!(c["EXT_V2_WHEEL_DEFAULTS"][6].is_i64());
    check("EXT_V2_WHEEL_DEFAULTS", v2);
}

// ---- decode ------------------------------------------------------------------------------

fn field_json(p: &PyField) -> Value {
    match p {
        PyField::Int(n) => json!(n),
        PyField::Float(x) => fb(*x),
        PyField::Bytes(b) => {
            json!({ "bytes": b.iter().map(|x| format!("{x:02x}")).collect::<String>() })
        }
    }
}

#[test]
fn decode_matches_python() {
    let fx = load("telemetry");
    let cases = fx["decode"].as_array().unwrap();
    assert!(cases.len() > 50);
    for c in cases {
        let case = format!("{} [{}]", c["name"].as_str().unwrap(), c["mode"]);
        let data = hex(c["data"].as_str().unwrap());
        let got = decode_datagram(&data, c["mode"].as_str().unwrap());
        let (kind, fields) = match &got {
            Decoded::Ignored => ("ignored", None),
            Decoded::MotionSim(v) => ("motionsim", Some(v)),
            Decoded::Telemetry { layout, unpacked } => (
                match layout {
                    Layout::OutGauge => "outgauge",
                    Layout::ExtendedV1 => "ext1",
                    Layout::ExtendedV2 => "ext2",
                    Layout::ExtendedV3 => "ext3",
                    Layout::ExtendedV4 => "ext4",
                    Layout::ExtendedV5 => "ext5",
                    Layout::Extended => "ext6",
                },
                Some(unpacked),
            ),
        };
        assert_eq!(kind, c["kind"].as_str().unwrap(), "{case}");
        match fields {
            None => assert!(c["unpacked"].is_null(), "{case}"),
            Some(v) => {
                let want = c["unpacked"].as_array().unwrap();
                assert_eq!(v.len(), want.len(), "{case}: tuple length");
                for (i, (g, w)) in v.iter().zip(want).enumerate() {
                    let g = field_json(g);
                    // Strict on type: an int must stay an int, a float a float.
                    assert_eq!(g.is_i64(), w.is_i64(), "{case}[{i}]: {g} vs {w}");
                    assert!(same(&g, w), "{case}[{i}]: {g} vs {w}");
                }
            }
        }
    }
    // A layout's own warning names its size and what is lost.
    assert_eq!(ext_version_warning(Layout::Extended), None);
    assert_eq!(ext_version_warning(Layout::OutGauge), None);
    assert!(
        ext_version_warning(Layout::ExtendedV2)
            .unwrap()
            .starts_with("Extended telemetry packet is 196 bytes, expected 256.")
    );
}

// ---- the loop ----------------------------------------------------------------------------

struct TestHooks {
    st: Mutex<State>,
    calls: Mutex<Vec<Value>>,
    quiet: AtomicBool,
    debug: AtomicBool,
    baseline: AtomicBool,
    now: Mutex<f64>,
    mono: Mutex<f64>,
}

impl TestHooks {
    fn rec(&self, v: Value) {
        self.calls.lock().push(v);
    }
}

fn ts_json(s: &TelemetryState) -> Value {
    fn b(v: Option<bool>) -> Value {
        json!(v.expect("every key is sent"))
    }
    fn x(v: Option<f64>) -> Value {
        fb(v.expect("every key is sent"))
    }
    fn i(v: Option<i64>) -> Value {
        json!(v.expect("every key is sent"))
    }
    let mut m = Map::new();
    let mut put = |k: &str, v: Value| {
        m.insert(k.to_string(), v);
    };
    put("shift_active", b(s.shift_active));
    put("tc_active", b(s.tc_active));
    put("pedal_tones_active", b(s.pedal_tones_active));
    put("last_clutch", x(s.last_clutch));
    put("last_brake", x(s.last_brake));
    put("last_throttle", x(s.last_throttle));
    put("last_steering", x(s.last_steering));
    put("last_actual_steering", x(s.last_actual_steering));
    put("last_steering_input", x(s.last_steering_input));
    put("trailer_artic", x(s.trailer_artic));
    put("trailer_reverse", b(s.trailer_reverse));
    put("implement_flags", i(s.implement_flags));
    put("implement_min_clearance", x(s.implement_min_clearance));
    put("implement_tilt_deg", x(s.implement_tilt_deg));
    put("implement_lift", x(s.implement_lift));
    put("implement_activity", x(s.implement_activity));
    put("inverted", b(s.inverted));
    put("last_roll_rad", x(s.last_roll_rad));
    put("last_pitch_rad", x(s.last_pitch_rad));
    put("guidance_active", b(s.guidance_active));
    put("guidance_error_deg", x(s.guidance_error_deg));
    put("drift_alert_active", b(s.drift_alert_active));
    put("drift_pan", x(s.drift_pan));
    put("drift_rate", x(s.drift_rate));
    put("ls_clicks_active", b(s.ls_clicks_active));
    put("ls_speed_mph", x(s.ls_speed_mph));
    put("scan_speed_ms", x(s.scan_speed_ms));
    put("ls_accel", x(s.ls_accel));
    put("ls_stopped", b(s.ls_stopped));
    put("slip_active", b(s.slip_active));
    put("slip_kind", i(s.slip_kind));
    put("slip_mag", x(s.slip_mag));
    put("slip_ratio", x(s.slip_ratio));
    put("slide_active", b(s.slide_active));
    put("slide_deg", x(s.slide_deg));
    put("slip_azimuth_deg", x(s.slip_azimuth_deg));
    put("coord_guidance_active", b(s.coord_guidance_active));
    put("coord_guidance_error_deg", x(s.coord_guidance_error_deg));
    Value::Object(m)
}

impl TelemetryHooks for TestHooks {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock()
    }
    fn say(&self, text: &str, interrupt: bool, exclude_from_buffer: bool) {
        self.rec(json!(["say", text, interrupt, exclude_from_buffer]));
    }
    fn drift_quiet(&self) -> bool {
        self.quiet.load(Ordering::SeqCst)
    }
    fn take_telemetry_baseline_pending(&self) -> bool {
        self.baseline.swap(false, Ordering::SeqCst)
    }
    fn push_gear_direction(&self, _st: &mut State, gear: &str) {
        self.rec(json!(["push_gear", gear]));
    }
    fn push_obstacle_state(&self, _st: &mut State) {
        self.rec(json!(["push_obstacle"]));
    }
    fn bnvda_debug_enabled(&self) -> bool {
        self.debug.load(Ordering::SeqCst)
    }
    /// `_travel_bearing_or_none()` on the scripted clock.
    fn travel_bearing_or_none(&self, st: &State) -> Option<f64> {
        if (*self.now.lock() - st.tel.ms_last_rx_ts) > beam_engine::state::LS_MS_STALE_S {
            return None;
        }
        if st.tel.last_ground_speed_ms < wheel_slip::DIR_MIN_MS {
            return None;
        }
        Some(wheel_slip::travel_bearing(
            st.tel.last_vel_x,
            st.tel.last_vel_y,
            st.tel.last_heading,
        ))
    }
    /// `_route_is_set()` on the scripted monotonic clock.
    fn route_is_set(&self, st: &State) -> bool {
        if st.route.route_dest_x.is_none() {
            return false;
        }
        (*self.mono.lock() - st.route.route_beacon_stamp) <= ROUTE_STALE_SEC
    }
    fn trailer_artic_norm(&self, st: &State) -> f64 {
        beam_engine::listeners::trailer::trailer_artic_norm(st)
    }
    fn log(&self, level: log::Level, msg: &str) {
        let name = match level {
            log::Level::Warn => "WARNING",
            log::Level::Info => "INFO",
            other => panic!("unexpected log level {other}"),
        };
        self.rec(json!(["log", name, msg]));
    }
    fn gear_tone(&self, degree: i64, sport: bool) {
        self.rec(json!(["gear_tone", degree, sport]));
    }
    fn trigger_compass_highlight(&self, heading: f64, pitch_mult: f64) {
        self.rec(json!(["highlight", fb(heading), fb(pitch_mult)]));
    }
    fn trigger_compass_click(&self, heading: f64, pitch_mult: f64) {
        self.rec(json!(["click", fb(heading), fb(pitch_mult)]));
    }
    fn trigger_lowspeed_stopped(&self) {
        self.rec(json!(["lowspeed_stopped"]));
    }
    fn update_route_beacon(&self, available: bool, bearing_deg: f64, distance_m: f64) {
        self.rec(json!([
            "route_beacon",
            available,
            fb(bearing_deg),
            fb(distance_m)
        ]));
    }
    fn update_telemetry_state(&self, state: &TelemetryState) {
        self.rec(json!(["telemetry_state", ts_json(state)]));
    }
    fn trigger_oil_chime(&self) {
        self.rec(json!(["oil_chime"]));
    }
}

fn opt_f(v: &Value) -> Option<f64> {
    if v.is_null() { None } else { Some(f(v)) }
}

fn apply_set(st: &mut State, key: &str, v: &Value) {
    let b = || v.as_bool().unwrap_or_else(|| panic!("{key}: bool"));
    match key {
        "announce_turn_signals" => st.cfg.announce_turn_signals = b(),
        "announce_speed" => st.cfg.announce_speed = b(),
        "speed_announce_interval" => st.cfg.speed_announce_interval = f(v),
        "oil_chime_enabled" => st.cfg.oil_chime_enabled = b(),
        "compass_highlight_enabled" => st.cfg.compass_highlight_enabled = b(),
        "compass_highlight_nth_click" => st.cfg.compass_highlight_nth_click = v.as_i64().unwrap(),
        "compass_click_interval_deg" => st.cfg.compass_click_interval_deg = f(v),
        "compass_min_interval" => st.tel.compass_min_interval = f(v),
        "cooldown_sec" => st.tel.cooldown_sec = f(v),
        "cannon_active" => st.dock.cannon_active = b(),
        "drift_mode_active" => st.tel.drift_mode_active = b(),
        "low_speed_mode_active" => st.tel.low_speed_mode_active = b(),
        "slip_mode_active" => st.tel.slip_mode_active = b(),
        "pedal_tones_active" => st.tel.pedal_tones_active = b(),
        "heading_guidance_active" => st.tel.heading_guidance_active = b(),
        "heading_guidance_target" => st.tel.heading_guidance_target = f(v),
        "coord_guidance_active" => st.tel.coord_guidance_active = b(),
        "marked_coord_x" => st.tel.marked_coord_x = opt_f(v),
        "marked_coord_y" => st.tel.marked_coord_y = opt_f(v),
        "route_beacon_active" => st.route.route_beacon_active = b(),
        "route_dest_x" => st.route.route_dest_x = opt_f(v),
        "route_dest_y" => st.route.route_dest_y = opt_f(v),
        "route_beacon_stamp" => st.route.route_beacon_stamp = f(v),
        "UNITS_MODE" => units::set_units_mode(v.as_str().unwrap()),
        "gear_mode" => st.cfg.gear_report_settings.mode = v.as_str().unwrap().to_string(),
        "collapse_sport" => st.cfg.gear_report_settings.collapse_sport = b(),
        "number_style" => {
            st.cfg.gear_report_settings.number_style = v.as_str().unwrap().to_string()
        }
        other => panic!("fixture sets unknown key {other}"),
    }
}

/// A global by its Python name, encoded as the generator encodes it.
fn get(st: &State, name: &str) -> Value {
    let t = &st.tel;
    let x = |v: f64| fb(v);
    match name {
        "last_speed_ms" => x(t.last_speed_ms),
        "last_rpm" => x(t.last_rpm),
        "last_fuel" => x(t.last_fuel),
        "last_turbo" => x(t.last_turbo),
        "last_engtemp" => x(t.last_engtemp),
        "last_oiltemp" => x(t.last_oiltemp),
        "last_oil_pressure" => x(t.last_oil_pressure),
        "last_rpm_max" => x(t.last_rpm_max),
        "last_turbo_max" => x(t.last_turbo_max),
        "last_protocol_flags" => json!(t.last_protocol_flags),
        "last_throttle" => x(t.last_throttle),
        "last_brake" => x(t.last_brake),
        "last_clutch" => x(t.last_clutch),
        "last_steering" => x(t.last_steering),
        "last_actual_steering" => x(t.last_actual_steering),
        "last_steering_input" => x(t.last_steering_input),
        "last_air_pressure" => x(t.last_air_pressure),
        "last_air_pressure_max" => x(t.last_air_pressure_max),
        "last_clutch_temp" => x(t.last_clutch_temp),
        "last_g_lat" => x(t.last_g_lat),
        "last_g_lon" => x(t.last_g_lon),
        "last_tire_pressure_fl" => x(t.last_tire_pressure_fl),
        "last_tire_pressure_fr" => x(t.last_tire_pressure_fr),
        "last_tire_pressure_rl" => x(t.last_tire_pressure_rl),
        "last_tire_pressure_rr" => x(t.last_tire_pressure_rr),
        "last_tire_temp_fl" => x(t.last_tire_temp_fl),
        "last_tire_temp_fr" => x(t.last_tire_temp_fr),
        "last_tire_temp_rl" => x(t.last_tire_temp_rl),
        "last_tire_temp_rr" => x(t.last_tire_temp_rr),
        "last_brake_temp_fl" => x(t.last_brake_temp_fl),
        "last_brake_temp_fr" => x(t.last_brake_temp_fr),
        "last_brake_temp_rl" => x(t.last_brake_temp_rl),
        "last_brake_temp_rr" => x(t.last_brake_temp_rr),
        "last_tire_pressure_f" => x(t.last_tire_pressure_f),
        "last_tire_pressure_r" => x(t.last_tire_pressure_r),
        "last_tire_temp_f" => x(t.last_tire_temp_f),
        "last_tire_temp_r" => x(t.last_tire_temp_r),
        "last_brake_temp_f" => x(t.last_brake_temp_f),
        "last_brake_temp_r" => x(t.last_brake_temp_r),
        "last_telemetry_presence" => json!(t.last_telemetry_presence),
        "last_motor_power_kw" => x(t.last_motor_power_kw),
        "last_motor_torque_nm" => x(t.last_motor_torque_nm),
        "last_ev_range_m" => x(t.last_ev_range_m),
        "last_trip_m" => x(t.last_trip_m),
        "last_odometer_m" => x(t.last_odometer_m),
        "last_lifetime_m" => x(t.last_lifetime_m),
        "last_signal_left_input" => json!(t.last_signal_left_input),
        "last_signal_right_input" => json!(t.last_signal_right_input),
        "last_hazard_enabled" => json!(t.last_hazard_enabled),
        "last_lightbar" => json!(t.last_lightbar),
        "last_fog" => json!(t.last_fog),
        "last_gear_byte" => json!(t.last_gear_byte),
        "last_gear_str" => json!(t.last_gear_str),
        "last_bucket" => json!(t.last_bucket),
        "last_speed_announce_ts" => x(t.last_speed_announce_ts),
        "_last_telemetry_ts" => x(t.last_telemetry_ts),
        "drift_alert_active" => json!(t.drift_alert_active),
        "drift_pan_direction" => x(t.drift_pan_direction),
        "drift_rate_val" => x(t.drift_rate_val),
        "_ls_prev_speed_ms" => x(t.ls_prev_speed_ms),
        "_ls_prev_speed_ts" => x(t.ls_prev_speed_ts),
        "_ls_steady_start_ts" => x(t.ls_steady_start_ts),
        "last_ground_speed_ms" => x(t.last_ground_speed_ms),
        "_ms_last_rx_ts" => x(t.ms_last_rx_ts),
        "_ls_accel_smooth" => x(t.ls_accel_smooth),
        "_ls_ms_prev_speed_ms" => x(t.ls_ms_prev_speed_ms),
        "_ls_ms_prev_ts" => x(t.ls_ms_prev_ts),
        "_ls_stopped" => json!(t.ls_stopped),
        "_ls_was_moving" => json!(t.ls_was_moving),
        "_ls_diag_last_ts" => x(t.ls_diag_last_ts),
        "last_vel_x" => x(t.last_vel_x),
        "last_vel_y" => x(t.last_vel_y),
        "coord_target_bearing" => x(t.coord_target_bearing),
        "_last_coord_bearing_ts" => x(t.last_coord_bearing_ts),
        "_ext_version_warned" => json!(t.ext_version_warned),
        "last_slip_active" => json!(t.last_slip_active),
        "last_slip_kind" => json!(t.last_slip_kind),
        "last_slip_mag" => x(t.last_slip_mag),
        "last_tc_active" => json!(t.last_tc_active),
        "last_wheel_max_driven" => x(t.last_wheel_max_driven),
        "last_wheel_min" => x(t.last_wheel_min),
        "last_implement_flags" => x(t.last_implement_flags),
        "last_implement_edge_height" => x(t.last_implement_edge_height),
        "last_implement_min_clearance" => x(t.last_implement_min_clearance),
        "last_implement_tilt_deg" => x(t.last_implement_tilt_deg),
        "last_implement_tilt_percent" => x(t.last_implement_tilt_percent),
        "last_implement_lift" => x(t.last_implement_lift),
        "last_implement_activity" => x(t.last_implement_activity),
        "last_articulation_deg" => x(t.last_articulation_deg),
        "last_pos_x" => x(t.last_pos_x),
        "last_pos_y" => x(t.last_pos_y),
        "last_pos_z" => x(t.last_pos_z),
        "last_yaw_rad" => x(t.last_yaw_rad),
        "last_roll_rad" => x(t.last_roll_rad),
        "last_pitch_rad" => x(t.last_pitch_rad),
        "last_up_z" => x(t.last_up_z),
        "last_heading" => x(t.last_heading),
        "last_click_heading_deg" => x(t.last_click_heading_deg),
        "compass_click_counter" => json!(t.compass_click_counter),
        "last_announced_compass_idx" => json!(t.last_announced_compass_idx),
        "last_compass_ts" => x(t.last_compass_ts),
        "inverted" => json!(t.inverted),
        "inverted_announced" => json!(t.inverted_announced),
        other => panic!("fixture snapshots unknown global {other}"),
    }
}

/// The Python's calls with the user-approved departures applied:
/// - python-bugs.md 31: the first lightbar and fog readings are a baseline, not spoken.
/// - python-bugs.md 32: low and high beam bits both set is "High beams on", not
///   "Headlights off" (the one fixture step that sets both is named here).
fn python_calls_after_departures(
    calls: &[Value],
    first_lightbar: bool,
    first_fog: bool,
    case: &str,
) -> Vec<Value> {
    let said = |c: &Value| c[0] == "say" && c[1].is_string();
    calls
        .iter()
        .filter(|c| {
            if !said(c) {
                return true;
            }
            let text = c[1].as_str().unwrap();
            let lightbar = text.starts_with("Lightbar ");
            let fog = text.starts_with("Fog lights ");
            !(lightbar && first_lightbar || fog && first_fog)
        })
        .map(|c| {
            if case == "extended_v6_drive step 16" && said(c) && c[1] == "Headlights off" {
                json!(["say", "High beams on", c[2], c[3]])
            } else {
                c.clone()
            }
        })
        .collect()
}

#[test]
fn loop_scenarios_match_python() {
    let fx = load("telemetry");
    let scenarios = fx["scenarios"].as_array().unwrap();
    assert!(scenarios.len() >= 15);
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut failures = Vec::new();
    for sc in scenarios {
        let name = sc["name"].as_str().unwrap();
        units::set_units_mode("imperial");
        let hooks = TestHooks {
            st: Mutex::new(State::default()),
            calls: Mutex::new(Vec::new()),
            quiet: AtomicBool::new(false),
            debug: AtomicBool::new(false),
            baseline: AtomicBool::new(false),
            now: Mutex::new(0.0),
            mono: Mutex::new(0.0),
        };
        let mut body = {
            let mut st = hooks.st.lock();
            st.tel.protocol_mode = sc["protocol"].as_str().unwrap().to_string();
            for (k, v) in sc["initial_set"].as_object().unwrap() {
                apply_set(&mut st, k, v);
            }
            TelemetryLoop::start(&mut st)
        };
        // python-bugs.md 33 (departure): where the Python raised on a NaN or infinite
        // lightbar, fog, speed or implement-flags value, the Rust carries on. From that
        // step the two diverge, so the rest of the scenario only checks for no panic.
        let mut diverged = false;
        for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
            let case = format!("{name} step {i}");
            let (first_lightbar, first_fog) = {
                let st = hooks.st.lock();
                (st.tel.last_lightbar == -1, st.tel.last_fog == -1)
            };
            let now = f(&step["now"]);
            *hooks.now.lock() = now;
            *hooks.mono.lock() = f(&step["mono"]);
            hooks
                .quiet
                .store(step["quiet"].as_bool().unwrap(), Ordering::SeqCst);
            hooks
                .debug
                .store(step["debug"].as_bool().unwrap(), Ordering::SeqCst);
            {
                let mut st = hooks.st.lock();
                for (k, v) in step["set"].as_object().unwrap() {
                    apply_set(&mut st, k, v);
                }
            }
            if step["baseline"].as_bool().unwrap() {
                hooks.baseline.store(true, Ordering::SeqCst);
            }
            let data = hex(step["data"].as_str().unwrap());
            let result = catch_unwind(AssertUnwindSafe(|| {
                body.handle_datagram(&hooks, &data, now);
            }));
            let nan_departure = name.starts_with("raises_nan_") || name.starts_with("raises_inf_");
            if nan_departure && (diverged || step["raised"].is_string()) {
                diverged = true;
                if result.is_err() {
                    failures.push(format!("{case}: Rust panicked on a non-finite value"));
                }
                hooks.calls.lock().clear();
                continue;
            }
            match (&result, step["raised"].as_str()) {
                (Ok(()), None) => {}
                (Err(p), Some(exc)) => {
                    let msg = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    if !msg.starts_with(exc) {
                        failures.push(format!(
                            "{case}: Python raised {exc}, Rust panicked {msg:?}"
                        ));
                    }
                }
                (Ok(()), Some(exc)) => {
                    failures.push(format!("{case}: Python raised {exc}, Rust did not"))
                }
                (Err(_), None) => failures.push(format!("{case}: Rust panicked, Python did not")),
            }
            let calls = std::mem::take(&mut *hooks.calls.lock());
            let want_calls = &python_calls_after_departures(
                step["calls"].as_array().unwrap(),
                first_lightbar,
                first_fog,
                &case,
            );
            if !same(
                &Value::Array(calls.clone()),
                &Value::Array(want_calls.clone()),
            ) {
                let n = calls.len().max(want_calls.len());
                let first = (0..n)
                    .find(|&j| match (calls.get(j), want_calls.get(j)) {
                        (Some(a), Some(b)) => !same(a, b),
                        _ => true,
                    })
                    .unwrap_or(0);
                failures.push(format!(
                    "{case}: calls differ at #{first}\n  rust:   {}\n  python: {}",
                    calls.get(first).map(|v| v.to_string()).unwrap_or_default(),
                    want_calls
                        .get(first)
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                ));
            }
            let st = hooks.st.lock();
            for (k, want) in step["snap"].as_object().unwrap() {
                // python-bugs.md 30 (deliberate): the Python's module-level steering stays
                // 0.0 all session because the loop assigns locals; the Rust writes the
                // shared state, which is the point of the fix. The loop's own use of the
                // values is still compared through the calls above.
                if matches!(
                    k.as_str(),
                    "last_steering" | "last_actual_steering" | "last_steering_input"
                ) {
                    continue;
                }
                let got = get(&st, k);
                if !same(&got, want) {
                    failures.push(format!("{case}: {k} rust {got} python {want}"));
                }
            }
        }
    }
    units::set_units_mode("imperial");
    std::panic::set_hook(prev_hook);
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// `_report_gear_change` itself: speech honours the drift announcer, audio needs a degree,
/// any other mode is silent.
#[test]
fn report_gear_change_branches() {
    let hooks = TestHooks {
        st: Mutex::new(State::default()),
        calls: Mutex::new(Vec::new()),
        quiet: AtomicBool::new(false),
        debug: AtomicBool::new(false),
        baseline: AtomicBool::new(false),
        now: Mutex::new(0.0),
        mono: Mutex::new(0.0),
    };
    // Exercised through the loop with OutGauge packets, one gear change each.
    let og = |gear: u8| {
        let mut p = vec![0u8; OG_SIZE];
        p[10] = gear;
        p
    };
    let mut body = {
        let mut st = hooks.st.lock();
        st.tel.protocol_mode = "outgauge".into();
        TelemetryLoop::start(&mut st)
    };
    let says = |hooks: &TestHooks| -> Vec<Value> {
        std::mem::take(&mut *hooks.calls.lock())
            .into_iter()
            .filter(|c| c[0] == "say" || c[0] == "gear_tone")
            .collect()
    };
    body.handle_datagram(&hooks, &og(3), 1.0);
    let first = says(&hooks);
    assert_eq!(first[0], json!(["say", "second", true, true]));
    hooks.quiet.store(true, Ordering::SeqCst);
    body.handle_datagram(&hooks, &og(4), 5.0);
    assert_eq!(says(&hooks), vec![json!(["say", "third", false, true])]);
    hooks.st.lock().cfg.gear_report_settings.mode = "audio".into();
    body.handle_datagram(&hooks, &og(0), 9.0);
    assert_eq!(says(&hooks), vec![json!(["gear_tone", -1, false])]);
    body.handle_datagram(&hooks, &og(255), 13.0);
    assert_eq!(says(&hooks), vec![json!(["gear_tone", 254, false])]);
    hooks.st.lock().cfg.gear_report_settings.mode = "off".into();
    body.handle_datagram(&hooks, &og(1), 17.0);
    assert!(says(&hooks).is_empty());
}
