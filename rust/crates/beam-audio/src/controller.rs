//! `AudioController` (audio.py:1449-3522 and 7025-7235): the state every cue reads, the
//! public API the engine calls, config, HRTF selection and waveform (re)generation. The
//! callback frame is `callback/mod.rs`; device handling is `device.rs`.
//!
//! Locking (see `rust_plans/audio-callback-split.md`):
//! - `lock: Mutex<Shared>` is Python's `self.lock`: everything API threads write and the
//!   callback snapshots.
//! - `voice: Mutex<Voice>` is what the callback mutates outside `self.lock` (phases,
//!   envelopes, overlap tails, playback cursors). The callback holds it for a whole block.
//! - Lock order is always `voice` then `lock`.
//! - `waves: Mutex<Arc<Waveforms>>`: swapped whole, cloned once per block.
//!
//! Field names are the Python attribute names. Methods keep Python's names with the leading
//! underscore dropped.

#![allow(non_snake_case)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use beam_base::gear_report;
use beam_base::hrtf_sets;
use beam_base::pyfmt::{py_mod, py_round};
use beam_base::pyval::{PyDict, PyExc, PyValue};
use parking_lot::{Condvar, Mutex};
use serde_json::{Map, Value};

use crate::callback::clock::{Clock, SystemClock};
use crate::callback::common::{
    self, PulseEntry, Tail, Wave, convolve_f32_into, db_to_amp, render_directional_pulse,
};
use crate::consts::*;
use crate::hrtf::HrtfSet;

/// A finished stereo terrain scan: Python's `(N, 2)` float32 array.
pub type ScanBuffer = Arc<Vec<[f32; 2]>>;

/// A JSON object as the R2 listeners hand it over (Python dicts from `json.loads`).
pub type JsonMap = Map<String, Value>;

/// The selected static hazard, after `update_selected_hazard`'s coercions.
#[derive(Clone, Debug, PartialEq)]
pub struct Hazard {
    pub bearing: f64,
    pub urgency: i64,
    pub gap: f64,
    pub state: i64,
    pub closing: f64,
    pub ttc: f64,
    pub stopping_margin: f64,
    pub legacy: bool,
}

/// `update_selected_hazard`'s argument: a dict whose missing keys take Python's `.get`
/// defaults. All `None` is an empty dict, which Python treats as "clear".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HazardIn {
    pub bearing: Option<f64>,
    pub urgency: Option<f64>,
    pub gap: Option<f64>,
    pub state: Option<f64>,
    pub closing: Option<f64>,
    pub ttc: Option<f64>,
    pub stopping_margin: Option<f64>,
    pub legacy: Option<bool>,
}

impl HazardIn {
    fn is_empty(&self) -> bool {
        *self == HazardIn::default()
    }
}

/// The slam-gate earcons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlamKind {
    Clear,
    Over,
    Committed,
}

impl SlamKind {
    pub fn from_str(kind: &str) -> Option<SlamKind> {
        match kind {
            "clear" => Some(SlamKind::Clear),
            "over" => Some(SlamKind::Over),
            "committed" => Some(SlamKind::Committed),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            SlamKind::Clear => "clear",
            SlamKind::Over => "over",
            SlamKind::Committed => "committed",
        }
    }
}

/// One in-flight gear tone: Python's `{"buf", "pos"}`.
#[derive(Clone, Debug)]
pub struct GearTone {
    pub buf: Wave,
    pub pos: usize,
}

/// The three slam-gate waveforms (Python's `SLAM_CUE_WAVEFORMS` dict).
#[derive(Clone, Debug, Default)]
pub struct SlamCueWaveforms {
    pub clear: Option<Wave>,
    pub over: Option<Wave>,
    pub committed: Option<Wave>,
}

impl SlamCueWaveforms {
    /// `SLAM_CUE_WAVEFORMS.get(kind)`.
    pub fn get(&self, kind: Option<SlamKind>) -> Option<&Wave> {
        match kind? {
            SlamKind::Clear => self.clear.as_ref(),
            SlamKind::Over => self.over.as_ref(),
            SlamKind::Committed => self.committed.as_ref(),
        }
    }
}

/// Every `*_WAVEFORM` attribute. `None` until generated, as in Python.
#[derive(Clone, Debug, Default)]
pub struct Waveforms {
    pub CLICK_WAVEFORM: Option<Wave>,
    pub HIGHLIGHT_CLICK_WAVEFORM: Option<Wave>,
    pub TC_TONE_WAVEFORM: Option<Wave>,
    pub CHECK_ENGINE_BUZZER_WAVEFORM: Option<Wave>,
    pub OIL_CHIME_WAVEFORM: Option<Wave>,
    pub SCANNER_BEEP_WAVEFORM: Option<Wave>,
    pub SCANNER_ALIGNED_WAVEFORM: Option<Wave>,
    pub OBSTACLE_BUZZ_WAVEFORM: Option<Wave>,
    pub DROPOFF_SWEEP_WAVEFORM: Option<Wave>,
    pub HILL_SWEEP_WAVEFORM: Option<Wave>,
    pub ROAD_BEEP_WAVEFORM: Option<Wave>,
    pub ROUTE_BEACON_WAVEFORM: Option<Wave>,
    pub ROAD_CHIME_FWD_WAVEFORM: Option<Wave>,
    pub ROAD_CHIME_BACK_WAVEFORM: Option<Wave>,
    pub ROAD_JUNCTION_WAVEFORM: Option<Wave>,
    pub ROAD_JUNCTION_ENTRY_WAVEFORM: Option<Wave>,
    pub ROAD_CORRECTION_SETTLED_WAVEFORM: Option<Wave>,
    /// Always `None`: `_generate_guidance_tone` returns None (the tone is synthesised live).
    pub GUIDANCE_WAVEFORM: Option<Wave>,
    pub LS_CLICK_WAVEFORM: Option<Wave>,
    pub LS_STOP_WAVEFORM: Option<Wave>,
    pub CAM_CLICK_WAVEFORM: Option<Wave>,
    pub CAM_HIGHLIGHT_CLICK_WAVEFORM: Option<Wave>,
    pub HYDRO_CENTER_CLICK_WAVEFORM: Option<Wave>,
    pub DOCK_LOCK_WAVEFORM: Option<Wave>,
    pub SLAM_CUE_WAVEFORMS: SlamCueWaveforms,
    pub ENTRY_CUE_WAVEFORM: Option<Wave>,
    pub DOCK_APPROACH_CUE_WAVEFORM: Option<Wave>,
    pub NODE_BEEP_WAVEFORM: Option<Wave>,
    pub NODE_BEEP_REV_WAVEFORM: Option<Wave>,
    pub CLICKSPOT_BEEP_WAVEFORM: Option<Wave>,
    pub CLICKSPOT_BEEP_REV_WAVEFORM: Option<Wave>,
    pub DESCRIBE_ERROR_WAVEFORM: Option<Wave>,
    /// `None` until generated; then five pings, one per placement-step rung.
    pub PLACEMENT_PING_WAVEFORMS: Option<Vec<Wave>>,
    /// Set as a side effect of `_generate_obstacle_buzz`; the steady voice reads it with
    /// `getattr(self, "_obstacle_harmonic_peak", 1.0)`.
    pub _obstacle_harmonic_peak: Option<f64>,
}

/// The validated gear-tone settings (`gear_report.settings_from_config`).
pub type GearToneSettings = gear_report::GearSettings;

/// `update_telemetry_state`'s dict: `None` is "key absent, keep the current value".
#[derive(Clone, Debug, Default)]
pub struct TelemetryState {
    pub shift_active: Option<bool>,
    pub pedal_tones_active: Option<bool>,
    pub tc_active: Option<bool>,
    pub last_clutch: Option<f64>,
    pub last_brake: Option<f64>,
    pub last_throttle: Option<f64>,
    pub last_steering: Option<f64>,
    pub inverted: Option<bool>,
    pub last_roll_rad: Option<f64>,
    pub last_pitch_rad: Option<f64>,
    pub guidance_active: Option<bool>,
    pub guidance_error_deg: Option<f64>,
    pub last_actual_steering: Option<f64>,
    pub last_steering_input: Option<f64>,
    pub trailer_artic: Option<f64>,
    pub trailer_reverse: Option<bool>,
    pub implement_flags: Option<i64>,
    pub implement_min_clearance: Option<f64>,
    pub implement_tilt_deg: Option<f64>,
    pub implement_lift: Option<f64>,
    pub implement_activity: Option<f64>,
    pub coord_guidance_active: Option<bool>,
    pub coord_guidance_error_deg: Option<f64>,
    pub drift_alert_active: Option<bool>,
    pub drift_pan: Option<f64>,
    pub drift_rate: Option<f64>,
    pub ls_clicks_active: Option<bool>,
    pub ls_speed_mph: Option<f64>,
    pub ls_accel: Option<f64>,
    pub ls_stopped: Option<bool>,
    pub scan_speed_ms: Option<f64>,
    pub slip_active: Option<bool>,
    pub slip_kind: Option<i64>,
    pub slip_mag: Option<f64>,
    pub slip_ratio: Option<f64>,
    pub slide_active: Option<bool>,
    pub slide_deg: Option<f64>,
    pub slip_azimuth_deg: Option<f64>,
}

/// `junction_proximity_snapshot()`'s dict.
#[derive(Clone, Debug, PartialEq)]
pub struct JunctionProximitySnapshot {
    pub enabled: bool,
    pub active: bool,
    pub age_s: f64,
    pub target: Option<JsonMap>,
    pub mode: &'static str,
    pub period_s: Option<f64>,
}

/// `update_road_guidance`'s return dict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoadGuidanceResult {
    pub settled_triggered: bool,
}

/// Python's `self.lock` state: written by API and telemetry threads, snapshotted by the
/// callback. Grouped as `__init__` groups it.
pub struct Shared {
    pub samplerate: f64,

    // HRTF state
    pub _hrtf: Option<Arc<HrtfSet>>,
    pub _hrtf_set_key: Option<(String, f64, bool)>,
    pub _click_conv_L: Option<Wave>,
    pub _click_conv_R: Option<Wave>,
    pub _hclick_conv_L: Option<Wave>,
    pub _hclick_conv_R: Option<Wave>,
    pub _click_use_hrtf: bool,
    pub _hclick_use_hrtf: bool,
    pub _ping_conv_L: Option<Wave>,
    pub _ping_conv_R: Option<Wave>,
    pub _ping_use_hrtf: bool,
    pub _hrtf_front_emphasis_db: f64,
    pub _hrtf_user_enabled: bool,
    pub _hrtf_distance_gain: f64,

    // Configurable enable flags and volumes
    pub _tc_clicks_enabled: bool,
    pub _pitch_roll_enabled: bool,
    pub _att_max_dbfs: f64,
    pub _att_min_dbfs: f64,
    pub _compass_click_amp: f64,
    pub _ls_click_amp_db: f64,
    pub _ls_stop_amp_db: f64,
    pub _slip_amp_db: f64,
    pub _obstacle_amp: f64,
    pub _ping_amp: f64,

    // One-shot requests (Python's threading.Events and their request attributes)
    pub _ping_request: bool,
    pub _ping_request_rung: i64,
    pub _ping_request_pitch_mult: f64,
    pub _ping_request_pan: f64,
    pub _click_request: bool,
    pub _click_request_pitch_mult: f64,
    pub _click_request_pan: f64,
    pub _highlight_click_request: bool,
    pub _highlight_request_pitch_mult: f64,
    pub _highlight_request_pan: f64,
    pub _cam_click_request: bool,
    pub _cam_click_request_pitch_mult: f64,
    pub _cam_click_request_pan: f64,
    pub _cam_highlight_click_request: bool,
    pub _cam_highlight_request_pitch_mult: f64,
    pub _cam_highlight_request_pan: f64,
    pub _cam_click_conv_L: Option<Wave>,
    pub _cam_click_conv_R: Option<Wave>,
    pub _cam_hclick_conv_L: Option<Wave>,
    pub _cam_hclick_conv_R: Option<Wave>,
    pub _cam_click_use_hrtf: bool,
    pub _cam_hclick_use_hrtf: bool,

    // Shared state from the main telemetry loop
    pub shift_active: bool,
    pub pedal_tones_active: bool,
    pub tc_active: bool,
    pub last_clutch: f64,
    pub last_brake: f64,
    pub last_throttle: f64,
    pub last_steering: f64,
    pub inverted: bool,
    pub last_roll_rad: f64,
    pub last_pitch_rad: f64,

    // Vehicle scanner
    pub _scan_mode_active: bool,
    pub _scanner_target_bearing: f64,
    pub _scanner_target_distance: f64,
    pub _scan_speed_ms: f64,
    pub _scan_closing_ms: f64,
    pub _scan_prev_dist: Option<f64>,
    pub _scan_prev_dist_t: f64,
    pub _scan_base_freq: f64,
    pub _scan_offset_oct: f64,
    pub _scan_steer_tone_enabled: bool,

    // Old Cannon elevation solution
    pub _cannon_aim_have: bool,
    pub _cannon_elevation: f64,
    pub _cannon_aim_bearing: f64,
    pub _cannon_target_angle: f64,
    pub _cannon_aim_solution_available: bool,
    pub _cannon_aim_reachable: bool,
    pub _cannon_aim_stamp: f64,

    // Coupler tracking
    pub _coupler_active: bool,
    pub _coupler_bearing: f64,
    pub _coupler_distance: f64,
    pub _coupler_in_range: bool,

    // Predictive obstacle warning
    pub _obstacle_mode_active: bool,
    pub _obstacle_hazard: Option<Hazard>,
    pub _obstacle_emergency_latched: bool,
    pub _obstacle_below_urgent_since: Option<f64>,
    pub _terrain_type: i64,

