//! Route beacon protocol, bearing and phrasing, from route_beacon.py.
//!
//! The UDP listener lives in the engine (`beam_engine::listeners::route_beacon`); this
//! module has no audio or speech dependencies so its parsing, bearing sign and phrasing
//! rules can be exercised without starting the application.

use crate::pyerr::ValueError;
use crate::pyfmt::{py_f, py_float_parse, py_mod, py_strip};

pub const ROUTE_PREFIX: &str = "ROUTE:";
pub const ROUTE_CLEAR: &str = "CLEAR";

/// Below this the beacon is on top of the destination and the bearing to it becomes
/// numerical residue -- the direction to a point you are standing on is not a fact.
pub const AT_DESTINATION_M: f64 = 15.0;

/// A parsed ROUTE datagram: Python's `{"dest": (x, y, z), "route_m": remaining}`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutePacket {
    pub dest: (f64, f64, f64),
    pub route_m: f64,
}

/// Parse one ROUTE datagram.
///
/// `Ok(None)` for `ROUTE:CLEAR` -- which is a *message*, not an absence: "no route is
/// set" and "the mod stopped talking" are different facts, and the engine must not have
/// to infer which from a timeout. `Err` on anything else that does not parse, so a
/// malformed packet is logged rather than silently becoming a destination somewhere
/// near the origin.
pub fn parse_route_packet(text: &str) -> Result<Option<RoutePacket>, ValueError> {
    let Some(rest) = text.strip_prefix(ROUTE_PREFIX) else {
        return Err(ValueError::new("not a ROUTE packet"));
    };
    let body = py_strip(rest);
    if body == ROUTE_CLEAR {
        return Ok(None);
    }

    let parts: Vec<&str> = body.split(',').collect();
    if parts.len() < 4 {
        return Err(ValueError::new(
            "ROUTE packet needs x, y, z and remaining distance",
        ));
    }
    let mut values = [0.0f64; 4];
    for (slot, part) in values.iter_mut().zip(&parts[..4]) {
        *slot =
            py_float_parse(part).ok_or_else(|| ValueError::new("ROUTE fields must be numbers"))?;
    }
    if !values.iter().all(|v| v.is_finite()) {
        return Err(ValueError::new("ROUTE fields must be finite"));
    }

    let [x, y, z, remaining] = values;
    Ok(Some(RoutePacket {
        dest: (x, y, z),
        // max(0.0, remaining): the first argument wins unless the second is greater.
        route_m: if remaining > 0.0 { remaining } else { 0.0 },
    }))
}

/// Absolute bearing to a destination, in BeamTel's own heading frame.
///
/// That frame is 180 degrees off a true compass bearing, which is why the `atan2`
/// arguments are negated. It is the *same* 180 that `yaw_to_heading_deg` puts into
/// every heading BeamTel handles, so the two cancel wherever one is subtracted from the
/// other -- see [`relative_bearing`]. Kept as one function so the sign cannot drift
/// between the places that need it.
pub fn target_bearing(pos_x: f64, pos_y: f64, dest_x: f64, dest_y: f64) -> f64 {
    let dx = dest_x - pos_x;
    let dy = dest_y - pos_y;
    py_mod((-dx).atan2(-dy).to_degrees(), 360.0)
}

/// Fold an angle onto -180..180, positive LEFT.
pub fn normalize_bearing(degrees: f64) -> f64 {
    let mut value = degrees;
    if value > 180.0 {
        value -= 360.0;
    } else if value < -180.0 {
        value += 360.0;
    }
    value
}

/// `(distance_m, signed_bearing_deg)` from a position to a destination.
///
/// **Positive bearing is the driver's LEFT**, the mod-wide convention: the marked-
/// waypoint readout and the route beacon both need exactly this, and a sign that
/// disagreed between them would be invisible in both. The audio's directional pulse
/// consumes the same convention, so the value can be handed to it unchanged.
///
/// Flattened -- z is ignored -- like every other bearing in this mod.
///
/// `heading_deg` is BeamTel's own heading, which is **180 degrees off a true compass
/// heading**, and the `atan2(-dx, -dy)` in [`target_bearing`] carries the matching
/// +180 so the two cancel in the subtraction. That is not a redundant pair of negations
/// to tidy away: correcting one without the other mirrors the answer front to back.
/// Facing north (heading 180 here), a destination due west answers +90 and one due
/// east answers -90.
pub fn relative_bearing(
    pos_x: f64,
    pos_y: f64,
    heading_deg: f64,
    dest_x: f64,
    dest_y: f64,
) -> (f64, f64) {
    let dx = dest_x - pos_x;
    let dy = dest_y - pos_y;
    let distance = (dx * dx + dy * dy).sqrt();
    let err = normalize_bearing(heading_deg - target_bearing(pos_x, pos_y, dest_x, dest_y));
    (distance, err)
}

