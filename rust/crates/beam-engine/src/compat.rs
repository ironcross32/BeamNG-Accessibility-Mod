//! TEMPORARY SHIMS. Delete at integration (workstream I1).
//!
//! Each item here stands in for a real item of another crate, declared **at the path the
//! real item has** (`crate::compat::beam_audio::controller::AudioController` ->
//! `beam_audio::controller::AudioController`). Integration replaces the `crate::compat::`
//! prefix with nothing, adapts the noted differences, and deletes this file. Nothing outside
//! this file may define a stand-in for another crate's item, and wave-2 agents call the real
//! crates directly for everything NOT listed here (config, gear_report, secretstore,
//! ai_registry, road_guidance's free functions, wheel_report, wheel_slip, camera_lineup,
//! audio_debug, fuel_stations, induction_report, poi_guidance, assistant_route, the two
//! diagnostic recorders, beam_speech all exist and are used directly).
//!
//! Why each shim is still here (state when EA finished):
//!
//! * `beam_audio::controller::AudioController`: the real one exists with the signatures
//!   mirrored below, but beam-audio did not compile (waveforms.rs pending), and beam-engine
//!   could not be type-checked against it. The shim's signatures are copied from the real
//!   `controller.rs`/`device.rs`, so the swap is a path change, except
//!   `junction_proximity_snapshot`, whose real return type has no JSON form yet (see
//!   [`beam_audio::controller::junction_proximity_snapshot_json`]).
//! * `beam_road::driving_assistant::DrivingAssistant`, `beam_road::road_guidance::RoadGuidanceFeed`,
//!   `beam_road::challenge_results::HillClimbChallengeRecorder`,
//!   `beam_reports::drift_speech::DriftAnnouncer`: the real types exist, but their
//!   constructors take the callbacks in a different shape (`SpeakOpts`, clocks, `Result`
//!   from `new`) and their `snapshot()`s return typed structs with no JSON form, which
//!   `state::mcp_snapshot_state` needs. Integration adapts `state.rs`'s four `LazyLock`s and
//!   adds `to_json` for `AssistantSnapshot` / `FeedSnapshot`.
#![allow(dead_code, unused_variables, clippy::too_many_arguments)]

pub use serde_json::{Map, Value};

/// A loaded config as JSON (`beam_base::pyval::to_json_map` of the real `PyDict`).
pub type ConfigMap = Map<String, Value>;

pub mod beam_audio {
    pub mod controller {
        use std::path::Path;
        use std::sync::Arc;

        use beam_base::pyval::{PyDict, PyExc};
        use serde_json::{Map, Value};

        pub type JsonMap = Map<String, Value>;

        /// Mirrors `beam_audio::controller::TelemetryState` (None = key absent).
        #[derive(Clone, Debug, Default)]
        pub struct TelemetryState {
            pub shift_active: Option<bool>,
            pub pedal_tones_active: Option<bool>,
            pub tc_active: Option<bool>,
        }

        /// Mirrors `HazardIn`.
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct HazardIn {
            pub bearing: Option<f64>,
            pub urgency: Option<f64>,
            pub gap: Option<f64>,
        }

        /// Mirrors `RoadGuidanceResult`.
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub struct RoadGuidanceResult {
            pub settled_triggered: bool,
        }

        /// Mirrors `JunctionProximitySnapshot` (fields abbreviated).
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct JunctionProximitySnapshot {
            pub enabled: bool,
            pub active: bool,
            pub age_s: f64,
        }

        /// The real snapshot as the dict Python's `junction_proximity_snapshot()` returns.
        /// NOT in beam-audio yet: integration adds it there (or serde derives).
        pub fn junction_proximity_snapshot_json(s: &JunctionProximitySnapshot) -> Value {
            serde_json::json!({"enabled": s.enabled, "active": s.active, "age_s": s.age_s})
        }

