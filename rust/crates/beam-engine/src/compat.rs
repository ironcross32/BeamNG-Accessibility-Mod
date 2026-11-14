//! TEMPORARY SHIMS. Delete at integration (workstream I1).
//!
//! When beam-engine's skeleton was written, the wave-1 crates it calls (beam-base's
//! config/gear_report/secretstore/ai_registry, beam-road, beam-reports, beam-speech,
//! beam-audio's controller) were empty files being written by other agents. So that this
//! crate compiles and every wave-2 agent can code against a stable name today, each item
//! beamtel.py uses from another module is declared here **at the path the naming rule
//! gives the real item**, with the Python signature and an inert body:
//!
//! ```text
//! crate::compat::beam_speech::speech::speak      ->  beam_speech::speech::speak
//! crate::compat::beam_audio::controller::AudioController
//!                                                ->  beam_audio::controller::AudioController
//! ```
//!
//! Integration replaces every `crate::compat::` prefix with nothing (and adapts any
//! signature the real crate chose differently), then deletes this file. Nothing outside
//! this file may define a stand-in for another crate's item.
//!
//! Shapes chosen here for Python dicts (`serde_json::Value`) are placeholders, not a
//! contract: the real crate's types win.
#![allow(dead_code, unused_variables, clippy::too_many_arguments)]

pub use serde_json::{Map, Value};

/// A loaded config: Python's `dict` from `load_config()`, insertion-ordered.
pub type ConfigMap = Map<String, Value>;

pub mod beam_base {
    pub mod config {
        use super::super::{ConfigMap, Value};
        /// beamtel.py `load_config()` (B1).
        pub fn load_config() -> ConfigMap {
            default_config()
        }
        /// beamtel.py `DEFAULT_CONFIG` (B1). Only the keys beam-engine reads are listed.
        pub fn default_config() -> ConfigMap {
            let mut m = ConfigMap::new();
            for (k, v) in [
                ("units", Value::from("imperial")),
                ("telemetry_protocol", Value::from("extended")),
                ("launch_beamng", Value::from(false)),
                ("mcp_server_enabled", Value::from(false)),
                ("mcp_server_port", Value::from(4481)),
                ("ai_describer_provider", Value::from("gemini")),
                ("ai_describer_api_key", Value::from("")),
                ("ai_describer_model", Value::from("models/gemini-3-flash-preview")),
                ("ai_describer_openai_api_key", Value::from("")),
                ("ai_describer_openai_model", Value::from("gpt-5.6-terra")),
                (
                    "ai_describer_openai_base_url",
                    Value::from("https://api.openai.com/v1"),
                ),
                ("ai_describer_openai_reasoning_effort", Value::from("low")),
                ("ai_describer_openai_detail", Value::from("auto")),
                ("ai_describer_disable_ui_toggle", Value::from(false)),
            ] {
                m.insert(k.to_string(), v);
            }
            m
        }
    }

    pub mod gear_report {
        use super::super::ConfigMap;
        /// `gear_report.settings_from_config(cfg)`'s dict.
        #[derive(Debug, Clone, PartialEq)]
        pub struct GearReportSettings {
            pub mode: String,
            pub collapse_sport: bool,
            pub number_style: String,
        }
        pub fn settings_from_config(cfg: &ConfigMap) -> GearReportSettings {
            GearReportSettings {
                mode: "speech".into(),
                collapse_sport: false,
                number_style: "ordinal".into(),
            }
        }
        pub fn gear_to_phrase(gear_byte: i64) -> String {
            String::new()
        }
        pub fn extended_gear_to_phrase(gear: &str) -> String {
            String::new()
        }
        pub fn sport_step_silenced(new: &str, old: Option<&str>, collapse_sport: bool) -> bool {
            false
        }
        pub fn extended_change_phrase(
            new: &str,
            old: Option<&str>,
            collapse_sport: bool,
            number_style: &str,
        ) -> Option<String> {
            None
        }
        pub fn extended_gear_degree(gear: &str) -> Option<i64> {
            None
        }
        pub fn is_sport(gear: &str) -> bool {
            false
        }
        pub fn outgauge_change_phrase(gear_byte: i64, number_style: &str) -> Option<String> {
            None
        }
        pub fn outgauge_gear_degree(gear_byte: i64) -> Option<i64> {
            None
        }
    }

