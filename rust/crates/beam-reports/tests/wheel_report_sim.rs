//! Port of `diagnostic/wheel_report_sim.py`: wheel_report's labels, and the cross-VM
//! chunk in wheelReport.lua.

use beam_reports::wheel_report as wr;
use beam_testutil::repo_root;

fn wheel(lon: f64, lat: f64) -> String {
    wheel_full(lon, lat, 0.265, "90", "60", "0")
}

fn wheel_w(lon: f64, lat: f64, width: f64) -> String {
    wheel_full(lon, lat, width, "90", "60", "0")
}

/// Python's `f"{lon},{lat},{width},..."`: floats print with `str()`.
fn wheel_full(lon: f64, lat: f64, width: f64, psi: &str, brake: &str, broken: &str) -> String {
    use beam_base::pyfmt::py_float_str as p;
    format!("{},{},{},{psi},{brake},{broken}", p(lon), p(lat), p(width))
}

fn packet(vehicles: &[(i64, i64, Vec<String>)]) -> String {
    let mut lines = vec!["WHEELS".to_string()];
    for (role, dual, wheels) in vehicles {
        lines.push(format!("V,{role},{},Vehicle {role}", 1000 + role));
        lines.push(format!("{dual}|{}", wheels.join(";")));
    }
    lines.join("\n")
}

