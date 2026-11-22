//! Owner: EA. The shared state guarded by `state_lock` (`beamtel.py:4351-5044`), every
//! other module-level mutable global of beamtel.py, the separate locks the Python keeps,
//! the module-level objects, and the state snapshot helpers.
//!
//! **Read `rust_plans/engine-map.md` section 1 before using this.** In short:
//!
//! ```ignore
//! let mut st = state();                 // with state_lock:
//! st.scanner.scan_mode_active = true;   // field name = Python global, leading _ dropped
//! drop(st);                             // never held across I/O, sleeps or say()-free waits
//! ```
//!
//! Functions the Python calls while holding `state_lock` take `&State` / `&mut State`
//! rather than locking (the mutex is not reentrant). Initial values below are the Python
//! ones, so `State::default()` is the state at import time.

use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock};

use beam_base::pyfmt::py_round;
use indexmap::{IndexMap, IndexSet};
use parking_lot::{Mutex, MutexGuard, RwLock};
use serde::Serialize;
use serde_json::{Map, Value, json};

pub use crate::convert::pyjson_to_json;
use crate::convert::{
    assistant_snapshot_json, feed_snapshot_json, junction_proximity_snapshot_json,
};
use crate::timer::{Event, Timer, monotonic, time_time};
use crate::units::{has_air_system, has_electric_drive, has_engine_temp};
use beam_audio::controller::AudioController;
use beam_base::gear_report::{self, GearSettings};
use beam_reports::camera_lineup::{self, Geometry};
use beam_reports::drift_speech::{self, DriftAnnouncer};
use beam_reports::wheel_slip::{self, SlipDetector, SlipResult};
use beam_road::assistant_diagnostics::AssistantDiagnosticRecorder;
use beam_road::challenge_results::HillClimbChallengeRecorder;
use beam_road::driving_assistant::{DrivingAssistant, SpeakOpts};
use beam_road::road_diagnostics::RoadDiagnosticRecorder;
use beam_road::road_guidance::RoadGuidanceFeed;

// ---- Constants that sit in the shared-state section --------------------------------------

/// `WHEEL_POS_CORNERS` (FL|FR|RL|RR), the initial telemetry-presence mask.
pub const WHEEL_POS_CORNERS: i64 = 0b1111;
pub const IMPL_FLAG_PRESENT: i64 = 1;
pub const IMPL_FLAG_ARTIC_VALID: i64 = 2;
pub const IMPL_FLAG_GROUND_VALID: i64 = 4;
pub const IMPL_FLAG_JACKING: i64 = 8;
pub const IMPL_FLAG_DETACHED: i64 = 16;
pub const TRAILER_FULL_DEG: f64 = 45.0;
pub const TRAILER_STALE_SEC: f64 = 1.0;
pub const WHEEL_REPORT_STALE_SEC: f64 = 3.0;
pub const ROUTE_STALE_SEC: f64 = 1.5;
pub const REVERSE: i64 = 0;
pub const NEUTRAL: i64 = 1;
pub const OBSTACLE_STATE_HEARTBEAT_S: f64 = 0.5;
pub const OBSTACLE_STEERING_DELTA: f64 = 0.03;
pub const OBSTACLE_PEDAL_DELTA: f64 = 0.05;
pub const DRIFT_SMOOTH_TAU: f64 = 0.25;
pub const DRIFT_ON_RATE: f64 = 0.8;
pub const DRIFT_OFF_RATE: f64 = 0.35;
pub const DRIFT_RELEASE_HOLD: f64 = 0.3;
pub const DRIFT_PAN_MIN_RATE: f64 = 0.2;
pub const LS_ACCEL_TAU_S: f64 = 0.15;
pub const LS_ACCEL_MIN_DT: f64 = 0.05;
pub const LS_ACCEL_MAX_DT: f64 = 1.0;
pub const LS_ACCEL_DEADBAND: f64 = 0.25;
pub const LS_STEADY_HOLD_S: f64 = 1.5;
pub const LS_STOP_ENTER_MS: f64 = 0.20;
pub const LS_STOP_EXIT_MS: f64 = 0.50;
pub const LS_STOP_TONE_MIN_MS: f64 = 1.0;
pub const LS_MS_STALE_S: f64 = 1.0;
pub const LOWSPEED_DIAG_INTERVAL_S: f64 = 0.25;
pub const WORLD_ACTIVE_TELEMETRY_S: f64 = 2.0;
pub const VEHICLE_SWITCH_QUIET_S: f64 = 2.5;
pub const CAMERA_STALE_SEC: f64 = 1.5;
pub use crate::speech_out::SPEECH_PROTECT_S;

// ---- Shared value types (Python dicts with fixed keys) -----------------------------------

/// A keyboard hook record (`_hook_suppressed`'s dict). Opaque: the keyboard module (E5)
/// owns the hooks and maps the id to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HookRecord(pub u64);

/// `{"ctrl": False, "shift": False, "alt": False}` (`_capture_mods`, `_ai_mods`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// A parsed `DOCK:` line (`implement_listener`), `last_dock`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DockReading {
    pub name: String,
    pub range: f64,
    pub lateral: f64,
    pub vertical: f64,
    pub idx: i64,
    pub count: i64,
    pub kind: String,
    pub lo_z: f64,
    pub hi_z: f64,
    pub yaw: f64,
    pub manual: bool,
    pub impl_word: String,
    pub entry_theta: Option<f64>,
    pub entry_depth: Option<f64>,
    pub mode: String,
    pub margin: Option<f64>,
    pub acquire: bool,
}

/// A `CANNONAIM:` line, `last_cannon_aim`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CannonAim {
    pub elevation: f64,
    pub bearing: f64,
    pub target_angle: f64,
    pub line_angle: f64,
    pub range: f64,
    pub speed: f64,
    pub reachable: bool,
    pub solution_available: bool,
    pub stamp: f64,
}

/// A `SHOT:` line, `last_cannon_shot` and the entries of `cannon_shot_session`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CannonShot {
    pub downrange: f64,
    pub lateral: f64,
    pub apex: f64,
    pub flight: f64,
    pub ramp_pitch: f64,
    pub strength: i64,
    pub settled: bool,
    pub index: i64,
    pub near_dist: f64,
    pub near_name: String,
    pub vehicle: String,
    pub stamp: f64,
    /// Present only when known (the Python key is absent otherwise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_downrange: Option<f64>,
}

/// `_parse_ramp_self`'s dict, `last_ramp_self`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RampSelf {
    pub pitch: Option<f64>,
    pub lip: Option<f64>,
    /// (directionElectricsName, percent, stroke metres)
    pub groups: Vec<(String, i64, f64)>,
}