    pub mod secretstore {
        /// `secretstore.unprotect(value)`: None when a sealed value will not decrypt.
        pub fn unprotect(value: &str) -> Option<String> {
            Some(value.to_string())
        }
    }

    pub mod ai_registry {
        use super::super::{ConfigMap, Map};
        pub const DEFAULT_PROVIDER: &str = "gemini";
        pub fn config_keys_for(provider: &str) -> (String, String) {
            (
                "ai_describer_api_key".into(),
                "ai_describer_model".into(),
            )
        }
        pub fn default_model_for(provider: &str) -> String {
            String::new()
        }
        pub fn provider_display_name(provider: &str) -> String {
            provider.to_string()
        }
        pub fn extra_args_for(provider: &str, cfg: &ConfigMap) -> Map<String, super::super::Value> {
            Map::new()
        }
    }
}

pub mod beam_speech {
    pub mod speech {
        use super::super::ConfigMap;
        /// `speech.speak(text, interrupt)`.
        pub fn speak(text: &str, interrupt: bool) {}
        /// `speech.stop()`.
        pub fn stop() {}
        /// `speech.init(cfg)`: the backend, or None when no backend is available.
        pub fn init(cfg: Option<&ConfigMap>) -> Option<String> {
            None
        }
        /// `speech.configure(cfg)`.
        pub fn configure(cfg: &ConfigMap) {}
        /// `speech.describe_capabilities()`.
        pub fn describe_capabilities() -> String {
            String::new()
        }
    }
}

