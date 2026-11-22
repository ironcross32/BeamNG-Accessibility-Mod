//! The audio callback frame (audio.py `_audio_callback`, 4138-7023): request intake, the
//! snapshot under the lock, every voice in Python's order with the obstacle duck buses,
//! the final clip to [-0.999, 0.999] and the stereo interleave. Callable without a device
//! (`AudioController::render_block`), which is how every audio test drives it.
//!
//! The voice bodies live one per file; see `rust_plans/audio-callback-split.md` for the
//! line ranges, couplings and who fills which.

pub mod block;
pub mod clock;
pub mod common;

pub mod assistant_cues;
pub mod attitude;
pub mod cannon_aim;
pub mod compass_clicks;
pub mod coord_guidance;
pub mod coupler;
pub mod dock;
pub mod dock_earcons;
pub mod drift;
pub mod gear_tones;
pub mod heading_guidance;
pub mod hydro_steer;
pub mod implement;
pub mod junction_proximity;
pub mod lineup;
pub mod lowspeed;
pub mod node_clickspot;
pub mod obstacle;
pub mod oneshots;
pub mod pedals_steer;
pub mod placement_ping;
pub mod road_beacon;
pub mod road_correction;
pub mod road_queues;
pub mod route_beacon;
pub mod scan_playback;
pub mod scanner;
pub mod shift_tone;
pub mod slip;

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::callback::block::{Block, Snapshot};
use crate::consts::{LINEUP_STALE_S, TRAILER_DUCK_TAU, obstacle_duck_gain};
use crate::controller::{AudioController, Shared, Voice};

/// `bufL = mix + (bufL - mix) * duck`, all in `f32`, applied only when `duck < 1.0`.
fn apply_duck(buf: &mut [f32], mix: &[f32], duck: f64) {
    if duck < 1.0 {
        let d = duck as f32;
        for (b, m) in buf.iter_mut().zip(mix.iter()) {
            *b = *m + (*b - *m) * d;
        }
    }
}

/// `scanner_mix_l = bufL.copy()` into a preallocated scratch.
fn copy_into(dst: &mut Vec<f32>, src: &[f32]) {
    dst.clear();
    dst.extend_from_slice(src);
}

impl AudioController {
    /// The cpal data callback body: fill `out` (interleaved stereo, `frames * 2` samples).
    /// Never panics. A panic inside a voice is Python's exception escaping into
    /// sounddevice's cffi callback (`error=paAbort`): logged once, and the stream stays
    /// silent from then on.
    pub fn audio_callback(&self, out: &mut [f32], frames: usize) {
        let frames = frames.min(out.len() / 2);
        let mut guard = self.voice.lock();
        let v = &mut *guard;
        if v.callback_aborted {
            out.fill(0.0);
            return;
        }
        let ok = catch_unwind(AssertUnwindSafe(|| self.run_block(v, frames))).is_ok();
        if !ok {
            log::error!("Audio callback raised; the stream is aborted.");
            v.callback_aborted = true;
            out.fill(0.0);
            return;
        }
        // out = np.stack([bufL, bufR], axis=1).astype(np.float32); np.clip(out, -0.999, 0.999)
        for k in 0..frames {
            out[2 * k] = v.buf_l[k].clamp(-0.999f32, 0.999f32);
            out[2 * k + 1] = v.buf_r[k].clamp(-0.999f32, 0.999f32);
        }
        for x in out[2 * frames..].iter_mut() {
            *x = 0.0;
        }
    }

    /// Run one block with no device and return the clipped `(left, right)` buffers: the
    /// shape the Python sims get from `_audio_callback(block, frames, None, None)`.
    pub fn render_block(&self, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut out = vec![0.0f32; frames * 2];
        self.audio_callback(&mut out, frames);
        let l = out.iter().step_by(2).copied().collect();
        let r = out.iter().skip(1).step_by(2).copied().collect();
        (l, r)
    }