/// Virtual browser callbacks. Always called with NO state guard held.
pub type OnEnter = Arc<dyn Fn(usize, &str, Option<Value>) + Send + Sync>;
pub type OnAdjust = Arc<dyn Fn(usize, &str, Option<Value>, i64) + Send + Sync>;
pub type OnEscape = Arc<dyn Fn() + Send + Sync>;

/// One entry of `_ai_vehicle_commands`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AiCommand {
    pub mode: String,
    pub target_id: Option<i64>,
}

// ---- The state, grouped by feature --------------------------------------------------------

/// Telemetry-derived values (written by `telemetry_loop`, E1).
#[derive(Debug, Clone)]
pub struct Telemetry {
    pub last_speed_ms: f64,
    pub last_rpm: f64,
    pub last_fuel: f64,
    pub last_turbo: f64,
    pub last_protocol_flags: i64,
    pub last_engtemp: f64,
    pub last_oiltemp: f64,
    pub last_motor_power_kw: f64,
    pub last_motor_torque_nm: f64,
    pub last_ev_range_m: f64,
    pub last_trip_m: f64,
    pub last_odometer_m: f64,
    pub last_lifetime_m: f64,
    pub last_throttle: f64,
    pub last_brake: f64,
    pub last_clutch: f64,
    pub last_steering: f64,
    pub last_actual_steering: f64,
    pub last_steering_input: f64,
    pub last_rpm_max: f64,
    pub last_turbo_max: f64,
    pub last_heading: f64,
    pub last_oil_pressure: f64,
    pub protocol_mode: String,
    pub last_air_pressure: f64,
    pub last_air_pressure_max: f64,
    pub last_implement_flags: f64,
    pub last_implement_edge_height: f64,
    pub last_implement_min_clearance: f64,
    pub last_implement_tilt_deg: f64,
    pub last_implement_tilt_percent: f64,
    pub last_implement_lift: f64,
    pub last_implement_activity: f64,
    pub last_articulation_deg: f64,
    pub last_clutch_temp: f64,
    pub last_g_lat: f64,
    pub last_g_lon: f64,
    pub last_tire_pressure_fl: f64,
    pub last_tire_pressure_fr: f64,
    pub last_tire_pressure_rl: f64,
    pub last_tire_pressure_rr: f64,
    pub last_tire_temp_fl: f64,
    pub last_tire_temp_fr: f64,
    pub last_tire_temp_rl: f64,
    pub last_tire_temp_rr: f64,
    pub last_brake_temp_fl: f64,
    pub last_brake_temp_fr: f64,
    pub last_brake_temp_rl: f64,
    pub last_brake_temp_rr: f64,
    pub last_tire_pressure_f: f64,
    pub last_tire_pressure_r: f64,
    pub last_tire_temp_f: f64,
    pub last_tire_temp_r: f64,
    pub last_brake_temp_f: f64,
    pub last_brake_temp_r: f64,
    pub last_telemetry_presence: i64,
    pub last_signal_left_input: bool,
    pub last_signal_right_input: bool,
    pub last_hazard_enabled: bool,
    pub last_lightbar: i64,
    pub last_fog: i64,
    // Compass click state
    pub last_click_heading_deg: f64,
    pub compass_click_counter: i64,
    // MotionSim state
    pub last_pos_x: f64,
    pub last_pos_y: f64,
    pub last_pos_z: f64,
    pub last_yaw_rad: f64,
    pub last_compass_ts: f64,
    pub last_announced_compass_idx: i64,
    pub compass_min_interval: f64,
    pub last_roll_rad: f64,
    pub last_pitch_rad: f64,
    pub last_up_z: f64,
    pub inverted: bool,
    pub inverted_announced: bool,
    pub last_bucket: Option<i64>,
    pub last_speed_announce_ts: f64,
    pub cooldown_sec: f64,
    pub last_gear_byte: Option<i64>,
    pub last_gear_str: Option<String>,
    pub pedal_tones_active: bool,
    pub heading_guidance_active: bool,
    pub heading_guidance_target: f64,
    pub drift_mode_active: bool,
    pub last_drift_check_ts: f64,
    pub drift_baseline_heading: f64,
    pub drift_alert_active: bool,
    pub drift_pan_direction: f64,
    /// `drift_rate_val`, declared `global` inside `telemetry_loop`.
    pub drift_rate_val: f64,
    pub low_speed_mode_active: bool,
    pub slip_mode_active: bool,
    pub last_slip_active: bool,
    pub last_slip_kind: i64,
    pub last_slip_mag: f64,
    pub last_tc_active: bool,
    pub slip_detector: SlipDetector,
    pub last_slip_result: SlipResult,
    pub last_wheel_max_driven: f64,
    pub last_wheel_min: f64,
    pub last_vel_x: f64,
    pub last_vel_y: f64,
    pub coord_guidance_active: bool,
    pub marked_coord_x: Option<f64>,
    pub marked_coord_y: Option<f64>,
    pub coord_target_bearing: f64,
    pub last_coord_bearing_ts: f64,
    pub ls_prev_speed_ms: f64,
    pub ls_prev_speed_ts: f64,
    pub ls_steady_start_ts: f64,
    pub last_ground_speed_ms: f64,
    pub ms_last_rx_ts: f64,
    pub ls_accel_smooth: f64,
    pub ls_ms_prev_speed_ms: f64,
    pub ls_ms_prev_ts: f64,
    pub ls_stopped: bool,
    pub ls_was_moving: bool,
    pub ls_diag_last_ts: f64,
    /// When telemetry last arrived on 4444 (`time.time()`).
    pub last_telemetry_ts: f64,
    pub ext_version_warned: bool,
}

