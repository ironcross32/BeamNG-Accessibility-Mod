//! Wheel rows for the status screen: every wheel's position label, pressure and brake
//! temperature.
//!
//! wheelReport.lua sends each vehicle's raw wheel list -- longitudinal and lateral
//! position in the vehicle's own frame, tire width, pressure, brake temperature -- and
//! this module decides what each wheel is called.
//!
//! Labels come from geometry, never from jbeam names, which differ per vehicle (the 0.40
//! Percheron's wheels are FL FR R2LL R2RR TLL TLR). A two-axle vehicle reads exactly as
//! the fixed-slot rows always did ("front left" ... "rear right"); more axles number the
//! middle ones ("front", "axle 2", "rear"); a coupled vehicle is prefixed "trailer"
//! ("trailer 2" when there is more than one).

use beam_base::pyfmt::py_strip;

use crate::pyops::{
    PyErr, int_text_to_i128, py_max_iter, py_min_iter, py_sort_by, py_str_to_float, py_str_to_int,
};

/// Wheels whose centres are closer than this along the vehicle are one axle. Duals share
/// a centre exactly; the closest real tandem axles are well over a metre apart.
pub const AXLE_GAP_M: f64 = 0.5;
/// Inside this of the centreline a wheel has no side (a motorcycle, a trike's nose wheel).
pub const CENTRE_LAT_M: f64 = 0.2;
/// With the jbeam's simplifiedDuallies switch on, a dual pair is ONE physics wheel about
/// twice as wide as a single (0.59 m against 0.265 on the Percheron), and only the width
/// says so.
pub const DUAL_WIDTH_RATIO: f64 = 1.6;

/// One wheel as the mod sent it. `psi` / `brake_c` are `None` when the game has no such
/// reading -- never zero, which is a real reading.
#[derive(Debug, Clone, PartialEq)]
pub struct Wheel {
    pub lon: f64,
    pub lat: f64,
    pub width: f64,
    pub psi: Option<f64>,
    pub brake_c: Option<f64>,
    pub broken: bool,
}

/// One vehicle of a WHEELS packet. Role 0 is the driven vehicle, 1.. the coupled ones.
#[derive(Debug, Clone, PartialEq)]
pub struct Vehicle {
    pub role: i128,
    pub id: i128,
    pub name: String,
    pub dual: bool,
    pub wheels: Vec<Wheel>,
}

/// One status-screen row.
#[derive(Debug, Clone, PartialEq)]
pub struct WheelRow {
    pub label: String,
    pub psi: Option<f64>,
    pub brake_c: Option<f64>,
    pub broken: bool,
}

fn float_or_none(text: &str) -> Result<Option<f64>, PyErr> {
    let text = py_strip(text);
    if text.is_empty() {
        return Ok(None);
    }
    py_str_to_float(text).map(Some)
}

/// `int(text)` for the role and id fields. Python's int is unbounded; a value outside
/// `i128` is treated like a malformed one (the vehicle is skipped).
fn int_field(text: &str) -> Option<i128> {
    py_str_to_int(text).ok().and_then(|t| int_text_to_i128(&t))
}

fn parse_wheel(fields: &[&str]) -> Result<Wheel, PyErr> {
    Ok(Wheel {
        lon: py_str_to_float(fields[0])?,
        lat: py_str_to_float(fields[1])?,
        width: py_str_to_float(fields[2])?,
        psi: float_or_none(fields[3])?,
        brake_c: float_or_none(fields[4])?,
        broken: py_strip(fields[5]) == "1",
    })
}

/// Parse a WHEELS packet into vehicles, or `None` if it is not one.
///
/// A malformed wheel is skipped rather than failing the vehicle.
pub fn parse_packet(text: &str) -> Option<Vec<Vehicle>> {
    let lines: Vec<&str> = py_strip(text).split('\n').collect();
    if lines.is_empty() || py_strip(lines[0]) != "WHEELS" {
        return None;
    }
    let mut vehicles = Vec::new();
    let mut i = 1;
    while i < lines.len() {
        let head = py_strip(lines[i]);
        i += 1;
        if !head.starts_with("V,") {
            continue;
        }
        let parts: Vec<&str> = head.splitn(4, ',').collect();
        if parts.len() < 3 {
            continue;
        }
        let (Some(role), Some(vid)) = (int_field(parts[1]), int_field(parts[2])) else {
            continue;
        };
        let name = if parts.len() >= 4 {
            py_strip(parts[3]).to_string()
        } else {
            String::new()
        };
        let mut body = if i < lines.len() {
            py_strip(lines[i])
        } else {
            ""
        };
        if body.starts_with("V,") {
            body = "";
        } else {
            i += 1;
        }
        let (dual_flag, wheel_text) = match body.split_once('|') {
            Some((a, b)) => (a, b),
            None => (body, ""),
        };
        let mut wheels = Vec::new();
        for item in wheel_text.split(';') {
            let fields: Vec<&str> = item.split(',').collect();
            if fields.len() < 6 {
                continue;
            }
            if let Ok(w) = parse_wheel(&fields) {
                wheels.push(w);
            }
        }
        vehicles.push(Vehicle {
            role,
            id: vid,
            name,
            dual: py_strip(dual_flag) == "1",
            wheels,
        });
    }
    Some(vehicles)
}

