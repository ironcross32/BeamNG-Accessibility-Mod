//! Helpers every voice shares: the numpy idioms the callback repeats (open `linspace`,
//! direct `np.convolve(..., mode="full")`), the HRTF overlap-add bookkeeping, the two
//! one-shot playback shapes, delayed-pulse queues, and the controller's small math
//! helpers (`_clamp`, `_pan_gains`, `_render_directional_pulse`).
//!
//! dtype rule of thumb (numpy 2, NEP 50): a Python float times an `f32` array stays `f32`;
//! anything involving an HRIR from `get_hrir` is `f64`. Each helper says which it does.

use std::sync::Arc;

use beam_base::pyfmt::py_mod;

use crate::hrtf::HrtfSet;

/// A finished mono buffer (`np.float32` array), cheap to clone and hand between threads.
pub type Wave = Arc<[f32]>;

/// An HRTF overlap-add tail. Python keeps `None` or a float64 array.
pub type Tail = Option<Vec<f64>>;

/// `AudioController._clamp`: `a if v < a else b if v > b else v` (NaN passes through).
#[inline]
pub fn clamp(v: f64, a: f64, b: f64) -> f64 {
    if v < a {
        a
    } else if v > b {
        b
    } else {
        v
    }
}

/// `AudioController._clamp` with its defaults `(0.0, 1.0)`.
#[inline]
pub fn clamp01(v: f64) -> f64 {
    clamp(v, 0.0, 1.0)
}

/// `AudioController._pan_gains`: delegates to the module-level `_equal_power_pan`, so the
/// callback's panning and `render_scan`'s cannot drift apart.
#[inline]
pub fn pan_gains(p: f64) -> (f64, f64) {
    crate::scan::equal_power_pan(p)
}

/// Element `i` of `np.linspace(a, b, n, endpoint=False)` (float64), numpy's own formula:
/// `i * step + a` with `step = (b - a) / n`, or `(i / n) * (b - a) + a` when the step is 0.
#[inline]
pub fn linspace_open_at(a: f64, b: f64, n: usize, i: usize) -> f64 {
    let delta = b - a;
    let step = delta / n as f64;
    if step == 0.0 {
        (i as f64 / n as f64) * delta + a
    } else {
        i as f64 * step + a
    }
}

/// Fill `out` with `np.linspace(a, b, n, endpoint=False)`.
pub fn linspace_open_into(a: f64, b: f64, n: usize, out: &mut Vec<f64>) {
    out.clear();
    out.extend((0..n).map(|i| linspace_open_at(a, b, n, i)));
}

/// `np.convolve(a, b, mode="full")` by direct summation into `out` (resized to
/// `len(a) + len(b) - 1`). numpy sums in a different order, so results agree to rounding,
/// not bit for bit.
pub fn convolve_full_into(a: &[f64], b: &[f64], out: &mut Vec<f64>) {
    out.clear();
    if a.is_empty() || b.is_empty() {
        return;
    }
    out.resize(a.len() + b.len() - 1, 0.0);
    for (i, &x) in a.iter().enumerate() {
        if x == 0.0 {
            continue;
        }
        for (j, &h) in b.iter().enumerate() {
            out[i + j] += x * h;
        }
    }
}

/// `np.convolve(mono_f32, ir, mode="full")` with the mono buffer widened to `f64`.
pub fn convolve_f32_into(a: &[f32], b: &[f64], scratch: &mut Vec<f64>, out: &mut Vec<f64>) {
    scratch.clear();
    scratch.extend(a.iter().map(|&x| x as f64));
    convolve_full_into(scratch, b, out);
}

/// The overlap-add step shared by every continuous HRTF voice:
/// `conv[:ol] += tail[:ol]` with `ol = min(len(tail), len(conv))`.
pub fn add_tail(conv: &mut [f64], tail: &Tail) {
    if let Some(t) = tail {
        let ol = t.len().min(conv.len());
        for k in 0..ol {
            conv[k] += t[k];
        }
    }
}