    /// Request intake (4139-4180) and the snapshot (4182-4308), under one hold of the lock.
    fn intake_and_snapshot(&self, v: &mut Voice, s: &mut Shared, frames: usize) -> Snapshot {
        if s._click_request {
            v._click_playback_pos = 0.0;
            v._click_pitch_mult = s._click_request_pitch_mult;
            if !s._click_use_hrtf {
                v._click_pan_pos = s._click_request_pan;
            }
            s._click_request = false;
        }
        if s._ping_request {
            v._ping_playback_pos = 0.0;
            v._ping_rung = s._ping_request_rung;
            v._ping_pitch_mult = s._ping_request_pitch_mult;
            if !s._ping_use_hrtf {
                v._ping_pan_pos = s._ping_request_pan;
            }
            s._ping_request = false;
        }
        if s._highlight_click_request {
            v._highlight_click_playback_pos = 0.0;
            v._highlight_click_pitch_mult = s._highlight_request_pitch_mult;
            if !s._hclick_use_hrtf {
                v._highlight_click_pan_pos = s._highlight_request_pan;
            }
            s._highlight_click_request = false;
        }
        if s._cam_click_request {
            v._cam_click_playback_pos = 0.0;
            v._cam_click_pitch_mult = s._cam_click_request_pitch_mult;
            if !s._cam_click_use_hrtf {
                v._cam_click_pan_pos = s._cam_click_request_pan;
            }
            s._cam_click_request = false;
        }
        if s._cam_highlight_click_request {
            v._cam_highlight_click_playback_pos = 0.0;
            v._cam_highlight_click_pitch_mult = s._cam_highlight_request_pitch_mult;
            if !s._cam_hclick_use_hrtf {
                v._cam_highlight_click_pan_pos = s._cam_highlight_request_pan;
            }
            s._cam_highlight_click_request = false;
        }

        let sr = s.samplerate;
        let obstacle_hazard = s._obstacle_hazard.clone();
        let obstacle_duck =
            obstacle_duck_gain(obstacle_hazard.as_ref().map(|h| h.state).unwrap_or(0));

        // One channel, two sources, never both: the frame bend wins whenever the vehicle
        // has hydraulic steering. "Exactly zero" is the wire contract for "no such thing".
        let mut hydro_actual = s._hydro_actual_steer;
        let hydro_from_trailer = hydro_actual == 0.0;
        if hydro_from_trailer {
            hydro_actual = s._trailer_artic;
        }
        // Duck only the trailer source, and only going forward. Glided, never stepped.
        let trailer_duck_target = if hydro_from_trailer && !s._trailer_reverse {
            1.0
        } else {
            0.0
        };
        s._trailer_duck_env += (trailer_duck_target - s._trailer_duck_env)
            * crate::consts::py_min(1.0, (frames as f64 / sr) / TRAILER_DUCK_TAU);

        let lineup_live =
            s._lineup_active && (self.clock.monotonic() - s._lineup_stamp) <= LINEUP_STALE_S;

        Snapshot {
            samplerate: sr,
            shift_on: s.shift_active,
            pt_on: s.pedal_tones_active,
            v_clutch: s.last_clutch,
            v_brake: s.last_brake,
            v_throt: s.last_throttle,
            v_steer: s._hydro_steer_input,
            inv: s.inverted,
            roll_rad: s.last_roll_rad,
            pitch_rad: s.last_pitch_rad,
            scan_active: s._scan_mode_active,
            scan_bearing: s._scanner_target_bearing,
            scan_dist: s._scanner_target_distance,
            cannon_aim_have: s._cannon_aim_have,
            cannon_elevation: s._cannon_elevation,
            cannon_aim_bearing: s._cannon_aim_bearing,
            cannon_target_angle: s._cannon_target_angle,
            cannon_aim_solution_available: s._cannon_aim_solution_available,
            cannon_aim_reachable: s._cannon_aim_reachable,
            cannon_aim_stamp: s._cannon_aim_stamp,
            coupler_active: s._coupler_active,
            coupler_bearing: s._coupler_bearing,
            coupler_dist: s._coupler_distance,
            coupler_in_range: s._coupler_in_range,
            obs_active: s._obstacle_mode_active,
            obstacle_hazard,
            terrain_type: s._terrain_type,
            road_active: s._road_mode_active,
            road_on_road: s._road_on_road,
            road_bearing: s._road_bearing,
            road_distance: s._road_distance,
            road_beacon_available: s._road_beacon_available,
            road_correction_active: s._road_correction_active,
            road_correction_bearing: s._road_correction_bearing,
            road_correction_severity: s._road_correction_severity,
            road_correction_phase: s._road_correction_phase.clone(),
            route_beacon_on: s._route_beacon_mode_active,
            route_beacon_have: s._route_beacon_available,
            route_beacon_bearing: s._route_beacon_bearing,
            route_beacon_distance: s._route_beacon_distance,
            obstacle_duck,
            guide_active: s._guidance_active,
            guide_error: s._guidance_error_deg,
            hydro_actual,
            hydro_from_trailer,
            trailer_duck_env: s._trailer_duck_env,
            impl_flags: s._impl_flags,
            impl_clearance: s._impl_min_clearance,
            impl_tilt_deg: s._impl_tilt_deg,
            impl_lift: s._impl_lift,
            impl_activity: s._impl_activity,
            impl_enabled: s._impl_tones_enabled,
            dock_on: s._dock_active && s._dock_tones_enabled,
            dock_have: s._dock_have,
            dock_range: s._dock_range,
            dock_lateral: s._dock_lateral,
            dock_null: s._dock_null,
            dock_mode: s._dock_mode.clone(),
            dock_acquire: s._dock_acquire,
            dock_bearing: s._dock_bearing,
            dock_stamp: s._dock_stamp,
            dock_pulse_db: s._dock_pulse_db,
            coord_guide_active: s._coord_guidance_active,
            coord_guide_error: s._coord_guidance_error,
            lineup_live,
            lineup_target: s._lineup_target,
            ls_active: s._ls_clicks_active,
            ls_speed_mph: s._ls_speed_mph,
            ls_accel: s._ls_accel,
            ls_stopped: s._ls_stopped,
            slip_active: s._slip_active,
            slip_kind: s._slip_kind,
            slip_mag: s._slip_mag,
            slip_ratio: s._slip_ratio,
            slide_active: s._slide_active,
            slide_deg: s._slide_deg,
            slip_azimuth_deg: s._slip_azimuth_deg,
            scan_speed_ms: s._scan_speed_ms,
            scan_closing_ms: s._scan_closing_ms,
            scan_base_freq: s._scan_base_freq,
            scan_offset_oct: s._scan_offset_oct,
            scan_steer_tone_enabled: s._scan_steer_tone_enabled,
            _drift_alert_active: s._drift_alert_active,
            _drift_pan: s._drift_pan,
            _drift_rate: s._drift_rate,
            _scan_tones_enabled: s._scan_tones_enabled,
            _pitch_roll_enabled: s._pitch_roll_enabled,
            _att_max_dbfs: s._att_max_dbfs,
            _att_min_dbfs: s._att_min_dbfs,
            shift_amp: s.shift_amp,
            _phase_inc_shift: s._phase_inc_shift,
            _hrtf_user_enabled: s._hrtf_user_enabled,
            _hrtf_front_emphasis_db: s._hrtf_front_emphasis_db,
            _hrtf_distance_gain: s._hrtf_distance_gain,
            _obstacle_amp: s._obstacle_amp,
            _slip_amp_db: s._slip_amp_db,
            _impl_ground_db: s._impl_ground_db,
            _impl_tilt_db: s._impl_tilt_db,
            _road_correction_amp: s._road_correction_amp,
            _hrtf: s._hrtf.clone(),
            _click_use_hrtf: s._click_use_hrtf,
            _click_conv_L: s._click_conv_L.clone(),
            _click_conv_R: s._click_conv_R.clone(),
            _hclick_use_hrtf: s._hclick_use_hrtf,
            _hclick_conv_L: s._hclick_conv_L.clone(),
            _hclick_conv_R: s._hclick_conv_R.clone(),
            _cam_click_use_hrtf: s._cam_click_use_hrtf,
            _cam_click_conv_L: s._cam_click_conv_L.clone(),
            _cam_click_conv_R: s._cam_click_conv_R.clone(),
            _cam_hclick_use_hrtf: s._cam_hclick_use_hrtf,
            _cam_hclick_conv_L: s._cam_hclick_conv_L.clone(),
            _cam_hclick_conv_R: s._cam_hclick_conv_R.clone(),
            _ping_use_hrtf: s._ping_use_hrtf,
            _ping_conv_L: s._ping_conv_L.clone(),
            _ping_conv_R: s._ping_conv_R.clone(),
        }
    }