    // Road detection
    pub _road_mode_active: bool,
    pub _road_on_road: bool,
    pub _road_bearing: f64,
    pub _road_distance: f64,
    pub _road_beacon_available: bool,
    pub _road_correction_active: bool,
    pub _road_correction_bearing: f64,
    pub _road_correction_severity: f64,
    pub _road_correction_phase: String,
    pub _road_follow_enabled: bool,
    pub _road_amp: f64,
    pub _road_correction_amp: f64,
    pub _road_correction_settled_last_time: f64,
    pub _road_chime_queue: Vec<PulseEntry>,
    pub _road_chime_next_allowed_time: f64,
    pub _road_junction_queue: Vec<PulseEntry>,
    pub _assistant_cue_queue: Vec<PulseEntry>,
    pub _gear_tones: Vec<GearTone>,
    pub _gear_tone_settings: GearToneSettings,
    pub _junction_proximity_enabled: bool,
    pub _junction_proximity: Option<JsonMap>,
    pub _junction_proximity_time: f64,
    pub _junction_proximity_phase: f64,
    pub _stop_bar_phase: f64,
    pub _stop_bar_sub_phase: f64,
    pub _stop_bar_env: f64,
    pub _stop_bar_gain: f64,
    pub _stop_bar_stopped: bool,
    pub _stop_bar_on_id: Option<Value>,
    pub _stop_bar_reached_id: Option<Value>,
    pub _stop_bar_departed_id: Option<Value>,
    pub _stop_bar_mode: StopBarState,
    pub _road_junction_amp: f64,

    // Route beacon
    pub _route_beacon_mode_active: bool,
    pub _route_beacon_available: bool,
    pub _route_beacon_bearing: f64,
    pub _route_beacon_distance: f64,
    pub _route_beacon_amp: f64,

    // Heading guidance
    pub _guidance_active: bool,
    pub _guidance_error_deg: f64,

    // Hydraulic steering / trailer articulation inputs
    pub _hydro_actual_steer: f64,
    pub _hydro_steer_input: f64,
    pub _trailer_artic: f64,
    pub _trailer_reverse: bool,
    /// Glided inside the callback's snapshot section, under the lock.
    pub _trailer_duck_env: f64,

    // Loader implement inputs and levels
    pub _impl_flags: i64,
    pub _impl_min_clearance: f64,
    pub _impl_tilt_deg: f64,
    pub _impl_lift: f64,
    pub _impl_activity: f64,
    pub _impl_tones_enabled: bool,
    pub _impl_ground_db: f64,
    pub _impl_tilt_db: f64,

    // Docking instrument inputs
    pub _dock_active: bool,
    pub _dock_mode: String,
    pub _dock_have: bool,
    pub _dock_range: f64,
    pub _dock_lateral: f64,
    pub _dock_null: f64,
    pub _dock_acquire: bool,
    pub _dock_bearing: f64,
    pub _dock_stamp: f64,
    pub _dock_tones_enabled: bool,
    pub _dock_pulse_db: f64,

    // Terrain sonification scanner config
    pub _scan_tones_enabled: bool,
    pub _scan_level_db: f64,

    // Coordinate guidance
    pub _coord_guidance_active: bool,
    pub _coord_guidance_error: f64,

    // Free-camera line-up guide
    pub _lineup_active: bool,
    pub _lineup_stamp: f64,
    pub _lineup_target: (f64, f64, f64, f64),

    // Drift detection
    pub _drift_alert_active: bool,
    pub _drift_pan: f64,
    pub _drift_rate: f64,

    // Low speed detection
    pub _ls_clicks_active: bool,
    pub _ls_speed_mph: f64,
    pub _ls_accel: f64,
    pub _ls_stopped: bool,

    // Wheel slip
    pub _slip_active: bool,
    pub _slip_kind: i64,
    pub _slip_mag: f64,
    pub _slip_ratio: f64,
    pub _slide_active: bool,
    pub _slide_deg: f64,
    pub _slip_azimuth_deg: f64,

    // Audio stream and device management
    pub _audio_stream: Option<cpal::Stream>,
    pub _device_watcher_thread: Option<JoinHandle<()>>,
    pub _current_device_index: Option<usize>,
    pub _current_device_name: Option<String>,
    pub _preferred_device_name: String,
    pub _follow_default_enabled: bool,
    pub _audio_poll_interval: f64,

    // Configurable params
    pub shift_freq: f64,
    pub shift_amp: f64,
    pub _phase_inc_shift: f64,
    pub buzzer_amp: f64,
    pub chime_amp: f64,
}

/// What `_audio_callback` writes outside `self.lock`. Owned by the callback for a whole
/// block; API methods that write one of these lock `voice` first.
#[derive(Default)]
pub struct Voice {
    // Playback state
    pub _click_playback_pos: f64,
    pub _click_pan_pos: f64,
    pub _click_pitch_mult: f64,
    pub _highlight_click_playback_pos: f64,
    pub _highlight_click_pan_pos: f64,
    pub _highlight_click_pitch_mult: f64,
    pub _tc_playback_pos: f64,
    pub _buzzer_playback_pos: f64,
    pub _chime_playback_pos: f64,
    pub _describe_error_playback_pos: f64,
    pub _ping_playback_pos: f64,
    pub _ping_pan_pos: f64,
    pub _ping_pitch_mult: f64,
    pub _ping_rung: i64,
    pub _scanner_playback_pos: f64,
    pub _node_beep_playback_pos: f64,
    pub _node_beep_freq: f64,
    pub _node_beep_reverse: bool,
    pub _clickspot_beep_playback_pos: f64,
    pub _clickspot_beep_reverse: bool,
    pub _cam_click_playback_pos: f64,
    pub _cam_click_pan_pos: f64,
    pub _cam_click_pitch_mult: f64,
    pub _cam_highlight_click_playback_pos: f64,
    pub _cam_highlight_click_pan_pos: f64,
    pub _cam_highlight_click_pitch_mult: f64,

    // Attitude HRTF tails
    pub _roll_hrtf_overlap_L: Tail,
    pub _roll_hrtf_overlap_R: Tail,
    pub _pitch_hrtf_overlap_L: Tail,
    pub _pitch_hrtf_overlap_R: Tail,

    // Vehicle scanner
    pub _scanner_beep_timer: f64,
    pub _scanner_overlap_L: Tail,
    pub _scanner_overlap_R: Tail,
    pub _sm_scan_bearing: Option<f64>,
    pub _scan_tone_level: f64,
    pub _scan_tone_phase: f64,
    pub _scan_tone_overlap_L: Tail,
    pub _scan_tone_overlap_R: Tail,

    // Old Cannon
    pub _cannon_aim_pulse_phase: f64,
    pub _cannon_aim_carrier_phase: f64,

    // Coupler
    pub _coupler_phase: f64,
    pub _coupler_beep_timer: f64,
    pub _coupler_overlap_L: Tail,
    pub _coupler_overlap_R: Tail,

    // Obstacle
    pub _obstacle_cadence_clock: f64,
    pub _obstacle_pulse_queue: Vec<PulseEntry>,
    pub _obstacle_doublet_requested: bool,
    pub _obstacle_render_bearing: f64,
    pub _obstacle_steady_phase: f64,
    pub _obstacle_steady_env: f64,
    pub _obstacle_steady_overlap_L: Tail,
    pub _obstacle_steady_overlap_R: Tail,
    pub _terrain_playback_pos: f64,

    // Road
    pub _road_beep_timer: f64,
    pub _road_playback_pos: f64,
    pub _road_pulse_L: Option<Wave>,
    pub _road_pulse_R: Option<Wave>,
    pub _road_correction_carrier_phase: f64,
    pub _road_correction_pulse_phase: f64,
    pub _road_correction_env: f64,
    pub _road_correction_render_bearing: f64,
    pub _road_correction_overlap_L: Tail,
    pub _road_correction_overlap_R: Tail,
    pub _road_correction_settled_pos: i64,

    // Route beacon
    pub _route_beacon_timer: f64,
    pub _route_beacon_playback_pos: f64,
    pub _route_beacon_pulse_L: Option<Wave>,
    pub _route_beacon_pulse_R: Option<Wave>,

    // Heading guidance
    pub _phase_guidance: f64,
    pub _sm_guidance_error: f64,

    // Hydraulic steering
    pub _hydro_steer_phase: f64,
    pub _hydro_steer_overlap_L: Tail,
    pub _hydro_steer_overlap_R: Tail,
    pub _hydro_steer_inc: f64,
    pub _hydro_steer_mod_phase: f64,
    pub _hydro_steer_ratio: f64,
    pub _hydro_steer_index: f64,
    pub _hydro_steer_env: f64,
    pub _hydro_steer_sm_mag: f64,
    pub _hydro_center_armed: bool,
    pub _hydro_center_pos: f64,
    pub _hydro_was_from_trailer: bool,

    // Loader implement
    pub _impl_gate_env: f64,
    pub _impl_gate_open: bool,
    pub _impl_prev_clearance: Option<f64>,
    pub _impl_clear_elapsed: f64,
    pub _impl_closing: f64,
    pub _impl_ground_phase: f64,
    pub _impl_ground_mod_phase: f64,
    pub _impl_ground_inc: f64,
    pub _impl_ground_ratio: f64,
    pub _impl_ground_index: f64,
    pub _impl_tilt_phase: f64,
    pub _impl_tilt_inc: f64,
    pub _impl_tilt_step: Option<i64>,
    pub _impl_tilt_held_step: Option<i64>,
    pub _impl_tilt_dip_left: f64,
    pub _impl_tilt_seen_up: f64,
    pub _impl_tilt_seen_dn: f64,
    pub _impl_tilt_stop_up: f64,
    pub _impl_tilt_stop_dn: f64,
    pub _impl_tilt_span_up: f64,
    pub _impl_tilt_span_dn: f64,

    // Docking instrument
    pub _dock_gate_env: f64,
    pub _dock_pulse_phase: f64,
    pub _dock_pulse_cycle: f64,
    pub _dock_pulse_rate: f64,
    pub _dock_overlap_L: Tail,
    pub _dock_overlap_R: Tail,
    pub _dock_ref_phase: f64,
    pub _dock_mov_phase: f64,
    pub _dock_ref_inc: f64,
    pub _dock_mov_inc: f64,
    pub _dock_am_phase: f64,
    pub _dock_beat_env: f64,
    pub _dock_lock_armed: bool,
    pub _dock_lock_pos: f64,
    pub _dock_approach_cue_pos: f64,
    pub _slam_cue_kind: Option<SlamKind>,
    pub _slam_cue_pos: f64,
    pub _entry_cue_pos: f64,

    // Terrain sonification playback
    pub _scan_buf: Option<ScanBuffer>,
    pub _scan_pos: usize,
    pub _scan_old_buf: Option<ScanBuffer>,
    pub _scan_old_pos: usize,
    pub _scan_old_left: i64,

    // Coordinate guidance
    pub _sm_coord_error: f64,
    pub _coord_phase_carrier: f64,
    pub _coord_phase_mod: f64,
    pub _coord_hrtf_overlap_L: Tail,
    pub _coord_hrtf_overlap_R: Tail,

    // Line-up guide
    pub _sm_lineup_amp: f64,
    pub _sm_lineup_index: f64,
    pub _sm_lineup_ratio: f64,
    pub _sm_lineup_az_xy: (f64, f64),
    pub _lineup_phase_carrier: f64,
    pub _lineup_phase_mod: f64,
    pub _lineup_ir: Option<(Vec<f64>, Vec<f64>)>,
    pub _lineup_overlap_L: Tail,
    pub _lineup_overlap_R: Tail,

    // Drift
    pub _phase_drift: f64,
    pub _sm_drift_rate: f64,
    pub _sm_drift_pan: f64,
    pub _drift_gain: f64,
    pub _drift_inc: f64,

    // Low speed
    pub _ls_click_timer: f64,
    pub _ls_playback_pos: f64,
    pub _ls_fade_gain: f64,
    pub _ls_conv_L: Option<Wave>,
    pub _ls_conv_R: Option<Wave>,
    pub _ls_use_hrtf: bool,
    pub _ls_pitch_mult: f64,
    pub _ls_stop_playback_pos: f64,

    // Wheel slip
    pub _slip_ratio_sm: f64,
    pub _slip_pitch_kind: i64,
    pub _slip_fade_gain: f64,
    pub _phase_slip: f64,
    pub _phase_slip_am: f64,
    pub _slide_fade_gain: f64,
    pub _phase_slide_am: f64,
    pub _slide_noise_pos: usize,
    pub _slip_az_x: f64,
    pub _slip_az_y: f64,
    pub _slip_overlap_L: Tail,
    pub _slip_overlap_R: Tail,
    pub _slip_tables_sr: Option<i64>,
    /// `(spin_table, lock_table, noise_lo, noise_hi)`.
    pub _slip_tables: Option<Arc<(Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>)>>,

    // Synthesis state (phases, smoothers)
    pub _phase_shift: f64,
    pub _sm_roll_int: f64,
    pub _sm_pitch_int: f64,
    pub _phase_roll: f64,
    pub _phase_pitch: f64,
    pub _roll_fade_mult: f64,
    pub _pitch_fade_mult: f64,
    pub _sm_c: f64,
    pub _sm_b: f64,
    pub _sm_t: f64,
    pub _detune_c: f64,
    pub _detune_b: f64,
    pub _detune_t: f64,
    pub _phase_c: f64,
    pub _phase_b: f64,
    pub _phase_t: f64,
    pub _phase_steer: f64,

    // Frame scratch (not Python state): preallocated buffers and the abort latch.
    pub buf_l: Vec<f32>,
    pub buf_r: Vec<f32>,
    pub mix_l: Vec<f32>,
    pub mix_r: Vec<f32>,
    /// Set when a block panicked: Python's paAbort. Every later block is silence.
    pub callback_aborted: bool,
}

