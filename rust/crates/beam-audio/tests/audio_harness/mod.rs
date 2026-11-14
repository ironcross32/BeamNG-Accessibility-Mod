//! Shared harness for the audio callback tests (AA's; `tests/common` is AH's): the Rust
//! twin of `rust/fixtures/gen/audio_callback_harness.py`. Builds a controller with no
//! stream and a fake clock (both `monotonic` and `perf_counter`, starting at 100.0),
//! zeroes the pedal detune, generates the waveforms, applies a config, and runs blocks.
//! Use from a test file with `mod audio_harness;`.

#![allow(dead_code)]

use std::sync::Arc;

use beam_audio::AudioController;
use beam_audio::FakeClock;
use beam_base::pyval::{PyDict, from_json_map};
use serde_json::Value;

pub const START_CLOCK: f64 = 100.0;

pub struct Harness {
    pub ctrl: Arc<AudioController>,
    pub clock: Arc<FakeClock>,
}

/// `make_controller(config)` in the Python harness.
pub fn harness(config: Option<&PyDict>) -> Harness {
    let clock = FakeClock::new(START_CLOCK);
    let ctrl = AudioController::with_clock(clock.clone());
    {
        let mut v = ctrl.voice.lock();
        v._detune_c = 0.0;
        v._detune_b = 0.0;
        v._detune_t = 0.0;
    }
    ctrl.regenerate_waveforms();
    if let Some(cfg) = config {
        ctrl.apply_config(cfg).expect("apply_config");
    }
    Harness { ctrl, clock }
}

/// A config from a fixture's JSON object (`null` gives `None`).
pub fn config_from_json(v: &Value) -> Option<PyDict> {
    v.as_object().map(from_json_map)
}

impl Harness {
    /// Run `n` blocks of `frames`, returning each block's clipped `(left, right)`.
    pub fn run_blocks(&self, n: usize, frames: usize) -> Vec<(Vec<f32>, Vec<f32>)> {
        (0..n).map(|_| self.ctrl.render_block(frames)).collect()
    }
}

/// The recorded blocks of a fixture case, as `(left, right)` f32 vectors.
pub fn fixture_blocks(case: &Value) -> Vec<(Vec<f32>, Vec<f32>)> {
    case["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .map(|b| {
            let side = |k: &str| -> Vec<f32> {
                b[k].as_array()
                    .expect("samples")
                    .iter()
                    .map(|x| x.as_f64().expect("float") as f32)
                    .collect()
            };
            (side("left"), side("right"))
        })
        .collect()
}

/// Largest absolute sample difference between two runs (panics on a shape mismatch).
pub fn max_abs_diff(a: &[(Vec<f32>, Vec<f32>)], b: &[(Vec<f32>, Vec<f32>)], what: &str) -> f32 {
    assert_eq!(a.len(), b.len(), "{what}: block count");
    let mut worst = 0.0f32;
    for (i, ((al, ar), (bl, br))) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(al.len(), bl.len(), "{what}: block {i} length");
        for (x, y) in al.iter().zip(bl.iter()).chain(ar.iter().zip(br.iter())) {
            worst = worst.max((x - y).abs());
        }
    }
    worst
}
