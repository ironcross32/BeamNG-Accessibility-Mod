//! Stop-bar tone (audio.py:7018) via `_mix_junction_proximity` (2413-2460) and
//! `_stop_bar_render_state` (2381-2411). Python runs the whole mix under `self.lock`, so
//! this takes `&mut Shared`. Not ducked. A2.

use std::f64::consts::PI;

use beam_base::pyfmt::py_mod;
use serde_json::Value;

use crate::callback::block::Block;
use crate::consts::*;
use crate::controller::{AudioController, JsonMap, Shared, Voice, json_float};

/// 7018: `self._mix_junction_proximity(bufL, bufR, frames)`.
pub fn render(b: &mut Block<'_>, _v: &mut Voice) {
    let now = b.clock.monotonic();
    let mut s = b.shared.lock();
    mix_junction_proximity(&mut s, now, b.buf_l, b.buf_r, b.frames);
}

/// `target["id"]` (a validated target always has one; a missing id compares as null).
fn target_id(target: &JsonMap) -> Value {
    target.get("id").cloned().unwrap_or(Value::Null)
}

/// `_mix_junction_proximity`, the caller holding the lock. `now` is `time.monotonic()`.
pub fn mix_junction_proximity(
    s: &mut Shared,
    now: f64,
    buf_l: &mut [f32],
    buf_r: &mut [f32],
    frames: usize,
) {
    if s._junction_proximity.is_some()
        && (!s._junction_proximity_enabled || now - s._junction_proximity_time > 0.75)
    {
        s._junction_proximity = None;
    }
    let target = s._junction_proximity.clone();
    let state = stop_bar_render_state(s, target.as_ref());
    s._stop_bar_mode = state;
    if state == StopBarState::Silent && s._stop_bar_env == 0.0 {
        s._stop_bar_gain = 1.0;
        return;
    }
    let sr = s.samplerate;
    let t = |k: usize| k as f64 / sr;

    let mut env: Vec<f64> = match state {
        StopBarState::Pulse(period) | StopBarState::Waiting(period) => {
            // The pulse clock counts cycles, not seconds: as the bar nears and the period
            // shortens, the tempo rises smoothly instead of cutting or stretching a pulse.
            let duty = if matches!(state, StopBarState::Pulse(_)) {
                0.5
            } else {
                STOP_BAR_WAIT_ON_S / period
            };
            let phase0 = s._junction_proximity_phase;
            s._junction_proximity_phase = py_mod(
                s._junction_proximity_phase + frames as f64 / sr / period,
                1.0,
            );
            (0..frames)
                .map(|k| {
                    let cycles = py_mod(phase0 + t(k) / period, 1.0);
                    let ramp =
                        (cycles.min(duty - cycles) * period / STOP_BAR_EDGE_S).clamp(0.0, 1.0);
                    let x = (0.5 * PI * ramp).sin();
                    x * x
                })
                .collect()
        }
        StopBarState::Solid => vec![1.0; frames],
        StopBarState::Silent => {
            // Released at a fixed rate, so a short block cannot cut the fade.
            let start = s._stop_bar_env;
            (1..=frames)
                .map(|n| (start - n as f64 / (sr * STOP_BAR_RELEASE_S)).max(0.0))
                .collect()
        }
    };
    // Every change of state starts from the previous block's last level.
    let edge = frames.min((sr * STOP_BAR_EDGE_S).trunc() as usize);
    if edge > 0 && state != StopBarState::Silent {
        let prev = s._stop_bar_env;
        for (i, e) in env[..edge].iter_mut().enumerate() {
            let blend = (i + 1) as f64 / edge as f64;
            *e = prev + (*e - prev) * blend;
        }
    }
    if frames > 0 {
        s._stop_bar_env = env[frames - 1];
    }
    let stopped_level = matches!(state, StopBarState::Waiting(_))
        || (state == StopBarState::Solid && s._stop_bar_stopped);
    let goal = if stopped_level {
        STOP_BAR_STOPPED_GAIN
    } else {
        1.0
    };
    let rate = (1.0 - STOP_BAR_STOPPED_GAIN) / STOP_BAR_FADE_S;
    let start = s._stop_bar_gain;
    let gain: Vec<f64> = (0..frames)
        .map(|k| {
            let step = rate * (t(k) + 1.0 / sr);
            if goal < start {
                np_maximum(start - step, goal)
            } else {
                np_minimum(start + step, goal)
            }
        })
        .collect();
    if frames > 0 {
        s._stop_bar_gain = gain[frames - 1];
    }
    let phase = s._stop_bar_phase;
    let sub_phase = s._stop_bar_sub_phase;
    s._stop_bar_phase = py_mod(s._stop_bar_phase + STOP_BAR_HZ * frames as f64 / sr, 1.0);
    s._stop_bar_sub_phase = py_mod(
        s._stop_bar_sub_phase + STOP_BAR_SUB_HZ * frames as f64 / sr,
        1.0,
    );
    let amp = s._road_junction_amp;
    for k in 0..frames {
        let cycles = phase + STOP_BAR_HZ * t(k);
        let sub_cycles = sub_phase + STOP_BAR_SUB_HZ * t(k);
        let tone = ((2.0 * PI * cycles).sin() + STOP_BAR_SUB_LEVEL * (2.0 * PI * sub_cycles).sin())
            / (1.0 + STOP_BAR_SUB_LEVEL);
        let wave = (tone * env[k] * gain[k] * amp) as f32;
        buf_l[k] += wave;
        buf_r[k] += wave;
    }
}

/// `np.maximum(a, b)`: NaN propagates.
fn np_maximum(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `np.minimum(a, b)`: NaN propagates.
fn np_minimum(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

/// `_stop_bar_render_state(target)`: advance the stopped and on-bar hysteresis for one
/// block and return the state.
pub fn stop_bar_render_state(s: &mut Shared, target: Option<&JsonMap>) -> StopBarState {
    let Some(target) = target.filter(|t| !t.is_empty()) else {
        s._stop_bar_on_id = None;
        s._stop_bar_departed_id = None;
        return StopBarState::Silent;
    };
    let id = target_id(target);
    let speed = json_float(target.get("speed"), f64::NAN);
    if s._stop_bar_stopped && speed > STOP_BAR_MOVING_MPS {
        s._stop_bar_stopped = false;
    } else if !s._stop_bar_stopped && speed < STOP_BAR_STOPPED_MPS {
        s._stop_bar_stopped = true;
    }
    let signed = AudioController::stop_bar_signed(target);
    if signed < 0.0 && !s._stop_bar_stopped {
        s._stop_bar_departed_id = Some(id.clone());
    } else if signed > STOP_BAR_REACH_M + STOP_BAR_HOLD_M {
        s._stop_bar_departed_id = None; // back before it: a new approach
    }
    if s._stop_bar_departed_id.as_ref() == Some(&id) {
        // Crossed the line moving: stopping a little further on does not bring the bar
        // back.
        s._stop_bar_on_id = None;
        return StopBarState::Silent;
    }
    let state = stop_bar_state(
        signed,
        speed,
        Some(s._stop_bar_stopped),
        s._stop_bar_on_id.as_ref() == Some(&id),
    );
    s._stop_bar_on_id = if state == StopBarState::Solid {
        Some(id.clone())
    } else {
        None
    };
    if state == StopBarState::Solid {
        s._stop_bar_reached_id = Some(id);
    } else if matches!(state, StopBarState::Waiting(_))
        && s._stop_bar_reached_id.as_ref() == Some(&id)
    {
        // Backed off a bar already reached: that is leaving it, not waiting.
        return StopBarState::Silent;
    }
    state
}