pub mod beam_audio {
    pub mod consts {
        pub const DOCK_RAMP_MAX_RANGE_M: f64 = 25.0;
        /// `SCAN_FAMILY_CODES`, as a lookup.
        pub fn scan_family_code(code: &str) -> Option<&'static str> {
            match code {
                "d" => Some("dirt"),
                "v" => Some("gravel"),
                "g" => Some("grass"),
                "i" => Some("ice"),
                "r" => Some("paved"),
                _ => None,
            }
        }
    }

    pub mod controller {
        use super::super::{ConfigMap, Value};

        /// The dict `telemetry_loop` hands to `update_telemetry_state` every packet.
        #[derive(Debug, Clone, Default)]
        pub struct TelemetryFrame {
            pub shift_active: bool,
            pub tc_active: bool,
            pub pedal_tones_active: bool,
            pub last_clutch: f64,
            pub last_brake: f64,
            pub last_throttle: f64,
            pub last_steering: f64,
            pub last_actual_steering: f64,
            pub last_steering_input: f64,
            pub trailer_artic: f64,
            pub trailer_reverse: bool,
            pub implement_flags: f64,
            pub implement_min_clearance: f64,
            pub implement_tilt_deg: f64,
            pub implement_lift: f64,
            pub implement_activity: f64,
            pub inverted: bool,
            pub last_roll_rad: f64,
            pub last_pitch_rad: f64,
            pub guidance_active: bool,
            pub guidance_error_deg: f64,
            pub drift_alert_active: bool,
            pub drift_pan: f64,
            pub drift_rate: f64,
            pub ls_clicks_active: bool,
            pub ls_speed_mph: f64,
            pub scan_speed_ms: f64,
            pub ls_accel: f64,
            pub ls_stopped: bool,
            pub slip_active: bool,
            pub slip_kind: i64,
            pub slip_mag: f64,
            pub slip_ratio: f64,
            pub slide_active: bool,
            pub slide_deg: f64,
            pub slip_azimuth_deg: f64,
            pub coord_guidance_active: bool,
            pub coord_guidance_error_deg: f64,
        }

        /// One scan sample: (bearing, range, elevation or None, depth or None, family or None).
        pub type ScanSample = (f64, f64, Option<f64>, Option<f64>, Option<&'static str>);

        /// `audio.AudioController`. Every method beamtel.py calls, with &self (the Python
        /// object guards itself with its own lock).
        #[derive(Debug, Default)]
        pub struct AudioController {
            pub junction_proximity_enabled: bool,
        }

        impl AudioController {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn apply_config(&self, cfg: &ConfigMap) {}
            pub fn select_hrtf_set(
                &self,
                set_id: Option<&str>,
                base_dir: &std::path::Path,
                elevation: Option<f64>,
                equalise: Option<bool>,
            ) {
            }
            pub fn start(&self) {}
            pub fn stop(&self) {}
            pub fn junction_proximity_enabled(&self) -> bool {
                self.junction_proximity_enabled
            }
            pub fn junction_proximity_snapshot(&self) -> Value {
                Value::Null
            }
            pub fn update_telemetry_state(&self, frame: &TelemetryFrame) {}
            pub fn clear_cannon_aim(&self) {}
            pub fn clear_dock_target(&self) {}
            pub fn clear_road_audio(&self) {}
            pub fn render_and_play_scan(
                &self,
                samples: Vec<ScanSample>,
                objects: Vec<(f64, f64, f64, String)>,
                reach: f64,
                pois: Vec<(f64, f64, f64)>,
            ) {
            }
            pub fn set_coupler_tracking(&self, on: bool) {}
            pub fn set_dock_mode(&self, on: bool) {}
            pub fn set_lineup(&self, on: bool, amp: f64, index: f64, ratio: f64, az: f64) {}
            pub fn set_obstacle_mode(&self, on: bool) {}
            pub fn set_road_mode(&self, on: bool) {}
            pub fn set_route_beacon_mode(&self, on: bool) {}
            pub fn set_scan_mode(&self, on: bool) {}
            pub fn trigger_assistant_cue(&self, kind: &str, choices: &[Value]) {}
            pub fn trigger_cam_compass_click(&self, heading: f64, pitch_mult: f64) {}
            pub fn trigger_cam_compass_highlight(&self, heading: f64, pitch_mult: f64) {}
            pub fn trigger_clickspot_beep(&self, reverse: bool) {}
            pub fn trigger_compass_click(&self, heading: f64, pitch_mult: f64) {}
            pub fn trigger_compass_highlight(&self, heading: f64, pitch_mult: f64) {}
            pub fn trigger_describe_error_buzz(&self) {}
            pub fn trigger_dock_approach_cue(&self) {}
            pub fn trigger_entry_cue(&self) {}
            pub fn trigger_gear_tone(&self, degree: i64, sport: bool) {}
            pub fn trigger_lowspeed_stopped(&self) {}
            pub fn trigger_node_hover_beep(&self, height_norm: f64, reverse: bool) {}
            pub fn trigger_oil_chime(&self) {}
            pub fn trigger_placement_ping(&self, rung: i64, azimuth_deg: f64, pitch_mult: f64) {}
            pub fn trigger_road_junction_earcon(&self) {}
            pub fn trigger_road_junction_entry_earcon(&self) {}
            pub fn trigger_road_orientation_chime(&self, first: f64, second: Option<f64>) {}
            pub fn trigger_slam_cue(&self, cue: &str) {}
            pub fn update_cannon_aim(
                &self,
                elevation: f64,
                bearing: f64,
                target_angle: f64,
                solution_available: bool,
                reachable: bool,
            ) {
            }
            pub fn update_coupler_target(&self, bearing: f64, distance: f64, in_range: bool) {}
            pub fn update_dock_target(
                &self,
                range_m: f64,
                lateral_m: f64,
                null_value: f64,
                mode: &str,
                acquire: bool,
                bearing_deg: f64,
            ) {
            }
            pub fn update_junction_proximity(&self, target: Option<&Value>, enabled: bool) {}
            pub fn update_obstacle(&self, pkt_type: i64, bearing: f64, urgency: i64, distance: f64) {}
            /// Returns the audio scheduling event the road recorder logs (None for none).
            pub fn update_road_guidance(
                &self,
                state: &str,
                off_road: Option<&Value>,
                correction: Option<&Value>,
                enabled: bool,
            ) -> Value {
                Value::Null
            }
            pub fn update_road_state(&self, on_road: bool, bearing: f64, distance: f64) {}
            pub fn update_route_beacon(&self, active: bool, bearing: f64, distance: f64) {}
            pub fn update_scanner_target(&self, bearing: f64, distance: f64) {}
            /// `None` clears the hazard (the "0" packet).
            pub fn update_selected_hazard(&self, hazard: Option<&Value>) {}
        }
    }
}

