//! numpy idioms used across audio.py and hrtf.py, each matching numpy's semantics (and,
//! where it is cheap to, numpy's exact arithmetic order): `linspace`, `arange`,
//! `rfftfreq`, pairwise `sum`/`mean`, `cumsum`, `convolve` (full), `searchsorted`,
//! `rfft`/`irfft` with numpy's length and normalisation rules, `clip`, dB conversion,
//! the one-pole coefficient, and numpy's `default_rng` (SeedSequence + PCG64 with
//! `integers`, `uniform` and the ziggurat `standard_normal`).
//!
//! Float dtype is the caller's business: these work in `f64` unless a name says `f32`,
//! and an `astype(np.float32)` is [`to_f32`].

use std::cell::RefCell;
use std::sync::Arc;

pub use realfft::num_complex::Complex64;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

// ---------------------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------------------

/// `np.linspace(start, stop, num, endpoint)`, with numpy's arithmetic: `i * step + start`,
/// and the last sample set to exactly `stop` when `endpoint` (and `num > 1`).
pub fn linspace(start: f64, stop: f64, num: usize, endpoint: bool) -> Vec<f64> {
    let div = if endpoint { num.saturating_sub(1) } else { num };
    let delta = stop - start;
    let mut y: Vec<f64> = (0..num).map(|i| i as f64).collect();
    if div > 0 {
        let step = delta / div as f64;
        if step == 0.0 {
            for v in y.iter_mut() {
                *v = (*v / div as f64) * delta;
            }
        } else {
            for v in y.iter_mut() {
                *v *= step;
            }
        }
    } else {
        for v in y.iter_mut() {
            *v *= delta;
        }
    }
    for v in y.iter_mut() {
        *v += start;
    }
    if endpoint && num > 1 {
        y[num - 1] = stop;
    }
    y
}

/// `np.arange(n)` as float64.
pub fn arange(n: usize) -> Vec<f64> {
    (0..n).map(|i| i as f64).collect()
}

/// `np.arange(n) / d` (float64 division per element, as numpy does it).
pub fn arange_div(n: usize, d: f64) -> Vec<f64> {
    (0..n).map(|i| i as f64 / d).collect()
}

/// `np.fft.rfftfreq(n, d)`: `arange(0, n // 2 + 1) * (1.0 / (n * d))`.
pub fn rfftfreq(n: usize, d: f64) -> Vec<f64> {
    let val = 1.0 / (n as f64 * d);
    (0..n / 2 + 1).map(|i| i as f64 * val).collect()
}

/// `np.concatenate` of slices.
pub fn concat(parts: &[&[f64]]) -> Vec<f64> {
    parts.iter().flat_map(|p| p.iter().copied()).collect()
}

/// `.astype(np.float32)`: round to nearest.
pub fn to_f32(x: &[f64]) -> Vec<f32> {
    x.iter().map(|&v| v as f32).collect()
}

// ---------------------------------------------------------------------------------------
// Reductions
// ---------------------------------------------------------------------------------------

