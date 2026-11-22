//! Gear report phrases, degrees, scales and tone, from gear_report.py.
//!
//! Pure like route_beacon and wheel_slip: the engine decides WHEN a gear changed, the
//! audio crate owns the stream, and everything in between lives here so
//! `tests/gear_report_sim.rs` can assert it with no game, no device and no clock.

use crate::pyfmt::py_strip;
use crate::pyval::{PyDict, PyExc, PyInt, PyValue, py_max, py_min};

pub const MODES: [&str; 3] = ["off", "speech", "audio"];
pub const NUMBER_STYLES: [&str; 2] = ["ordinal", "number"];

pub const ORDINAL_WORDS: [(i64, &str); 12] = [
    (1, "first"),
    (2, "second"),
    (3, "third"),
    (4, "fourth"),
    (5, "fifth"),
    (6, "sixth"),
    (7, "seventh"),
    (8, "eighth"),
    (9, "ninth"),
    (10, "tenth"),
    (11, "eleventh"),
    (12, "twelfth"),
];

/// (config value, spoken label). Enharmonic pairs share one entry: they are one pitch
/// in equal temperament, and two entries for it would read as two choices that differ.
pub const ROOT_NOTES: [(&str, &str); 12] = [
    ("C", "C"),
    ("C#", "C sharp / D flat"),
    ("D", "D"),
    ("D#", "D sharp / E flat"),
    ("E", "E"),
    ("F", "F"),
    ("F#", "F sharp / G flat"),
    ("G", "G"),
    ("G#", "G sharp / A flat"),
    ("A", "A"),
    ("A#", "A sharp / B flat"),
    ("B", "B"),
];
pub const ROOT_OCTAVES: [i64; 5] = [2, 3, 4, 5, 6];

/// A scale: config value, label, steps in semitones above the root within one octave.
/// Steps are floats so the quarter-tone scales sit on the same arithmetic as the rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    pub key: &'static str,
    pub label: &'static str,
    pub steps: &'static [f64],
}

const fn scale(key: &'static str, label: &'static str, steps: &'static [f64]) -> Scale {
    Scale { key, label, steps }
}

pub const SCALES: [Scale; 16] = [
    scale("major", "Major", &[0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0]),
    scale(
        "natural_minor",
        "Natural minor",
        &[0.0, 2.0, 3.0, 5.0, 7.0, 8.0, 10.0],
    ),
    scale(
        "harmonic_minor",
        "Harmonic minor",
        &[0.0, 2.0, 3.0, 5.0, 7.0, 8.0, 11.0],
    ),
    scale(
        "melodic_minor",
        "Melodic minor",
        &[0.0, 2.0, 3.0, 5.0, 7.0, 9.0, 11.0],
    ),
    scale("dorian", "Dorian", &[0.0, 2.0, 3.0, 5.0, 7.0, 9.0, 10.0]),
    scale(
        "phrygian",
        "Phrygian",
        &[0.0, 1.0, 3.0, 5.0, 7.0, 8.0, 10.0],
    ),
    scale("lydian", "Lydian", &[0.0, 2.0, 4.0, 6.0, 7.0, 9.0, 11.0]),
    scale(
        "mixolydian",
        "Mixolydian",
        &[0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 10.0],
    ),
    scale(
        "major_pentatonic",
        "Major pentatonic",
        &[0.0, 2.0, 4.0, 7.0, 9.0],
    ),
    scale(
        "minor_pentatonic",
        "Minor pentatonic",
        &[0.0, 3.0, 5.0, 7.0, 10.0],
    ),
    scale("blues", "Blues", &[0.0, 3.0, 5.0, 6.0, 7.0, 10.0]),
    scale("whole_tone", "Whole tone", &[0.0, 2.0, 4.0, 6.0, 8.0, 10.0]),
    scale(
        "chromatic",
        "Chromatic",
        &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0],
    ),
    scale(
        "quarter_tone",
        "Quarter-tone chromatic",
        &[
            0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0, 6.5, 7.0, 7.5, 8.0,
            8.5, 9.0, 9.5, 10.0, 10.5, 11.0, 11.5,
        ],
    ),
    scale(
        "rast",
        "Maqam Rast (quarter tones)",
        &[0.0, 2.0, 3.5, 5.0, 7.0, 9.0, 10.5],
    ),
    scale(
        "bayati",
        "Maqam Bayati (quarter tones)",
        &[0.0, 1.5, 3.0, 5.0, 7.0, 8.0, 10.0],
    ),
];

