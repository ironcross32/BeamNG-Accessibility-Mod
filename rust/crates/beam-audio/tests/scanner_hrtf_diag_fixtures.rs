//! beam_audio::scanner_hrtf_diag against scanner_hrtf_diag.py
//! (`rust/fixtures/gen/scanner_hrtf_diag.py`): header, RMS (bit for bit), ILD (within
//! 1e-12 dB; numpy's log10 against the C library's) and the CSV row format, plus the
//! recorder end to end through a temporary file.

use beam_audio::scanner_hrtf_diag::*;
use beam_testutil::{f, load, same_f64};

#[test]
fn header_rms_ild_and_row_format_match() {
    let fx = load("scanner_hrtf_diag");
    assert_eq!(CSV_HEADER, fx["header"].as_str().unwrap());
    for c in fx["rms"].as_array().unwrap() {
        let x: Vec<f64> = c["x"].as_array().unwrap().iter().map(f).collect();
        let got = match c["dtype"].as_str().unwrap() {
            "<f4" => rms_f32(Some(&x.iter().map(|&v| v as f32).collect::<Vec<_>>())),
            "<f8" => rms(Some(&x)),
            _ => rms(None),
        };
        assert!(same_f64(got, f(&c["rms"])), "rms of {} values", x.len());
    }
    for c in fx["ild"].as_array().unwrap() {
        let got = ild_db(f(&c["l"]), f(&c["r"]));
        let want = f(&c["out"]);
        assert!(
            (got.is_nan() && want.is_nan()) || (got - want).abs() <= 1e-12,
            "ild {} {}: {got} vs {want}",
            f(&c["l"]),
            f(&c["r"])
        );
    }
    for c in fx["rows"].as_array().unwrap() {
        let r = c["row"].as_array().unwrap();
        let v = |i: usize| f(&r[i]);
        let row = BeepRow {
            t_ms: v(0),
            dt_ms: v(1),
            bearing_deg: v(2),
            dist_m: v(3),
            aligned: r[4].as_i64().unwrap() != 0,
            pitch_norm: v(5),
            pitch_mult: v(6),
            hrtf_used: r[7].as_i64().unwrap() != 0,
            hrtf_az_deg: v(8),
            ir_ild_db: v(9),
            out_ild_db: v(10),
            rms_l: v(11),
            rms_r: v(12),
        };
        assert_eq!(row.csv_line(), c["line"].as_str().unwrap());
    }
}

#[test]
fn recorder_writes_header_and_rows_and_stops() {
    let dir = std::env::temp_dir().join(format!("beam_scanner_diag_{}", std::process::id()));
    let path = dir.join("scanner_hrtf_diag.csv");
    let mut diag = ScannerHrtfDiagnostic::with_enabled(Some(path.clone()), true);
    assert!(diag.enabled());
    let ir_l = vec![0.5f64, 0.25, 0.0];
    let ir_r = vec![0.25f64, 0.125, 0.0];
    let out_l = vec![0.1f32; 64];
    let out_r = vec![0.05f32; 64];
    diag.record_beep(
        -2.5,
        14.0,
        true,
        0.8,
        1.5,
        true,
        2.5,
        Some((&ir_l, &ir_r)),
        &out_l,
        &out_r,
    );
    diag.record_beep(1.0, 13.0, false, 0.7, 1.4, false, 0.0, None, &out_l, &[]);
    diag.stop_and_join();
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(format!("{}\n", lines[0]), CSV_HEADER);
    let cols: Vec<&str> = lines[1].split(',').collect();
    assert_eq!(cols.len(), 13);
    assert_eq!(cols[1], "0.000", "first dt is zero");
    assert_eq!(
        &cols[2..9],
        ["-2.5000", "14.000", "1", "0.8000", "1.5000", "1", "2.500"]
    );
    assert_eq!(cols[9], "6.021", "IR ILD: 20 log10(2)");
    assert_eq!(cols[10], "6.021", "output ILD");
    let cols2: Vec<&str> = lines[2].split(',').collect();
    assert_eq!(cols2[9], "0.000", "no IR, no IR ILD");
    assert_eq!(cols2[12], "0.000000", "empty right output has zero RMS");
    let _ = std::fs::remove_dir_all(&dir);

    let off = ScannerHrtfDiagnostic::with_enabled(Some(dir.join("never.csv")), false);
    off.record_beep(0.0, 0.0, false, 0.0, 0.0, false, 0.0, None, &[], &[]);
    off.stop();
    assert!(!dir.join("never.csv").exists());
}