fn labels(text: &str) -> Vec<String> {
    let vs = wr::parse_packet(text);
    wr::all_rows(vs.as_deref())
        .into_iter()
        .map(|r| r.label)
        .collect()
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn scenario_1_four_wheel_car_reads_as_fixed_slots() {
    let car = packet(&[(
        0,
        0,
        vec![
            wheel(-1.3, -0.8),
            wheel(1.4, 0.8),
            wheel(1.4, -0.8),
            wheel(-1.3, 0.8),
        ],
    )]);
    assert_eq!(
        labels(&car),
        strs(&["front left", "front right", "rear left", "rear right"]),
        "car labels match the fixed slots"
    );
}

#[test]
fn scenario_1b_off_centre_origin_and_trike() {
    let atv = "WHEELS\nV,0,39148,Scout\n0|0.6,-0.4,0.62,1.2,,0;0.6,1.7,0.62,1.2,,0;-1.6,-0.4,0.62,1.2,,0;-1.6,1.7,0.62,1.2,,0";
    assert_eq!(
        labels(atv),
        strs(&["front left", "front right", "rear left", "rear right"]),
        "off-centre origin still reads left and right"
    );
    let trike = packet(&[(
        0,
        0,
        vec![wheel(1.0, 1.3), wheel(1.0, 0.1), wheel(-1.0, 0.7)],
    )]);
    assert_eq!(
        labels(&trike),
        strs(&["front left", "front right", "rear"]),
        "trike's single rear wheel is centred"
    );
}

#[test]
fn scenario_2_percheron_three_axles_drive_axle_dual() {
    let perch = packet(&[(
        0,
        1,
        vec![
            wheel_w(-2.63, -0.82, 0.59),
            wheel_w(-2.63, 0.83, 0.59),
            wheel(-3.98, -0.99),
            wheel(-3.98, 0.99),
            wheel(1.97, -0.99),
            wheel(1.97, 0.99),
        ],
    )]);
    assert_eq!(
        labels(&perch),
        strs(&[
            "front left",
            "front right",
            "axle 2 left dual",
            "axle 2 right dual",
            "rear left",
            "rear right"
        ]),
        "Percheron: three axles, the drive axle dual"
    );
}

#[test]
fn scenario_3_wide_singles_without_the_switch_are_not_dual() {
    let wide = packet(&[(
        0,
        0,
        vec![
            wheel(1.5, 0.9),
            wheel(1.5, -0.9),
            wheel_w(-1.5, 0.9, 0.59),
            wheel_w(-1.5, -0.9, 0.59),
        ],
    )]);
    let got = labels(&wide);
    assert!(
        !got.join(" ").contains("dual"),
        "wide singles without the switch are not dual: {got:?}"
    );
}

#[test]
fn scenario_4_real_duals_inner_and_outer_left_to_right() {
    let real_duals = packet(&[(
        0,
        0,
        vec![
            wheel(2.0, 1.0),
            wheel(2.0, -1.0),
            wheel(-2.0, 0.73),
            wheel(-2.0, 1.08),
            wheel(-2.0, -0.73),
            wheel(-2.0, -1.08),
        ],
    )]);
    assert_eq!(
        labels(&real_duals),
        strs(&[
            "front left",
            "front right",
            "rear left outer",
            "rear left inner",
            "rear right inner",
            "rear right outer"
        ]),
        "real duals are inner and outer, left to right"
    );
}

#[test]
fn scenario_5_motorcycle_has_no_sides() {
    let bike = packet(&[(0, 0, vec![wheel(0.7, 0.0), wheel(-0.7, 0.01)])]);
    assert_eq!(
        labels(&bike),
        strs(&["front", "rear"]),
        "motorcycle reads front and rear"
    );
}

#[test]
fn scenario_6_trailers_unnumbered_alone_numbered_together() {
    let one = packet(&[
        (
            0,
            0,
            vec![
                wheel(1.4, 0.8),
                wheel(1.4, -0.8),
                wheel(-1.3, 0.8),
                wheel(-1.3, -0.8),
            ],
        ),
        (1, 0, vec![wheel(-0.2, 0.9), wheel(-0.2, -0.9)]),
    ]);
    let got = labels(&one);
    assert_eq!(
        got[4..],
        strs(&["trailer left", "trailer right"]),
        "single trailer is unnumbered"
    );
    let two = packet(&[
        (0, 0, vec![wheel(1.4, 0.8), wheel(1.4, -0.8)]),
        (2, 0, vec![wheel(0.0, 0.9), wheel(0.0, -0.9)]),
        (
            1,
            0,
            vec![
                wheel(3.0, 0.9),
                wheel(3.0, -0.9),
                wheel(-3.0, 0.9),
                wheel(-3.0, -0.9),
            ],
        ),
    ]);
    let got = labels(&two);
    assert_eq!(
        got[2..],
        strs(&[
            "trailer 1 front left",
            "trailer 1 front right",
            "trailer 1 rear left",
            "trailer 1 rear right",
            "trailer 2 left",
            "trailer 2 right",
        ]),
        "two trailers are numbered, in role order"
    );
}

#[test]
fn scenario_7_tandem_axles_stay_separate() {
    let tandem = packet(&[(
        0,
        0,
        vec![
            wheel(5.0, 1.0),
            wheel(5.0, -1.0),
            wheel(-1.0, 1.0),
            wheel(-1.0, -1.0),
            wheel(-2.3, 1.0),
            wheel(-2.3, -1.0),
        ],
    )]);
    let got = labels(&tandem);
    assert_eq!(
        got[2..],
        strs(&["axle 2 left", "axle 2 right", "rear left", "rear right"]),
        "tandem rear axles are separate"
    );
}

#[test]
fn scenario_8_unknown_is_none_and_zero_is_a_reading() {
    let text = packet(&[(
        0,
        0,
        vec![
            wheel_full(1.0, 0.8, 0.265, "", "", "0"),
            wheel_full(1.0, -0.8, 0.265, "0", "0", "0"),
        ],
    )]);
    let rows = wr::all_rows(wr::parse_packet(&text).as_deref());
    assert!(
        rows[0].psi.is_none() && rows[0].brake_c.is_none(),
        "empty fields are unknown: {:?}",
        rows[0]
    );
    assert!(
        rows[1].psi == Some(0.0) && rows[1].brake_c == Some(0.0),
        "zero is a reading: {:?}",
        rows[1]
    );
}

#[test]
fn scenario_9_broken_flag_malformed_wheel_foreign_and_empty_packets() {
    let text = packet(&[(
        0,
        0,
        vec![
            wheel_full(1.0, 0.8, 0.265, "90", "60", "1"),
            "junk,1".to_string(),
            wheel(1.0, -0.8),
        ],
    )]);
    let rows = wr::all_rows(wr::parse_packet(&text).as_deref());
    assert!(
        rows.len() == 2 && rows[0].broken && !rows[1].broken,
        "broken wheel flagged, junk skipped: {rows:?}"
    );
    assert!(
        wr::parse_packet("TRAILER:CLEAR").is_none(),
        "non-WHEELS packet is None"
    );
    assert_eq!(
        wr::parse_packet("WHEELS"),
        Some(vec![]),
        "bare WHEELS is no vehicles"
    );
    assert!(
        wr::all_rows(wr::parse_packet("WHEELS\nV,0,5,Prop\n0|").as_deref()).is_empty(),
        "vehicle with an empty payload has no rows"
    );
}

#[test]
fn scenario_10_cross_vm_chunk() {
    let path = repo_root()
        .join("bng_mod")
        .join("lua")
        .join("ge")
        .join("extensions")
        .join("wheelReport.lua");
    // Python reads in text mode, which turns CRLF into LF.
    let src = std::fs::read_to_string(&path)
        .expect("read wheelReport.lua")
        .replace("\r\n", "\n");
    let re = regex::Regex::new(r"(?s)local VEHICLE_CHUNK = \[\[\n(.*?)\n\]\]").unwrap();
    let m = re.captures(&src);
    assert!(m.is_some(), "VEHICLE_CHUNK found");
    let chunk = &m.unwrap()[1];
    assert!(!chunk.contains('%'), "chunk has no percent sign");
    assert!(!chunk.contains("]]"), "chunk has no doubled close bracket");
    assert!(
        chunk.contains("up:cross(fwd)"),
        "chunk builds lateral as up:cross(fwd)"
    );
}