pub const DEFAULT_MODE: &str = "speech";
pub const DEFAULT_NUMBER_STYLE: &str = "ordinal";
pub const DEFAULT_ROOT: &str = "C";
pub const DEFAULT_OCTAVE: i64 = 4;
pub const DEFAULT_SCALE: &str = "major";
pub const DEFAULT_LEVEL_DB: f64 = -3.0;

// Envelope. The attack is only long enough to keep the onset from clicking; the hold
// is what lets the ear lock onto a pitch (below ~30 ms of steady tone a note reads as
// a click with a colour, not a note); the cubic tail keeps it short in practice -- it
// is 18 dB down halfway through -- while never cutting off audibly.
pub const TONE_ATTACK_MS: f64 = 3.0;
pub const TONE_HOLD_MS: f64 = 8.5;
pub const TONE_DECAY_MS: f64 = 32.0;
/// Harmonic partials (multiple, relative amplitude). A sine at a few hundred hertz
/// disappears under engine noise; the upper partials put energy where the ear is most
/// sensitive, so the note stands out without needing to be loud.
pub const TONE_PARTIALS: [(i64, f64); 4] = [(1, 1.0), (2, 0.5), (3, 0.25), (4, 0.12)];
/// Sport gears: same pitch, brighter. Drive and sport 1 share a degree, so shifting
/// D -> S would otherwise be a repeat of the note just heard; a richer top end marks
/// the mode change without moving the pitch.
pub const TONE_PARTIALS_SPORT: [(i64, f64); 6] = [
    (1, 1.0),
    (2, 0.7),
    (3, 0.5),
    (4, 0.35),
    (5, 0.25),
    (6, 0.18),
];
pub const TONE_MIN_HZ: f64 = 40.0;
pub const TONE_MAX_HZ: f64 = 8000.0;

/// Turn the old announce_gear boolean into gear_report_mode. True if it changed.
pub fn migrate_config(cfg: &mut PyDict) -> bool {
    let Some(old) = cfg.shift_remove("announce_gear") else {
        return false;
    };
    if !cfg.contains_key("gear_report_mode") {
        let mode = if old.truthy() { "speech" } else { "off" };
        cfg.insert("gear_report_mode".to_string(), PyValue::from(mode));
    }
    true
}

/// `str(value or "").strip().lower()`, kept if allowed.
fn choice(value: Option<&PyValue>, allowed: &[&str], fallback: &str) -> String {
    let text = match value {
        Some(v) if v.truthy() => v.py_str(),
        _ => String::new(),
    };
    let v = py_strip(&text).to_lowercase();
    if allowed.contains(&v.as_str()) {
        v
    } else {
        fallback.to_string()
    }
}

/// The validated gear-report settings (Python returns them as a dict).
#[derive(Debug, Clone, PartialEq)]
pub struct GearSettings {
    pub mode: String,
    pub collapse_sport: bool,
    pub number_style: String,
    pub root: String,
    pub octave: i64,
    pub scale: String,
    pub level_db: f64,
}