        pub mod scan {
            #[derive(Clone, Debug, PartialEq)]
            pub struct ScanSample {
                pub bearing_deg: f64,
                pub range_m: f64,
                pub dz_m: Option<f64>,
            }
            #[derive(Clone, Debug, PartialEq)]
            pub struct ScanObject {
                pub bearing_deg: f64,
                pub range_m: f64,
                pub dz_m: f64,
            }
            #[derive(Clone, Debug, PartialEq)]
            pub struct ScanPoi {
                pub bearing_deg: f64,
                pub range_m: f64,
                pub dz_m: f64,
            }
            #[derive(Clone, Debug, PartialEq)]
            pub struct ScanError(pub String);
        }

        #[derive(Debug, Default)]
        pub struct AudioController;

        impl AudioController {
            pub fn new() -> Arc<Self> {
                Arc::new(AudioController)
            }
            pub fn apply_config(self: &Arc<Self>, cfg: &PyDict) -> Result<(), PyExc> {
                Ok(())
            }
            pub fn select_hrtf_set(
                &self,
                set_id: &str,
                base_dir: &Path,
                elevation_deg: Option<f64>,
                equalise: Option<bool>,
            ) {
            }
            /// device.rs
            pub fn start(self: &Arc<Self>) {}
            /// device.rs
            pub fn stop(&self) {}
            pub fn junction_proximity_enabled(&self) -> bool {
                false
            }
            pub fn junction_proximity_snapshot(&self) -> JunctionProximitySnapshot {
                JunctionProximitySnapshot::default()
            }
            pub fn update_telemetry_state(&self, state: &TelemetryState) {}
            pub fn set_scan_mode(&self, is_active: bool) {}
            pub fn update_scanner_target(&self, bearing: f64, distance: f64) {}
            pub fn update_cannon_aim(
                &self,
                elevation_deg: f64,
                bearing_deg: f64,
                target_angle_deg: f64,
                solution_available: bool,
                reachable: bool,
            ) {
            }
            pub fn clear_cannon_aim(&self) {}
            pub fn set_dock_mode(&self, active: bool) {}
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
            pub fn clear_dock_target(&self) {}
            pub fn trigger_slam_cue(&self, kind: &str) {}
            pub fn trigger_entry_cue(&self) {}
            pub fn trigger_dock_approach_cue(&self) {}
            pub fn render_and_play_scan(
                &self,
                samples: &[scan::ScanSample],
                objects: &[scan::ScanObject],
                reach_m: f64,
                pois: &[scan::ScanPoi],
            ) -> Result<(), scan::ScanError> {
                Ok(())
            }
            pub fn set_coupler_tracking(&self, active: bool) {}
            pub fn update_coupler_target(&self, bearing: f64, distance: f64, in_range: bool) {}
            pub fn set_obstacle_mode(&self, is_active: bool) {}
            pub fn update_obstacle(
                &self,
                obstacle_type: i64,
                bearing: f64,
                urgency: f64,
                distance: f64,
            ) {
            }
            pub fn update_selected_hazard(&self, hazard: Option<&HazardIn>) {}
            pub fn set_road_mode(&self, is_active: bool) {}
            pub fn clear_road_audio(&self) {}
            pub fn set_route_beacon_mode(&self, is_active: bool) {}
            pub fn update_route_beacon(&self, available: bool, bearing_deg: f64, distance_m: f64) {}
            pub fn trigger_assistant_cue(&self, kind: &str, choices: &[&str]) {}
            pub fn update_junction_proximity(&self, target: Option<&JsonMap>, enabled: bool) {}
            pub fn trigger_gear_tone(&self, degree: i64, sport: bool) {}
            pub fn trigger_road_orientation_chime(
                &self,
                bearing_first: f64,
                bearing_second: Option<f64>,
            ) {
            }
            pub fn trigger_road_junction_earcon(&self) {}
            pub fn trigger_road_junction_entry_earcon(&self) {}
            pub fn update_road_state(&self, on_road: bool, bearing: f64, distance: f64) {}
            pub fn update_road_guidance(
                &self,
                state: &str,
                off_road: Option<&JsonMap>,
                correction: Option<&JsonMap>,
                follow_enabled: bool,
            ) -> RoadGuidanceResult {
                RoadGuidanceResult::default()
            }
            pub fn trigger_placement_ping(&self, rung: i64, azimuth_deg: f64, pitch_mult: f64) {}
            pub fn trigger_compass_click(&self, azimuth_deg: f64, pitch_mult: f64) {}
            pub fn trigger_compass_highlight(&self, azimuth_deg: f64, pitch_mult: f64) {}
            pub fn trigger_cam_compass_click(&self, azimuth_deg: f64, pitch_mult: f64) {}
            pub fn trigger_cam_compass_highlight(&self, azimuth_deg: f64, pitch_mult: f64) {}
            pub fn trigger_oil_chime(&self) {}
            pub fn trigger_lowspeed_stopped(&self) {}
            pub fn trigger_node_hover_beep(&self, height_normalized: f64, reverse: bool) {}
            pub fn trigger_clickspot_beep(&self, reverse: bool) {}
            pub fn trigger_describe_error_buzz(&self) {}
            pub fn set_lineup(&self, active: bool, amp: f64, index: f64, ratio: f64, az: f64) {}
        }
    }
}