/// Phrase a distance that may be kilometres rather than metres.
///
/// `road_guidance::format_road_distance` answers in feet or metres, which is right for
/// a junction a couple of hundred metres off and useless for a route: "12467 feet" is
/// not a distance anybody can hold. This switches to the larger unit once the smaller
/// one stops being readable.
pub fn format_long_distance(metres: f64, units: &str) -> String {
    // max(0.0, metres): NaN and -0.0 both give 0.0, as in Python.
    let value = if metres > 0.0 { metres } else { 0.0 };
    if units.to_lowercase().starts_with("imp") {
        let feet = value * 3.28084;
        if feet < 1000.0 {
            return format!("{} feet", py_f(feet, 0));
        }
        return format!("{} miles", py_f(feet / 5280.0, 1));
    }
    if value < 1000.0 {
        return format!("{} meters", py_f(value, 0));
    }
    format!("{} kilometers", py_f(value / 1000.0, 1))
}

/// The sentence spoken when the beacon is switched on.
///
/// Names the crow-flies distance *first* because that is what the beacon itself is
/// indicating; the route distance follows, and is worth saying because the gap between
/// the two is how far the roads are about to take you out of your way. Omitted when the
/// mod could not supply it, rather than spoken as zero.
///
/// `Err` where Python's `direction_label` raises: a non-finite bearing with the
/// destination more than [`AT_DESTINATION_M`] away.
pub fn route_beacon_phrase(
    distance_m: f64,
    bearing_deg: f64,
    route_m: f64,
    units: &str,
) -> Result<String, ValueError> {
    let head = if distance_m <= AT_DESTINATION_M {
        "Route beacon on, at the destination".to_string()
    } else {
        let mut where_ = direction_label(bearing_deg, true)?;
        if where_ == "straight" {
            where_ = "straight ahead".to_string();
        } else if where_ == "U-turn" {
            where_ = "behind you".to_string();
        }
        format!(
            "Route beacon on, destination {} {}",
            format_long_distance(distance_m, units),
            where_
        )
    };
    // `route_m and route_m > 0.0`: zero is falsy, NaN fails the comparison.
    if route_m > 0.0 {
        return Ok(format!(
            "{head}, {} by road",
            format_long_distance(route_m, units)
        ));
    }
    Ok(head)
}

/// `road_guidance.direction_label`, which route_beacon.py imports.
///
/// Repeated here because beam-base sits below beam-road in the crate graph; the
/// canonical port is `beam_road::road_guidance::direction_label`, and the fixture
/// `route_beacon.json` checks this copy against the Python original.
fn direction_label(bearing: f64, u_turn: bool) -> Result<String, ValueError> {
    if !bearing.is_finite() {
        return Err(ValueError::new("bearing must be finite"));
    }
    let bearing = py_mod(bearing + 180.0, 360.0) - 180.0;
    let magnitude = bearing.abs();
    if magnitude <= 15.0 {
        return Ok("straight".to_string());
    }
    let side = if bearing > 0.0 { "left" } else { "right" };
    if magnitude <= 45.0 {
        return Ok(format!("slight {side}"));
    }
    if magnitude <= 120.0 {
        return Ok(side.to_string());
    }
    if magnitude <= 150.0 {
        return Ok(format!("sharp {side}"));
    }
    Ok(if u_turn { "U-turn" } else { "behind" }.to_string())
}

#[doc(hidden)]
/// Test hook for the fixture that checks the private `direction_label` copy.
pub fn direction_label_for_tests(bearing: f64, u_turn: bool) -> Result<String, ValueError> {
    direction_label(bearing, u_turn)
}