impl Default for Telemetry {
    fn default() -> Self {
        Telemetry {
            last_speed_ms: 0.0,
            last_rpm: 0.0,
            last_fuel: 0.0,
            last_turbo: 0.0,
            last_protocol_flags: 0,
            last_engtemp: 0.0,
            last_oiltemp: 0.0,
            last_motor_power_kw: 0.0,
            last_motor_torque_nm: 0.0,
            last_ev_range_m: -1.0,
            last_trip_m: -1.0,
            last_odometer_m: -1.0,
            last_lifetime_m: -1.0,
            last_throttle: 0.0,
            last_brake: 0.0,
            last_clutch: 0.0,
            last_steering: 0.0,
            last_actual_steering: 0.0,
            last_steering_input: 0.0,
            last_rpm_max: 0.0,
            last_turbo_max: 0.0,
            last_heading: 0.0,
            last_oil_pressure: 0.0,
            protocol_mode: "outgauge".to_string(),
            last_air_pressure: 0.0,
            last_air_pressure_max: 0.0,
            last_implement_flags: 0.0,
            last_implement_edge_height: -1.0,
            last_implement_min_clearance: -1.0,
            last_implement_tilt_deg: 0.0,
            last_implement_tilt_percent: 0.0,
            last_implement_lift: 0.0,
            last_implement_activity: 0.0,
            last_articulation_deg: 0.0,
            last_clutch_temp: 0.0,
            last_g_lat: 0.0,
            last_g_lon: 0.0,
            last_tire_pressure_fl: 0.0,
            last_tire_pressure_fr: 0.0,
            last_tire_pressure_rl: 0.0,
            last_tire_pressure_rr: 0.0,
            last_tire_temp_fl: 0.0,
            last_tire_temp_fr: 0.0,
            last_tire_temp_rl: 0.0,
            last_tire_temp_rr: 0.0,
            last_brake_temp_fl: 0.0,
            last_brake_temp_fr: 0.0,
            last_brake_temp_rl: 0.0,
            last_brake_temp_rr: 0.0,
            last_tire_pressure_f: 0.0,
            last_tire_pressure_r: 0.0,
            last_tire_temp_f: 0.0,
            last_tire_temp_r: 0.0,
            last_brake_temp_f: 0.0,
            last_brake_temp_r: 0.0,
            last_telemetry_presence: WHEEL_POS_CORNERS,
            last_signal_left_input: false,
            last_signal_right_input: false,
            last_hazard_enabled: false,
            last_lightbar: -1,
            last_fog: -1,
            last_click_heading_deg: 0.0,
            compass_click_counter: 0,
            last_pos_x: 0.0,
            last_pos_y: 0.0,
            last_pos_z: 0.0,
            last_yaw_rad: 0.0,
            last_compass_ts: 0.0,
            last_announced_compass_idx: -1,
            compass_min_interval: 0.1,
            last_roll_rad: 0.0,
            last_pitch_rad: 0.0,
            last_up_z: 1.0,
            inverted: false,
            inverted_announced: false,
            last_bucket: None,
            last_speed_announce_ts: 0.0,
            cooldown_sec: 1.0,
            last_gear_byte: None,
            last_gear_str: None,
            pedal_tones_active: false,
            heading_guidance_active: false,
            heading_guidance_target: 0.0,
            drift_mode_active: false,
            last_drift_check_ts: 0.0,
            drift_baseline_heading: 0.0,
            drift_alert_active: false,
            drift_pan_direction: 0.0,
            drift_rate_val: 0.0,
            low_speed_mode_active: false,
            slip_mode_active: false,
            last_slip_active: false,
            last_slip_kind: 0,
            last_slip_mag: 0.0,
            last_tc_active: false,
            slip_detector: SlipDetector::new(),
            last_slip_result: wheel_slip::INACTIVE,
            last_wheel_max_driven: -1.0,
            last_wheel_min: -1.0,
            last_vel_x: 0.0,
            last_vel_y: 0.0,
            coord_guidance_active: false,
            marked_coord_x: None,
            marked_coord_y: None,
            coord_target_bearing: 0.0,
            last_coord_bearing_ts: 0.0,
            ls_prev_speed_ms: 0.0,
            ls_prev_speed_ts: 0.0,
            ls_steady_start_ts: 0.0,
            last_ground_speed_ms: 0.0,
            ms_last_rx_ts: 0.0,
            ls_accel_smooth: 0.0,
            ls_ms_prev_speed_ms: 0.0,
            ls_ms_prev_ts: 0.0,
            ls_stopped: false,
            ls_was_moving: false,
            ls_diag_last_ts: 0.0,
            last_telemetry_ts: 0.0,
            ext_version_warned: false,
        }
    }
}

/// Settings applied from the config by `_run_engine` / `_apply_live_config` (EA).
#[derive(Debug, Clone)]
pub struct Settings {
    pub oil_chime_enabled: bool,
    pub announce_turn_signals: bool,
    pub announce_speed: bool,
    /// Python keeps whatever the config held (normally an int); see engine-map bug 4.
    pub speed_announce_interval: f64,
    pub gear_report_settings: GearSettings,
    pub ui_nav_hold_suppression: bool,
    pub ai_describer_provider: String,
    pub ai_describer_settings: Map<String, Value>,
    pub ai_describer_disable_ui_toggle: bool,
    pub compass_highlight_enabled: bool,
    pub compass_highlight_nth_click: i64,
    pub compass_click_interval_deg: f64,
    pub scanner_distance_callout_enabled: bool,
    pub scanner_distance_callout_interval: f64,
    pub announce_implement_proximity: bool,
    pub announce_cannon_shot: bool,
    pub announce_binding_learn_description: bool,
    pub announce_clickspot_actions: bool,
    pub road_follow_guidance_enabled: bool,
    pub road_junction_speech_enabled: bool,
    pub road_stop_sign_speech_enabled: bool,
    pub road_traffic_light_speech_enabled: bool,
    pub road_speed_limit_speech_enabled: bool,
    pub road_surface_speech_enabled: bool,
    pub road_junction_earcon_enabled: bool,
    pub road_include_private: bool,
    pub obstacle_warning_sensitivity: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            oil_chime_enabled: true,
            announce_turn_signals: true,
            announce_speed: true,
            speed_announce_interval: 25.0,
            gear_report_settings: gear_report::settings_from_config(
                &beam_base::pyval::PyDict::new(),
            )
            .expect("gear_report.settings_from_config({}) cannot fail"),
            ui_nav_hold_suppression: true,
            ai_describer_provider: "gemini".to_string(),
            ai_describer_settings: Map::new(),
            ai_describer_disable_ui_toggle: false,
            compass_highlight_enabled: true,
            compass_highlight_nth_click: 6,
            compass_click_interval_deg: 15.0,
            scanner_distance_callout_enabled: false,
            scanner_distance_callout_interval: 10.0,
            announce_implement_proximity: true,
            announce_cannon_shot: true,
            announce_binding_learn_description: false,
            announce_clickspot_actions: false,
            road_follow_guidance_enabled: true,
            road_junction_speech_enabled: true,
            road_stop_sign_speech_enabled: true,
            road_traffic_light_speech_enabled: true,
            road_speed_limit_speech_enabled: true,
            road_surface_speech_enabled: true,
            road_junction_earcon_enabled: true,
            road_include_private: false,
            obstacle_warning_sensitivity: "normal".to_string(),
        }
    }
}

/// Vehicle scanner (E2).
#[derive(Debug, Clone)]
pub struct Scanner {
    pub scan_mode_active: bool,
    pub coupler_dist_mode: bool,
    pub coupler_run_active: bool,
    pub last_scanner_target_name: String,
    pub last_scanner_distance: f64,
    pub last_scanner_approach_deg: f64,
    pub last_scanner_bearing: f64,
    pub scanner_ref_reversed: bool,
}

impl Default for Scanner {
    fn default() -> Self {
        Scanner {
            scan_mode_active: false,
            coupler_dist_mode: false,
            coupler_run_active: false,
            last_scanner_target_name: String::new(),
            last_scanner_distance: f64::INFINITY,
            last_scanner_approach_deg: 0.0,
            last_scanner_bearing: 0.0,
            scanner_ref_reversed: false,
        }
    }
}