/// numpy's `pairwise_sum` for a contiguous float64 array: below 8 a plain loop, up to 128
/// eight interleaved accumulators, above that split at a multiple of 8 and recurse.
fn pairwise_sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        let mut res = 0.0;
        for &v in a {
            res += v;
        }
        res
    } else if n <= 128 {
        let mut r = [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
        let mut i = 8;
        while i < n - (n % 8) {
            for j in 0..8 {
                r[j] += a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise_sum(&a[..n2]) + pairwise_sum(&a[n2..])
    }
}

/// `np.sum(x)` for a 1-D float64 array, bit for bit (the reduction starts from 0.0).
pub fn sum(x: &[f64]) -> f64 {
    0.0 + pairwise_sum(x)
}

/// `np.mean(x)`: `sum / n`; NaN for an empty slice, as numpy (with a warning).
pub fn mean(x: &[f64]) -> f64 {
    sum(x) / x.len() as f64
}

/// `np.mean(x * x)`.
pub fn mean_square(x: &[f64]) -> f64 {
    let sq: Vec<f64> = x.iter().map(|v| v * v).collect();
    mean(&sq)
}

/// `float(np.sqrt(np.mean(x * x)))`.
pub fn rms(x: &[f64]) -> f64 {
    mean_square(x).sqrt()
}

/// `float(np.max(np.abs(x)))`. NaN propagates as numpy's `max` does. Panics on empty
/// input, where numpy raises.
pub fn max_abs(x: &[f64]) -> f64 {
    assert!(
        !x.is_empty(),
        "zero-size array to reduction operation maximum"
    );
    let mut m = x[0].abs();
    for &v in &x[1..] {
        let a = v.abs();
        if a.is_nan() || m.is_nan() {
            m = f64::NAN;
        } else if a > m {
            m = a;
        }
    }
    m
}

/// [`max_abs`] for float32 buffers.
pub fn max_abs_f32(x: &[f32]) -> f32 {
    assert!(
        !x.is_empty(),
        "zero-size array to reduction operation maximum"
    );
    let mut m = x[0].abs();
    for &v in &x[1..] {
        let a = v.abs();
        if a.is_nan() || m.is_nan() {
            m = f32::NAN;
        } else if a > m {
            m = a;
        }
    }
    m
}

/// `np.max(x)` (first-maximum semantics are irrelevant for a value).
pub fn max(x: &[f64]) -> f64 {
    assert!(
        !x.is_empty(),
        "zero-size array to reduction operation maximum"
    );
    let mut m = x[0];
    for &v in &x[1..] {
        if v.is_nan() || m.is_nan() {
            m = f64::NAN;
        } else if v > m {
            m = v;
        }
    }
    m
}

/// `np.argmax(x)`: the first index of the maximum (the first NaN, if any).
pub fn argmax(x: &[f64]) -> usize {
    let mut best = 0;
    for i in 1..x.len() {
        if x[best].is_nan() {
            return best;
        }
        if x[i].is_nan() || x[i] > x[best] {
            best = i;
        }
    }
    best
}

/// `np.cumsum(x)`: a plain running sum, which is exactly what numpy does.
pub fn cumsum(x: &[f64]) -> Vec<f64> {
    let mut acc = 0.0;
    x.iter()
        .map(|&v| {
            acc += v;
            acc
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Elementwise helpers
// ---------------------------------------------------------------------------------------

/// `np.clip(x, lo, hi)` for one value: `min(max(x, lo), hi)` with NaN passing through.
#[inline]
pub fn clip(x: f64, lo: f64, hi: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let y = if x < lo { lo } else { x };
    if y > hi { hi } else { y }
}

/// `10.0 ** (db / 20.0)`.
#[inline]
pub fn db_to_lin(db: f64) -> f64 {
    10.0f64.powf(db / 20.0)
}

/// `20.0 * log10(x)`.
#[inline]
pub fn lin_to_db(x: f64) -> f64 {
    20.0 * x.log10()
}

/// The one-pole smoothing coefficient audio.py writes as `1.0 - math.exp(-dt / tau)`;
/// the smoother is then `y += alpha * (target - y)`.
#[inline]
pub fn one_pole_alpha(dt: f64, tau: f64) -> f64 {
    1.0 - (-dt / tau).exp()
}

/// `0.5 - 0.5 * np.cos(np.linspace(0.0, pi, n))`: the raised-cosine rise used for fades
/// throughout audio.py.
pub fn raised_cosine_rise(n: usize) -> Vec<f64> {
    linspace(0.0, std::f64::consts::PI, n, true)
        .into_iter()
        .map(|x| 0.5 - 0.5 * x.cos())
        .collect()
}

/// `np.searchsorted(a, v, side="left")` on a sorted slice.
pub fn searchsorted_left(a: &[f64], v: f64) -> usize {
    a.partition_point(|&x| x < v)
}

/// `np.searchsorted(a, v, side="right")` on a sorted slice.
pub fn searchsorted_right(a: &[f64], v: f64) -> usize {
    a.partition_point(|&x| x <= v)
}

/// `np.convolve(a, v, mode="full")` (the only mode audio.py uses), float64.
///
/// numpy computes each output as a dot product, which its BLAS may reorder, so the
/// result agrees with numpy to rounding rather than bit for bit.
pub fn convolve_full(a: &[f64], v: &[f64]) -> Vec<f64> {
    if a.is_empty() || v.is_empty() {
        return Vec::new();
    }
    let n = a.len() + v.len() - 1;
    let mut out = vec![0.0f64; n];
    for (i, &x) in a.iter().enumerate() {
        if x == 0.0 {
            continue;
        }
        for (j, &h) in v.iter().enumerate() {
            out[i + j] += x * h;
        }
    }
    out
}

/// `np.convolve` of two float32 arrays: numpy keeps float32 throughout, so this
/// accumulates in f32.
pub fn convolve_full_f32(a: &[f32], v: &[f32]) -> Vec<f32> {
    if a.is_empty() || v.is_empty() {
        return Vec::new();
    }
    let n = a.len() + v.len() - 1;
    let mut out = vec![0.0f32; n];
    for (i, &x) in a.iter().enumerate() {
        if x == 0.0 {
            continue;
        }
        for (j, &h) in v.iter().enumerate() {
            out[i + j] += x * h;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// FFT (numpy.fft semantics over realfft)
// ---------------------------------------------------------------------------------------

struct Planners {
    planner: RealFftPlanner<f64>,
}

thread_local! {
    static PLANNERS: RefCell<Planners> = RefCell::new(Planners { planner: RealFftPlanner::new() });
}

fn forward_plan(n: usize) -> Arc<dyn RealToComplex<f64>> {
    PLANNERS.with(|p| p.borrow_mut().planner.plan_fft_forward(n))
}

fn inverse_plan(n: usize) -> Arc<dyn ComplexToReal<f64>> {
    PLANNERS.with(|p| p.borrow_mut().planner.plan_fft_inverse(n))
}

/// `np.fft.rfft(x, n)`: `x` truncated or zero-padded to `n`, unnormalised, `n // 2 + 1`
/// bins.
pub fn rfft(x: &[f64], n: usize) -> Vec<Complex64> {
    assert!(n >= 1, "Invalid number of FFT data points");
    let plan = forward_plan(n);
    let mut input = vec![0.0f64; n];
    let m = x.len().min(n);
    input[..m].copy_from_slice(&x[..m]);
    let mut out = plan.make_output_vec();
    let mut scratch = plan.make_scratch_vec();
    plan.process_with_scratch(&mut input, &mut out, &mut scratch)
        .expect("rfft buffer sizes are fixed by the plan");
    out
}

/// `np.fft.irfft(spec, n)`: the first `n // 2 + 1` bins (zero-padded if fewer), the
/// imaginary part of DC (and of Nyquist for even `n`) ignored, scaled by `1 / n`.
pub fn irfft(spec: &[Complex64], n: usize) -> Vec<f64> {
    assert!(n >= 1, "Invalid number of FFT data points");
    let plan = inverse_plan(n);
    let bins = n / 2 + 1;
    let mut input = vec![Complex64::new(0.0, 0.0); bins];
    let m = spec.len().min(bins);
    input[..m].copy_from_slice(&spec[..m]);
    input[0].im = 0.0;
    if n % 2 == 0 {
        input[bins - 1].im = 0.0;
    }
    let mut out = plan.make_output_vec();
    let mut scratch = plan.make_scratch_vec();
    plan.process_with_scratch(&mut input, &mut out, &mut scratch)
        .expect("irfft imaginary parts were zeroed");
    let fct = 1.0 / n as f64;
    for v in out.iter_mut() {
        *v *= fct;
    }
    out
}

thread_local! {
    static PLANNER_F32: RefCell<RealFftPlanner<f32>> = RefCell::new(RealFftPlanner::new());
}

/// `np.fft.rfft` of a float32 array. numpy 2.x transforms float32 input in single
/// precision (complex64 out), so this does too.
pub fn rfft_f32(x: &[f32], n: usize) -> Vec<realfft::num_complex::Complex32> {
    assert!(n >= 1, "Invalid number of FFT data points");
    let plan = PLANNER_F32.with(|p| p.borrow_mut().plan_fft_forward(n));
    let mut input = vec![0.0f32; n];
    let m = x.len().min(n);
    input[..m].copy_from_slice(&x[..m]);
    let mut out = plan.make_output_vec();
    let mut scratch = plan.make_scratch_vec();
    plan.process_with_scratch(&mut input, &mut out, &mut scratch)
        .expect("rfft buffer sizes are fixed by the plan");
    out
}

/// `np.fft.irfft` of a complex64 spectrum: single precision, scaled by `1 / n`.
pub fn irfft_f32(spec: &[realfft::num_complex::Complex32], n: usize) -> Vec<f32> {
    assert!(n >= 1, "Invalid number of FFT data points");
    let plan = PLANNER_F32.with(|p| p.borrow_mut().plan_fft_inverse(n));
    let bins = n / 2 + 1;
    let mut input = vec![realfft::num_complex::Complex32::new(0.0, 0.0); bins];
    let m = spec.len().min(bins);
    input[..m].copy_from_slice(&spec[..m]);
    input[0].im = 0.0;
    if n % 2 == 0 {
        input[bins - 1].im = 0.0;
    }
    let mut out = plan.make_output_vec();
    let mut scratch = plan.make_scratch_vec();
    plan.process_with_scratch(&mut input, &mut out, &mut scratch)
        .expect("irfft imaginary parts were zeroed");
    let fct = (1.0 / n as f64) as f32;
    for v in out.iter_mut() {
        *v *= fct;
    }
    out
}

// ---------------------------------------------------------------------------------------
// numpy.random.default_rng: SeedSequence + PCG64
// ---------------------------------------------------------------------------------------

const INIT_A: u32 = 0x43b0_d7e5;
const MULT_A: u32 = 0x931e_8875;
const INIT_B: u32 = 0x8b51_f9dd;
const MULT_B: u32 = 0x58f3_8ded;
const MIX_MULT_L: u32 = 0xca01_f9dd;
const MIX_MULT_R: u32 = 0x4973_f715;
const XSHIFT: u32 = 16;
const POOL_SIZE: usize = 4;

/// numpy's `SeedSequence(entropy)` for a non-negative integer seed with no spawn key.
pub struct SeedSequence {
    pool: [u32; POOL_SIZE],
}

impl SeedSequence {
    pub fn new(entropy: u128) -> Self {
        // _coerce_to_uint32_array: little-endian 32-bit words, [0] for zero.
        let mut words = Vec::new();
        let mut e = entropy;
        loop {
            words.push(e as u32);
            e >>= 32;
            if e == 0 {
                break;
            }
        }
        let mut hash_const = INIT_A;
        let mut hashmix = |value: u32| -> u32 {
            let mut v = value ^ hash_const;
            hash_const = hash_const.wrapping_mul(MULT_A);
            v = v.wrapping_mul(hash_const);
            v ^ (v >> XSHIFT)
        };
        let mix = |x: u32, y: u32| -> u32 {
            let r = MIX_MULT_L
                .wrapping_mul(x)
                .wrapping_sub(MIX_MULT_R.wrapping_mul(y));
            r ^ (r >> XSHIFT)
        };
        let mut pool = [0u32; POOL_SIZE];
        for (i, slot) in pool.iter_mut().enumerate() {
            *slot = hashmix(words.get(i).copied().unwrap_or(0));
        }
        for i_src in 0..POOL_SIZE {
            for i_dst in 0..POOL_SIZE {
                if i_src != i_dst {
                    let h = hashmix(pool[i_src]);
                    pool[i_dst] = mix(pool[i_dst], h);
                }
            }
        }
        for &w in words.iter().skip(POOL_SIZE) {
            for i_dst in 0..POOL_SIZE {
                let h = hashmix(w);
                pool[i_dst] = mix(pool[i_dst], h);
            }
        }
        SeedSequence { pool }
    }

    /// `generate_state(n_words, np.uint32)`.
    pub fn generate_state_u32(&self, n_words: usize) -> Vec<u32> {
        let mut hash_const = INIT_B;
        (0..n_words)
            .map(|i| {
                let mut v = self.pool[i % POOL_SIZE] ^ hash_const;
                hash_const = hash_const.wrapping_mul(MULT_B);
                v = v.wrapping_mul(hash_const);
                v ^ (v >> XSHIFT)
            })
            .collect()
    }

    /// `generate_state(n_words, np.uint64)`: pairs of u32 words, low word first.
    pub fn generate_state_u64(&self, n_words: usize) -> Vec<u64> {
        self.generate_state_u32(n_words * 2)
            .chunks(2)
            .map(|c| (c[0] as u64) | ((c[1] as u64) << 32))
            .collect()
    }
}

const PCG_MULT: u128 = (2549297995355413924u128 << 64) | 4865540595714422341u128;

/// numpy's `PCG64` bit generator (XSL-RR output), with its buffered 32-bit draw.
pub struct Pcg64 {
    state: u128,
    inc: u128,
    has_uint32: bool,
    uinteger: u32,
}

/// `np.random.default_rng(seed)`.
pub fn default_rng(seed: u128) -> Pcg64 {
    let s = SeedSequence::new(seed).generate_state_u64(4);
    let initstate = ((s[0] as u128) << 64) | s[1] as u128;
    let initseq = ((s[2] as u128) << 64) | s[3] as u128;
    let mut g = Pcg64 {
        state: 0,
        inc: (initseq << 1) | 1,
        has_uint32: false,
        uinteger: 0,
    };
    g.step();
    g.state = g.state.wrapping_add(initstate);
    g.step();
    g
}

impl Pcg64 {
    #[inline]
    fn step(&mut self) {
        self.state = self.state.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
    }

    pub fn next_u64(&mut self) -> u64 {
        self.step();
        let s = self.state;
        let x = ((s >> 64) as u64) ^ (s as u64);
        x.rotate_right((s >> 122) as u32)
    }

    pub fn next_u32(&mut self) -> u32 {
        if self.has_uint32 {
            self.has_uint32 = false;
            return self.uinteger;
        }
        let next = self.next_u64();
        self.has_uint32 = true;
        self.uinteger = (next >> 32) as u32;
        next as u32
    }

    /// `next_double`: 53 random bits in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9007199254740992.0)
    }

    /// `Generator.uniform(low, high)` for one value: `low + (high - low) * next_double`.
    pub fn uniform(&mut self, low: f64, high: f64) -> f64 {
        let range = high - low;
        low + range * self.next_f64()
    }

    /// `Generator.uniform(low, high, n)`.
    pub fn uniform_n(&mut self, low: f64, high: f64, n: usize) -> Vec<f64> {
        (0..n).map(|_| self.uniform(low, high)).collect()
    }

    /// `Generator.integers(low, high)` (high exclusive, int64) for one value.
    pub fn integers(&mut self, low: i64, high: i64) -> i64 {
        assert!(high > low, "low >= high");
        let rng = (high - 1).wrapping_sub(low) as u64;
        if rng == 0 {
            return low;
        }
        let v: u64 = if rng <= 0xFFFF_FFFF {
            if rng == 0xFFFF_FFFF {
                self.next_u32() as u64
            } else {
                self.bounded_lemire_u32(rng as u32) as u64
            }
        } else if rng == u64::MAX {
            self.next_u64()
        } else {
            self.bounded_lemire_u64(rng)
        };
        low.wrapping_add(v as i64)
    }

    fn bounded_lemire_u32(&mut self, rng: u32) -> u32 {
        let rng_excl = rng + 1;
        let mut m = self.next_u32() as u64 * rng_excl as u64;
        let mut leftover = m as u32;
        if leftover < rng_excl {
            let threshold = (u32::MAX - rng) % rng_excl;
            while leftover < threshold {
                m = self.next_u32() as u64 * rng_excl as u64;
                leftover = m as u32;
            }
        }
        (m >> 32) as u32
    }

    fn bounded_lemire_u64(&mut self, rng: u64) -> u64 {
        let rng_excl = rng + 1;
        let mut m = self.next_u64() as u128 * rng_excl as u128;
        let mut leftover = m as u64;
        if leftover < rng_excl {
            let threshold = (u64::MAX - rng) % rng_excl;
            while leftover < threshold {
                m = self.next_u64() as u128 * rng_excl as u128;
                leftover = m as u64;
            }
        }
        (m >> 64) as u64
    }

    /// `Generator.standard_normal()`: numpy's 256-layer ziggurat, with its own tables.
    pub fn standard_normal(&mut self) -> f64 {
        const ZIGGURAT_NOR_R: f64 = 3.6541528853610087963519472518;
        const ZIGGURAT_NOR_INV_R: f64 = 0.27366123732975827203338247596;
        loop {
            let mut r = self.next_u64();
            let idx = (r & 0xff) as usize;
            r >>= 8;
            let sign = r & 0x1;
            let rabs = (r >> 1) & 0x000f_ffff_ffff_ffff;
            let mut x = rabs as f64 * ZIG_WI[idx];
            if sign & 0x1 != 0 {
                x = -x;
            }
            if rabs < ZIG_KI[idx] {
                return x;
            }
            if idx == 0 {
                loop {
                    let xx = -ZIGGURAT_NOR_INV_R * (-self.next_f64()).ln_1p();
                    let yy = -(-self.next_f64()).ln_1p();
                    if yy + yy > xx * xx {
                        return if (rabs >> 8) & 0x1 != 0 {
                            -(ZIGGURAT_NOR_R + xx)
                        } else {
                            ZIGGURAT_NOR_R + xx
                        };
                    }
                }
            } else if (ZIG_FI[idx - 1] - ZIG_FI[idx]) * self.next_f64() + ZIG_FI[idx]
                < (-0.5 * x * x).exp()
            {
                return x;
            }
        }
    }

    /// `Generator.standard_normal(n)`.
    pub fn standard_normal_n(&mut self, n: usize) -> Vec<f64> {
        (0..n).map(|_| self.standard_normal()).collect()
    }
}

// numpy's ziggurat tables (ziggurat_constants.h: ki_double, wi_double, fi_double), read
// out of numpy 2.3.3's compiled _generator module; standard_normal is fixture-checked
// against numpy's own output.
#[rustfmt::skip]
const ZIG_KI: [u64; 256] = [
    0x000EF33D8025EF6A,
    0x0000000000000000,
    0x000C08BE98FBC6A8,
    0x000DA354FABD8142,
    0x000E51F67EC1EEEA,
    0x000EB255E9D3F77E,
    0x000EEF4B817ECAB9,
    0x000F19470AFA44AA,
    0x000F37ED61FFCB18,
    0x000F4F469561255C,
    0x000F61A5E41BA396,
    0x000F707A755396A4,
    0x000F7CB2EC28449A,
    0x000F86F10C6357D3,
    0x000F8FA6578325DE,
    0x000F9724C74DD0DA,
    0x000F9DA907DBF509,
    0x000FA360F581FA74,
    0x000FA86FDE5B4BF8,
    0x000FACF160D354DC,
    0x000FB0FB6718B90F,
    0x000FB49F8D5374C6,
    0x000FB7EC2366FE77,
    0x000FBAECE9A1E50E,
    0x000FBDAB9D040BED,
    0x000FC03060FF6C57,
    0x000FC2821037A248,
    0x000FC4A67AE25BD1,
    0x000FC6A2977AEE31,
    0x000FC87AA92896A4,
    0x000FCA325E4BDE85,
    0x000FCBCCE902231A,
    0x000FCD4D12F839C4,
    0x000FCEB54D8FEC99,
    0x000FD007BF1DC930,
    0x000FD1464DD6C4E6,
    0x000FD272A8E2F450,
    0x000FD38E4FF0C91E,
    0x000FD49A9990B478,
    0x000FD598B8920F53,
    0x000FD689C08E99EC,
    0x000FD76EA9C8E832,
    0x000FD848547B08E8,
    0x000FD9178BAD2C8C,
    0x000FD9DD07A7ADD2,
    0x000FDA9970105E8C,
    0x000FDB4D5DC02E20,
    0x000FDBF95C5BFCD0,
    0x000FDC9DEBB99A7D,
    0x000FDD3B8118729D,
    0x000FDDD288342F90,
    0x000FDE6364369F64,
    0x000FDEEE708D514E,
    0x000FDF7401A6B42E,
    0x000FDFF46599ED40,
    0x000FE06FE4BC24F2,
    0x000FE0E6C225A258,
    0x000FE1593C28B84C,
    0x000FE1C78CBC3F99,
    0x000FE231E9DB1CAA,
    0x000FE29885DA1B91,
    0x000FE2FB8FB54186,
    0x000FE35B33558D4A,
    0x000FE3B799D0002A,
    0x000FE410E99EAD7F,
    0x000FE46746D47734,
    0x000FE4BAD34C095C,
    0x000FE50BAED29524,
    0x000FE559F74EBC78,
    0x000FE5A5C8E41212,
    0x000FE5EF3E138689,
    0x000FE6366FD91078,
    0x000FE67B75C6D578,
    0x000FE6BE661E11AA,
    0x000FE6FF55E5F4F2,
    0x000FE73E5900A702,
    0x000FE77B823E9E39,
    0x000FE7B6E37070A2,
    0x000FE7F08D774243,
    0x000FE8289053F08C,
    0x000FE85EFB35173A,
    0x000FE893DC840864,
    0x000FE8C741F0CEBC,
    0x000FE8F9387D4EF6,
    0x000FE929CC879B1D,
    0x000FE95909D388EA,
    0x000FE986FB939AA2,
    0x000FE9B3AC714866,
    0x000FE9DF2694B6D5,
    0x000FEA0973ABE67C,
    0x000FEA329CF166A4,
    0x000FEA5AAB32952C,
    0x000FEA81A6D5741A,
    0x000FEAA797DE1CF0,
    0x000FEACC85F3D920,
    0x000FEAF07865E63C,
    0x000FEB13762FEC13,
    0x000FEB3585FE2A4A,
    0x000FEB56AE3162B4,
    0x000FEB76F4E284FA,
    0x000FEB965FE62014,
    0x000FEBB4F4CF9D7C,
    0x000FEBD2B8F449D0,
    0x000FEBEFB16E2E3E,
    0x000FEC0BE31EBDE8,
    0x000FEC2752B15A15,
    0x000FEC42049DAFD3,
    0x000FEC5BFD29F196,
    0x000FEC75406CEEF4,
    0x000FEC8DD2500CB4,
    0x000FECA5B6911F12,
    0x000FECBCF0C427FE,
    0x000FECD38454FB15,
    0x000FECE97488C8B3,
    0x000FECFEC47F91B7,
    0x000FED1377358528,
    0x000FED278F844903,
    0x000FED3B10242F4C,
    0x000FED4DFBAD586E,
    0x000FED605498C3DD,
    0x000FED721D414FE8,
    0x000FED8357E4A982,
    0x000FED9406A42CC8,
    0x000FEDA42B85B704,
    0x000FEDB3C8746AB4,
    0x000FEDC2DF416652,
    0x000FEDD171A46E52,
    0x000FEDDF813C8AD3,
    0x000FEDED0F909980,
    0x000FEDFA1E0FD414,
    0x000FEE06AE124BC4,
    0x000FEE12C0D95A06,
    0x000FEE1E579006E0,
    0x000FEE29734B6524,
    0x000FEE34150AE4BC,
    0x000FEE3E3DB89B3C,
    0x000FEE47EE2982F4,
    0x000FEE51271DB086,
    0x000FEE59E9407F41,
    0x000FEE623528B42E,
    0x000FEE6A0B5897F1,
    0x000FEE716C3E077A,
    0x000FEE7858327B82,
    0x000FEE7ECF7B06BA,
    0x000FEE84D2484AB2,
    0x000FEE8A60B66343,
    0x000FEE8F7ACCC851,
    0x000FEE94207E25DA,
    0x000FEE9851A829EA,
    0x000FEE9C0E13485C,
    0x000FEE9F557273F4,
    0x000FEEA22762CCAE,
    0x000FEEA4836B42AC,
    0x000FEEA668FC2D71,
    0x000FEEA7D76ED6FA,
    0x000FEEA8CE04FA0A,
    0x000FEEA94BE8333B,
    0x000FEEA950296410,
    0x000FEEA8D9C0075E,
    0x000FEEA7E7897654,
    0x000FEEA678481D24,
    0x000FEEA48AA29E83,
    0x000FEEA21D22E4DA,
    0x000FEE9F2E352024,
    0x000FEE9BBC26AF2E,
    0x000FEE97C524F2E4,
    0x000FEE93473C0A3A,
    0x000FEE8E40557516,
    0x000FEE88AE369C7A,
    0x000FEE828E7F3DFD,
    0x000FEE7BDEA7B888,
    0x000FEE749BFF37FF,
    0x000FEE6CC3A9BD5E,
    0x000FEE64529E007E,
    0x000FEE5B45A32888,
    0x000FEE51994E57B6,
    0x000FEE474A0006CF,
    0x000FEE3C53E12C50,
    0x000FEE30B2E02AD8,
    0x000FEE2462AD8205,
    0x000FEE175EB83C5A,
    0x000FEE09A22A1447,
    0x000FEDFB27E349CC,
    0x000FEDEBEA76216C,
    0x000FEDDBE422047E,
    0x000FEDCB0ECE39D3,
    0x000FEDB964042CF4,
    0x000FEDA6DCE938C9,
    0x000FED937237E98D,
    0x000FED7F1C38A836,
    0x000FED69D2B9C02B,
    0x000FED538D06AE00,
    0x000FED3C41DEA422,
    0x000FED23E76A2FD8,
    0x000FED0A732FE644,
    0x000FECEFDA07FE34,
    0x000FECD4100EB7B8,
    0x000FECB708956EB4,
    0x000FEC98B61230C1,
    0x000FEC790A0DA978,
    0x000FEC57F50F31FE,
    0x000FEC356686C962,
    0x000FEC114CB4B335,
    0x000FEBEB948E6FD0,
    0x000FEBC429A0B692,
    0x000FEB9AF5EE0CDC,
    0x000FEB6FE1C98542,
    0x000FEB42D3AD1F9E,
    0x000FEB13B00B2D4B,
    0x000FEAE2591A02E9,
    0x000FEAAEAE992257,
    0x000FEA788D8EE326,
    0x000FEA3FCFFD73E5,
    0x000FEA044C8DD9F6,
    0x000FE9C5D62F563B,
    0x000FE9843BA947A4,
    0x000FE93F471D4728,
    0x000FE8F6BD76C5D6,
    0x000FE8AA5DC4E8E6,
    0x000FE859E07AB1EA,
    0x000FE804F690A940,
    0x000FE7AB488233C0,
    0x000FE74C751F6AA5,
    0x000FE6E8102AA202,
    0x000FE67DA0B6ABD8,
    0x000FE60C9F38307E,
    0x000FE5947338F742,
    0x000FE51470977280,
    0x000FE48BD436F458,
    0x000FE3F9BFFD1E37,
    0x000FE35D35EEB19C,
    0x000FE2B5122FE4FE,
    0x000FE20003995557,
    0x000FE13C82788314,
    0x000FE068C4EE67B0,
    0x000FDF82B02B71AA,
    0x000FDE87C57EFEAA,
    0x000FDD7509C63BFD,
    0x000FDC46E529BF13,
    0x000FDAF8F82E0282,
    0x000FD985E1B2BA75,
    0x000FD7E6EF48CF04,
    0x000FD613ADBD650B,
    0x000FD40149E2F012,
    0x000FD1A1A7B4C7AC,
    0x000FCEE204761F9E,
    0x000FCBA8D85E11B2,
    0x000FC7D26ECD2D22,
    0x000FC32B2F1E22ED,
    0x000FBD6581C0B83A,
    0x000FB606C4005434,
    0x000FAC40582A2874,
    0x000F9E971E014598,
    0x000F89FA48A41DFC,
    0x000F66C5F7F0302C,
    0x000F1A5A4B331C4A,
];
#[rustfmt::skip]
const ZIG_WI: [f64; 256] = [
    f64::from_bits(0x3CCF493B7815D979),
    f64::from_bits(0x3C8B8D0BE3FDF6C6),
    f64::from_bits(0x3C9250AF3C2C5BB4),
    f64::from_bits(0x3C957CB938443B61),
    f64::from_bits(0x3C9801FCE82FA70C),
    f64::from_bits(0x3C9A230C2E4CD0BC),
    f64::from_bits(0x3C9C004D2F3861F7),
    f64::from_bits(0x3C9DAC2F5A747274),
    f64::from_bits(0x3C9F32482D4CD5C3),
    f64::from_bits(0x3CA04D32278EBBAD),
    f64::from_bits(0x3CA0F5053B025D43),
    f64::from_bits(0x3CA192A697413677),
    f64::from_bits(0x3CA227A28F7A1AF5),
    f64::from_bits(0x3CA2B52E3863D880),
    f64::from_bits(0x3CA33C3FC05791F5),
    f64::from_bits(0x3CA3BD9EC1A2B12F),
    f64::from_bits(0x3CA439EF8DFF9B55),
    f64::from_bits(0x3CA4B1BB363DFEA7),
    f64::from_bits(0x3CA52575621AD374),
    f64::from_bits(0x3CA59580A707CE96),
    f64::from_bits(0x3CA60231CFD97EEA),
    f64::from_bits(0x3CA66BD261A37C3D),
    f64::from_bits(0x3CA6D2A292000570),
    f64::from_bits(0x3CA736DAD346F8A6),
    f64::from_bits(0x3CA798AD10B32A77),
    f64::from_bits(0x3CA7F845AD46F543),
    f64::from_bits(0x3CA855CC53430A77),
    f64::from_bits(0x3CA8B1649E7B769A),
    f64::from_bits(0x3CA90B2EA94ECF98),
    f64::from_bits(0x3CA96347822C1EEA),
    f64::from_bits(0x3CA9B9C98E38C546),
    f64::from_bits(0x3CAA0ECCDCA4A72C),
    f64::from_bits(0x3CAA62676D77CD59),
    f64::from_bits(0x3CAAB4AD6E101630),
    f64::from_bits(0x3CAB05B16D136C9C),
    f64::from_bits(0x3CAB558487427A29),
    f64::from_bits(0x3CABA4368E529F3A),
    f64::from_bits(0x3CABF1D62ABF8232),
    f64::from_bits(0x3CAC3E70F9594EF3),
    f64::from_bits(0x3CAC8A13A5323B61),
    f64::from_bits(0x3CACD4C9FE72268B),
    f64::from_bits(0x3CAD1E9F0E80B748),
    f64::from_bits(0x3CAD679D29E41F10),
    f64::from_bits(0x3CADAFCE0023B8C3),
    f64::from_bits(0x3CADF73AA9F17653),
    f64::from_bits(0x3CAE3DEBB5D2EDFE),
    f64::from_bits(0x3CAE83E9337A6F00),
    f64::from_bits(0x3CAEC93ABDF982CE),
    f64::from_bits(0x3CAF0DE784F06226),
    f64::from_bits(0x3CAF51F654D8F688),
    f64::from_bits(0x3CAF956D9E87D7AE),
    f64::from_bits(0x3CAFD8537DFA2EAC),
    f64::from_bits(0x3CB00D56E04234EC),
    f64::from_bits(0x3CB02E40F5398F9A),
    f64::from_bits(0x3CB04EEA9E16A5FC),
    f64::from_bits(0x3CB06F565B72A010),
    f64::from_bits(0x3CB08F869071F40B),
    f64::from_bits(0x3CB0AF7D84BC6113),
    f64::from_bits(0x3CB0CF3D664BCC7F),
    f64::from_bits(0x3CB0EEC84B16086B),
    f64::from_bits(0x3CB10E20329515EE),
    f64::from_bits(0x3CB12D4707310FBE),
    f64::from_bits(0x3CB14C3E9F8E9141),
    f64::from_bits(0x3CB16B08BFC4201E),
    f64::from_bits(0x3CB189A71A78DA34),
    f64::from_bits(0x3CB1A81B51EE6D88),
    f64::from_bits(0x3CB1C666F8F82ACB),
    f64::from_bits(0x3CB1E48B93E0D42E),
    f64::from_bits(0x3CB2028A9940A09F),
    f64::from_bits(0x3CB2206572C4C6E9),
    f64::from_bits(0x3CB23E1D7DE9C31F),
    f64::from_bits(0x3CB25BB40CA96BFB),
    f64::from_bits(0x3CB2792A661DD37F),
    f64::from_bits(0x3CB29681C719D71B),
    f64::from_bits(0x3CB2B3BB62B82EDA),
    f64::from_bits(0x3CB2D0D862E1B853),
    f64::from_bits(0x3CB2EDD9E8CBA98E),
    f64::from_bits(0x3CB30AC10D6E48D7),
    f64::from_bits(0x3CB3278EE1F4B930),
    f64::from_bits(0x3CB3444470265EA1),
    f64::from_bits(0x3CB360E2BACA52D5),
    f64::from_bits(0x3CB37D6ABE05586A),
    f64::from_bits(0x3CB399DD6FB2B264),
    f64::from_bits(0x3CB3B63BBFB83D03),
    f64::from_bits(0x3CB3D28698561DE0),
    f64::from_bits(0x3CB3EEBEDE725A83),
    f64::from_bits(0x3CB40AE571E09E74),
    f64::from_bits(0x3CB426FB2DA6745D),
    f64::from_bits(0x3CB44300E83C30A4),
    f64::from_bits(0x3CB45EF773CAC75D),
    f64::from_bits(0x3CB47ADF9E66C336),
    f64::from_bits(0x3CB496BA32488F2F),
    f64::from_bits(0x3CB4B287F602415D),
    f64::from_bits(0x3CB4CE49ACB311DC),
    f64::from_bits(0x3CB4EA001638A605),
    f64::from_bits(0x3CB505ABEF5E5562),
    f64::from_bits(0x3CB5214DF20A8B5A),
    f64::from_bits(0x3CB53CE6D56A664F),
    f64::from_bits(0x3CB558774E1BB2C8),
    f64::from_bits(0x3CB574000E555F78),
    f64::from_bits(0x3CB58F81C60E8514),
    f64::from_bits(0x3CB5AAFD23241B59),
    f64::from_bits(0x3CB5C672D17D733D),
    f64::from_bits(0x3CB5E1E37B2F8CD3),
    f64::from_bits(0x3CB5FD4FC89F5E38),
    f64::from_bits(0x3CB618B860A31FC3),
    f64::from_bits(0x3CB6341DE8A2B0A2),
    f64::from_bits(0x3CB64F8104B7260B),
    f64::from_bits(0x3CB66AE257C99672),
    f64::from_bits(0x3CB6864283B13137),
    f64::from_bits(0x3CB6A1A22950B2B1),
    f64::from_bits(0x3CB6BD01E8B343BB),
    f64::from_bits(0x3CB6D8626128D352),
    f64::from_bits(0x3CB6F3C43161F854),
    f64::from_bits(0x3CB70F27F78B68EB),
    f64::from_bits(0x3CB72A8E516914C6),
    f64::from_bits(0x3CB745F7DC70EEDC),
    f64::from_bits(0x3CB7616535E5731F),
    f64::from_bits(0x3CB77CD6FAEFF449),
    f64::from_bits(0x3CB7984DC8BABD93),
    f64::from_bits(0x3CB7B3CA3C8B1409),
    f64::from_bits(0x3CB7CF4CF3DB22FB),
    f64::from_bits(0x3CB7EAD68C73DEE7),
    f64::from_bits(0x3CB80667A486EA1F),
    f64::from_bits(0x3CB82200DAC88676),
    f64::from_bits(0x3CB83DA2CE899F15),
    f64::from_bits(0x3CB8594E1FD1F5BD),
    f64::from_bits(0x3CB875036F7A7EC5),
    f64::from_bits(0x3CB890C35F47F72D),
    f64::from_bits(0x3CB8AC8E9205C043),
    f64::from_bits(0x3CB8C865ABA10C9C),
    f64::from_bits(0x3CB8E44951446A27),
    f64::from_bits(0x3CB9003A2973B58F),
    f64::from_bits(0x3CB91C38DC288347),
    f64::from_bits(0x3CB9384612EF0AFC),
    f64::from_bits(0x3CB954627903A28A),
    f64::from_bits(0x3CB9708EBB70D5EE),
    f64::from_bits(0x3CB98CCB892E2A31),
    f64::from_bits(0x3CB9A919933F99BF),
    f64::from_bits(0x3CB9C5798CD5D92C),
    f64::from_bits(0x3CB9E1EC2B6F7411),
    f64::from_bits(0x3CB9FE7226FAD24A),
    f64::from_bits(0x3CBA1B0C39F93692),
    f64::from_bits(0x3CBA37BB21A2C85B),
    f64::from_bits(0x3CBA547F9E0BBB88),
    f64::from_bits(0x3CBA715A724AA9A4),
    f64::from_bits(0x3CBA8E4C64A0313D),
    f64::from_bits(0x3CBAAB563E9FF108),
    f64::from_bits(0x3CBAC878CD5AF5CE),
    f64::from_bits(0x3CBAE5B4E18BB336),
    f64::from_bits(0x3CBB030B4FC3A11A),
    f64::from_bits(0x3CBB207CF09A985B),
    f64::from_bits(0x3CBB3E0AA0E00C00),
    f64::from_bits(0x3CBB5BB541CE3D03),
    f64::from_bits(0x3CBB797DB93F8927),
    f64::from_bits(0x3CBB9764F1E5F73C),
    f64::from_bits(0x3CBBB56BDB85256E),
    f64::from_bits(0x3CBBD3936B2EC0A2),
    f64::from_bits(0x3CBBF1DC9B81AE83),
    f64::from_bits(0x3CBC10486CEC16A0),
    f64::from_bits(0x3CBC2ED7E5F07A2D),
    f64::from_bits(0x3CBC4D8C136E0D1C),
    f64::from_bits(0x3CBC6C6608EC8705),
    f64::from_bits(0x3CBC8B66E0EBA617),
    f64::from_bits(0x3CBCAA8FBD36A2AB),
    f64::from_bits(0x3CBCC9E1C73BD690),
    f64::from_bits(0x3CBCE95E3068E037),
    f64::from_bits(0x3CBD0906328B8F6E),
    f64::from_bits(0x3CBD28DB1037EF20),
    f64::from_bits(0x3CBD48DE1533C647),
    f64::from_bits(0x3CBD691096E7F123),
    f64::from_bits(0x3CBD8973F4D7FBA5),
    f64::from_bits(0x3CBDAA0999206E70),
    f64::from_bits(0x3CBDCAD2F8FC490E),
    f64::from_bits(0x3CBDEBD195522E37),
    f64::from_bits(0x3CBE0D06FB49D21C),
    f64::from_bits(0x3CBE2E74C4EA46F6),
    f64::from_bits(0x3CBE501C99C1D188),
    f64::from_bits(0x3CBE72002F97FE25),
    f64::from_bits(0x3CBE94214B2ABF0A),
    f64::from_bits(0x3CBEB681C0F76F08),
    f64::from_bits(0x3CBED9237610A73A),
    f64::from_bits(0x3CBEFC086101ECA9),
    f64::from_bits(0x3CBF1F328AC25321),
    f64::from_bits(0x3CBF42A40FB74D6D),
    f64::from_bits(0x3CBF665F20C90168),
    f64::from_bits(0x3CBF8A6604899782),
    f64::from_bits(0x3CBFAEBB187122BF),
    f64::from_bits(0x3CBFD360D22FE785),
    f64::from_bits(0x3CBFF859C118F60B),
    f64::from_bits(0x3CC00ED447D3A075),
    f64::from_bits(0x3CC021A8028FC947),
    f64::from_bits(0x3CC034A983A902AB),
    f64::from_bits(0x3CC047DA4E3EF5C7),
    f64::from_bits(0x3CC05B3BF6ADB37E),
    f64::from_bits(0x3CC06ED023A72668),
    f64::from_bits(0x3CC082988F632E17),
    f64::from_bits(0x3CC0969708E8A254),
    f64::from_bits(0x3CC0AACD7571C0C4),
    f64::from_bits(0x3CC0BF3DD1EED448),
    f64::from_bits(0x3CC0D3EA34AA3D30),
    f64::from_bits(0x3CC0E8D4CF116593),
    f64::from_bits(0x3CC0FDFFEFA69FB6),
    f64::from_bits(0x3CC1136E04207041),
    f64::from_bits(0x3CC129219BBB5D35),
    f64::from_bits(0x3CC13F1D69C4096D),
    f64::from_bits(0x3CC1556448602E3B),
    f64::from_bits(0x3CC16BF93B9DEEF3),
    f64::from_bits(0x3CC182DF74D21261),
    f64::from_bits(0x3CC19A1A564EEBAC),
    f64::from_bits(0x3CC1B1AD777F2F8E),
    f64::from_bits(0x3CC1C99CA971A694),
    f64::from_bits(0x3CC1E1EBFBE4AE39),
    f64::from_bits(0x3CC1FA9FC2E2D901),
    f64::from_bits(0x3CC213BC9D04CC81),
    f64::from_bits(0x3CC22D477A6FD3EE),
    f64::from_bits(0x3CC24745A4AC9C24),
    f64::from_bits(0x3CC261BCC77658E0),
    f64::from_bits(0x3CC27CB2FAA8592E),
    f64::from_bits(0x3CC2982ECD770E78),
    f64::from_bits(0x3CC2B437532A0A52),
    f64::from_bits(0x3CC2D0D43196DB97),
    f64::from_bits(0x3CC2EE0DB1A978F5),
    f64::from_bits(0x3CC30BECD256AEEE),
    f64::from_bits(0x3CC32A7B5E68A4A3),
    f64::from_bits(0x3CC349C405AE12A3),
    f64::from_bits(0x3CC369D27A33A840),
    f64::from_bits(0x3CC38AB39256410A),
    f64::from_bits(0x3CC3AC7570AE88FA),
    f64::from_bits(0x3CC3CF27B31704A6),
    f64::from_bits(0x3CC3F2DBAA60F475),
    f64::from_bits(0x3CC417A49CB9E5DA),
    f64::from_bits(0x3CC43D9815545E94),
    f64::from_bits(0x3CC464CE44A73A15),
    f64::from_bits(0x3CC48D62759C43BC),
    f64::from_bits(0x3CC4B7739D6B5A27),
    f64::from_bits(0x3CC4E3250DCD8902),
    f64::from_bits(0x3CC5109F53E9AC41),
    f64::from_bits(0x3CC54011523A7E42),
    f64::from_bits(0x3CC571B1A94AE41B),
    f64::from_bits(0x3CC5A5C08B718DD9),
    f64::from_bits(0x3CC5DC8A243AD0FE),
    f64::from_bits(0x3CC61669CF861E4C),
    f64::from_bits(0x3CC653CE7B006AEA),
    f64::from_bits(0x3CC69540BE9FE5C3),
    f64::from_bits(0x3CC6DB6B8D09E232),
    f64::from_bits(0x3CC72728F05F7A34),
    f64::from_bits(0x3CC7799556090673),
    f64::from_bits(0x3CC7D42DF4D6CE8C),
    f64::from_bits(0x3CC839030529F234),
    f64::from_bits(0x3CC8AB0FBFAA7C14),
    f64::from_bits(0x3CC92EE0946F4496),
    f64::from_bits(0x3CC9CBEE014057AB),
    f64::from_bits(0x3CCA8FDC7894775A),
    f64::from_bits(0x3CCB981F3878FDB1),
    f64::from_bits(0x3CCD3BB48209AD33),
];
#[rustfmt::skip]
const ZIG_FI: [f64; 256] = [
    f64::from_bits(0x3FF0000000000000),
    f64::from_bits(0x3FEF446AC979F087),
    f64::from_bits(0x3FEEB7545B6CA915),
    f64::from_bits(0x3FEE3F11E027F077),
    f64::from_bits(0x3FEDD36FA704DE95),
    f64::from_bits(0x3FED70920657BCF2),
    f64::from_bits(0x3FED144978A119DC),
    f64::from_bits(0x3FECBD33A8A72DEB),
    f64::from_bits(0x3FEC6A5ECEA9787F),
    f64::from_bits(0x3FEC1B1CD9EEBAEA),
    f64::from_bits(0x3FEBCEEB4EE1DC82),
    f64::from_bits(0x3FEB85653A8FF552),
    f64::from_bits(0x3FEB3E3A8234DD10),
    f64::from_bits(0x3FEAF92A3F6CE8A2),
    f64::from_bits(0x3FEAB5FEF17A2504),
    f64::from_bits(0x3FEA748BD550C9E1),
    f64::from_bits(0x3FEA34AAFDF5AF0F),
    f64::from_bits(0x3FE9F63BEE651FD8),
    f64::from_bits(0x3FE9B9228D240681),
    f64::from_bits(0x3FE97D4657617AC1),
    f64::from_bits(0x3FE94291C21B7A47),
    f64::from_bits(0x3FE908F1BD31714F),
    f64::from_bits(0x3FE8D0554FE60AA8),
    f64::from_bits(0x3FE898AD48BADF02),
    f64::from_bits(0x3FE861EBFC37BCAC),
    f64::from_bits(0x3FE82C050F56CF6E),
    f64::from_bits(0x3FE7F6ED4B20E2CB),
    f64::from_bits(0x3FE7C29A779C6858),
    f64::from_bits(0x3FE78F033CA0B0D5),
    f64::from_bits(0x3FE75C1F0770D856),
    f64::from_bits(0x3FE729E5F43F6D12),
    f64::from_bits(0x3FE6F850BAEA7AEE),
    f64::from_bits(0x3FE6C7589E635A89),
    f64::from_bits(0x3FE696F75E513B2A),
    f64::from_bits(0x3FE667272A92E323),
    f64::from_bits(0x3FE637E298550C18),
    f64::from_bits(0x3FE6092498802665),
    f64::from_bits(0x3FE5DAE86F4AFF6A),
    f64::from_bits(0x3FE5AD29ACC85C89),
    f64::from_bits(0x3FE57FE4264C8D8F),
    f64::from_bits(0x3FE55313F08D9E46),
    f64::from_bits(0x3FE526B55A656CD5),
    f64::from_bits(0x3FE4FAC4E820B667),
    f64::from_bits(0x3FE4CF3F4F494EC0),
    f64::from_bits(0x3FE4A42172DC5278),
    f64::from_bits(0x3FE479685FDF5012),
    f64::from_bits(0x3FE44F114A493679),
    f64::from_bits(0x3FE425198A355FE3),
    f64::from_bits(0x3FE3FB7E99585B82),
    f64::from_bits(0x3FE3D23E10AF31A3),
    f64::from_bits(0x3FE3A955A662CD0E),
    f64::from_bits(0x3FE380C32BDA00D5),
    f64::from_bits(0x3FE358848BF550E9),
    f64::from_bits(0x3FE33097C9703A35),
    f64::from_bits(0x3FE308FAFD6438EF),
    f64::from_bits(0x3FE2E1AC55EA3BEE),
    f64::from_bits(0x3FE2BAAA14D7954A),
    f64::from_bits(0x3FE293F28E93CD15),
    f64::from_bits(0x3FE26D84290504ED),
    f64::from_bits(0x3FE2475D5A90DB84),
    f64::from_bits(0x3FE2217CA92FF7F2),
    f64::from_bits(0x3FE1FBE0A9929620),
    f64::from_bits(0x3FE1D687FE549969),
    f64::from_bits(0x3FE1B171573FD111),
    f64::from_bits(0x3FE18C9B709B3C50),
    f64::from_bits(0x3FE16805128639DA),
    f64::from_bits(0x3FE143AD105EA99C),
    f64::from_bits(0x3FE11F9248311F38),
    f64::from_bits(0x3FE0FBB3A2325913),
    f64::from_bits(0x3FE0D810104142A0),
    f64::from_bits(0x3FE0B4A68D70D9AE),
    f64::from_bits(0x3FE091761D995D81),
    f64::from_bits(0x3FE06E7DCCF03C36),
    f64::from_bits(0x3FE04BBCAFA63F2E),
    f64::from_bits(0x3FE02931E18B822A),
    f64::from_bits(0x3FE006DC85B8CAC4),
    f64::from_bits(0x3FDFC9778C7BBDA1),
    f64::from_bits(0x3FDF859DA7A900CA),
    f64::from_bits(0x3FDF4229CB2F7AF3),
    f64::from_bits(0x3FDEFF1A717E8F95),
    f64::from_bits(0x3FDEBC6E20BD1F54),
    f64::from_bits(0x3FDE7A236A4EC3C5),
    f64::from_bits(0x3FDE3838EA5F9B85),
    f64::from_bits(0x3FDDF6AD47763A09),
    f64::from_bits(0x3FDDB57F320B56B1),
    f64::from_bits(0x3FDD74AD6426DE33),
    f64::from_bits(0x3FDD3436A1021080),
    f64::from_bits(0x3FDCF419B4AE5B6D),
    f64::from_bits(0x3FDCB45573C0A848),
    f64::from_bits(0x3FDC74E8BB00D7C7),
    f64::from_bits(0x3FDC35D26F1D2CB8),
    f64::from_bits(0x3FDBF7117C616A17),
    f64::from_bits(0x3FDBB8A4D6716D91),
    f64::from_bits(0x3FDB7A8B7807131B),
    f64::from_bits(0x3FDB3CC462B331CA),
    f64::from_bits(0x3FDAFF4E9EA18552),
    f64::from_bits(0x3FDAC2293A5F5A9E),
    f64::from_bits(0x3FDA85534AA4D880),
    f64::from_bits(0x3FDA48CBEA20C04D),
    f64::from_bits(0x3FDA0C923946843E),
    f64::from_bits(0x3FD9D0A55E1E93DF),
    f64::from_bits(0x3FD995048418C0C6),
    f64::from_bits(0x3FD959AEDBE09F93),
    f64::from_bits(0x3FD91EA39B33CB17),
    f64::from_bits(0x3FD8E3E1FCB9F115),
    f64::from_bits(0x3FD8A9693FDE9188),
    f64::from_bits(0x3FD86F38A8AC5AB6),
    f64::from_bits(0x3FD8354F7FAA0DD9),
    f64::from_bits(0x3FD7FBAD11B8D911),
    f64::from_bits(0x3FD7C250AFF414B0),
    f64::from_bits(0x3FD78939AF9252EB),
    f64::from_bits(0x3FD7506769C7B1ED),
    f64::from_bits(0x3FD717D93BA9614C),
    f64::from_bits(0x3FD6DF8E86124CAA),
    f64::from_bits(0x3FD6A786AD88DE21),
    f64::from_bits(0x3FD66FC11A25CBE2),
    f64::from_bits(0x3FD6383D377BE515),
    f64::from_bits(0x3FD600FA7480D2C8),
    f64::from_bits(0x3FD5C9F84376C244),
    f64::from_bits(0x3FD5933619D6EEBE),
    f64::from_bits(0x3FD55CB3703D0100),
    f64::from_bits(0x3FD5266FC2533BED),
    f64::from_bits(0x3FD4F06A8EBF6D92),
    f64::from_bits(0x3FD4BAA357109CA2),
    f64::from_bits(0x3FD485199FAD6AD4),
    f64::from_bits(0x3FD44FCCEFC324FE),
    f64::from_bits(0x3FD41ABCD1357A19),
    f64::from_bits(0x3FD3E5E8D08ED2DB),
    f64::from_bits(0x3FD3B1507CF143AE),
    f64::from_bits(0x3FD37CF368081379),
    f64::from_bits(0x3FD348D125F9D19E),
    f64::from_bits(0x3FD314E94D5AF62F),
    f64::from_bits(0x3FD2E13B77210766),
    f64::from_bits(0x3FD2ADC73E963FDD),
    f64::from_bits(0x3FD27A8C414DB11E),
    f64::from_bits(0x3FD2478A1F17DE89),
    f64::from_bits(0x3FD214C079F7CC9E),
    f64::from_bits(0x3FD1E22EF6188116),
    f64::from_bits(0x3FD1AFD539C2F050),
    f64::from_bits(0x3FD17DB2ED5454E8),
    f64::from_bits(0x3FD14BC7BB34EE67),
    f64::from_bits(0x3FD11A134FCF2423),
    f64::from_bits(0x3FD0E895598709C4),
    f64::from_bits(0x3FD0B74D88B242DA),
    f64::from_bits(0x3FD0863B8F904336),
    f64::from_bits(0x3FD0555F2242E9D9),
    f64::from_bits(0x3FD024B7F6C7747E),
    f64::from_bits(0x3FCFE88B89DF93C5),
    f64::from_bits(0x3FCF88108CB83235),
    f64::from_bits(0x3FCF27FE6CE998D2),
    f64::from_bits(0x3FCEC854A4C99C44),
    f64::from_bits(0x3FCE6912B2283CDD),
    f64::from_bits(0x3FCE0A3816457184),
    f64::from_bits(0x3FCDABC455C7900A),
    f64::from_bits(0x3FCD4DB6F8B2514F),
    f64::from_bits(0x3FCCF00F8A5E6FCC),
    f64::from_bits(0x3FCC92CD9971DF53),
    f64::from_bits(0x3FCC35F0B7D89D47),
    f64::from_bits(0x3FCBD9787ABE18A1),
    f64::from_bits(0x3FCB7D647A8731AA),
    f64::from_bits(0x3FCB21B452CCD13A),
    f64::from_bits(0x3FCAC667A2571807),
    f64::from_bits(0x3FCA6B7E0B19267E),
    f64::from_bits(0x3FCA10F7322D7E3D),
    f64::from_bits(0x3FC9B6D2BFD2FE5A),
    f64::from_bits(0x3FC95D105F6A7C27),
    f64::from_bits(0x3FC903AFBF74FA69),
    f64::from_bits(0x3FC8AAB09192815B),
    f64::from_bits(0x3FC852128A819A38),
    f64::from_bits(0x3FC7F9D5621F7175),
    f64::from_bits(0x3FC7A1F8D368A323),
    f64::from_bits(0x3FC74A7C9C7AB5A6),
    f64::from_bits(0x3FC6F3607E964716),
    f64::from_bits(0x3FC69CA43E21F25C),
    f64::from_bits(0x3FC64647A2ADF19C),
    f64::from_bits(0x3FC5F04A76F883F9),
    f64::from_bits(0x3FC59AAC88F31D6C),
    f64::from_bits(0x3FC5456DA9C86835),
    f64::from_bits(0x3FC4F08DADE31FC1),
    f64::from_bits(0x3FC49C0C6CF5CE2D),
    f64::from_bits(0x3FC447E9C20375D5),
    f64::from_bits(0x3FC3F4258B6931AE),
    f64::from_bits(0x3FC3A0BFAAE8D7EE),
    f64::from_bits(0x3FC34DB805B4AB88),
    f64::from_bits(0x3FC2FB0E847C2A65),
    f64::from_bits(0x3FC2A8C3137A071A),
    f64::from_bits(0x3FC256D5A2835EB7),
    f64::from_bits(0x3FC2054625183C34),
    f64::from_bits(0x3FC1B41492757D42),
    f64::from_bits(0x3FC16340E5A82D63),
    f64::from_bits(0x3FC112CB1DA26EB9),
    f64::from_bits(0x3FC0C2B33D5209BA),
    f64::from_bits(0x3FC072F94BB8BF85),
    f64::from_bits(0x3FC0239D54067D2A),
    f64::from_bits(0x3FBFA93ECB6B222C),
    f64::from_bits(0x3FBF0BFF29520E1C),
    f64::from_bits(0x3FBE6F7BF29AA54B),
    f64::from_bits(0x3FBDD3B56176E88F),
    f64::from_bits(0x3FBD38ABB9BD91E5),
    f64::from_bits(0x3FBC9E5F493B740A),
    f64::from_bits(0x3FBC04D0680B1015),
    f64::from_bits(0x3FBB6BFF78F2E233),
    f64::from_bits(0x3FBAD3ECE9CAF633),
    f64::from_bits(0x3FBA3C9933EA6286),
    f64::from_bits(0x3FB9A604DC9D5B19),
    f64::from_bits(0x3FB9103075A4A0AB),
    f64::from_bits(0x3FB87B1C9DBF2852),
    f64::from_bits(0x3FB7E6CA013EEFD6),
    f64::from_bits(0x3FB753395AAA1176),
    f64::from_bits(0x3FB6C06B73694A4C),
    f64::from_bits(0x3FB62E6124854D18),
    f64::from_bits(0x3FB59D1B577466A4),
    f64::from_bits(0x3FB50C9B06FA2BAE),
    f64::from_bits(0x3FB47CE1401B2213),
    f64::from_bits(0x3FB3EDEF23269A86),
    f64::from_bits(0x3FB35FC5E4D93E70),
    f64::from_bits(0x3FB2D266CF9B3111),
    f64::from_bits(0x3FB245D344DD0D91),
    f64::from_bits(0x3FB1BA0CBE97897D),
    f64::from_bits(0x3FB12F14D0F2179D),
    f64::from_bits(0x3FB0A4ED2C159625),
    f64::from_bits(0x3FB01B979E30E497),
    f64::from_bits(0x3FAF262C2B6C6E35),
    f64::from_bits(0x3FAE16D547B25181),
    f64::from_bits(0x3FAD092EFEADF162),
    f64::from_bits(0x3FABFD3E0F282A2C),
    f64::from_bits(0x3FAAF30790385F70),
    f64::from_bits(0x3FA9EA90F9295563),
    f64::from_bits(0x3FA8E3E02A68B5AB),
    f64::from_bits(0x3FA7DEFB77AF271E),
    f64::from_bits(0x3FA6DBE9B398D064),
    f64::from_bits(0x3FA5DAB23CF2ADD4),
    f64::from_bits(0x3FA4DB5D0E11275D),
    f64::from_bits(0x3FA3DDF2CE98EECB),
    f64::from_bits(0x3FA2E27CE83DF497),
    f64::from_bits(0x3FA1E9059F1F6ABC),
    f64::from_bits(0x3FA0F1982E968011),
    f64::from_bits(0x3F9FF881D718A5C4),
    f64::from_bits(0x3F9E121ADB828C75),
    f64::from_bits(0x3F9C301983CD091A),
    f64::from_bits(0x3F9A529F4E22EBF8),
    f64::from_bits(0x3F9879D1B600C10A),
    f64::from_bits(0x3F96A5DAF40BBF82),
    f64::from_bits(0x3F94D6EAF2FBB064),
    f64::from_bits(0x3F930D388DAB5E13),
    f64::from_bits(0x3F91490334603012),
    f64::from_bits(0x3F8F152A4F72DD49),
    f64::from_bits(0x3F8BA48D274F8FAC),
    f64::from_bits(0x3F8841040D8DA478),
    f64::from_bits(0x3F84EB96421ACFE0),
    f64::from_bits(0x3F81A59229952F92),
    f64::from_bits(0x3F7CE160F8EC6837),
    f64::from_bits(0x3F769EA8D90CB85D),
    f64::from_bits(0x3F708A1F03B0B1FD),
    f64::from_bits(0x3F655F9F43C1B067),
    f64::from_bits(0x3F54A605B6B9F70F),
];