pub mod beam_road {
    pub mod road_guidance {
        use super::super::Value;
        use beam_base::pyerr::ValueError;
        pub fn parse_r2_packet(text: &str) -> Result<Value, ValueError> {
            Err(ValueError::new("road_guidance not ported yet"))
        }
        pub fn parse_surface_packet(text: &str) -> Option<(String, bool)> {
            None
        }
        pub fn surface_phrase(word: &str, on_road: bool) -> String {
            String::new()
        }
        pub fn junction_phrase(junction: &Value, units: &str) -> String {
            String::new()
        }
        pub fn signal_phrase(signal: &Value, units: &str, change: &Value) -> String {
            String::new()
        }
        pub fn one_way_phrase(direction: f64) -> String {
            String::new()
        }
        pub fn map_speed_limit_phrase(value: f64, units: &str) -> String {
            String::new()
        }

        /// `RoadGuidanceFeed`. Internally locked like the Python object, shared by `&self`.
        #[derive(Debug, Default)]
        pub struct RoadGuidanceFeed;
        impl RoadGuidanceFeed {
            pub fn new() -> Self {
                RoadGuidanceFeed
            }
            pub fn accept_r2(
                &self,
                packet: &Value,
                stop_signs: bool,
                traffic_lights: bool,
                speed_limits: bool,
            ) -> Value {
                Value::Null
            }
            /// `accept_legacy(state, bearing=, distance=, directions=)`.
            pub fn accept_legacy(
                &self,
                state: &str,
                bearing: Option<f64>,
                distance: Option<f64>,
                directions: &[f64],
            ) -> bool {
                false
            }
            pub fn arm_orientation(&self) {}
            pub fn check_timeout(&self) -> bool {
                false
            }
            pub fn reset(&self) {}
            pub fn snapshot(&self) -> Value {
                Value::Null
            }
            pub fn speed_limit(&self, active: bool) -> Option<f64> {
                None
            }
            pub fn speed_limit_phrase(&self, active: bool, units: &str, label: &str) -> String {
                String::new()
            }
            pub fn status_phrase(&self, active: bool, units: &str) -> String {
                String::new()
            }
        }
    }

    pub mod road_diagnostics {
        use super::super::Value;
        #[derive(Debug)]
        pub struct RoadDiagnosticRecorder {
            pub root: std::path::PathBuf,
        }
        impl RoadDiagnosticRecorder {
            pub fn new(root: std::path::PathBuf) -> Self {
                RoadDiagnosticRecorder { root }
            }
            pub fn record(&self, packet: &Value, telemetry: &Value, audio_event: &Value) {}
            pub fn start(&self, label: &str) -> Value {
                Value::Null
            }
            pub fn stop(&self) -> Value {
                Value::Null
            }
            pub fn mark(&self, note: Option<&str>) -> Value {
                Value::Null
            }
            pub fn review(&self, session: Option<&str>) -> Value {
                Value::Null
            }
            pub fn list_sessions(&self, limit: i64) -> Value {
                Value::Null
            }
            pub fn status(&self) -> Value {
                serde_json::json!({"active": false})
            }
        }
    }

    pub mod assistant_diagnostics {
        use super::super::Value;
        #[derive(Debug)]
        pub struct AssistantDiagnosticRecorder {
            pub root: std::path::PathBuf,
        }
        impl AssistantDiagnosticRecorder {
            pub fn new(root: std::path::PathBuf) -> Self {
                AssistantDiagnosticRecorder { root }
            }
            pub fn record_road(&self, packet: &Value, proximity: &Value) {}
            pub fn control(&self, action: &str, kwargs: &Value) -> Value {
                Value::Null
            }
            pub fn status(&self) -> Value {
                Value::Null
            }
        }
    }