/// `threading.Event` for `_stop_event`.
#[derive(Default)]
pub struct StopEvent {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl StopEvent {
    pub fn set(&self) {
        *self.flag.lock() = true;
        self.cv.notify_all();
    }
    pub fn clear(&self) {
        *self.flag.lock() = false;
    }
    pub fn is_set(&self) -> bool {
        *self.flag.lock()
    }
    /// `Event.wait(timeout)`: returns the flag.
    pub fn wait(&self, timeout_s: f64) -> bool {
        let mut g = self.flag.lock();
        if !*g && timeout_s > 0.0 && timeout_s.is_finite() {
            let _ = self
                .cv
                .wait_for(&mut g, std::time::Duration::from_secs_f64(timeout_s));
        }
        *g
    }
}

pub struct AudioController {
    /// `_is_enabled`. Rust always has its audio stack, so this starts true; `start()`
    /// clears it when no device is found, as Python does.
    pub _is_enabled: AtomicBool,
    /// Python's `self.lock`.
    pub lock: Mutex<Shared>,
    /// Callback-owned synthesis state.
    pub voice: Mutex<Voice>,
    /// Every `*_WAVEFORM` attribute, swapped whole.
    pub waves: Mutex<Arc<Waveforms>>,
    pub _stop_event: StopEvent,
    /// `time.monotonic` / `time.perf_counter`, injectable for tests.
    pub clock: Arc<dyn Clock>,
}

fn new_shared() -> Shared {
    let sr = DEFAULT_SR as f64;
    let shift_freq = 880.0;
    Shared {
        samplerate: sr,
        _hrtf: None,
        _hrtf_set_key: None,
        _click_conv_L: None,
        _click_conv_R: None,
        _hclick_conv_L: None,
        _hclick_conv_R: None,
        _click_use_hrtf: false,
        _hclick_use_hrtf: false,
        _ping_conv_L: None,
        _ping_conv_R: None,
        _ping_use_hrtf: false,
        _hrtf_front_emphasis_db: -6.0,
        _hrtf_user_enabled: true,
        _hrtf_distance_gain: 1.0,
        _tc_clicks_enabled: true,
        _pitch_roll_enabled: true,
        _att_max_dbfs: ATT_MAX_DBFS,
        _att_min_dbfs: ATT_MIN_DBFS,
        _compass_click_amp: 0.5,
        _ls_click_amp_db: LS_AMP_DB,
        _ls_stop_amp_db: LS_STOP_AMP_DB,
        _slip_amp_db: SLIP_AMP_DB,
        _obstacle_amp: db_to_amp(OBSTACLE_DEFAULT_DB),
        _ping_amp: db_to_amp(PLACEMENT_PING_AMP_DB),
        _ping_request: false,
        _ping_request_rung: 0,
        _ping_request_pitch_mult: 1.0,
        _ping_request_pan: 0.0,
        // The four click request attributes are first created by their trigger in
        // Python; the intake never reads them before that.
        _click_request: false,
        _click_request_pitch_mult: 1.0,
        _click_request_pan: 0.0,
        _highlight_click_request: false,
        _highlight_request_pitch_mult: 1.0,
        _highlight_request_pan: 0.0,
        _cam_click_request: false,
        _cam_click_request_pitch_mult: 1.0,
        _cam_click_request_pan: 0.0,
        _cam_highlight_click_request: false,
        _cam_highlight_request_pitch_mult: 1.0,
        _cam_highlight_request_pan: 0.0,
        _cam_click_conv_L: None,
        _cam_click_conv_R: None,
        _cam_hclick_conv_L: None,
        _cam_hclick_conv_R: None,
        _cam_click_use_hrtf: false,
        _cam_hclick_use_hrtf: false,
        shift_active: false,
        pedal_tones_active: false,
        tc_active: false,
        last_clutch: 0.0,
        last_brake: 0.0,
        last_throttle: 0.0,
        last_steering: 0.0,
        inverted: false,
        last_roll_rad: 0.0,
        last_pitch_rad: 0.0,
        _scan_mode_active: false,
        _scanner_target_bearing: 0.0,
        _scanner_target_distance: f64::INFINITY,
        _scan_speed_ms: 0.0,
        _scan_closing_ms: 0.0,
        _scan_prev_dist: None,
        _scan_prev_dist_t: 0.0,
        _scan_base_freq: SCANNER_BASE_FREQ_DEFAULT,
        _scan_offset_oct: SCANNER_OFFSET_OCT_DEFAULT,
        _scan_steer_tone_enabled: true,
        _cannon_aim_have: false,
        _cannon_elevation: 0.0,
        _cannon_aim_bearing: 0.0,
        _cannon_target_angle: 0.0,
        _cannon_aim_solution_available: false,
        _cannon_aim_reachable: false,
        _cannon_aim_stamp: 0.0,
        _coupler_active: false,
        _coupler_bearing: 0.0,
        _coupler_distance: f64::INFINITY,
        _coupler_in_range: false,
        _obstacle_mode_active: false,
        _obstacle_hazard: None,
        _obstacle_emergency_latched: false,
        _obstacle_below_urgent_since: None,
        _terrain_type: 0,
        _road_mode_active: false,
        _road_on_road: true,
        _road_bearing: 0.0,
        _road_distance: 0.0,
        _road_beacon_available: true,
        _road_correction_active: false,
        _road_correction_bearing: 0.0,
        _road_correction_severity: 0.0,
        _road_correction_phase: "idle".to_string(),
        _road_follow_enabled: true,
        _road_amp: db_to_amp(-14.0),
        _road_correction_amp: db_to_amp(-24.0),
        _road_correction_settled_last_time: f64::NEG_INFINITY,
        _road_chime_queue: Vec::new(),
        _road_chime_next_allowed_time: 0.0,
        _road_junction_queue: Vec::new(),
        _assistant_cue_queue: Vec::new(),
        _gear_tones: Vec::new(),
        _gear_tone_settings: default_gear_settings(),
        _junction_proximity_enabled: true,
        _junction_proximity: None,
        _junction_proximity_time: 0.0,
        _junction_proximity_phase: 0.0,
        _stop_bar_phase: 0.0,
        _stop_bar_sub_phase: 0.0,
        _stop_bar_env: 0.0,
        _stop_bar_gain: 1.0,
        _stop_bar_stopped: false,
        _stop_bar_on_id: None,
        _stop_bar_reached_id: None,
        _stop_bar_departed_id: None,
        _stop_bar_mode: StopBarState::Silent,
        _road_junction_amp: db_to_amp(ROAD_JUNCTION_AMP_DB),
        _route_beacon_mode_active: false,
        _route_beacon_available: false,
        _route_beacon_bearing: 0.0,
        _route_beacon_distance: 0.0,
        _route_beacon_amp: db_to_amp(ROUTE_BEACON_AMP_DB),
        _guidance_active: false,
        _guidance_error_deg: 0.0,
        _hydro_actual_steer: 0.0,
        _hydro_steer_input: 0.0,
        _trailer_artic: 0.0,
        _trailer_reverse: false,
        _trailer_duck_env: 0.0,
        _impl_flags: 0,
        _impl_min_clearance: -1.0,
        _impl_tilt_deg: 0.0,
        _impl_lift: 0.0,
        _impl_activity: 0.0,
        _impl_tones_enabled: true,
        _impl_ground_db: IMPL_GROUND_MAX_DB,
        _impl_tilt_db: IMPL_TILT_MAX_DB,
        _dock_active: false,
        _dock_mode: "IMPL".to_string(),
        _dock_have: false,
        _dock_range: f64::INFINITY,
        _dock_lateral: 0.0,
        _dock_null: 0.0,
        _dock_acquire: false,
        _dock_bearing: 0.0,
        _dock_stamp: 0.0,
        _dock_tones_enabled: true,
        _dock_pulse_db: DOCK_PULSE_DB,
        _scan_tones_enabled: true,
        _scan_level_db: SCAN_TERRAIN_DB,
        _coord_guidance_active: false,
        _coord_guidance_error: 0.0,
        _lineup_active: false,
        _lineup_stamp: 0.0,
        _lineup_target: (0.0, 1.0, 1.0, 0.0),
        _drift_alert_active: false,
        _drift_pan: 0.0,
        _drift_rate: 0.0,
        _ls_clicks_active: false,
        _ls_speed_mph: 0.0,
        _ls_accel: 0.0,
        _ls_stopped: false,
        _slip_active: false,
        _slip_kind: 0,
        _slip_mag: 0.0,
        _slip_ratio: 0.0,
        _slide_active: false,
        _slide_deg: 0.0,
        _slip_azimuth_deg: 0.0,
        _audio_stream: None,
        _device_watcher_thread: None,
        _current_device_index: None,
        _current_device_name: None,
        _preferred_device_name: String::new(),
        _follow_default_enabled: true,
        _audio_poll_interval: 2.0,
        shift_freq,
        shift_amp: db_to_amp(-12.0),
        _phase_inc_shift: shift_freq / sr,
        // Python writes `1.0 ** (-12.0 / 20.0)`, which is 1.0, not -12 dB. Only the
        // initial value; apply_config replaces both before anything plays.
        buzzer_amp: 1.0f64.powf(-12.0 / 20.0),
        chime_amp: 1.0f64.powf(-12.0 / 20.0),
    }
}

/// `gear_report.settings_from_config({})`.
fn default_gear_settings() -> GearToneSettings {
    match gear_report::settings_from_config(&PyDict::new()) {
        Ok(s) => s,
        Err(_) => GearToneSettings {
            mode: gear_report::DEFAULT_MODE.to_string(),
            collapse_sport: false,
            number_style: gear_report::DEFAULT_NUMBER_STYLE.to_string(),
            root: gear_report::DEFAULT_ROOT.to_string(),
            octave: gear_report::DEFAULT_OCTAVE,
            scale: gear_report::DEFAULT_SCALE.to_string(),
            level_db: gear_report::DEFAULT_LEVEL_DB,
        },
    }
}

/// A seed for the unseeded `np.random.default_rng()` the pedal detune uses.
fn entropy_seed() -> u128 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    nanos ^ ((std::process::id() as u128) << 64)
}

fn new_voice() -> Voice {
    // `np.random.default_rng().uniform(-3.0, 3.0, 3)`: unseeded, so a fresh draw per run.
    let mut rng = crate::dsp::default_rng(entropy_seed());
    let detune = rng.uniform_n(-3.0, 3.0, 3);
    Voice {
        _click_playback_pos: -1.0,
        _click_pitch_mult: 1.0,
        _highlight_click_playback_pos: -1.0,
        _highlight_click_pitch_mult: 1.0,
        _tc_playback_pos: -1.0,
        _buzzer_playback_pos: -1.0,
        _chime_playback_pos: -1.0,
        _describe_error_playback_pos: -1.0,
        _ping_playback_pos: -1.0,
        _ping_pitch_mult: 1.0,
        _scanner_playback_pos: -1.0,
        _node_beep_playback_pos: -1.0,
        _node_beep_freq: NODE_BEEP_BASE_FREQ_HZ,
        _clickspot_beep_playback_pos: -1.0,
        _cam_click_playback_pos: -1.0,
        _cam_click_pitch_mult: 1.0,
        _cam_highlight_click_playback_pos: -1.0,
        _cam_highlight_click_pitch_mult: 1.0,
        _terrain_playback_pos: -1.0,
        _road_playback_pos: -1.0,
        _road_correction_settled_pos: -1,
        _route_beacon_playback_pos: -1.0,
        _hydro_center_pos: -1.0,
        _dock_lock_pos: -1.0,
        _dock_approach_cue_pos: -1.0,
        _slam_cue_pos: -1.0,
        _entry_cue_pos: -1.0,
        _sm_lineup_index: 1.0,
        _sm_lineup_ratio: 1.0,
        _sm_lineup_az_xy: (1.0, 0.0),
        _ls_playback_pos: -1.0,
        _ls_pitch_mult: 1.0,
        _ls_stop_playback_pos: -1.0,
        _slip_az_x: 1.0,
        _slip_az_y: 0.0,
        _roll_fade_mult: 1.0,
        _pitch_fade_mult: 1.0,
        _detune_c: detune.first().copied().unwrap_or(0.0),
        _detune_b: detune.get(1).copied().unwrap_or(0.0),
        _detune_t: detune.get(2).copied().unwrap_or(0.0),
        ..Voice::default()
    }
}

/// Python `max(a, b)` / `min(a, b)` argument order matters for NaN; see `consts::py_max`.
#[inline]
fn clamp_db(x: f64) -> f64 {
    py_min(0.0, py_max(-120.0, x))
}

/// `float(cfg.get(key, default))`.
fn cfg_float(cfg: &PyDict, key: &str, default: f64) -> Result<f64, PyExc> {
    match cfg.get(key) {
        Some(v) => v.py_float(),
        None => Ok(default),
    }
}

/// `bool(cfg.get(key, default))`.
fn cfg_bool(cfg: &PyDict, key: &str, default: bool) -> bool {
    match cfg.get(key) {
        Some(v) => v.truthy(),
        None => default,
    }
}

/// `float(x)` of a JSON value from an R2 packet. Non-numeric values raise in Python; the
/// listeners have validated these dicts, so a value that is not a number reads as NaN.
pub fn json_float(v: Option<&Value>, default: f64) -> f64 {
    match v {
        None => default,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::Bool(b)) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Some(Value::String(s)) => beam_base::pyfmt::py_float_parse(s).unwrap_or(f64::NAN),
        Some(_) => f64::NAN,
    }
}

/// Python truthiness of a JSON value.
pub fn json_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|x| x != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `str(x)` of a JSON value (used for the correction phase).
fn json_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => beam_base::pyval::from_json(other).py_str(),
    }
}