/// Obstacle detector (E2). `obstacle_state_last` is `(direction, steering, throttle, brake)`.
#[derive(Debug, Clone, Default)]
pub struct Obstacle {
    pub obstacle_mode_active: bool,
    pub obstacle_state_last: Option<(String, f64, f64, f64)>,
    pub obstacle_state_last_sent: f64,
}

/// Docking instrument, ramps and cannons (E3).
#[derive(Debug, Clone)]
pub struct Dock {
    pub dock_mode_active: bool,
    pub last_dock: Option<DockReading>,
    pub last_dock_fail: Option<String>,
    pub cannon_active: bool,
    pub cannon_kind: String,
    pub last_ramp_self: Option<RampSelf>,
    pub last_cannon_aim: Option<CannonAim>,
    pub last_dock_mode: Option<String>,
    pub last_dock_name: Option<String>,
    pub last_cannon_shot: Option<CannonShot>,
    pub cannon_shot_session: Vec<CannonShot>,
    /// `_implement_word_current`: "Bucket"/"Forks"/"Grapple", or "".
    pub implement_word_current: String,
}

impl Default for Dock {
    fn default() -> Self {
        Dock {
            dock_mode_active: false,
            last_dock: None,
            last_dock_fail: None,
            cannon_active: false,
            cannon_kind: "NONE".to_string(),
            last_ramp_self: None,
            last_cannon_aim: None,
            last_dock_mode: None,
            last_dock_name: None,
            last_cannon_shot: None,
            cannon_shot_session: Vec::new(),
            implement_word_current: String::new(),
        }
    }
}

/// Trailer articulation (E3).
#[derive(Debug, Clone, Default)]
pub struct Trailer {
    pub last_trailer_deg: f64,
    pub last_trailer_id: Option<i64>,
    pub last_trailer_name: String,
    pub last_trailer_stamp: f64,
}

/// Wheel report rows (E3): `wheel_report.all_rows()` dicts.
#[derive(Debug, Clone, Default)]
pub struct Wheels {
    pub last_wheel_rows: Vec<Value>,
    pub last_wheel_stamp: f64,
}

/// Route beacon (E3).
#[derive(Debug, Clone, Default)]
pub struct Route {
    pub route_beacon_active: bool,
    pub route_dest_x: Option<f64>,
    pub route_dest_y: Option<f64>,
    pub route_remaining_m: f64,
    pub route_beacon_stamp: f64,
}

/// Terrain scan and nearest-POI reply latches (E3), `time.time()` stamps.
#[derive(Debug, Clone, Default)]
pub struct ScanReplies {
    pub last_scan_reply_ts: f64,
    pub last_poi_reply_ts: f64,
}

/// Free-camera line-up guide (E3).
#[derive(Debug, Clone)]
pub struct Lineup {
    pub lineup_mode: String,
    pub lineup_offset_m: f64,
    pub lineup_depth_m: f64,
    pub lineup_enabled: bool,
    pub lineup_muted: bool,
    pub lineup_geom: Option<Geometry>,
    pub lineup_stamp: f64,
    pub lineup_hooks: Vec<HookRecord>,
    pub lineup_layer_open: bool,
}

impl Default for Lineup {
    fn default() -> Self {
        Lineup {
            lineup_mode: camera_lineup::HEAD_ON.to_string(),
            lineup_offset_m: camera_lineup::DEFAULT_OFFSET_M,
            lineup_depth_m: camera_lineup::DEFAULT_DEPTH_M,
            lineup_enabled: false,
            lineup_muted: false,
            lineup_geom: None,
            lineup_stamp: 0.0,
            lineup_hooks: Vec::new(),
            lineup_layer_open: false,
        }
    }
}

/// Camera info (E4).
#[derive(Debug, Clone)]
pub struct Camera {
    pub free_cam_active: bool,
    pub cam_yaw_deg: f64,
    pub cam_pitch_deg: f64,
    pub cam_agl: f64,
    pub cam_agl_valid: bool,
    pub cam_last_packet_ts: f64,
    pub cam_veh_bearing: f64,
    pub cam_veh_distance: f64,
    pub cam_is_free: bool,
    pub cam_last_click_heading_deg: f64,
    pub cam_compass_click_counter: i64,
    pub cam_last_announced_compass_idx: i64,
    pub cam_last_compass_ts: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            free_cam_active: false,
            cam_yaw_deg: 0.0,
            cam_pitch_deg: 0.0,
            cam_agl: 0.0,
            cam_agl_valid: true,
            cam_last_packet_ts: 0.0,
            cam_veh_bearing: 0.0,
            cam_veh_distance: -1.0,
            cam_is_free: false,
            cam_last_click_heading_deg: 0.0,
            cam_compass_click_counter: 0,
            cam_last_announced_compass_idx: -1,
            cam_last_compass_ts: 0.0,
        }
    }
}

/// Node grabber (E4). `nodegrab_mode_active` and `_nodegrab_space_held` are the atomics
/// [`NODEGRAB_MODE_ACTIVE`] / [`NODEGRAB_SPACE_HELD`] because the hook reads them.
#[derive(Debug, Clone)]
pub struct Nodegrab {
    pub nodegrab_scanning: bool,
    pub nodegrab_pad_active: bool,
    pub nodegrab_last_cid: i64,
    pub nodegrab_space_hook: Option<HookRecord>,
}

impl Default for Nodegrab {
    fn default() -> Self {
        Nodegrab {
            nodegrab_scanning: false,
            nodegrab_pad_active: false,
            nodegrab_last_cid: -1,
            nodegrab_space_hook: None,
        }
    }
}

/// One clickspot row: (cache_index, trigger_id, display_name).
pub type ClickspotRow = (i64, i64, String);

/// Clickspot detection (E4).
#[derive(Debug, Clone)]
pub struct Clickspot {
    pub clickspot_mode_active: bool,
    pub clickspot_trigger_list: Vec<ClickspotRow>,
    pub clickspot_list_loading: bool,
    /// Staged rows keyed by cache index (published sorted by key).
    pub clickspot_trigger_building: Option<std::collections::BTreeMap<i64, ClickspotRow>>,
    pub clickspot_expected_count: Option<i64>,
    pub clickspot_last_hover_id: i64,
}

impl Default for Clickspot {
    fn default() -> Self {
        Clickspot {
            clickspot_mode_active: false,
            clickspot_trigger_list: Vec::new(),
            clickspot_list_loading: false,
            clickspot_trigger_building: None,
            clickspot_expected_count: None,
            clickspot_last_hover_id: -1,
        }
    }
}

/// An environment row: the mod's `k=v` fields in order.
pub type EnvRow = IndexMap<String, String>;