/// The validated gear-report settings, whatever the config file holds.
///
/// `Err(OverflowError)` where Python raises it uncaught: an infinite octave, or a
/// level that is an integer too large for a float. The config loaders treat that as
/// a corrupt file.
pub fn settings_from_config(cfg: &PyDict) -> Result<GearSettings, PyExc> {
    let level = match cfg.get("gear_tone_level_dbfs") {
        None => py_max(-120.0, py_min(0.0, DEFAULT_LEVEL_DB)),
        Some(v) => match v.py_float() {
            Ok(x) => py_max(-120.0, py_min(0.0, x)),
            Err(PyExc::OverflowError) => return Err(PyExc::OverflowError),
            Err(_) => DEFAULT_LEVEL_DB,
        },
    };
    let octave = match cfg.get("gear_tone_octave") {
        None => PyInt::Small(DEFAULT_OCTAVE),
        Some(v) => match v.py_int() {
            Ok(n) => n,
            Err(PyExc::OverflowError) => return Err(PyExc::OverflowError),
            Err(_) => PyInt::Small(DEFAULT_OCTAVE),
        },
    };
    let root_text = match cfg.get("gear_tone_root") {
        None => DEFAULT_ROOT.to_string(),
        Some(v) if v.truthy() => v.py_str(),
        Some(_) => String::new(),
    };
    let root = py_strip(&root_text).to_string();
    let scale_keys: Vec<&str> = SCALES.iter().map(|s| s.key).collect();
    Ok(GearSettings {
        mode: choice(cfg.get("gear_report_mode"), &MODES, DEFAULT_MODE),
        collapse_sport: cfg
            .get("gear_speech_sport_once")
            .is_some_and(PyValue::truthy),
        number_style: choice(
            cfg.get("gear_speech_number_style"),
            &NUMBER_STYLES,
            DEFAULT_NUMBER_STYLE,
        ),
        root: if note_semitone(&root).is_some() {
            root
        } else {
            DEFAULT_ROOT.to_string()
        },
        octave: match octave.to_i64() {
            Some(o) if ROOT_OCTAVES.contains(&o) => o,
            _ => DEFAULT_OCTAVE,
        },
        scale: choice(cfg.get("gear_tone_scale"), &scale_keys, DEFAULT_SCALE),
        level_db: level,
    })
}

// =========================
//  Speech
// =========================

fn forward(n: &PyInt, number_style: &str) -> String {
    if number_style == "number" {
        return n.to_string();
    }
    if let Some(v) = n.to_i64()
        && let Some((_, word)) = ORDINAL_WORDS.iter().find(|(k, _)| *k == v)
    {
        return (*word).to_string();
    }
    format!("{n}th")
}

/// `str.isdigit()` on a gear-string tail. ASCII digits only (see the module report).
fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// `(gear_str or "").strip().upper()`.
fn norm(gear_str: &str) -> String {
    py_strip(gear_str).to_uppercase()
}

/// Everything after the first character.
fn tail(s: &str) -> &str {
    let mut it = s.chars();
    it.next();
    it.as_str()
}

/// OutGauge byte: 0 is reverse, 1 neutral, 2 first gear.
pub fn gear_to_phrase(gear_byte: i64, number_style: &str) -> String {
    if gear_byte == 0 {
        return "reverse".to_string();
    }
    if gear_byte == 1 {
        return "neutral".to_string();
    }
    if gear_byte >= 2 {
        return forward(&PyInt::Small(gear_byte - 1), number_style);
    }
    "unknown".to_string()
}

pub fn extended_gear_to_phrase(gear_str: &str, number_style: &str) -> String {
    let s = norm(gear_str);
    if s.is_empty() {
        return "unknown".to_string();
    }
    match s.as_str() {
        "P" => return "park".to_string(),
        "D" => return "drive".to_string(),
        "R" => return "reverse".to_string(),
        "N" => return "neutral".to_string(),
        "S" => return "sport".to_string(),
        "M" => return "manual".to_string(),
        _ => {}
    }
    if s.chars().count() > 1 && is_digits(tail(&s)) {
        let num_part = tail(&s);
        if s.starts_with('S') {
            return format!("sport {num_part}");
        }
        if s.starts_with('M') {
            return format!("manual {num_part}");
        }
    }
    if is_digits(&s) || s.starts_with('-') {
        let Some(num) = crate::pyval::py_int_parse(&s) else {
            return "unknown".to_string();
        };
        if num == PyInt::Small(-1) {
            return "reverse".to_string();
        }
        if num.is_zero() {
            return "neutral".to_string();
        }
        return forward(&num, number_style);
    }
    "unknown".to_string()
}

/// S, or S followed by a gear number: what a CVT (or an automatic) in sport reports.
pub fn is_sport(gear_str: &str) -> bool {
    let s = norm(gear_str);
    s.starts_with('S') && (s.chars().count() == 1 || is_digits(tail(&s)))
}