pub mod beam_road {
    pub mod road_guidance {
        use serde_json::Value;
        /// Stand-in for the real `RoadGuidanceFeed` (whose `snapshot()` is a typed struct).
        #[derive(Debug, Default)]
        pub struct RoadGuidanceFeed;
        impl RoadGuidanceFeed {
            pub fn new() -> Self {
                RoadGuidanceFeed
            }
            /// The real `snapshot()` as JSON.
            pub fn snapshot_json(&self) -> Value {
                Value::Object(Default::default())
            }
        }
    }

    pub mod challenge_results {
        use serde_json::Value;
        pub type FinalizeFn = dyn Fn(Value, bool) + Send + Sync;
        /// Stand-in for the real recorder (whose `new` returns `Result`).
        #[derive(Default)]
        pub struct HillClimbChallengeRecorder;
        impl HillClimbChallengeRecorder {
            pub fn new(directory: std::path::PathBuf) -> Self {
                HillClimbChallengeRecorder
            }
            pub fn set_finalize_callback(&self, callback: Option<std::sync::Arc<FinalizeFn>>) {}
            pub fn is_capturing(&self) -> bool {
                false
            }
            pub fn shutdown(&self) -> Result<(), String> {
                Ok(())
            }
        }
    }

    pub mod driving_assistant {
        use crate::timer::Event;
        use serde_json::Value;

        /// The callbacks `DrivingAssistant(...)` takes in beamtel.py.
        pub struct Callbacks {
            pub send_ai_command: Box<dyn Fn(&str) + Send + Sync>,
            pub say: Box<dyn Fn(&str, bool, bool, &str) + Send + Sync>,
            pub cue: Box<dyn Fn(&str, &[String]) + Send + Sync>,
            pub units: Box<dyn Fn() -> String + Send + Sync>,
            pub on_arrival: Box<dyn Fn(&Value) + Send + Sync>,
        }

        /// Stand-in for the real `DrivingAssistant`.
        pub struct DrivingAssistant {
            pub callbacks: Callbacks,
        }
        impl DrivingAssistant {
            pub fn new(callbacks: Callbacks) -> Self {
                DrivingAssistant { callbacks }
            }
            pub fn active(&self) -> bool {
                false
            }
            /// The real `snapshot()` as JSON (`following_route` among its keys).
            pub fn snapshot_json(&self) -> Value {
                serde_json::json!({"following_route": false})
            }
            pub fn seconds_since_arrival(&self) -> Option<f64> {
                None
            }
            /// The real `run(wait)`; here a loop on the stop event.
            pub fn run(&self, stop_event: &Event) {}
        }
    }
}

pub mod beam_reports {
    pub mod drift_speech {
        use serde_json::Value;
        /// Stand-in for the real `DriftAnnouncer` (built with say/enabled/clock).
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
            pub fn handle(&self, event: &Value) {}
        }
    }
}