/// Environment browser (E4).
#[derive(Debug, Clone)]
pub struct Env {
    pub env_rows: Vec<EnvRow>,
    pub env_level: String,
    pub env_can_change: bool,
    pub env_building: Option<Vec<EnvRow>>,
    pub env_unavailable: String,
    pub env_ambient_c: Option<f64>,
    pub env_awaiting_change: bool,
    pub env_change_timer: Option<Timer>,
    pub env_browser_open: bool,
}

impl Default for Env {
    fn default() -> Self {
        Env {
            env_rows: Vec::new(),
            env_level: String::new(),
            env_can_change: true,
            env_building: None,
            env_unavailable: String::new(),
            env_ambient_c: None,
            env_awaiting_change: false,
            env_change_timer: None,
            env_browser_open: false,
        }
    }
}

/// Vehicle information (E4). `_vinfo_event` is [`VINFO_EVENT`]. `vinfo_error` is `None`
/// for both of the Python's falsy values (`None` and `""`).
#[derive(Debug, Clone, Default)]
pub struct VehicleInfo {
    pub vinfo_rows: Vec<Value>,
    pub vinfo_building: Option<Vec<Value>>,
    pub vinfo_error: Option<(String, String)>,
    pub vinfo_absent: bool,
}

/// UI page text (E4). `_page_text_event` is [`PAGE_TEXT_EVENT`].
#[derive(Debug, Clone, Default)]
pub struct PageText {
    pub ui_screen_context: String,
    pub ui_screen_title: String,
    pub page_text_lines: Vec<String>,
    pub page_text_title: String,
    pub page_text_error: Option<(String, String)>,
    pub page_text_absent: bool,
}

/// Vehicle-specific bindings and learn mode (E4). Rows are (cache_index, line).
#[derive(Debug, Clone, Default)]
pub struct Bindings {
    pub vehicle_bindings_list: Vec<(i64, String)>,
    pub vehicle_bindings_vehicle: String,
    pub vehicle_bindings_building: Option<Vec<(i64, String)>>,
    pub vehicle_bindings_building_name: String,
    pub binding_learn_active: bool,
}

/// The generic virtual browser (E5).
#[derive(Clone, Default)]
pub struct VBrowser {
    pub vbrowser_lines: Vec<String>,
    pub vbrowser_index: usize,
    pub vbrowser_hooks: Vec<HookRecord>,
    pub vbrowser_active: bool,
    pub vbrowser_on_enter: Option<OnEnter>,
    pub vbrowser_on_adjust: Option<OnAdjust>,
    pub vbrowser_on_escape: Option<OnEscape>,
    pub vbrowser_entry_data: Vec<Value>,
}

impl std::fmt::Debug for VBrowser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VBrowser")
            .field("vbrowser_lines", &self.vbrowser_lines)
            .field("vbrowser_index", &self.vbrowser_index)
            .field("vbrowser_active", &self.vbrowser_active)
            .finish_non_exhaustive()
    }
}

/// Status/Functions/Click spots menus, status mode and buffer mode (E5).
#[derive(Debug, Clone)]
pub struct Menu {
    pub status_keyboard_mode_active: bool,
    pub current_status_metric_index: i64,
    pub current_functions_item_index: i64,
    pub current_clickspot_index: i64,
    pub current_accessibility_screen_index: i64,
    pub status_arrow_hooks: Vec<HookRecord>,
    pub buffer_mode_active: bool,
    pub current_buffer_index: i64,
    pub buffer_key_hooks: Vec<HookRecord>,
}

impl Default for Menu {
    fn default() -> Self {
        Menu {
            status_keyboard_mode_active: false,
            current_status_metric_index: 0,
            current_functions_item_index: 0,
            current_clickspot_index: 0,
            current_accessibility_screen_index: 0,
            status_arrow_hooks: Vec::new(),
            buffer_mode_active: false,
            current_buffer_index: -1,
            buffer_key_hooks: Vec::new(),
        }
    }
}

/// F9 layer state touched only on the command worker (E5/E6). The hook-side globals
/// (`_kb_layer`, `_live_mods`) are atomics in the keyboard module.
#[derive(Debug, Clone, Default)]
pub struct Kb {
    pub next_key_timer: Option<Timer>,
    pub capture_mods: Mods,
    pub input_help_mode: bool,
}

/// Fuel stations (E2).
#[derive(Debug, Clone, Default)]
pub struct Fuel {
    pub fuel_rows: Vec<Value>,
    pub fuel_building: Option<Vec<Value>>,
    pub fuel_pending: bool,
    pub fuel_timer: Option<Timer>,
    pub fuel_target_id: Option<Value>,
    pub fuel_target_name: Option<String>,
    pub fuel_in_range: bool,
    pub fuel_parking: bool,
}

/// Audio debug browsers (E4).
#[derive(Debug, Clone)]
pub struct Aud {
    pub aud_rows: Vec<Value>,
    pub aud_building: Option<Vec<Value>>,
    pub aud_sfx_rows: Vec<Value>,
    pub aud_sfx_building: Option<Vec<Value>>,
    pub aud_sfx_head: Value,
    pub aud_sfx_stack: Vec<i64>,
    pub aud_pending: Option<String>,
    pub aud_pending_start: i64,
    pub aud_awaiting: bool,
    pub aud_timer: Option<Timer>,
    pub aud_browser: Option<String>,
}

impl Default for Aud {
    fn default() -> Self {
        Aud {
            aud_rows: Vec::new(),
            aud_building: None,
            aud_sfx_rows: Vec::new(),
            aud_sfx_building: None,
            aud_sfx_head: Value::Object(Map::new()),
            aud_sfx_stack: Vec::new(),
            aud_pending: None,
            aud_pending_start: 0,
            aud_awaiting: false,
            aud_timer: None,
            aud_browser: None,
        }
    }
}

/// F10 AI layer (E7). The slots themselves are behind [`slots()`] (`_slots_lock`).
#[derive(Debug, Clone)]
pub struct Ai {
    pub ai_speed_limit_ms: Option<f64>,
    pub ai_avoid_mode: String,
    pub ai_lane_driving: bool,
    pub pending_target_confirm: Option<i64>,
    pub ai_vehicle_commands: IndexMap<i64, AiCommand>,
    /// `_police_lights_active`. Python's set iterates in hash order; this keeps insertion
    /// order (the LIGHTBAR id list order can differ; the game does not care).
    pub police_lights_active: IndexSet<i64>,
    pub ai_timer: Option<Timer>,
    pub ai_mods: Mods,
}

impl Default for Ai {
    fn default() -> Self {
        Ai {
            ai_speed_limit_ms: None,
            ai_avoid_mode: "auto".to_string(),
            ai_lane_driving: false,
            pending_target_confirm: None,
            ai_vehicle_commands: IndexMap::new(),
            police_lights_active: IndexSet::new(),
            ai_timer: None,
            ai_mods: Mods::default(),
        }
    }
}