/// True for a sport-to-sport step (S1 -> S2) while sport collapse is on.
///
/// Collapse is one setting for both report modes: speech says "sport" once and the
/// tone plays once, on entry, and neither reports the steps the gearbox takes inside
/// it.
pub fn sport_step_silenced(gear_str: &str, prev_gear_str: &str, collapse_sport: bool) -> bool {
    collapse_sport && is_sport(gear_str) && is_sport(prev_gear_str)
}

/// What to say when the extended gear string changes, or None to stay silent.
///
/// With collapse_sport, entering sport says "sport" once and the transmission stepping
/// through S1, S2, S3 inside it says nothing: a CVT does that on its own,
/// continuously, and naming each step is noise about something the driver did not do.
/// `prev_gear_str` is `None` before the first gear is seen.
pub fn extended_change_phrase(
    gear_str: &str,
    prev_gear_str: Option<&str>,
    collapse_sport: bool,
    number_style: &str,
) -> Option<String> {
    if collapse_sport && is_sport(gear_str) {
        return if is_sport(prev_gear_str.unwrap_or("")) {
            None
        } else {
            Some("sport".to_string())
        };
    }
    if norm(gear_str) == "N" {
        return Some("neutral".to_string());
    }
    let phrase = extended_gear_to_phrase(gear_str, number_style);
    if phrase == "unknown" || phrase == "neutral" {
        None
    } else {
        Some(phrase)
    }
}

pub fn outgauge_change_phrase(gear_byte: i64, number_style: &str) -> Option<String> {
    let phrase = gear_to_phrase(gear_byte, number_style);
    if gear_byte == 1 {
        return Some("neutral".to_string());
    }
    if phrase == "unknown" || phrase == "neutral" {
        None
    } else {
        Some(phrase)
    }
}

// =========================
//  Tone
// =========================

/// A gear's tone is a scale DEGREE counted from neutral, which sits on the root: the
/// lever's own order (P, R, N, 1, 2, 3...) laid onto the scale, so the root is "home"
/// and every forward gear is a step further up. Sport and manual gears take their
/// number's degree; D sits where first gear does.
///
/// A degree beyond `i64` saturates (Python keeps the huge int and then overflows in
/// the pitch arithmetic).
pub fn extended_gear_degree(gear_str: &str) -> Option<i64> {
    let s = norm(gear_str);
    match s.as_str() {
        "P" => return Some(-2),
        "R" => return Some(-1),
        "N" => return Some(0),
        "D" | "S" | "M" => return Some(1),
        _ => {}
    }
    if s.chars().count() > 1 && (s.starts_with('S') || s.starts_with('M')) && is_digits(tail(&s)) {
        return Some(saturate(&crate::pyval::py_int_parse(tail(&s))?));
    }
    let num = crate::pyval::py_int_parse(&s)?;
    // A bare "0" is a manual box's neutral, and it is silent here as it is in speech:
    // the game drops a manual into neutral for as long as a shift button is held, so it
    // would sound on every shift. Automatics report "N", which is handled above.
    if num.is_zero() {
        return None;
    }
    if num.cmp_i64(0) == std::cmp::Ordering::Less {
        Some(-1)
    } else {
        Some(saturate(&num))
    }
}

fn saturate(n: &PyInt) -> i64 {
    match n.to_i64() {
        Some(v) => v,
        None if n.cmp_i64(0) == std::cmp::Ordering::Less => i64::MIN,
        None => i64::MAX,
    }
}

pub fn outgauge_gear_degree(gear_byte: i64) -> Option<i64> {
    if gear_byte == 0 {
        return Some(-1);
    }
    if gear_byte >= 1 {
        return Some(gear_byte - 1);
    }
    None
}

/// Semitones above C for a note name such as C, F#, Bb. None if it is not one.
pub fn note_semitone(name: &str) -> Option<i64> {
    let s = py_strip(name);
    let mut chars = s.chars();
    let first = chars.next()?;
    let upper: String = first.to_uppercase().collect();
    let base = match upper.as_str() {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => return None,
    };
    match chars.as_str() {
        "" | "\u{266e}" => Some(base),
        "#" | "\u{266f}" => Some((base + 1).rem_euclid(12)),
        "b" | "\u{266d}" => Some((base - 1).rem_euclid(12)),
        _ => None,
    }
}

