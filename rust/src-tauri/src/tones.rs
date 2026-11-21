//! The Configuration panel's two audio previews (`configurator.play_test_tone`,
//! `configurator.play_gear_tone_preview`), as buffers for `beam_audio::device::play_buffer`.

use beam_base::gear_report;

pub const SR: u32 = 48000;

/// `play_test_tone`'s 0.75 s triangle wave with a 50 ms linear fade-out. Computed in f64
/// and narrowed at the end, as numpy did before `astype(np.float32)`.
pub fn test_tone(freq_hz: f64, level_dbfs: f64) -> Vec<f32> {
    let amp = 10f64.powf(level_dbfs / 20.0);
    let phase_inc = freq_hz / SR as f64;
    let n = (SR as f64 * 0.75) as usize;
    let mut wave: Vec<f64> = (0..n)
        .map(|i| {
            let t = (i as f64 * phase_inc).rem_euclid(1.0);
            amp * (2.0 * (2.0 * t - 1.0).abs() - 1.0)
        })
        .collect();
    let fade = (SR as f64 * 0.05) as usize;
    let start = n - fade;
    for k in 0..fade {
        // np.linspace(1, 0, fade): inclusive at both ends.
        let g = 1.0 - k as f64 / (fade - 1) as f64;
        wave[start + k] *= g;
    }
    wave.into_iter().map(|x| x as f32).collect()
}

/// Reverse, neutral, first to sixth, drive and sport 1, 0.3 s apart.
pub const PREVIEW_GEARS: [&str; 10] = ["R", "N", "1", "2", "3", "4", "5", "6", "D", "S1"];

/// `play_gear_tone_preview`'s buffer.
pub fn gear_preview(root: &str, octave: i64, scale: &str, level_dbfs: f64) -> Vec<f32> {
    let spacing = (SR as f64 * 0.3) as usize;
    let mut out = vec![0f32; spacing * PREVIEW_GEARS.len() + SR as usize / 4];
    for (i, gear) in PREVIEW_GEARS.iter().enumerate() {
        let Some(degree) = gear_report::extended_gear_degree(gear) else {
            continue;
        };
        let hz = gear_report::degree_frequency(degree, root, octave, scale);
        let tone = gear_report::render_tone(hz, level_dbfs, SR, gear_report::is_sport(gear));
        let at = i * spacing;
        for (j, s) in tone.iter().enumerate() {
            if let Some(o) = out.get_mut(at + j) {
                *o += *s;
            }
        }
    }
    out
}