/// Every module-level mutable global of beamtel.py that is guarded by, or read alongside,
/// `state_lock`.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub tel: Telemetry,
    pub cfg: Settings,
    pub scanner: Scanner,
    pub obstacle: Obstacle,
    /// `road_mode_active`.
    pub road_mode_active: bool,
    pub dock: Dock,
    pub trailer: Trailer,
    pub wheels: Wheels,
    pub route: Route,
    pub scan: ScanReplies,
    pub lineup: Lineup,
    pub camera: Camera,
    pub nodegrab: Nodegrab,
    pub clickspot: Clickspot,
    pub env: Env,
    pub vinfo: VehicleInfo,
    pub page: PageText,
    pub bindings: Bindings,
    pub vbrowser: VBrowser,
    pub menu: Menu,
    pub kb: Kb,
    pub fuel: Fuel,
    pub aud: Aud,
    pub ai: Ai,
    /// `_last_vehicle_switch_ts` (monotonic).
    pub last_vehicle_switch_ts: f64,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

/// `with state_lock:`. Not reentrant: never call while a guard is alive on this thread.
pub fn state() -> MutexGuard<'static, State> {
    STATE.lock()
}

/// `state_lock` with a timeout, for the few places that must not block (diagnostics).
pub fn try_state_for(timeout: std::time::Duration) -> Option<MutexGuard<'static, State>> {
    STATE.try_lock_for(timeout)
}

// ---- Separate locks and flags ------------------------------------------------------------

/// One slot entry of `_vehicle_slots`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SlotInfo {
    pub id: i64,
    pub name: String,
}

/// `_vehicle_slots`, `_selected_slots`, `_target_slot`, behind `_slots_lock`.
#[derive(Debug, Clone, Default)]
pub struct Slots {
    pub vehicle_slots: IndexMap<i64, SlotInfo>,
    pub selected_slots: BTreeSet<i64>,
    pub target_slot: Option<i64>,
}

static SLOTS: LazyLock<Mutex<Slots>> = LazyLock::new(|| Mutex::new(Slots::default()));

/// `with _slots_lock:`.
pub fn slots() -> MutexGuard<'static, Slots> {
    SLOTS.lock()
}

/// `_capture_lock`: held across the whole HIDE -> grab -> SHOW sequence.
pub static CAPTURE_LOCK: Mutex<()> = Mutex::new(());
/// `_describer_lock` guarding `_describer_busy`.
pub static DESCRIBER_BUSY: Mutex<bool> = Mutex::new(false);
/// `_vinfo_event`.
pub static VINFO_EVENT: Event = Event::new();
/// `_page_text_event`.
pub static PAGE_TEXT_EVENT: Event = Event::new();
/// `nodegrab_mode_active` (read inside the Ctrl+Space hook).
pub static NODEGRAB_MODE_ACTIVE: AtomicBool = AtomicBool::new(false);
/// `_nodegrab_space_held` (read and written inside the hook).
pub static NODEGRAB_SPACE_HELD: AtomicBool = AtomicBool::new(false);

static AUDIO_CONTROLLER_REF: RwLock<Option<Arc<AudioController>>> = RwLock::new(None);

/// `audio_controller_ref` (None until `_run_engine` has started audio).
pub fn audio_controller_ref() -> Option<Arc<AudioController>> {
    AUDIO_CONTROLLER_REF.read().clone()
}

pub fn set_audio_controller_ref(ac: Option<Arc<AudioController>>) {
    *AUDIO_CONTROLLER_REF.write() = ac;
}

// ---- Module-level objects ----------------------------------------------------------------

fn config_dir() -> std::path::PathBuf {
    beam_base::paths::config_dir()
}

/// Shared with the driving assistant, which records into it (`ASSISTANT_DIAGNOSTICS`).
static ASSISTANT_DIAGNOSTICS: LazyLock<Arc<AssistantDiagnosticRecorder>> = LazyLock::new(|| {
    Arc::new(AssistantDiagnosticRecorder::new(
        config_dir().join("assistant_diagnostics"),
    ))
});
static ROAD_DIAGNOSTICS: LazyLock<RoadDiagnosticRecorder> =
    LazyLock::new(|| RoadDiagnosticRecorder::new(config_dir().join("road_diagnostics")));
/// The Python constructor creates the directory at import; a failure there stops the app
/// from starting, so it is a panic here (on first use).
static HILL_CLIMB_CHALLENGE: LazyLock<HillClimbChallengeRecorder> = LazyLock::new(|| {
    let dir = config_dir().join("challenges").join("hill_climb");
    HillClimbChallengeRecorder::new(&dir)
        .unwrap_or_else(|e| panic!("HillClimbChallengeRecorder({}): {e}", dir.display()))
});
/// `DriftAnnouncer(lambda *args, **kwargs: say(*args, **kwargs))`: enabled, on
/// `time.monotonic`. It calls `say(phrase, interrupt=..., source="drift")`.
static DRIFT_ANNOUNCER: LazyLock<DriftAnnouncer> = LazyLock::new(|| {
    DriftAnnouncer::new(
        Box::new(|text, interrupt, source| crate::speech_out::say(text, interrupt, false, source)),
        true,
        Box::new(drift_speech::monotonic),
    )
});
static ROAD_GUIDANCE_FEED: LazyLock<RoadGuidanceFeed> = LazyLock::new(RoadGuidanceFeed::new);
/// `DrivingAssistant(_send_ai_command, say, _assistant_cue, ASSISTANT_DIAGNOSTICS,
/// units=lambda: UNITS_MODE, on_arrival=_assistant_arrival)`. The assistant calls
/// `speak(text, [interrupt=...], exclude_from_buffer=...)`, so `say`'s own defaults fill
/// the rest (interrupt True, source "python").
static DRIVING_ASSISTANT: LazyLock<DrivingAssistant> = LazyLock::new(|| {
    let da = DrivingAssistant::new(
        crate::ai_layer::send_ai_command,
        |text: &str, opts: SpeakOpts| {
            crate::speech_out::say(
                text,
                opts.interrupt.unwrap_or(true),
                opts.exclude_from_buffer,
                "python",
            )
        },
        Some(Arc::new(crate::units::units_mode)),
        Some(Box::new(crate::ai_layer::assistant_arrival)),
    );
    da.set_cue(Arc::new(crate::ai_layer::assistant_cue));
    da.set_diagnostics(Some(Arc::clone(&ASSISTANT_DIAGNOSTICS)));
    da
});