    pub mod challenge_results {
        use super::super::Value;
        pub type FinalizeCallback = Box<dyn Fn(Value, bool) + Send + Sync>;
        #[derive(Default)]
        pub struct HillClimbChallengeRecorder {
            pub root: std::path::PathBuf,
        }
        impl HillClimbChallengeRecorder {
            pub fn new(root: std::path::PathBuf) -> Self {
                HillClimbChallengeRecorder { root }
            }
            pub fn record(&self, packet: &Value, telemetry: &Value) {}
            pub fn handle_event(&self, data: &Value, telemetry: &Value) -> Option<Value> {
                None
            }
            pub fn set_finalize_callback(&self, cb: FinalizeCallback) {}
            pub fn is_capturing(&self) -> bool {
                false
            }
            pub fn shutdown(&self) {}
            pub fn list_reports(&self, limit: Option<usize>) -> Vec<Value> {
                Vec::new()
            }
            pub fn report_lines(&self, summary: &Value, units: &str) -> Vec<String> {
                Vec::new()
            }
            pub fn history_line(&self, summary: &Value, units: &str) -> String {
                String::new()
            }
        }
        pub fn completion_speech(summary: &Value) -> String {
            String::new()
        }
    }

    pub mod assistant_route {
        pub fn suppressed_toast(
            text: &str,
            following_route: bool,
            seconds_since_arrival: Option<f64>,
        ) -> bool {
            false
        }
    }

    pub mod driving_assistant {
        use super::super::Value;
        use crate::timer::Event;

        /// `DrivingAssistant(send_ai_command, say, cue, diagnostics, units=, on_arrival=)`.
        pub struct Callbacks {
            pub send_ai_command: Box<dyn Fn(&str) + Send + Sync>,
            pub say: Box<dyn Fn(&str, bool, bool, &str) + Send + Sync>,
            pub cue: Box<dyn Fn(&str, &[Value]) + Send + Sync>,
            pub units: Box<dyn Fn() -> String + Send + Sync>,
            pub on_arrival: Box<dyn Fn(&Value) + Send + Sync>,
        }

        pub struct DrivingAssistant {
            pub callbacks: Callbacks,
        }
        impl DrivingAssistant {
            pub fn new(callbacks: Callbacks) -> Self {
                DrivingAssistant { callbacks }
            }
            /// The `active` attribute.
            pub fn active(&self) -> bool {
                false
            }
            /// The `vehicle` attribute.
            pub fn vehicle(&self) -> Option<i64> {
                None
            }
            pub fn receive(&self, payload: &str) {}
            pub fn run(&self, stop_event: &Event) {}
            pub fn snapshot(&self) -> Value {
                serde_json::json!({"following_route": false})
            }
            pub fn seconds_since_arrival(&self) -> Option<f64> {
                None
            }
            pub fn toggle(&self) {}
        }
    }
}

pub mod beam_reports {
    pub mod wheel_slip {
        pub const DIR_MIN_MS: f64 = 0.5;
        #[derive(Debug, Clone, Copy, PartialEq, Default)]
        pub struct SlipResult {
            pub active: bool,
            pub kind: i64,
            pub magnitude_ms: f64,
            pub slide_active: bool,
            pub slide_deg: f64,
            pub azimuth_deg: f64,
            pub v_long_ms: f64,
            pub filtered_ms: f64,
            pub legacy: bool,
            pub ratio: f64,
        }
        pub const INACTIVE: SlipResult = SlipResult {
            active: false,
            kind: 0,
            magnitude_ms: 0.0,
            slide_active: false,
            slide_deg: 0.0,
            azimuth_deg: 0.0,
            v_long_ms: 0.0,
            filtered_ms: 0.0,
            legacy: false,
            ratio: 0.0,
        };
        #[derive(Debug, Clone, Default)]
        pub struct SlipDetector;
        impl SlipDetector {
            pub fn new() -> Self {
                SlipDetector
            }
            pub fn reset(&mut self) {}
            pub fn step(
                &mut self,
                now: f64,
                ground_ms: f64,
                travel_bearing: Option<f64>,
                wheel_max_driven: f64,
                wheel_min: f64,
                wheel_speed: f64,
                in_reverse: bool,
            ) -> SlipResult {
                INACTIVE
            }
        }
        pub fn travel_bearing(vel_x: f64, vel_y: f64, heading_deg: f64) -> f64 {
            0.0
        }
    }

    pub mod wheel_report {
        use super::super::Value;
        /// `parse_packet(text)`: the vehicles list, or None.
        pub fn parse_packet(text: &str) -> Option<Value> {
            None
        }
        /// `all_rows(vehicles)`: dicts with label, broken, psi, brake_c, ...
        pub fn all_rows(vehicles: &Value) -> Vec<Value> {
            Vec::new()
        }
    }