/// `tail = conv[frames:].copy()`.
pub fn keep_tail(conv: &[f64], frames: usize, tail: &mut Tail) {
    let rest = if frames < conv.len() {
        &conv[frames..]
    } else {
        &[][..]
    };
    match tail {
        Some(t) => {
            t.clear();
            t.extend_from_slice(rest);
        }
        None => *tail = Some(rest.to_vec()),
    }
}

/// `buf[:frames] += conv[:frames].astype(np.float32)` (f32 + f32).
pub fn add_conv_f32(buf: &mut [f32], conv: &[f64], frames: usize) {
    let n = frames.min(conv.len()).min(buf.len());
    for k in 0..n {
        buf[k] += conv[k] as f32;
    }
}

/// The drain used when a continuous HRTF voice falls silent:
/// `ol = min(len(tail), frames); buf[:ol] += tail[:ol].astype(np.float32)`, then both
/// tails `None`. Callers that gate on `len(tail) > 0` get the same result, since an empty
/// tail adds nothing.
pub fn drain_tails(
    buf_l: &mut [f32],
    buf_r: &mut [f32],
    tail_l: &mut Tail,
    tail_r: &mut Tail,
    frames: usize,
) {
    if let Some(t) = tail_l.as_ref() {
        let ol = t.len().min(frames).min(buf_l.len());
        for k in 0..ol {
            buf_l[k] += t[k] as f32;
        }
    }
    if let Some(t) = tail_r.as_ref() {
        let ol = t.len().min(frames).min(buf_r.len());
        for k in 0..ol {
            buf_r[k] += t[k] as f32;
        }
    }
    *tail_l = None;
    *tail_r = None;
}

/// Straight one-shot playback, centred, the shape used by the TC tone, buzzers, chimes,
/// clickspot beep, terrain sweep, centre click, lock chime and the dock earcons:
///
/// ```text
/// start = int(pos); n = min(frames, len - start)
/// if n > 0: buf[:n] += wave[start:start+n]
/// next = pos + frames; pos = next if next < len else -1
/// ```
///
/// `pos` is the Python float cursor; callers check `pos >= 0` first. Several Python sites
/// compute `next` from `int(pos)` rather than `pos`; cursors only ever hold whole numbers,
/// so the two agree.
pub fn play_centred(
    buf_l: &mut [f32],
    buf_r: &mut [f32],
    wave: &[f32],
    pos: &mut f64,
    frames: usize,
) {
    let start = pos.trunc() as i64;
    let len = wave.len() as i64;
    let n = (frames as i64).min(len - start);
    if n > 0 && start >= 0 {
        let s = start as usize;
        for k in 0..n as usize {
            buf_l[k] += wave[s + k];
            buf_r[k] += wave[s + k];
        }
    }
    let next = *pos + frames as f64;
    *pos = if next < len as f64 { next } else { -1.0 };
}

/// Resampled one-shot playback with linear interpolation, the shape used by the placement
/// ping, the four compass/cam clicks, the node beep and the low-speed click:
///
/// ```text
/// indices = pos + arange(frames) * mult; valid = indices < len - 1
/// i = floor(indices[valid]); f = indices[valid] - i
/// buf[valid] += (src[i] + (src[i+1] - src[i]) * f) * gain      # f64, then cast on add
/// next = pos + frames * mult; pos = next if next < len - 1 else -1
/// ```
///
/// `src[i+1] - src[i]` is an `f32` subtraction; the rest is `f64`. A `gain` of exactly 1.0
/// is the Python sites that apply none (the multiply is exact). Negative indices (a
/// negative `mult`, which no caller passes) would wrap in numpy; here they are skipped.
#[allow(clippy::too_many_arguments)]
pub fn play_resampled(
    buf_l: &mut [f32],
    buf_r: &mut [f32],
    src_l: &[f32],
    src_r: &[f32],
    gain_l: f64,
    gain_r: f64,
    pos: &mut f64,
    mult: f64,
    frames: usize,
) {
    let len = src_l.len().min(src_r.len());
    let limit = len as f64 - 1.0;
    for k in 0..frames.min(buf_l.len()) {
        let idx = *pos + k as f64 * mult;
        if !(idx < limit) || idx < 0.0 {
            continue;
        }
        let i = idx.floor() as usize;
        let f = idx - i as f64;
        let l = src_l[i] as f64 + (src_l[i + 1] - src_l[i]) as f64 * f;
        let r = src_r[i] as f64 + (src_r[i + 1] - src_r[i]) as f64 * f;
        buf_l[k] = (buf_l[k] as f64 + l * gain_l) as f32;
        buf_r[k] = (buf_r[k] as f64 + r * gain_r) as f32;
    }
    let next = *pos + frames as f64 * mult;
    *pos = if next < limit { next } else { -1.0 };
}

