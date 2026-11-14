//! What one callback block hands each voice: the snapshot Python copies under the lock
//! (`audio.py:4182-4308`, same local names), the waveforms, the HRTF set, the clock, the
//! output buffers, and the state lock for the voices Python mixes under it.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::callback::clock::Clock;
use crate::callback::common::Wave;
use crate::controller::{Hazard, Shared, Waveforms};
use crate::hrtf::HrtfSet;

/// The locals `_audio_callback` copies under `self.lock`, plus what Python reads from
/// `self` outside the lock that another thread writes (config params, drift inputs, the
/// HRTF set and the five one-shot convolutions). Names are the Python local names; where
/// Python read `self._x` directly the field is `_x`.
#[derive(Clone, Default)]
#[allow(non_snake_case)]
pub struct Snapshot {
    pub samplerate: f64,
    pub shift_on: bool,
    pub pt_on: bool,
    pub v_clutch: f64,
    pub v_brake: f64,
    pub v_throt: f64,
    /// `_hydro_steer_input`, normalised -1..1 steering input.
    pub v_steer: f64,
    pub inv: bool,
    pub roll_rad: f64,
    pub pitch_rad: f64,
    pub scan_active: bool,
    pub scan_bearing: f64,
    pub scan_dist: f64,
    pub cannon_aim_have: bool,
    pub cannon_elevation: f64,
    pub cannon_aim_bearing: f64,
    pub cannon_target_angle: f64,
    pub cannon_aim_solution_available: bool,
    pub cannon_aim_reachable: bool,
    pub cannon_aim_stamp: f64,
    pub coupler_active: bool,
    pub coupler_bearing: f64,
    pub coupler_dist: f64,
    pub coupler_in_range: bool,
    pub obs_active: bool,
    pub obstacle_hazard: Option<Hazard>,
    pub terrain_type: i64,
    pub road_active: bool,
    pub road_on_road: bool,
    pub road_bearing: f64,
    pub road_distance: f64,
    pub road_beacon_available: bool,
    pub road_correction_active: bool,
    pub road_correction_bearing: f64,
    pub road_correction_severity: f64,
    pub road_correction_phase: String,
    pub route_beacon_on: bool,
    pub route_beacon_have: bool,
    pub route_beacon_bearing: f64,
    pub route_beacon_distance: f64,
    pub obstacle_duck: f64,
    pub guide_active: bool,
    pub guide_error: f64,
    /// The articulation tone's source: the frame bend, or the trailer when that is exactly 0.
    pub hydro_actual: f64,
    pub hydro_from_trailer: bool,
    pub trailer_duck_env: f64,
    pub impl_flags: i64,
    pub impl_clearance: f64,
    pub impl_tilt_deg: f64,
    pub impl_lift: f64,
    pub impl_activity: f64,
    pub impl_enabled: bool,
    pub dock_on: bool,
    pub dock_have: bool,
    pub dock_range: f64,
    pub dock_lateral: f64,
    pub dock_null: f64,
    pub dock_mode: String,
    pub dock_acquire: bool,
    pub dock_bearing: f64,
    pub dock_stamp: f64,
    pub dock_pulse_db: f64,
    pub coord_guide_active: bool,
    pub coord_guide_error: f64,
    pub lineup_live: bool,
    pub lineup_target: (f64, f64, f64, f64),
    pub ls_active: bool,
    pub ls_speed_mph: f64,
    pub ls_accel: f64,
    pub ls_stopped: bool,
    pub slip_active: bool,
    pub slip_kind: i64,
    pub slip_mag: f64,
    pub slip_ratio: f64,
    pub slide_active: bool,
    pub slide_deg: f64,
    pub slip_azimuth_deg: f64,
    pub scan_speed_ms: f64,
    pub scan_closing_ms: f64,
    pub scan_base_freq: f64,
    pub scan_offset_oct: f64,
    pub scan_steer_tone_enabled: bool,

    // Read from `self` outside the lock in Python.
    pub _drift_alert_active: bool,
    pub _drift_pan: f64,
    pub _drift_rate: f64,
    pub _scan_tones_enabled: bool,
    pub _pitch_roll_enabled: bool,
    pub _att_max_dbfs: f64,
    pub _att_min_dbfs: f64,
    pub shift_amp: f64,
    pub _phase_inc_shift: f64,
    pub _hrtf_user_enabled: bool,
    pub _hrtf_front_emphasis_db: f64,
    pub _hrtf_distance_gain: f64,
    pub _obstacle_amp: f64,
    pub _slip_amp_db: f64,
    pub _impl_ground_db: f64,
    pub _impl_tilt_db: f64,
    pub _road_correction_amp: f64,
    pub _hrtf: Option<Arc<HrtfSet>>,
    pub _click_use_hrtf: bool,
    pub _click_conv_L: Option<Wave>,
    pub _click_conv_R: Option<Wave>,
    pub _hclick_use_hrtf: bool,
    pub _hclick_conv_L: Option<Wave>,
    pub _hclick_conv_R: Option<Wave>,
    pub _cam_click_use_hrtf: bool,
    pub _cam_click_conv_L: Option<Wave>,
    pub _cam_click_conv_R: Option<Wave>,
    pub _cam_hclick_use_hrtf: bool,
    pub _cam_hclick_conv_L: Option<Wave>,
    pub _cam_hclick_conv_R: Option<Wave>,
    pub _ping_use_hrtf: bool,
    pub _ping_conv_L: Option<Wave>,
    pub _ping_conv_R: Option<Wave>,
}

impl Snapshot {
    /// The HRTF set when the user has it enabled: Python's
    /// `self._hrtf is not None and self._hrtf_user_enabled`.
    pub fn hrtf(&self) -> Option<&HrtfSet> {
        if self._hrtf_user_enabled {
            self._hrtf.as_deref()
        } else {
            None
        }
    }
}

/// Everything one voice call receives besides `&mut Voice`.
pub struct Block<'a> {
    /// The callback's `frames`.
    pub frames: usize,
    /// `self.samplerate`.
    pub sr: f64,
    pub snap: &'a Snapshot,
    pub waves: &'a Waveforms,
    pub clock: &'a dyn Clock,
    /// `self.lock`, for the voices Python mixes while holding it (road queues, junction
    /// proximity, assistant cues, gear tones). Never lock `voice` from inside a voice.
    pub shared: &'a Mutex<Shared>,
    /// `bufL` / `bufR`, `frames` long, `float32`.
    pub buf_l: &'a mut [f32],
    pub buf_r: &'a mut [f32],
}
