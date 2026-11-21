//! Port of `diagnostic/poi_guidance_sim.py`: the F9, Shift+W nearest-POI readout.
//!
//! The last two checks read `beamtel.py`, exactly as the Python sim does: they guard the
//! Python engine's key help and reply latches, and keep passing as long as that file is
//! the reference. The Rust engine's equivalents belong to its own tests.

use beam_reports::poi_guidance::{parse_poi_packet, poi_phrase};
use beam_testutil::repo_root;

#[test]
fn main() {
    let packet = r#"POI:{"name":"Hill Climb - Base","description":"Foot &amp; start<br/>Asphalt","distance":3.2,"bearing":91,"radius":0}"#;
    let poi = parse_poi_packet(packet).unwrap();
    let parked = poi_phrase(&poi, "imperial");
    assert_eq!(
        poi.description, "Foot & start. Asphalt",
        "HTML is converted to clean speech: {poi:?}"
    );
    assert!(
        parked.ends_with("Parked on it."),
        "inside the fallback footprint says parked: {parked}"
    );
    assert!(
        !parked.contains("feet") && !parked.contains("bearing"),
        "parked wording ends the announcement: {parked}"
    );

    let poi = parse_poi_packet(
        r#"POI:{"name":"Garage","description":"Repairs","distance":108,"bearing":-27.4,"radius":8}"#,
    )
    .unwrap();
    let imperial = poi_phrase(&poi, "imperial");
    let metric = poi_phrase(&poi, "metric");
    assert!(
        imperial.contains("328 feet"),
        "distance is measured to the POI footprint: {imperial}"
    );
    assert!(
        imperial.contains("bearing 27 degrees right"),
        "imperial wording includes a right bearing: {imperial}"
    );
    assert!(
        metric.contains("100 meters"),
        "metric wording uses metres: {metric}"
    );

    let ahead = poi_phrase(
        &parse_poi_packet(r#"POI:{"name":"Fuel","distance":18,"bearing":0.2,"radius":0}"#).unwrap(),
        "metric",
    );
    assert!(
        ahead.contains("bearing 0 degrees, straight ahead"),
        "near-zero bearing is unambiguous: {ahead}"
    );

    for malformed in [
        "SCAN,1",
        "POI:not-json",
        r#"POI:{"name":"Broken"}"#,
        r#"POI:{"distance":-1,"bearing":0}"#,
    ] {
        let got = parse_poi_packet(malformed);
        assert!(
            matches!(&got, Err(e) if e.kind == beam_reports::pyops::PyErrKind::ValueError),
            "malformed packet accepted: {malformed}: {got:?}"
        );
    }

    let engine = repo_root().join("rust/crates/beam-engine/src");
    let read = |rel: &str| {
        std::fs::read_to_string(engine.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
    };
    let help = read("f9_commands.rs");
    let scan = read("listeners/terrain_scan.rs");
    assert!(
        help.contains(r#"(("w", F, T, F), "Nearest point of interest")"#)
            && scan.contains(r#"send_scan_cmd("NEAREST_POI")"#),
        "F9 Shift+W is documented in input help and sends the POI request"
    );
    assert!(
        scan.contains("last_scan_reply_ts") && scan.contains("last_poi_reply_ts"),
        "scan and POI requests keep independent reply latches"
    );
}
