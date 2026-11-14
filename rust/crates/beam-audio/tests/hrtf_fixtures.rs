//! beam_audio::npy and beam_audio::hrtf against the real hrtf.py
//! (`rust/fixtures/gen/hrtf.py`), over all three bundled sets.
//!
//! The npz contents are compared exactly. Everything downstream of an FFT is compared
//! within a tolerance, because numpy's pocketfft and rustfft round differently (both to
//! O(eps log n)); the processing is a smooth function of those spectra, so the only
//! visible effect is the last bit of a float32 output sample:
//!
//! - ITD: 1e-12 s (a millionth of a microsecond; the curve spans +-760 us).
//! - EQ curve, band energies, gain, max ITD: 1e-9 relative.
//! - IR samples (float32 rows and the float64 get_hrir blend): 1e-6 of the ring's
//!   peak, i.e. a few float32 ulps at full scale and -120 dB below the IR.
//! - Per-row energies: 1e-6 relative.

use std::path::PathBuf;

use beam_audio::hrtf::{self, HrtfSet};
use beam_audio::npy::{NpyData, Npz};
use beam_testutil::{f, load, repo_root, same_f64};
use serde_json::Value;

fn b64(s: &str) -> Vec<u8> {
    let mut map = [255u8; 256];
    for (i, c) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        .iter()
        .enumerate()
    {
        map[*c as usize] = i as u8;
    }
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in s.as_bytes() {
        if c == b'=' {
            break;
        }
        acc = (acc << 6) | map[c as usize] as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn f8(v: &Value) -> Vec<f64> {
    b64(v.as_str().unwrap())
        .chunks_exact(8)
        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn f4(v: &Value) -> Vec<f32> {
    b64(v.as_str().unwrap())
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn rel_close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * b.abs().max(1e-300)
}

fn set_path(file: &str) -> PathBuf {
    repo_root().join(file)
}

#[test]
fn npz_members_load_exactly() {
    let fx = load("hrtf");
    for (set_id, s) in fx["sets"].as_object().unwrap() {
        let path = set_path(s["file"].as_str().unwrap());
        // The application finds the same file through beam_base::paths.
        assert!(path.is_file(), "{set_id}: {} missing", path.display());
        let npz = Npz::open(&path).unwrap_or_else(|e| panic!("{set_id}: {e}"));
        let members = s["members"].as_array().unwrap();
        assert_eq!(npz.files().len(), members.len(), "{set_id}: member count");
        for m in members {
            let name = m["name"].as_str().unwrap();
            let a = npz.get(name).unwrap();
            assert_eq!(
                a.dtype(),
                m["dtype"].as_str().unwrap(),
                "{set_id}/{name} dtype"
            );
            let shape: Vec<usize> = m["shape"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as usize)
                .collect();
            assert_eq!(a.shape, shape, "{set_id}/{name} shape");
            let flat = a.to_f64();
            assert!(
                same_f64(beam_audio::dsp::sum(&flat), f(&m["sum"])),
                "{set_id}/{name} sum"
            );
            let head = f8(&m["head"]);
            for (i, h) in head.iter().enumerate() {
                assert!(same_f64(flat[i], *h), "{set_id}/{name}[{i}]");
            }
            if name.starts_with("irs_") {
                assert!(matches!(a.data, NpyData::F32(_)));
            }
        }
    }
}

fn check_rows(case: &str, got: &[Vec<f32>], idx: &[usize], want: &[f32], peak: f64) {
    let n = got[0].len();
    assert_eq!(want.len(), idx.len() * n, "{case}: row length");
    let mut worst = 0.0f64;
    for (k, &i) in idx.iter().enumerate() {
        for j in 0..n {
            let d = (got[i][j] as f64 - want[k * n + j] as f64).abs();
            worst = worst.max(d);
            assert!(
                d <= 1e-6 * peak,
                "{case}: az {i} sample {j}: {} vs {} (peak {peak})",
                got[i][j],
                want[k * n + j]
            );
        }
    }
    eprintln!("{case}: worst IR difference {:.3e} of peak", worst / peak);
}

fn check_energies(case: &str, got: &[Vec<f32>], want: &[f64]) {
    assert_eq!(got.len(), want.len(), "{case}: row count");
    for (i, (row, w)) in got.iter().zip(want).enumerate() {
        let e: f64 = row.iter().map(|&v| (v as f64) * (v as f64)).sum();
        assert!(rel_close(e, *w, 1e-6), "{case}: energy row {i}: {e} vs {w}");
    }
}

fn peak_of(rows: &[Vec<f32>]) -> f64 {
    rows.iter()
        .flat_map(|r| r.iter())
        .fold(0.0f64, |m, &v| m.max((v as f64).abs()))
}

#[test]
fn processed_rings_match_python() {
    let fx = load("hrtf");
    let az_full: Vec<usize> = fx["az_full"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as usize)
        .collect();
    for c in fx["cases"].as_array().unwrap() {
        let set_id = c["set_id"].as_str().unwrap();
        let elev = f(&c["elevation"]);
        let eq = c["equalise"].as_bool().unwrap();
        let case = format!("{set_id} el {elev} eq {eq}");
        let file = fx["sets"][set_id]["file"].as_str().unwrap();
        let path = set_path(file);

        // The intermediate curves, from the pure functions.
        let npz = Npz::open(&path).unwrap();
        let az = npz.get("azimuths").unwrap().to_f64();
        let el = npz.get("elevations").unwrap().to_f64();
        let l = npz.get("irs_left").unwrap().rows_f64().unwrap();
        let r = npz.get("irs_right").unwrap().rows_f64().unwrap();
        let sr = npz.get("samplerate").unwrap().scalar_f64().unwrap();
        assert!(same_f64(sr, f(&c["samplerate"])), "{case}: samplerate");
        let (out_l, out_r, info, products) =
            hrtf::process_ring_detail(&az, &el, &l, &r, sr, elev, eq);

        assert!(
            same_f64(info.elevation, f(&c["info"]["elevation"])),
            "{case}: elevation"
        );
        let rings: Vec<f64> = c["info"]["rings"]
            .as_array()
            .unwrap()
            .iter()
            .map(f)
            .collect();
        assert_eq!(info.rings.len(), rings.len(), "{case}: rings");
        for (g, w) in info.rings.iter().zip(&rings) {
            assert!(same_f64(*g, *w), "{case}: ring {g} vs {w}");
        }
        assert!(
            rel_close(info.max_itd_us, f(&c["info"]["max_itd_us"]), 1e-9),
            "{case}: max ITD {} vs {}",
            info.max_itd_us,
            f(&c["info"]["max_itd_us"])
        );

        let itd = f8(&c["itd"]);
        for (i, (g, w)) in products.itd.iter().zip(&itd).enumerate() {
            assert!((g - w).abs() <= 1e-12, "{case}: ITD at {i}: {g} vs {w}");
        }
        match (&products.eq, c["eq"].is_null()) {
            (None, true) => {}
            (Some(g), false) => {
                for (i, (a, b)) in g.iter().zip(&f8(&c["eq"])).enumerate() {
                    assert!(rel_close(*a, *b, 1e-9), "{case}: EQ bin {i}: {a} vs {b}");
                }
            }
            _ => panic!("{case}: EQ presence"),
        }

        let mut both = out_l.clone();
        both.extend(out_r.iter().cloned());
        let energy = hrtf::band_energy_f32(&both, sr);
        assert!(
            rel_close(energy, f(&c["band_energy_unscaled"]), 1e-9),
            "{case}: band energy {energy} vs {}",
            f(&c["band_energy_unscaled"])
        );

        // The loaded set: processing, level match, resample, lookup.
        let set = HrtfSet::load_quiet(&path, Some(elev), Some(eq));
        assert!(set.is_loaded(), "{case}: {}", set.load_message);
        assert!(rel_close(set.gain, f(&c["gain"]), 1e-9), "{case}: gain");
        let (sl, sr_rows) = set.irs();
        assert!(
            rel_close(
                hrtf::band_energy_f32(&[sl, sr_rows].concat(), sr),
                f(&c["band_energy_scaled"]),
                1e-9
            ),
            "{case}: scaled band energy"
        );
        let native = &c["native"];
        assert_eq!(
            set.ir_len() as u64,
            native["ir_len"].as_u64().unwrap(),
            "{case}: IR length"
        );
        let peak = peak_of(sl).max(peak_of(sr_rows));
        check_rows(
            &format!("{case} native L"),
            sl,
            &az_full,
            &f4(&native["rows_l"]),
            peak,
        );
        check_rows(
            &format!("{case} native R"),
            sr_rows,
            &az_full,
            &f4(&native["rows_r"]),
            peak,
        );
        check_energies(&format!("{case} native L"), sl, &f8(&native["energy_l"]));
        check_energies(
            &format!("{case} native R"),
            sr_rows,
            &f8(&native["energy_r"]),
        );

        for rs in c["resampled"].as_array().unwrap() {
            let rate = f(&rs["rate"]);
            let mut s2 = set.clone();
            s2.resample(rate);
            let rcase = format!("{case} at {rate}");
            assert_eq!(
                s2.ir_len() as u64,
                rs["ir_len"].as_u64().unwrap(),
                "{rcase}: length"
            );
            let (a, b) = s2.irs();
            let p2 = peak_of(a).max(peak_of(b));
            check_rows(
                &format!("{rcase} L"),
                a,
                &[0, 90, 271],
                &f4(&rs["rows_l"]),
                p2,
            );
            check_rows(
                &format!("{rcase} R"),
                b,
                &[0, 90, 271],
                &f4(&rs["rows_r"]),
                p2,
            );
            check_energies(&format!("{rcase} L"), a, &f8(&rs["energy_l"]));
            check_energies(&format!("{rcase} R"), b, &f8(&rs["energy_r"]));
            if let Some(list) = rs.get("hrir").and_then(Value::as_array) {
                for h in list {
                    let azd = f(&h["az"]);
                    assert_eq!(h["dtype"].as_str().unwrap(), "<f8", "get_hrir is float64");
                    let (gl, gr) = s2.get_hrir(azd).unwrap();
                    for (side, got, want) in [("L", gl, f8(&h["l"])), ("R", gr, f8(&h["r"]))] {
                        assert_eq!(got.len(), want.len());
                        for (j, (x, y)) in got.iter().zip(&want).enumerate() {
                            assert!(
                                (x - y).abs() <= 1e-6 * p2,
                                "{rcase}: get_hrir({azd}) {side}[{j}]: {x} vs {y}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_missing_set_fails_to_load_without_panicking() {
    let set = HrtfSet::load_quiet(&PathBuf::from("no_such_hrtf.npz"), None, None);
    assert!(!set.is_loaded());
    assert!(set.get_hrir(10.0).is_none());
    assert!(
        set.load_message
            .starts_with("Failed to load HRTF from 'no_such_hrtf.npz': "),
        "{}",
        set.load_message
    );
}

#[test]
fn load_message_matches_python_format() {
    let path = set_path("hrtf_mit_kemar.npz");
    let set = HrtfSet::load_quiet(&path, None, None);
    let msg = &set.load_message;
    let expected_tail = ": elevation -3.0 deg (from rings -10, +0), equalised, max ITD 699 us, ";
    assert!(msg.contains(expected_tail), "{msg}");
    assert!(
        msg.contains("samples/IR at 44100 Hz, level matched by +2.3 dB"),
        "{msg}"
    );
}
