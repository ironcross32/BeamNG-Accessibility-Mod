//! diagnostic/obstacle_audio_sim.py, the listener half: the four
//! `beamtel.parse_obstacle_packet` checks, in the sim's order. The audio half is
//! `beam-audio/tests/obstacle_audio_sim.rs`.

use beam_engine::listeners::obstacle::{ObstaclePacket, parse_obstacle_packet};

fn static_of(text: &str) -> beam_engine::listeners::obstacle::StaticHazard {
    match parse_obstacle_packet(text) {
        Ok(ObstaclePacket::Static(h)) => h,
        other => panic!("{text}: expected a static hazard, got {other:?}"),
    }
}

#[test]
fn extended_packet_tail_must_be_accepted() {
    let parsed = static_of("1,1,12.5,220,3.4,2,5.0,0.68,1.71");
    assert!(
        parsed.state == 2 && !parsed.legacy,
        "extended packet tail must be accepted"
    );
}

#[test]
fn legacy_multi_obstacle_packet_must_select_its_highest_urgency() {
    let parsed = static_of("1,2,-30,80,8,15,240,4");
    assert!(
        parsed.bearing == 15.0 && parsed.state == 2 && parsed.legacy,
        "legacy multi-obstacle packet must select its highest urgency"
    );
}

#[test]
fn legacy_centre_distance_must_never_infer_emergency() {
    let parsed = static_of("1,1,0,255,0.1");
    assert_eq!(
        parsed.state, 2,
        "legacy centre distance must never infer emergency"
    );
}

#[test]
fn clear_packet_must_remain_compatible() {
    assert_eq!(
        parse_obstacle_packet("0"),
        Ok(ObstaclePacket::Clear),
        "clear packet must remain compatible"
    );
}
