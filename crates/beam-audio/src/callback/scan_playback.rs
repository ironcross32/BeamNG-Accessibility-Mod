//! Pre-rendered terrain scan playback and crossfade (audio.py:6979-7016). Not ducked. A2.
//!
//! Nothing is synthesised here: the whole scan arrived finished from the listener thread
//! (`render_scan`), because a six-hundred-grain cloud per block would blow the callback's
//! budget. The cursor is clamped to the buffer, as the Python does because `play_scan`
//! can swap them between its two reads.

use crate::callback::block::Block;
use crate::consts::SCAN_CROSSFADE_MS;
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    if !b.snap._scan_tones_enabled {
        return;
    }
    let frames = b.frames;

    if let Some(sb) = v._scan_buf.clone() {
        let len = sb.len();
        let pos = v._scan_pos.min(len);
        let n = frames.min(len - pos);
        if n > 0 {
            for k in 0..n {
                let [l, r] = sb[pos + k];
                b.buf_l[k] += l;
                b.buf_r[k] += r;
            }
            v._scan_pos = pos + frames;
        } else {
            v._scan_buf = None;
            v._scan_pos = 0;
        }
    }

    if let Some(ob) = v._scan_old_buf.clone() {
        if v._scan_old_left > 0 {
            let len = ob.len();
            let opos = v._scan_old_pos.min(len);
            let n = (frames as i64)
                .min((len - opos) as i64)
                .min(v._scan_old_left);
            if n > 0 {
                let n = n as usize;
                let total = 1i64.max((b.sr * SCAN_CROSSFADE_MS / 1000.0).trunc() as i64);
                let done = total - v._scan_old_left;
                let g = linspace_f32(
                    1.0 - done as f64 / total as f64,
                    1.0 - (done + n as i64) as f64 / total as f64,
                    n,
                );
                for k in 0..n {
                    let [l, r] = ob[opos + k];
                    b.buf_l[k] += l * g[k];
                    b.buf_r[k] += r * g[k];
                }
                v._scan_old_pos = opos + n;
                v._scan_old_left -= n as i64;
            }
            if n <= 0 || v._scan_old_left <= 0 {
                v._scan_old_buf = None;
                v._scan_old_left = 0;
            }
        }
    }
}

/// `np.linspace(start, stop, num, dtype=np.float32)` (endpoint included): computed in
/// float64 as `i * step + start` with the last sample set to `stop`, then rounded.
fn linspace_f32(start: f64, stop: f64, num: usize) -> Vec<f32> {
    let delta = stop - start;
    let div = num as f64 - 1.0;
    let mut y: Vec<f64> = if num > 1 {
        let step = delta / div;
        if step == 0.0 {
            (0..num).map(|i| (i as f64 / div) * delta + start).collect()
        } else {
            (0..num).map(|i| i as f64 * step + start).collect()
        }
    } else {
        (0..num).map(|i| i as f64 * delta + start).collect()
    };
    if num > 1 {
        y[num - 1] = stop;
    }
    y.into_iter().map(|x| x as f32).collect()
}