    /// One block into `v.buf_l` / `v.buf_r` (unclipped). Voices run in Python's order.
    fn run_block(&self, v: &mut Voice, frames: usize) {
        let snap = {
            let mut s = self.lock.lock();
            self.intake_and_snapshot(v, &mut s, frames)
        };
        let waves = self.waveforms();

        // bufL, bufR = np.zeros(frames, float32) -- reusing the previous block's storage.
        let mut buf_l = std::mem::take(&mut v.buf_l);
        let mut buf_r = std::mem::take(&mut v.buf_r);
        let mut mix_l = std::mem::take(&mut v.mix_l);
        let mut mix_r = std::mem::take(&mut v.mix_r);
        buf_l.clear();
        buf_l.resize(frames, 0.0);
        buf_r.clear();
        buf_r.resize(frames, 0.0);
        let duck = snap.obstacle_duck;

        {
            let mut b = Block {
                frames,
                sr: snap.samplerate,
                snap: &snap,
                waves: &waves,
                clock: &*self.clock,
                shared: &self.lock,
                buf_l: &mut buf_l,
                buf_r: &mut buf_r,
            };

            oneshots::render(&mut b, v); // 4315-4332
            placement_ping::render(&mut b, v); // 4334-4369
            compass_clicks::render(&mut b, v); // 4371-4627
            shift_tone::render(&mut b, v); // 4629-4635
            pedals_steer::render(&mut b, v); // 4637-4690
            attitude::render(&mut b, v); // 4692-4855
            drift::render(&mut b, v); // 4857-4924
            scanner::smooth_bearing(&mut b, v); // 4926-4947

            // Scanner duck bus (4949 .. 5179).
            copy_into(&mut mix_l, b.buf_l);
            copy_into(&mut mix_r, b.buf_r);
            scanner::render(&mut b, v); // 4951-5177
            apply_duck(b.buf_l, &mix_l, duck);
            apply_duck(b.buf_r, &mix_r, duck);

            cannon_aim::render(&mut b, v); // 5183-5241
            node_clickspot::render(&mut b, v); // 5243-5287
            coupler::render(&mut b, v); // 5289-5369
            obstacle::render(&mut b, v); // 5371-5514

            // Road duck bus (5516 .. 5716).
            copy_into(&mut mix_l, b.buf_l);
            copy_into(&mut mix_r, b.buf_r);
            road_beacon::render(&mut b, v); // 5516-5575
            route_beacon::render(&mut b, v); // 5577-5622
            road_correction::render(&mut b, v); // 5624-5654
            road_queues::render(&mut b, v); // 5656-5714
            apply_duck(b.buf_l, &mix_l, duck);
            apply_duck(b.buf_r, &mix_r, duck);

            lowspeed::render(&mut b, v); // 5720-5843
            slip::render(&mut b, v); // 5845-5964

            // Heading duck bus (5966 .. 6034).
            copy_into(&mut mix_l, b.buf_l);
            copy_into(&mut mix_r, b.buf_r);
            heading_guidance::render(&mut b, v); // 5968-6032
            apply_duck(b.buf_l, &mix_l, duck);
            apply_duck(b.buf_r, &mix_r, duck);

            hydro_steer::render(&mut b, v); // 6038-6221
            implement::render(&mut b, v); // 6223-6457

            // Dock duck bus (6460 .. 6814).
            copy_into(&mut mix_l, b.buf_l);
            copy_into(&mut mix_r, b.buf_r);
            dock::render(&mut b, v); // 6462-6812
            apply_duck(b.buf_l, &mix_l, duck);
            apply_duck(b.buf_r, &mix_r, duck);

            dock_earcons::render(&mut b, v); // 6818-6867

            // Coordinate duck bus (6869 .. 6970).
            copy_into(&mut mix_l, b.buf_l);
            copy_into(&mut mix_r, b.buf_r);
            coord_guidance::render(&mut b, v); // 6871-6968
            apply_duck(b.buf_l, &mix_l, duck);
            apply_duck(b.buf_r, &mix_r, duck);

            lineup::render(&mut b, v); // 6974-6977
            scan_playback::render(&mut b, v); // 6979-7016
            junction_proximity::render(&mut b, v); // 7018
            assistant_cues::render(&mut b, v); // 7019
            gear_tones::render(&mut b, v); // 7020
        }

        v.buf_l = buf_l;
        v.buf_r = buf_r;
        v.mix_l = mix_l;
        v.mix_r = mix_r;
    }
}