    pub mod camera_lineup {
        pub const HEAD_ON: &str = "head_on";
        pub const CROSSING: &str = "crossing";
        pub const DEFAULT_OFFSET_M: f64 = 3.0;
        pub const DEFAULT_DEPTH_M: f64 = 10.0;
        #[derive(Debug, Clone, Copy, PartialEq, Default)]
        pub struct Geometry {
            pub cam_x: f64,
            pub cam_y: f64,
            pub cam_fx: f64,
            pub cam_fy: f64,
            pub nose_x: f64,
            pub nose_y: f64,
        }
        #[derive(Debug, Clone, Copy, PartialEq, Default)]
        pub struct Tone {
            pub amp: f64,
            pub index: f64,
            pub ratio: f64,
            pub az: f64,
        }
        /// `parse_packet`: 'OFF', a Geometry, or None.
        pub enum Parsed {
            Off,
            Geom(Geometry),
        }
        pub fn parse_packet(text: &str) -> Option<Parsed> {
            None
        }
        pub fn solve(
            geom: &Geometry,
            mode: &str,
            offset_m: f64,
            depth_m: f64,
        ) -> (String, Option<f64>, Option<f64>) {
            ("unknown".into(), None, None)
        }
        pub fn tone_for(status: &str, miss: Option<f64>, bearing: Option<f64>) -> Tone {
            Tone::default()
        }
        pub fn step_value(mode: &str, offset_m: f64, depth_m: f64, delta: i64) -> (f64, f64) {
            (offset_m, depth_m)
        }
        pub fn target_phrase(mode: &str, offset_m: f64, depth_m: f64, units: &str) -> String {
            String::new()
        }
        pub fn value_phrase(mode: &str, offset_m: f64, depth_m: f64, units: &str) -> String {
            String::new()
        }
    }

    pub mod audio_debug {
        use super::super::Value;
        pub fn parse_fields(payload: &str) -> Value {
            Value::Null
        }
        pub fn row_line(row: &Value) -> String {
            String::new()
        }
        pub fn sfx_line(row: &Value) -> String {
            String::new()
        }
        pub fn select_target(row: &Value, delta: i64) -> Option<String> {
            None
        }
        pub fn stepped_value(row: &Value, delta: i64) -> Option<f64> {
            None
        }
        pub fn wire_number(value: f64) -> String {
            String::new()
        }
    }

    pub mod fuel_stations {
        use super::super::Value;
        use beam_base::pyerr::ValueError;
        pub fn parse_row(text: &str) -> Result<Value, ValueError> {
            Err(ValueError::new("fuel_stations not ported yet"))
        }
        pub fn parse_event(text: &str) -> Result<Value, ValueError> {
            Err(ValueError::new("fuel_stations not ported yet"))
        }
        pub fn row_line(row: &Value, units: &str) -> String {
            String::new()
        }
        pub fn event_phrase(data: &Value) -> Option<String> {
            None
        }
    }

    pub mod induction_report {
        use super::super::Value;
        pub fn parse_reply(text: &str) -> Option<Value> {
            None
        }
        /// `report_phrase(report, fmt_pressure)`.
        pub fn report_phrase(
            report: &Value,
            fmt_pressure: &dyn Fn(f64) -> (crate::units::PyVal, &'static str),
        ) -> String {
            String::new()
        }
    }

    pub mod poi_guidance {
        use super::super::Value;
        use beam_base::pyerr::ValueError;
        pub fn parse_poi_packet(text: &str) -> Result<Value, ValueError> {
            Err(ValueError::new("poi_guidance not ported yet"))
        }
        pub fn poi_phrase(poi: &Value, units: &str) -> String {
            String::new()
        }
    }

    pub mod drift_speech {
        use super::super::Value;
        /// `DriftAnnouncer(say)`. Internally locked; shared by `&self`.
        #[derive(Default)]
        pub struct DriftAnnouncer;
        impl DriftAnnouncer {
            pub fn new() -> Self {
                DriftAnnouncer
            }
            pub fn quiet(&self) -> bool {
                false
            }
            pub fn set_enabled(&self, enabled: bool) {}
            pub fn handle(&self, data: &Value) {}
        }
    }
}