pub fn scale_steps(scale: &str) -> &'static [f64] {
    SCALES
        .iter()
        .find(|s| s.key == scale)
        .map(|s| s.steps)
        .unwrap_or(SCALES[0].steps)
}

/// Semitones above the root for a scale degree; negative degrees go below it.
pub fn degree_semitones(degree: i64, scale: &str) -> f64 {
    let steps = scale_steps(scale);
    let n = steps.len() as i64;
    let octave = degree.div_euclid(n);
    let idx = degree.rem_euclid(n) as usize;
    12.0 * octave as f64 + steps[idx]
}

/// Equal-tempered, A4 = 440 Hz; octave numbering is scientific (C4 is middle C).
pub fn root_frequency(root: &str, octave: i64) -> f64 {
    let pc = note_semitone(root).unwrap_or(0);
    let midi = 12 * (octave + 1) + pc;
    440.0 * 2.0f64.powf((midi - 69) as f64 / 12.0)
}

pub fn degree_frequency(degree: i64, root: &str, octave: i64, scale: &str) -> f64 {
    let hz = root_frequency(root, octave) * 2.0f64.powf(degree_semitones(degree, scale) / 12.0);
    py_max(TONE_MIN_HZ, py_min(TONE_MAX_HZ, hz))
}

/// `int(x)` of a positive, finite sample count.
fn trunc_count(x: f64) -> usize {
    if x > 0.0 { x.trunc() as usize } else { 0 }
}

/// The tone's amplitude envelope (float64 in the Python).
pub fn tone_envelope(sr: u32) -> Vec<f64> {
    let srf = sr as f64;
    let attack = trunc_count(srf * TONE_ATTACK_MS / 1000.0).max(1);
    let hold = trunc_count(srf * TONE_HOLD_MS / 1000.0);
    let decay = trunc_count(srf * TONE_DECAY_MS / 1000.0).max(1);
    let mut env = Vec::with_capacity(attack + hold + decay);
    // Half-cosine onset: no corner at the start of the tail, unlike a linear ramp.
    for k in 0..attack {
        env.push(0.5 - 0.5 * (std::f64::consts::PI * k as f64 / attack as f64).cos());
    }
    env.extend(std::iter::repeat_n(1.0, hold));
    for k in 0..decay {
        env.push((1.0 - k as f64 / decay as f64).powf(3.0));
    }
    env
}

/// One gear tone as a mono float32 buffer whose peak is exactly `level_db` dBFS.
///
/// `sport` selects the brighter partial set; the pitch and level are unchanged. The
/// arithmetic is float64 throughout and the result is cast to float32 at the end, as
/// the Python's `astype(np.float32)` does.
pub fn render_tone(freq_hz: f64, level_db: f64, sr: u32, sport: bool) -> Vec<f32> {
    let env = tone_envelope(sr);
    let srf = sr as f64;
    let mut wave = vec![0.0f64; env.len()];
    let partials: &[(i64, f64)] = if sport {
        &TONE_PARTIALS_SPORT
    } else {
        &TONE_PARTIALS
    };
    for &(mult, amp) in partials {
        // Partials above ~Nyquist would alias back down as an unrelated pitch.
        if freq_hz * (mult as f64) < 0.45 * srf {
            let w = 2.0 * std::f64::consts::PI * freq_hz * mult as f64;
            for (n, s) in wave.iter_mut().enumerate() {
                let t = n as f64 / srf;
                *s += amp * (w * t).sin();
            }
        }
    }
    for (s, e) in wave.iter_mut().zip(&env) {
        *s *= e;
    }
    // np.max propagates NaN.
    let mut peak = 0.0f64;
    let mut nan = false;
    for s in &wave {
        let a = s.abs();
        if a.is_nan() {
            nan = true;
        } else if a > peak {
            peak = a;
        }
    }
    if nan {
        peak = f64::NAN;
    }
    if peak > 0.0 {
        let gain = 10.0f64.powf(level_db / 20.0) / peak;
        for s in &mut wave {
            *s *= gain;
        }
    }
    wave.into_iter().map(|s| s as f32).collect()
}
