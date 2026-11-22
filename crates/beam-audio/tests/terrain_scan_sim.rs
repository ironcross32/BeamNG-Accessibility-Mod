//! Replays the terrain sonification scanner against known geometry: the port of
//! `diagnostic/terrain_scan_sim.py`, same scenario numbers, names and order.
//!
//! render_scan is a pure function of the packet, so every property of the instrument --
//! the tuning, the time axis, click-freedom, headroom -- is checkable with no stream, no
//! device and no game. Several scenarios are deliberately NEGATIVE: they assert that the
//! obvious wrong implementation produces a specific wrong answer, so a check cannot pass
//! for free. Scenarios 11, 12 and 18 read `bng_mod/lua/ge/extensions/terrainScanner.lua`.

use beam_audio::consts::*;
use beam_audio::dsp;
use beam_audio::scan::*;
use beam_testutil::repo_root;

const SR: f64 = 48000.0;
const SRU: usize = 48000;

struct Checks {
    failures: Vec<String>,
}

impl Checks {
    fn check(&mut self, label: &str, ok: bool, detail: impl AsRef<str>) {
        let d = detail.as_ref();
        println!(
            "   {label}: {}{}",
            if ok { "OK" } else { "FAIL" },
            if d.is_empty() {
                String::new()
            } else {
                format!(" - {d}")
            }
        );
        if !ok {
            self.failures.push(label.to_string());
        }
    }
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn octave_reduce(mut ratio: f64) -> f64 {
    while ratio < 1.0 - 1e-12 {
        ratio *= 2.0;
    }
    while ratio >= 2.0 - 1e-12 {
        ratio /= 2.0;
    }
    ratio
}

fn nearest_table_error(ratio: f64) -> f64 {
    let r = octave_reduce(ratio);
    SCAN_PYTHAGOREAN_RATIOS
        .iter()
        .map(|t| (r - t).abs())
        .fold(f64::INFINITY, f64::min)
}

fn sample(b: f64, r: f64, dz: Option<f64>, depth: Option<f64>, fam: Option<&str>) -> ScanSample {
    ScanSample {
        bearing_deg: b,
        range_m: r,
        dz_m: dz,
        depth_m: depth,
        family: fam.map(str::to_string),
    }
}

fn flat_scan(dz: Option<f64>, reach: f64) -> Vec<ScanSample> {
    let (bearings, rings) = (24usize, 25usize);
    let denom = 1.max(rings - 1) as f64;
    let mut out = Vec::new();
    for s in 0..bearings {
        let b = -90.0 + s as f64 * (180.0 / 1.max(bearings - 1) as f64);
        for r in 0..rings {
            out.push(sample(b, (r as f64 / denom) * reach, dz, None, None));
        }
    }
    out
}

fn obj(b: f64, r: f64, dz: f64, kind: Option<&str>) -> ScanObject {
    ScanObject {
        bearing_deg: b,
        range_m: r,
        dz_m: dz,
        kind: kind.map(str::to_string),
    }
}

type Buf = Vec<[f32; 2]>;

fn render(samples: &[ScanSample], objects: &[ScanObject], reach: f64) -> Buf {
    render_scan(samples, objects, reach, SR, SCAN_TERRAIN_DB, &[]).unwrap()
}

fn render_level(samples: &[ScanSample], objects: &[ScanObject], reach: f64, level: f64) -> Buf {
    render_scan(samples, objects, reach, SR, level, &[]).unwrap()
}

fn render_pois(samples: &[ScanSample], objects: &[ScanObject], pois: &[ScanPoi]) -> Buf {
    render_scan(samples, objects, 200.0, SR, SCAN_TERRAIN_DB, pois).unwrap()
}

fn channel(buf: &Buf, ch: usize, from: usize) -> Vec<f64> {
    buf[from.min(buf.len())..]
        .iter()
        .map(|f| f[ch] as f64)
        .collect()
}

fn energy_after(buf: &Buf, t_s: f64) -> f64 {
    let i = (SR * t_s) as usize;
    buf[i..]
        .iter()
        .map(|f| (f[0].abs() + f[1].abs()) as f64)
        .sum()
}

fn first_onset(buf: &Buf) -> Option<usize> {
    let i0 = (SR * (SCAN_REF_LEAD_S * 0.9)) as usize;
    buf[i0..]
        .iter()
        .position(|f| f[0].abs().max(f[1].abs()) as f64 > 1e-4)
        .map(|i| i0 + i)
}

fn spectrum(wave: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let spec: Vec<f64> = dsp::rfft(wave, wave.len())
        .iter()
        .map(|z| z.norm())
        .collect();
    (spec, dsp::rfftfreq(wave.len(), 1.0 / SR))
}

fn centroid_hz(wave: &[f64]) -> f64 {
    let (spec, freqs) = spectrum(wave);
    let total: f64 = spec.iter().sum();
    if total > 0.0 {
        spec.iter().zip(&freqs).map(|(s, f)| s * f).sum::<f64>() / total
    } else {
        0.0
    }
}

fn wide(w: &[f32]) -> Vec<f64> {
    w.iter().map(|&v| v as f64).collect()
}

fn peak_hz(wave: &[f64]) -> f64 {
    let (spec, freqs) = spectrum(wave);
    freqs[dsp::argmax(&spec)]
}

/// Level at a frequency relative to the waveform's own strongest partial.
fn partial_db(wave: &[f64], target_hz: f64) -> f64 {
    let (spec, freqs) = spectrum(wave);
    let diffs: Vec<f64> = freqs.iter().map(|f| -(f - target_hz).abs()).collect();
    let i = dsp::argmax(&diffs);
    let lo = i.saturating_sub(3);
    let hi = (i + 4).min(spec.len());
    let local = spec[lo..hi].iter().cloned().fold(f64::MIN, f64::max);
    let top = spec.iter().cloned().fold(f64::MIN, f64::max);
    20.0 * (local.max(1e-12) / top).log10()
}

fn max_abs_buf(buf: &Buf) -> f64 {
    buf.iter()
        .flat_map(|f| [f[0], f[1]])
        .fold(0.0f64, |m, v| m.max((v as f64).abs()))
}

/// `\nlocal NAME\s*=\s*(<chars>+)` from the Lua source.
fn lua_local(src: &str, name: &str, allowed: fn(char) -> bool) -> Option<String> {
    let pat = format!("\nlocal {name}");
    let mut from = 0;
    while let Some(i) = src[from..].find(&pat) {
        let rest = &src[from + i + pat.len()..];
        let r = rest.trim_start_matches([' ', '\t', '\r', '\n']);
        if let Some(r) = r.strip_prefix('=') {
            let r = r.trim_start_matches([' ', '\t', '\r', '\n']);
            let v: String = r.chars().take_while(|c| allowed(*c)).collect();
            if !v.is_empty() {
                return Some(v);
            }
        }
        from += i + 1;
    }
    None
}

fn digits_dot(c: char) -> bool {
    c.is_ascii_digit() || c == '.'
}

fn digits(c: char) -> bool {
    c.is_ascii_digit()
}

fn lua_path() -> std::path::PathBuf {
    repo_root()
        .join("bng_mod")
        .join("lua")
        .join("ge")
        .join("extensions")
        .join("terrainScanner.lua")
}

/// Rising edges of `|x|`'s 2 ms running maximum through 0.08, as the sim counts events.
fn count_events(w: &[f32]) -> usize {
    let win = (SR * 0.002) as usize;
    let env: Vec<bool> = (0..w.len() - win)
        .map(|i| w[i..i + win].iter().fold(0.0f32, |m, v| m.max(v.abs())) > 0.08)
        .collect();
    let rises = env.windows(2).filter(|p| !p[0] && p[1]).count();
    rises + usize::from(env[0])
}

#[test]
fn terrain_scan_sim() {
    let mut c = Checks {
        failures: Vec::new(),
    };
    let c_hz = SCAN_MIDDLE_C_HZ;
    let mut ring_ms: Option<f64> = None;

    // =========================================================================================
    println!("1. flat ground renders one pitch, and that pitch is middle C");
    c.check(
        "level ground is middle C",
        approx(scan_pitch_hz(0.0, 0.0), c_hz, 1e-9),
        format!("{:.3} Hz", scan_pitch_hz(0.0, 0.0)),
    );
    let buf = render(&flat_scan(Some(0.0), 200.0), &[], SCAN_MAX_RANGE_M);
    let seg: Vec<f64> = buf[SRU..(SR * 2.5) as usize]
        .iter()
        .map(|f| f[0] as f64)
        .collect();
    let pk = peak_hz(&seg);
    c.check(
        "the rendered flat cloud peaks at middle C",
        (pk - c_hz).abs() < 3.0,
        format!("peak {pk:.1} Hz"),
    );
    println!();

    // =========================================================================================
    println!("2. every pitch the scanner can produce is a Pythagorean degree");
    let grid = dsp::linspace(-60.0, 60.0, 601, true);
    let worst = grid
        .iter()
        .map(|&dz| nearest_table_error(scan_pitch_hz(dz, 0.0) / c_hz))
        .fold(0.0f64, f64::max);
    c.check(
        "every pitch lands on a table degree",
        worst < 1e-9,
        format!("worst error {worst:.2e}"),
    );
    let et_worst = (0..12)
        .map(|k| nearest_table_error(2.0f64.powf(k as f64 / 12.0)))
        .fold(0.0f64, f64::max);
    c.check(
        "an equal-tempered scale would FAIL this check",
        et_worst > 1e-4,
        format!("12-TET worst error {et_worst:.5}"),
    );
    let steps: Vec<i64> = grid.iter().map(|&d| scan_step_from_dz(d)).collect();
    c.check(
        "the step index is monotonic in elevation",
        steps.windows(2).all(|p| p[1] >= p[0]),
        "",
    );
    let lim = (SCAN_OCTAVE_CLAMP * 12.0).round_ties_even() as i64;
    c.check(
        "the pitch pins rather than wrapping past the clamp",
        scan_step_from_dz(1e6) == lim && scan_step_from_dz(-1e6) == -lim,
        format!(
            "steps {}..{}",
            scan_step_from_dz(-1e6),
            scan_step_from_dz(1e6)
        ),
    );
    println!();

    // =========================================================================================
    println!("3. the fifth is pure 3:2, which is the reason for the tuning");
    let f5 = scan_pitch_hz(SCAN_M_PER_OCTAVE * 7.0 / 12.0, 0.0);
    let cents = 1200.0 * (f5 / c_hz).log2();
    c.check(
        "the fifth is exactly 3:2",
        approx(f5 / c_hz, 1.5, 1e-12),
        format!("{:.12}", f5 / c_hz),
    );
    c.check(
        "...i.e. 701.955 cents, not 700",
        (cents - 701.955).abs() < 0.01 && (cents - 700.0).abs() > 1.9,
        format!("{cents:.3} cents"),
    );
    let f4 = scan_pitch_hz(SCAN_M_PER_OCTAVE * 5.0 / 12.0, 0.0);
    c.check(
        "the fourth is exactly 4:3",
        approx(f4 / c_hz, 4.0 / 3.0, 1e-12),
        "",
    );
    println!();

    // =========================================================================================
    println!("4. the scan takes the same time no matter how far it reached");
    let near = render(&flat_scan(Some(0.0), 60.0), &[], 60.0);
    let far = render(&flat_scan(Some(0.0), 200.0), &[], 200.0);
    c.check(
        "a 60 m scan and a 200 m scan are the same length",
        near.len() == far.len(),
        format!("{} vs {}", near.len(), far.len()),
    );
    let expected = (SR * (SCAN_REF_LEAD_S + SCAN_DURATION_S)) as usize;
    let tail = ((SR * SCAN_GRAIN_MS / 1000.0) as usize).max((SR * SCAN_PING_MS / 1000.0) as usize);
    c.check(
        "...and that length is the lead plus the duration plus one grain",
        expected < far.len()
            && far.len() <= expected + tail + (SR * SCAN_TIME_JITTER_MS / 1000.0) as usize + 8,
        format!("{} samples = {:.3} s", far.len(), far.len() as f64 / SR),
    );
    c.check(
        "the reference ping fits inside the lead",
        SCAN_REF_LEAD_S >= SCAN_REF_PING_MS / 1000.0,
        format!("lead {SCAN_REF_LEAD_S}s vs ping {SCAN_REF_PING_MS}ms"),
    );
    println!();

    // =========================================================================================
    println!("5. time is distance, and it is monotonic");
    let mut onsets = Vec::new();
    for rng_m in [0.0, 25.0, 50.0, 100.0, 150.0, 200.0] {
        let b = render(&[sample(0.0, rng_m, Some(0.0), None, None)], &[], 200.0);
        onsets.push((rng_m, first_onset(&b)));
    }
    let ok = onsets
        .windows(2)
        .all(|p| matches!((p[0].1, p[1].1), (Some(a), Some(b)) if b > a));
    c.check(
        "a farther sample always starts later",
        ok,
        onsets
            .iter()
            .map(|(r, o)| format!("{r:.0}m@{:.3}s", o.unwrap_or(0) as f64 / SR))
            .collect::<Vec<_>>()
            .join(", "),
    );
    let near_t = onsets[0].1.unwrap_or(0) as f64 / SR;
    c.check(
        "the sample AT the vehicle starts as the cloud opens",
        (near_t - SCAN_REF_LEAD_S).abs() < (SCAN_TIME_JITTER_MS / 1000.0) + 0.01,
        format!("{near_t:.3}s vs lead {SCAN_REF_LEAD_S}s"),
    );
    let far_t = onsets[onsets.len() - 1].1.unwrap_or(0) as f64 / SR;
    c.check(
        "the sample at full reach lands at the end of the scan",
        (far_t - (SCAN_REF_LEAD_S + SCAN_DURATION_S)).abs() < (SCAN_TIME_JITTER_MS / 1000.0) + 0.01,
        format!("{far_t:.3}s vs {:.3}s", SCAN_REF_LEAD_S + SCAN_DURATION_S),
    );
    println!();

    // =========================================================================================
    println!("6. water: deeper is lower, and it is unmistakably brighter");
    let freqs: Vec<f64> = dsp::linspace(0.0, 40.0, 81, true)
        .iter()
        .map(|&d| scan_pitch_hz(-d, 0.0))
        .collect();
    c.check(
        "deeper water is monotonically lower",
        freqs.windows(2).all(|p| p[1] <= p[0] + 1e-12),
        format!(
            "{:.1} Hz at the surface -> {:.1} Hz at 40 m",
            freqs[0],
            freqs[freqs.len() - 1]
        ),
    );
    let wc = centroid_hz(&wide(&scan_water_grain(c_hz, SR)));
    let tc = centroid_hz(&wide(&scan_terrain_grain(c_hz, SR, SCAN_PAVED)));
    c.check(
        "the water grain is far brighter at the SAME pitch",
        wc > tc * 1.5,
        format!("centroid {wc:.0} Hz vs terrain {tc:.0} Hz"),
    );
    let dry_same_pitch = render(&[sample(0.0, 50.0, Some(0.0), None, None)], &[], 200.0);
    let zero_depth_water = render(&[sample(0.0, 50.0, Some(0.0), Some(0.0), None)], &[], 200.0);
    let positive_depth_water = render(&[sample(0.0, 50.0, Some(0.0), Some(1.0), None)], &[], 200.0);
    let cloud_start = (SR * SCAN_REF_LEAD_S) as usize;
    let dry_rendered_c = centroid_hz(&channel(&dry_same_pitch, 0, cloud_start));
    let zero_water_c = centroid_hz(&channel(&zero_depth_water, 0, cloud_start));
    c.check(
        "a zero-depth water cell selects the bright water grain",
        zero_water_c > dry_rendered_c * 1.5,
        format!("centroid {zero_water_c:.0} Hz vs dry {dry_rendered_c:.0} Hz"),
    );
    c.check(
        "the water tag is authoritative at zero depth",
        zero_depth_water.len() == positive_depth_water.len()
            && zero_depth_water
                .iter()
                .zip(&positive_depth_water)
                .all(|(a, b)| (a[0] - b[0]).abs() <= 1e-7 && (a[1] - b[1]).abs() <= 1e-7),
        "",
    );
    c.check(
        "water and dry cells at identical pitch stay spectrally distinct",
        (zero_water_c - dry_rendered_c).abs() > 300.0,
        format!("centroid gap {:.0} Hz", zero_water_c - dry_rendered_c),
    );
    let oc = centroid_hz(&wide(&scan_object_ping(c_hz * 2.0, SR, "v")));
    c.check(
        "the object ping is brighter still",
        oc > wc,
        format!("centroid {oc:.0} Hz"),
    );
    println!();

    // =========================================================================================
    println!("7. headroom is structural, not a tuned hope");
    let dense = flat_scan(Some(0.0), 200.0);
    let mut objs = Vec::new();
    for b in (-80..=80).step_by(20) {
        for r in (10..200).step_by(40) {
            objs.push(obj(b as f64, r as f64, 2.0, None));
        }
    }
    let b = render(&dense, &objs, 200.0);
    let peak = max_abs_buf(&b);
    let ceiling = 10.0f64.powf(SCAN_TERRAIN_DB / 20.0) + 10.0f64.powf(SCAN_REF_PING_DB / 20.0);
    c.check(
        "a dense scan stays well under full scale",
        peak < 0.999,
        format!("peak {peak:.4}"),
    );
    c.check(
        "...and under the cloud level plus the ping level",
        peak <= ceiling + 1e-6,
        format!("peak {peak:.4} vs {ceiling:.4}"),
    );
    let loud = render_level(&dense, &objs, 200.0, 0.0);
    let lp = max_abs_buf(&loud);
    c.check(
        "even at a 0 dBFS setting it does not exceed full scale",
        lp < 1.0 + 10.0f64.powf(SCAN_REF_PING_DB / 20.0) + 1e-6,
        format!("peak {lp:.4}"),
    );
    println!();

    // =========================================================================================
    println!("8. no clicks anywhere");
    for (name, wave) in [
        ("terrain grain", scan_terrain_grain(c_hz, SR, SCAN_PAVED)),
        ("water grain", scan_water_grain(c_hz, SR)),
        ("object ping", scan_object_ping(c_hz * 2.0, SR, "v")),
        ("reference ping", scan_reference_ping(SR)),
    ] {
        let (a, z) = (wave[0] as f64, wave[wave.len() - 1] as f64);
        c.check(
            &format!("{name} starts and ends at silence"),
            a.abs() < 1e-6 && z.abs() < 1e-6,
            format!("{a:.2e} .. {z:.2e}"),
        );
    }
    let naive0 = (1.0f64).sin(); // sin(2 pi 333 t + 1) at t = 0, no envelope
    c.check(
        "an un-faded grain WOULD click",
        naive0.abs() > 0.1,
        format!("first sample {naive0:.3}"),
    );
    let b = render(&flat_scan(Some(0.0), 200.0), &[], 200.0);
    c.check(
        "the finished buffer starts and ends at silence",
        (b[0][0] as f64).abs() < 1e-6 && (b[b.len() - 1][0] as f64).abs() < 1e-6,
        "",
    );
    println!();

    // =========================================================================================
    println!("9. no surface is silence, never a plateau at level");
    let none_buf = render(&flat_scan(None, 200.0), &[], 200.0);
    let plateau_buf = render(&flat_scan(Some(0.0), 200.0), &[], 200.0);
    let e_none = energy_after(&none_buf, SCAN_REF_LEAD_S + 0.2);
    let e_flat = energy_after(&plateau_buf, SCAN_REF_LEAD_S + 0.2);
    c.check(
        "an all-missing scan is silent after the reference ping",
        e_none < 1e-3,
        format!("energy {e_none:.2e}"),
    );
    c.check(
        "...where the naive zero-fill form reports a plateau",
        e_flat > 100.0,
        format!("energy {e_flat:.1}"),
    );
    let mixed = render(
        &[
            sample(0.0, 20.0, Some(0.0), None, None),
            sample(0.0, 120.0, None, None, None),
        ],
        &[],
        200.0,
    );
    c.check(
        "a single missing sample drops out without disturbing its neighbours",
        first_onset(&mixed).is_some(),
        "",
    );
    println!();

    // =========================================================================================
    println!("10. positive bearing is LEFT");
    let left = render(&[sample(45.0, 100.0, Some(0.0), None, None)], &[], 200.0);
    let right = render(&[sample(-45.0, 100.0, Some(0.0), None, None)], &[], 200.0);
    let lo = (SR * (SCAN_REF_LEAD_S + 0.1)) as usize;
    let sum_abs =
        |buf: &Buf, ch: usize| -> f64 { channel(buf, ch, lo).iter().map(|v| v.abs()).sum() };
    let (el, er) = (sum_abs(&left, 0), sum_abs(&left, 1));
    c.check(
        "a bearing of +45 is louder on the LEFT",
        el > er * 1.3,
        format!("L {el:.1} vs R {er:.1}"),
    );
    let (rl, rr) = (sum_abs(&right, 0), sum_abs(&right, 1));
    c.check(
        "a bearing of -45 is louder on the RIGHT",
        rr > rl * 1.3,
        format!("L {rl:.1} vs R {rr:.1}"),
    );
    c.check(
        "the two are mirror images",
        approx(el, rr, 1e-3) && approx(er, rl, 1e-3),
        "",
    );
    println!();

    // =========================================================================================
    println!("11. the mod and audio.py agree about the reach");
    match std::fs::read_to_string(lua_path()) {
        Ok(src) => {
            let m = lua_local(&src, "SCAN_MAX_RANGE_M", digits_dot);
            c.check(
                "terrainScanner.lua declares SCAN_MAX_RANGE_M",
                m.is_some(),
                "",
            );
            if let Some(m) = &m {
                let v: f64 = m.parse().unwrap_or(f64::NAN);
                c.check(
                    "...and it matches audio.py",
                    approx(v, SCAN_MAX_RANGE_M, 1e-9),
                    format!("lua {m} vs python {SCAN_MAX_RANGE_M}"),
                );
            }
            let bm = lua_local(&src, "SCAN_BEARINGS", digits);
            let rm = lua_local(&src, "SCAN_RINGS", digits);
            c.check(
                "the grid is declared and is a half circle worth of spokes",
                bm.is_some()
                    && rm.is_some()
                    && bm.as_deref().unwrap().parse::<i64>().unwrap_or(0) >= 8,
                format!(
                    "{} x {}",
                    bm.as_deref().unwrap_or("?"),
                    rm.as_deref().unwrap_or("?")
                ),
            );
            c.check(
                "the mod reports missing ground as the silence sentinel, not as zero",
                src.contains("\"~\""),
                "",
            );
            let primary_ray = "local h = rayAt(p.x, p.y, SCAN_RAY_TOP_Z)";
            let heightmap_fallback =
                "if h == nil then h = heightmapAt(p.x, p.y, scan.origin.z) end";
            c.check(
                "visible-surface raycasts precede the TerrainBlock fallback",
                matches!((src.find(primary_ray), src.find(heightmap_fallback)), (Some(a), Some(b)) if a < b),
                "",
            );
            c.check(
                "River water is bounds-prefiltered and containment-tested",
                src.contains("findClassObjects(class)")
                    && src.contains("inXY(w.bounds, p.x, p.y)")
                    && src.contains(":containsPoint(vec3(")
                    && src.contains("segment >= 0"),
                "",
            );
            c.check(
                "WaterBlock is footprint-scoped while WaterPlane can be unbounded",
                src.contains("class == \"WaterPlane\" then desc.bounds = nil")
                    && src.contains("w.class ~= \"River\" and inXY(w.bounds, p.x, p.y)"),
                "",
            );
            let counters = src.matches("scan.rayCount = scan.rayCount + 1").count();
            let rays = src.matches("be:getSurfaceHeightBelow").count();
            c.check(
                "reference, surface, and bed rays all spend the shared budget",
                counters == 3 && rays == 2,
                format!("{counters} counters, {rays} textual ray mentions"),
            );
            c.check(
                "diagnostics retain the visible range, cell counts, and ray total",
                [
                    "lastScanDiag",
                    "surfaceMin",
                    "surfaceMax",
                    "waterCells",
                    "dryCells",
                    "missingCells",
                    "rayCount",
                ]
                .iter()
                .all(|t| src.contains(t)),
                "",
            );
            if let Some(rm) = &rm {
                // Onset jitter must stay under HALF a ring's spacing in time, or two rings can
                // swap and "later means farther" stops being true.
                let rings: i64 = rm.parse().unwrap_or(1);
                let spacing_ms = SCAN_DURATION_S * 1000.0 / 1.max(rings - 1) as f64;
                c.check(
                    "the onset jitter stays inside half a ring's spacing",
                    SCAN_TIME_JITTER_MS < spacing_ms / 2.0,
                    format!("jitter {SCAN_TIME_JITTER_MS} ms vs spacing {spacing_ms:.1} ms"),
                );
            }
            c.check(
                "bearings are built as up-cross-forward, i.e. positive is LEFT",
                src.contains("vec3(0, 0, 1):cross(fwd)"),
                "",
            );
        }
        Err(e) => c.check("terrainScanner.lua is readable", false, e.to_string()),
    }
    println!();

    // =========================================================================================
    println!("12. the grain melds, and it does not buy that with the time axis");
    let grain_n = (SR * SCAN_GRAIN_MS / 1000.0) as usize;
    let genv = scan_grain_envelope(grain_n, SR, 1.0);
    let flat_top_envelope = |n: usize, fade_ms: f64| -> Vec<f64> {
        // The pre-change window, rebuilt as the negative control.
        let mut e = vec![1.0f64; n];
        let f = ((SR * fade_ms / 1000.0) as usize).min(n / 2);
        if f > 0 {
            let r = dsp::raised_cosine_rise(f);
            for i in 0..f {
                e[i] *= r[i];
                e[n - f + i] *= r[f - 1 - i];
            }
        }
        e
    };
    let oenv = flat_top_envelope((SR * 90.0 / 1000.0) as usize, 8.0);
    let gl = genv[grain_n - 1];
    c.check(
        "the grain envelope ends at exactly zero",
        gl == 0.0,
        format!("{gl:?}"),
    );
    let slope = (genv[grain_n - 1] - genv[grain_n - 2]).abs();
    c.check(
        "...and arrives there with zero SLOPE, which is what makes a taper unnecessary",
        slope < 1e-9,
        format!("final slope {slope:.2e}"),
    );
    let lin_slope = 1.0 / (grain_n - 1) as f64;
    c.check(
        "...where a linear decay reaches zero with a slope three orders larger",
        lin_slope > 1e-6,
        format!("linear slope {lin_slope:.2e}"),
    );
    c.check(
        "there is no plateau: at half its length the grain is far below full level",
        genv[grain_n / 2] < 0.2,
        format!("{:.3}", genv[grain_n / 2]),
    );
    c.check(
        "...where the flat-topped form is still at full level there",
        oenv[oenv.len() / 2] > 0.99,
        format!("{:.3}", oenv[oenv.len() / 2]),
    );
    let max_diff = |e: &[f64], from: usize| -> f64 {
        e[from..]
            .windows(2)
            .map(|p| (p[1] - p[0]).abs())
            .fold(0.0, f64::max)
    };
    let new_slope = max_diff(&genv, (0.8 * grain_n as f64) as usize);
    let old_slope = max_diff(&oenv, (0.8 * oenv.len() as f64) as usize);
    c.check(
        "the flat-topped form's offset is two orders steeper than the cubic's decay",
        old_slope > new_slope * 50.0,
        format!("old {old_slope:.2e} vs new {new_slope:.2e}"),
    );
    let centroid_t = |e: &[f64]| -> f64 {
        let e2: Vec<f64> = e.iter().map(|v| v * v).collect();
        let num: f64 = e2
            .iter()
            .enumerate()
            .map(|(i, v)| v * (i as f64 / SR))
            .sum();
        num / e2.iter().sum::<f64>()
    };
    let cen_new = centroid_t(&genv);
    let cen_old = centroid_t(&oenv);
    let cen_frac = cen_new / (SCAN_GRAIN_MS / 1000.0);
    let closed = 1.0 / (2.0 * SCAN_GRAIN_DECAY_POW + 2.0);
    c.check(
        "the grain's energy centroid sits just past the bare cubic's 1/(2p+2)",
        closed <= cen_frac && cen_frac < closed + SCAN_GRAIN_ATTACK_MS / SCAN_GRAIN_MS,
        format!(
            "{:.1} ms of {:.0} ms = {cen_frac:.3}, closed form {closed:.3}",
            cen_new * 1000.0,
            SCAN_GRAIN_MS
        ),
    );
    c.check(
        "...and is EARLIER than the old flat-topped grain's, not later",
        cen_new < cen_old,
        format!(
            "new {:.1} ms vs old {:.1} ms",
            cen_new * 1000.0,
            cen_old * 1000.0
        ),
    );
    match std::fs::read_to_string(lua_path())
        .ok()
        .and_then(|s| lua_local(&s, "SCAN_RINGS", digits))
    {
        Some(rm) => {
            let rings: i64 = rm.parse().unwrap_or(1);
            let rms = SCAN_DURATION_S * 1000.0 / 1.max(rings - 1) as f64;
            ring_ms = Some(rms);
            let gmax = genv.iter().cloned().fold(f64::MIN, f64::max);
            let last_audible = genv
                .iter()
                .rposition(|&v| 20.0 * (v.max(1e-12) / gmax).log10() > -20.0)
                .unwrap_or(0);
            let audible_ms = last_audible as f64 / SR * 1000.0;
            c.check(
                "the grain is still sounding when the next ring starts",
                audible_ms > rms,
                format!("audible to {audible_ms:.1} ms vs ring spacing {rms:.1} ms"),
            );
        }
        None => c.check("the ring spacing is readable from the mod", false, ""),
    }
    let renv = scan_envelope(
        (SR * SCAN_REF_PING_MS / 1000.0) as usize,
        SR,
        SCAN_REF_RELEASE_MS,
    );
    c.check(
        "the reference ping keeps a sustained body and only its RELEASE is cubic",
        renv[renv.len() / 3] > 0.99 && renv[renv.len() - 1] == 0.0,
        format!(
            "third {:.3}, end {:?}",
            renv[renv.len() / 3],
            renv[renv.len() - 1]
        ),
    );
    println!();

    // =========================================================================================
    println!("13. surface families are separated on three axes, and none of them is pitch");
    let f0 = scan_pitch_hz(0.0, 0.0);
    let fams = [SCAN_PAVED, "grass", "dirt", "gravel", "ice"];
    let grains: Vec<(&str, Vec<f64>)> = fams
        .iter()
        .map(|&fam| (fam, wide(&scan_terrain_grain(f0, SR, fam))))
        .collect();
    let g = |name: &str| -> &Vec<f64> { &grains.iter().find(|(f, _)| *f == name).unwrap().1 };
    let cen = |name: &str| centroid_hz(g(name));
    c.check(
        "brightness is ordered loose < grass < paved",
        cen("dirt") < cen("grass")
            && cen("gravel") < cen("grass")
            && cen("grass") < cen(SCAN_PAVED),
        fams.iter()
            .map(|f| format!("{f} {:.2}", cen(f) / f0))
            .collect::<Vec<_>>()
            .join(", "),
    );
    c.check(
        "...and the loose pair is a clear tier below grass, not a rounding error",
        cen("grass") - cen("dirt").max(cen("gravel")) > 0.05 * f0,
        format!(
            "gap {:.3} f0",
            (cen("grass") - cen("dirt").max(cen("gravel"))) / f0
        ),
    );
    let water_c = centroid_hz(&wide(&scan_water_grain(f0, SR)));
    c.check(
        "ice is brighter than paved and darker than water",
        cen(SCAN_PAVED) < cen("ice") && cen("ice") < water_c,
        format!(
            "paved {:.2}, ice {:.2}, water {:.2}",
            cen(SCAN_PAVED) / f0,
            cen("ice") / f0,
            water_c / f0
        ),
    );
    c.check(
        "the FM modulator is below the carrier",
        SCAN_LOOSE_FM_RATIO < 1.0,
        format!("ratio {SCAN_LOOSE_FM_RATIO:.4}"),
    );
    let lower = f0 * (1.0 - SCAN_LOOSE_FM_RATIO);
    c.check(
        "...and its lower sideband is more than an octave under the fundamental",
        lower < f0 / 2.0,
        format!("{lower:.1} Hz against f0 {f0:.1} Hz"),
    );
    let d_side = partial_db(g("dirt"), lower);
    let v_side = partial_db(g("gravel"), lower);
    c.check(
        "dirt and gravel both carry that sideband, well below the fundamental",
        -25.0 < d_side && d_side < -5.0 && -25.0 < v_side && v_side < -5.0,
        format!("dirt {d_side:.1} dB, gravel {v_side:.1} dB"),
    );
    c.check(
        "...gravel is audibly rougher than dirt, which is what tells them apart",
        v_side > d_side + 3.0,
        format!("gravel {v_side:.1} dB vs dirt {d_side:.1} dB"),
    );
    let (pp, gp) = (
        partial_db(g(SCAN_PAVED), lower),
        partial_db(g("grass"), lower),
    );
    c.check(
        "...and neither paved nor grass has anything there, which is what tells THEM apart",
        pp < -40.0 && gp < -40.0,
        format!("paved {pp:.1} dB, grass {gp:.1} dB"),
    );
    let water = wide(&scan_water_grain(f0, SR));
    let (ice3, water3) = (partial_db(g("ice"), f0 * 3.0), partial_db(&water, f0 * 3.0));
    c.check(
        "ice carries no third harmonic where water carries a strong one",
        ice3 < -35.0 && water3 > -20.0,
        format!("ice {ice3:.1} dB, water {water3:.1} dB"),
    );
    for (fam, w) in &grains {
        let ph = peak_hz(w);
        c.check(
            &format!("the {fam} grain still sounds the elevation pitch and nothing else"),
            approx(ph, f0, 6.0),
            format!("{ph:.1} Hz vs {f0:.1} Hz"),
        );
    }
    println!();

    // =========================================================================================
    println!("14. no family is louder than its neighbours, and that is a property of the code");
    for dz in [0.0, 20.0, -30.0, 40.0, -40.0] {
        let hz = scan_pitch_hz(dz, 0.0);
        let reference = wide(&scan_terrain_grain(hz, SR, SCAN_PAVED));
        let ref_rms = dsp::rms(&reference);
        for fam in ["grass", "dirt", "gravel", "ice"] {
            let w = wide(&scan_terrain_grain(hz, SR, fam));
            let rel = 20.0 * (dsp::rms(&w) / ref_rms).log10();
            let pk = dsp::max_abs(&w);
            c.check(
                &format!("{fam} at dz {dz:+.0} matches paved's loudness exactly"),
                rel.abs() < 0.05 && pk < 1.5,
                format!("{rel:+.3} dB, peak {pk:.3}"),
            );
        }
    }
    let mut peak_only_spread = 0.0f64;
    for dz in [0.0, 20.0, -30.0, 40.0, -40.0] {
        let hz = scan_pitch_hz(dz, 0.0);
        let ref_rms = dsp::rms(&wide(&scan_terrain_grain(hz, SR, SCAN_PAVED)));
        let mut rels = Vec::new();
        for fam in ["grass", "dirt", "gravel", "ice"] {
            let n = (SR * SCAN_GRAIN_MS / 1000.0) as usize;
            let mult = if fam == "ice" {
                SCAN_ICE_ATTACK_MULT
            } else {
                1.0
            };
            let t = dsp::arange_div(n, SR);
            let raw: Vec<f64> = scan_family_wave(hz, &t, SR, fam)
                .iter()
                .zip(scan_grain_envelope(n, SR, mult))
                .map(|(a, b)| a * b)
                .collect();
            let pk = dsp::max_abs(&raw);
            let raw: Vec<f64> = raw.iter().map(|v| v / pk).collect();
            rels.push(20.0 * (dsp::rms(&raw) / ref_rms).log10());
        }
        let hi = rels.iter().cloned().fold(f64::MIN, f64::max);
        let lo = rels.iter().cloned().fold(f64::MAX, f64::min);
        peak_only_spread = peak_only_spread.max(hi - lo);
    }
    c.check(
        "...where peak normalisation alone lets the families spread by over a decibel",
        peak_only_spread > 1.0,
        format!("spread {peak_only_spread:.2} dB"),
    );
    println!();

    // =========================================================================================
    println!("15. an absent, unknown or water-covered family is today's tone");
    let one = |fam: Option<&str>, depth: Option<f64>| {
        render(&[sample(0.0, 50.0, Some(0.0), depth, fam)], &[], 200.0)
    };
    let paved_5 = one(Some(SCAN_PAVED), None);
    let absent_4 = one(None, None);
    let none_5 = one(Some(""), None);
    let unknown = one(Some("quicksand"), None);
    c.check(
        "a four-field sample from an older mod half renders as paved, bit for bit",
        absent_4 == paved_5,
        "",
    );
    c.check(
        "an explicit empty family renders as paved, bit for bit",
        none_5 == paved_5,
        "",
    );
    c.check(
        "a family name from a NEWER mod half renders as paved rather than raising",
        unknown == paved_5,
        "",
    );
    for fam in ["grass", "dirt", "gravel", "ice"] {
        c.check(
            &format!("...while {fam} genuinely differs from paved"),
            one(Some(fam), None) != paved_5,
            "",
        );
    }
    c.check(
        "depth beats family: the material under a lake is not what you are looking at",
        one(Some("gravel"), Some(1.0)) == one(None, Some(1.0)),
        "",
    );
    let mut vals: Vec<&str> = SCAN_FAMILY_CODES.iter().map(|(_, v)| *v).collect();
    vals.sort();
    vals.dedup();
    c.check(
        "every code the mod can send is a family audio.py knows",
        SCAN_FAMILY_CODES
            .iter()
            .all(|(_, v)| ["paved", "dirt", "gravel", "grass", "ice"].contains(v)),
        format!("{vals:?}"),
    );
    println!();

    // =========================================================================================
    println!("16. a POI is two events, and it cannot be read as two cells");
    let doub = scan_poi_doublet(scan_pitch_hz(0.0, SCAN_POI_OCTAVE), SR);
    let onsets_n = count_events(&doub);
    c.check(
        "the doublet is exactly two events",
        onsets_n == 2,
        format!("{onsets_n} onsets"),
    );
    let gap_ms = SCAN_POI_PING_MS + SCAN_POI_GAP_MS;
    match ring_ms {
        Some(rms) => c.check(
            "...and they are closer together than one ring, so they are one event not two cells",
            gap_ms < rms,
            format!("{gap_ms:.1} ms vs ring spacing {rms:.1} ms"),
        ),
        None => c.check("the ring spacing was resolved in scenario 12", false, ""),
    }
    let single = scan_object_ping(scan_pitch_hz(0.0, SCAN_OBJECT_OCTAVE), SR, "v");
    c.check(
        "...where the object ping is exactly one, so the count is the cue",
        count_events(&single) == 1,
        "",
    );
    let pf = scan_pitch_hz(0.0, SCAN_POI_OCTAVE);
    let of = scan_pitch_hz(0.0, SCAN_OBJECT_OCTAVE);
    let (dc, sc) = (
        centroid_hz(&wide(&doub)) / pf,
        centroid_hz(&wide(&single)) / of,
    );
    c.check(
        "...and the two are timbrally distinct as well as rhythmically",
        (dc - sc).abs() > 0.3,
        format!("POI {dc:.2} f0 vs object {sc:.2} f0"),
    );
    let half = (SR * SCAN_POI_PING_MS / 1000.0) as usize;
    c.check(
        "both halves of the doublet are at the SAME pitch",
        doub[..half] == doub[doub.len() - half..],
        "",
    );
    let s50 = [sample(0.0, 50.0, Some(0.0), None, None)];
    c.check(
        "a scan with no pois argument matches one given an empty list",
        render(&s50, &[], 200.0) == render_pois(&s50, &[], &[]),
        "",
    );
    let poi_buf = render_pois(
        &[],
        &[],
        &[ScanPoi {
            bearing_deg: 0.0,
            range_m: 50.0,
            dz_m: 0.0,
        }],
    );
    c.check(
        "a POI on its own renders",
        first_onset(&poi_buf).is_some(),
        "",
    );
    println!();

    // =========================================================================================
    println!("17. the object kind is read, and it changes length rather than identity");
    let veh = scan_object_ping(scan_pitch_hz(0.0, SCAN_OBJECT_OCTAVE), SR, "v");
    let prop = scan_object_ping(scan_pitch_hz(0.0, SCAN_OBJECT_OCTAVE), SR, "p");
    c.check(
        "a prop ping is shorter than a vehicle ping",
        prop.len() < veh.len()
            && approx(
                prop.len() as f64 / veh.len() as f64,
                SCAN_PROP_PING_MS / SCAN_PING_MS,
                0.01,
            ),
        format!("{} vs {} samples", prop.len(), veh.len()),
    );
    c.check(
        "...but they sound the same elevation pitch",
        approx(peak_hz(&wide(&veh)), peak_hz(&wide(&prop)), 30.0),
        "",
    );
    let o3 = render(&[], &[obj(0.0, 50.0, 0.0, None)], 200.0);
    let ov = render(&[], &[obj(0.0, 50.0, 0.0, Some("v"))], 200.0);
    let op = render(&[], &[obj(0.0, 50.0, 0.0, Some("p"))], 200.0);
    c.check(
        "a three-field object from an older mod half renders as a vehicle",
        o3 == ov,
        "",
    );
    c.check(
        "...and the tag is genuinely consulted, not ignored",
        ov != op,
        "",
    );
    c.check(
        "an unrecognised kind falls back to vehicle, the safer default",
        render(&[], &[obj(0.0, 50.0, 0.0, Some("?"))], 200.0) == ov,
        "",
    );
    let (v0, vl, vl2) = (
        veh[0] as f64,
        veh[veh.len() - 1] as f64,
        veh[veh.len() - 2] as f64,
    );
    c.check(
        "the object ping still ends at silence after the release change",
        v0.abs() < 1e-6 && vl.abs() < 1e-6 && (vl - vl2).abs() < 1e-6,
        format!("{vl:.2e}, slope {:.2e}", (vl - vl2).abs()),
    );
    println!();

    // =========================================================================================
    println!("18. the mod half agrees about families, roads and POIs");
    match std::fs::read_to_string(lua_path()) {
        Ok(lsrc) => {
            // Comment lines are stripped because the prose explaining these rules has to
            // write the very tokens the greps look for.
            let code: String = lsrc
                .split('\n')
                .filter(|ln| !ln.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");
            for (code_char, _) in SCAN_FAMILY_CODES.iter() {
                c.check(
                    &format!("the mod can emit the family code '{code_char}'"),
                    code.contains(&format!("\"{code_char}\"")),
                    "",
                );
            }
            c.check(
                "the material lookup is present",
                code.contains("getMaterialIdxWs") && code.contains("getGroundmodelName"),
                "",
            );
            c.check(
                "families are classified by collisiontype id, resolved through the by-name lookup",
                code.contains("collisiontype")
                    && code.contains("getGroundModelIDByName")
                    && code.contains("getGroundModelByID"),
                "",
            );
            c.check(
                "...and the family table is representative NAMES, not transcribed ids",
                code.contains("FAMILY_NAMES")
                    && code.contains("\"GRAVEL\"")
                    && code.contains("\"SNOW\""),
                "",
            );
            let counters = lsrc.matches("scan.rayCount = scan.rayCount + 1").count();
            c.check(
                "the material lookup spends no rays, so the budget counters are still three",
                counters == 3 && lsrc.matches("be:getSurfaceHeightBelow").count() == 2,
                format!("{counters} counters"),
            );
            c.check(
                "a ray that disagrees with the heightmap reports no family, so a bridge deck is not painted with the riverbed under it",
                code.contains("MAT_TERRAIN_AGREE_M"),
                "",
            );
            c.check(
                "the road overlay is built from the navigation graph and bucketed",
                code.contains("map.getMap")
                    && code.contains("ROAD_BUCKET_SIZE_M")
                    && code.contains("ROAD_Z_TOLERANCE_M"),
                "",
            );
            c.check(
                "POIs come from the game's own big-map aggregation",
                code.contains("gameplay_rawPois")
                    && code.contains("getRawPoiListByLevel")
                    && code.contains("getCurrentLevelIdentifier"),
                "",
            );
            c.check(
                "POIs are capped, so a mission cluster cannot bury the terrain bed",
                code.contains("POI_MAX"),
                "",
            );
            c.check(
                "the named nearest-POI request uses the same big-map source",
                code.contains("cmd == \"NEAREST_POI\"")
                    && code.contains("nearestPoi")
                    && code.contains("send(\"POI:\" .. jsonEncode(result))"),
                "",
            );
            c.check(
                "equal-distance duplicates prefer the marker with a description",
                code.contains("richer") && code.contains("best.description"),
                "",
            );
            c.check(
                "POI names and descriptions use the game locale helper",
                code.contains("translateWithOrWithoutContext"),
                "",
            );
            c.check(
                "the diag reports the family histogram, the overlay count and the POI count",
                [
                    "famCounts",
                    "roadCells",
                    "roadEdges",
                    "poiCount",
                    "hasTerrain",
                ]
                .iter()
                .all(|t| code.contains(t)),
                "",
            );
        }
        Err(e) => c.check("terrainScanner.lua is readable", false, e.to_string()),
    }
    println!();

    // =========================================================================================
    if !c.failures.is_empty() {
        panic!("{} FAILURE(S): {}", c.failures.len(), c.failures.join(", "));
    }
    println!("all checks passed");
}