/// One entry of a delayed-pulse queue: Python's `{"delay", "L", "R", "pos"}` dicts (road
/// chime, road junction, assistant cues, obstacle pulses).
#[derive(Clone, Debug)]
#[allow(non_snake_case)]
pub struct PulseEntry {
    pub delay: i64,
    pub L: Wave,
    pub R: Wave,
    pub pos: usize,
}

/// Mix and advance a delayed-pulse queue for one block, dropping finished entries. The
/// four Python copies of this loop differ only in spelling; each adds
/// `L[pos:pos+n]` at `offset = max(0, delay)` in `f32`, clears the delay once it lands
/// inside the block, and keeps the entry while `pos < len(L)`.
pub fn mix_pulse_queue(
    queue: &mut Vec<PulseEntry>,
    buf_l: &mut [f32],
    buf_r: &mut [f32],
    frames: usize,
) {
    let frames_i = frames as i64;
    queue.retain_mut(|entry| {
        if entry.delay >= frames_i {
            entry.delay -= frames_i;
            return true;
        }
        let offset = entry.delay.max(0) as usize;
        entry.delay = 0;
        let len = entry.L.len();
        let n = (frames - offset).min(len.saturating_sub(entry.pos));
        for k in 0..n {
            buf_l[offset + k] += entry.L[entry.pos + k];
            buf_r[offset + k] += entry.R[entry.pos + k];
        }
        entry.pos += n;
        entry.pos < len
    });
}

/// `AudioController._render_directional_pulse`: HRTF-convolve (or stereo-pan as fallback)
/// a mono pulse at the given bearing. Free function because Python calls it both under the
/// lock (`trigger_assistant_cue`) and from the callback.
pub fn render_directional_pulse(
    hrtf: Option<&HrtfSet>,
    hrtf_user_enabled: bool,
    base_pulse: &[f32],
    bearing_deg: f64,
) -> (Wave, Wave) {
    if let (Some(h), true) = (hrtf, hrtf_user_enabled) {
        if let Some((ir_l, ir_r)) = h.get_hrir(py_mod(bearing_deg, 360.0)) {
            let mut scratch = Vec::new();
            let mut conv = Vec::new();
            convolve_f32_into(base_pulse, &ir_l, &mut scratch, &mut conv);
            let l: Wave = conv.iter().map(|&x| x as f32).collect();
            convolve_full_into(&scratch, &ir_r, &mut conv);
            let r: Wave = conv.iter().map(|&x| x as f32).collect();
            return (l, r);
        }
    }
    let pan_pos = -bearing_deg / 90.0;
    let (lg, rg) = pan_gains(pan_pos);
    // f32 array times a Python float: the gain is rounded to f32 and multiplied in f32.
    let (lg, rg) = (lg as f32, rg as f32);
    let l: Wave = base_pulse.iter().map(|&x| x * lg).collect();
    let r: Wave = base_pulse.iter().map(|&x| x * rg).collect();
    (l, r)
}

/// `10.0 ** (db / 20.0)`.
#[inline]
pub fn db_to_amp(db: f64) -> f64 {
    10.0f64.powf(db / 20.0)
}