/// `ASSISTANT_DIAGNOSTICS`.
pub fn assistant_diagnostics() -> &'static AssistantDiagnosticRecorder {
    &ASSISTANT_DIAGNOSTICS
}
/// `ROAD_DIAGNOSTICS`.
pub fn road_diagnostics() -> &'static RoadDiagnosticRecorder {
    &ROAD_DIAGNOSTICS
}
/// `HILL_CLIMB_CHALLENGE`.
pub fn hill_climb_challenge() -> &'static HillClimbChallengeRecorder {
    &HILL_CLIMB_CHALLENGE
}
/// `DRIFT_ANNOUNCER` (speaks through `speech_out::say`).
pub fn drift_announcer() -> &'static DriftAnnouncer {
    &DRIFT_ANNOUNCER
}
/// `ROAD_GUIDANCE_FEED`.
pub fn road_guidance_feed() -> &'static RoadGuidanceFeed {
    &ROAD_GUIDANCE_FEED
}
/// `driving_assistant`.
pub fn driving_assistant() -> &'static DrivingAssistant {
    &DRIVING_ASSISTANT
}

// ---- Snapshot helpers (beamtel.py:4679-4876) ---------------------------------------------

/// `_world_is_active()` with the state already locked.
pub fn world_is_active_in(st: &State) -> bool {
    (time_time() - st.tel.last_telemetry_ts) < WORLD_ACTIVE_TELEMETRY_S
}

/// `_world_is_active()`: is there a world to scan, as opposed to a menu to press a button in?
pub fn world_is_active() -> bool {
    world_is_active_in(&state())
}