/// HRTF-convolve a one-shot waveform at `hrtf_az` and scale it:
/// `(np.convolve(wave, ir, mode="full") * gain).astype(np.float32)`.
fn convolve_scaled(wave: &[f32], ir: &[f64], gain: f64) -> Wave {
    let mut scratch = Vec::new();
    let mut conv = Vec::new();
    convolve_f32_into(wave, ir, &mut scratch, &mut conv);
    conv.iter().map(|&x| (x * gain) as f32).collect()
}

/// Which of the four compass/cam click slots a trigger fills.
#[derive(Clone, Copy)]
enum ClickSlot {
    Click,
    Highlight,
    CamClick,
    CamHighlight,
}

impl AudioController {
    /// `AudioController(logger)` with the real clocks.
    pub fn new() -> Arc<Self> {
        Self::with_clock(Arc::new(SystemClock::new()))
    }

    /// The same, with an injected clock (the sims' patched `time.monotonic`).
    pub fn with_clock(clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(AudioController {
            _is_enabled: AtomicBool::new(true),
            lock: Mutex::new(new_shared()),
            voice: Mutex::new(new_voice()),
            waves: Mutex::new(Arc::new(Waveforms::default())),
            _stop_event: StopEvent::default(),
            clock,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self._is_enabled.load(Ordering::SeqCst)
    }

    /// `self.samplerate`.
    pub fn samplerate(&self) -> f64 {
        self.lock.lock().samplerate
    }

    /// The current waveforms (one `Arc` clone).
    pub fn waveforms(&self) -> Arc<Waveforms> {
        self.waves.lock().clone()
    }

    fn update_waves(&self, f: impl FnOnce(&mut Waveforms)) {
        let mut guard = self.waves.lock();
        let mut w = (**guard).clone();
        f(&mut w);
        *guard = Arc::new(w);
    }

    // ------------------------------------------------------------------------------
    // Vehicle scanner
    // ------------------------------------------------------------------------------

    pub fn set_scan_mode(&self, is_active: bool) {
        let mut s = self.lock.lock();
        s._scan_mode_active = is_active;
        if !s._scan_mode_active {
            // Reset distance when turned off so it doesn't beep once on reactivation.
            s._scanner_target_distance = f64::INFINITY;
        }
    }

    pub fn update_scanner_target(&self, bearing: f64, distance: f64) {
        let mut s = self.lock.lock();
        s._scanner_target_bearing = bearing;
        // The mod reports a surface GAP, which legitimately reaches zero when the vehicles
        // touch; floored here so callers never announce "minus one feet".
        let mut distance = distance;
        if distance.is_finite() && distance < 0.0 {
            distance = 0.0;
        }
        s._scanner_target_distance = distance;

        // Closing speed (positive = approaching) from the change in distance.
        let now = self.clock.perf_counter();
        if let Some(prev) = s._scan_prev_dist {
            if prev.is_finite() && distance.is_finite() {
                let dt = now - s._scan_prev_dist_t;
                if 1e-3 < dt && dt < 1.0 {
                    let raw = (prev - distance) / dt;
                    let raw = py_max(-SCANNER_CLOSING_MAX_MS, py_min(SCANNER_CLOSING_MAX_MS, raw));
                    s._scan_closing_ms += SCANNER_CLOSING_SMOOTH * (raw - s._scan_closing_ms);
                }
            }
        }
        if !distance.is_finite() {
            s._scan_closing_ms = 0.0;
        }
        s._scan_prev_dist = if distance.is_finite() { Some(distance) } else { None };
        s._scan_prev_dist_t = now;
    }

    /// Old Cannon elevation guidance for the scanner's current target.
    pub fn update_cannon_aim(
        &self,
        elevation_deg: f64,
        bearing_deg: f64,
        target_angle_deg: f64,
        solution_available: bool,
        reachable: bool,
    ) {
        let now = self.clock.perf_counter();
        let mut s = self.lock.lock();
        s._cannon_aim_have = true;
        s._cannon_elevation = elevation_deg;
        s._cannon_aim_bearing = bearing_deg;
        s._cannon_target_angle = target_angle_deg;
        s._cannon_aim_solution_available = solution_available;
        s._cannon_aim_reachable = reachable;
        s._cannon_aim_stamp = now;
    }

    pub fn clear_cannon_aim(&self) {
        self.lock.lock()._cannon_aim_have = false;
    }

    // ------------------------------------------------------------------------------
    // Docking instrument
    // ------------------------------------------------------------------------------

    pub fn set_dock_mode(&self, active: bool) {
        let mut s = self.lock.lock();
        s._dock_active = active;
        if !s._dock_active {
            s._dock_have = false;
            s._dock_range = f64::INFINITY;
        }
    }

    /// The three continuous channels plus which quantity the third one nulls. The mode
    /// travels WITH the values (not on `set_dock_mode`) because the two arrive on different
    /// threads; a mode change one packet ahead of its own scaling fires a spurious lock
    /// chime. `acquire` (ramp mode only) means the channels answer "where is the mouth";
    /// `bearing_deg` is the direction to it, positive-LEFT.
    pub fn update_dock_target(
        &self,
        range_m: f64,
        lateral_m: f64,
        null_value: f64,
        mode: &str,
        acquire: bool,
        bearing_deg: f64,
    ) {
        let mode = if mode.to_uppercase() == "RAMP" { "RAMP" } else { "IMPL" };
        let acquire = acquire && mode == "RAMP";
        let now = self.clock.perf_counter();
        let mut v = self.voice.lock();
        let mut s = self.lock.lock();
        if mode != s._dock_mode {
            s._dock_mode = mode.to_string();
            // The arm latch is hysteresis on a quantity whose unit and scale just changed,
            // and the pair's fade-in range is per-mode too.
            v._dock_lock_armed = false;
            v._dock_beat_env = 0.0;
        }
        s._dock_have = true;
        s._dock_acquire = acquire;
        if acquire {
            // Hunting: the range is the straight-line distance to the mouth (hypot of the
            // signed along-range and the lateral), the pan the bearing to it.
            s._dock_range = range_m.hypot(lateral_m);
            s._dock_bearing = bearing_deg;
        } else {
            s._dock_range = py_max(0.0, range_m);
            s._dock_bearing = 0.0;
        }
        s._dock_lateral = lateral_m;
        s._dock_null = null_value;
        s._dock_stamp = now;
    }

    pub fn clear_dock_target(&self) {
        let mut s = self.lock.lock();
        s._dock_have = false;
        s._dock_range = f64::INFINITY;
        s._dock_acquire = false;
    }

    /// Fire a slam-gate earcon ("clear", "over", "committed"); anything else is ignored.
    pub fn trigger_slam_cue(&self, kind: &str) {
        let Some(kind) = SlamKind::from_str(kind) else {
            return;
        };
        let mut v = self.voice.lock();
        let _s = self.lock.lock();
        v._slam_cue_kind = Some(kind);
        v._slam_cue_pos = 0.0;
    }

    /// The docking entry-gate earcon: the selected band just became enterable.
    pub fn trigger_entry_cue(&self) {
        let mut v = self.voice.lock();
        let _s = self.lock.lock();
        v._entry_cue_pos = 0.0;
    }

    /// The ramp handoff from mouth acquisition to approach-line guidance.
    pub fn trigger_dock_approach_cue(&self) {
        let mut v = self.voice.lock();
        let _s = self.lock.lock();
        v._dock_approach_cue_pos = 0.0;
    }

    // ------------------------------------------------------------------------------
    // Terrain sonification scan
    // ------------------------------------------------------------------------------

    /// Hand over a finished terrain scan. A scan landing while one plays crossfades over it.
    pub fn play_scan(&self, buf: Option<ScanBuffer>) {
        let Some(buf) = buf else {
            return;
        };
        let mut v = self.voice.lock();
        let s = self.lock.lock();
        if !s._scan_tones_enabled {
            return;
        }
        if let Some(cur) = v._scan_buf.take() {
            if v._scan_pos < cur.len() {
                v._scan_old_pos = v._scan_pos;
                v._scan_old_buf = Some(cur);
                v._scan_old_left = (s.samplerate * SCAN_CROSSFADE_MS / 1000.0).trunc() as i64;
            }
        }
        v._scan_buf = Some(buf);
        v._scan_pos = 0;
    }

    /// Render a scan on the CALLING thread (the UDP listener) and queue it.
    pub fn render_and_play_scan(
        &self,
        samples: &[crate::scan::ScanSample],
        objects: &[crate::scan::ScanObject],
        reach_m: f64,
        pois: &[crate::scan::ScanPoi],
    ) {
        if !self.is_enabled() {
            return;
        }
        let (sr, level) = {
            let s = self.lock.lock();
            if !s._scan_tones_enabled {
                return;
            }
            (s.samplerate, s._scan_level_db)
        };
        let started = std::time::Instant::now();
        let buf = crate::scan::render_scan(samples, objects, reach_m, sr, level, pois);
        log::debug!(
            "Terrain scan rendered in {} ms ({} samples, {} objects, {} POIs).",
            beam_base::pyfmt::py_f(started.elapsed().as_secs_f64() * 1000.0, 0),
            samples.len(),
            objects.len(),
            pois.len()
        );
        self.play_scan(Some(Arc::new(buf)));
    }

    pub fn stop_scan(&self) {
        let mut v = self.voice.lock();
        let _s = self.lock.lock();
        Self::stop_scan_locked(&mut v);
    }

    fn stop_scan_locked(v: &mut Voice) {
        v._scan_buf = None;
        v._scan_pos = 0;
        v._scan_old_buf = None;
        v._scan_old_left = 0;
    }

    // ------------------------------------------------------------------------------
    // Coupler tracking
    // ------------------------------------------------------------------------------

    pub fn set_coupler_tracking(&self, active: bool) {
        let mut v = self.voice.lock();
        let mut s = self.lock.lock();
        s._coupler_active = active;
        if !active {
            s._coupler_distance = f64::INFINITY;
            s._coupler_in_range = false;
            v._coupler_phase = 0.0;
            v._coupler_beep_timer = 0.0;
            v._coupler_overlap_L = None;
            v._coupler_overlap_R = None;
        }
    }

    pub fn update_coupler_target(&self, bearing: f64, distance: f64, in_range: bool) {
        let mut s = self.lock.lock();
        s._coupler_bearing = bearing;
        s._coupler_distance = distance;
        s._coupler_in_range = in_range;
    }

    // ------------------------------------------------------------------------------
    // Obstacles
    // ------------------------------------------------------------------------------

    pub fn set_obstacle_mode(&self, is_active: bool) {
        let mut v = self.voice.lock();
        let mut s = self.lock.lock();
        s._obstacle_mode_active = is_active;
        if !s._obstacle_mode_active {
            Self::clear_static_hazard_locked(&mut v, &mut s, true);
            v._terrain_playback_pos = -1.0;
            s._terrain_type = 0;
        }
    }

    /// Terrain-only update path (types 2 = drop-off, 3 = hill).
    pub fn update_obstacle(&self, obstacle_type: i64, _bearing: f64, _urgency: f64, _distance: f64) {
        let mut v = self.voice.lock();
        let mut s = self.lock.lock();
        if obstacle_type == 2 || obstacle_type == 3 {
            s._terrain_type = obstacle_type;
            v._terrain_playback_pos = 0.0;
        }
    }

    /// Select one entry from an old multi-obstacle packet `(bearing, urgency, distance)`.
    /// Legacy packets are never promoted to emergency.
    pub fn update_static_obstacles(&self, obstacles: &[(f64, f64, f64)]) {
        if obstacles.is_empty() {
            self.update_selected_hazard(None);
            return;
        }
        // max(..., key=urgency): the first of equal maxima wins.
        let mut best = obstacles[0];
        for o in &obstacles[1..] {
            if o.1 > best.1 {
                best = *o;
            }
        }
        let (bearing, urgency, distance) = best;
        let urgency_int = urgency.trunc();
        self.update_selected_hazard(Some(&HazardIn {
            bearing: Some(bearing),
            urgency: Some(urgency),
            gap: Some(distance),
            state: Some(if urgency_int >= 170.0 { 2.0 } else { 1.0 }),
            closing: Some(0.0),
            ttc: Some(-1.0),
            stopping_margin: Some(f64::INFINITY),
            legacy: Some(true),
        }));
    }

    fn clear_static_hazard_locked(v: &mut Voice, s: &mut Shared, rearm: bool) {
        s._obstacle_hazard = None;
        v._obstacle_cadence_clock = 0.0;
        v._obstacle_pulse_queue.clear();
        v._obstacle_doublet_requested = false;
        s._obstacle_below_urgent_since = None;
        if rearm {
            s._obstacle_emergency_latched = false;
        }
    }

    /// Install the one hazard selected by Lua, or clear it with `None` (or an empty dict).
    pub fn update_selected_hazard(&self, hazard: Option<&HazardIn>) {
        let mut v = self.voice.lock();
        let mut s = self.lock.lock();
        let hazard = match hazard {
            Some(h) if !h.is_empty() => h,
            _ => {
                Self::clear_static_hazard_locked(&mut v, &mut s, true);
                return;
            }
        };
        // int() truncates; a NaN or infinite value raises in Python and saturates here.
        let state = (hazard.state.unwrap_or(1.0).trunc() as i64).clamp(1, 3);
        let now = self.clock.monotonic();
        if state >= 3 {
            s._obstacle_below_urgent_since = None;
            if !s._obstacle_emergency_latched {
                s._obstacle_emergency_latched = true;
                v._obstacle_doublet_requested = true;
                v._obstacle_pulse_queue.clear();
            }
        } else if state < 2 {
            match s._obstacle_below_urgent_since {
                None => s._obstacle_below_urgent_since = Some(now),
                Some(since) => {
                    if now - since >= OBSTACLE_REARM_S {
                        s._obstacle_emergency_latched = false;
                    }
                }
            }
        } else {
            s._obstacle_below_urgent_since = None;
        }
        s._obstacle_hazard = Some(Hazard {
            bearing: hazard.bearing.unwrap_or(0.0),
            urgency: (hazard.urgency.unwrap_or(0.0).trunc() as i64).clamp(0, 255),
            gap: py_max(0.0, hazard.gap.unwrap_or(f64::INFINITY)),
            state,
            closing: py_max(0.0, hazard.closing.unwrap_or(0.0)),
            ttc: hazard.ttc.unwrap_or(-1.0),
            stopping_margin: hazard.stopping_margin.unwrap_or(0.0),
            legacy: hazard.legacy.unwrap_or(false),
        });
    }

    /// Clear all obstacle data.
    pub fn clear_obstacles(&self) {
        self.update_selected_hazard(None);
    }

    // ------------------------------------------------------------------------------
    // Road guidance and route beacon
    // ------------------------------------------------------------------------------

    pub fn set_road_mode(&self, is_active: bool) {
        let mut v = self.voice.lock();
        let mut s = self.lock.lock();
        s._road_mode_active = is_active;
        if !s._road_mode_active {
            s._road_on_road = true;
            s._road_correction_active = false;
            s._road_beacon_available = false;
            v._road_beep_timer = 0.0;
            v._road_playback_pos = -1.0;
            v._road_pulse_L = None;
            v._road_pulse_R = None;
            Self::stop_road_correction_voice_locked(&mut v);
            v._road_correction_settled_pos = -1;
            s._road_correction_settled_last_time = f64::NEG_INFINITY;
            s._road_chime_queue.clear();
            s._road_junction_queue.clear();
            s._road_chime_next_allowed_time = 0.0;
        }
    }

    /// Silence every road cue without changing the user's mode toggle.
    pub fn clear_road_audio(&self) {
        let mut v = self.voice.lock();
        let mut s = self.lock.lock();
        s._junction_proximity = None;
        s._road_on_road = true;
        s._road_correction_active = false;
        s._road_beacon_available = false;
        v._road_beep_timer = 0.0;
        v._road_playback_pos = -1.0;
        v._road_pulse_L = None;
        v._road_pulse_R = None;
        Self::stop_road_correction_voice_locked(&mut v);
        v._road_correction_settled_pos = -1;
        s._road_correction_settled_last_time = f64::NEG_INFINITY;
        s._road_chime_queue.clear();
        s._road_junction_queue.clear();
    }

    /// Arm or silence the route beacon. The user's toggle, nothing else.
    pub fn set_route_beacon_mode(&self, is_active: bool) {
        let mut v = self.voice.lock();
        let mut s = self.lock.lock();
        s._route_beacon_mode_active = is_active;
        if !s._route_beacon_mode_active {
            Self::silence_route_beacon(&mut v, &mut s);
        }
    }

    /// Drop the beacon's voice, pulse buffers included: a bearing already convolved into
    /// them is a statement about where the destination WAS. Caller holds both locks.
    pub fn silence_route_beacon(v: &mut Voice, s: &mut Shared) {
        s._route_beacon_available = false;
        v._route_beacon_timer = 0.0;
        v._route_beacon_playback_pos = -1.0;
        v._route_beacon_pulse_L = None;
        v._route_beacon_pulse_R = None;
    }

    /// Feed the crow-flies bearing and range to the route's destination. `available`
    /// false means silence (no route, stale feed, standing on it).
    pub fn update_route_beacon(&self, available: bool, bearing_deg: f64, distance_m: f64) {
        let mut v = self.voice.lock();
        let mut s = self.lock.lock();
        s._route_beacon_available = available;
        if !s._route_beacon_available {
            Self::silence_route_beacon(&mut v, &mut s);
            return;
        }
        s._route_beacon_bearing = bearing_deg;
        s._route_beacon_distance = py_max(0.0, distance_m);
    }

    /// `_render_directional_pulse` with the controller's current HRTF state.
    pub fn render_directional_pulse(&self, base_pulse: &[f32], bearing_deg: f64) -> (Wave, Wave) {
        let (hrtf, enabled) = {
            let s = self.lock.lock();
            (s._hrtf.clone(), s._hrtf_user_enabled)
        };
        render_directional_pulse(hrtf.as_deref(), enabled, base_pulse, bearing_deg)
    }

    /// Independent cues: three spatial exit slots ("junction", with `choices` among
    /// "left"/"straight"/"right"), or a descending off alarm ("off").
    pub fn trigger_assistant_cue(&self, kind: &str, choices: &[&str]) {
        if !self.is_enabled() {
            return;
        }
        let mut s = self.lock.lock();
        let sr = s.samplerate;
        let mut queue = Vec::new();
        if kind == "junction" {
            // Fixed 240 ms slots preserve the missing centre at a T junction.
            let n = (sr * 0.12).trunc() as usize;
            let pulse: Vec<f32> = (0..n)
                .map(|i| {
                    let t = i as f64 / sr;
                    let wave = (2.0 * std::f64::consts::PI * 880.0 * t).sin()
                        + 0.35 * (2.0 * std::f64::consts::PI * 1760.0 * t).sin();
                    (wave * (std::f64::consts::PI * t / 0.12).sin().powi(2) * 0.2) as f32
                })
                .collect();
            for (slot, (name, bearing)) in [("left", 90.0), ("straight", 0.0), ("right", -90.0)]
                .into_iter()
                .enumerate()
            {
                if choices.contains(&name) {
                    let (l, r) =
                        render_directional_pulse(s._hrtf.as_deref(), s._hrtf_user_enabled, &pulse, bearing);
                    queue.push(PulseEntry {
                        delay: (slot as f64 * 0.24 * sr).trunc() as i64,
                        L: l,
                        R: r,
                        pos: 0,
                    });
                }
            }
        } else if kind == "off" {
            // Two falling, harmonically rich tones differ from road pips.
            let n = (sr * 0.23).trunc() as usize;
            let pulse: Wave = (0..n)
                .map(|i| {
                    let t = i as f64 / sr;
                    let phase = 2.0 * std::f64::consts::PI * (660.0 * t - 440.0 * t * t / (2.0 * 0.23));
                    ((phase.sin() + 0.3 * (2.0 * phase).sin())
                        * (std::f64::consts::PI * t / 0.23).sin().powi(2)
                        * 0.3) as f32
                })
                .collect();
            for delay in [0, (sr * 0.31).trunc() as i64] {
                queue.push(PulseEntry {
                    delay,
                    L: pulse.clone(),
                    R: pulse.clone(),
                    pos: 0,
                });
            }
        } else {
            return;
        }
        s._assistant_cue_queue = queue;
    }

    /// The `junction_proximity_enabled` property.
    pub fn junction_proximity_enabled(&self) -> bool {
        self.lock.lock()._junction_proximity_enabled
    }

    /// Refresh the shared manual/assisted stop bar from validated R2 data.
    pub fn update_junction_proximity(&self, target: Option<&JsonMap>, enabled: bool) {
        let now = self.clock.monotonic();
        let mut s = self.lock.lock();
        let mut target = target.filter(|t| !t.is_empty());
        if !enabled || !s._junction_proximity_enabled {
            target = None;
        }
        // A private-road entry is a spoken cue, not a place to stop.
        if let Some(t) = target {
            if t.get("kind").and_then(|k| k.as_str()) == Some("privateEntry") {
                target = None;
            }
        }
        if let Some(t) = target {
            let same = s
                ._junction_proximity
                .as_ref()
                .map(|cur| cur.get("id") == t.get("id"))
                .unwrap_or(false);
            if !same {
                s._junction_proximity_phase = 0.0;
            }
        }
        s._junction_proximity = target.cloned();
        s._junction_proximity_time = now;
    }

    /// `_stop_bar_signed`: `target.get("signedDistance", target["distance"])`.
    pub fn stop_bar_signed(target: &JsonMap) -> f64 {
        match target.get("signedDistance") {
            Some(v) => json_float(Some(v), f64::NAN),
            None => json_float(target.get("distance"), f64::NAN),
        }
    }

    pub fn junction_proximity_snapshot(&self) -> JunctionProximitySnapshot {
        let now = self.clock.monotonic();
        let s = self.lock.lock();
        let age = now - s._junction_proximity_time;
        let target = s._junction_proximity.clone();
        let mode = if target.is_some() {
            s._stop_bar_mode
        } else {
            StopBarState::Silent
        };
        JunctionProximitySnapshot {
            enabled: s._junction_proximity_enabled,
            active: target.is_some()
                && s._junction_proximity_enabled
                && age <= 0.75
                && mode.name() != "silent",
            age_s: py_round(age, 3),
            target,
            mode: mode.name(),
            period_s: mode.period(),
        }
    }

    /// Play the tone for a gear, given as its scale degree from neutral; sport gears play
    /// the same pitch brighter. Rendered on the caller's thread.
    pub fn trigger_gear_tone(&self, degree: i64, sport: bool) {
        if !self.is_enabled() {
            return;
        }
        let (settings, sr) = {
            let s = self.lock.lock();
            (s._gear_tone_settings.clone(), s.samplerate)
        };
        let hz = gear_report::degree_frequency(degree, &settings.root, settings.octave, &settings.scale);
        let buf = gear_report::render_tone(hz, settings.level_db, sr as u32, sport);
        let mut s = self.lock.lock();
        // Three in flight is already a shift every few tens of ms; the oldest is inaudible.
        let keep = s._gear_tones.len().saturating_sub(3);
        s._gear_tones.drain(..keep);
        s._gear_tones.push(GearTone {
            buf: buf.into(),
            pos: 0,
        });
    }

    /// `_stop_road_correction_voice`: hard-stop correction, HRTF tail included.
    pub fn stop_road_correction_voice(&self) {
        let mut v = self.voice.lock();
        Self::stop_road_correction_voice_locked(&mut v);
    }

    pub fn stop_road_correction_voice_locked(v: &mut Voice) {
        v._road_correction_env = 0.0;
        v._road_correction_pulse_phase = 0.0;
        v._road_correction_overlap_L = None;
        v._road_correction_overlap_R = None;
    }

    /// Schedule the two-tone orientation chime: G5 at `bearing_first` (the more
    /// forward-ish direction), then C5 at `bearing_second` after a gap. Rate-limited: Lua
    /// sends this every tick, but a new chime starts only once the last one has finished.
    pub fn trigger_road_orientation_chime(&self, bearing_first: f64, bearing_second: Option<f64>) {
        let (road_mode, hrtf, enabled, sr, next_allowed) = {
            let s = self.lock.lock();
            (
                s._road_mode_active,
                s._hrtf.clone(),
                s._hrtf_user_enabled,
                s.samplerate,
                s._road_chime_next_allowed_time,
            )
        };
        if !self.is_enabled() || !road_mode {
            return;
        }
        let waves = self.waveforms();
        let Some(first) = waves.ROAD_CHIME_FWD_WAVEFORM.clone() else {
            return;
        };
        let second = waves.ROAD_CHIME_BACK_WAVEFORM.clone();
        if bearing_second.is_some() && second.is_none() {
            return;
        }
        let now = self.clock.monotonic();
        if now < next_allowed {
            return; // previous chime still in flight
        }
        let (first_l, first_r) = render_directional_pulse(hrtf.as_deref(), enabled, &first, bearing_first);
        let second_rendered = match (bearing_second, second) {
            (Some(b), Some(w)) => Some(render_directional_pulse(hrtf.as_deref(), enabled, &w, b)),
            _ => None,
        };
        // The second pulse starts after the first tone's body + gap (raw timing).
        let raw_dur_samples = (sr * ROAD_CHIME_DUR_MS as f64 / 1000.0).trunc() as i64;
        let gap_samples = (sr * ROAD_CHIME_GAP_MS as f64 / 1000.0).trunc() as i64;
        let second_delay = raw_dur_samples + gap_samples;
        let mut total_ms = (ROAD_CHIME_DUR_MS + ROAD_CHIME_REPEAT_GAP_MS) as f64;
        if second_rendered.is_some() {
            total_ms += (ROAD_CHIME_DUR_MS + ROAD_CHIME_GAP_MS) as f64;
        }
        let mut s = self.lock.lock();
        s._road_chime_next_allowed_time = now + total_ms / 1000.0;
        s._road_chime_queue = vec![PulseEntry {
            delay: 0,
            L: first_l,
            R: first_r,
            pos: 0,
        }];
        if let Some((l, r)) = second_rendered {
            s._road_chime_queue.push(PulseEntry {
                delay: second_delay,
                L: l,
                R: r,
                pos: 0,
            });
        }
    }

    /// One centred double pip for the near-junction phase.
    pub fn trigger_road_junction_earcon(&self) {
        let waveform = self.waveforms().ROAD_JUNCTION_WAVEFORM.clone();
        let mut s = self.lock.lock();
        let Some(w) = waveform else { return };
        if !self.is_enabled() || !s._road_mode_active {
            return;
        }
        let delay = w.len() as i64 + (s.samplerate * ROAD_JUNCTION_GAP_MS as f64 / 1000.0).trunc() as i64;
        s._road_junction_queue = vec![
            PulseEntry {
                delay: 0,
                L: w.clone(),
                R: w.clone(),
                pos: 0,
            },
            PulseEntry {
                delay,
                L: w.clone(),
                R: w,
                pos: 0,
            },
        ];
    }

    /// One descending tone on entering the junction area.
    pub fn trigger_road_junction_entry_earcon(&self) {
        let waveform = self.waveforms().ROAD_JUNCTION_ENTRY_WAVEFORM.clone();
        let mut s = self.lock.lock();
        let Some(w) = waveform else { return };
        if !self.is_enabled() || !s._road_mode_active {
            return;
        }
        s._road_junction_queue = vec![PulseEntry {
            delay: 0,
            L: w.clone(),
            R: w,
            pos: 0,
        }];
    }

    /// A packet from a legacy road detector.
    pub fn update_road_state(&self, on_road: bool, bearing: f64, distance: f64) {
        let mut off = JsonMap::new();
        if !on_road {
            off.insert("bearing".into(), Value::from(bearing));
            off.insert("distance".into(), Value::from(distance));
        }
        self.update_road_guidance(
            if on_road { "onRoad" } else { "offRoad" },
            if on_road { None } else { Some(&off) },
            None,
            true,
        );
    }

    /// Apply one validated R2 state (`"onRoad"`, `"offRoad"`, or anything else for
    /// dormant) to the road audio renderer.
    pub fn update_road_guidance(
        &self,
        state: &str,
        off_road: Option<&JsonMap>,
        correction: Option<&JsonMap>,
        follow_enabled: bool,
    ) -> RoadGuidanceResult {
        let mut settled_triggered = false;
        let settled_wave_present = self.waveforms().ROAD_CORRECTION_SETTLED_WAVEFORM.is_some();
        let now = self.clock.monotonic();
        let mut v = self.voice.lock();
        let mut s = self.lock.lock();
        let was_off = !s._road_on_road;
        let was_correction_active = s._road_correction_active;
        let previous_phase = s._road_correction_phase.clone();
        s._road_on_road = state != "offRoad";
        s._road_beacon_available = state != "offRoad" || off_road.is_some();
        if let Some(off) = off_road.filter(|o| !o.is_empty()) {
            s._road_bearing = json_float(off.get("bearing"), 0.0);
            s._road_distance = json_float(off.get("distance"), 0.0);
        }
        s._road_follow_enabled = follow_enabled;
        let correction = correction.filter(|c| !c.is_empty());
        s._road_correction_active = state == "onRoad"
            && s._road_follow_enabled
            && correction.is_some()
            && json_truthy(correction.and_then(|c| c.get("active")));
        s._road_correction_bearing = match correction {
            Some(c) => json_float(c.get("bearing"), 0.0),
            None => 0.0,
        };
        s._road_correction_severity = py_min(
            1.0,
            py_max(
                0.0,
                match correction {
                    Some(c) => json_float(c.get("severity"), 0.0),
                    None => 0.0,
                },
            ),
        );
        s._road_correction_phase = match correction {
            Some(c) => match c.get("phase") {
                Some(p) => json_str(p),
                None => "correct".to_string(),
            },
            None => "idle".to_string(),
        };
        if s._road_correction_active
            && (!was_correction_active || s._road_correction_phase != previous_phase)
        {
            // Every new instruction starts audibly now, without the previous voice's
            // silent part of the cycle or its HRTF tail.
            Self::stop_road_correction_voice_locked(&mut v);
        }
        if state == "onRoad"
            && s._road_follow_enabled
            && json_truthy(correction.and_then(|c| c.get("settled")))
        {
            Self::stop_road_correction_voice_locked(&mut v);
            if settled_wave_present
                && now - s._road_correction_settled_last_time >= ROAD_CORRECTION_SETTLED_REFRACTORY_S
            {
                v._road_correction_settled_pos = 0;
                s._road_correction_settled_last_time = now;
                settled_triggered = true;
            }
        }
        if state != "onRoad" {
            // Off-road acquisition owns the pulsed beacon; dormant has no road voice.
            s._road_correction_active = false;
            s._road_correction_phase = "idle".to_string();
            Self::stop_road_correction_voice_locked(&mut v);
            v._road_correction_settled_pos = -1;
            s._road_correction_settled_last_time = f64::NEG_INFINITY;
        }
        if s._road_on_road && was_off {
            // Just rejoined the road: kill any pending pulse.
            v._road_playback_pos = -1.0;
            v._road_pulse_L = None;
            v._road_pulse_R = None;
        }
        if s._road_on_road {
            v._road_beep_timer = 0.0;
            v._road_playback_pos = -1.0;
            v._road_pulse_L = None;
            v._road_pulse_R = None;
        }
        RoadGuidanceResult { settled_triggered }
    }

    // ------------------------------------------------------------------------------
    // Config
    // ------------------------------------------------------------------------------

    /// `apply_config(cfg)`. Every key, clamp and regeneration in Python's order. A value
    /// whose `float()` raises aborts the rest, as the Python exception does (what was
    /// applied before it stays applied).
    pub fn apply_config(self: &Arc<Self>, cfg: &PyDict) -> Result<(), PyExc> {
        if !self.is_enabled() {
            return Ok(());
        }
        // Python regenerates waveforms with no lock held. Here the values are applied under
        // `lock` and the regenerations queued, then run after it is released (even when a
        // later key raised, since Python had already run the earlier ones).
        let mut tasks: Vec<Box<dyn FnOnce(&mut Waveforms) + Send>> = Vec::new();
        let mut stop_scan = false;
        let applied = self.apply_config_locked(cfg, &mut tasks, &mut stop_scan);
        if stop_scan {
            self.stop_scan();
        }
        if !tasks.is_empty() {
            self.update_waves(|w| {
                for task in tasks {
                    task(w);
                }
            });
        }
        let restart_target = applied?;

        // If the device selection changed while the stream is running, switch now.
        if restart_target {
            if let Some(new_idx) = self.find_target_device(true) {
                self.restart_audio_stream(new_idx);
            }
            let start_watcher = {
                let s = self.lock.lock();
                s._follow_default_enabled && s._device_watcher_thread.is_none()
            };
            if start_watcher {
                self.spawn_device_watcher();
            }
        }
        Ok(())
    }

    /// The part of `apply_config` that runs under the lock. Returns whether the device
    /// selection changed while a stream is running.
    fn apply_config_locked(
        &self,
        cfg: &PyDict,
        tasks: &mut Vec<Box<dyn FnOnce(&mut Waveforms) + Send>>,
        stop_scan: &mut bool,
    ) -> Result<bool, PyExc> {
        let restart_target;
        {
            let mut s = self.lock.lock();
            let sr = s.samplerate;

            let db = clamp_db(cfg_float(cfg, "shift_tone_level_dbfs", -12.0)?);
            s.shift_freq = py_max(
                20.0,
                py_min(20000.0, cfg_float(cfg, "shift_tone_frequency_hz", 880.0)?),
            );
            s.shift_amp = db_to_amp(db);
            s._phase_inc_shift = s.shift_freq / sr;

            let buzzer_db = cfg_float(cfg, "check_engine_buzzer_level_dbfs", -12.0)?;
            s.buzzer_amp = db_to_amp(buzzer_db);
            let chime_db = cfg_float(cfg, "oil_chime_level_dbfs", -12.0)?;
            s.chime_amp = db_to_amp(chime_db);

            s._gear_tone_settings = gear_report::settings_from_config(cfg)?;

            let (buzzer_amp, chime_amp) = (s.buzzer_amp, s.chime_amp);
            tasks.push(Box::new(move |w: &mut Waveforms| {
                w.CHECK_ENGINE_BUZZER_WAVEFORM = Some(gen::check_engine_buzzer(sr, buzzer_amp));
                w.OIL_CHIME_WAVEFORM = Some(gen::oil_chime(sr, chime_amp));
            }));

            s._hrtf_front_emphasis_db = py_min(0.0, cfg_float(cfg, "hrtf_front_emphasis_db", -6.0)?);
            s._hrtf_user_enabled = cfg_bool(cfg, "hrtf_enabled", true);
            let dist_db = cfg_float(cfg, "hrtf_distance_gain_db", 0.0)?;
            s._hrtf_distance_gain = db_to_amp(dist_db);

            // Loader implement tones.
            s._impl_tones_enabled = cfg_bool(cfg, "implement_tones_enabled", true);
            s._impl_ground_db = clamp_db(cfg_float(cfg, "implement_ground_tone_dbfs", IMPL_GROUND_MAX_DB)?);
            s._impl_tilt_db = clamp_db(cfg_float(cfg, "implement_tilt_tone_dbfs", IMPL_TILT_MAX_DB)?);

            // Docking instrument: only the pulse voice is user-levelled.
            s._dock_tones_enabled = cfg_bool(cfg, "dock_tones_enabled", true);
            s._dock_pulse_db = clamp_db(cfg_float(cfg, "dock_tone_dbfs", DOCK_PULSE_DB)?);

            // Terrain scan: only the cloud is levelled.
            s._scan_tones_enabled = cfg_bool(cfg, "scan_tones_enabled", true);
            s._scan_level_db = clamp_db(cfg_float(cfg, "scan_tone_dbfs", SCAN_TERRAIN_DB)?);
            if !s._scan_tones_enabled {
                // The callback gate alone would hold an in-flight scan's buffer for the
                // rest of the session.
                *stop_scan = true;
            }

            // Vehicle scanner pitch + steering-locked steady tone.
            s._scan_steer_tone_enabled = cfg_bool(cfg, "scanner_steer_tone_enabled", true);
            s._scan_base_freq = py_max(
                100.0,
                py_min(8000.0, cfg_float(cfg, "scanner_base_freq_hz", SCANNER_BASE_FREQ_DEFAULT)?),
            );
            s._scan_offset_oct = py_max(
                SCANNER_OFFSET_OCT_MIN,
                py_min(
                    SCANNER_OFFSET_OCT_MAX,
                    cfg_float(cfg, "scanner_pitch_offset_oct", SCANNER_OFFSET_OCT_DEFAULT)?,
                ),
            );

            s._tc_clicks_enabled = cfg_bool(cfg, "tc_clicks_enabled", true);
            s._pitch_roll_enabled = cfg_bool(cfg, "pitch_roll_tones_enabled", true);
            s._att_max_dbfs = cfg_float(cfg, "pitch_roll_max_dbfs", -24.0)?;
            s._att_min_dbfs = cfg_float(cfg, "pitch_roll_min_dbfs", -36.0)?;
            let cc_db = cfg_float(cfg, "compass_click_level_dbfs", -6.0)?;
            s._compass_click_amp = db_to_amp(cc_db);
            s._ls_click_amp_db = cfg_float(cfg, "lowspeed_click_level_dbfs", -14.0)?;
            s._ls_stop_amp_db = cfg_float(cfg, "lowspeed_stop_tone_level_dbfs", LS_STOP_AMP_DB)?;
            s._slip_amp_db = cfg_float(cfg, "slip_tone_level_dbfs", SLIP_AMP_DB)?;

            let ping_db = cfg_float(cfg, "placement_ping_volume_db", PLACEMENT_PING_AMP_DB)?;
            s._ping_amp = db_to_amp(ping_db);
            let ping_amp = s._ping_amp;
            tasks.push(Box::new(move |w: &mut Waveforms| w.PLACEMENT_PING_WAVEFORMS = Some(gen::placement_pings(sr, ping_amp))));

            let obs_db = cfg_float(cfg, "obstacle_buzz_volume_db", OBSTACLE_DEFAULT_DB)?;
            s._obstacle_amp = db_to_amp(obs_db);
            let obstacle_amp = s._obstacle_amp;
            tasks.push(Box::new(move |w: &mut Waveforms| {
                let (buzz, peak) = gen::obstacle_buzz(sr, obstacle_amp);
                w.OBSTACLE_BUZZ_WAVEFORM = Some(buzz);
                w._obstacle_harmonic_peak = Some(peak);
                w.DROPOFF_SWEEP_WAVEFORM = Some(gen::dropoff_sweep(sr));
                w.HILL_SWEEP_WAVEFORM = Some(gen::hill_sweep(sr));
            }));

            let road_db = cfg_float(cfg, "road_beep_volume_db", ROAD_BEEP_AMP_DB)?;
            s._road_amp = db_to_amp(road_db);
            let correction_db = cfg_float(cfg, "road_correction_volume_db", -24.0)?;
            s._road_correction_amp = db_to_amp(correction_db);
            let road_amp = s._road_amp;
            tasks.push(Box::new(move |w: &mut Waveforms| w.ROAD_BEEP_WAVEFORM = gen::road_beep(sr, road_amp)));
            // Route beacon volume: assigned here AND in regenerate_waveforms.
            let beacon_db = cfg_float(cfg, "route_beacon_volume_db", ROUTE_BEACON_AMP_DB)?;
            s._route_beacon_amp = db_to_amp(beacon_db);
            let beacon_amp = s._route_beacon_amp;
            tasks.push(Box::new(move |w: &mut Waveforms| {
                w.ROUTE_BEACON_WAVEFORM = gen::route_beacon_pulse(sr, beacon_amp);
                w.ROAD_CHIME_FWD_WAVEFORM = gen::road_chime_tone(sr, ROAD_CHIME_FWD_FREQ_HZ);
                w.ROAD_CHIME_BACK_WAVEFORM = gen::road_chime_tone(sr, ROAD_CHIME_BACK_FREQ_HZ);
            }));
            let junction_db = cfg_float(cfg, "road_junction_volume_db", ROAD_JUNCTION_AMP_DB)?;
            s._road_junction_amp = db_to_amp(junction_db);
            s._junction_proximity_enabled = cfg_bool(cfg, "road_junction_proximity_enabled", true);
            let (junction_amp, correction_amp) = (s._road_junction_amp, s._road_correction_amp);
            tasks.push(Box::new(move |w: &mut Waveforms| {
                w.ROAD_JUNCTION_WAVEFORM = gen::road_junction_pip(sr, junction_amp);
                w.ROAD_JUNCTION_ENTRY_WAVEFORM = gen::road_junction_entry(sr, junction_amp);
                w.ROAD_CORRECTION_SETTLED_WAVEFORM = gen::road_correction_settled(sr, correction_amp);
            }));

            // Regenerate volume-dependent waveforms.
            let (cc, ls_db, ls_stop_db) = (s._compass_click_amp, s._ls_click_amp_db, s._ls_stop_amp_db);
            tasks.push(Box::new(move |w: &mut Waveforms| {
                w.CLICK_WAVEFORM = Some(gen::click(sr, cc));
                w.HIGHLIGHT_CLICK_WAVEFORM = Some(gen::highlight_click(sr, cc));
                w.LS_CLICK_WAVEFORM = Some(gen::lowspeed_click(sr, ls_db));
                w.LS_STOP_WAVEFORM = Some(gen::lowspeed_stop_tone(sr, ls_stop_db));
                w.CAM_CLICK_WAVEFORM = Some(gen::cam_click(sr, cc));
                w.CAM_HIGHLIGHT_CLICK_WAVEFORM = Some(gen::cam_highlight_click(sr, cc));
                w.HYDRO_CENTER_CLICK_WAVEFORM = Some(gen::center_click(sr));
                w.DOCK_LOCK_WAVEFORM = Some(gen::dock_lock(sr));
                w.SLAM_CUE_WAVEFORMS = gen::slam_cues(sr);
                w.ENTRY_CUE_WAVEFORM = Some(gen::entry_cue(sr));
                w.DOCK_APPROACH_CUE_WAVEFORM = Some(gen::dock_approach_cue(sr));
            }));

            let old_follow = s._follow_default_enabled;
            let old_device = s._preferred_device_name.clone();
            s._follow_default_enabled = cfg_bool(cfg, "follow_default_audio_device", true);
            s._preferred_device_name = match cfg.get("preferred_device_name") {
                Some(PyValue::Str(text)) => beam_base::pyfmt::py_strip(text).to_string(),
                Some(other) => beam_base::pyfmt::py_strip(&other.py_str()).to_string(),
                None => String::new(),
            };
            s._audio_poll_interval = cfg_float(cfg, "audio_poll_interval_sec", 2.0)?;
            restart_target = s._audio_stream.is_some()
                && (old_follow != s._follow_default_enabled || old_device != s._preferred_device_name);
        }
        Ok(restart_target)
    }

    // ------------------------------------------------------------------------------
    // HRTF
    // ------------------------------------------------------------------------------

    /// Load a baked HRIR set. Built and resampled completely before it replaces the old
    /// one in a single assignment; on failure the current set, if any, stays.
    pub fn load_hrtf(
        &self,
        hrir_path: &Path,
        set_key: Option<(String, f64, bool)>,
        elevation_deg: Option<f64>,
        equalise: Option<bool>,
    ) -> bool {
        if !self.is_enabled() {
            return false;
        }
        let mut hrtf = HrtfSet::load(hrir_path, elevation_deg, equalise);
        let have_current = self.lock.lock()._hrtf.is_some();
        if hrtf.is_loaded() {
            let sr = self.samplerate();
            hrtf.resample(sr);
            let mut s = self.lock.lock();
            s._hrtf = Some(Arc::new(hrtf));
            s._hrtf_set_key = set_key;
            return true;
        }
        log::warn!(
            "HRTF file could not be loaded — {}",
            if have_current {
                "keeping the current set."
            } else {
                "falling back to stereo panning."
            }
        );
        false
    }

    /// Load the named set at the given elevation and equalisation unless exactly that is
    /// already active. Runs on the calling thread (the config watcher).
    pub fn select_hrtf_set(
        &self,
        set_id: &str,
        base_dir: &Path,
        elevation_deg: Option<f64>,
        equalise: Option<bool>,
    ) {
        let info = hrtf_sets::resolve(set_id);
        let elevation =
            hrtf_sets::clamp_elevation(elevation_deg.unwrap_or(hrtf_sets::DEFAULT_ELEVATION_DEG));
        let eq = equalise.unwrap_or(hrtf_sets::DEFAULT_EQUALISE);
        let key = (info.set_id.to_string(), elevation, eq);
        {
            let s = self.lock.lock();
            if s._hrtf.is_some() && s._hrtf_set_key.as_ref() == Some(&key) {
                return;
            }
        }
        if self.load_hrtf(&base_dir.join(info.filename), Some(key), Some(elevation), Some(eq)) {
            log::info!("HRTF set: {}", info.label);
        }
    }

    // ------------------------------------------------------------------------------
    // Telemetry
    // ------------------------------------------------------------------------------

    pub fn update_telemetry_state(&self, state: &TelemetryState) {
        let mut v = self.voice.lock();
        let mut s = self.lock.lock();
        if let Some(x) = state.shift_active {
            s.shift_active = x;
        }
        if let Some(x) = state.pedal_tones_active {
            s.pedal_tones_active = x;
        }
        if state.tc_active == Some(true) && !s.tc_active && s._tc_clicks_enabled {
            v._tc_playback_pos = 0.0; // TC tone on the rising edge
        }
        if let Some(x) = state.tc_active {
            s.tc_active = x;
        }
        macro_rules! take {
            ($src:ident => $dst:ident) => {
                if let Some(x) = state.$src {
                    s.$dst = x;
                }
            };
        }
        take!(last_clutch => last_clutch);
        take!(last_brake => last_brake);
        take!(last_throttle => last_throttle);
        take!(last_steering => last_steering);
        take!(inverted => inverted);
        take!(last_roll_rad => last_roll_rad);
        take!(last_pitch_rad => last_pitch_rad);
        take!(guidance_active => _guidance_active);
        take!(guidance_error_deg => _guidance_error_deg);
        take!(last_actual_steering => _hydro_actual_steer);
        take!(last_steering_input => _hydro_steer_input);
        // Already on the tone's scale; 0.0 means none or stale, which the gate silences.
        take!(trailer_artic => _trailer_artic);
        take!(trailer_reverse => _trailer_reverse);
        take!(implement_flags => _impl_flags);
        take!(implement_min_clearance => _impl_min_clearance);
        take!(implement_tilt_deg => _impl_tilt_deg);
        take!(implement_lift => _impl_lift);
        take!(implement_activity => _impl_activity);
        take!(coord_guidance_active => _coord_guidance_active);
        take!(coord_guidance_error_deg => _coord_guidance_error);
        take!(drift_alert_active => _drift_alert_active);
        take!(drift_pan => _drift_pan);
        take!(drift_rate => _drift_rate);
        take!(ls_clicks_active => _ls_clicks_active);
        take!(ls_speed_mph => _ls_speed_mph);
        take!(ls_accel => _ls_accel);
        take!(ls_stopped => _ls_stopped);
        take!(scan_speed_ms => _scan_speed_ms);
        take!(slip_active => _slip_active);
        take!(slip_kind => _slip_kind);
        take!(slip_mag => _slip_mag);
        take!(slip_ratio => _slip_ratio);
        take!(slide_active => _slide_active);
        take!(slide_deg => _slide_deg);
        take!(slip_azimuth_deg => _slip_azimuth_deg);
    }

    /// Front-back emphasis gain: 0 dB at front (0 deg), `_hrtf_front_emphasis_db` behind.
    pub fn hrtf_emphasis_gain_for(front_emphasis_db: f64, hrtf_az_deg: f64) -> f64 {
        if front_emphasis_db >= 0.0 {
            return 1.0;
        }
        let behind_norm = (1.0 - hrtf_az_deg.to_radians().cos()) * 0.5;
        db_to_amp(front_emphasis_db * behind_norm)
    }

    /// `_hrtf_emphasis_gain`.
    pub fn hrtf_emphasis_gain(&self, hrtf_az_deg: f64) -> f64 {
        Self::hrtf_emphasis_gain_for(self.lock.lock()._hrtf_front_emphasis_db, hrtf_az_deg)
    }

    // ------------------------------------------------------------------------------
    // One-shot triggers
    // ------------------------------------------------------------------------------

    /// Fire one placement ping. `rung` 0..4 selects the timbre, `azimuth_deg` is a game
    /// bearing (0 ahead, positive clockwise), `pitch_mult` shifts it for height cues.
    /// The HRTF convolution runs here, never in the callback.
    pub fn trigger_placement_ping(&self, rung: i64, azimuth_deg: f64, pitch_mult: f64) {
        if !self.is_enabled() {
            return;
        }
        let waves = self.waveforms();
        let Some(pings) = waves.PLACEMENT_PING_WAVEFORMS.as_ref().filter(|p| !p.is_empty()) else {
            return;
        };
        let rung = rung.clamp(0, pings.len() as i64 - 1);
        let base = pings[rung as usize].clone();
        let mut s = self.lock.lock();
        s._ping_request_rung = rung;
        s._ping_request_pitch_mult = pitch_mult;
        let hrtf = if s._hrtf_user_enabled { s._hrtf.clone() } else { None };
        match hrtf {
            Some(h) => {
                let hrtf_az = py_mod(360.0 - azimuth_deg, 360.0);
                if let Some((ir_l, ir_r)) = h.get_hrir(hrtf_az) {
                    let gain = Self::hrtf_emphasis_gain_for(s._hrtf_front_emphasis_db, hrtf_az)
                        * s._hrtf_distance_gain;
                    s._ping_conv_L = Some(convolve_scaled(&base, &ir_l, gain));
                    s._ping_conv_R = Some(convolve_scaled(&base, &ir_r, gain));
                    s._ping_use_hrtf = true;
                } else {
                    s._ping_use_hrtf = false;
                    s._ping_request_pan = azimuth_deg.to_radians().sin();
                }
            }
            None => {
                s._ping_use_hrtf = false;
                s._ping_request_pan = azimuth_deg.to_radians().sin();
            }
        }
        s._ping_request = true;
    }

    fn trigger_click_slot(&self, slot: ClickSlot, azimuth_deg: f64, pitch_mult: f64) {
        let waves = self.waveforms();
        let mut guard = self.lock.lock();
        let s = &mut *guard;
        let wave = match slot {
            ClickSlot::Click => waves.CLICK_WAVEFORM.clone(),
            ClickSlot::Highlight => waves.HIGHLIGHT_CLICK_WAVEFORM.clone(),
            ClickSlot::CamClick => waves.CAM_CLICK_WAVEFORM.clone(),
            ClickSlot::CamHighlight => waves.CAM_HIGHLIGHT_CLICK_WAVEFORM.clone(),
        };
        let mut conv: Option<(Wave, Wave)> = None;
        let mut tried_hrtf = false;
        if let (Some(h), true, Some(w)) = (s._hrtf.clone(), s._hrtf_user_enabled, wave) {
            tried_hrtf = true;
            let hrtf_az = py_mod(360.0 - azimuth_deg, 360.0);
            if let Some((ir_l, ir_r)) = h.get_hrir(hrtf_az) {
                let gain = Self::hrtf_emphasis_gain_for(s._hrtf_front_emphasis_db, hrtf_az)
                    * s._hrtf_distance_gain;
                conv = Some((convolve_scaled(&w, &ir_l, gain), convolve_scaled(&w, &ir_r, gain)));
            }
        }
        let _ = tried_hrtf;
        let pan = azimuth_deg.to_radians().sin();
        let (use_hrtf, request_pitch, request_pan, conv_l, conv_r, request) = match slot {
            ClickSlot::Click => (
                &mut s._click_use_hrtf,
                &mut s._click_request_pitch_mult,
                &mut s._click_request_pan,
                &mut s._click_conv_L,
                &mut s._click_conv_R,
                &mut s._click_request,
            ),
            ClickSlot::Highlight => (
                &mut s._hclick_use_hrtf,
                &mut s._highlight_request_pitch_mult,
                &mut s._highlight_request_pan,
                &mut s._hclick_conv_L,
                &mut s._hclick_conv_R,
                &mut s._highlight_click_request,
            ),
            ClickSlot::CamClick => (
                &mut s._cam_click_use_hrtf,
                &mut s._cam_click_request_pitch_mult,
                &mut s._cam_click_request_pan,
                &mut s._cam_click_conv_L,
                &mut s._cam_click_conv_R,
                &mut s._cam_click_request,
            ),
            ClickSlot::CamHighlight => (
                &mut s._cam_hclick_use_hrtf,
                &mut s._cam_highlight_request_pitch_mult,
                &mut s._cam_highlight_request_pan,
                &mut s._cam_hclick_conv_L,
                &mut s._cam_hclick_conv_R,
                &mut s._cam_highlight_click_request,
            ),
        };
        *request_pitch = pitch_mult;
        match conv {
            Some((l, r)) => {
                *conv_l = Some(l);
                *conv_r = Some(r);
                *use_hrtf = true;
            }
            None => {
                *use_hrtf = false;
                *request_pan = pan;
            }
        }
        *request = true;
    }

    pub fn trigger_compass_click(&self, azimuth_deg: f64, pitch_mult: f64) {
        self.trigger_click_slot(ClickSlot::Click, azimuth_deg, pitch_mult);
    }

    pub fn trigger_compass_highlight(&self, azimuth_deg: f64, pitch_mult: f64) {
        self.trigger_click_slot(ClickSlot::Highlight, azimuth_deg, pitch_mult);
    }

    pub fn trigger_cam_compass_click(&self, azimuth_deg: f64, pitch_mult: f64) {
        self.trigger_click_slot(ClickSlot::CamClick, azimuth_deg, pitch_mult);
    }

    pub fn trigger_cam_compass_highlight(&self, azimuth_deg: f64, pitch_mult: f64) {
        self.trigger_click_slot(ClickSlot::CamHighlight, azimuth_deg, pitch_mult);
    }

    pub fn trigger_check_engine_buzzer(&self) {
        if self.is_enabled() {
            self.voice.lock()._buzzer_playback_pos = 0.0;
        }
    }

    pub fn trigger_oil_chime(&self) {
        if self.is_enabled() {
            self.voice.lock()._chime_playback_pos = 0.0;
        }
    }

    /// One-shot tone confirming the vehicle has come to a complete stop.
    pub fn trigger_lowspeed_stopped(&self) {
        if self.is_enabled() {
            self.voice.lock()._ls_stop_playback_pos = 0.0;
        }
    }

    /// A short beep whose pitch follows node height (0 underside .. 1 top); `reverse`
    /// plays the leaving variant.
    pub fn trigger_node_hover_beep(&self, height_normalized: f64, reverse: bool) {
        if self.is_enabled() {
            let h = py_max(0.0, py_min(1.0, height_normalized));
            let mut v = self.voice.lock();
            v._node_beep_freq =
                NODE_BEEP_LOW_FREQ_HZ + (NODE_BEEP_HIGH_FREQ_HZ - NODE_BEEP_LOW_FREQ_HZ) * h;
            v._node_beep_reverse = reverse;
            v._node_beep_playback_pos = 0.0;
        }
    }

    /// FM click; `reverse` plays the leaving variant.
    pub fn trigger_clickspot_beep(&self, reverse: bool) {
        if self.is_enabled() {
            let mut v = self.voice.lock();
            v._clickspot_beep_reverse = reverse;
            v._clickspot_beep_playback_pos = 0.0;
        }
    }

    /// Low FM buzz: an AI Describe request rejected because one is already in flight.
    pub fn trigger_describe_error_buzz(&self) {
        if self.is_enabled() {
            self.voice.lock()._describe_error_playback_pos = 0.0;
        }
    }

    /// Free-camera line-up guide, per GE packet (20 Hz); `active` false when muted,
    /// closed or out of free camera. A feed that stops is aged out in the callback.
    pub fn set_lineup(&self, active: bool, amp: f64, index: f64, ratio: f64, az: f64) {
        let now = self.clock.monotonic();
        let mut s = self.lock.lock();
        s._lineup_active = active;
        s._lineup_stamp = now;
        s._lineup_target = (amp, index, ratio, az);
    }

    // ------------------------------------------------------------------------------
    // Small math helpers (methods in Python)
    // ------------------------------------------------------------------------------

    pub fn clamp(v: f64, a: f64, b: f64) -> f64 {
        common::clamp(v, a, b)
    }

    pub fn pan_gains(p: f64) -> (f64, f64) {
        common::pan_gains(p)
    }

    /// `_norm_from_angle_deg(deg, start=ATT_START_DEG, stop=ATT_STOP_DEG, cap=90.0)`.
    pub fn norm_from_angle_deg(deg: f64, start: f64, stop: f64, cap: f64) -> f64 {
        let d = deg.abs();
        if d < stop {
            return 0.0;
        }
        if d <= start {
            let n = (d - stop) / py_max(1e-6, start - stop);
            return py_max(1e-6, n * 1e-3);
        }
        common::clamp((d - start) / py_max(1e-6, cap - start), 0.0, 1.0)
    }

    /// `_amp_att_from_norm(n)` with the configured attitude dB range.
    pub fn amp_att_from_norm(att_min_dbfs: f64, att_max_dbfs: f64, n: f64) -> f64 {
        let n = common::clamp01(n);
        if n <= 0.0 {
            return 0.0;
        }
        let db = att_min_dbfs + (att_max_dbfs - att_min_dbfs) * n.powf(0.6);
        db_to_amp(db)
    }

    /// `_amp_from_val(v)`.
    pub fn amp_from_val(v: f64) -> f64 {
        if v <= 0.0 {
            return 0.0;
        }
        let db = -30.0 + 12.0 * v.powf(0.5);
        db_to_amp(db)
    }

    // ------------------------------------------------------------------------------
    // Waveforms
    // ------------------------------------------------------------------------------

    /// `_regenerate_waveforms`: every waveform at the current sample rate and levels, and
    /// the HRTF set resampled to it.
    pub fn regenerate_waveforms(&self) {
        let mut s = self.lock.lock();
        let sr = s.samplerate;
        log::info!("Generating audio waveforms for {} Hz sample rate.", beam_base::pyfmt::py_float_str(sr));
        let p = (
            s._compass_click_amp,
            s.buzzer_amp,
            s.chime_amp,
            s._obstacle_amp,
            s._road_amp,
            s._route_beacon_amp,
            s._road_junction_amp,
            s._road_correction_amp,
            s._ls_click_amp_db,
            s._ls_stop_amp_db,
            s._ping_amp,
        );
        let (cc, buzzer, chime, obstacle, road, beacon, junction, correction, ls_db, ls_stop_db, ping) = p;
        self.update_waves(|w| {
            w.CLICK_WAVEFORM = Some(gen::click(sr, cc));
            w.HIGHLIGHT_CLICK_WAVEFORM = Some(gen::highlight_click(sr, cc));
            w.TC_TONE_WAVEFORM = Some(gen::tc_tone(sr));
            w.CHECK_ENGINE_BUZZER_WAVEFORM = Some(gen::check_engine_buzzer(sr, buzzer));
            w.OIL_CHIME_WAVEFORM = Some(gen::oil_chime(sr, chime));
            w.SCANNER_BEEP_WAVEFORM = Some(gen::scanner_beep(sr));
            w.SCANNER_ALIGNED_WAVEFORM = Some(gen::scanner_aligned_beep(sr));
            let (buzz, peak) = gen::obstacle_buzz(sr, obstacle);
            w.OBSTACLE_BUZZ_WAVEFORM = Some(buzz);
            w._obstacle_harmonic_peak = Some(peak);
            w.DROPOFF_SWEEP_WAVEFORM = Some(gen::dropoff_sweep(sr));
            w.HILL_SWEEP_WAVEFORM = Some(gen::hill_sweep(sr));
            w.ROAD_BEEP_WAVEFORM = gen::road_beep(sr, road);
            w.ROUTE_BEACON_WAVEFORM = gen::route_beacon_pulse(sr, beacon);
            w.ROAD_CHIME_FWD_WAVEFORM = gen::road_chime_tone(sr, ROAD_CHIME_FWD_FREQ_HZ);
            w.ROAD_CHIME_BACK_WAVEFORM = gen::road_chime_tone(sr, ROAD_CHIME_BACK_FREQ_HZ);
            w.ROAD_JUNCTION_WAVEFORM = gen::road_junction_pip(sr, junction);
            w.ROAD_JUNCTION_ENTRY_WAVEFORM = gen::road_junction_entry(sr, junction);
            w.ROAD_CORRECTION_SETTLED_WAVEFORM = gen::road_correction_settled(sr, correction);
            w.GUIDANCE_WAVEFORM = None; // synthesised on the fly
            w.LS_CLICK_WAVEFORM = Some(gen::lowspeed_click(sr, ls_db));
            w.LS_STOP_WAVEFORM = Some(gen::lowspeed_stop_tone(sr, ls_stop_db));
            w.CAM_CLICK_WAVEFORM = Some(gen::cam_click(sr, cc));
            w.CAM_HIGHLIGHT_CLICK_WAVEFORM = Some(gen::cam_highlight_click(sr, cc));
            w.HYDRO_CENTER_CLICK_WAVEFORM = Some(gen::center_click(sr));
            w.DOCK_LOCK_WAVEFORM = Some(gen::dock_lock(sr));
            w.SLAM_CUE_WAVEFORMS = gen::slam_cues(sr);
            w.ENTRY_CUE_WAVEFORM = Some(gen::entry_cue(sr));
            w.DOCK_APPROACH_CUE_WAVEFORM = Some(gen::dock_approach_cue(sr));
            w.NODE_BEEP_WAVEFORM = Some(gen::node_beep(sr));
            w.NODE_BEEP_REV_WAVEFORM = Some(gen::node_beep_reverse(sr));
            w.CLICKSPOT_BEEP_WAVEFORM = Some(gen::clickspot_beep(sr));
            w.CLICKSPOT_BEEP_REV_WAVEFORM = Some(gen::clickspot_beep_reverse(sr));
            w.DESCRIBE_ERROR_WAVEFORM = Some(gen::describe_error_buzz(sr));
            w.PLACEMENT_PING_WAVEFORMS = Some(gen::placement_pings(sr, ping));
        });
        if let Some(h) = s._hrtf.as_ref() {
            let mut h2 = (**h).clone();
            h2.resample(sr);
            s._hrtf = Some(Arc::new(h2));
        }
    }
}

/// The waveform generators (AH's `waveforms` module), adapted to `Wave`. Every Python
/// `_generate_*` reads `self.samplerate` plus the one level attribute named here.
mod gen {
    use super::{SlamCueWaveforms, Wave};
    use crate::waveforms as wf;

    fn w(v: Vec<f32>) -> Wave {
        v.into()
    }
    fn ow(v: Option<Vec<f32>>) -> Option<Wave> {
        v.map(Into::into)
    }

    pub fn click(sr: f64, compass_click_amp: f64) -> Wave {
        w(wf::generate_click(sr, compass_click_amp))
    }
    pub fn highlight_click(sr: f64, compass_click_amp: f64) -> Wave {
        w(wf::generate_highlight_click(sr, compass_click_amp))
    }
    pub fn cam_click(sr: f64, compass_click_amp: f64) -> Wave {
        w(wf::generate_cam_click(sr, compass_click_amp))
    }
    pub fn cam_highlight_click(sr: f64, compass_click_amp: f64) -> Wave {
        w(wf::generate_cam_highlight_click(sr, compass_click_amp))
    }
    pub fn placement_pings(sr: f64, ping_amp: f64) -> Vec<Wave> {
        wf::generate_placement_pings(sr, ping_amp).into_iter().map(w).collect()
    }
    pub fn center_click(sr: f64) -> Wave {
        w(wf::generate_center_click(sr))
    }
    pub fn dock_lock(sr: f64) -> Wave {
        w(wf::generate_dock_lock(sr))
    }
    pub fn slam_cues(sr: f64) -> SlamCueWaveforms {
        SlamCueWaveforms {
            clear: Some(w(wf::generate_slam_cue(sr, "clear"))),
            over: Some(w(wf::generate_slam_cue(sr, "over"))),
            committed: Some(w(wf::generate_slam_cue(sr, "committed"))),
        }
    }
    pub fn entry_cue(sr: f64) -> Wave {
        w(wf::generate_entry_cue(sr))
    }
    pub fn dock_approach_cue(sr: f64) -> Wave {
        w(wf::generate_dock_approach_cue(sr))
    }
    pub fn tc_tone(sr: f64) -> Wave {
        w(wf::generate_tc_tone(sr))
    }
    pub fn check_engine_buzzer(sr: f64, amplitude: f64) -> Wave {
        w(wf::generate_check_engine_buzzer(sr, amplitude))
    }
    pub fn describe_error_buzz(sr: f64) -> Wave {
        w(wf::generate_describe_error_buzz(sr))
    }
    pub fn oil_chime(sr: f64, amplitude: f64) -> Wave {
        w(wf::generate_oil_chime(sr, amplitude))
    }
    pub fn scanner_beep(sr: f64) -> Wave {
        w(wf::generate_scanner_beep(sr))
    }
    pub fn scanner_aligned_beep(sr: f64) -> Wave {
        w(wf::generate_scanner_aligned_beep(sr))
    }
    /// The pulse and the `_obstacle_harmonic_peak` Python stores as a side effect.
    pub fn obstacle_buzz(sr: f64, obstacle_amp: f64) -> (Wave, f64) {
        let (wave, peak) = wf::generate_obstacle_buzz(sr, obstacle_amp);
        (w(wave), peak)
    }
    pub fn dropoff_sweep(sr: f64) -> Wave {
        w(wf::generate_dropoff_sweep(sr))
    }
    pub fn hill_sweep(sr: f64) -> Wave {
        w(wf::generate_hill_sweep(sr))
    }
    pub fn road_beep(sr: f64, road_amp: f64) -> Option<Wave> {
        ow(wf::generate_road_beep(sr, road_amp))
    }
    pub fn route_beacon_pulse(sr: f64, route_beacon_amp: f64) -> Option<Wave> {
        ow(wf::generate_route_beacon_pulse(sr, route_beacon_amp))
    }
    pub fn road_chime_tone(sr: f64, freq_hz: f64) -> Option<Wave> {
        ow(wf::generate_road_chime_tone(sr, freq_hz))
    }
    pub fn road_junction_pip(sr: f64, road_junction_amp: f64) -> Option<Wave> {
        ow(wf::generate_road_junction_pip(sr, road_junction_amp))
    }
    pub fn road_junction_entry(sr: f64, road_junction_amp: f64) -> Option<Wave> {
        ow(wf::generate_road_junction_entry(sr, road_junction_amp))
    }
    pub fn road_correction_settled(sr: f64, road_correction_amp: f64) -> Option<Wave> {
        ow(wf::generate_road_correction_settled(sr, road_correction_amp))
    }
    pub fn lowspeed_click(sr: f64, ls_click_amp_db: f64) -> Wave {
        w(wf::generate_lowspeed_click(sr, ls_click_amp_db))
    }
    pub fn lowspeed_stop_tone(sr: f64, ls_stop_amp_db: f64) -> Wave {
        w(wf::generate_lowspeed_stop_tone(sr, ls_stop_amp_db))
    }
    pub fn clickspot_beep(sr: f64) -> Wave {
        w(wf::generate_clickspot_beep(sr))
    }
    pub fn clickspot_beep_reverse(sr: f64) -> Wave {
        w(wf::generate_clickspot_beep_reverse(sr))
    }
    pub fn node_beep(sr: f64) -> Wave {
        w(wf::generate_node_beep(sr))
    }
    pub fn node_beep_reverse(sr: f64) -> Wave {
        w(wf::generate_node_beep_reverse(sr))
    }
}