fn axle_names(count: usize) -> Vec<String> {
    if count <= 1 {
        return vec![String::new()];
    }
    if count == 2 {
        return vec!["front".into(), "rear".into()];
    }
    let mut names = vec!["front".to_string()];
    names.extend((2..count).map(|n| format!("axle {n}")));
    names.push("rear".into());
    names
}

/// Names for wheels sharing one axle end, innermost first.
fn stack_names(count: usize) -> Vec<String> {
    match count {
        1 => vec![String::new()],
        2 => vec!["inner".into(), "outer".into()],
        3 => vec!["inner".into(), "middle".into(), "outer".into()],
        _ => (1..=count).map(|n| n.to_string()).collect(),
    }
}

fn vehicle_prefix(role: i128, trailer_count: usize) -> String {
    if role == 0 {
        return String::new();
    }
    if trailer_count <= 1 {
        "trailer".to_string()
    } else {
        format!("trailer {role}")
    }
}

/// One row per wheel, front to back and left to right. Python's default
/// `trailer_count` is 1.
pub fn vehicle_rows(vehicle: &Vehicle, trailer_count: usize) -> Vec<WheelRow> {
    let mut wheels: Vec<Wheel> = vehicle.wheels.clone();
    py_sort_by(&mut wheels, |a, b| -a.lon < -b.lon);
    if wheels.is_empty() {
        return Vec::new();
    }
    // Positions are relative to the vehicle's origin, which need not be on its
    // centreline: the ATV's wheels arrive at lateral -0.4 and +1.7. Centre on the middle
    // of the wheel spread, which also keeps a trike's single wheel in the middle.
    let lats: Vec<f64> = wheels.iter().map(|w| w.lat).collect();
    let mid = (py_min_iter(&lats).unwrap_or(0.0) + py_max_iter(&lats).unwrap_or(0.0)) / 2.0;
    for w in wheels.iter_mut() {
        w.lat -= mid;
    }
    let mut axles: Vec<Vec<Wheel>> = vec![vec![wheels[0].clone()]];
    for w in &wheels[1..] {
        let last_lon = axles
            .last()
            .and_then(|a| a.last())
            .map(|x| x.lon)
            .unwrap_or(0.0);
        if last_lon - w.lon > AXLE_GAP_M {
            axles.push(vec![w.clone()]);
        } else if let Some(a) = axles.last_mut() {
            a.push(w.clone());
        }
    }

    let widths: Vec<f64> = wheels
        .iter()
        .map(|w| w.width)
        .filter(|&x| x > 0.0)
        .collect();
    let narrowest = py_min_iter(&widths).unwrap_or(0.0);
    let prefix = vehicle_prefix(vehicle.role, trailer_count);
    let names = axle_names(axles.len());

    let mut rows = Vec::new();
    for (axle_name, axle) in names.iter().zip(axles.iter()) {
        // Positive lateral is LEFT, so the left end reads first.
        let ends: [(&str, Vec<&Wheel>); 3] = [
            (
                "left",
                axle.iter().filter(|w| w.lat > CENTRE_LAT_M).collect(),
            ),
            (
                "",
                axle.iter()
                    .filter(|w| -CENTRE_LAT_M <= w.lat && w.lat <= CENTRE_LAT_M)
                    .collect(),
            ),
            (
                "right",
                axle.iter().filter(|w| w.lat < -CENTRE_LAT_M).collect(),
            ),
        ];
        for (side, mut group) in ends {
            if group.is_empty() {
                continue;
            }
            py_sort_by(&mut group, |a, b| a.lat.abs() < b.lat.abs());
            let stack = stack_names(group.len());
            // Read outward on the left and inward on the right: left to right across.
            let mut order: Vec<(&String, &Wheel)> =
                stack.iter().zip(group.iter().copied()).collect();
            if side == "left" {
                order.reverse();
            }
            for (stack_name, w) in order {
                let dual =
                    vehicle.dual && narrowest > 0.0 && w.width >= narrowest * DUAL_WIDTH_RATIO;
                let words = [
                    prefix.as_str(),
                    axle_name.as_str(),
                    side,
                    stack_name.as_str(),
                    if dual { "dual" } else { "" },
                ];
                let label: Vec<&str> = words.iter().copied().filter(|w| !w.is_empty()).collect();
                let label = if label.is_empty() {
                    "wheel".to_string()
                } else {
                    label.join(" ")
                };
                rows.push(WheelRow {
                    label,
                    psi: w.psi,
                    brake_c: w.brake_c,
                    broken: w.broken,
                });
            }
        }
    }
    rows
}

/// Rows for the driven vehicle, then each coupled vehicle in role order.
pub fn all_rows(vehicles: Option<&[Vehicle]>) -> Vec<WheelRow> {
    let mut vehicles: Vec<&Vehicle> = vehicles.unwrap_or(&[]).iter().collect();
    py_sort_by(&mut vehicles, |a, b| a.role < b.role);
    let trailers = vehicles.iter().filter(|v| v.role > 0).count();
    let mut rows = Vec::new();
    for v in vehicles {
        rows.extend(vehicle_rows(v, trailers));
    }
    rows
}
