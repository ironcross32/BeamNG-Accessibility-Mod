//! Wheel slip: spin/lock wavetable voice, slide noise, HRTF (audio.py:5845-5964), with
//! `_slip_voice_tables` (3408-3449). A1. `dsp::default_rng` reproduces numpy's PCG64, so
//! the slide noise tables can match Python exactly.

use std::sync::Arc;

use crate::callback::block::Block;
use crate::controller::Voice;

/// `(spin_table, lock_table, noise_lo, noise_hi)`.
pub type SlipTables = Arc<(Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>)>;

/// PLACEHOLDER (wave 2): adds silence.
pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let _ = (b, v);
}

/// `_slip_voice_tables()`: rebuilt only when the sample rate changes (cached in
/// `v._slip_tables` / `v._slip_tables_sr`).
pub fn slip_voice_tables(v: &mut Voice, sr: f64) -> SlipTables {
    // PLACEHOLDER (wave 2): empty tables.
    let _ = sr;
    v._slip_tables
        .clone()
        .unwrap_or_else(|| Arc::new((Vec::new(), Vec::new(), Vec::new(), Vec::new())))
}