/// `_travel_bearing_or_none()`: direction of travel, +LEFT of the nose, or None.
pub fn travel_bearing_or_none(st: &State) -> Option<f64> {
    if (time_time() - st.tel.ms_last_rx_ts) > LS_MS_STALE_S {
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

/// JSON for a float the way `json.dumps` writes it (NaN/inf cannot be represented by
/// serde_json and become null; Python would write the non-standard `NaN`/`Infinity`).
fn jf(x: f64) -> Value {
    serde_json::Number::from_f64(x)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn to_json<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// `_road_diagnostic_telemetry_snapshot()`.
pub fn road_diagnostic_telemetry_snapshot() -> Value {
    let st = state();
    road_diagnostic_telemetry_snapshot_in(&st)
}

/// As [`road_diagnostic_telemetry_snapshot`] with the state already locked.
pub fn road_diagnostic_telemetry_snapshot_in(st: &State) -> Value {
    let t = &st.tel;
    let r = &t.last_slip_result;
    json!({
        "position": {"x": jf(t.last_pos_x), "y": jf(t.last_pos_y), "z": jf(t.last_pos_z)},
        "speed_ms": jf(t.last_speed_ms),
        "ground_speed_ms": jf(t.last_ground_speed_ms),
        "wheel_speed_ms": jf(t.last_speed_ms),
        "throttle": jf(t.last_throttle),
        "brake": jf(t.last_brake),
        "steering": jf(t.last_steering),
        "actual_steering": jf(t.last_actual_steering),
        "steering_input": jf(t.last_steering_input),
        "heading": jf(t.last_heading),
        "roll_rad": jf(t.last_roll_rad),
        "pitch_rad": jf(t.last_pitch_rad),
        "gear": t.last_gear_str,
        "traction_control_active": t.last_tc_active,
        "slip_detector_enabled": t.slip_mode_active,
        "slip_active": t.last_slip_active,
        "slip_kind": t.last_slip_kind,
        "slip_magnitude_mps": jf(t.last_slip_mag),
        "slip_ratio": jf(r.ratio),
        "slip_filtered_mps": jf(r.filtered_ms),
        "slip_legacy_rule": r.legacy,
        "travel_bearing_deg": travel_bearing_or_none(st).map(jf),
        "longitudinal_speed_ms": jf(r.v_long_ms),
        "wheel_max_driven_ms": jf(t.last_wheel_max_driven),
        "wheel_min_ms": jf(t.last_wheel_min),
        "slide_active": r.slide_active,
        "slide_deg": jf(r.slide_deg),
        "slip_azimuth_deg": jf(r.azimuth_deg),
    })
}

/// `_mcp_snapshot_state(sections=None)`: a flat snapshot of live state for the MCP server.
/// Everything inside `state_lock` is copied out under one guard, as in the Python.
pub fn mcp_snapshot_state(sections: Option<&[&str]>) -> Value {
    let on = |name: &str| sections.is_none_or(|s| s.contains(&name));
    let mut out = Map::new();
    {
        let st = state();
        let t = &st.tel;
        if on("telemetry") {
            let engine_temp = has_engine_temp(&st);
            let electric = if has_electric_drive(&st) {
                json!({
                    "motor_power_kw": jf(t.last_motor_power_kw),
                    "motor_torque_nm": jf(t.last_motor_torque_nm),
                    "range_m": if t.last_ev_range_m >= 0.0 { jf(t.last_ev_range_m) } else { Value::Null },
                })
            } else {
                Value::Null
            };
            out.insert(
                "telemetry".into(),
                json!({
                    "speed_ms": jf(t.last_speed_ms),
                    "rpm": jf(t.last_rpm),
                    "rpm_max": jf(t.last_rpm_max),
                    "gear": t.last_gear_str,
                    "fuel": jf(t.last_fuel),
                    "turbo": jf(t.last_turbo),
                    "engine_temp": if engine_temp { jf(t.last_engtemp) } else { Value::Null },
                    "oil_temp": if engine_temp { jf(t.last_oiltemp) } else { Value::Null },
                    "air_pressure_psi": jf(t.last_air_pressure),
                    "air_pressure_max_psi": jf(t.last_air_pressure_max),
                    "trip_m": if t.last_trip_m >= 0.0 { jf(t.last_trip_m) } else { Value::Null },
                    "odometer_m": if t.last_odometer_m >= 0.0 { jf(t.last_odometer_m) } else { Value::Null },
                    "lifetime_m": if t.last_lifetime_m >= 0.0 { jf(t.last_lifetime_m) } else { Value::Null },
                    "has_air_system": has_air_system(&st),
                    "electric": electric,
                    "oil_pressure": jf(t.last_oil_pressure),
                    "throttle": jf(t.last_throttle),
                    "brake": jf(t.last_brake),
                    "clutch": jf(t.last_clutch),
                    "steering": jf(t.last_steering),
                    "actual_steering": jf(t.last_actual_steering),
                    "heading": jf(t.last_heading),
                    "ground_speed_ms": jf(t.last_ground_speed_ms),
                }),
            );
        }
        if on("position") {
            out.insert(
                "position".into(),
                json!({
                    "x": jf(t.last_pos_x), "y": jf(t.last_pos_y), "z": jf(t.last_pos_z),
                    "yaw_rad": jf(t.last_yaw_rad), "roll_rad": jf(t.last_roll_rad),
                    "pitch_rad": jf(t.last_pitch_rad),
                }),
            );
        }
        if on("implement") {
            out.insert(
                "implement".into(),
                json!({
                    "flags": jf(t.last_implement_flags),
                    "edge_height_m": jf(t.last_implement_edge_height),
                    "min_clearance_m": jf(t.last_implement_min_clearance),
                    "tilt_deg": jf(t.last_implement_tilt_deg),
                    "tilt_percent": jf(t.last_implement_tilt_percent),
                    "lift_m": jf(t.last_implement_lift),
                    "activity": jf(t.last_implement_activity),
                    "articulation_deg": jf(t.last_articulation_deg),
                }),
            );
        }
        if on("trailer") {
            let tr = &st.trailer;
            out.insert(
                "trailer".into(),
                json!({
                    "coupled": tr.last_trailer_id.is_some(),
                    "id": tr.last_trailer_id,
                    "name": tr.last_trailer_name,
                    "angle_deg": jf(tr.last_trailer_deg),
                    "artic_norm": jf(crate::listeners::trailer::trailer_artic_norm(&st)),
                    "age_s": if tr.last_trailer_stamp > 0.0 {
                        jf(monotonic() - tr.last_trailer_stamp)
                    } else {
                        Value::Null
                    },
                }),
            );
        }
        if on("dock") {
            let d = &st.dock;
            out.insert(
                "dock".into(),
                json!({
                    "mode_active": d.dock_mode_active,
                    "last_dock": to_json(&d.last_dock),
                    "last_fail": d.last_dock_fail,
                    "mode": d.last_dock_mode,
                    "name": d.last_dock_name,
                }),
            );
        }
        if on("cannon") {
            let d = &st.dock;
            out.insert(
                "cannon".into(),
                json!({
                    "active": d.cannon_active,
                    "kind": d.cannon_kind,
                    "aim": to_json(&d.last_cannon_aim),
                    "last_shot": to_json(&d.last_cannon_shot),
                    "session_shots": d.cannon_shot_session.len(),
                }),
            );
        }
        if on("scanner") {
            let s = &st.scanner;
            let dist = s.last_scanner_distance;
            out.insert(
                "scanner".into(),
                json!({
                    "target_name": s.last_scanner_target_name,
                    "distance_m": if dist == f64::INFINITY { Value::Null } else { jf(dist) },
                    "bearing_deg": jf(s.last_scanner_bearing),
                    "approach_deg": jf(s.last_scanner_approach_deg),
                    "reference_reversed": s.scanner_ref_reversed,
                }),
            );
        }
        if on("modes") {
            out.insert(
                "modes".into(),
                json!({
                    "scan": st.scanner.scan_mode_active,
                    "coupler_distance": st.scanner.coupler_dist_mode,
                    "obstacle": st.obstacle.obstacle_mode_active,
                    "road": st.road_mode_active,
                    "dock": st.dock.dock_mode_active,
                }),
            );
        }
        if on("liveness") {
            let ever = t.last_telemetry_ts > 0.0;
            let age = time_time() - t.last_telemetry_ts;
            out.insert(
                "liveness".into(),
                json!({
                    "world_active": ever && age < WORLD_ACTIVE_TELEMETRY_S,
                    "seconds_since_telemetry": if ever { jf(py_round(age, 2)) } else { Value::Null },
                    "telemetry_ever_seen": ever,
                    "world_active_threshold_s": jf(WORLD_ACTIVE_TELEMETRY_S),
                }),
            );
        }
    }
    if on("assistant") {
        let mut snap = assistant_snapshot_json(&driving_assistant().snapshot());
        if let Value::Object(m) = &mut snap {
            m.insert(
                "diagnostics".into(),
                pyjson_to_json(&assistant_diagnostics().status()),
            );
        }
        out.insert("assistant".into(), snap);
    }
    if on("road") {
        let mut snap = feed_snapshot_json(&road_guidance_feed().snapshot());
        let proximity = audio_controller_ref()
            .map(|c| junction_proximity_snapshot_json(&c.junction_proximity_snapshot()))
            .unwrap_or(Value::Null);
        if let Value::Object(m) = &mut snap {
            m.insert("proximity_audio".into(), proximity);
        }
        out.insert("road".into(), snap);
    }
    if on("slots") {
        let s = slots();
        let vehicles: Map<String, Value> = s
            .vehicle_slots
            .iter()
            .map(|(k, v)| (k.to_string(), to_json(v)))
            .collect();
        out.insert(
            "slots".into(),
            json!({
                "vehicles": vehicles,
                "selected": s.selected_slots.iter().collect::<Vec<_>>(),
                "target": s.target_slot,
            }),
        );
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_values_match_python() {
        let st = State::default();
        assert_eq!(st.tel.protocol_mode, "outgauge");
        assert_eq!(st.tel.last_telemetry_presence, 15);
        assert_eq!(st.tel.last_trip_m, -1.0);
        assert_eq!(st.tel.last_up_z, 1.0);
        assert_eq!(st.tel.last_announced_compass_idx, -1);
        assert!(st.scanner.last_scanner_distance.is_infinite());
        assert_eq!(st.dock.cannon_kind, "NONE");
        assert_eq!(st.camera.cam_veh_distance, -1.0);
        assert!(st.camera.cam_agl_valid);
        assert_eq!(st.menu.current_buffer_index, -1);
        assert_eq!(st.ai.ai_avoid_mode, "auto");
        assert_eq!(st.lineup.lineup_mode, "head_on");
        assert_eq!(st.cfg.compass_highlight_nth_click, 6);
        assert_eq!(st.cfg.obstacle_warning_sensitivity, "normal");
    }

    #[test]
    fn snapshot_has_every_python_section() {
        let snap = mcp_snapshot_state(None);
        for key in [
            "telemetry",
            "position",
            "implement",
            "trailer",
            "dock",
            "cannon",
            "scanner",
            "modes",
            "liveness",
            "assistant",
            "road",
            "slots",
        ] {
            assert!(snap.get(key).is_some(), "missing {key}");
        }
        assert_eq!(snap["scanner"]["distance_m"], Value::Null);
        assert_eq!(snap["liveness"]["telemetry_ever_seen"], Value::Bool(false));
        assert_eq!(snap["liveness"]["seconds_since_telemetry"], Value::Null);
        // The real driving assistant, feed and (absent) audio controller.
        assert_eq!(snap["assistant"]["following_route"], Value::Bool(false));
        assert_eq!(snap["assistant"]["sequence"], json!(-1));
        assert!(snap["assistant"]["diagnostics"].is_object());
        assert_eq!(snap["road"]["mode"], json!("unavailable"));
        assert_eq!(snap["road"]["packet"], Value::Null);
        assert_eq!(snap["road"]["proximity_audio"], Value::Null);
        let only = mcp_snapshot_state(Some(&["modes"]));
        assert_eq!(only.as_object().unwrap().len(), 1);
        let road = road_diagnostic_telemetry_snapshot();
        assert_eq!(road["travel_bearing_deg"], Value::Null);
        assert_eq!(road["gear"], Value::Null);
        assert!(!world_is_active());
    }
}
